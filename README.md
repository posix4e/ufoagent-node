# ufoagent-node

The open-source **UFOAgent node agent** for [Microsoft UFO2](https://github.com/microsoft/UFO) — a single
native **Rust** binary (`ufoagent.exe`). Install it on a Windows machine, link it to your
[ufoagent.xyz](https://ufoagent.xyz) account, and it keeps a live **interactive desktop** ready for
unattended GUI tasks while refreshing UFO2's short-lived LLM credential.

- **Native, tiny, robust** — no Python runtime, no PyInstaller, no encoding/AV footguns.
- **Runs GUI work from the login desktop** — the tray worker owns screenshots, mouse, keyboard, and UFO2.
- **Service helper for maintenance** — credentials, heartbeats, updates, and login-worker self-heal.
- **No long-lived keys on disk** — the control plane vends a short-lived credential the agent refreshes.
- **Unattended by default** — installer prompts for Windows auto-logon so reboots land on a usable desktop.

> Companion: the TypeScript control plane (`app.ufoagent.xyz`) and Python UFO2 (launched as a subprocess).

## Architecture
- **Login desktop worker**: **UFO2** (`python -m ufo …`, drives the desktop) and the tray/manager run in the logged-in session — that's where the screen is.
- **Service helper** (Session 0, headless): refreshes credentials, heartbeats, writes `status.json`/log, checks updates, and re-launches the login worker when a desktop exists. It never drives the GUI.
- **Unattended nodes**: `ufoagent autologon` configures Windows auto-logon to a live, unlocked **console** session so UFO2 has a desktop to drive after boot.

## CLI
```text
ufoagent link [--control-plane URL] [--name N]   # device-code linking
ufoagent configure --control-plane URL --agent-token T --ufo-home DIR
ufoagent refresh                                  # fetch credential -> agents.yaml
ufoagent run --task t [-r "request"]              # refresh then run a UFO2 task (interactive session)
ufoagent bootstrap                                # install UFO2 + deps into a managed venv (one-time)
ufoagent repair                                   # idempotent self-heal
ufoagent service install | uninstall | run        # maintenance helper; not the GUI runner
ufoagent autologon                                # prompt for unattended GUI auto-logon
ufoagent autologon --user U --password P          # non-interactive auto-logon setup
ufoagent status | version
```

## Build
```bash
cargo build --release        # -> target/release/ufoagent.exe
cargo test && cargo clippy --all-targets -- -D warnings
```
Cross-platform code builds on macOS/Linux for dev; Windows-only bits (`service`, DPAPI, tray) are
`#[cfg(windows)]` and built/tested in **Windows CI** (`.github/workflows/ci.yml` exercises the real binary:
`version`/`status`/`configure`, a `run-daemon` smoke, `service install/uninstall`, and a live `link_start`).

## Install (users)
Download `ufoagent-setup.exe` from [Releases](https://github.com/ufoagent/ufoagent-node/releases), run it,
and leave **Configure unattended GUI mode (auto-login)** checked. Use a dedicated, low-privilege Windows
account on an isolated agent machine, then approve the pairing code in your dashboard.

## Remote access
Keep UFOAgent's runtime desktop as the auto-logged-in console session. If you need to inspect the node,
prefer a console-style viewer that attaches to the same desktop (for example VNC, noVNC, MeshCentral, or
a VM console). Normal Windows RDP is useful as an admin fallback, but disconnecting or switching RDP
sessions can make GUI automation unavailable until the console desktop is restored.

## Status
- ✅ Rust core (link/refresh/daemon/bootstrap/repair/update), login desktop worker, Windows service helper, Windows CI, installer.
- ✅ Tray/manager UI + unattended GUI auto-logon prompt (Windows-only).
- Installer is unsigned until Azure Trusted Signing identity validation is approved.

## License
Apache-2.0, © 2026 UFOAgent, Inc. (see [LICENSE](LICENSE), [NOTICE](NOTICE)).
