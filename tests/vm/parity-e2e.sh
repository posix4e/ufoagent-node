#!/usr/bin/env bash
# Full headful e2e on a bare Windows VM — the re-host of the GitHub-runner GUI test, at parity. Reuses
# the SAME assertion scripts (tests/e2e/*.ps1, tests/gui/TrayTest) the runner uses: the VM downloads
# the repo branch tarball and runs them, with RUNNER_TEMP + the CI node tokens set as machine env so
# both Session-0 (run-command) and Session-1 (interactive scheduled task) processes inherit them.
#
# Stages (each credentialed/GUI stage skips gracefully when its prerequisite is absent, so a TOKENLESS
# local run still exercises the whole machinery + the token-free artifacts):
#   provision (cold-start) -> autologon -> reboot -> tray-alive
#   Session 0: verify-install, linked-smoke [needs CI_AGENT_TOKEN]
#   Session 1: install/link-screen captures, local-task, remote-task [token], taskbar (.NET) [token]
#   assemble GIFs -> pull all shots off the no-IP VM via an ephemeral blob
#
# Usage:   tests/vm/parity-e2e.sh
# Env:     LOCATION SIZE RG OUT(./parity-shots) E2E_REF(branch) UFOAGENT_BETA_URL
#          SKIP_PROVISION=1 (fast iteration) CI_AGENT_TOKEN CI_AGENT_ID CI_ADMIN_TOKEN
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
LOCATION="${LOCATION:-westus2}"
SIZE="${SIZE:-Standard_D4s_v5}"
IMAGE="${IMAGE:-MicrosoftWindowsServer:WindowsServer:2025-datacenter-azure-edition:latest}"
STAMP="$(date +%Y%m%d-%H%M%S)"
RG="${RG:-ufo-parity-${STAMP}-$$}"
VM="parity"
ADMIN_USER="ufoadmin"
ADMIN_PASS="Par1!$(openssl rand -hex 10)Zz"
OUT="${OUT:-$PWD/parity-shots}"
E2E_REF="${E2E_REF:-main}"
REPO="ufoagent/ufoagent-node"
SA="ufopar$(date +%H%M%S)$RANDOM"; SA="$(echo "$SA" | tr -cd 'a-z0-9' | cut -c1-24)"
HAVE_TOKEN=0; [ -n "${CI_AGENT_TOKEN:-}" ] && HAVE_TOKEN=1

cleanup() { echo "=== teardown: deleting RG $RG ==="; az group delete -n "$RG" --yes --no-wait >/dev/null 2>&1 || true; }
trap cleanup EXIT

# Run a SHORT PowerShell snippet on the VM (Session 0) and echo its output.
rc() { az vm run-command invoke -g "$RG" -n "$VM" --command-id RunPowerShellScript --scripts "$1" --query "value[0].message" -o tsv 2>/dev/null || true; }

# Run an e2e script in the interactive Session 1 via a scheduled task, capture its exit code + log.
# $1 = short name, $2 = absolute .ps1 path on the VM. Echoes the captured log; sets global RC1.
RC1=0
run_session1() {
  local name="$1" script="$2"
  rc "
\$ErrorActionPreference='SilentlyContinue'
Remove-Item C:\\rt\\$name.rc,C:\\rt\\$name.log -ErrorAction SilentlyContinue
# No-spaces launcher: set RUNNER_TEMP (the e2e scripts' shots dir), run the script, capture exit + log.
# (Task Scheduler gives the task a fresh env block, so machine-env tokens are also visible here.)
Set-Content C:\\rt\\$name-launch.cmd -Value 'set RUNNER_TEMP=C:\\rt
powershell -NoProfile -ExecutionPolicy Bypass -File \"$script\" > C:\\rt\\$name.log 2>&1
echo %ERRORLEVEL% > C:\\rt\\$name.rc' -Encoding Ascii
schtasks /Create /TN P_$name /TR C:\\rt\\$name-launch.cmd /SC ONCE /ST 23:30 /RU $ADMIN_USER /IT /RL HIGHEST /F | Out-Null
schtasks /Run /TN P_$name | Out-Null
" >/dev/null
  # Poll for the .rc (these GUI tasks can take minutes — UFO2 runs up to ~6m).
  local d=$(( $(date +%s) + 8 * 60 )) got=""
  while [ "$(date +%s)" -lt "$d" ]; do
    got="$(rc "if(Test-Path C:\\rt\\$name.rc){(Get-Content C:\\rt\\$name.rc -Raw).Trim()}else{'PENDING'}")"
    got="$(printf '%s' "$got" | tr -dc '0-9PENDIG')"
    case "$got" in PENDING|'') sleep 15;; *) break;; esac
  done
  rc "schtasks /Delete /TN P_$name /F 2>\$null | Out-Null; if(Test-Path C:\\rt\\$name.log){Get-Content C:\\rt\\$name.log -Tail 40}"
  RC1="${got:-X}"
}

echo "=== creating RG $RG + bare VM ($SIZE) ==="
az group create -n "$RG" -l "$LOCATION" -o none
az vm create -g "$RG" -n "$VM" --image "$IMAGE" --size "$SIZE" \
  --admin-username "$ADMIN_USER" --admin-password "$ADMIN_PASS" --public-ip-address "" --nsg "" -o none

echo "=== creating ephemeral storage $SA (shots ferry) ==="
az storage account create -n "$SA" -g "$RG" -l "$LOCATION" --sku Standard_LRS --kind StorageV2 --allow-blob-public-access false -o none
KEY="$(az storage account keys list -n "$SA" -g "$RG" --query "[0].value" -o tsv)"
az storage container create -n shots --account-name "$SA" --account-key "$KEY" -o none
EXPIRY="$(date -u -v+3H +%Y-%m-%dT%H:%MZ 2>/dev/null || date -u -d '+3 hours' +%Y-%m-%dT%H:%MZ)"
WSAS="$(az storage container generate-sas -n shots --account-name "$SA" --account-key "$KEY" --permissions cw --expiry "$EXPIRY" --https-only -o tsv)"

# 1) machine env (tokens + RUNNER_TEMP) so the reused e2e scripts run unmodified; download the repo.
echo "=== seeding VM: machine env + repo ($E2E_REF) ==="
rc "
New-Item -ItemType Directory -Force -Path C:\\rt\\shots | Out-Null
[Environment]::SetEnvironmentVariable('RUNNER_TEMP','C:\\rt','Machine')
[Environment]::SetEnvironmentVariable('CI_AGENT_TOKEN','${CI_AGENT_TOKEN:-}','Machine')
[Environment]::SetEnvironmentVariable('CI_AGENT_ID','${CI_AGENT_ID:-}','Machine')
[Environment]::SetEnvironmentVariable('CI_ADMIN_TOKEN','${CI_ADMIN_TOKEN:-}','Machine')
[Net.ServicePointManager]::SecurityProtocol=[Net.SecurityProtocolType]::Tls12
Invoke-WebRequest -UseBasicParsing 'https://github.com/$REPO/archive/refs/heads/$E2E_REF.tar.gz' -OutFile C:\\repo.tgz
tar -xf C:\\repo.tgz -C C:\\
\$d=(Get-ChildItem C:\\ -Directory -Filter 'ufoagent-node-*' | Select-Object -First 1).FullName
\$e=Join-Path \$d 'tests\\e2e'
# Windows PowerShell 5.1 (what run-command + schtasks use here) reads a BOM-less .ps1 as ANSI, mangling
# the em-dashes/emoji in these scripts -> parse errors. Re-save each as UTF-8 WITH BOM so 5.1 parses
# them as UTF-8. (The runner never hit this: ci.yml uses 'shell: pwsh' (PS7), which defaults to UTF-8.)
\$bom = New-Object System.Text.UTF8Encoding \$true
Get-ChildItem \$e\\*.ps1 | ForEach-Object { [System.IO.File]::WriteAllText(\$_.FullName, [System.IO.File]::ReadAllText(\$_.FullName), \$bom) }
[Environment]::SetEnvironmentVariable('E2E_DIR', \$e, 'Machine')
'e2e dir: ' + \$e + ' ; scripts: ' + ((Get-ChildItem \$e\\*.ps1 -EA SilentlyContinue).Count)
" | tail -2

# Resolve the on-VM e2e dir once (for building absolute script paths).
E2EDIR="$(rc '[Environment]::GetEnvironmentVariable("E2E_DIR","Machine")' | tr -d '\r')"
echo "VM e2e dir: $E2EDIR"
[ -n "$E2EDIR" ] || { echo "FAIL: repo did not download / e2e dir not found"; exit 1; }

# 2) install (+ provision unless skipped) + autologon. Reuse tray-probe-install.ps1 (skips provision)
#    or provision-and-probe.ps1 (full cold-start). Forward creds + beta url via prelude.
prelude="\$env:GUI_USER='$ADMIN_USER'; \$env:GUI_PASS='$ADMIN_PASS';"
[ -n "${UFOAGENT_BETA_URL:-}" ] && prelude="$prelude \$env:UFOAGENT_BETA_URL='$UFOAGENT_BETA_URL';"
if [ "${SKIP_PROVISION:-0}" = "1" ]; then
  echo "=== install (SKIP provision) + autologon ==="
  out="$(rc "$prelude
$(cat "$HERE/tray-probe-install.ps1")")"; echo "$out" | tail -3
  printf '%s\n' "$out" | grep -q 'TRAY-PROBE-INSTALL OK' || { echo "install failed"; exit 1; }
else
  echo "=== install + cold-provision (managed run-command) ==="
  RCN=parityprov
  provscript="$(cat "$HERE/provision-and-probe.ps1")"
  [ -n "${UFOAGENT_BETA_URL:-}" ] && provscript="\$env:UFOAGENT_BETA_URL='$UFOAGENT_BETA_URL';
$provscript"
  az vm run-command create -g "$RG" --vm-name "$VM" --name "$RCN" --script "$provscript" --timeout-in-seconds 3600 --no-wait -o none
  d=$(( $(date +%s) + 60*60 )); st=Running
  while [ "$(date +%s)" -lt "$d" ]; do
    st="$(az vm run-command show -g "$RG" --vm-name "$VM" --run-command-name "$RCN" --instance-view --query "instanceView.executionState" -o tsv 2>/dev/null || echo Unknown)"
    echo "  provision: $st"; case "$st" in Succeeded|Failed|TimedOut|Canceled|Error) break;; esac; sleep 30
  done
  pout="$(az vm run-command show -g "$RG" --vm-name "$VM" --run-command-name "$RCN" --instance-view --query "instanceView.output" -o tsv 2>/dev/null)"
  printf '%s\n' "$pout" | grep -q 'COLD-PROVISION PASS' || { echo "provision failed:"; echo "$pout" | tail -20; exit 1; }
  echo "cold provisioning: PASS"
  # set autologon (provision-and-probe doesn't); reuse the agent's autologon.
  rc "& \"\$env:ProgramFiles\\UFOAgent\\ufoagent.exe\" autologon --user $ADMIN_USER --password '$ADMIN_PASS'" >/dev/null
fi

echo "=== rebooting (auto-logon -> service launches tray in-session) ==="
az vm restart -g "$RG" -n "$VM" -o none

echo "=== waiting for tray-alive ==="
tray_ok=0; td=$(( $(date +%s) + 10*60 ))
while [ "$(date +%s)" -lt "$td" ]; do
  o="$(rc '$m="C:\ProgramData\UFOAgent\tasks\tray-alive"; if(Test-Path $m){"TRAY age="+[int]((Get-Date)-(Get-Item $m).LastWriteTime).TotalSeconds+"s"}else{"TRAY none"}')"
  echo "  $o"; a="${o##*age=}"; a="${a%%s*}"; a="$(printf '%s' "$a" | tr -dc '0-9')"
  if [ -n "$a" ] && [ "$a" -le 90 ]; then tray_ok=1; break; fi; sleep 20
done
[ "$tray_ok" -eq 1 ] || { echo "FAIL: tray never came alive"; exit 1; }
echo "tray live in Session 1: OK"

fails=()

# 3) Session 0: verify-install (always) + linked-smoke (needs token).
echo "=== [s0] verify-install ==="
vout="$(rc "& \"\$env:ProgramFiles\\UFOAgent\\ufoagent.exe\" version; \$ProgressPreference='SilentlyContinue'; try{(Invoke-WebRequest 'https://app.ufoagent.xyz/v1/agent/version' -UseBasicParsing -TimeoutSec 20).StatusCode}catch{'cp-unreachable: '+\$_.Exception.Message}")"
echo "$vout"

if [ "$HAVE_TOKEN" = "1" ]; then
  echo "=== [s0] linked-smoke ==="
  # Session-0 run-command inherits the run-command service's STALE env block, so set env inline here
  # (machine env isn't picked up by these processes — only by freshly-launched Session-1 tasks).
  s0env="\$env:RUNNER_TEMP='C:\\rt'; \$env:CI_AGENT_TOKEN='${CI_AGENT_TOKEN:-}'; \$env:CI_AGENT_ID='${CI_AGENT_ID:-}'; \$env:CI_ADMIN_TOKEN='${CI_ADMIN_TOKEN:-}';"
  ls_out="$(rc "$s0env & '$E2EDIR\\linked-smoke.ps1'")"
  echo "$ls_out" | tail -20
  printf '%s\n' "$ls_out" | grep -q 'linked smoke OK' || fails+=("linked-smoke")
else
  echo "=== [s0] linked-smoke SKIPPED (no CI_AGENT_TOKEN) ==="
fi

# 4) Session 1 (token-free): the link-screen capture — also validates the Session-1 bridge. These
#    capture scripts set ErrorActionPreference=Continue (rc is always 0), so assert the PNG artifact.
echo "=== [s1] capture link-screen ==="
run_session1 linkscreen "$E2EDIR\\capture-link-screen.ps1"
ls_png="$(rc 'if(Test-Path C:\rt\shots\link-screen.png){"YES "+(Get-Item C:\rt\shots\link-screen.png).Length}else{"NO"}')"
echo "  link-screen.png: $ls_png (rc=$RC1)"
printf '%s' "$ls_png" | grep -q YES || fails+=("link-screen-png")

# 5) Session 1 credentialed run_task (needs a linked node for the LLM credential): the service routes
#    the enqueued task to the tray executor (same Session 1), which runs UFO2 -> Notepad; the script
#    (also Session 1) records frames + screenshots it.
if [ "$HAVE_TOKEN" = "1" ]; then
  echo "=== [s1] local-task ==="
  run_session1 localtask "$E2EDIR\\local-task.ps1"; echo "  local-task rc=$RC1"; [ "$RC1" = "0" ] || fails+=("local-task(rc=$RC1)")
  echo "=== [s1] remote-task ==="
  run_session1 remotetask "$E2EDIR\\remote-task.ps1"; echo "  remote-task rc=$RC1"; [ "$RC1" = "0" ] || fails+=("remote-task(rc=$RC1)")
else
  echo "=== [s1] run_task (local + remote) SKIPPED (no CI_AGENT_TOKEN / no LLM credential) ==="
fi

# 5) assemble GIFs (Session 0; needs the frames local-task recorded).
echo "=== [s0] assemble-gifs ==="
rc "\$env:RUNNER_TEMP='C:\\rt'; & '$E2EDIR\\assemble-gifs.ps1'" | tail -6 || true

# 6) pull all shots off the VM.
echo "=== uploading shots from VM ==="
rc "\$sas='$WSAS'; \$base='https://$SA.blob.core.windows.net/shots'
Get-ChildItem C:\\rt\\shots\\*.png,C:\\rt\\shots\\*.gif -EA SilentlyContinue | ForEach-Object {
  try { Invoke-WebRequest -Method Put -InFile \$_.FullName -Headers @{'x-ms-blob-type'='BlockBlob'} -Uri (\"\$base/\$(\$_.Name)?\$sas\") -UseBasicParsing | Out-Null; 'up '+\$_.Name } catch { 'up-fail '+\$_.Name }
}" | tail -20
mkdir -p "$OUT"; az storage blob download-batch -d "$OUT" -s shots --account-name "$SA" --account-key "$KEY" -o none 2>/dev/null || true
echo "shots -> $OUT:"; ls -la "$OUT" 2>/dev/null

if [ "${#fails[@]}" -eq 0 ]; then echo "=== PARITY-E2E: PASS (token=$HAVE_TOKEN) ==="; exit 0
else echo "=== PARITY-E2E: FAIL: ${fails[*]} ==="; exit 1; fi
