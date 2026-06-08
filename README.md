# ufoagent-node

The open-source **UFOAgent node agent** for [Microsoft UFO2](https://github.com/microsoft/UFO) — a single
native **Rust** binary (`ufoagent.exe`). Install it on a Windows machine, link it to your
[ufoagent.xyz](https://ufoagent.xyz) account, and it keeps a **short-lived LLM credential** fresh in UFO2's
`config/ufo/agents.yaml` so UFO2 always has a working key — you never handle one.

- **Native, tiny, robust** — no Python runtime, no PyInstaller, no encoding/AV footguns.
- **Runs as a Windows service** (auto-start at boot, auto-restart; online without anyone logged in).
- **No long-lived keys on disk** — the control plane vends a short-lived credential the agent refreshes.

> Companion: the TypeScript control plane (`app.ufoagent.xyz`) and Python UFO2 (launched as a subprocess).

## Architecture
- **Service** (Session 0, headless): refreshes the credential + heartbeats + writes `status.json`/log. Never touches the GUI.
- **Interactive session**: **UFO2** (`python -m ufo …`, drives the desktop) and the tray/manager run in the logged-in session — that's where the screen is.
- **Unattended nodes** (no one logs in): `ufoagent autologon` configures auto-logon to a live, unlocked **console** session so UFO2 has a desktop to drive.

## CLI
```text
ufoagent link [--control-plane URL] [--name N]   # device-code linking
ufoagent configure --control-plane URL --agent-token T --ufo-home DIR
ufoagent refresh                                  # fetch credential -> agents.yaml
ufoagent run --task t [-r "request"]              # refresh then run a UFO2 task (interactive session)
ufoagent bootstrap                                # install UFO2 + deps into a managed venv (one-time)
ufoagent repair                                   # idempotent self-heal
ufoagent service install | uninstall | run        # Windows service
ufoagent autologon --user U --password P          # unattended-node auto-logon (opt-in)
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
Download `ufoagent-setup.exe` from [Releases](https://github.com/posix4e/ufoagent-node/releases), run it
(installs the service, provisions UFO2, opens the linker). Approve the pairing code in your dashboard.

## Status
- ✅ Rust core (link/refresh/daemon/bootstrap/repair/update), Windows service, Windows CI, installer.
- 🔜 Tray/manager UI + `autologon` implementation (Windows-only; landing next, verified in CI).
- Installer is unsigned until Azure Trusted Signing identity validation is approved.

## License
Apache-2.0 (see [LICENSE](LICENSE), [NOTICE](NOTICE)). Microsoft UFO2 is a separate MIT project.
