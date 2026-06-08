//! Local config (control plane URL, UFO2 home, python) persisted as JSON.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Control plane lives on the app. subdomain (ufoagent.xyz is the marketing site).
pub const DEFAULT_CONTROL_PLANE: &str = "https://app.ufoagent.xyz";

/// GitHub repo the self-updater pulls signed releases from.
pub const DEFAULT_UPDATE_REPO: &str = "ufoagent/ufoagent-node";

/// Per-machine config dir. Override with UFOAGENT_HOME (used by tests / non-Windows dev).
pub fn config_dir() -> PathBuf {
    if let Ok(p) = std::env::var("UFOAGENT_HOME") {
        return PathBuf::from(p);
    }
    #[cfg(windows)]
    {
        let base = std::env::var("ProgramData").unwrap_or_else(|_| r"C:\ProgramData".to_string());
        PathBuf::from(base).join("UFOAgent")
    }
    #[cfg(not(windows))]
    {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".config")
            .join("ufoagent")
    }
}

#[derive(Default, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control_plane: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ufo_home: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub python: Option<String>,
    /// Kill-switch for service self-update (default on).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_update: Option<bool>,
    /// Override the GitHub repo updates are pulled from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update_repo: Option<String>,
}

impl Config {
    pub fn load() -> Config {
        std::fs::read_to_string(config_dir().join("config.json"))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<()> {
        let dir = config_dir();
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("config.json"), serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    pub fn control_plane_url(&self) -> String {
        self.control_plane
            .clone()
            .unwrap_or_else(|| DEFAULT_CONTROL_PLANE.to_string())
    }

    pub fn auto_update_enabled(&self) -> bool {
        self.auto_update.unwrap_or(true)
    }

    pub fn update_repo(&self) -> String {
        self.update_repo
            .clone()
            .unwrap_or_else(|| DEFAULT_UPDATE_REPO.to_string())
    }

    /// Managed default so refresh/heartbeat work even before UFO2 is installed.
    pub fn ufo_home_path(&self) -> PathBuf {
        self.ufo_home
            .clone()
            .map(PathBuf::from)
            .unwrap_or_else(|| config_dir().join("ufo"))
    }
}
