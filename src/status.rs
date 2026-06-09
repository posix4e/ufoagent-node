//! On-disk node status (written by the daemon, read by the tray/manager).

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::config::config_dir;

#[derive(Default, Clone, Serialize, Deserialize)]
pub struct Status {
    /// True while the WebSocket to the control plane is up (the liveness signal).
    pub online: bool,
    pub linked: bool,
    pub agent_version: String,
    pub last_refresh: i64,
    pub lease_expires_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_task: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[allow(dead_code)] // only caller is the cfg(windows) tray, so non-Windows builds see it unused
pub fn load() -> Status {
    std::fs::read_to_string(config_dir().join("status.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(s: &Status) -> Result<()> {
    let dir = config_dir();
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("status.json"), serde_json::to_string_pretty(s)?)?;
    Ok(())
}
