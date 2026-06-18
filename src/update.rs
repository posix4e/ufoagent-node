//! Update check + login-agent self-update.
//!
//! Flow: compare against the latest GitHub release (and the control plane's `min_version`
//! floor) → download the signed `ufoagent-setup.exe` → **verify its Authenticode signature**
//! → launch it silently (it stops the login agent, swaps files, restarts). The signature check is
//! the safety gate: an unsigned/tampered installer is refused, so this is safe to ship before
//! code-signing is live (it simply won't self-install until signed builds exist).

use anyhow::{anyhow, Result};
use std::path::Path;

use crate::config::Config;
use crate::controlplane::ControlPlane;

const SETUP_ASSET: &str = "ufoagent-setup.exe";

pub fn version_tuple(s: &str) -> Vec<u32> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for ch in s.chars() {
        if ch.is_ascii_digit() {
            cur.push(ch);
        } else if !cur.is_empty() {
            out.push(cur.parse().unwrap_or(0));
            cur.clear();
        }
    }
    if !cur.is_empty() {
        out.push(cur.parse().unwrap_or(0));
    }
    out.truncate(3);
    if out.is_empty() {
        out.push(0);
    }
    out
}

pub fn needs_update(current: &str, minimum: &str) -> bool {
    version_tuple(current) < version_tuple(minimum)
}

pub struct UpdateStatus {
    pub current: String,
    pub min_version: String,
    pub update_required: bool,
}

pub fn check(cp: &ControlPlane, current: &str) -> Result<UpdateStatus> {
    let v = cp.version()?;
    let min = v
        .get("min_version")
        .and_then(|x| x.as_str())
        .unwrap_or("0.0.0")
        .to_string();
    Ok(UpdateStatus {
        update_required: needs_update(current, &min),
        current: current.to_string(),
        min_version: min,
    })
}

/// Latest release tag (e.g. "v0.3.0") from the GitHub Releases API.
pub fn latest_release_tag(repo: &str) -> Result<String> {
    let url = format!("https://api.github.com/repos/{repo}/releases/latest");
    let resp = ureq::get(&url)
        .set("user-agent", crate::controlplane::USER_AGENT)
        .set("accept", "application/vnd.github+json")
        .call()
        .map_err(|e| anyhow!("GitHub release check failed: {e}"))?;
    let v: serde_json::Value = resp.into_json()?;
    v.get("tag_name")
        .and_then(|t| t.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow!("no tag_name in GitHub release response"))
}

/// Stream the latest signed installer to `dest`.
pub fn download_installer(repo: &str, dest: &Path) -> Result<()> {
    let url = format!("https://github.com/{repo}/releases/latest/download/{SETUP_ASSET}");
    let resp = ureq::get(&url)
        .set("user-agent", crate::controlplane::USER_AGENT)
        .call()
        .map_err(|e| anyhow!("installer download failed: {e}"))?;
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut reader = resp.into_reader();
    let mut file = std::fs::File::create(dest)?;
    std::io::copy(&mut reader, &mut file)?;
    Ok(())
}

/// Verify the file carries a valid Authenticode signature chaining to a trusted root.
/// `Err` on anything unsigned/broken — this is the gate that keeps us from running an
/// untrusted installer as SYSTEM.
#[cfg(windows)]
pub fn verify_authenticode(path: &Path) -> Result<()> {
    win::verify_authenticode(path)
}
#[cfg(not(windows))]
pub fn verify_authenticode(_path: &Path) -> Result<()> {
    anyhow::bail!("Authenticode verification is only available on Windows")
}

/// Launch the verified installer silently and detached, so it survives the login agent being
/// stopped mid-update. The installer stops the agent, replaces files, and restarts it.
#[cfg(windows)]
pub fn apply_update(installer: &Path) -> Result<()> {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    std::process::Command::new(installer)
        .args(["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART"])
        .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
        .spawn()?;
    Ok(())
}
#[cfg(not(windows))]
pub fn apply_update(_installer: &Path) -> Result<()> {
    anyhow::bail!("self-update install is only available on Windows")
}

/// Decide whether to update and, if so, do it. Returns Ok(true) when an installer was launched.
/// `min_version` is the control plane's hard floor (from heartbeat / the version endpoint).
pub fn maybe_self_update(cfg: &Config, current: &str, min_version: Option<&str>) -> Result<bool> {
    if !cfg.auto_update_enabled() {
        return Ok(false);
    }
    let repo = cfg.update_repo();

    let latest = match latest_release_tag(&repo) {
        Ok(tag) => tag,
        Err(e) => {
            log::warn!("update check failed: {e}");
            String::new()
        }
    };
    let forced = min_version
        .map(|m| needs_update(current, m))
        .unwrap_or(false);
    let newer = !latest.is_empty() && needs_update(current, &latest);
    if !forced && !newer {
        return Ok(false);
    }
    let target = if newer {
        latest.as_str()
    } else {
        min_version.unwrap_or("")
    };
    log::info!("update available: current={current} target={target} forced={forced}; downloading");

    let dest = crate::config::config_dir()
        .join("update")
        .join(format!("ufoagent-setup-{target}.exe"));
    download_installer(&repo, &dest)?;

    if let Err(e) = verify_authenticode(&dest) {
        log::warn!("downloaded installer failed signature verification — refusing to install: {e}");
        let _ = std::fs::remove_file(&dest);
        return Ok(false);
    }

    log::info!("installer signature verified; launching silent update");
    apply_update(&dest)?;
    Ok(true)
}

#[cfg(windows)]
mod win {
    use anyhow::{bail, Result};
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Security::WinTrust::{
        WinVerifyTrust, WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_DATA, WINTRUST_FILE_INFO,
        WTD_CHOICE_FILE, WTD_REVOKE_NONE, WTD_STATEACTION_CLOSE, WTD_STATEACTION_VERIFY,
        WTD_UI_NONE,
    };

    pub fn verify_authenticode(path: &Path) -> Result<()> {
        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();

        // SAFETY: zeroed C structs are the documented initial state (WinVerifyTrust's `= {0}`);
        // `file_info`, `data`, `wide`, and `action` all outlive both WinVerifyTrust calls.
        unsafe {
            let mut file_info: WINTRUST_FILE_INFO = std::mem::zeroed();
            file_info.cbStruct = std::mem::size_of::<WINTRUST_FILE_INFO>() as u32;
            file_info.pcwszFilePath = PCWSTR(wide.as_ptr());

            let mut data: WINTRUST_DATA = std::mem::zeroed();
            data.cbStruct = std::mem::size_of::<WINTRUST_DATA>() as u32;
            data.dwUIChoice = WTD_UI_NONE;
            data.fdwRevocationChecks = WTD_REVOKE_NONE;
            data.dwUnionChoice = WTD_CHOICE_FILE;
            data.dwStateAction = WTD_STATEACTION_VERIFY;
            data.Anonymous.pFile = &mut file_info as *mut _;

            let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
            let status = WinVerifyTrust(
                HWND::default(),
                &mut action,
                &mut data as *mut _ as *mut core::ffi::c_void,
            );

            // Always release the state data, regardless of the verdict. Re-borrow `data` here
            // (rather than reuse a cached pointer) so the CLOSE write is seen as observed.
            data.dwStateAction = WTD_STATEACTION_CLOSE;
            let _ = WinVerifyTrust(
                HWND::default(),
                &mut action,
                &mut data as *mut _ as *mut core::ffi::c_void,
            );

            if status != 0 {
                bail!("Authenticode verification failed (status 0x{:08x})", status);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_compares() {
        assert_eq!(version_tuple("0.2.1"), vec![0, 2, 1]);
        assert_eq!(version_tuple("v1.10.0-rc2"), vec![1, 10, 0]);
        assert!(needs_update("0.1.0", "0.2.0"));
        assert!(!needs_update("0.2.0", "0.2.0"));
        assert!(!needs_update("1.0.0", "0.9.9"));
    }

    #[test]
    fn refuses_unsigned_file() {
        // A non-PE garbage file must never verify (Err on Windows; the stub bails elsewhere).
        let p = std::env::temp_dir().join("ufoagent-test-unsigned.bin");
        std::fs::write(&p, b"not a signed installer").unwrap();
        assert!(verify_authenticode(&p).is_err());
        let _ = std::fs::remove_file(&p);
    }
}
