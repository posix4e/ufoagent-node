//! File-based task queue bridging the SYSTEM service (Session 0, holds the WebSocket) and the
//! in-session tray (Session 1, has the interactive desktop UFO2 needs).
//!
//! The service can't drive the GUI from Session 0, so a `run_task` command is written to `inbox/`
//! and the tray — already running on the logged-in desktop — picks it up, runs UFO2, and writes the
//! result to `outbox/`. Everything lives under the machine-wide config dir (ProgramData on Windows,
//! admin/SYSTEM-ACL'd), the same place `status.json` already lives. Plain JSON files + serde so it
//! compiles and unit-tests on any platform.

use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::config::config_dir;

/// A queued job the tray should execute in the interactive session. `kind` selects the action:
/// "run_task" (default — `ufoagent run --task <task> -r <request>`) or "screenshot" (capture the
/// desktop to `outbox/<id>.png`). `kind` defaults so older inbox files / callers still deserialize.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRequest {
    pub id: String,
    #[serde(default = "default_kind")]
    pub kind: String,
    pub task: String,
    pub request: Option<String>,
}

fn default_kind() -> String {
    "run_task".to_string()
}

/// What the tray reports back after running the task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskResult {
    pub id: String,
    pub status: String, // "done" | "failed"
    pub result: String,
}

fn tasks_dir() -> PathBuf {
    config_dir().join("tasks")
}
fn inbox_dir() -> PathBuf {
    tasks_dir().join("inbox")
}
fn outbox_dir() -> PathBuf {
    tasks_dir().join("outbox")
}
fn alive_path() -> PathBuf {
    tasks_dir().join("tray-alive")
}

// ---- service side (producer) ----

/// Enqueue a task for the tray to run. Writes `inbox/<id>.json`.
pub fn enqueue(req: &TaskRequest) -> Result<()> {
    let dir = inbox_dir();
    std::fs::create_dir_all(&dir)?;
    std::fs::write(
        dir.join(format!("{}.json", req.id)),
        serde_json::to_vec(req)?,
    )?;
    Ok(())
}

/// Read + remove `outbox/<id>.json` if the tray has finished the task; `None` if not yet done.
pub fn take_result(id: &str) -> Option<TaskResult> {
    let p = outbox_dir().join(format!("{id}.json"));
    let data = std::fs::read(&p).ok()?;
    let res: TaskResult = serde_json::from_slice(&data).ok()?;
    let _ = std::fs::remove_file(&p);
    Some(res)
}

/// Read + remove the screenshot PNG the tray captured for `id` (binary, so it rides alongside the
/// JSON result rather than inside it). The service uploads it to the control plane.
pub fn take_screenshot(id: &str) -> Option<Vec<u8>> {
    let p = outbox_dir().join(format!("{id}.png"));
    let data = std::fs::read(&p).ok()?;
    let _ = std::fs::remove_file(&p);
    Some(data)
}

/// True if the tray touched its liveness marker within `max_age_secs` — i.e. there's a live
/// interactive session that can actually run the task. Lets the service fail fast instead of
/// queuing into the void when nobody is logged in.
pub fn tray_alive(max_age_secs: u64) -> bool {
    let Ok(meta) = std::fs::metadata(alive_path()) else {
        return false;
    };
    let Ok(modified) = meta.modified() else {
        return false;
    };
    modified
        .elapsed()
        .map(|e| e.as_secs() <= max_age_secs)
        .unwrap_or(false)
}

// ---- tray side (consumer) — compiled on Windows (the tray) and in tests ----

/// Touch the liveness marker (the tray calls this each tick so the service knows it's running).
#[cfg(any(windows, test))]
pub fn touch_alive() -> Result<()> {
    let dir = tasks_dir();
    std::fs::create_dir_all(&dir)?;
    // Only the file's mtime matters to tray_alive(); the content is informational.
    std::fs::write(alive_path(), crate::util::now().to_string())?;
    Ok(())
}

/// Pop the next pending task (oldest first), removing its inbox file. `None` if the inbox is empty.
#[cfg(any(windows, test))]
pub fn next_pending() -> Option<TaskRequest> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(inbox_dir())
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().map(|x| x == "json").unwrap_or(false))
        .collect();
    entries.sort();
    for p in entries {
        if let Ok(data) = std::fs::read(&p) {
            if let Ok(req) = serde_json::from_slice::<TaskRequest>(&data) {
                let _ = std::fs::remove_file(&p);
                return Some(req);
            }
        }
        // Unparseable file — drop it so it doesn't wedge the queue.
        let _ = std::fs::remove_file(&p);
    }
    None
}

/// Write the task result for the service to pick up. Writes `outbox/<id>.json`.
#[cfg(any(windows, test))]
pub fn report(res: &TaskResult) -> Result<()> {
    let dir = outbox_dir();
    std::fs::create_dir_all(&dir)?;
    std::fs::write(
        dir.join(format!("{}.json", res.id)),
        serde_json::to_vec(res)?,
    )?;
    Ok(())
}

/// Write the captured screenshot PNG for the service to upload. Writes `outbox/<id>.png`.
#[cfg(any(windows, test))]
pub fn report_screenshot(id: &str, png: &[u8]) -> Result<()> {
    let dir = outbox_dir();
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join(format!("{id}.png")), png)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::with_temp_home;

    // UFOAGENT_HOME is process-global; all tests that redirect it serialize on one shared lock
    // (in config) so cmdlog's and taskqueue's tests can't race over the env var.
    fn with_temp<T>(f: impl FnOnce() -> T) -> T {
        with_temp_home("ufoq", f)
    }

    #[test]
    fn enqueue_then_consume_roundtrip() {
        with_temp(|| {
            let req = TaskRequest {
                id: "cmd_test1".into(),
                kind: "run_task".into(),
                task: "adhoc".into(),
                request: Some("open notepad".into()),
            };
            enqueue(&req).unwrap();
            // No result yet.
            assert!(take_result(&req.id).is_none());
            // Tray consumes it.
            let got = next_pending().expect("a pending task");
            assert_eq!(got.id, "cmd_test1");
            assert_eq!(got.request.as_deref(), Some("open notepad"));
            // Inbox is now empty.
            assert!(next_pending().is_none());
            // Tray reports, service takes.
            report(&TaskResult {
                id: req.id.clone(),
                status: "done".into(),
                result: "ok".into(),
            })
            .unwrap();
            let res = take_result(&req.id).expect("a result");
            assert_eq!(res.status, "done");
            // Taken results are consumed once.
            assert!(take_result(&req.id).is_none());
        });
    }

    #[test]
    fn screenshot_roundtrip() {
        with_temp(|| {
            // Nothing captured yet.
            assert!(take_screenshot("cmd_shot1").is_none());
            // Tray writes the PNG, service picks it up exactly once.
            let png = b"\x89PNG\r\n\x1a\n fake bytes";
            report_screenshot("cmd_shot1", png).unwrap();
            assert_eq!(take_screenshot("cmd_shot1").as_deref(), Some(&png[..]));
            assert!(take_screenshot("cmd_shot1").is_none());
        });
    }

    #[test]
    fn alive_marker() {
        with_temp(|| {
            assert!(!tray_alive(15));
            touch_alive().unwrap();
            assert!(tray_alive(15));
        });
    }
}
