//! Ensure the tray manager is running in the active interactive console session.
//!
//! Why this exists: on an unattended auto-logon node the installer's {commonstartup} shortcut does
//! NOT reliably launch the tray (the interactive shell's Startup-folder processing doesn't run the
//! way it does for a normal sign-in — observed on Azure Windows Server 2025 auto-logon). Without a
//! tray there's no interactive Session-1 worker, so `run_task` silently never executes. So the
//! always-running SYSTEM service takes responsibility: each tick it checks whether a live tray exists
//! (the tray-alive marker) and, if not, spawns `ufoagent tray` directly into the active console
//! session via CreateProcessAsUser. This is self-healing (re-spawns if the tray dies), needs no logon
//! event, and works for any logged-on user.

use std::path::Path;

/// Spawn `ufoagent tray` into the active console session if no live tray is present. No-op when
/// nobody is logged on or a tray is already alive. Best-effort: errors are logged, never fatal.
#[cfg(windows)]
pub fn ensure_tray_in_active_session(exe: &Path) {
    imp::ensure(exe)
}

#[cfg(not(windows))]
pub fn ensure_tray_in_active_session(_exe: &Path) {}

#[cfg(windows)]
mod imp {
    use std::ffi::{c_void, OsStr};
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::Foundation::{CloseHandle, FALSE, HANDLE};
    use windows::Win32::Security::{
        DuplicateTokenEx, SecurityIdentification, TokenPrimary, TOKEN_ALL_ACCESS,
    };
    use windows::Win32::System::Environment::{CreateEnvironmentBlock, DestroyEnvironmentBlock};
    use windows::Win32::System::RemoteDesktop::{WTSGetActiveConsoleSessionId, WTSQueryUserToken};
    use windows::Win32::System::Threading::{
        CreateProcessAsUserW, CREATE_NO_WINDOW, CREATE_UNICODE_ENVIRONMENT, PROCESS_INFORMATION,
        STARTUPINFOW,
    };

    fn wide(s: &str) -> Vec<u16> {
        OsStr::new(s)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    pub fn ensure(exe: &Path) {
        // A live tray already touched its marker recently — nothing to do.
        if crate::taskqueue::tray_alive(120) {
            return;
        }
        unsafe {
            // Which session owns the physical console? 0xFFFFFFFF ⇒ nobody is logged on.
            let session = WTSGetActiveConsoleSessionId();
            if session == 0xFFFF_FFFF {
                return;
            }
            // Token of the user logged into that session (SYSTEM-only; the service has SE_TCB).
            let mut user_token = HANDLE::default();
            if WTSQueryUserToken(session, &mut user_token).is_err() {
                return; // e.g. session present but no user token yet (still at logon screen)
            }
            // CreateProcessAsUser needs a *primary* token; WTSQueryUserToken hands back impersonation.
            let mut primary = HANDLE::default();
            let dup = DuplicateTokenEx(
                user_token,
                TOKEN_ALL_ACCESS,
                None,
                SecurityIdentification,
                TokenPrimary,
                &mut primary,
            );
            let _ = CloseHandle(user_token);
            if dup.is_err() {
                log::warn!("session: DuplicateTokenEx failed: {:?}", dup);
                return;
            }

            // The user's environment block, so the tray sees the same env a normal logon would.
            let mut env_block: *mut c_void = std::ptr::null_mut();
            let have_env = CreateEnvironmentBlock(&mut env_block, primary, FALSE).is_ok();

            // Launch on the interactive desktop. Console-subsystem exe → CREATE_NO_WINDOW so no
            // console flashes (the tray FreeConsole()s its own anyway).
            let mut cmd = wide(&format!("\"{}\" tray", exe.display()));
            let mut desktop = wide("winsta0\\default");
            let si = STARTUPINFOW {
                cb: std::mem::size_of::<STARTUPINFOW>() as u32,
                lpDesktop: PWSTR(desktop.as_mut_ptr()),
                ..Default::default()
            };
            let mut pi = PROCESS_INFORMATION::default();
            let res = CreateProcessAsUserW(
                primary,
                PCWSTR::null(),
                PWSTR(cmd.as_mut_ptr()),
                None,
                None,
                FALSE,
                CREATE_UNICODE_ENVIRONMENT | CREATE_NO_WINDOW,
                if have_env { Some(env_block) } else { None },
                PCWSTR::null(),
                &si,
                &mut pi,
            );
            match res {
                Ok(()) => {
                    let _ = CloseHandle(pi.hProcess);
                    let _ = CloseHandle(pi.hThread);
                    log::info!("session: launched tray into console session {session}");
                }
                Err(e) => log::warn!("session: CreateProcessAsUser(tray) failed: {e}"),
            }
            if have_env {
                let _ = DestroyEnvironmentBlock(env_block);
            }
            let _ = CloseHandle(primary);
        }
    }
}
