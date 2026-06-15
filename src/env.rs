//! Environment registry + health reporting. An "environment" is a runtime a remote task can target
//! — today only `ufo2` (Microsoft UFO2); `ufo3` and `claude` are planned (control-plane issues #1/#2).
//!
//! The agent reports each environment's install state to the control plane over the WebSocket so the
//! dashboard can show per-node chips and so `run_task` is gated to environments that are actually
//! `ready`. The authoritative state is written by the only things that install — `bootstrap`/`repair`
//! — into a small marker file under the config dir; the probe reads that marker and reconciles it
//! with what's on disk (a `ready` marker whose venv has vanished reads `broken`). The probe is
//! deliberately cheap (read one JSON file + stat the venv) so it can run on every WS hello and tick;
//! the expensive readiness verdict (does `import ufo` actually work) is settled once, at install time.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::{config_dir, Config};
use crate::util;

/// The only environment we ship today. Registry is a slice so ufo3/claude slot in later.
pub const UFO2: &str = "ufo2";
const KNOWN_ENVS: &[&str] = &[UFO2];

/// An `installing` marker older than this is treated as stuck (mirrors the control plane's
/// `INSTALLING_STALE_SECONDS`) so a crashed bootstrap surfaces as "run Repair" rather than a
/// forever-spinning chip.
const INSTALLING_STALE_SECS: i64 = 45 * 60;

/// Install state of one environment. Serializes to the exact strings the control plane's
/// `EnvState` (src/types.ts) expects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvState {
    NotInstalled,
    Installing,
    Ready,
    Broken,
}

/// One environment's health, as sent to the control plane. Mirror of the server's `EnvReport`
/// (src/types.ts): `detail`/`version_ref` omitted when absent (the server reads them as null),
/// `updated_at` in epoch seconds (the server uses it to detect a stuck install).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvReport {
    pub name: String,
    pub state: EnvState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_ref: Option<String>,
    pub updated_at: i64,
}

/// The on-disk marker bootstrap/repair maintain — the authoritative lifecycle state for an env.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct EnvMarker {
    state: EnvState,
    #[serde(default)]
    detail: Option<String>,
    #[serde(default)]
    version_ref: Option<String>,
    updated_at: i64,
}

fn marker_path(name: &str) -> PathBuf {
    config_dir().join("envs").join(format!("{name}.json"))
}

fn read_marker(name: &str) -> Option<EnvMarker> {
    let raw = std::fs::read_to_string(marker_path(name)).ok()?;
    serde_json::from_str(&raw).ok()
}

/// Record an environment's install state. Called by bootstrap/repair on every lifecycle transition.
/// Best-effort: a failed write is logged, never propagated — provisioning must not fail because a
/// status file couldn't be written.
pub fn set_state(name: &str, state: EnvState, detail: Option<&str>, version_ref: Option<&str>) {
    let marker = EnvMarker {
        state,
        // The server caps detail at 400 chars; keep the marker tidy and trim to a single line so a
        // multi-line bootstrap error doesn't bloat the chip tooltip.
        detail: detail.map(|d| first_line(d, 400)),
        version_ref: version_ref.map(|v| v.chars().take(64).collect()),
        updated_at: util::now(),
    };
    let path = marker_path(name);
    let ok = path.parent().map(std::fs::create_dir_all).unwrap_or(Ok(()));
    let written = ok.and_then(|()| {
        serde_json::to_vec(&marker)
            .map_err(std::io::Error::other)
            .and_then(|bytes| std::fs::write(&path, bytes))
    });
    if let Err(e) = written {
        log::warn!("could not record env state for {name}: {e}");
    }
}

/// First non-empty line of `s`, truncated to `max` bytes on a char boundary.
fn first_line(s: &str, max: usize) -> String {
    let line = s
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim();
    util::prefix_on_char_boundary(line, max).to_string()
}

/// Report every known environment (always non-empty; an env with nothing on disk reads
/// `not_installed`, never absent — so the control plane never sees a new agent as "too old to
/// report").
pub fn report_all() -> Vec<EnvReport> {
    KNOWN_ENVS.iter().map(|&n| report(n)).collect()
}

/// Report one environment's health.
pub fn report(name: &str) -> EnvReport {
    match name {
        UFO2 => report_ufo2(),
        // Registered but unimplemented env: report it as not-installed rather than lying.
        other => EnvReport {
            name: other.to_string(),
            state: EnvState::NotInstalled,
            detail: None,
            version_ref: None,
            updated_at: 0,
        },
    }
}

fn report_ufo2() -> EnvReport {
    let cfg = Config::load();
    let venv_ok = venv_python_exists(&cfg);

    if let Some(m) = read_marker(UFO2) {
        // Reconcile the marker against reality: a "ready" install whose venv interpreter has gone
        // missing is actually broken, regardless of what the marker last recorded.
        if m.state == EnvState::Ready && !venv_ok {
            return EnvReport {
                name: UFO2.to_string(),
                state: EnvState::Broken,
                detail: Some("UFO2 virtualenv is missing — run Repair".to_string()),
                version_ref: m.version_ref,
                updated_at: util::now(),
            };
        }
        return EnvReport {
            name: UFO2.to_string(),
            state: m.state,
            detail: m.detail,
            version_ref: m.version_ref,
            updated_at: m.updated_at,
        };
    }

    // No marker: a node provisioned before env tracking existed, or one that never installed UFO2.
    // Infer from disk — source present + a usable venv reads ready; source without a venv reads
    // broken; neither reads not_installed.
    let source_present = cfg.ufo_home_path().join("requirements.txt").exists();
    let state = match (source_present, venv_ok) {
        (true, true) => EnvState::Ready,
        (true, false) => EnvState::Broken,
        (false, _) => EnvState::NotInstalled,
    };
    EnvReport {
        name: UFO2.to_string(),
        state,
        detail: None,
        version_ref: None,
        updated_at: 0,
    }
}

/// True if the configured venv interpreter exists on disk (bootstrap records it as `config.python`).
fn venv_python_exists(cfg: &Config) -> bool {
    cfg.python
        .as_ref()
        .map(|p| Path::new(p).is_file())
        .unwrap_or(false)
}

/// True when UFO2 is genuinely provisioned: the recorded venv interpreter exists AND the UFO2 source
/// (requirements.txt) is present. `requirements.txt` alone is NOT enough — it appears right after the
/// source-zip download, long before pip finishes — so `repair` uses this (not file existence) before
/// declaring the env ready.
pub fn ufo2_provisioned() -> bool {
    let cfg = Config::load();
    venv_python_exists(&cfg) && cfg.ufo_home_path().join("requirements.txt").exists()
}

/// Self-gate a `run_task`: a human-readable reason to refuse, or `None` if the environment is ready.
/// Mirrors the control plane's `gateRunTask` so the node rejects with the same words even when the
/// server's last-reported state is stale (the node has ground truth).
pub fn gate(name: &str) -> Option<String> {
    let r = report(name);
    match r.state {
        EnvState::Ready => None,
        EnvState::Installing => Some(if util::now() - r.updated_at > INSTALLING_STALE_SECS {
            format!("{name} install looks stuck — run Repair on this node")
        } else {
            match r.detail {
                Some(d) => format!("{name} is installing ({d}) — try again in a few minutes"),
                None => format!("{name} is installing — try again in a few minutes"),
            }
        }),
        EnvState::Broken => Some(match r.detail {
            Some(d) => format!("{name} is broken: {d} — run Repair on this node"),
            None => format!("{name} is broken — run Repair on this node"),
        }),
        EnvState::NotInstalled => Some(format!(
            "{name} is not installed on this node — run Repair to provision it"
        )),
    }
}

/// One-line "ufo2=ready, ufo3=not_installed" summary for the connect log (and CI assertions).
pub fn summary() -> String {
    report_all()
        .iter()
        .map(|e| format!("{}={}", e.name, state_str(e.state)))
        .collect::<Vec<_>>()
        .join(", ")
}

fn state_str(s: EnvState) -> &'static str {
    match s {
        EnvState::NotInstalled => "not_installed",
        EnvState::Installing => "installing",
        EnvState::Ready => "ready",
        EnvState::Broken => "broken",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::with_temp_home;

    /// Point config at a freshly provisioned-looking UFO2: a ufo_home with requirements.txt and an
    /// existing venv interpreter file, both under the temp config dir.
    fn provision_ufo2() {
        let mut cfg = Config::load();
        let home = config_dir().join("ufo");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(home.join("requirements.txt"), "ufo\n").unwrap();
        let py = home.join("py-fake");
        std::fs::write(&py, "").unwrap();
        cfg.ufo_home = Some(home.to_string_lossy().to_string());
        cfg.python = Some(py.to_string_lossy().to_string());
        cfg.save().unwrap();
    }

    #[test]
    fn nothing_installed_reads_not_installed() {
        with_temp_home("env-none", || {
            let r = report(UFO2);
            assert_eq!(r.state, EnvState::NotInstalled);
            assert!(gate(UFO2).unwrap().contains("not installed"));
        });
    }

    #[test]
    fn legacy_install_without_marker_reads_ready() {
        with_temp_home("env-legacy", || {
            provision_ufo2();
            let r = report(UFO2);
            assert_eq!(r.state, EnvState::Ready);
            assert!(gate(UFO2).is_none());
        });
    }

    #[test]
    fn installing_marker_gates_and_reports() {
        with_temp_home("env-installing", || {
            set_state(
                UFO2,
                EnvState::Installing,
                Some("pip: installing torch"),
                Some("main"),
            );
            let r = report(UFO2);
            assert_eq!(r.state, EnvState::Installing);
            assert_eq!(r.detail.as_deref(), Some("pip: installing torch"));
            assert_eq!(r.version_ref.as_deref(), Some("main"));
            let reason = gate(UFO2).unwrap();
            assert!(reason.contains("installing") && reason.contains("torch"));
        });
    }

    #[test]
    fn stale_installing_marker_reads_stuck() {
        with_temp_home("env-stuck", || {
            set_state(UFO2, EnvState::Installing, Some("pip"), None);
            // Backdate the marker past the stale threshold.
            let raw = std::fs::read_to_string(marker_path(UFO2)).unwrap();
            let mut m: EnvMarker = serde_json::from_str(&raw).unwrap();
            m.updated_at = util::now() - INSTALLING_STALE_SECS - 60;
            std::fs::write(marker_path(UFO2), serde_json::to_vec(&m).unwrap()).unwrap();
            assert!(gate(UFO2).unwrap().contains("stuck"));
        });
    }

    #[test]
    fn broken_marker_gates() {
        with_temp_home("env-broken", || {
            set_state(UFO2, EnvState::Broken, Some("python not found"), None);
            let r = report(UFO2);
            assert_eq!(r.state, EnvState::Broken);
            assert!(gate(UFO2).unwrap().contains("python not found"));
        });
    }

    #[test]
    fn ready_marker_with_missing_venv_reconciles_to_broken() {
        with_temp_home("env-reconcile", || {
            // Marker says ready, but config.python points nowhere (no venv on disk).
            set_state(UFO2, EnvState::Ready, None, Some("main"));
            let r = report(UFO2);
            assert_eq!(r.state, EnvState::Broken);
            assert!(gate(UFO2).unwrap().contains("Repair"));
        });
    }

    #[test]
    fn ready_marker_with_venv_reads_ready() {
        with_temp_home("env-ready", || {
            provision_ufo2();
            set_state(UFO2, EnvState::Ready, None, Some("main"));
            assert_eq!(report(UFO2).state, EnvState::Ready);
            assert!(gate(UFO2).is_none());
        });
    }

    #[test]
    fn report_all_covers_registry_and_serializes_to_server_shape() {
        with_temp_home("env-shape", || {
            set_state(UFO2, EnvState::Ready, None, Some("main"));
            provision_ufo2();
            let all = report_all();
            assert_eq!(all.len(), KNOWN_ENVS.len());
            let v = serde_json::to_value(&all).unwrap();
            let first = &v[0];
            assert_eq!(first["name"], "ufo2");
            assert_eq!(first["state"], "ready"); // snake_case strings the control plane expects
            assert_eq!(first["version_ref"], "main");
            // Absent optionals are omitted, not null:false — sanitizeEnvs handles either.
            assert!(first.get("detail").is_none());
        });
    }

    #[test]
    fn unknown_env_gate_says_not_installed() {
        with_temp_home("env-unknown", || {
            assert!(gate("ufo3").unwrap().contains("not installed"));
        });
    }

    #[test]
    fn ufo2_provisioned_requires_venv_not_just_source() {
        with_temp_home("env-prov", || {
            assert!(!ufo2_provisioned()); // nothing on disk
                                          // Mid-bootstrap: source downloaded (requirements.txt) but no venv yet (config.python unset).
            let home = config_dir().join("ufo");
            std::fs::create_dir_all(&home).unwrap();
            std::fs::write(home.join("requirements.txt"), "ufo\n").unwrap();
            let mut cfg = Config::load();
            cfg.ufo_home = Some(home.to_string_lossy().to_string());
            cfg.save().unwrap();
            assert!(!ufo2_provisioned()); // requirements.txt alone must NOT read provisioned
                                          // Bootstrap completes: venv interpreter recorded.
            provision_ufo2();
            assert!(ufo2_provisioned());
        });
    }
}
