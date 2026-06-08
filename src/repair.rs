//! Idempotent self-heal: fix config, (re)provision UFO2, refresh credential.

use anyhow::Result;

use crate::config::{Config, DEFAULT_CONTROL_PLANE};
use crate::controlplane::ControlPlane;
use crate::{bootstrap, daemon, store};

pub fn repair() -> Result<Vec<String>> {
    let mut log = Vec::new();
    let mut cfg = Config::load();

    if cfg.control_plane.is_none() {
        cfg.control_plane = Some(DEFAULT_CONTROL_PLANE.to_string());
        cfg.save()?;
        log.push(format!("set control plane -> {DEFAULT_CONTROL_PLANE}"));
    }

    let home = cfg.ufo_home_path();
    if !home.join("requirements.txt").exists() {
        log.push("UFO2 not installed -> bootstrapping".into());
        bootstrap::bootstrap(None, "main")?;
        log.push("UFO2 provisioned".into());
    } else {
        log.push("UFO2 present".into());
    }

    if store::get_token().is_some() {
        let cp = ControlPlane::new(&cfg.control_plane_url(), store::get_token());
        match daemon::refresh_once(&cp, &home) {
            Ok(c) => log.push(format!("credential refreshed (lease {})", c.lease_id)),
            Err(e) => log.push(format!("credential refresh failed: {e}")),
        }
    } else {
        log.push("not linked — run `ufoagent link`".into());
    }

    Ok(log)
}
