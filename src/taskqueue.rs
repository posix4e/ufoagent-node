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

/// Whether the in-session worker has a desktop UFO2 can drive. This is intentionally separate from
/// WebSocket/node liveness: a service can be online while Windows has no usable GUI session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DesktopState {
    Ready,
    Unavailable,
}

/// Structured content of `tasks/tray-alive`. Older agents wrote only a timestamp; readers tolerate
/// that as a legacy "ready" marker when it is fresh, so rolling updates do not strand a live tray.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesktopReport {
    pub state: DesktopState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub updated_at: i64,
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
/// interactive worker process. This says the worker is alive, not that the desktop is usable; call
/// `gate_desktop` before queuing GUI work.
#[allow(dead_code)] // Windows service watchdog uses this; non-Windows clippy sees it as unused.
pub fn tray_alive(max_age_secs: u64) -> bool {
    marker_fresh(max_age_secs).is_some()
}

/// Current usable-desktop verdict from the tray marker. A missing/stale marker means the service may
/// be online but there is no confirmed interactive worker, so GUI commands should not be accepted.
pub fn desktop_report(max_age_secs: u64) -> DesktopReport {
    let stale = |detail: &str| DesktopReport {
        state: DesktopState::Unavailable,
        detail: Some(detail.to_string()),
        updated_at: 0,
    };
    let Some(path) = marker_fresh(max_age_secs) else {
        return stale("no fresh interactive desktop worker");
    };

    let Ok(raw) = std::fs::read_to_string(&path) else {
        return stale("desktop worker marker is unreadable");
    };
    if let Ok(mut report) = serde_json::from_str::<DesktopReport>(&raw) {
        if report.updated_at <= 0 {
            report.updated_at = crate::util::now();
        }
        return report;
    }

    // Legacy marker was just the timestamp as text. Treat a fresh legacy marker as ready because a
    // live older tray can still run tasks; the next tray restart will upgrade the marker format.
    DesktopReport {
        state: DesktopState::Ready,
        detail: Some("desktop worker alive (legacy marker)".to_string()),
        updated_at: raw.trim().parse().unwrap_or_else(|_| crate::util::now()),
    }
}

fn marker_fresh(max_age_secs: u64) -> Option<PathBuf> {
    let path = alive_path();
    let meta = std::fs::metadata(&path).ok()?;
    let modified = meta.modified().ok()?;
    modified
        .elapsed()
        .ok()
        .filter(|e| e.as_secs() <= max_age_secs)
        .map(|_| path)
}

/// Human-readable reason a GUI command should not run, or `None` when a usable desktop is present.
pub fn gate_desktop(max_age_secs: u64) -> Option<String> {
    let r = desktop_report(max_age_secs);
    match r.state {
        DesktopState::Ready => None,
        DesktopState::Unavailable => Some(match r.detail {
            Some(d) => format!(
                "desktop unavailable: {d}; sign in or run `ufoagent autologon` on this node"
            ),
            None => {
                "desktop unavailable; sign in or run `ufoagent autologon` on this node".to_string()
            }
        }),
    }
}

// ---- tray side (consumer) — compiled on Windows (the tray) and in tests ----

/// Touch the liveness marker (the tray calls this each tick so the service knows it's running).
#[cfg(any(windows, test))]
pub fn touch_alive() -> Result<()> {
    let dir = tasks_dir();
    std::fs::create_dir_all(&dir)?;
    let report = probe_desktop();
    std::fs::write(alive_path(), serde_json::to_vec(&report)?)?;
    Ok(())
}

#[cfg(all(windows, not(test)))]
fn probe_desktop() -> DesktopReport {
    imp::probe_desktop()
}

#[cfg(test)]
fn probe_desktop() -> DesktopReport {
    DesktopReport {
        state: DesktopState::Ready,
        detail: None,
        updated_at: crate::util::now(),
    }
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
            let r = desktop_report(15);
            assert_eq!(r.state, DesktopState::Ready);
            assert!(gate_desktop(15).is_none());
        });
    }

    #[test]
    fn stale_or_unavailable_desktop_gates_gui_work() {
        with_temp(|| {
            assert!(gate_desktop(15).unwrap().contains("desktop unavailable"));
            std::fs::create_dir_all(tasks_dir()).unwrap();
            std::fs::write(
                alive_path(),
                serde_json::to_vec(&DesktopReport {
                    state: DesktopState::Unavailable,
                    detail: Some("desktop is locked".into()),
                    updated_at: crate::util::now(),
                })
                .unwrap(),
            )
            .unwrap();
            let reason = gate_desktop(15).unwrap();
            assert!(reason.contains("desktop is locked"));
            assert!(tray_alive(15));
        });
    }
}

#[cfg(all(windows, not(test)))]
mod imp {
    use super::{DesktopReport, DesktopState};

    pub fn probe_desktop() -> DesktopReport {
        let updated_at = crate::util::now();
        let mut problems = Vec::new();

        if let Some(state) = session_connect_state() {
            if state != "active" {
                problems.push(format!("session is {state}"));
            }
        }

        match input_desktop_name() {
            Some(name) if name.eq_ignore_ascii_case("Default") => {}
            Some(name) => problems.push(format!("input desktop is {name}")),
            None => problems.push("input desktop is unavailable".to_string()),
        }

        if problems.is_empty() {
            DesktopReport {
                state: DesktopState::Ready,
                detail: None,
                updated_at,
            }
        } else {
            DesktopReport {
                state: DesktopState::Unavailable,
                detail: Some(problems.join("; ")),
                updated_at,
            }
        }
    }

    fn session_connect_state() -> Option<&'static str> {
        use std::ffi::c_void;
        use windows::core::PWSTR;
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::System::RemoteDesktop::{
            ProcessIdToSessionId, WTSActive, WTSConnectQuery, WTSConnectState, WTSConnected,
            WTSDisconnected, WTSDown, WTSFreeMemory, WTSIdle, WTSInit, WTSListen,
            WTSQuerySessionInformationW, WTSReset, WTSShadow, WTS_CONNECTSTATE_CLASS,
        };
        use windows::Win32::System::Threading::GetCurrentProcessId;

        let mut session = 0u32;
        unsafe {
            ProcessIdToSessionId(GetCurrentProcessId(), &mut session).ok()?;

            let mut buf = PWSTR::null();
            let mut len = 0u32;
            WTSQuerySessionInformationW(
                HANDLE::default(),
                session,
                WTSConnectState,
                &mut buf,
                &mut len,
            )
            .ok()?;
            if buf.is_null() {
                return None;
            }
            let state = *(buf.0 as *const WTS_CONNECTSTATE_CLASS);
            WTSFreeMemory(buf.0 as *mut c_void);
            Some(if state == WTSActive {
                "active"
            } else if state == WTSConnected {
                "connected"
            } else if state == WTSConnectQuery {
                "connect-query"
            } else if state == WTSShadow {
                "shadow"
            } else if state == WTSDisconnected {
                "disconnected"
            } else if state == WTSIdle {
                "idle"
            } else if state == WTSListen {
                "listening"
            } else if state == WTSReset {
                "resetting"
            } else if state == WTSDown {
                "down"
            } else if state == WTSInit {
                "initializing"
            } else {
                "unknown"
            })
        }
    }

    fn input_desktop_name() -> Option<String> {
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::System::StationsAndDesktops::{
            CloseDesktop, GetUserObjectInformationW, OpenInputDesktop, DESKTOP_READOBJECTS,
            UOI_NAME,
        };

        unsafe {
            let desktop = OpenInputDesktop(Default::default(), false, DESKTOP_READOBJECTS).ok()?;
            let desktop_handle = HANDLE(desktop.0);

            let mut needed = 0u32;
            let _ = GetUserObjectInformationW(desktop_handle, UOI_NAME, None, 0, Some(&mut needed));
            if needed == 0 {
                let _ = CloseDesktop(desktop);
                return None;
            }
            let mut buf = vec![0u16; (needed as usize).div_ceil(2)];
            let read_name = GetUserObjectInformationW(
                desktop_handle,
                UOI_NAME,
                Some(buf.as_mut_ptr() as *mut _),
                needed,
                Some(&mut needed),
            );
            let _ = CloseDesktop(desktop);
            read_name.ok()?;
            if let Some(pos) = buf.iter().position(|&c| c == 0) {
                buf.truncate(pos);
            }
            String::from_utf16(&buf).ok()
        }
    }
}
