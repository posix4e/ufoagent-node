//! Idempotent self-heal: fix config, (re)provision UFO2, refresh credential.

use anyhow::Result;

use crate::config::{Config, DEFAULT_CONTROL_PLANE};
use crate::controlplane::ControlPlane;
use crate::env::{self, EnvState};
use crate::{agent, bootstrap, store, ufo_config};

pub fn repair() -> Result<Vec<String>> {
    let mut log = Vec::new();
    let mut cfg = Config::load();

    if cfg.control_plane.is_none() {
        cfg.control_plane = Some(DEFAULT_CONTROL_PLANE.to_string());
        cfg.save()?;
        log.push(format!("set control plane -> {DEFAULT_CONTROL_PLANE}"));
    }

    let home = cfg.ufo_home_path();
    // "Provisioned" = venv interpreter + source both present — NOT just requirements.txt, which lands
    // right after the source download (before pip finishes). Re-bootstrap unless genuinely complete,
    // so repair can't stamp a half-installed env as ready.
    if !env::ufo2_provisioned() {
        log.push("UFO2 not provisioned -> bootstrapping".into());
        bootstrap::bootstrap(None, "main")?;
        log.push("UFO2 provisioned".into());
    } else {
        // Genuinely provisioned (possibly before env tracking existed) — record a ready marker so the
        // dashboard chip and run_task gate are marker-based from here on.
        ufo_config::apply_managed_defaults(&home)?;
        env::set_state(env::UFO2, EnvState::Ready, None, None);
        log.push("UFO2 present".into());
    }

    if store::get_token().is_some() {
        let cp = ControlPlane::new(&cfg.control_plane_url(), store::get_token());
        match agent::refresh_once(&cp, &home) {
            Ok(c) => log.push(format!("credential refreshed (lease {})", c.lease_id)),
            Err(e) => log.push(format!("credential refresh failed: {e}")),
        }
    } else {
        log.push("not linked — run `ufoagent link`".into());
    }

    Ok(log)
}
