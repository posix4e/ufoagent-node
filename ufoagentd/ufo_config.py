"""Render and write UFO2's ``config/ufo/agents.yaml`` from a vended credential.

UFO2 reads HOST_AGENT / APP_AGENT sections; with ``API_TYPE: openai`` it treats ``API_BASE`` as
the OpenAI-compatible base URL and sends ``Authorization: Bearer <API_KEY>``. We write the
short-lived vended key directly; ufoagentd rewrites this file as the lease is refreshed.
"""

from __future__ import annotations

from pathlib import Path
from typing import Any

from .controlplane import Credential

_HEADER = (
    "# Managed by ufoagentd — do not edit by hand.\n"
    "# The API_KEY below is a short-lived credential refreshed by the UFOAgent daemon.\n"
)


def _yaml_scalar(v: Any) -> str:
    if isinstance(v, bool):
        return "true" if v else "false"
    if isinstance(v, (int, float)):
        return str(v)
    s = str(v).replace("\\", "\\\\").replace('"', '\\"')
    return f'"{s}"'


def _emit(obj: dict[str, Any], indent: int = 0) -> str:
    pad = "  " * indent
    out = []
    for key, val in obj.items():
        if isinstance(val, dict):
            out.append(f"{pad}{key}:")
            out.append(_emit(val, indent + 1))
        else:
            out.append(f"{pad}{key}: {_yaml_scalar(val)}")
    return "\n".join(out)


def _agent_section(cred: Credential) -> dict[str, Any]:
    return {
        "VISUAL_MODE": True,
        "API_TYPE": "openai",
        "API_BASE": cred.base_url,
        "API_KEY": cred.api_key,
        "API_MODEL": cred.model,
    }


def render_agents_yaml(cred: Credential) -> str:
    doc = {"HOST_AGENT": _agent_section(cred), "APP_AGENT": _agent_section(cred)}
    return _HEADER + _emit(doc) + "\n"


def agents_yaml_path(ufo_home: str | Path) -> Path:
    return Path(ufo_home) / "config" / "ufo" / "agents.yaml"


def write_agents_yaml(ufo_home: str | Path, cred: Credential) -> Path:
    path = agents_yaml_path(ufo_home)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(render_agents_yaml(cred), encoding="utf-8")
    return path
