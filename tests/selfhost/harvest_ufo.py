#!/usr/bin/env python3
"""Harvest UFO trajectory logs and build website-contract assets."""

from __future__ import annotations

import argparse
import base64
import json
import os
import re
import shutil
import subprocess
import sys
from pathlib import Path
from typing import Any

SCREENSHOT_KEYS = [
    "clean_screenshot_path",
    "annotated_screenshot_path",
    "concat_screenshot_path",
    "selected_control_screenshot_path",
]
PREFERRED_ASSET_KEYS = [
    "annotated_screenshot_path",
    "selected_control_screenshot_path",
]
TRACE_KEYS = [
    "request",
    "observation",
    "thought",
    "plan",
    "comment",
    "ControlLabel",
    "ControlText",
    "Function",
    "Args",
    "status",
    "agent_type",
    "Round",
    "Step",
    "session_step",
    "subtask",
    "action",
    "error",
]


def normalize_text(value: Any) -> str:
    if value is None:
        return ""
    if not isinstance(value, str):
        value = str(value)
    return re.sub(r"\s+", " ", value).strip()


def expected_request(args: argparse.Namespace) -> str:
    if getattr(args, "request_file", ""):
        return Path(args.request_file).read_text(encoding="utf-8", errors="ignore")
    return getattr(args, "request", "")


def extract_request_values(value: Any) -> list[str]:
    if value is None:
        return []
    if isinstance(value, str):
        values = [value]
        stripped = value.strip()
        if stripped.startswith(("{", "[")):
            try:
                values.extend(extract_request_values(json.loads(stripped)))
            except json.JSONDecodeError:
                pass
        return values
    if isinstance(value, dict):
        values: list[str] = []
        for key, child in value.items():
            if key.startswith("request"):
                values.extend(extract_request_values(child))
            elif isinstance(child, (dict, list)):
                values.extend(extract_request_values(child))
        return values
    if isinstance(value, list):
        values: list[str] = []
        for child in value:
            values.extend(extract_request_values(child))
        return values
    return [str(value)]


def row_matches_request(row: dict[str, Any], request: str) -> bool:
    expected = normalize_text(request)
    if not expected:
        return True
    for value in extract_request_values(row.get("request")):
        if normalize_text(value) == expected:
            return True
    return False


def filter_rows_by_request(
    rows: list[dict[str, Any]], request: str
) -> tuple[list[dict[str, Any]], list[int]]:
    if not normalize_text(request):
        return rows, list(range(len(rows)))
    matched: list[dict[str, Any]] = []
    indices: list[int] = []
    for index, row in enumerate(rows):
        if row_matches_request(row, request):
            matched.append(row)
            indices.append(index)
    return matched, indices


def read_json_lines(path: Path) -> list[dict[str, Any]]:
    if not path.exists():
        return []
    rows: list[dict[str, Any]] = []
    for line in path.read_text(encoding="utf-8", errors="ignore").splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            obj = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(obj, dict):
            rows.append(obj)
    return rows


def write_json_lines(path: Path, rows: list[dict[str, Any]]) -> None:
    with path.open("w", encoding="utf-8") as f:
        for row in rows:
            f.write(json.dumps(row) + "\n")


def decode_request_images(steps: list[dict[str, Any]], log_dir: Path) -> list[Path]:
    images: list[Path] = []
    for step in steps:
        step_no = step.get("step", step.get("Step", len(images)))
        for idx, value in enumerate(step.get("image_list") or []):
            if not isinstance(value, str) or not value.startswith("data:image/png;base64,"):
                continue
            out = log_dir / f"request_step{step_no}_{idx}.png"
            if not out.exists():
                out.write_bytes(base64.b64decode(value.split(",", 1)[1]))
            images.append(out)
    return images


def load_trajectory(log_dir: Path, ufo_home: Path) -> tuple[list[dict[str, Any]], dict[str, Any]]:
    sys.path.insert(0, str(ufo_home))
    meta: dict[str, Any] = {"parser": "ufo.trajectory.parser.Trajectory"}
    try:
        from ufo.trajectory.parser import Trajectory  # type: ignore

        traj = Trajectory(str(log_dir))
        steps = list(traj.step_log)
        meta["parser_ok"] = True
        meta["step_count"] = len(steps)
        return steps, meta
    except Exception as exc:  # noqa: BLE001 - exported for diagnostics
        meta["parser_ok"] = False
        meta["parser_error"] = repr(exc)
        return [], meta


def normalize_action(action: Any) -> Any:
    if isinstance(action, list):
        return [
            {
                k: v
                for k, v in item.items()
                if k in {"action_string", "function", "arguments", "result", "status", "error"}
            }
            for item in action
            if isinstance(item, dict)
        ]
    if isinstance(action, dict):
        return {
            k: v
            for k, v in action.items()
            if k in {"action_string", "function", "arguments", "result", "status", "error"}
        }
    return action


def normalize_steps(steps: list[dict[str, Any]]) -> list[dict[str, Any]]:
    out: list[dict[str, Any]] = []
    for i, step in enumerate(steps):
        row: dict[str, Any] = {"index": i}
        for key in TRACE_KEYS:
            if key not in step:
                continue
            value = step[key]
            if key == "action":
                value = normalize_action(value)
            if value not in (None, "", [], {}):
                row[key] = value
        screenshots = {}
        for key in SCREENSHOT_KEYS:
            value = step.get(key)
            if isinstance(value, str) and value.strip():
                screenshots[key] = os.path.basename(value)
        if screenshots:
            row["screenshots"] = screenshots
        out.append(row)
    return out


def transcript_excerpt(path: Path) -> str:
    if not path.exists():
        return ""
    text = path.read_text(encoding="utf-8", errors="ignore")
    text = re.sub(r"\s+", " ", text).strip()
    return text[-8000:]


def copy_tree(src: Path, dst: Path) -> None:
    if dst.exists():
        shutil.rmtree(dst)
    shutil.copytree(src, dst)


def isolate_steps(
    steps: list[dict[str, Any]],
    response_rows: list[dict[str, Any]],
    request: str,
) -> list[dict[str, Any]]:
    if not normalize_text(request):
        return steps
    if not steps:
        return []
    direct, _ = filter_rows_by_request(steps, request)
    if direct:
        return direct
    if len(steps) == len(response_rows):
        return steps
    raise SystemExit(
        "could not isolate UFO trajectory for request: parser rows do not expose "
        "request text and do not align with response.log"
    )


def prune_unreferenced_pngs(log_dir: Path, rows: list[dict[str, Any]]) -> None:
    keep: set[str] = set()
    for row in rows:
        screenshots = row.get("screenshots") or {}
        for filename in screenshots.values():
            if isinstance(filename, str) and filename:
                keep.add(os.path.basename(filename))
    if not keep:
        return
    for path in log_dir.glob("*.png"):
        if path.name not in keep:
            path.unlink(missing_ok=True)


def export(args: argparse.Namespace) -> None:
    root = Path(args.out)
    label = args.label
    log_src = Path(args.ufo_home) / "logs" / args.log_name
    if not log_src.is_dir():
        raise SystemExit(f"UFO log directory missing: {log_src}")
    request = expected_request(args)

    log_dst = root / "logs" / label
    copy_tree(log_src, log_dst)
    response_rows = read_json_lines(log_dst / "response.log")
    request_rows = read_json_lines(log_dst / "request.log")
    evaluation_rows = read_json_lines(log_dst / "evaluation.log")
    response_rows, _ = filter_rows_by_request(response_rows, request)
    request_rows, _ = filter_rows_by_request(request_rows, request)
    evaluation_rows, _ = filter_rows_by_request(evaluation_rows, request)
    if normalize_text(request) and not response_rows:
        raise SystemExit(f"UFO response.log has no rows for requested task: {label}")
    write_json_lines(log_dst / "response.log", response_rows)
    write_json_lines(log_dst / "request.log", request_rows)
    if evaluation_rows:
        write_json_lines(log_dst / "evaluation.log", evaluation_rows)
    output = log_dst / "output.md"
    if output.exists():
        output.write_text(
            f"# Harvested UFO Trajectory\n\nLabel: {label}\n\n"
            "This output was isolated by ufoagent for one run_task request.\n",
            encoding="utf-8",
        )

    transcript_src = Path(os.environ.get("ProgramData", r"C:\ProgramData")) / "UFOAgent" / "tasks" / "logs" / f"{args.task_id}.txt"
    transcript_dst = root / "transcripts" / f"{label}.txt"
    transcript_dst.parent.mkdir(parents=True, exist_ok=True)
    if transcript_src.exists():
        shutil.copy2(transcript_src, transcript_dst)

    steps, meta = load_trajectory(log_dst, Path(args.ufo_home))
    steps = isolate_steps(steps, response_rows, request)
    fallback_used = False
    if not steps:
        steps = response_rows
    if not steps:
        steps = request_rows
        fallback_used = bool(steps)
        decode_request_images(steps, log_dst)

    trace = {
        "label": label,
        "task_id": args.task_id,
        "source_log": str(log_src),
        "request": normalize_text(request),
        "trajectory": meta,
        "fallback_used": fallback_used,
        "step_count": len(steps),
        "steps": normalize_steps(steps),
        "transcript_excerpt": transcript_excerpt(transcript_dst),
    }
    prune_unreferenced_pngs(log_dst, trace["steps"])

    trace_dir = root / "trace"
    trace_dir.mkdir(parents=True, exist_ok=True)
    (trace_dir / f"{label}.json").write_text(json.dumps(trace, indent=2), encoding="utf-8")
    with (trace_dir / f"{label}.ndjson").open("w", encoding="utf-8") as f:
        for row in trace["steps"]:
            f.write(json.dumps({"label": label, **row}) + "\n")

    preferred_images: list[Path] = []
    fallback_images: list[Path] = []
    for row in trace["steps"]:
        screenshots = row.get("screenshots") or {}
        for key in PREFERRED_ASSET_KEYS:
            filename = screenshots.get(key)
            if not filename:
                continue
            path = log_dst / filename
            if path.exists() and path.suffix.lower() == ".png":
                preferred_images.append(path)
        for filename in screenshots.values():
            path = log_dst / filename
            if path.exists() and path.suffix.lower() == ".png":
                fallback_images.append(path)
    image_paths = preferred_images or fallback_images
    if not image_paths:
        image_paths.extend(sorted(log_dst.glob("*.png")))

    rel_images = []
    for path in dict.fromkeys(image_paths):
        try:
            rel_images.append(path.relative_to(root).as_posix())
        except ValueError:
            rel_images.append(str(path))

    manifest = {
        "label": label,
        "task_id": args.task_id,
        "log_dir": (Path("logs") / label).as_posix(),
        "trace": (Path("trace") / f"{label}.json").as_posix(),
        "images": rel_images,
        "step_count": len(steps),
        "parser_ok": meta.get("parser_ok", False),
    }
    manifest_dir = root / "manifests"
    manifest_dir.mkdir(parents=True, exist_ok=True)
    (manifest_dir / f"{label}.json").write_text(json.dumps(manifest, indent=2), encoding="utf-8")
    print(f"harvested {label}: {len(steps)} steps, {len(rel_images)} images")


def load_manifest(root: Path, label: str) -> dict[str, Any]:
    path = root / "manifests" / f"{label}.json"
    if not path.exists():
        return {"label": label, "images": []}
    return json.loads(path.read_text(encoding="utf-8"))


def make_gif(images: list[Path], out: Path) -> None:
    frames = out.parent / f"frames-{out.stem}"
    if frames.exists():
        shutil.rmtree(frames)
    frames.mkdir(parents=True)
    chosen = images[:80] or images
    if len(chosen) == 1:
        chosen = chosen * 8
    for i, image in enumerate(chosen):
        shutil.copy2(image, frames / f"{i:04d}.png")
    vf = (
        "mpdecimate,setpts=N/(4*TB),"
        "scale=min(900\\,iw):-2:flags=lanczos,"
        "split[a][b];[a]palettegen=max_colors=128[p];[b][p]paletteuse=dither=bayer:bayer_scale=3"
    )
    subprocess.run(
        [
            "ffmpeg",
            "-y",
            "-v",
            "error",
            "-framerate",
            "4",
            "-i",
            str(frames / "%04d.png"),
            "-vf",
            vf,
            "-loop",
            "0",
            str(out),
        ],
        check=True,
    )


def build_asset(name: str, images: list[Path], out_dir: Path, still_index: int = -1) -> None:
    existing = [p for p in images if p.exists()]
    if not existing:
        raise SystemExit(f"no UFO images available for {name}")
    still = existing[still_index]
    shutil.copy2(still, out_dir / f"{name}.png")
    make_gif(existing, out_dir / f"{name}.gif")
    print(f"built {name} from {len(existing)} UFO images")


def build(args: argparse.Namespace) -> None:
    root = Path(args.harvest_root)
    out_dir = Path(args.out)
    out_dir.mkdir(parents=True, exist_ok=True)

    remote = load_manifest(root, "remote")
    thirdparty = load_manifest(root, "thirdparty")
    remote_images = [root / p for p in remote.get("images", [])]
    thirdparty_images = [root / p for p in thirdparty.get("images", [])]

    build_asset("install-screen", remote_images, out_dir, 0)
    build_asset("link-screen", remote_images, out_dir, -1)
    build_asset("third-party-app", thirdparty_images, out_dir, -1)

    mission = Path(args.mission_control) if args.mission_control else out_dir / "mission-control.png"
    if mission.exists():
        shutil.copy2(mission, out_dir / "mission-control.png")
        make_gif([mission], out_dir / "mission-control.gif")

    combined = []
    for path in sorted((root / "trace").glob("*.json")):
        combined.append(json.loads(path.read_text(encoding="utf-8")))
    (out_dir / "decision-trace.json").write_text(json.dumps(combined, indent=2), encoding="utf-8")
    with (out_dir / "decision-trace.ndjson").open("w", encoding="utf-8") as f:
        for item in combined:
            for row in item.get("steps", []):
                f.write(json.dumps({"label": item.get("label"), **row}) + "\n")


def main() -> None:
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="cmd", required=True)
    exp = sub.add_parser("export")
    exp.add_argument("--label", required=True)
    exp.add_argument("--task-id", required=True)
    exp.add_argument("--ufo-home", required=True)
    exp.add_argument("--log-name", default="adhoc")
    exp.add_argument("--request", default="")
    exp.add_argument("--request-file", default="")
    exp.add_argument("--out", required=True)
    bld = sub.add_parser("build")
    bld.add_argument("--harvest-root", required=True)
    bld.add_argument("--out", required=True)
    bld.add_argument("--mission-control", default="")
    args = parser.parse_args()
    if args.cmd == "export":
        export(args)
    elif args.cmd == "build":
        build(args)


if __name__ == "__main__":
    main()
