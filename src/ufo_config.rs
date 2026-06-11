//! Render UFO2's config/ufo/agents.yaml from a vended credential.

use anyhow::Result;
use std::path::{Path, PathBuf};

use crate::controlplane::Credential;

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
    // Every UFO2 agent role uses the one vended credential. Roles we leave unconfigured — notably
    // EVALUATION_AGENT — fall back to UFO2's Azure-AD default and crash *after* the task completes
    // (see issue #8), which recorded successful run_tasks as "failed". Configure all of them.
    ["HOST_AGENT", "APP_AGENT", "EVALUATION_AGENT", "BACKUP_AGENT"]
        .iter()
        .fold(
            "# Managed by ufoagent — do not edit by hand.\n# Short-lived credential, refreshed by the agent.\n".to_string(),
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
        // All four roles must be present — a missing EVALUATION_AGENT/BACKUP_AGENT falls back to
        // UFO2's Azure-AD default and crashes after the task (issue #8).
        for role in [
            "HOST_AGENT:",
            "APP_AGENT:",
            "EVALUATION_AGENT:",
            "BACKUP_AGENT:",
        ] {
            assert!(y.contains(role), "missing {role}");
        }
        // The credential is set on every role (one per section).
        assert_eq!(y.matches("API_KEY: \"sk-secret\"").count(), 4);
        assert!(y.contains("API_TYPE: \"openai\""));
        assert!(y.contains("VISUAL_MODE: true"));
    }

    #[test]
    fn escapes_quotes_and_backslashes() {
        let y = render(&cred("a\"b\\c"));
        assert!(y.contains("API_KEY: \"a\\\"b\\\\c\""));
    }
}
