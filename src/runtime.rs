//! Remote task types plus desktop-availability reporting for the login-session agent.
//!
//! GUI work runs directly from the logged-in desktop process. The liveness marker under the
//! machine-wide config dir lets the connection loop report whether that desktop is usable.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::config::config_dir;

/// Live text emitted by UFO2 while a remote task is running. The WebSocket layer maps this onto the
/// dashboard's `current_task` field.
pub type ProgressCallback = Arc<dyn Fn(String) + Send + Sync + 'static>;
pub type AbortSignal = Arc<AtomicBool>;

/// A remote job the login-session agent should execute.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteTaskRequest {
    pub id: String,
    pub task: String,
    pub request: Option<String>,
}

/// What the login-session agent reports back after running the task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteTaskResult {
    pub id: String,
    pub status: String, // "done" | "failed" | "halted"
    pub result: String,
}

/// Whether the login-session worker has a desktop UFO2 can drive. This is intentionally separate
/// from WebSocket/node liveness: the agent can be online while Windows has no usable GUI desktop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DesktopState {
    Ready,
    Unavailable,
}

/// Structured content of `tasks/tray-alive`.
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
fn alive_path() -> PathBuf {
    tasks_dir().join("tray-alive")
}

/// Current usable-desktop verdict from the tray marker. A missing/stale marker means there is no
/// confirmed interactive worker, so GUI commands should not be accepted.
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

    stale("desktop worker marker is unreadable")
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

/// Touch the liveness marker (the tray calls this each tick so desktop availability is current).
#[cfg(any(windows, test))]
pub fn touch_alive() -> anyhow::Result<()> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::with_temp_home;

    // UFOAGENT_HOME is process-global; all tests that redirect it serialize on one shared lock
    // (in config) so cmdlog's and runtime's tests can't race over the env var.
    fn with_temp<T>(f: impl FnOnce() -> T) -> T {
        with_temp_home("ufoq", f)
    }

    #[test]
    fn alive_marker() {
        with_temp(|| {
            assert_eq!(desktop_report(15).state, DesktopState::Unavailable);
            touch_alive().unwrap();
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
