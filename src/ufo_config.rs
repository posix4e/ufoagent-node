//! Render UFO2's config/ufo/agents.yaml from a vended credential.

use anyhow::Result;
use std::path::{Path, PathBuf};

use crate::controlplane::Credential;

const UNATTENDED_SAFE_GUARD_LINE: &str =
    "SAFE_GUARD: False  # Managed by ufoagent: unattended runs auto-approve confirmations.";

fn yaml_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn agent_section(c: &Credential) -> String {
    format!(
        "  VISUAL_MODE: true\n  API_TYPE: \"openai\"\n  API_BASE: \"{}\"\n  API_KEY: \"{}\"\n  API_MODEL: \"{}\"\n",
        yaml_escape(&c.base_url),
        yaml_escape(&c.api_key),
        yaml_escape(&c.model),
    )
}

pub fn render(c: &Credential) -> String {
    let section = agent_section(c);
    // Every UFO2 agent role uses the one vended credential. Roles we leave unconfigured, notably
    // EVALUATION_AGENT, fall back to UFO2's Azure-AD default and crash after the task completes.
    ["HOST_AGENT", "APP_AGENT", "EVALUATION_AGENT", "BACKUP_AGENT"]
        .iter()
        .fold(
            "# Managed by ufoagent - do not edit by hand.\n# Short-lived credential, refreshed by the agent.\n".to_string(),
            |mut acc, name| {
                acc.push_str(name);
                acc.push_str(":\n");
                acc.push_str(&section);
                acc
            },
        )
}

pub fn agents_yaml_path(ufo_home: &Path) -> PathBuf {
    ufo_home.join("config").join("ufo").join("agents.yaml")
}

pub fn system_yaml_path(ufo_home: &Path) -> PathBuf {
    ufo_home.join("config").join("ufo").join("system.yaml")
}

/// Apply only the UFOAgent-owned runtime setting.
///
/// UFO's native mcp.yaml and prompt/source configuration are intentionally left intact. We only use
/// UFO's own SAFE_GUARD flag to keep unattended runs from blocking on interactive confirmation.
pub fn apply_unattended_mode(ufo_home: &Path) -> Result<PathBuf> {
    let path = system_yaml_path(ufo_home);
    let original = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e.into()),
    };

    let mut saw_safe_guard = false;
    let mut changed = false;
    let mut lines = Vec::new();

    for line in original.lines() {
        if line.trim_start().starts_with("SAFE_GUARD:") {
            saw_safe_guard = true;
            if line != UNATTENDED_SAFE_GUARD_LINE {
                changed = true;
            }
            lines.push(UNATTENDED_SAFE_GUARD_LINE.to_string());
        } else {
            lines.push(line.to_string());
        }
    }

    if !saw_safe_guard {
        if !lines.is_empty() && lines.last().is_some_and(|l| !l.trim().is_empty()) {
            lines.push(String::new());
        }
        lines.push("# UFOAgent unattended mode".to_string());
        lines.push(UNATTENDED_SAFE_GUARD_LINE.to_string());
        changed = true;
    }

    let mut rendered = lines.join("\n");
    rendered.push('\n');
    if changed || rendered != original {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, rendered)?;
    }
    Ok(path)
}

pub fn write(ufo_home: &Path, c: &Credential) -> Result<PathBuf> {
    let path = agents_yaml_path(ufo_home);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, render(c))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cred(key: &str) -> Credential {
        Credential {
            base_url: "https://api.openai.com/v1".into(),
            model: "gpt-4o".into(),
            api_key: key.into(),
            expires_at: 0,
            lease_id: "lse_x".into(),
        }
    }

    #[test]
    fn renders_all_agent_sections() {
        let y = render(&cred("sk-secret"));
        for role in [
            "HOST_AGENT:",
            "APP_AGENT:",
            "EVALUATION_AGENT:",
            "BACKUP_AGENT:",
        ] {
            assert!(y.contains(role), "missing {role}");
        }
        assert_eq!(y.matches("API_KEY: \"sk-secret\"").count(), 4);
        assert!(y.contains("API_TYPE: \"openai\""));
        assert!(y.contains("VISUAL_MODE: true"));
    }

    #[test]
    fn escapes_quotes_and_backslashes() {
        let y = render(&cred("a\"b\\c"));
        assert!(y.contains("API_KEY: \"a\\\"b\\\\c\""));
    }

    fn temp_home(name: &str) -> PathBuf {
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!("ufoagent-{name}-{}-{nanos}", std::process::id()))
    }

    #[test]
    fn unattended_mode_only_disables_safe_guard() {
        let home = temp_home("safe-guard-existing");
        let system = system_yaml_path(&home);
        std::fs::create_dir_all(system.parent().unwrap()).unwrap();
        std::fs::write(
            &system,
            "# UFO System Configuration\nUSE_MCP: True\nSAFE_GUARD: True  # upstream default\nMCP_FALLBACK_TO_UI: True\n",
        )
        .unwrap();

        let path = apply_unattended_mode(&home).unwrap();
        let y = std::fs::read_to_string(&path).unwrap();
        assert!(y.contains(UNATTENDED_SAFE_GUARD_LINE));
        assert!(y.contains("USE_MCP: True"));
        assert!(y.contains("MCP_FALLBACK_TO_UI: True"));
        assert!(!y.contains("SAFE_GUARD: True"));

        apply_unattended_mode(&home).unwrap();
        let y = std::fs::read_to_string(&path).unwrap();
        assert_eq!(y.matches("SAFE_GUARD:").count(), 1);

        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn unattended_mode_creates_system_yaml_when_missing() {
        let home = temp_home("safe-guard-missing");

        let path = apply_unattended_mode(&home).unwrap();
        let y = std::fs::read_to_string(&path).unwrap();
        assert!(y.contains("# UFOAgent unattended mode"));
        assert!(y.contains(UNATTENDED_SAFE_GUARD_LINE));

        let _ = std::fs::remove_dir_all(home);
    }
}
