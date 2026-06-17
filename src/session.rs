//! Ensure the tray manager is running in the active interactive console session.
//!
//! Why this exists: on an unattended auto-logon node the installer's {commonstartup} shortcut does
//! NOT reliably launch the tray (the interactive shell's Startup-folder processing doesn't run the
//! way it does for a normal sign-in — observed on Azure Windows Server 2025 auto-logon). Without a
//! tray there's no interactive Session-1 worker, so `run_task` silently never executes. So the
//! always-running SYSTEM service takes responsibility: each tick, if no live tray exists (the
//! tray-alive marker is stale), it launches `ufoagent tray` into the active console session.
//!
//! HOW it launches matters: a tray spawned cross-session via CreateProcessAsUser runs, but its
//! `Shell_NotifyIcon` (the visible 🛸 icon) fails with E_FAIL — so the menu never appears. Launching
//! the tray as a NORMAL in-session process makes the icon succeed (verified on a real VM, and it's
//! how a normal sign-in / the GitHub-runner test launch it). The portable way for a SYSTEM service to
//! do that is an INTERACTIVE scheduled task (`/IT`) run in the logged-on user's session. Self-healing
//! (re-runs if the tray dies), needs no logon event.

use std::path::Path;

/// Launch `ufoagent tray` in the active console session if no live tray is present. No-op when nobody
/// is logged on or a tray is already alive. Best-effort: errors are logged, never fatal.
#[cfg(windows)]
pub fn ensure_tray_in_active_session(exe: &Path) {
    imp::ensure(exe)
}

#[cfg(not(windows))]
pub fn ensure_tray_in_active_session(_exe: &Path) {}

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;
    use std::os::windows::process::CommandExt;
    use std::path::Path;
    use std::process::Command;

    use windows::core::PWSTR;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::RemoteDesktop::{
        WTSFreeMemory, WTSGetActiveConsoleSessionId, WTSQuerySessionInformationW, WTSUserName,
    };

    // Don't pop a console for the schtasks child (the service has none of its own).
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const TASK_NAME: &str = "UFOAgentTray";

    /// User name of the account logged into `session` (bare name; empty/None at the logon screen).
    fn console_username(session: u32) -> Option<String> {
        unsafe {
            let mut buf = PWSTR::null();
            let mut len = 0u32;
            // HANDLE::default() == WTS_CURRENT_SERVER_HANDLE (local).
            WTSQuerySessionInformationW(
                HANDLE::default(),
                session,
                WTSUserName,
                &mut buf,
                &mut len,
            )
            .ok()?;
            if buf.is_null() {
                return None;
            }
            let name = buf.to_string().ok();
            WTSFreeMemory(buf.0 as *mut c_void);
            let name = name?;
            (!name.is_empty()).then_some(name)
        }
    }

    pub fn ensure(exe: &Path) {
        // A live tray already touched its marker recently — nothing to do.
        if crate::taskqueue::tray_alive(120) {
            return;
        }
        // Which session owns the physical console? 0xFFFFFFFF ⇒ nobody is logged on.
        let session = unsafe { WTSGetActiveConsoleSessionId() };
        if session == 0xFFFF_FFFF {
            return;
        }
        let Some(user) = console_username(session) else {
            return; // session present but no user yet (still at the logon screen)
        };

        // Launch via a no-spaces launcher .cmd in ProgramData (Program Files has spaces, which schtasks
        // /TR can't quote reliably). `start` detaches the tray so the launcher console closes at once.
        let launcher = crate::config::config_dir().join("tray-launch.cmd");
        if let Err(e) = std::fs::write(
            &launcher,
            format!("start \"\" \"{}\" tray\r\n", exe.display()),
        ) {
            log::warn!("session: could not write tray launcher: {e}");
            return;
        }
        let launcher = launcher.to_string_lossy().into_owned();

        // (Idempotent) interactive task that runs the launcher in the user's session, then run it now.
        let _ = Command::new("schtasks")
            .args([
                "/Create", "/TN", TASK_NAME, "/TR", &launcher, "/SC", "ONCE", "/ST", "00:00",
                "/RU", &user, "/IT", "/RL", "HIGHEST", "/F",
            ])
            .creation_flags(CREATE_NO_WINDOW)
            .output();
        match Command::new("schtasks")
            .args(["/Run", "/TN", TASK_NAME])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
        {
            Ok(o) if o.status.success() => {
                log::info!(
                    "session: launched in-session tray for {user} (console session {session})"
                )
            }
            Ok(o) => log::warn!(
                "session: schtasks /Run failed: {}",
                String::from_utf8_lossy(&o.stderr).trim()
            ),
            Err(e) => log::warn!("session: schtasks /Run error: {e}"),
        }
    }
}
