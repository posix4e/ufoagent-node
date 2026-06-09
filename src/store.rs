//! Agent bearer-token storage. Stored in the per-machine config dir (ProgramData on Windows,
//! whose default ACLs are admin/SYSTEM-only). TODO: wrap with DPAPI on Windows for at-rest
//! encryption (cfg(windows), verified in Windows CI).

use anyhow::Result;
use std::path::PathBuf;

use crate::config::config_dir;

fn token_path() -> PathBuf {
    config_dir().join("agent.token")
}

pub fn set_token(token: &str) -> Result<()> {
    std::fs::create_dir_all(config_dir())?;
    std::fs::write(token_path(), token.as_bytes())?;
    Ok(())
}

pub fn get_token() -> Option<String> {
    std::fs::read_to_string(token_path())
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}
