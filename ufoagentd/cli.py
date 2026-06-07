"""ufoagentd command-line interface.

  ufoagentd link        --control-plane https://ufoagent.xyz   # interactive device-code linking
  ufoagentd configure   --control-plane URL --agent-token T --ufo-home DIR  # non-interactive
  ufoagentd refresh     # fetch a credential, write agents.yaml, exit
  ufoagentd run-daemon  # background loop: keep credential fresh + heartbeat
  ufoagentd run --task t [-r "request"]   # refresh then run a UFO2 task
  ufoagentd update [--apply]
  ufoagentd status | version
"""

from __future__ import annotations

import argparse
import logging
import platform as _platform
import socket
import subprocess
import sys

from . import __version__
from . import store
from .controlplane import ControlPlane, ControlPlaneError
from .credentials import refresh_once, run_daemon
from .linker import LinkError, link
from .updater import apply_update, check


def platform_string() -> str:
    return f"{_platform.system()} {_platform.release()}".strip()


def _control_plane(args, *, require_token: bool) -> ControlPlane:
    cfg = store.load_config()
    base = getattr(args, "control_plane", None) or cfg.get("control_plane")
    if not base:
        raise SystemExit("No control plane configured. Pass --control-plane or run `ufoagentd configure` first.")
    token = store.get_token() if require_token else None
    if require_token and not token:
        raise SystemExit("This machine is not linked. Run `ufoagentd link` first.")
    return ControlPlane(base, token=token)


def _ufo_home(args) -> str:
    home = getattr(args, "ufo_home", None) or store.load_config().get("ufo_home")
    if not home:
        raise SystemExit("UFO2 home unknown. Pass --ufo-home or set it via `ufoagentd configure`.")
    return home


def cmd_link(args) -> int:
    cfg = store.update_config(control_plane=args.control_plane, ufo_home=args.ufo_home)
    cp = ControlPlane(cfg["control_plane"])
    name = args.name or socket.gethostname()

    def announce(user_code: str, uri: str) -> None:
        print(f"\n  Link this machine: open  {uri}\n  and confirm the code:  {user_code}\n")

    try:
        agent_id, token = link(cp, name, platform_string(), announce)
    except (LinkError, ControlPlaneError) as e:
        print(f"Linking failed: {e}", file=sys.stderr)
        return 1
    store.set_token(token)
    print(f"Linked ✓  agent={agent_id} name={name}")
    return 0


def cmd_configure(args) -> int:
    store.update_config(control_plane=args.control_plane, ufo_home=args.ufo_home)
    if args.agent_token:
        store.set_token(args.agent_token)
    print("Configured ✓")
    return 0


def cmd_refresh(args) -> int:
    cp = _control_plane(args, require_token=True)
    cred = refresh_once(cp, _ufo_home(args))
    print(f"Credential written ✓  lease={cred.lease_id} model={cred.model} expires_at={cred.expires_at}")
    return 0


def cmd_run_daemon(args) -> int:
    cp = _control_plane(args, require_token=True)
    print("ufoagentd daemon started")
    run_daemon(cp, _ufo_home(args), __version__, platform_string())
    return 0


def cmd_run(args) -> int:
    cp = _control_plane(args, require_token=True)
    home = _ufo_home(args)
    refresh_once(cp, home)
    python = store.load_config().get("python", sys.executable)
    cmd = [python, "-m", "ufo", "--task", args.task]
    if args.request:
        cmd += ["-r", args.request]
    print(f"Running UFO2: {' '.join(cmd)} (cwd={home})")
    return subprocess.run(cmd, cwd=home).returncode


def cmd_update(args) -> int:
    cp = _control_plane(args, require_token=False)
    status = check(cp)
    print(f"current={status['current']} min_version={status['min_version']} update_required={status['update_required']}")
    if args.apply and status["update_required"]:
        ok = apply_update()
        print("Updated ✓" if ok else "No update applied")
    return 0


def cmd_status(args) -> int:
    cfg = store.load_config()
    print(f"control_plane: {cfg.get('control_plane', '(unset)')}")
    print(f"ufo_home:      {cfg.get('ufo_home', '(unset)')}")
    print(f"linked:        {'yes' if store.get_token() else 'no'}")
    print(f"version:       {__version__}")
    print(f"config_dir:    {store.config_dir()}")
    return 0


def cmd_version(_args) -> int:
    print(__version__)
    return 0


def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(prog="ufoagentd", description="UFOAgent node daemon for Microsoft UFO2")
    sub = p.add_subparsers(dest="command", required=True)

    pl = sub.add_parser("link", help="Interactive device-code linking")
    pl.add_argument("--control-plane", required=False)
    pl.add_argument("--name")
    pl.add_argument("--ufo-home")
    pl.set_defaults(func=cmd_link)

    pc = sub.add_parser("configure", help="Non-interactive configure (Azure bootstrap)")
    pc.add_argument("--control-plane")
    pc.add_argument("--agent-token")
    pc.add_argument("--ufo-home")
    pc.set_defaults(func=cmd_configure)

    pr = sub.add_parser("refresh", help="Fetch a credential and write agents.yaml")
    pr.add_argument("--control-plane")
    pr.add_argument("--ufo-home")
    pr.set_defaults(func=cmd_refresh)

    pd = sub.add_parser("run-daemon", help="Background refresh + heartbeat loop")
    pd.add_argument("--control-plane")
    pd.add_argument("--ufo-home")
    pd.set_defaults(func=cmd_run_daemon)

    pn = sub.add_parser("run", help="Refresh credential then run a UFO2 task")
    pn.add_argument("--task", required=True)
    pn.add_argument("-r", "--request")
    pn.add_argument("--control-plane")
    pn.add_argument("--ufo-home")
    pn.set_defaults(func=cmd_run)

    pu = sub.add_parser("update", help="Check (and optionally apply) updates")
    pu.add_argument("--apply", action="store_true")
    pu.add_argument("--control-plane")
    pu.set_defaults(func=cmd_update)

    sub.add_parser("status", help="Show local config").set_defaults(func=cmd_status)
    sub.add_parser("version", help="Print version").set_defaults(func=cmd_version)
    return p


def main(argv: list[str] | None = None) -> int:
    logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(message)s")
    args = build_parser().parse_args(argv)
    return args.func(args)
