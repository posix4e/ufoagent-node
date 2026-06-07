"""Self-update: compare the running version against the control-plane minimum and the latest
signed GitHub release; on Windows, download + verify + silently install the new installer."""

from __future__ import annotations

import json
import logging
import re
import subprocess
import sys
import tempfile
import urllib.request
from pathlib import Path
from typing import Any

from . import __version__
from .controlplane import ControlPlane

log = logging.getLogger("ufoagentd")

# TODO: set to the published public repo before release.
GITHUB_REPO = "posix4e/ufoagent-node"


def version_tuple(s: str) -> tuple[int, ...]:
    nums = re.findall(r"\d+", s or "")
    return tuple(int(n) for n in nums[:3]) or (0,)


def needs_update(current: str, minimum: str) -> bool:
    return version_tuple(current) < version_tuple(minimum)


def check(cp: ControlPlane, current: str = __version__) -> dict[str, Any]:
    info = cp.version()
    minimum = info.get("min_version", "0.0.0")
    return {"current": current, "min_version": minimum, "update_required": needs_update(current, minimum)}


def latest_release_asset() -> tuple[str, str] | None:
    """Return (tag, installer_download_url) for the latest GitHub release, or None."""
    url = f"https://api.github.com/repos/{GITHUB_REPO}/releases/latest"
    req = urllib.request.Request(url, headers={"accept": "application/vnd.github+json", "user-agent": "ufoagentd"})
    try:
        with urllib.request.urlopen(req, timeout=30) as resp:
            rel = json.loads(resp.read().decode("utf-8"))
    except Exception as e:  # noqa: BLE001
        log.warning("Could not query latest release: %s", e)
        return None
    for asset in rel.get("assets", []):
        name = asset.get("name", "")
        if name.endswith(".exe"):
            return rel.get("tag_name", ""), asset["browser_download_url"]
    return None


def apply_update() -> bool:
    """Download + silently install the latest signed installer. Windows only."""
    if sys.platform != "win32":
        raise RuntimeError("Auto-update install is only supported on Windows")
    found = latest_release_asset()
    if not found:
        log.info("No installer asset found; nothing to update")
        return False
    tag, dl = found
    dest = Path(tempfile.gettempdir()) / "ufoagent-setup.exe"
    log.info("Downloading %s -> %s", tag, dest)
    urllib.request.urlretrieve(dl, dest)  # noqa: S310 — GitHub release over HTTPS
    if not _verify_authenticode(dest):
        log.error("Authenticode verification failed for %s; aborting update", dest)
        return False
    # Inno Setup silent install; closes/relaunches the service.
    subprocess.run([str(dest), "/VERYSILENT", "/NORESTART"], check=True)
    return True


def _verify_authenticode(path: Path) -> bool:
    """Verify the downloaded installer is validly signed (Windows)."""
    ps = (
        f"$sig = Get-AuthenticodeSignature -FilePath '{path}'; "
        "if ($sig.Status -eq 'Valid') { exit 0 } else { exit 1 }"
    )
    try:
        return subprocess.run(["powershell", "-NoProfile", "-Command", ps], check=False).returncode == 0
    except Exception:  # noqa: BLE001
        return False
