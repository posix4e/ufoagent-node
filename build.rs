//! Capture a short build identifier at compile time so the running agent can report *exactly* which
//! build it is (dashboard + logs) — `CARGO_PKG_VERSION` alone can't distinguish two builds of the
//! same version. Prefers CI's `$GITHUB_SHA`, falls back to `git rev-parse`, else "dev".
//! Exposed to the crate as `env!("UFOAGENT_GIT_SHA")`.

use std::process::Command;

fn main() {
    let sha = std::env::var("GITHUB_SHA")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(git_short_sha)
        .map(|s| s.chars().take(12).collect::<String>())
        .unwrap_or_else(|| "dev".to_string());
    println!("cargo:rustc-env=UFOAGENT_GIT_SHA={sha}");

    // Re-run when HEAD moves so the embedded sha stays current across local rebuilds.
    println!("cargo:rerun-if-env-changed=GITHUB_SHA");
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs");
}

fn git_short_sha() -> Option<String> {
    let out = Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!s.is_empty()).then_some(s)
}
