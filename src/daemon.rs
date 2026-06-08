//! Credential-refresh + heartbeat loop (the service body), and one-shot refresh.

use anyhow::Result;
use log::{info, warn};
use std::path::Path;
use std::time::Duration;

use crate::config::Config;
use crate::controlplane::{ControlPlane, Credential};
use crate::status::{self, Status};
use crate::{store, ufo_config};

pub fn platform() -> String {
    format!("{} ({})", std::env::consts::OS, std::env::consts::ARCH)
}

/// Fetch a credential and write it into agents.yaml.
pub fn refresh_once(cp: &ControlPlane, ufo_home: &Path) -> Result<Credential> {
    let cred = cp.get_credentials()?;
    ufo_config::write(ufo_home, &cred)?;
    info!(
        "refreshed credential lease={} model={} expires_at={}",
        cred.lease_id, cred.model, cred.expires_at
    );
    Ok(cred)
}

/// Long-running loop: keep the credential fresh + heartbeat; write status.json each cycle.
/// Runs until `should_stop()` returns true (the service handler flips it).
pub fn run_daemon(version: &str, should_stop: impl Fn() -> bool) -> Result<()> {
    let cfg = Config::load();
    let ufo_home = cfg.ufo_home_path();

    while !should_stop() {
        let cp = ControlPlane::new(&cfg.control_plane_url(), store::get_token());
        let mut st = Status {
            agent_version: version.to_string(),
            linked: store::get_token().is_some(),
            ..Default::default()
        };
        let mut nap: u64 = 60;

        match refresh_once(&cp, &ufo_home) {
            Ok(c) => {
                st.last_refresh = status::now();
                st.lease_expires_at = c.expires_at;
                let secs_left = c.expires_at - status::now();
                nap = std::cmp::max(60, secs_left - 120) as u64;
            }
            Err(e) => {
                warn!("credential refresh failed: {}", e);
                st.last_error = Some(e.to_string());
            }
        }

        match cp.heartbeat(version, &platform()) {
            Ok(()) => {
                st.online = true;
                st.last_heartbeat_ok = true;
            }
            Err(e) => {
                warn!("heartbeat failed: {}", e);
                st.last_error = Some(e.to_string());
            }
        }

        let _ = status::save(&st);

        // Sleep in short slices so the service can stop promptly.
        let mut slept = 0;
        while slept < nap && !should_stop() {
            std::thread::sleep(Duration::from_secs(2));
            slept += 2;
        }
    }
    Ok(())
}
