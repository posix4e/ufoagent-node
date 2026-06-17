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
if [ -n "${CI_AGENT_TOKEN:-}" ]; then
  SSH "Set-Content C:\\e2e\\env.ps1 -Encoding Ascii -Value (\"\\\$env:CI_AGENT_TOKEN='${CI_AGENT_TOKEN}'\",\"\\\$env:CI_AGENT_ID='${CI_AGENT_ID:-}'\",\"\\\$env:CI_ADMIN_TOKEN='${CI_ADMIN_TOKEN:-}'\")"
fi
[ -n "${UFOAGENT_BETA_URL:-}" ] && SSH "Add-Content C:\\e2e\\env.ps1 -Encoding Ascii -Value \"\\\$env:UFOAGENT_BETA_URL='${UFOAGENT_BETA_URL}'\""
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
launch='powershell -NoProfile -ExecutionPolicy Bypass -File C:\e2e\journey.ps1'
SSH "schtasks /Create /TN UFOJourney /TR '$launch' /SC ONCE /ST 23:30 /RU $GUSER /IT /RL HIGHEST /F | Out-Null; schtasks /Run /TN UFOJourney | Out-Null; 'launched'"

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
SSH 'schtasks /Delete /TN UFOJourney /F 2>$null | Out-Null; "task cleaned"' >/dev/null 2>&1 || true
echo "--- phases ---"; cat "$WORK/phases.json" 2>/dev/null

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
