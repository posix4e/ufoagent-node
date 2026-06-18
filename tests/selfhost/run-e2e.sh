#!/usr/bin/env bash
# Runs ON the self-hosted KVM host. The WHOLE self-hosted e2e, first-principles:
#   revert the `cold` snapshot -> start the VM -> continuously screen-record the display from OUTSIDE
#   (virsh screenshot) -> run the ONE in-guest journey in the interactive desktop session -> collect its
#   result -> slice the one recording into per-phase gifs. No Session-0/1 split, no per-step capture.
# Run it from its own directory (journey.ps1 + slice.sh live alongside).
set -uo pipefail
VM=${VM:-ufo-ws2025-base}; SNAP=${SNAP:-cold}; PW=${VM_PW:-'Ufo!Spike2026'}; GUSER=${VM_USER:-ufoadmin}
WORK=${WORK:-/mnt/ram/e2e}; REC="$WORK/frames"; FPS_SLEEP=${FPS_SLEEP:-0.7}
HERE="$(cd "$(dirname "$0")" && pwd)"
IP=""
SSH(){ sshpass -p "$PW" ssh -n -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o ConnectTimeout=12 "$GUSER@$IP" "$@"; }
SCP(){ sshpass -p "$PW" scp -O -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null "$@"; }

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

# the e2e assertion library the journey composes (repo layout: ../e2e; dev layout: ./e2e)
E2EDIR="$HERE/e2e"; [ -d "$E2EDIR" ] || E2EDIR="$HERE/../e2e"
echo "=== stage journey + e2e lib ($E2EDIR) ==="
SSH 'New-Item -ItemType Directory -Force C:\e2e,C:\e2e\out | Out-Null; Remove-Item C:\e2e\out\*,C:\e2e\e2e,C:\e2e\journey.ps1 -Recurse -Force -EA SilentlyContinue; "staged"'
SCP "$HERE/journey.ps1" "$GUSER@$IP:C:/e2e/journey.ps1"
SCP -r "$E2EDIR" "$GUSER@$IP:C:/e2e/e2e"
# Stage tokens via base64: token values can contain chars that break naive quoting through
# ssh -> powershell -> Set-Content (the first CI run crashed with a token parsed as a command). Each
# value is base64 so env.ps1 holds no raw token chars, and the whole file is base64 for safe transport.
if [ -n "${CI_AGENT_TOKEN:-}" ] || [ -n "${UFOAGENT_BETA_URL:-}" ]; then
  b64() { printf '%s' "${1:-}" | base64 -w0; }
  ENV_PS1="\$env:CI_AGENT_TOKEN=[Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('$(b64 "${CI_AGENT_TOKEN:-}")'))
\$env:CI_AGENT_ID=[Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('$(b64 "${CI_AGENT_ID:-}")'))
\$env:CI_ADMIN_TOKEN=[Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('$(b64 "${CI_ADMIN_TOKEN:-}")'))
\$env:UFOAGENT_BETA_URL=[Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('$(b64 "${UFOAGENT_BETA_URL:-}")'))"
  ENV_B64=$(printf '%s' "$ENV_PS1" | base64 -w0)
  SSH "[IO.File]::WriteAllText('C:\\e2e\\env.ps1',[Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('$ENV_B64')))"
fi
# Windows PowerShell 5.1 reads BOM-less UTF-8 as ANSI (mangles em-dashes in helpers.ps1 -> parse errors).
# Re-encode every staged .ps1 as UTF-8 WITH BOM, reading bytes as UTF-8 explicitly via .NET.
SSH 'Get-ChildItem C:\e2e -Recurse -Filter *.ps1 | ForEach-Object { $c=[IO.File]::ReadAllText($_.FullName); [IO.File]::WriteAllText($_.FullName, $c, (New-Object System.Text.UTF8Encoding $true)) }; "reencoded utf8-bom"'

echo "=== start recorder (virsh screenshot loop) ==="
rm -rf "$REC" "$WORK/gifs" "$WORK/stop"; mkdir -p "$REC"   # clear stale frames+gifs so a failed run can't publish old ones
( while [ ! -f "$WORK/stop" ]; do
    sudo virsh screenshot "$VM" "$REC/frame-$(( $(date +%s%N) / 1000000 )).png" >/dev/null 2>&1 || true
    sleep "$FPS_SLEEP"
  done ) &
RECPID=$!

echo "=== launch journey (one interactive task) ==="
# schtasks /TR with spaces is fragile, and a direct /TR gives no output if journey.ps1 dies. Use a
# no-spaces launcher .cmd that redirects the journey's output to journey.log (same idea as the old
# run_session1), so a crash is visible.
SSH 'Set-Content C:\e2e\run-journey.cmd -Encoding Ascii -Value @("@echo off","powershell -NoProfile -ExecutionPolicy Bypass -File C:\e2e\journey.ps1 > C:\e2e\out\journey.log 2>&1","echo LAUNCHER-EXIT %ERRORLEVEL% >> C:\e2e\out\journey.log"); "launcher written"'
SSH "schtasks /Create /TN UFOJourney /TR C:\\e2e\\run-journey.cmd /SC ONCE /ST 23:30 /RU $GUSER /IT /RL HIGHEST /F | Out-Null; schtasks /Run /TN UFOJourney | Out-Null; 'launched'"

echo "=== poll result (<=15m) ==="
status="PENDING"; d=$(( $(date +%s) + 15*60 ))
while [ "$(date +%s)" -lt "$d" ]; do
  status=$(SSH 'if(Test-Path C:\e2e\out\result.json){(Get-Content C:\e2e\out\result.json -Raw | ConvertFrom-Json).status}else{"PENDING"}' 2>/dev/null | tr -dc 'A-Za-z')
  echo "  $(date +%T) status=$status"
  case "$status" in PASS|FAIL) break;; esac
  sleep 10
done

echo "=== stop recorder ==="
touch "$WORK/stop"; wait "$RECPID" 2>/dev/null || true
echo "frames=$(ls "$REC" 2>/dev/null | wc -l)"

echo "=== collect ==="
SSH 'Get-Content C:\e2e\out\result.json -Raw' > "$WORK/result.json" 2>/dev/null || true
SSH 'Get-Content C:\e2e\out\phases.json -Raw' > "$WORK/phases.json" 2>/dev/null || true
SSH 'Get-Content C:\e2e\out\journey.log -Raw' > "$WORK/journey.log" 2>/dev/null || true
SSH 'schtasks /Delete /TN UFOJourney /F 2>$null | Out-Null; "task cleaned"' >/dev/null 2>&1 || true
echo "--- journey.log (tail) ---"; tail -40 "$WORK/journey.log" 2>/dev/null
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

# Clock skew computed NOW (post-journey): the guest RTC can be hours off at boot but Windows time-sync
# corrects it within seconds, so by here host+guest are stable and aligned. (Computing it at boot caught
# the pre-sync clock and mis-mapped every window.)
echo "=== clock skew (guest->host) ==="
guest_ms=$(SSH '[DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()' | tr -d '\r' | grep -oE '[0-9]{13}' | head -1)
host_ms=$(( $(date +%s%N) / 1000000 ))
skew=$(( host_ms - guest_ms ))
echo "skew_ms=$skew"

echo "=== slice ==="
bash "$HERE/slice.sh" "$REC" "$WORK/phases.json" "$skew" "$WORK/gifs" || true
ls -la "$WORK/gifs" 2>/dev/null

echo "SELFHOST-E2E: $status"
[ "$status" = "PASS" ]
