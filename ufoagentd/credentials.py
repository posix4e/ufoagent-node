"""Credential refresh loop: keep UFO2's agents.yaml stocked with a valid short-lived key,
and heartbeat the control plane."""

from __future__ import annotations

import logging
import time
from pathlib import Path
from typing import Callable

from .controlplane import ControlPlane, Credential
from .ufo_config import write_agents_yaml

log = logging.getLogger("ufoagentd")


def refresh_once(cp: ControlPlane, ufo_home: str | Path) -> Credential:
    """Fetch a credential and write it into agents.yaml. Returns the credential."""
    cred = cp.get_credentials()
    write_agents_yaml(ufo_home, cred)
    log.info("Refreshed credential lease=%s model=%s expires_at=%s", cred.lease_id, cred.model, cred.expires_at)
    return cred


def run_daemon(
    cp: ControlPlane,
    ufo_home: str | Path,
    agent_version: str,
    platform: str,
    *,
    refresh_buffer: int = 120,
    min_sleep: int = 60,
    should_stop: Callable[[], bool] | None = None,
    sleep: Callable[[float], None] = time.sleep,
    now: Callable[[], float] = time.time,
    iterations: int | None = None,
) -> None:
    """Refresh-and-heartbeat loop. Runs forever unless `iterations` is set (testing) or
    `should_stop()` returns True."""
    count = 0
    while True:
        if should_stop and should_stop():
            return
        try:
            cred = refresh_once(cp, ufo_home)
            seconds_left = cred.expires_at - now()
            nap = max(min_sleep, seconds_left - refresh_buffer)
        except Exception as e:  # noqa: BLE001 — keep the daemon alive on transient errors
            log.warning("Credential refresh failed: %s", e)
            nap = min_sleep
        try:
            cp.heartbeat(agent_version, platform)
        except Exception as e:  # noqa: BLE001
            log.warning("Heartbeat failed: %s", e)

        count += 1
        if iterations is not None and count >= iterations:
            return
        if should_stop and should_stop():
            return
        sleep(nap)
