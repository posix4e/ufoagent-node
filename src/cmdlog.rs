//! On-node command history — the record of what this machine has done, both remote (commands
//! pushed from the dashboard over the WebSocket) and local (`ufoagent run` typed at the box). It's
//! an append-only JSON-lines file in the machine-wide config dir, kept small by trimming to the
//! last N entries. The `activity` command feeds this to an LLM for a friendly summary; it's the
//! structured history the raw `ufoagent.log` doesn't give cleanly. Cross-platform (plain files).

use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::config::config_dir;
use crate::util::{now, prefix_on_char_boundary};

/// One thing the agent did.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub ts: i64,        // epoch seconds
    pub source: String, // "local" | "remote"
    pub kind: String,   // refresh | repair | run_task | run
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<String>, // the task request, if any
    pub status: String, // done | failed
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>, // short result/error
}

/// Keep the file bounded — plenty for "what has this node been up to" without unbounded growth.
const MAX_ENTRIES: usize = 200;

fn log_path() -> PathBuf {
    config_dir().join("commands.jsonl")
}

/// Append a command to the history (stamping `ts`), trimming the file to the last MAX_ENTRIES.
/// Best-effort: never fails the caller's real work over a logging hiccup.
pub fn record(source: &str, kind: &str, request: Option<&str>, status: &str, result: Option<&str>) {
    let entry = Entry {
        ts: now(),
        source: source.to_string(),
        kind: kind.to_string(),
        request: request.map(str::to_string),
        status: status.to_string(),
        // Keep results short — this is a history line, not the full transcript.
        result: result.map(|r| truncate(r, 300)),
    };
    if let Err(e) = append(&entry) {
        log::warn!("cmdlog: could not record {kind}: {e}");
    }
}

fn append(entry: &Entry) -> Result<()> {
    let path = log_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut lines: Vec<String> = std::fs::read_to_string(&path)
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect();
    lines.push(serde_json::to_string(entry)?);
    if lines.len() > MAX_ENTRIES {
        lines.drain(0..lines.len() - MAX_ENTRIES);
    }
    std::fs::write(&path, lines.join("\n") + "\n")?;
    Ok(())
}

/// The most recent `n` entries, oldest-first.
pub fn recent(n: usize) -> Vec<Entry> {
    let data = match std::fs::read_to_string(log_path()) {
        Ok(d) => d,
        Err(_) => return Vec::new(),
    };
    let mut all: Vec<Entry> = data
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    if all.len() > n {
        all.drain(0..all.len() - n);
    }
    all
}

fn truncate(s: &str, max: usize) -> String {
    let p = prefix_on_char_boundary(s, max);
    if p.len() == s.len() {
        s.to_string()
    } else {
        format!("{p}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::with_temp_home;

    fn with_temp<T>(f: impl FnOnce() -> T) -> T {
        with_temp_home("ufolog", f)
    }

    #[test]
    fn records_and_reads_recent() {
        with_temp(|| {
            assert!(recent(10).is_empty());
            record("remote", "refresh", None, "done", Some("lease abc"));
            record("local", "run_task", Some("open notepad"), "done", None);
            let r = recent(10);
            assert_eq!(r.len(), 2);
            assert_eq!(r[0].kind, "refresh");
            assert_eq!(r[1].source, "local");
            assert_eq!(r[1].request.as_deref(), Some("open notepad"));
        });
    }

    #[test]
    fn trims_to_max() {
        with_temp(|| {
            for i in 0..(MAX_ENTRIES + 25) {
                record("remote", "refresh", None, "done", Some(&format!("n{i}")));
            }
            let r = recent(MAX_ENTRIES + 100);
            assert_eq!(r.len(), MAX_ENTRIES);
            // Oldest were trimmed; the last one is the newest.
            assert_eq!(
                r.last().unwrap().result.as_deref(),
                Some(&format!("n{}", MAX_ENTRIES + 24)[..])
            );
        });
    }
}
