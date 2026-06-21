//! Warm UFO2 worker owned by the login-session tray process.
//!
//! Windows uses one long-lived Python worker that imports UFO below the CLI layer and accepts JSONL
//! commands. Non-Windows builds keep stubs so ordinary cargo validation stays portable.

#[cfg(not(windows))]
use crate::runtime;

#[cfg(not(windows))]
pub fn prewarm() {}

#[cfg(not(windows))]
pub fn run_task(
    _req: &runtime::RemoteTaskRequest,
    _progress: runtime::ProgressCallback,
    _abort: runtime::AbortSignal,
) -> anyhow::Result<runtime::RemoteTaskResult> {
    anyhow::bail!("warm UFO worker is only available on Windows")
}

#[cfg(not(windows))]
pub fn abort_task(_cmd_id: &str) -> bool {
    false
}

#[cfg(not(windows))]
pub fn resume_task(_cmd_id: &str, _correction: &str) -> bool {
    false
}

#[cfg(windows)]
mod imp {
    use anyhow::{anyhow, bail, Context, Result};
    use serde::{Deserialize, Serialize};
    use std::collections::HashMap;
    use std::io::{BufRead, BufReader, Write};
    use std::net::{TcpListener, TcpStream};
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, Stdio};
    use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
    use std::sync::{Arc, Mutex, Once, OnceLock};
    use std::time::{Duration, Instant, UNIX_EPOCH};

    use crate::config::{config_dir, Config};
    use crate::controlplane::ControlPlane;
    use crate::{agent, env, runtime, status, store, ufo_config, util};

    const IDLE_TIMEOUT: Duration = Duration::from_secs(15 * 60);
    const ABORT_GRACE: Duration = Duration::from_secs(5);
    const RECV_POLL: Duration = Duration::from_millis(200);
    const WORKER_FIRST_EVENT_TIMEOUT: Duration = Duration::from_secs(15);
    const WORKER_FIRST_STEP_TIMEOUT: Duration = Duration::from_secs(120);
    const WORKER_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
    const MAX_TASKS_PER_WORKER: usize = 12;
    const MAX_WORKING_SET_BYTES: u64 = 1536 * 1024 * 1024;

    static MANAGER: OnceLock<Arc<Manager>> = OnceLock::new();
    static MONITOR: Once = Once::new();

    #[derive(Debug, Clone, Deserialize)]
    struct WorkerEvent {
        #[serde(rename = "type")]
        kind: String,
        #[serde(default)]
        cmd_id: Option<String>,
        #[serde(default)]
        message: Option<String>,
        #[serde(default)]
        status: Option<String>,
        #[serde(default)]
        result: Option<String>,
        #[serde(default)]
        ok: Option<bool>,
        #[serde(default)]
        error: Option<String>,
    }

    #[derive(Debug, Serialize)]
    #[serde(tag = "type")]
    enum WorkerCommand<'a> {
        #[serde(rename = "run")]
        Run {
            cmd_id: &'a str,
            task: &'a str,
            request: Option<&'a str>,
        },
        #[serde(rename = "resume")]
        Resume {
            cmd_id: &'a str,
            correction: &'a str,
        },
        #[serde(rename = "abort")]
        Abort { cmd_id: &'a str },
    }

    struct WorkerProc {
        child: Child,
        control: TcpStream,
        started_sig: String,
        tasks_completed: usize,
        last_used: Instant,
    }

    #[derive(Default)]
    struct Inner {
        proc: Option<WorkerProc>,
    }

    #[derive(Default)]
    struct Manager {
        inner: Mutex<Inner>,
        waiters: Mutex<HashMap<String, Sender<WorkerEvent>>>,
    }

    impl Manager {
        fn global() -> Arc<Self> {
            MANAGER.get_or_init(|| Arc::new(Self::default())).clone()
        }

        fn start_monitor(manager: Arc<Self>) {
            MONITOR.call_once(move || {
                std::thread::spawn(move || loop {
                    std::thread::sleep(Duration::from_secs(60));
                    manager.recycle_if_idle();
                });
            });
        }

        fn prewarm(self: &Arc<Self>) -> Result<()> {
            if env::gate(env::UFO2).is_some() {
                return Ok(());
            }
            prepare_ufo_home()?;
            let sig = config_signature(&Config::load().ufo_home_path());
            let mut inner = self
                .inner
                .lock()
                .map_err(|_| anyhow!("worker lock poisoned"))?;
            self.ensure_started_locked(&mut inner, sig)?;
            Ok(())
        }

        fn run_task(
            self: &Arc<Self>,
            req: &runtime::RemoteTaskRequest,
            progress: runtime::ProgressCallback,
            abort: runtime::AbortSignal,
        ) -> Result<runtime::RemoteTaskResult> {
            prepare_ufo_home()?;
            let cfg = Config::load();
            let home = cfg.ufo_home_path();
            let sig = config_signature(&home);
            let response_log = response_log_path(&home, &req.task);
            let response_log_baseline = response_log_line_count(&response_log);
            log::info!(
                "warm worker dispatch: cmd={} task={} response_log={} baseline_lines={}",
                req.id,
                req.task,
                response_log.display(),
                response_log_baseline
            );
            let rx = self.register_waiter(&req.id)?;
            let send_result = self.send_command(
                WorkerCommand::Run {
                    cmd_id: &req.id,
                    task: &req.task,
                    request: req.request.as_deref(),
                },
                sig,
            );
            if let Err(e) = send_result {
                self.unregister_waiter(&req.id);
                return Err(e);
            }

            let mut abort_sent = false;
            let mut abort_deadline: Option<Instant> = None;
            let started_at = Instant::now();
            let mut saw_worker_event = false;
            let mut saw_response_step = false;
            loop {
                if abort.load(std::sync::atomic::Ordering::Relaxed) && !abort_sent {
                    abort_sent = true;
                    abort_deadline = Some(Instant::now() + ABORT_GRACE);
                    let _ = self.abort_task(&req.id);
                }
                if abort_deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                    self.recycle("abort did not settle");
                    self.unregister_waiter(&req.id);
                    return Ok(runtime::RemoteTaskResult {
                        id: req.id.clone(),
                        status: "halted".into(),
                        result: "halted by overseer; warm worker was restarted".into(),
                    });
                }
                if !saw_response_step
                    && response_log_line_count(&response_log) > response_log_baseline
                {
                    saw_response_step = true;
                    log::info!(
                        "warm worker first UFO response.log step observed: cmd={} task={}",
                        req.id,
                        req.task
                    );
                }
                if !saw_worker_event && started_at.elapsed() >= WORKER_FIRST_EVENT_TIMEOUT {
                    self.recycle("worker accepted task but emitted no task event");
                    self.unregister_waiter(&req.id);
                    bail!(
                        "warm worker produced no task event within {}s after dispatch; recycled for cold fallback",
                        WORKER_FIRST_EVENT_TIMEOUT.as_secs()
                    );
                }
                if saw_worker_event
                    && !saw_response_step
                    && started_at.elapsed() >= WORKER_FIRST_STEP_TIMEOUT
                {
                    self.recycle("worker emitted diagnostics but no UFO response.log step");
                    self.unregister_waiter(&req.id);
                    bail!(
                        "warm worker produced no new {} line within {}s after dispatch; recycled for cold fallback",
                        response_log.display(),
                        WORKER_FIRST_STEP_TIMEOUT.as_secs()
                    );
                }

                match rx.recv_timeout(RECV_POLL) {
                    Ok(ev) => match ev.kind.as_str() {
                        "diag" => {
                            saw_worker_event = true;
                            log::info!(
                                "warm worker diag cmd={}: {}",
                                req.id,
                                ev.message.unwrap_or_else(|| "diagnostic".into())
                            );
                        }
                        "progress" => {
                            saw_worker_event = true;
                            if let Some(message) = ev.message {
                                log::info!("warm worker progress cmd={}: {}", req.id, message);
                                progress(message);
                            }
                        }
                        "resume_ack" => {
                            saw_worker_event = true;
                            if ev.ok == Some(false) {
                                log::warn!(
                                    "warm worker resume rejected for {}: {}",
                                    req.id,
                                    ev.error.unwrap_or_else(|| "unknown error".into())
                                );
                            }
                        }
                        "abort_ack" => {
                            saw_worker_event = true;
                            log::info!("warm worker abort_ack cmd={}", req.id);
                        }
                        "result" => {
                            log::info!(
                                "warm worker result cmd={} status={}",
                                req.id,
                                ev.status.clone().unwrap_or_else(|| "failed".into())
                            );
                            self.unregister_waiter(&req.id);
                            self.mark_task_done();
                            return Ok(runtime::RemoteTaskResult {
                                id: req.id.clone(),
                                status: ev.status.unwrap_or_else(|| "failed".into()),
                                result: ev.result.unwrap_or_default(),
                            });
                        }
                        other => {
                            saw_worker_event = true;
                            log::debug!("warm worker event for {}: {}", req.id, other);
                        }
                    },
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => {
                        self.unregister_waiter(&req.id);
                        return Err(anyhow!("warm worker event channel closed"));
                    }
                }
            }
        }

        fn resume_task(self: &Arc<Self>, cmd_id: &str, correction: &str) -> bool {
            if cmd_id.trim().is_empty() {
                return false;
            }
            match self.send_command_if_running(WorkerCommand::Resume { cmd_id, correction }) {
                Ok(()) => true,
                Err(e) => {
                    log::warn!("warm worker resume send failed for {cmd_id}: {e:#}");
                    false
                }
            }
        }

        fn abort_task(self: &Arc<Self>, cmd_id: &str) -> bool {
            if cmd_id.trim().is_empty() {
                return false;
            }
            match self.send_command_if_running(WorkerCommand::Abort { cmd_id }) {
                Ok(()) => true,
                Err(e) => {
                    log::warn!("warm worker abort send failed for {cmd_id}: {e:#}");
                    false
                }
            }
        }

        fn register_waiter(&self, cmd_id: &str) -> Result<Receiver<WorkerEvent>> {
            let (tx, rx) = mpsc::channel();
            let mut waiters = self
                .waiters
                .lock()
                .map_err(|_| anyhow!("worker waiter lock poisoned"))?;
            waiters.insert(cmd_id.to_string(), tx);
            Ok(rx)
        }

        fn unregister_waiter(&self, cmd_id: &str) {
            if let Ok(mut waiters) = self.waiters.lock() {
                waiters.remove(cmd_id);
            }
        }

        fn send_command(&self, command: WorkerCommand<'_>, sig: String) -> Result<()> {
            let mut inner = self
                .inner
                .lock()
                .map_err(|_| anyhow!("worker lock poisoned"))?;
            self.ensure_started_locked(&mut inner, sig)?;
            let proc = inner
                .proc
                .as_mut()
                .ok_or_else(|| anyhow!("warm worker is not running"))?;
            serde_json::to_writer(&mut proc.control, &command)?;
            proc.control.write_all(b"\n")?;
            proc.control.flush()?;
            proc.last_used = Instant::now();
            log::info!("warm worker command sent over tcp");
            Ok(())
        }

        fn send_command_if_running(&self, command: WorkerCommand<'_>) -> Result<()> {
            let mut inner = self
                .inner
                .lock()
                .map_err(|_| anyhow!("worker lock poisoned"))?;
            let Some(proc) = inner.proc.as_mut() else {
                bail!("warm worker is not running");
            };
            if let Some(status) = proc.child.try_wait()? {
                kill_proc(inner.proc.take());
                bail!("warm worker already exited with {status}");
            }
            serde_json::to_writer(&mut proc.control, &command)?;
            proc.control.write_all(b"\n")?;
            proc.control.flush()?;
            proc.last_used = Instant::now();
            log::info!("warm worker control command sent over tcp");
            Ok(())
        }

        fn ensure_started_locked(&self, inner: &mut Inner, sig: String) -> Result<()> {
            let mut recycle_reason = None;
            if let Some(proc) = inner.proc.as_mut() {
                if let Some(status) = proc.child.try_wait()? {
                    recycle_reason = Some(format!("worker exited with {status}"));
                } else if proc.tasks_completed >= MAX_TASKS_PER_WORKER {
                    recycle_reason = Some("max task count reached".to_string());
                } else if proc.started_sig != sig {
                    recycle_reason = Some("managed UFO config changed".to_string());
                } else if working_set_bytes(proc.child.id())
                    .is_some_and(|bytes| bytes > MAX_WORKING_SET_BYTES)
                {
                    recycle_reason = Some("worker memory threshold reached".to_string());
                }
            }
            if let Some(reason) = recycle_reason {
                log::info!("warm worker recycling: {reason}");
                kill_proc(inner.proc.take());
            }
            if inner.proc.is_none() {
                inner.proc = Some(spawn_worker(sig)?);
            }
            Ok(())
        }

        fn dispatch(&self, event: WorkerEvent) {
            if matches!(event.kind.as_str(), "ready" | "pong") {
                log::info!("warm worker event: {}", event.kind);
                return;
            }
            if event.kind == "fatal" || event.kind == "error" {
                log::warn!(
                    "warm worker {}: {}",
                    event.kind,
                    event
                        .error
                        .clone()
                        .unwrap_or_else(|| "unknown error".into())
                );
            }
            let Some(cmd_id) = event.cmd_id.clone() else {
                if let Some(message) = event.message {
                    log::info!("warm worker: {message}");
                }
                return;
            };
            let waiter = self
                .waiters
                .lock()
                .ok()
                .and_then(|waiters| waiters.get(&cmd_id).cloned());
            if let Some(waiter) = waiter {
                let _ = waiter.send(event);
            } else {
                log::info!(
                    "warm worker event without waiter for {cmd_id}: {}",
                    event.kind
                );
            }
        }

        fn mark_task_done(&self) {
            if let Ok(mut inner) = self.inner.lock() {
                if let Some(proc) = inner.proc.as_mut() {
                    proc.tasks_completed += 1;
                    proc.last_used = Instant::now();
                }
            }
        }

        fn recycle(&self, reason: &str) {
            log::info!("warm worker recycling: {reason}");
            if let Ok(mut inner) = self.inner.lock() {
                kill_proc(inner.proc.take());
            }
        }

        fn recycle_if_idle(&self) {
            if let Ok(mut inner) = self.inner.lock() {
                let idle = inner
                    .proc
                    .as_ref()
                    .map(|proc| proc.last_used.elapsed() >= IDLE_TIMEOUT)
                    .unwrap_or(false);
                if idle {
                    log::info!("warm worker recycling: idle timeout");
                    kill_proc(inner.proc.take());
                }
            }
        }
    }

    fn spawn_worker(sig: String) -> Result<WorkerProc> {
        let cfg = Config::load();
        let home = cfg.ufo_home_path();
        let python = cfg
            .python
            .clone()
            .map(PathBuf::from)
            .ok_or_else(|| anyhow!("UFO2 not provisioned; run `ufoagent bootstrap` first"))?;
        let script = ufo_config::install_worker(&home)?;
        let logs = home.join("logs");
        std::fs::create_dir_all(&logs)?;
        let stdio_log = logs.join("ufoagent_worker_stdio.log");
        let stdout = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&stdio_log)
            .with_context(|| format!("opening worker stdio log {}", stdio_log.display()))?;
        let stderr = stdout.try_clone()?;
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .with_context(|| "binding warm worker control socket")?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let mut cmd = Command::new(python);
        cmd.arg(&script)
            .current_dir(&home)
            .env("PYTHONUTF8", "1")
            .env("UFOAGENT_WORKER_HOST", "127.0.0.1")
            .env("UFOAGENT_WORKER_PORT", port.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr));
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
        let mut child = cmd.spawn().with_context(|| "spawning warm UFO worker")?;
        let control = accept_worker_control(&listener, &mut child)?;
        control.set_nodelay(true)?;
        let read_control = control.try_clone()?;
        let manager = Manager::global();
        std::thread::spawn(move || {
            for line in BufReader::new(read_control).lines().map_while(|l| l.ok()) {
                match serde_json::from_str::<WorkerEvent>(&line) {
                    Ok(event) => manager.dispatch(event),
                    Err(e) => log::warn!("warm worker emitted non-JSON control ({e}): {line}"),
                }
            }
            log::warn!("warm worker control socket closed");
        });
        log::info!(
            "warm worker spawned: pid={} script={} cwd={} control=127.0.0.1:{} stdio_log={}",
            child.id(),
            script.display(),
            home.display(),
            port,
            stdio_log.display()
        );
        Ok(WorkerProc {
            child,
            control,
            started_sig: sig,
            tasks_completed: 0,
            last_used: Instant::now(),
        })
    }

    fn accept_worker_control(listener: &TcpListener, child: &mut Child) -> Result<TcpStream> {
        let deadline = Instant::now() + WORKER_CONNECT_TIMEOUT;
        loop {
            match listener.accept() {
                Ok((stream, addr)) => {
                    log::info!("warm worker control connected from {addr}");
                    return Ok(stream);
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    if let Some(status) = child.try_wait()? {
                        bail!("warm worker exited before control connection: {status}");
                    }
                    if Instant::now() >= deadline {
                        let _ = child.kill();
                        let _ = child.wait();
                        bail!(
                            "warm worker did not connect control socket within {}s",
                            WORKER_CONNECT_TIMEOUT.as_secs()
                        );
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(e) => return Err(e).with_context(|| "accepting warm worker control socket"),
            }
        }
    }

    fn prepare_ufo_home() -> Result<()> {
        if let Some(reason) = env::gate(env::UFO2) {
            bail!("{reason}");
        }
        let cfg = Config::load();
        let home = cfg.ufo_home_path();
        refresh_credential_if_needed(&cfg, &home)?;
        ufo_config::apply_managed_defaults(&home)?;
        Ok(())
    }

    fn refresh_credential_if_needed(cfg: &Config, home: &Path) -> Result<()> {
        let agents = ufo_config::agents_yaml_path(home);
        let lease_expires_at = status::load().lease_expires_at;
        let needs_refresh = !agents.is_file() || lease_expires_at <= util::now() + 120;
        if !needs_refresh {
            return Ok(());
        }
        let cp = ControlPlane::new(&cfg.control_plane_url(), store::get_token());
        agent::refresh_once(&cp, home)?;
        Ok(())
    }

    fn config_signature(home: &Path) -> String {
        let paths = [
            ufo_config::agents_yaml_path(home),
            ufo_config::system_yaml_path(home),
            ufo_config::mcp_yaml_path(home),
            ufo_config::worker_path(home),
            config_dir().join("config.json"),
        ];
        paths
            .iter()
            .map(|path| file_sig(path))
            .collect::<Vec<_>>()
            .join("|")
    }

    fn file_sig(path: &Path) -> String {
        let Ok(meta) = std::fs::metadata(path) else {
            return format!("{}:missing", path.display());
        };
        let modified = meta
            .modified()
            .ok()
            .and_then(|m| m.duration_since(UNIX_EPOCH).ok())
            .unwrap_or_default();
        format!(
            "{}:{}:{}:{}",
            path.display(),
            meta.len(),
            modified.as_secs(),
            modified.subsec_nanos()
        )
    }

    fn response_log_path(home: &Path, task: &str) -> PathBuf {
        home.join("logs").join(task).join("response.log")
    }

    fn response_log_line_count(path: &Path) -> usize {
        std::fs::read_to_string(path)
            .map(|raw| raw.lines().count())
            .unwrap_or(0)
    }

    fn kill_proc(proc: Option<WorkerProc>) {
        if let Some(mut proc) = proc {
            let _ = proc.child.kill();
            let _ = proc.child.wait();
        }
    }

    fn working_set_bytes(pid: u32) -> Option<u64> {
        use windows::Win32::Foundation::{CloseHandle, HANDLE};
        use windows::Win32::System::ProcessStatus::{
            GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
        };
        use windows::Win32::System::Threading::{
            OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_READ,
        };
        unsafe {
            let handle: HANDLE = OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ,
                false,
                pid,
            )
            .ok()?;
            let mut counters = PROCESS_MEMORY_COUNTERS::default();
            let ok = GetProcessMemoryInfo(
                handle,
                &mut counters,
                std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
            )
            .is_ok();
            let _ = CloseHandle(handle);
            ok.then_some(counters.WorkingSetSize as u64)
        }
    }

    pub fn prewarm() {
        let manager = Manager::global();
        Manager::start_monitor(manager.clone());
        std::thread::spawn(move || {
            if let Err(e) = manager.prewarm() {
                log::info!("warm worker prewarm skipped: {e:#}");
            }
        });
    }

    pub fn run_task(
        req: &runtime::RemoteTaskRequest,
        progress: runtime::ProgressCallback,
        abort: runtime::AbortSignal,
    ) -> Result<runtime::RemoteTaskResult> {
        let manager = Manager::global();
        Manager::start_monitor(manager.clone());
        manager.run_task(req, progress, abort)
    }

    pub fn abort_task(cmd_id: &str) -> bool {
        Manager::global().abort_task(cmd_id)
    }

    pub fn resume_task(cmd_id: &str, correction: &str) -> bool {
        Manager::global().resume_task(cmd_id, correction)
    }
}

#[cfg(windows)]
pub use imp::{abort_task, prewarm, resume_task, run_task};
