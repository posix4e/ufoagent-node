//! Device-code linking: pair this machine to a tenant via a short code the operator approves.

use anyhow::{anyhow, bail, Result};
use std::time::{Duration, Instant};

use crate::controlplane::ControlPlane;

/// Runs the device-code flow. `announce(user_code, verification_uri)` is called once the code
/// is available (print it / show it in the UI). Returns (agent_id, agent_token) on approval.
pub fn link(
    cp: &ControlPlane,
    name: &str,
    platform: &str,
    announce: impl Fn(&str, &str),
    max_secs: u64,
) -> Result<(String, String)> {
    let start = cp.link_start(name, platform)?;
    let uri = start
        .verification_uri_complete
        .as_deref()
        .unwrap_or(&start.verification_uri);
    announce(&start.user_code, uri);

    let mut interval = start.interval.unwrap_or(5);
    let deadline = Instant::now() + Duration::from_secs(max_secs);
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_secs(interval));
        let poll = cp.link_poll(&start.device_code)?;
        match poll.status.as_str() {
            "approved" => {
                let tok = poll
                    .agent_token
                    .ok_or_else(|| anyhow!("approved but no token returned"))?;
                return Ok((poll.agent_id.unwrap_or_default(), tok));
            }
            "denied" => bail!("link request was denied"),
            "expired" => bail!("link request expired"),
            _ => {
                if let Some(i) = poll.interval {
                    interval = i;
                }
            }
        }
    }
    bail!("timed out waiting for approval")
}
