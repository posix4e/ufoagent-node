//! Persistent WebSocket to the control plane's per-node Durable Object — the single
//! control-plane connection: receives pushed commands, executes them (refresh / repair), streams
//! results back, carries liveness (app-level pings), and learns the min_version floor from the
//! hello_ack. Runs on its own thread with reconnect/backoff.

use anyhow::{anyhow, Result};
use std::io::ErrorKind;
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tungstenite::client::IntoClientRequest;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

use crate::config::Config;
use crate::controlplane::{ControlPlane, USER_AGENT};
use crate::{daemon, repair, store, taskqueue};

/// Max time we wait for the tray to finish a run_task before reporting a timeout.
const RUN_TASK_TIMEOUT: Duration = Duration::from_secs(600);
/// Result strings are unbounded TEXT on the control plane; keep WS frames sane.
const RESULT_MAX: usize = 8192;

type Sock = WebSocket<MaybeTlsStream<TcpStream>>;

const PING_EVERY: Duration = Duration::from_secs(45);
const READ_TIMEOUT: Duration = Duration::from_secs(20);

/// Shared with the daemon thread: socket liveness (the tray's online state), the control plane's
/// minimum-version floor from the hello_ack (drives forced self-updates), and an outbound queue so
/// background run_task workers can push results/status without owning the socket (the read loop
/// drains and sends them — see `connect_and_serve`). The queue survives reconnects, so a result
/// produced while briefly disconnected is delivered on the next connection.
#[derive(Default)]
pub struct WsState {
    connected: AtomicBool,
    min_version: Mutex<Option<String>>,
    outbound: Mutex<Vec<Value>>,
}

impl WsState {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }
    pub fn min_version(&self) -> Option<String> {
        self.min_version.lock().ok().and_then(|g| g.clone())
    }
    /// Queue a message for the read loop to send on the live socket.
    pub fn queue_send(&self, v: Value) {
        if let Ok(mut q) = self.outbound.lock() {
            q.push(v);
        }
    }
    fn drain_outbound(&self) -> Vec<Value> {
        self.outbound
            .lock()
            .map(|mut q| std::mem::take(&mut *q))
            .unwrap_or_default()
    }
}

/// `https://app… → wss://app…/v1/connect` (and http → ws for local dev).
fn ws_url(control_plane: &str) -> String {
    let base = control_plane.trim_end_matches('/');
    let base = if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        base.to_string()
    };
    format!("{base}/v1/connect")
}

/// Run the WS client until `should_stop`, reconnecting with backoff. Meant to run on its own thread.
pub fn run(version: &str, state: Arc<WsState>, should_stop: impl Fn() -> bool) {
    let mut backoff = 2u64;
    let mut announced_unlinked = false;
    while !should_stop() {
        // Not linked yet is the normal state on a fresh install, not an error — say so once,
        // then quietly poll for the token instead of spamming the log on every backoff.
        if store::get_token().is_none() {
            if !announced_unlinked {
                log::info!("ws: waiting for this machine to be linked");
                announced_unlinked = true;
            }
            sleep_unless_stopped(30, &should_stop);
            continue;
        }
        announced_unlinked = false;
        match connect_and_serve(version, &state, &should_stop) {
            Ok(()) => backoff = 2,
            Err(e) => log::warn!("ws: {e}"),
        }
        state.connected.store(false, Ordering::Relaxed);
        sleep_unless_stopped(backoff, &should_stop);
        backoff = (backoff * 2).min(60);
    }
}

fn sleep_unless_stopped(secs: u64, should_stop: &impl Fn() -> bool) {
    let mut slept = 0;
    while slept < secs && !should_stop() {
        std::thread::sleep(Duration::from_secs(1));
        slept += 1;
    }
}

fn connect_and_serve(
    version: &str,
    state: &Arc<WsState>,
    should_stop: &impl Fn() -> bool,
) -> Result<()> {
    let token = store::get_token().ok_or_else(|| anyhow!("not linked; no token"))?;
    let cfg = Config::load();
    let url = ws_url(&cfg.control_plane_url());

    let mut req = url.as_str().into_client_request()?;
    req.headers_mut()
        .insert("authorization", format!("Bearer {token}").parse()?);
    req.headers_mut().insert("user-agent", USER_AGENT.parse()?);

    let (mut socket, _resp) = tungstenite::connect(req)?;
    set_read_timeout(&mut socket, READ_TIMEOUT);

    let environments = serde_json::to_value(crate::env::report_all()).unwrap_or(Value::Null);
    send(
        &mut socket,
        json!({ "type": "hello", "agent_version": version, "platform": daemon::platform(), "environments": environments }),
    )?;
    log::info!(
        "ws: connected to control plane; environments: {}",
        crate::env::summary()
    );
    state.connected.store(true, Ordering::Relaxed);

    let mut last_ping = Instant::now();
    while !should_stop() {
        match socket.read() {
            Ok(Message::Text(txt)) => {
                if let Err(e) = handle_message(&mut socket, state, &cfg, &txt) {
                    log::warn!("ws: handling message failed: {e}");
                }
            }
            Ok(Message::Close(_)) => return Ok(()),
            Ok(_) => {} // binary / protocol ping-pong handled by tungstenite
            Err(tungstenite::Error::Io(e))
                if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::TimedOut => {}
            Err(e) => return Err(e.into()),
        }
        // Flush anything background run_task workers queued (results / status updates).
        for msg in state.drain_outbound() {
            send(&mut socket, msg)?;
        }
        if last_ping.elapsed() >= PING_EVERY {
            send(&mut socket, json!({ "type": "ping" }))?;
            last_ping = Instant::now();
        }
    }
    let _ = socket.close(None);
    Ok(())
}

fn handle_message(socket: &mut Sock, state: &Arc<WsState>, cfg: &Config, txt: &str) -> Result<()> {
    let msg: Value = serde_json::from_str(txt)?;
    let kind = msg.get("type").and_then(Value::as_str);
    if kind == Some("hello_ack") {
        if let Some(v) = msg.get("min_version").and_then(Value::as_str) {
            if let Ok(mut g) = state.min_version.lock() {
                *g = Some(v.to_string());
            }
        }
        return Ok(());
    }
    if kind != Some("command") {
        return Ok(());
    }
    let id = msg
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let cmd = msg.get("kind").and_then(Value::as_str).unwrap_or("");
    log::info!("ws: command {cmd} ({id})");

    // run_task drives the GUI, so it runs in the interactive session (the tray), and can take
    // minutes — hand it to a worker so the read loop keeps pinging. The result/status come back
    // asynchronously via the outbound queue.
    if cmd == "run_task" {
        // Self-gate before queuing: the control plane already gated on its last-reported state, but
        // the node has ground truth (the env may have broken since). Same words as the server.
        let env_name = msg
            .get("args")
            .and_then(|a| a.get("env"))
            .and_then(Value::as_str)
            .unwrap_or(crate::env::UFO2)
            .to_string();
        if let Some(reason) = crate::env::gate(&env_name) {
            crate::cmdlog::record("remote", "run_task", None, "failed", Some(&reason));
            send(
                socket,
                json!({ "type": "result", "id": id, "status": "failed", "result": reason }),
            )?;
            return Ok(());
        }
        let task = msg
            .get("args")
            .and_then(|a| a.get("task"))
            .and_then(Value::as_str)
            .unwrap_or("adhoc")
            .to_string();
        let request = msg
            .get("args")
            .and_then(|a| a.get("request"))
            .and_then(Value::as_str)
            .map(str::to_string);
        spawn_run_task(state.clone(), id, task, request);
        return Ok(());
    }

    let (status, result) = execute(cfg, cmd);
    crate::cmdlog::record("remote", cmd, None, status, Some(&result));
    send(
        socket,
        json!({ "type": "result", "id": id, "status": status, "result": result }),
    )?;
    Ok(())
}

fn execute(cfg: &Config, kind: &str) -> (&'static str, String) {
    match kind {
        "refresh" => {
            let cp = ControlPlane::new(&cfg.control_plane_url(), store::get_token());
            match daemon::refresh_once(&cp, &cfg.ufo_home_path()) {
                Ok(c) => (
                    "done",
                    format!("credential refreshed (lease {})", c.lease_id),
                ),
                Err(e) => ("failed", e.to_string()),
            }
        }
        "repair" => match repair::repair() {
            Ok(lines) => ("done", lines.join("; ")),
            Err(e) => ("failed", e.to_string()),
        },
        other => ("failed", format!("unknown command kind: {other}")),
    }
}

/// Hand a run_task to the Session-1 tray and report the result asynchronously. The service (this
/// process) is in Session 0 with no desktop; the tray — already on the logged-in desktop — picks
/// the task off the file queue, runs UFO2, and writes back the result.
fn spawn_run_task(state: Arc<WsState>, id: String, task: String, request: Option<String>) {
    std::thread::spawn(move || {
        let label = request.clone().unwrap_or_else(|| task.clone());
        let req_log = request.clone();
        state.queue_send(json!({ "type": "status", "current_task": label }));

        let finish = |state: &WsState, status: &str, result: String| {
            // On-node history (the tray's `ufoagent run` is suppressed via UFOAGENT_FROM_QUEUE so
            // this remote entry is the single record of the task).
            crate::cmdlog::record(
                "remote",
                "run_task",
                req_log.as_deref(),
                status,
                Some(&result),
            );
            state.queue_send(json!({ "type": "result", "id": id, "status": status, "result": truncate(&result, RESULT_MAX) }));
            // Clear the "running" indicator on the dashboard.
            state.queue_send(json!({ "type": "status", "current_task": Value::Null }));
        };

        // No live desktop session means no tray to run UFO2 — fail fast and actionably.
        if !taskqueue::tray_alive(20) {
            finish(
                &state,
                "failed",
                "no interactive desktop (tray not running); run `ufoagent autologon` on this node"
                    .to_string(),
            );
            return;
        }

        let req = taskqueue::TaskRequest {
            id: id.clone(),
            task,
            request,
        };
        if let Err(e) = taskqueue::enqueue(&req) {
            finish(&state, "failed", format!("could not queue task: {e}"));
            return;
        }

        let deadline = Instant::now() + RUN_TASK_TIMEOUT;
        loop {
            if let Some(res) = taskqueue::take_result(&id) {
                finish(&state, &res.status, res.result);
                return;
            }
            if Instant::now() >= deadline {
                finish(
                    &state,
                    "failed",
                    "task timed out after 10 minutes".to_string(),
                );
                return;
            }
            std::thread::sleep(Duration::from_secs(2));
        }
    });
}

/// Truncate a result string to `max` bytes on a char boundary, marking it was cut.
fn truncate(s: &str, max: usize) -> String {
    let p = crate::util::prefix_on_char_boundary(s, max);
    if p.len() == s.len() {
        s.to_string()
    } else {
        format!("{p}… (truncated)")
    }
}

fn send(socket: &mut Sock, v: Value) -> Result<()> {
    socket.send(Message::Text(v.to_string()))?;
    Ok(())
}

/// Give reads a timeout so the loop can send periodic app-level pings (and notice `should_stop`).
fn set_read_timeout(socket: &mut Sock, dur: Duration) {
    match socket.get_mut() {
        MaybeTlsStream::Plain(s) => {
            let _ = s.set_read_timeout(Some(dur));
        }
        MaybeTlsStream::Rustls(s) => {
            let _ = s.get_ref().set_read_timeout(Some(dur));
        }
        _ => {}
    }
}
