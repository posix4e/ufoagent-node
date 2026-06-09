//! The service body: credential-refresh + self-update loop, with the WebSocket command channel
//! on its own thread as the single control-plane connection. Also the one-shot refresh.

use anyhow::Result;
use log::{info, warn};
use std::path::Path;
use std::time::Duration;

use crate::config::Config;
use crate::controlplane::{ControlPlane, Credential};
use crate::status::{self, Status};
use crate::{store, ufo_config, update, ws};

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

/// Per-node jitter (0–30 min) so a fleet doesn't all hit GitHub for updates at once.
fn boot_jitter_secs() -> i64 {
    let name = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_default();
    let h: u32 = name.bytes().fold(2166136261u32, |acc, b| {
        (acc ^ b as u32).wrapping_mul(16777619)
    });
    (h % 1800) as i64
}

/// Long-running loop (the service body). The WebSocket (own thread) is the control-plane
/// connection: command delivery, liveness, and the min_version floor (hello_ack). This loop
/// refreshes the credential near lease expiry, checks self-updates on a slow cadence (and right
/// after boot, to catch a forced min_version bump), and writes status.json each tick. Runs
/// until `should_stop()` flips.
pub fn run_daemon(version: &str, should_stop: impl Fn() -> bool) -> Result<()> {
    let cfg = Config::load();
    let ufo_home = cfg.ufo_home_path();

    // Not joined on exit: the WS read timeout means it can take ~20s to notice the stop flag,
    // and the service must stop promptly; the thread dies with the process.
    let ws_state = std::sync::Arc::new(ws::WsState::new());
    let ws_stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let state = ws_state.clone();
        let stop = ws_stop.clone();
        let version = version.to_string();
        std::thread::spawn(move || {
            ws::run(&version, state, move || {
                stop.load(std::sync::atomic::Ordering::Relaxed)
            })
        });
    }

    const TICK_SECS: u64 = 60;
    let update_interval = 21_600 + boot_jitter_secs(); // ~6h + jitter
    let mut next_refresh: i64 = 0; // refresh creds when now >= this (0 = immediately)
    let mut next_update_check: i64 = 0; // 0 = check soon after boot (catches forced updates)

    let mut st = Status {
        agent_version: version.to_string(),
        ..Default::default()
    };

    while !should_stop() {
        let now = status::now();
        st.linked = store::get_token().is_some();
        st.last_error = None;
        let cp = ControlPlane::new(&cfg.control_plane_url(), store::get_token());

        // 1) Credentials — only refresh when the current lease is near expiry.
        if now >= next_refresh {
            match refresh_once(&cp, &ufo_home) {
                Ok(c) => {
                    st.last_refresh = status::now();
                    st.lease_expires_at = c.expires_at;
                    next_refresh = c.expires_at - 120;
                }
                Err(e) => {
                    warn!("credential refresh failed: {}", e);
                    st.last_error = Some(e.to_string());
                    next_refresh = status::now() + 300; // retry in 5 min
                }
            }
        }

        // 2) Liveness for the tray = the socket state (the control plane sees the same thing).
        st.online = ws_state.connected();

        // 3) Self-update — slow cadence (first pass right after boot catches forced bumps).
        //    min_version arrives over the WS (hello_ack).
        if now >= next_update_check {
            next_update_check = status::now() + update_interval;
            let min_version = ws_state.min_version();
            match update::maybe_self_update(&cfg, version, min_version.as_deref()) {
                Ok(true) => info!("self-update launched; expecting service restart"),
                Ok(false) => {}
                Err(e) => warn!("self-update check failed: {}", e),
            }
        }

        let _ = status::save(&st);

        // Sleep one tick, in short slices so the service can stop promptly.
        let mut slept = 0;
        while slept < TICK_SECS && !should_stop() {
            std::thread::sleep(Duration::from_secs(2));
            slept += 2;
        }
    }
    ws_stop.store(true, std::sync::atomic::Ordering::Relaxed);
    Ok(())
}
