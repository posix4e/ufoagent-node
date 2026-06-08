//! Update check against the control plane's minimum version.

use anyhow::Result;

use crate::controlplane::ControlPlane;

pub fn version_tuple(s: &str) -> Vec<u32> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for ch in s.chars() {
        if ch.is_ascii_digit() {
            cur.push(ch);
        } else if !cur.is_empty() {
            out.push(cur.parse().unwrap_or(0));
            cur.clear();
        }
    }
    if !cur.is_empty() {
        out.push(cur.parse().unwrap_or(0));
    }
    out.truncate(3);
    if out.is_empty() {
        out.push(0);
    }
    out
}

pub fn needs_update(current: &str, minimum: &str) -> bool {
    version_tuple(current) < version_tuple(minimum)
}

pub struct UpdateStatus {
    pub current: String,
    pub min_version: String,
    pub update_required: bool,
}

pub fn check(cp: &ControlPlane, current: &str) -> Result<UpdateStatus> {
    let v = cp.version()?;
    let min = v
        .get("min_version")
        .and_then(|x| x.as_str())
        .unwrap_or("0.0.0")
        .to_string();
    Ok(UpdateStatus {
        update_required: needs_update(current, &min),
        current: current.to_string(),
        min_version: min,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_compares() {
        assert_eq!(version_tuple("0.2.1"), vec![0, 2, 1]);
        assert_eq!(version_tuple("v1.10.0-rc2"), vec![1, 10, 0]);
        assert!(needs_update("0.1.0", "0.2.0"));
        assert!(!needs_update("0.2.0", "0.2.0"));
        assert!(!needs_update("1.0.0", "0.9.9"));
    }
}
