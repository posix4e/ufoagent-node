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
use crate::{agent, repair, runtime, store};

/// Max time we wait for the login-session executor to finish a run_task before reporting a timeout.
const RUN_TASK_TIMEOUT: Duration = Duration::from_secs(600);
/// Screenshots are a quick GDI grab + PNG encode in the login session.
const SCREENSHOT_TIMEOUT: Duration = Duration::from_secs(60);
/// Result strings are unbounded TEXT on the control plane; keep WS frames sane.
const RESULT_MAX: usize = 8192;
/// Live progress appears in the dashboard header; keep it compact.
const PROGRESS_MAX: usize = 240;
const PROGRESS_MIN_INTERVAL: Duration = Duration::from_secs(2);

type Sock = WebSocket<MaybeTlsStream<TcpStream>>;

const PING_EVERY: Duration = Duration::from_secs(45);
const READ_TIMEOUT: Duration = Duration::from_secs(20);

/// Shared with the login-agent loop: socket liveness, the control plane's
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
    /// Drop queued transient state frames. The `hello` sent on (re)connect carries the authoritative
    /// current env + desktop state, so deltas queued while disconnected are stale — draining them
    /// after the hello could clobber the server with an old state. Results/status frames are kept
    /// (those must still be delivered). Called right after hello.
    fn drop_transient_state_frames(&self) {
        if let Ok(mut q) = self.outbound.lock() {
            q.retain(|v| {
                !matches!(
                    v.get("type").and_then(Value::as_str),
                    Some("environments" | "desktop")
                )
            });
        }
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
    let desktop = serde_json::to_value(runtime::desktop_report(20)).unwrap_or(Value::Null);
    send(
        &mut socket,
        json!({ "type": "hello", "agent_version": version, "platform": agent::platform(), "environments": environments, "desktop": desktop }),
    )?;
    log::info!(
        "ws: connected to control plane (ufoagent {version}); environments: {}",
        crate::env::summary()
    );
    // The hello above carries the current env state; discard any stale env deltas queued while we
    // were disconnected so they can't drain afterward and overwrite it with an older state.
    state.drop_transient_state_frames();
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

    // run_task drives the GUI, so it runs in the login-session executor, and can take
    // minutes — hand it to a worker so the read loop keeps pinging. The result/status come back
    // asynchronously via the outbound queue.
    if cmd == "run_task" {
        // Self-gate before running: the control plane already gated on its last-reported state, but
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

    // A screenshot is captured in the login-session executor, like run_task, and the PNG is
    // uploaded out-of-band to the control plane; hand it to a worker so the read loop keeps pinging.
    if cmd == "screenshot" {
        spawn_screenshot(state.clone(), id, cfg.control_plane_url());
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
            match agent::refresh_once(&cp, &cfg.ufo_home_path()) {
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

/// Hand a run_task to the login-session executor and report the result asynchronously. The executor
/// runs UFO2 on the logged-in desktop and returns the result.
fn spawn_run_task(state: Arc<WsState>, id: String, task: String, request: Option<String>) {
    std::thread::spawn(move || {
        let label = request.clone().unwrap_or_else(|| task.clone());
        let req_log = request.clone();
        state.queue_send(
            json!({ "type": "status", "current_task": truncate(&label, PROGRESS_MAX) }),
        );

        let finish = |state: &WsState, status: &str, result: String| {
            // On-node history (the child `ufoagent run` is suppressed via UFOAGENT_REMOTE_TASK so
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

        // No live, usable desktop means the executor cannot drive UFO2 — fail fast and actionably.
        if let Some(reason) = runtime::gate_desktop(20) {
            finish(&state, "failed", reason);
            return;
        }

        let req = runtime::RemoteTaskRequest {
            id: id.clone(),
            task,
            request,
        };
        let progress = progress_callback(state.clone());
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(crate::tray::execute_remote(&req, progress));
        });
        match rx.recv_timeout(RUN_TASK_TIMEOUT) {
            Ok(res) => finish(&state, &res.status, res.result),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => finish(
                &state,
                "failed",
                "task timed out after 10 minutes".to_string(),
            ),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                finish(&state, "failed", "task worker stopped".to_string());
            }
        }
    });
}

fn progress_callback(state: Arc<WsState>) -> runtime::ProgressCallback {
    let last = Arc::new(Mutex::new((None::<Instant>, String::new())));
    Arc::new(move |line: String| {
        let Some(text) = progress_text(&line) else {
            return;
        };
        let mut send = false;
        if let Ok(mut prev) = last.lock() {
            let interval_ok = prev
                .0
                .map(|sent_at| sent_at.elapsed() >= PROGRESS_MIN_INTERVAL)
                .unwrap_or(true);
            if prev.1 != text && interval_ok {
                *prev = (Some(Instant::now()), text.clone());
                send = true;
            }
        }
        if send {
            state.queue_send(json!({ "type": "status", "current_task": text }));
        }
    })
}

fn progress_text(line: &str) -> Option<String> {
    let cleaned: String = line
        .chars()
        .filter(|c| *c == '\t' || !c.is_control())
        .collect();
    let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    if cleaned.is_empty() {
        None
    } else {
        Some(format!("UFO2: {}", truncate(&cleaned, PROGRESS_MAX)))
    }
}

/// Capture a desktop screenshot and upload the PNG to the control plane. Like run_task, the capture
/// happens in the login-session executor; this worker then PUTs it to the control plane and reports
/// the command result. The PNG rides out-of-band (binary upload), not inside the WS result.
fn spawn_screenshot(state: Arc<WsState>, id: String, control_plane: String) {
    std::thread::spawn(move || {
        let finish = |state: &WsState, status: &str, result: String| {
            crate::cmdlog::record("remote", "screenshot", None, status, Some(&result));
            state.queue_send(json!({ "type": "result", "id": id, "status": status, "result": truncate(&result, RESULT_MAX) }));
        };

        // No live, usable desktop means the executor cannot capture the screen — fail fast and actionably.
        if let Some(reason) = runtime::gate_desktop(20) {
            finish(&state, "failed", reason);
            return;
        }

        let shot_id = id.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(crate::tray::capture_remote_screenshot(&shot_id));
        });
        match rx.recv_timeout(SCREENSHOT_TIMEOUT) {
            Ok(Ok(png)) => match upload_screenshot(&control_plane, &id, &png) {
                Ok(()) => finish(
                    &state,
                    "done",
                    format!("screenshot captured ({} bytes)", png.len()),
                ),
                Err(e) => finish(&state, "failed", format!("screenshot upload failed: {e}")),
            },
            Ok(Err(e)) => finish(&state, "failed", e),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                finish(&state, "failed", "screenshot timed out".to_string())
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                finish(&state, "failed", "screenshot worker stopped".to_string())
            }
        }
    });
}

/// PUT the captured PNG to `/v1/agent/screenshot?cmd=<id>` (agent-token auth). The control plane
/// stashes it in R2 keyed by the command id; the dashboard fetches it back for the requesting owner.
fn upload_screenshot(control_plane: &str, id: &str, png: &[u8]) -> Result<()> {
    let token = store::get_token().ok_or_else(|| anyhow!("not linked; no token"))?;
    let base = control_plane.trim_end_matches('/');
    let url = format!("{base}/v1/agent/screenshot?cmd={id}");
    match ureq::put(&url)
        .set("authorization", &format!("Bearer {token}"))
        .set("user-agent", USER_AGENT)
        .set("content-type", "image/png")
        .send_bytes(png)
    {
        Ok(_) => Ok(()),
        Err(ureq::Error::Status(code, r)) => {
            let body = r.into_string().unwrap_or_default();
            Err(anyhow!(
                "control plane HTTP {code}: {}",
                body.chars().take(200).collect::<String>()
            ))
        }
        Err(e) => Err(anyhow!("request failed: {e}")),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drop_transient_state_frames_keeps_results_and_status() {
        let s = WsState::new();
        s.queue_send(json!({ "type": "environments", "environments": [] }));
        s.queue_send(
            json!({ "type": "desktop", "desktop": { "state": "ready", "updated_at": 1 } }),
        );
        s.queue_send(json!({ "type": "result", "id": "x", "status": "done" }));
        s.queue_send(json!({ "type": "status", "current_task": "y" }));
        s.queue_send(json!({ "type": "environments", "environments": [] }));
        s.drop_transient_state_frames();
        let left = s.drain_outbound();
        assert_eq!(
            left.len(),
            2,
            "environment and desktop frames should be dropped"
        );
        assert!(left.iter().all(|v| v["type"] != "environments"));
        assert!(left.iter().all(|v| v["type"] != "desktop"));
        assert_eq!(left[0]["type"], "result");
        assert_eq!(left[1]["type"], "status");
    }

    #[test]
    fn progress_text_is_dashboard_safe() {
        assert_eq!(
            progress_text("  Observing   Notepad window\r\n").as_deref(),
            Some("UFO2: Observing Notepad window")
        );
        assert!(progress_text("\x07\r\n").is_none());
    }
}
