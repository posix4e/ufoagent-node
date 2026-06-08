//! System-tray manager UI (status, link/repair, run-task, logs).
//! Stubbed in this build; the Windows tray (tray-icon + event loop) lands in the next iteration,
//! verified via Windows CI. CLI `link`/`repair`/`run` already cover the same actions headlessly.

use anyhow::Result;

pub fn run(_version: &str) -> Result<()> {
    anyhow::bail!("the tray manager is not implemented in this build yet — use the `link`, `repair`, and `run` commands")
}
