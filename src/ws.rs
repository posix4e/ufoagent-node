//! Persistent WebSocket to the control plane's per-node Durable Object: receive pushed commands,
//! execute them (refresh / repair), and stream results + liveness pings back. Runs on its own
//! thread with reconnect/backoff. The HTTP heartbeat loop stays as a fallback.

use anyhow::{anyhow, Result};
use std::io::ErrorKind;
use std::net::TcpStream;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tungstenite::client::IntoClientRequest;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

use crate::config::Config;
use crate::controlplane::{ControlPlane, USER_AGENT};
use crate::{daemon, repair, store};

type Sock = WebSocket<MaybeTlsStream<TcpStream>>;

const PING_EVERY: Duration = Duration::from_secs(45);
const READ_TIMEOUT: Duration = Duration::from_secs(20);

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
pub fn run(version: &str, should_stop: impl Fn() -> bool) {
    let mut backoff = 2u64;
    while !should_stop() {
        match connect_and_serve(version, &should_stop) {
            Ok(()) => backoff = 2,
            Err(e) => log::warn!("ws: {e}"),
        }
        let mut slept = 0;
        while slept < backoff && !should_stop() {
            std::thread::sleep(Duration::from_secs(1));
            slept += 1;
        }
        backoff = (backoff * 2).min(60);
    }
}

fn connect_and_serve(version: &str, should_stop: &impl Fn() -> bool) -> Result<()> {
    let token = store::get_token().ok_or_else(|| anyhow!("not linked; no token"))?;
    let cfg = Config::load();
    let url = ws_url(&cfg.control_plane_url());

    let mut req = url.as_str().into_client_request()?;
    req.headers_mut()
        .insert("authorization", format!("Bearer {token}").parse()?);
    req.headers_mut().insert("user-agent", USER_AGENT.parse()?);

    let (mut socket, _resp) = tungstenite::connect(req)?;
    set_read_timeout(&mut socket, READ_TIMEOUT);

    send(
        &mut socket,
        json!({ "type": "hello", "agent_version": version, "platform": daemon::platform() }),
    )?;
    log::info!("ws: connected to control plane");

    let mut last_ping = Instant::now();
    while !should_stop() {
        match socket.read() {
            Ok(Message::Text(txt)) => {
                if let Err(e) = handle_message(&mut socket, &cfg, &txt) {
                    log::warn!("ws: handling message failed: {e}");
                }
            }
            Ok(Message::Close(_)) => return Ok(()),
            Ok(_) => {} // binary / protocol ping-pong handled by tungstenite
            Err(tungstenite::Error::Io(e))
                if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::TimedOut => {}
            Err(e) => return Err(e.into()),
        }
        if last_ping.elapsed() >= PING_EVERY {
            send(&mut socket, json!({ "type": "ping" }))?;
            last_ping = Instant::now();
        }
    }
    let _ = socket.close(None);
    Ok(())
}

fn handle_message(socket: &mut Sock, cfg: &Config, txt: &str) -> Result<()> {
    let msg: Value = serde_json::from_str(txt)?;
    if msg.get("type").and_then(Value::as_str) != Some("command") {
        return Ok(());
    }
    let id = msg.get("id").and_then(Value::as_str).unwrap_or("").to_string();
    let kind = msg.get("kind").and_then(Value::as_str).unwrap_or("");
    log::info!("ws: command {kind} ({id})");

    let (status, result) = execute(cfg, kind);
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
                Ok(c) => ("done", format!("credential refreshed (lease {})", c.lease_id)),
                Err(e) => ("failed", e.to_string()),
            }
        }
        "repair" => match repair::repair() {
            Ok(lines) => ("done", lines.join("; ")),
            Err(e) => ("failed", e.to_string()),
        },
        "run_task" => (
            "failed",
            "run_task not supported yet (needs the interactive-desktop bridge)".to_string(),
        ),
        other => ("failed", format!("unknown command kind: {other}")),
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
