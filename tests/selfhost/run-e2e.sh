#!/usr/bin/env bash
# Runs ON the self-hosted KVM host. The WHOLE self-hosted e2e, first-principles:
#   revert the `cold` snapshot -> start the VM -> run the ONE in-guest journey in the interactive desktop
#   session -> collect its result and UFO's own trajectories -> build website assets from UFO artifacts.
# No Session-0/1 split and no external display recorder.
# Run it from its own directory (journey.ps1 + harvest_ufo.py live alongside).
set -uo pipefail
VM=${VM:-ufo-ws2025-base}; SNAP=${SNAP:-cold}; PW=${VM_PW:-'Ufo!Spike2026'}; GUSER=${VM_USER:-ufoadmin}
WORK=${WORK:-/mnt/ram/e2e}
HERE="$(cd "$(dirname "$0")" && pwd)"
IP=""
SSH(){ sshpass -p "$PW" ssh -n -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o ConnectTimeout=12 "$GUSER@$IP" "$@"; }
SCP(){ sshpass -p "$PW" scp -O -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null "$@"; }
PROGRESS="$WORK/progress.ndjson"
progress_seen=0
staged_installer=0

gha_escape(){
  local s="${1:-}"
  s=${s//'%'/'%25'}; s=${s//$'\r'/'%0D'}; s=${s//$'\n'/'%0A'}
  printf '%s' "$s"
}

progress_message(){
  local line="$1" event phase detail
  if command -v jq >/dev/null 2>&1 && printf '%s' "$line" | jq -e . >/dev/null 2>&1; then
    event=$(printf '%s' "$line" | jq -r '.event // "event"')
    phase=$(printf '%s' "$line" | jq -r '.phase // ""')
    detail=$(printf '%s' "$line" | jq -r '.detail // ""')
    case "$event" in
      journey_start) echo "journey started";;
      journey_done) echo "journey done";;
      phase_start) echo "phase started: $phase";;
      phase_update) echo "phase update: $phase${detail:+ - $detail}";;
      phase_done) echo "phase done: $phase${detail:+ ($detail)}";;
      phase_skipped) echo "phase skipped: $phase${detail:+ ($detail)}";;
      phase_failed) echo "phase failed: $phase${detail:+ - $detail}";;
      *) echo "$event${phase:+: $phase}${detail:+ - $detail}";;
    esac
  else
    echo "$line"
  fi
}

gray_average(){
  local png="$1"
  ffmpeg -v error -i "$png" -vf 'scale=1:1,format=gray' -frames:v 1 -f rawvideo - 2>/dev/null |
    od -An -tu1 | tr -d '[:space:]'
}

assert_nonblack_png(){
  local png="$1" avg
  [ -s "$png" ] || return 1
  avg=$(gray_average "$png" || true)
  [[ "$avg" =~ ^[0-9]+$ ]] || return 1
  [ "$avg" -gt 5 ]
}

emit_progress(){
  local msg="$1"
  echo "  progress: $msg"
  if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
    printf -- '- %s %s\n' "$(date -u +%H:%M:%SZ)" "$msg" >> "$GITHUB_STEP_SUMMARY"
  fi
  if [ -n "${GITHUB_ACTIONS:-}" ]; then
    printf '::notice title=Selfhost e2e::%s\n' "$(gha_escape "$msg")"
  fi
}

report_progress(){
  local tmp="$WORK/progress.tmp" total line
  SSH 'if(Test-Path C:\e2e\out\progress.ndjson){Get-Content C:\e2e\out\progress.ndjson}' > "$tmp" 2>/dev/null || true
  [ -s "$tmp" ] || return 0
  tr -d '\r' < "$tmp" > "$PROGRESS"
  total=$(wc -l < "$PROGRESS" | tr -d ' ')
  [ "$total" -gt "$progress_seen" ] 2>/dev/null || return 0
  while IFS= read -r line; do
    [ -n "$line" ] || continue
    emit_progress "$(progress_message "$line")"
  done < <(tail -n +"$((progress_seen + 1))" "$PROGRESS")
  progress_seen="$total"
}

echo "=== revert $SNAP + start $VM ==="
sudo virsh snapshot-revert "$VM" "$SNAP" 2>&1 | tail -1 || true
sudo virsh start "$VM" 2>&1 | tail -1 || true
for i in $(seq 1 60); do
  mac=$(sudo virsh domiflist "$VM" 2>/dev/null | awk '/default/{print $5}' | head -1)
  IP=$(sudo virsh net-dhcp-leases default 2>/dev/null | awk -v m="$mac" 'index($0,m){print $5}' | cut -d/ -f1 | head -1)
  if [ -n "$IP" ] && sshpass -p "$PW" ssh -n -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o ConnectTimeout=6 "$GUSER@$IP" "echo up" 2>/dev/null | grep -q up; then break; fi
  echo "  waiting for guest ssh (ip=${IP:-none})"; sleep 6
done
[ -n "$IP" ] || { echo "VM never came up"; exit 1; }
echo "guest ip=$IP"

# sshd comes up BEFORE the autologon desktop session is ready; the interactive (/IT) journey task can't
# run until the console is logged in. Wait for explorer.exe (the shell) so the launch isn't a race.
echo "=== wait for interactive desktop (autologon) ==="
for i in $(seq 1 40); do
  if SSH '[bool](Get-Process explorer -ErrorAction SilentlyContinue)' 2>/dev/null | grep -qi true; then echo "desktop ready"; break; fi
  echo "  waiting for desktop (explorer)"; sleep 5
done

# A sleeping/blanked virtual display can still break GUI tasks and the live screenshot command. Disable
# display sleep for the CI session.
echo "=== keep interactive display awake ==="
SSH 'powercfg /change monitor-timeout-ac 0; powercfg /change monitor-timeout-dc 0; powercfg /change standby-timeout-ac 0; powercfg /change standby-timeout-dc 0; reg add "HKCU\Control Panel\Desktop" /v ScreenSaveActive /t REG_SZ /d 0 /f | Out-Null; "display sleep disabled"'

# the e2e assertion library the journey composes (repo layout: ../e2e; dev layout: ./e2e)
E2EDIR="$HERE/e2e"; [ -d "$E2EDIR" ] || E2EDIR="$HERE/../e2e"
echo "=== stage journey + e2e lib ($E2EDIR) ==="
SSH 'New-Item -ItemType Directory -Force C:\e2e,C:\e2e\out,C:\e2e\dl | Out-Null; Remove-Item C:\e2e\out\*,C:\e2e\e2e,C:\e2e\journey.ps1,C:\e2e\harvest_ufo.py,C:\e2e\dl\ufoagent-setup.exe -Recurse -Force -EA SilentlyContinue; "staged"'
SCP "$HERE/journey.ps1" "$GUSER@$IP:C:/e2e/journey.ps1"
SCP "$HERE/harvest_ufo.py" "$GUSER@$IP:C:/e2e/harvest_ufo.py"
SCP -r "$E2EDIR" "$GUSER@$IP:C:/e2e/e2e"
if [ -n "${UFOAGENT_INSTALLER_PATH:-}" ]; then
  [ -f "${UFOAGENT_INSTALLER_PATH:-}" ] || { echo "installer artifact not found: $UFOAGENT_INSTALLER_PATH"; exit 1; }
  echo "=== stage installer artifact ==="
  SCP "$UFOAGENT_INSTALLER_PATH" "$GUSER@$IP:C:/e2e/dl/ufoagent-setup.exe"
  staged_installer=1
fi
# Stage tokens via base64: token values can contain chars that break naive quoting through
# ssh -> powershell -> Set-Content (the first CI run crashed with a token parsed as a command). Each
# value is base64 so env.ps1 holds no raw token chars, and the whole file is base64 for safe transport.
if [ -n "${CI_AGENT_TOKEN:-}" ] || [ -n "${UFOAGENT_BETA_URL:-}" ] || [ "$staged_installer" = 1 ]; then
  b64() { printf '%s' "${1:-}" | base64 -w0; }
  guest_installer=""
  [ "$staged_installer" = 1 ] && guest_installer='C:\e2e\dl\ufoagent-setup.exe'
  ENV_PS1="\$env:CI_AGENT_TOKEN=[Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('$(b64 "${CI_AGENT_TOKEN:-}")'))
\$env:CI_AGENT_ID=[Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('$(b64 "${CI_AGENT_ID:-}")'))
\$env:CI_ADMIN_TOKEN=[Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('$(b64 "${CI_ADMIN_TOKEN:-}")'))
\$env:UFOAGENT_BETA_URL=[Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('$(b64 "${UFOAGENT_BETA_URL:-}")'))
\$env:UFOAGENT_INSTALLER_PATH=[Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('$(b64 "$guest_installer")'))"
  ENV_B64=$(printf '%s' "$ENV_PS1" | base64 -w0)
  SSH "[IO.File]::WriteAllText('C:\\e2e\\env.ps1',[Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('$ENV_B64')))"
fi
# Windows PowerShell 5.1 reads BOM-less UTF-8 as ANSI (mangles em-dashes in helpers.ps1 -> parse errors).
# Re-encode every staged .ps1 as UTF-8 WITH BOM, reading bytes as UTF-8 explicitly via .NET.
SSH 'Get-ChildItem C:\e2e -Recurse -Filter *.ps1 | ForEach-Object { $c=[IO.File]::ReadAllText($_.FullName); [IO.File]::WriteAllText($_.FullName, $c, (New-Object System.Text.UTF8Encoding $true)) }; "reencoded utf8-bom"'

echo "=== clear local e2e outputs ==="
rm -rf "$WORK/gifs" "$WORK/ufo" "$WORK/result.json" "$WORK/phases.json" "$WORK/journey.log" "$PROGRESS" "$WORK/progress.tmp" "$WORK/service-errors.txt"
mkdir -p "$WORK/gifs"

echo "=== launch journey (one interactive task) ==="
# schtasks /TR with spaces is fragile, and a direct /TR gives no output if journey.ps1 dies. Use a
# no-spaces launcher .cmd that redirects the journey's output to journey.log (same idea as the old
# run_session1), so a crash is visible.
SSH 'Set-Content C:\e2e\run-journey.cmd -Encoding Ascii -Value @("@echo off","powershell -NoProfile -ExecutionPolicy Bypass -File C:\e2e\journey.ps1 > C:\e2e\out\journey.log 2>&1","echo LAUNCHER-EXIT %ERRORLEVEL% >> C:\e2e\out\journey.log"); "launcher written"'
SSH "schtasks /Create /TN UFOJourney /TR C:\\e2e\\run-journey.cmd /SC ONCE /ST 23:30 /RU $GUSER /IT /RL HIGHEST /F | Out-Null; schtasks /Run /TN UFOJourney | Out-Null; 'launched'"

echo "=== poll result (<=75m) ==="
status="PENDING"; d=$(( $(date +%s) + 75*60 ))
while [ "$(date +%s)" -lt "$d" ]; do
  report_progress || true
  status=$(SSH 'if(Test-Path C:\e2e\out\result.json){(Get-Content C:\e2e\out\result.json -Raw | ConvertFrom-Json).status}else{"PENDING"}' 2>/dev/null | tr -dc 'A-Za-z')
  echo "  $(date +%T) status=$status"
  case "$status" in PASS|FAIL) break;; esac
  sleep 10
done
report_progress || true

echo "=== collect ==="
SSH 'Get-Content C:\e2e\out\result.json -Raw' > "$WORK/result.json" 2>/dev/null || true
SSH 'Get-Content C:\e2e\out\phases.json -Raw' > "$WORK/phases.json" 2>/dev/null || true
SSH 'Get-Content C:\e2e\out\journey.log -Raw' > "$WORK/journey.log" 2>/dev/null || true
SSH 'Get-Content C:\e2e\out\progress.ndjson -Raw' > "$PROGRESS" 2>/dev/null || true
# UFO trajectories and real captured dashboard stills (binary -> SCP, not Get-Content).
SCP -r "$GUSER@$IP:C:/e2e/out/ufo" "$WORK/ufo" 2>/dev/null || true
SCP "$GUSER@$IP:C:/e2e/out/node-desktop.png" "$WORK/gifs/node-desktop.png" 2>/dev/null || true
SCP "$GUSER@$IP:C:/e2e/out/mission-control.png" "$WORK/gifs/mission-control.png" 2>/dev/null || true
SSH 'schtasks /Delete /TN UFOJourney /F 2>$null | Out-Null; "task cleaned"' >/dev/null 2>&1 || true
echo "--- journey.log (tail) ---"; tail -40 "$WORK/journey.log" 2>/dev/null
echo "--- progress ---"; cat "$PROGRESS" 2>/dev/null
echo "--- phases ---"; cat "$WORK/phases.json" 2>/dev/null

# Capture the Windows service health the box's Server Manager flags (the red "Services" count), so we can
# analyze them across runs. Diagnostic only - never fails the run.
echo "=== capture service errors (Server Manager 'Services' flags) ==="
SSH '
"=== auto-start services NOT running ==="
Get-CimInstance Win32_Service | Where-Object { $_.StartMode -eq "Auto" -and $_.State -ne "Running" } |
  Select-Object Name, DisplayName, State, ExitCode | Format-Table -AutoSize | Out-String -Width 200
"=== Service Control Manager errors/warnings (System log, last 50) ==="
Get-WinEvent -FilterHashtable @{LogName="System"; ProviderName="Service Control Manager"; Level=1,2,3} -MaxEvents 50 -ErrorAction SilentlyContinue |
  Select-Object TimeCreated, Id, LevelDisplayName, @{n="Msg";e={($_.Message -split "`r?`n")[0]}} | Format-Table -AutoSize | Out-String -Width 200
' > "$WORK/service-errors.txt" 2>/dev/null || true
echo "--- service-errors (head) ---"; head -50 "$WORK/service-errors.txt" 2>/dev/null

if [ "$status" = "PASS" ]; then
  echo "=== build UFO marketing assets ==="
  python3 "$HERE/harvest_ufo.py" build --harvest-root "$WORK/ufo" --out "$WORK/gifs" --mission-control "$WORK/gifs/mission-control.png" || exit 1
  for f in install-screen link-screen third-party-app mission-control; do
    [ -s "$WORK/gifs/$f.gif" ] || { echo "missing required capture asset: $f.gif"; exit 1; }
    [ -s "$WORK/gifs/$f.png" ] || { echo "missing required capture asset: $f.png"; exit 1; }
    assert_nonblack_png "$WORK/gifs/$f.png" || { echo "required capture asset appears black: $f.png"; exit 1; }
  done
  [ -s "$WORK/gifs/node-desktop.png" ] || { echo "missing required capture asset: node-desktop.png"; exit 1; }
  assert_nonblack_png "$WORK/gifs/node-desktop.png" || { echo "required capture asset appears black: node-desktop.png"; exit 1; }
  [ -s "$WORK/gifs/decision-trace.json" ] || { echo "missing required decision trace: decision-trace.json"; exit 1; }
  [ -s "$WORK/gifs/decision-trace.ndjson" ] || { echo "missing required decision trace: decision-trace.ndjson"; exit 1; }
fi
ls -la "$WORK/gifs" 2>/dev/null

echo "SELFHOST-E2E: $status"
[ "$status" = "PASS" ]
