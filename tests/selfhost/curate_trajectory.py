#!/usr/bin/env python3
"""Build the public-site UFO trajectory artifact from real harvested UFO logs.

Input is C:\\e2e\\out\\ufo-trajectories copied from the self-hosted VM. Each child
directory is a snapshot of UFO's logs/<task>/ folder plus meta.json and, when
available, the control-plane trajectory response with overseer verdicts.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import textwrap
import time
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any

SCREENSHOT_KEYS = (
    "annotated_screenshot_path",
    "selected_control_screenshot_path",
    "concat_screenshot_path",
    "clean_screenshot_path",
    "desktop_screenshot_path",
)
BAD_VERDICTS = {"off_track", "hallucinating", "unsafe"}
MAX_CANDIDATES = 60
MAX_STEPS = 6


def load_json(path: Path, default: Any) -> Any:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except Exception:
        return default


def compact(value: Any, limit: int = 900) -> str:
    if value is None:
        return ""
    if isinstance(value, str):
        out = value
    else:
        out = json.dumps(value, ensure_ascii=True, sort_keys=True)
    out = re.sub(r"\s+", " ", out).strip()
    return out[:limit]


def first_text(obj: dict[str, Any], *keys: str) -> str:
    for key in keys:
        value = obj.get(key)
        if isinstance(value, str) and value.strip():
            return value.strip()
    return ""


def control_text(raw: Any) -> str:
    def one(item: Any) -> str:
        if not isinstance(item, dict):
            return ""
        for key in ("control_text", "target_name", "name"):
            value = item.get(key)
            if isinstance(value, str) and value.strip():
                return value.strip()
        return ""

    if isinstance(raw, dict):
        return one(raw)
    if isinstance(raw, list):
        for item in raw:
            text = one(item)
            if text:
                return text
    return ""


def find_frame(snapshot: Path, row: dict[str, Any]) -> Path | None:
    for key in SCREENSHOT_KEYS:
        raw = row.get(key)
        if not isinstance(raw, str) or not raw.strip():
            continue
        p = Path(raw.strip())
        candidates = []
        if p.is_absolute():
            candidates.append(p)
        candidates.extend([snapshot / p, snapshot / p.name])
        for candidate in candidates:
            if candidate.is_file():
                return candidate
        matches = list(snapshot.rglob(p.name))
        if matches:
            return matches[0]
    return None


def overseer_for_step(snapshot: Path, step: int) -> dict[str, str]:
    data = load_json(snapshot / "control-plane-trajectory.json", {})
    verdicts = data.get("verdicts") if isinstance(data, dict) else None
    if not isinstance(verdicts, list):
        return {}
    for verdict in verdicts:
        if not isinstance(verdict, dict):
            continue
        if verdict.get("at_step") == step:
            return {
                "verdict": compact(verdict.get("verdict"), 80),
                "action": compact(verdict.get("action"), 80),
                "reasoning": compact(verdict.get("reasoning"), 600),
            }
    return {}


def parse_snapshot(snapshot: Path) -> list[dict[str, Any]]:
    meta = load_json(snapshot / "meta.json", {})
    if not isinstance(meta, dict):
        meta = {}
    if meta.get("status") and meta.get("status") != "done":
        return []
    response_log = snapshot / "response.log"
    if not response_log.is_file():
        return []

    steps: list[dict[str, Any]] = []
    for idx, line in enumerate(response_log.read_text(encoding="utf-8", errors="replace").splitlines()):
        try:
            row = json.loads(line)
        except json.JSONDecodeError:
            continue
        if not isinstance(row, dict):
            continue
        step = row.get("session_step", row.get("round_step", row.get("Step", idx)))
        if not isinstance(step, int):
            step = idx
        frame = find_frame(snapshot, row)
        if frame is None:
            continue
        overseer = overseer_for_step(snapshot, step)
        if overseer.get("verdict") in BAD_VERDICTS:
            continue
        steps.append(
            {
                "id": f"{snapshot.name}:{step}",
                "phase": compact(meta.get("phase"), 80) or snapshot.name,
                "cmd_id": compact(meta.get("cmd_id"), 120),
                "round": row.get("round_num", row.get("Round", 0)),
                "step": step,
                "agent_type": compact(row.get("agent_type"), 80),
                "status": compact(row.get("status"), 80),
                "observation": compact(first_text(row, "result", "results"), 900),
                "thought": compact(row.get("thought"), 900),
                "plan": compact(row.get("plan"), 900),
                "control_text": compact(control_text(row.get("control_log")), 240),
                "action": compact(row.get("action_representation") or row.get("function_call") or row.get("action"), 900),
                "overseer": overseer,
                "frame": str(frame),
            }
        )
    return steps


def candidate_score(step: dict[str, Any]) -> int:
    score = 0
    for key in ("observation", "thought", "plan", "action", "control_text"):
        if step.get(key):
            score += 1
    if step.get("status") == "FINISH":
        score += 2
    return score


def load_candidates(root: Path) -> list[dict[str, Any]]:
    all_steps: list[dict[str, Any]] = []
    for snapshot in sorted(p for p in root.iterdir() if p.is_dir()):
        all_steps.extend(parse_snapshot(snapshot))
    if len(all_steps) <= MAX_CANDIDATES:
        return all_steps
    by_phase: dict[str, list[dict[str, Any]]] = {}
    for step in sorted(all_steps, key=candidate_score, reverse=True):
        by_phase.setdefault(str(step.get("phase") or "run"), []).append(step)
    out: list[dict[str, Any]] = []
    while len(out) < MAX_CANDIDATES and any(by_phase.values()):
        for phase in sorted(by_phase):
            if by_phase[phase] and len(out) < MAX_CANDIDATES:
                out.append(by_phase[phase].pop(0))
    return out


def yaml_unescape(raw: str) -> str:
    return raw.replace(r"\\", "\\").replace(r"\"", '"')


def read_llm_config(path: Path) -> tuple[str, str, str]:
    text = path.read_text(encoding="utf-8", errors="replace")

    def field(name: str) -> str:
        m = re.search(rf"{name}:\s*\"((?:\\.|[^\"])*)\"", text)
        return yaml_unescape(m.group(1)) if m else ""

    base = os.environ.get("UFOAGENT_LLM_BASE_URL") or field("API_BASE")
    key = os.environ.get("UFOAGENT_LLM_API_KEY") or field("API_KEY")
    model = os.environ.get("UFOAGENT_LLM_MODEL") or field("API_MODEL")
    if not base or not key or not model:
        raise SystemExit("missing LLM credentials; expected UFO agents.yaml or UFOAGENT_LLM_* env vars")
    return base.rstrip("/"), key, model


def call_llm(base: str, key: str, model: str, candidates: list[dict[str, Any]]) -> list[dict[str, str]]:
    prompt_candidates = [
        {k: v for k, v in step.items() if k != "frame"}
        for step in candidates
    ]
    schema = {
        "name": "ufo_marketing_trajectory",
        "strict": True,
        "schema": {
            "type": "object",
            "additionalProperties": False,
            "properties": {
                "steps": {
                    "type": "array",
                    "minItems": 3,
                    "maxItems": MAX_STEPS,
                    "items": {
                        "type": "object",
                        "additionalProperties": False,
                        "properties": {
                            "id": {"type": "string"},
                            "label": {"type": "string"},
                            "caption": {"type": "string"},
                        },
                        "required": ["id", "label", "caption"],
                    },
                }
            },
            "required": ["steps"],
        },
    }
    body = {
        "model": model,
        "messages": [
            {
                "role": "system",
                "content": (
                    "You curate a public UFOAgent demo from real UFO trajectory rows. "
                    "Pick the few strongest steps that tell the story across the run. "
                    "Captions must be grounded only in the row's observation, thought, plan, "
                    "control, action, status, and overseer fields. Do not say a task succeeded "
                    "unless the row itself supports that. Do not invent apps, outcomes, or praise."
                ),
            },
            {
                "role": "user",
                "content": json.dumps(
                    {
                        "goal": "Choose 3-6 real steps for a captioned marketing trajectory.",
                        "candidates": prompt_candidates,
                    },
                    ensure_ascii=True,
                ),
            },
        ],
        "response_format": {"type": "json_schema", "json_schema": schema},
    }
    req = urllib.request.Request(
        f"{base}/chat/completions",
        data=json.dumps(body).encode("utf-8"),
        headers={"authorization": f"Bearer {key}", "content-type": "application/json"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(req, timeout=120) as resp:
            data = json.loads(resp.read().decode("utf-8"))
    except urllib.error.HTTPError as e:
        detail = e.read().decode("utf-8", errors="replace")[:600]
        raise SystemExit(f"LLM curation failed: HTTP {e.code} {detail}") from e
    content = data["choices"][0]["message"]["content"]
    parsed = json.loads(content)
    return parsed.get("steps", [])


def validate_selection(selection: list[dict[str, str]], candidates: list[dict[str, Any]]) -> list[dict[str, Any]]:
    by_id = {step["id"]: step for step in candidates}
    out: list[dict[str, Any]] = []
    seen: set[str] = set()
    for item in selection:
        step_id = item.get("id", "")
        if step_id in seen or step_id not in by_id:
            continue
        seen.add(step_id)
        step = dict(by_id[step_id])
        step["label"] = compact(item.get("label"), 48) or step["phase"]
        step["caption"] = compact(item.get("caption"), 180)
        if step["caption"]:
            out.append(step)
    if len(out) < 3:
        raise SystemExit("LLM curation returned fewer than 3 usable real steps")
    return out[:MAX_STEPS]


def wrap_caption(label: str, caption: str, status: str) -> str:
    lines = [label.upper()]
    lines.extend(textwrap.wrap(caption, width=58)[:3])
    if status:
        lines.append(f"status: {status}")
    return "\n".join(lines)


def run(cmd: list[str]) -> None:
    subprocess.run(cmd, check=True)


def render_artifact(selected: list[dict[str, Any]], out_dir: Path) -> None:
    if not shutil.which("ffmpeg"):
        raise SystemExit("ffmpeg is required to render the trajectory artifact")
    out_dir.mkdir(parents=True, exist_ok=True)
    tmp = out_dir / "ufo-trajectory-frames"
    if tmp.exists():
        shutil.rmtree(tmp)
    tmp.mkdir()

    rendered: list[Path] = []
    for idx, step in enumerate(selected):
        caption = wrap_caption(step["label"], step["caption"], step.get("status", ""))
        caption_path = tmp / f"{idx:04d}.txt"
        caption_path.write_text(caption, encoding="utf-8")
        frame_out = tmp / f"{idx:04d}.png"
        vf = (
            "scale=900:460:force_original_aspect_ratio=decrease,"
            "pad=900:620:(ow-iw)/2:20:color=0x0b0710,"
            "drawbox=x=0:y=482:w=900:h=138:color=0x160e29@0.94:t=fill,"
            f"drawtext=textfile={caption_path}:fontcolor=0xfff6ec:fontsize=24:"
            "line_spacing=7:x=26:y=498"
        )
        run(["ffmpeg", "-y", "-i", step["frame"], "-vf", vf, "-frames:v", "1", str(frame_out)])
        rendered.append(frame_out)

    shutil.copyfile(rendered[0], out_dir / "ufo-trajectory.png")
    run(
        [
            "ffmpeg",
            "-y",
            "-framerate",
            "0.7",
            "-i",
            str(tmp / "%04d.png"),
            "-vf",
            "split[a][b];[a]palettegen=max_colors=128[p];[b][p]paletteuse=dither=bayer:bayer_scale=3",
            "-loop",
            "0",
            str(out_dir / "ufo-trajectory.gif"),
        ]
    )


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--input", required=True, type=Path)
    ap.add_argument("--agents-yaml", required=True, type=Path)
    ap.add_argument("--out", required=True, type=Path)
    args = ap.parse_args()

    candidates = load_candidates(args.input)
    if len(candidates) < 3:
        raise SystemExit(f"not enough real trajectory steps with frames: {len(candidates)}")
    base, key, model = read_llm_config(args.agents_yaml)
    selection = validate_selection(call_llm(base, key, model, candidates), candidates)
    render_artifact(selection, args.out)
    manifest = {
        "generated_at": int(time.time()),
        "model": model,
        "source": "UFO response.log snapshots from self-hosted e2e",
        "steps": [
            {k: v for k, v in step.items() if k != "frame"}
            for step in selection
        ],
        "candidate_count": len(candidates),
    }
    (args.out / "ufo-trajectory.json").write_text(json.dumps(manifest, indent=2), encoding="utf-8")


if __name__ == "__main__":
    main()
