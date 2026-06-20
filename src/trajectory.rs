//! Live UFO trajectory harvesting from `logs/<task>/response.log`.
//!
//! UFO's built-in trajectory parser treats each JSON line in response.log as a completed step and
//! loads screenshots from the same log directory. The node mirrors that shape while a remote task is
//! running: upload the best available frame, then send a compact decision event over the existing
//! WebSocket.

use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use crate::controlplane::USER_AGENT;
use crate::ws::WsState;
use crate::{config::Config, store};

const POLL_INTERVAL: Duration = Duration::from_secs(1);
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(30);
const DRAIN_POLLS_AFTER_STOP: usize = 2;
const SCREENSHOT_KEYS: [&str; 5] = [
    "annotated_screenshot_path",
    "selected_control_screenshot_path",
    "concat_screenshot_path",
    "clean_screenshot_path",
    "desktop_screenshot_path",
];

#[derive(Debug, Clone)]
struct ParsedStep {
    round: i64,
    step: i64,
    agent_type: Option<String>,
    status: Option<String>,
    observation: Option<String>,
    thought: Option<String>,
    plan: Option<String>,
    control_text: Option<String>,
    action: Value,
    action_representation: Value,
    function_call: Value,
    subtask: Option<String>,
    frame_path: Option<PathBuf>,
}

pub fn spawn_watcher(
    state: Arc<WsState>,
    control_plane: String,
    cmd_id: String,
    task: String,
    stop: Arc<AtomicBool>,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        if let Err(e) = watch_response_log(state, &control_plane, &cmd_id, &task, stop) {
            log::warn!("trajectory: watcher stopped: {e:#}");
        }
    })
}

fn watch_response_log(
    state: Arc<WsState>,
    control_plane: &str,
    cmd_id: &str,
    task: &str,
    stop: Arc<AtomicBool>,
) -> Result<()> {
    let ufo_home = Config::load().ufo_home_path();
    let log_dir = ufo_home.join("logs").join(task);
    let response_log = log_dir.join("response.log");
    let mut processed = read_lines(&response_log).map(|l| l.len()).unwrap_or(0);
    let mut seen_steps: HashSet<i64> = HashSet::new();
    let mut quiet_after_stop = 0usize;

    loop {
        let lines = read_lines(&response_log).unwrap_or_default();
        if lines.len() < processed {
            processed = 0;
            seen_steps.clear();
        }

        let mut saw_new = false;
        for (idx, line) in lines.iter().enumerate().skip(processed) {
            let Some(step) = parse_step_line(line, idx, &ufo_home, &log_dir) else {
                continue;
            };
            if !seen_steps.insert(step.step) {
                continue;
            }
            saw_new = true;
            let frame_key = match step.frame_path.as_ref() {
                Some(path) => match upload_frame(control_plane, cmd_id, step.step, path) {
                    Ok(key) => key,
                    Err(e) => {
                        log::warn!(
                            "trajectory: frame upload failed for command {cmd_id} step {}: {e:#}",
                            step.step
                        );
                        None
                    }
                },
                None => None,
            };
            state.queue_send(json!({
                "type": "trajectory_step",
                "cmd_id": cmd_id,
                "round": step.round,
                "step": step.step,
                "agent_type": step.agent_type,
                "status": step.status,
                "observation": step.observation,
                "thought": step.thought,
                "plan": step.plan,
                "control_text": step.control_text,
                "action": step.action,
                "action_representation": step.action_representation,
                "function_call": step.function_call,
                "subtask": step.subtask,
                "frame_key": frame_key,
            }));
        }
        processed = lines.len();

        if stop.load(Ordering::Relaxed) {
            if saw_new {
                quiet_after_stop = 0;
            } else {
                quiet_after_stop += 1;
                if quiet_after_stop >= DRAIN_POLLS_AFTER_STOP {
                    break;
                }
            }
        }
        std::thread::sleep(POLL_INTERVAL);
    }
    Ok(())
}

fn read_lines(path: &Path) -> Result<Vec<String>> {
    let raw = std::fs::read_to_string(path)?;
    Ok(raw.lines().map(str::to_string).collect())
}

fn parse_step_line(
    line: &str,
    line_index: usize,
    ufo_home: &Path,
    log_dir: &Path,
) -> Option<ParsedStep> {
    let v: Value = serde_json::from_str(line).ok()?;
    let step = int_field(&v, &["session_step", "round_step", "Step"]).unwrap_or(line_index as i64);
    let round = int_field(&v, &["round_num", "Round"]).unwrap_or(0);
    Some(ParsedStep {
        round,
        step,
        agent_type: string_field(&v, "agent_type"),
        status: string_field(&v, "status"),
        observation: string_field(&v, "result").or_else(|| string_field(&v, "results")),
        thought: string_field(&v, "thought"),
        plan: string_field(&v, "plan"),
        control_text: control_text(v.get("control_log")),
        action: v.get("action").cloned().unwrap_or(Value::Null),
        action_representation: v
            .get("action_representation")
            .cloned()
            .unwrap_or(Value::Null),
        function_call: v.get("function_call").cloned().unwrap_or(Value::Null),
        subtask: string_field(&v, "subtask"),
        frame_path: best_frame_path(&v, ufo_home, log_dir),
    })
}

fn int_field(v: &Value, keys: &[&str]) -> Option<i64> {
    keys.iter()
        .find_map(|key| v.get(*key).and_then(Value::as_i64))
        .filter(|n| *n >= 0)
}

fn string_field(v: &Value, key: &str) -> Option<String> {
    let s = v.get(key)?.as_str()?.trim();
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

fn control_text(raw: Option<&Value>) -> Option<String> {
    match raw? {
        Value::Object(m) => object_control_text(m),
        Value::Array(items) => items.iter().find_map(|v| match v {
            Value::Object(m) => object_control_text(m),
            _ => None,
        }),
        _ => None,
    }
}

fn object_control_text(m: &serde_json::Map<String, Value>) -> Option<String> {
    ["control_text", "target_name", "name"]
        .iter()
        .find_map(|key| m.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn best_frame_path(v: &Value, ufo_home: &Path, log_dir: &Path) -> Option<PathBuf> {
    for key in SCREENSHOT_KEYS {
        let Some(raw) = v.get(key).and_then(Value::as_str).map(str::trim) else {
            continue;
        };
        if raw.is_empty() {
            continue;
        }
        let p = PathBuf::from(raw);
        let candidates = if p.is_absolute() {
            vec![p]
        } else {
            let file_name = p.file_name().map(PathBuf::from);
            let mut out = vec![ufo_home.join(&p)];
            if let Some(name) = file_name {
                out.push(log_dir.join(name));
            }
            out
        };
        if let Some(existing) = candidates.into_iter().find(|p| p.is_file()) {
            return Some(existing);
        }
    }
    None
}

fn upload_frame(
    control_plane: &str,
    cmd_id: &str,
    step: i64,
    path: &Path,
) -> Result<Option<String>> {
    let token = store::get_token().ok_or_else(|| anyhow!("not linked; no token"))?;
    let png = std::fs::read(path)?;
    let base = control_plane.trim_end_matches('/');
    let url = format!("{base}/v1/agent/trajectory-frame?cmd={cmd_id}&step={step}");
    let agent = ureq::AgentBuilder::new().timeout(UPLOAD_TIMEOUT).build();
    match agent
        .put(&url)
        .set("authorization", &format!("Bearer {token}"))
        .set("user-agent", USER_AGENT)
        .set("content-type", "image/png")
        .send_bytes(&png)
    {
        Ok(resp) => {
            let body = resp.into_string().unwrap_or_default();
            let frame_key = serde_json::from_str::<Value>(&body).ok().and_then(|v| {
                v.get("frame_key")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            });
            Ok(frame_key)
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::with_temp_home;

    fn with_temp<T>(f: impl FnOnce(PathBuf) -> T) -> T {
        with_temp_home("ufotraj", || f(Config::load().ufo_home_path()))
    }

    #[test]
    fn parses_host_agent_step_text() {
        with_temp(|home| {
            let log_dir = home.join("logs").join("adhoc");
            let line = r#"{"status":"CONTINUE","round_step":0,"round_num":0,"action":[{"function":"run_shell","arguments":{"bash_command":"notepad"},"action_string":"run_shell(bash_command='notepad')"}],"function_call":"run_shell","action_representation":"[Action] run_shell(bash_command='notepad')","result":"Plan: launch Notepad","agent_type":"HostAgent","control_log":{}}"#;
            let s = parse_step_line(line, 9, &home, &log_dir).unwrap();
            assert_eq!(s.step, 0);
            assert_eq!(s.round, 0);
            assert_eq!(s.agent_type.as_deref(), Some("HostAgent"));
            assert_eq!(s.observation.as_deref(), Some("Plan: launch Notepad"));
            assert_eq!(s.function_call.as_str(), Some("run_shell"));
        });
    }

    #[test]
    fn resolves_app_agent_frame_and_control_text() {
        with_temp(|home| {
            let log_dir = home.join("logs").join("adhoc");
            std::fs::create_dir_all(&log_dir).unwrap();
            let frame = log_dir.join("action_step2_annotated.png");
            std::fs::write(&frame, b"png").unwrap();
            let line = r#"{"agent_type":"AppAgent","session_step":2,"round_step":2,"round_num":0,"status":"FINISH","subtask":"Type text","action_representation":["[Action] set_edit_text(...)"],"result":"Typed the text","annotated_screenshot_path":"logs/adhoc/action_step2_annotated.png","control_log":[{"kind":"control","name":"Text Editor","id":"1"}]}"#;
            let s = parse_step_line(line, 0, &home, &log_dir).unwrap();
            assert_eq!(s.step, 2);
            assert_eq!(s.control_text.as_deref(), Some("Text Editor"));
            assert_eq!(s.frame_path.as_deref(), Some(frame.as_path()));
            assert_eq!(s.subtask.as_deref(), Some("Type text"));
        });
    }
}
