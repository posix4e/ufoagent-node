# ufoagent-node

The open-source **UFOAgent node agent** for [Microsoft UFO2](https://github.com/microsoft/UFO).

Install it on your **own Windows machine**, link it to your [ufoagent.xyz](https://ufoagent.xyz)
account, and the resident daemon (`ufoagentd`) keeps a **short-lived LLM credential** fresh in
UFO2's `config/ufo/agents.yaml` — so UFO2 always has a working key without you ever handling one.

- **Bring your own Windows.** Download → install → link.
- **No long-lived keys on disk.** The control plane vends a short-lived credential; the daemon refreshes it and stores your machine's link token with **DPAPI**.
- **Signed & auto-updating.** Installer is Authenticode-signed; the daemon updates itself from signed GitHub Releases.
- **Pure standard library.** The daemon has zero runtime dependencies.

## Install (users)

1. Download `ufoagent-setup.exe` from [Releases](https://github.com/posix4e/ufoagent-node/releases) and run it.
2. It registers a background task and opens the linker. Approve the pairing code in your dashboard.
3. Done — UFO2 now has a live, auto-refreshed credential.

## CLI

```text
ufoagentd link        --control-plane https://ufoagent.xyz   # interactive device-code linking
ufoagentd configure   --control-plane URL --agent-token T --ufo-home DIR   # non-interactive (Azure bootstrap)
ufoagentd refresh     # fetch a credential, write agents.yaml, exit
ufoagentd run-daemon  # background loop: keep credential fresh + heartbeat
ufoagentd run --task t [-r "request"]   # refresh then run a UFO2 task
ufoagentd update [--apply]              # check / apply auto-update
ufoagentd status | version
```

## How it works

```
ufoagentd  ──POST /v1/link/start──►  control plane     (you approve the code in the dashboard)
           ◄─── agent token ─────
           ──GET  /v1/credentials─►  control plane     {base_url, api_key, model, expires_at}
           writes  {ufo_home}/config/ufo/agents.yaml   (HOST_AGENT + APP_AGENT)
           ──POST /v1/heartbeat───►  control plane     (liveness + version)
UFO2  ── reads agents.yaml ──►  api.openai.com          (direct; control plane never proxies LLM traffic)
```

The daemon re-fetches before the lease's `expires_at` and rewrites `agents.yaml`; UFO2 picks up the
fresh key on its next task run.

## Build from source (Windows)

```powershell
pip install -e ".[build,windows]"
pwsh installer\build.ps1 -Version 0.1.0   # -> installer\Output\ufoagent-setup.exe
```

Or just the daemon: `pip install -e .` then `ufoagentd --help`.

## Develop / test

```bash
python tests/test_unit.py          # offline unit tests (no deps)
python tests/e2e_local.py http://localhost:8788   # full loop vs a running control plane (wrangler dev)
```

## Release

Tag `vX.Y.Z`; `.github/workflows/release.yml` builds on a Windows runner, signs the exe + installer
with **Azure Trusted Signing**, and publishes a GitHub Release. The daemon's updater reads the latest
release as its appcast. Configure these repo secrets: `AZURE_TENANT_ID`, `AZURE_CLIENT_ID`,
`AZURE_CLIENT_SECRET`, `AZURE_SIGNING_ENDPOINT`, `AZURE_SIGNING_ACCOUNT`, `AZURE_SIGNING_PROFILE`.

## Security notes

- The machine **link token** is stored via Windows DPAPI (`CryptProtectData`); plaintext-0600 fallback off-Windows (dev only).
- The vended LLM key is short-lived and written into `agents.yaml` (a local, user-protected file). The control plane revokes the lease on expiry/teardown.
- The auto-updater verifies the downloaded installer's Authenticode signature before running it.

## License

Apache-2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE). Microsoft UFO2 is a separate MIT-licensed project.
