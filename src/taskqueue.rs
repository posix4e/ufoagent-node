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

/// A queued task the tray should execute (`ufoagent run --task <task> -r <request>`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRequest {
    pub id: String,
    pub task: String,
    pub request: Option<String>,
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

// Used by the tray-side helpers (Windows) and the tests; absent on a non-test macOS/Linux build.
#[cfg(any(windows, test))]
fn now() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
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
    std::fs::write(alive_path(), now().to_string())?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // UFOAGENT_HOME is process-global, so the two tests must not race over it.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    // Point config_dir() at a temp dir for the duration of a test.
    fn with_temp<T>(f: impl FnOnce() -> T) -> T {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("ufoq-{}-{}", std::process::id(), now()));
        std::env::set_var("UFOAGENT_HOME", &dir);
        let out = f();
        std::env::remove_var("UFOAGENT_HOME");
        let _ = std::fs::remove_dir_all(&dir);
        out
    }

    #[test]
    fn enqueue_then_consume_roundtrip() {
        with_temp(|| {
            let req = TaskRequest {
                id: "cmd_test1".into(),
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
    fn alive_marker() {
        with_temp(|| {
            assert!(!tray_alive(15));
            touch_alive().unwrap();
            assert!(tray_alive(15));
        });
    }
}
