"""Thin stdlib HTTP client for the UFOAgent control plane API."""

from __future__ import annotations

import json
import urllib.error
import urllib.request
from dataclasses import dataclass
from typing import Any

from . import __version__


class ControlPlaneError(Exception):
    def __init__(self, status: int, code: str, message: str):
        super().__init__(f"{status} {code}: {message}")
        self.status = status
        self.code = code


@dataclass
class Credential:
    provider: str
    base_url: str
    model: str
    api_key: str
    expires_at: int
    lease_id: str

    @staticmethod
    def from_json(d: dict[str, Any]) -> "Credential":
        return Credential(
            provider=d["provider"], base_url=d["base_url"], model=d["model"],
            api_key=d["api_key"], expires_at=int(d["expires_at"]), lease_id=d["lease_id"],
        )


class ControlPlane:
    def __init__(self, base_url: str, token: str | None = None, timeout: float = 30.0):
        self.base_url = base_url.rstrip("/")
        self.token = token
        self.timeout = timeout

    def _request(self, method: str, path: str, body: dict | None = None, auth: bool = False) -> dict[str, Any]:
        url = f"{self.base_url}{path}"
        data = json.dumps(body).encode("utf-8") if body is not None else None
        # Explicit User-Agent: the default "Python-urllib/x" is blocked by Cloudflare's bot rules.
        headers = {"accept": "application/json", "user-agent": f"ufoagentd/{__version__}"}
        if data is not None:
            headers["content-type"] = "application/json"
        if auth:
            if not self.token:
                raise ControlPlaneError(401, "no_token", "Agent is not linked (no token)")
            headers["authorization"] = f"Bearer {self.token}"
        req = urllib.request.Request(url, data=data, headers=headers, method=method)
        try:
            with urllib.request.urlopen(req, timeout=self.timeout) as resp:
                return json.loads(resp.read().decode("utf-8") or "{}")
        except urllib.error.HTTPError as e:
            payload: dict[str, Any] = {}
            try:
                payload = json.loads(e.read().decode("utf-8"))
            except Exception:
                pass
            raise ControlPlaneError(e.code, payload.get("error", "http_error"), payload.get("message", str(e)))
        except urllib.error.URLError as e:
            raise ControlPlaneError(0, "network_error", str(e.reason))

    # ---- agent endpoints ----
    def version(self) -> dict[str, Any]:
        return self._request("GET", "/v1/agent/version")

    def link_start(self, agent_name: str, platform: str) -> dict[str, Any]:
        return self._request("POST", "/v1/link/start", {"agent_name": agent_name, "platform": platform})

    def link_poll(self, device_code: str) -> dict[str, Any]:
        return self._request("POST", "/v1/link/poll", {"device_code": device_code})

    def get_credentials(self) -> Credential:
        return Credential.from_json(self._request("GET", "/v1/credentials", auth=True))

    def heartbeat(self, agent_version: str, platform: str) -> dict[str, Any]:
        return self._request("POST", "/v1/heartbeat", {"agent_version": agent_version, "platform": platform}, auth=True)
