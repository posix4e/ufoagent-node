//! "What has this node been doing?" — a friendly, plain-English summary of the recent command
//! history, generated on-device by the same LLM the agent already has credentials for. Reads the
//! command log (cmdlog), mints a credential the usual way, and asks the model for an operator-
//! facing recap. Falls back to a plain listing if there's no credential or the call fails — it
//! never errors out the caller (the tray/CLI just shows what it can).

use crate::cmdlog::{self, Entry};
use crate::config::Config;
use crate::controlplane::{ControlPlane, USER_AGENT};
use crate::store;
use std::time::Duration;

const SYSTEM_PROMPT: &str = "You are explaining to a non-technical operator what their Windows \
automation agent has been doing. You are given its recent activity log (most recent last). Write \
a short, friendly recap — a sentence or two plus a few bullet points. Group similar actions, say \
roughly how recent things are, and clearly flag anything that failed. Don't invent anything not in \
the log. If the log is empty, say the agent hasn't done anything yet.";

/// Build the operator-facing summary. Returns the LLM's prose, or a plain fallback listing.
pub fn summarize() -> String {
    let entries = cmdlog::recent(25);
    if entries.is_empty() {
        return "This machine hasn't run any commands yet.".to_string();
    }
    match llm_summary(&entries) {
        Ok(s) if !s.trim().is_empty() => s,
        Ok(_) | Err(_) => plain_listing(&entries),
    }
}

/// One chat-completion against the agent's vended credential (OpenAI-compatible).
fn llm_summary(entries: &[Entry]) -> anyhow::Result<String> {
    let cfg = Config::load();
    let cp = ControlPlane::new(&cfg.control_plane_url(), store::get_token());
    let cred = cp.get_credentials()?; // mints/reuses the node's short-lived key

    let log_text = entries
        .iter()
        .map(format_entry)
        .collect::<Vec<_>>()
        .join("\n");

    let body = serde_json::json!({
        "model": cred.model,
        "messages": [
            { "role": "system", "content": SYSTEM_PROMPT },
            { "role": "user", "content": format!("Recent activity:\n{log_text}") },
        ],
        "max_tokens": 350,
        "temperature": 0.3,
    });

    let url = format!("{}/chat/completions", cred.base_url.trim_end_matches('/'));
    let resp = ureq::post(&url)
        .set("authorization", &format!("Bearer {}", cred.api_key))
        .set("user-agent", USER_AGENT)
        .timeout(Duration::from_secs(25))
        .send_json(body)?;
    let v: serde_json::Value = resp.into_json()?;
    let text = v["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("")
        .trim()
        .to_string();
    Ok(text)
}

fn format_entry(e: &Entry) -> String {
    let when = rel_time(e.ts);
    let src = if e.source == "remote" {
        "from dashboard"
    } else {
        "locally"
    };
    let req = e
        .request
        .as_deref()
        .map(|r| format!(" \"{r}\""))
        .unwrap_or_default();
    let res = e
        .result
        .as_deref()
        .map(|r| format!(" — {r}"))
        .unwrap_or_default();
    format!("- {when}: {} {}{} [{}]{}", e.kind, src, req, e.status, res)
}

/// Coarse "x ago" so the model (and the fallback) can speak to recency without exact timestamps.
fn rel_time(ts: i64) -> String {
    let d = (crate::util::now() - ts).max(0);
    if d < 90 {
        "just now".into()
    } else if d < 3600 {
        format!("{}m ago", d / 60)
    } else if d < 86400 {
        format!("{}h ago", d / 3600)
    } else {
        format!("{}d ago", d / 86400)
    }
}

/// No-LLM fallback: a clean human-readable listing of the same entries.
fn plain_listing(entries: &[Entry]) -> String {
    let mut out =
        String::from("Recent activity (summary unavailable — showing the raw history):\n\n");
    for e in entries {
        out.push_str(&format_entry(e));
        out.push('\n');
    }
    out
}
