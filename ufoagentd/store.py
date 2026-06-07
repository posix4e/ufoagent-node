"""Local configuration + agent-token storage.

Config (control-plane URL, UFO2 home) lives in a JSON file. The agent bearer token is stored
separately and, on Windows, encrypted with DPAPI (CryptProtectData) so only this machine/user
can read it. On other platforms (dev) it falls back to a 0600 plaintext file.
"""

from __future__ import annotations

import json
import os
import sys
from pathlib import Path
from typing import Any


def config_dir() -> Path:
    """Per-machine config dir. Override with UFOAGENT_HOME (used by tests / non-Windows dev)."""
    override = os.environ.get("UFOAGENT_HOME")
    if override:
        return Path(override)
    if sys.platform == "win32":
        base = os.environ.get("ProgramData", r"C:\ProgramData")
        return Path(base) / "UFOAgent"
    return Path.home() / ".config" / "ufoagent"


def _config_path() -> Path:
    return config_dir() / "config.json"


def _token_path() -> Path:
    return config_dir() / "agent.token"


def load_config() -> dict[str, Any]:
    p = _config_path()
    if not p.exists():
        return {}
    return json.loads(p.read_text(encoding="utf-8"))


def save_config(cfg: dict[str, Any]) -> None:
    d = config_dir()
    d.mkdir(parents=True, exist_ok=True)
    _config_path().write_text(json.dumps(cfg, indent=2), encoding="utf-8")


def update_config(**fields: Any) -> dict[str, Any]:
    cfg = load_config()
    cfg.update({k: v for k, v in fields.items() if v is not None})
    save_config(cfg)
    return cfg


# ---- token storage ----

def _dpapi_protect(data: bytes) -> bytes | None:
    try:
        import win32crypt  # type: ignore

        return win32crypt.CryptProtectData(data, "ufoagent", None, None, None, 0)
    except Exception:
        return None


def _dpapi_unprotect(data: bytes) -> bytes | None:
    try:
        import win32crypt  # type: ignore

        _, plain = win32crypt.CryptUnprotectData(data, None, None, None, 0)
        return plain
    except Exception:
        return None


def set_token(token: str) -> None:
    d = config_dir()
    d.mkdir(parents=True, exist_ok=True)
    raw = token.encode("utf-8")
    protected = _dpapi_protect(raw) if sys.platform == "win32" else None
    path = _token_path()
    if protected is not None:
        path.write_bytes(b"DPAPI\x00" + protected)
    else:
        path.write_bytes(b"PLAIN\x00" + raw)
    try:
        os.chmod(path, 0o600)
    except OSError:
        pass


def get_token() -> str | None:
    path = _token_path()
    if not path.exists():
        return None
    blob = path.read_bytes()
    if blob.startswith(b"DPAPI\x00"):
        plain = _dpapi_unprotect(blob[6:])
        return plain.decode("utf-8") if plain else None
    if blob.startswith(b"PLAIN\x00"):
        return blob[6:].decode("utf-8")
    return blob.decode("utf-8")  # legacy/raw


def clear_token() -> None:
    p = _token_path()
    if p.exists():
        p.unlink()
