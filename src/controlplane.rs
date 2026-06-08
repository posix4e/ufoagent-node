//! Thin HTTP client for the UFOAgent control plane API (ureq, sync).

use anyhow::{anyhow, bail, Result};
use serde::Deserialize;

/// Explicit User-Agent: the default agent string is blocked by Cloudflare's bot rules (403).
pub const USER_AGENT: &str = concat!("ufoagent/", env!("CARGO_PKG_VERSION"));

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct Credential {
    pub provider: String,
    pub base_url: String,
    pub model: String,
    pub api_key: String,
    pub expires_at: i64,
    pub lease_id: String,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct LinkStart {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    #[serde(default)]
    pub verification_uri_complete: Option<String>,
    #[serde(default)]
    pub interval: Option<u64>,
    #[serde(default)]
    pub expires_in: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct Heartbeat {
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub min_version: Option<String>,
    #[serde(default)]
    pub server_time: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct LinkPoll {
    pub status: String,
    #[serde(default)]
    pub agent_id: Option<String>,
    #[serde(default)]
    pub agent_token: Option<String>,
    #[serde(default)]
    pub interval: Option<u64>,
}

pub struct ControlPlane {
    base: String,
    token: Option<String>,
}

impl ControlPlane {
    pub fn new(base: &str, token: Option<String>) -> Self {
        ControlPlane {
            base: base.trim_end_matches('/').to_string(),
            token,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    fn get_json<T: for<'de> Deserialize<'de>>(&self, path: &str, auth: bool) -> Result<T> {
        let mut req = ureq::get(&self.url(path))
            .set("user-agent", USER_AGENT)
            .set("accept", "application/json");
        if auth {
            req = req.set("authorization", &format!("Bearer {}", self.bearer()?));
        }
        Self::send(req.call())
    }

    fn post_json<T: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        body: serde_json::Value,
        auth: bool,
    ) -> Result<T> {
        let mut req = ureq::post(&self.url(path))
            .set("user-agent", USER_AGENT)
            .set("accept", "application/json");
        if auth {
            req = req.set("authorization", &format!("Bearer {}", self.bearer()?));
        }
        Self::send(req.send_json(body))
    }

    fn bearer(&self) -> Result<&str> {
        self.token
            .as_deref()
            .ok_or_else(|| anyhow!("agent is not linked (no token)"))
    }

    fn send<T: for<'de> Deserialize<'de>>(
        resp: std::result::Result<ureq::Response, ureq::Error>,
    ) -> Result<T> {
        match resp {
            Ok(r) => Ok(r.into_json::<T>()?),
            Err(ureq::Error::Status(code, r)) => {
                let body = r.into_string().unwrap_or_default();
                bail!(
                    "control plane HTTP {}: {}",
                    code,
                    body.chars().take(300).collect::<String>()
                )
            }
            Err(e) => bail!("control plane request failed: {}", e),
        }
    }

    pub fn version(&self) -> Result<serde_json::Value> {
        self.get_json("/v1/agent/version", false)
    }

    pub fn link_start(&self, agent_name: &str, platform: &str) -> Result<LinkStart> {
        self.post_json(
            "/v1/link/start",
            serde_json::json!({ "agent_name": agent_name, "platform": platform }),
            false,
        )
    }

    pub fn link_poll(&self, device_code: &str) -> Result<LinkPoll> {
        self.post_json(
            "/v1/link/poll",
            serde_json::json!({ "device_code": device_code }),
            false,
        )
    }

    pub fn get_credentials(&self) -> Result<Credential> {
        self.get_json("/v1/credentials", true)
    }

    pub fn heartbeat(&self, agent_version: &str, platform: &str) -> Result<Heartbeat> {
        self.post_json(
            "/v1/heartbeat",
            serde_json::json!({ "agent_version": agent_version, "platform": platform }),
            true,
        )
    }
}
