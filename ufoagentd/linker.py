"""Device-code linking: pair this machine to a tenant via a short user code the operator
approves in the dashboard."""

from __future__ import annotations

import time
from typing import Callable

from .controlplane import ControlPlane

Announce = Callable[[str, str], None]  # (user_code, verification_uri)


class LinkError(Exception):
    pass


def link(
    cp: ControlPlane,
    agent_name: str,
    platform: str,
    announce: Announce,
    *,
    sleep: Callable[[float], None] = time.sleep,
    max_seconds: float = 600.0,
) -> tuple[str, str]:
    """Run the full flow; returns (agent_id, agent_token). Raises LinkError on denial/timeout."""
    start = cp.link_start(agent_name, platform)
    device_code = start["device_code"]
    interval = float(start.get("interval", 5))
    announce(start["user_code"], start.get("verification_uri_complete", start["verification_uri"]))

    deadline = time.monotonic() + max_seconds
    while time.monotonic() < deadline:
        sleep(interval)
        res = cp.link_poll(device_code)
        status = res.get("status")
        if status == "approved":
            return res["agent_id"], res["agent_token"]
        if status == "denied":
            raise LinkError("Link request was denied")
        if status == "expired":
            raise LinkError("Link request expired before approval")
        # 'pending' / 'claimed' → keep waiting; honor a server-suggested interval
        interval = float(res.get("interval", interval))
    raise LinkError("Timed out waiting for approval")
