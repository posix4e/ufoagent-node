"""Provision Microsoft UFO2 into a managed home so the node can actually run tasks.

Clones microsoft/UFO, creates an isolated venv, installs its requirements, and records
`ufo_home` + the venv `python` in config. `ufoagentd run` then uses that python to invoke
`python -m ufo`. Heavy (UFO2 pulls torch via sentence-transformers) — runs once per node.
"""

from __future__ import annotations

import logging
import os
import subprocess
import sys
import urllib.request
import zipfile
from pathlib import Path

from . import store

log = logging.getLogger("ufoagentd")

UFO_GIT = "https://github.com/microsoft/UFO.git"
UFO_ZIP = "https://github.com/microsoft/UFO/archive/refs/heads/{ref}.zip"

# UFO2 pins a few packages with no Python 3.11 wheel, which then build from source and fail
# on modern setuptools (pkg_resources removed). Rewrite those pins to the nearest wheel-backed
# version before installing.
PIN_OVERRIDES = {
    "pandas==1.4.3": "pandas==1.5.3",  # 1.4.3 has no cp311 wheel
}


def _default_home() -> Path:
    return Path(str(store.config_dir())) / "ufo"


def find_base_python() -> str:
    """Locate a real (non-frozen) Python 3.10/3.11 to build the venv with."""
    cfg = store.load_config()
    if cfg.get("base_python") and Path(cfg["base_python"]).exists():
        return cfg["base_python"]
    candidates = [["py", "-3.11"], ["py", "-3.10"], ["py", "-3"], ["python"], ["python3"]]
    for cand in candidates:
        try:
            out = subprocess.run([*cand, "-c", "import sys;print(sys.executable)"],
                                 capture_output=True, text=True, timeout=20)
            if out.returncode == 0 and out.stdout.strip():
                return out.stdout.strip()
        except Exception:  # noqa: BLE001
            continue
    raise RuntimeError("No system Python found. Install Python 3.10/3.11 and re-run `ufoagentd bootstrap`.")


def _venv_python(venv_dir: Path) -> Path:
    return venv_dir / ("Scripts/python.exe" if os.name == "nt" else "bin/python")


def _fetch_ufo(home: Path, ref: str) -> None:
    """Clone UFO2 into `home` (git preferred; zip fallback if git is absent)."""
    if (home / "requirements.txt").exists():
        log.info("UFO2 source already present at %s", home)
        return
    git = _which("git")
    if git:
        log.info("git clone microsoft/UFO -> %s", home)
        subprocess.run([git, "clone", "--depth", "1", "--branch", ref, UFO_GIT, str(home)], check=True)
        return
    # zip fallback
    log.info("downloading UFO2 zip (%s)", ref)
    tmp = home.parent / f"ufo-{ref}.zip"
    urllib.request.urlretrieve(UFO_ZIP.format(ref=ref), tmp)  # noqa: S310
    with zipfile.ZipFile(tmp) as z:
        z.extractall(home.parent)
    extracted = home.parent / f"UFO-{ref}"
    extracted.replace(home)
    tmp.unlink(missing_ok=True)


def _which(name: str) -> str | None:
    from shutil import which
    return which(name)


def bootstrap(ufo_home: str | None = None, ref: str = "main") -> dict:
    """Provision UFO2; returns {ufo_home, python}. Idempotent."""
    home = Path(ufo_home or store.load_config().get("ufo_home") or _default_home())
    home.parent.mkdir(parents=True, exist_ok=True)

    _fetch_ufo(home, ref)

    base = find_base_python()
    venv_dir = home / ".venv"
    vpy = _venv_python(venv_dir)
    if not vpy.exists():
        log.info("creating venv at %s (base: %s)", venv_dir, base)
        subprocess.run([base, "-m", "venv", str(venv_dir)], check=True)

    # Patch known-bad pins to wheel-backed versions.
    text = (home / "requirements.txt").read_text(encoding="utf-8")
    for bad, good in PIN_OVERRIDES.items():
        text = text.replace(bad, good)
    patched = home / "requirements.ufoagent.txt"
    patched.write_text(text, encoding="utf-8")

    log.info("installing UFO2 requirements (this is large — torch etc.)…")
    # setuptools/wheel: not in fresh venvs by default and some deps import pkg_resources.
    subprocess.run([str(vpy), "-m", "pip", "install", "--upgrade", "pip", "setuptools", "wheel"], check=True)
    subprocess.run([str(vpy), "-m", "pip", "install", "-r", str(patched)], check=True)

    store.update_config(ufo_home=str(home), python=str(vpy))
    log.info("UFO2 provisioned: ufo_home=%s python=%s", home, vpy)
    return {"ufo_home": str(home), "python": str(vpy)}
