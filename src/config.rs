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

/// Shared test helpers: `config_dir()` reads the process-global `UFOAGENT_HOME`, so every test that
/// redirects it must serialize on the SAME lock — separate per-module locks would still race. Run a
/// closure with `UFOAGENT_HOME` pointed at a unique temp dir, holding this one lock for its duration.
#[cfg(test)]
pub(crate) fn with_temp_home<T>(prefix: &str, f: impl FnOnce() -> T) -> T {
    use std::sync::Mutex;
    use std::time::{SystemTime, UNIX_EPOCH};
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("{prefix}-{}-{nanos}", std::process::id()));
    std::env::set_var("UFOAGENT_HOME", &dir);
    let out = f();
    std::env::remove_var("UFOAGENT_HOME");
    let _ = std::fs::remove_dir_all(&dir);
    out
}
