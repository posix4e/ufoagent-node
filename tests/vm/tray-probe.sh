#!/usr/bin/env bash
# FAST diagnostic for the tray-autostart bug the GUI e2e surfaced: after an auto-logon reboot the
# UFOAgent tray never touched tasks\tray-alive, even though the {commonstartup} shortcut exists and the
# admin was logged into an Active console session. This skips the ~30-min UFO2 provisioning (irrelevant
# to autostart) to iterate in ~12 min, then dumps the LIVE tray state so we stop guessing.
#
# Usage:  tests/vm/tray-probe.sh
# Env:    LOCATION (westus2) SIZE (Standard_D2s_v5) RG (auto) UFOAGENT_BETA_URL
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
LOCATION="${LOCATION:-westus2}"
SIZE="${SIZE:-Standard_D2s_v5}"   # no provisioning here, so 2 cores is plenty (cheaper/faster)
IMAGE="${IMAGE:-MicrosoftWindowsServer:WindowsServer:2025-datacenter-azure-edition:latest}"
STAMP="$(date +%Y%m%d-%H%M%S)"
RG="${RG:-ufo-trayprobe-${STAMP}-$$}"
VM="trayprobe"
ADMIN_USER="ufoadmin"
ADMIN_PASS="Tray1!$(openssl rand -hex 10)Zz"

cleanup() { echo "=== teardown: deleting RG $RG ==="; az group delete -n "$RG" --yes --no-wait >/dev/null 2>&1 || true; }
trap cleanup EXIT

rc() {  # rc <powershell-inline>
  az vm run-command invoke -g "$RG" -n "$VM" --command-id RunPowerShellScript \
    --scripts "$1" --query "value[0].message" -o tsv 2>/dev/null || true
}

echo "=== creating RG $RG ($LOCATION) ==="
az group create -n "$RG" -l "$LOCATION" -o none
echo "=== creating bare Windows VM ($SIZE; no public IP) ==="
az vm create -g "$RG" -n "$VM" --image "$IMAGE" --size "$SIZE" \
  --admin-username "$ADMIN_USER" --admin-password "$ADMIN_PASS" \
  --public-ip-address "" --nsg "" -o none
echo "VM up."

# install (no provision wait) + set auto-logon. Pass creds by prepending $env: assignments to the
# script (run-command doesn't inherit our shell env). Password has no single quotes → safe to quote.
echo "=== install (skip provisioning) + enable auto-logon ==="
prelude="\$env:GUI_USER='$ADMIN_USER'; \$env:GUI_PASS='$ADMIN_PASS';"
[ -n "${UFOAGENT_BETA_URL:-}" ] && prelude="$prelude \$env:UFOAGENT_BETA_URL='$UFOAGENT_BETA_URL';"
install_out="$(rc "$prelude
$(cat "$HERE/tray-probe-install.ps1")")"
echo "$install_out"
printf '%s\n' "$install_out" | grep -q 'TRAY-PROBE-INSTALL OK' || { echo "=== install/autologon step failed ==="; exit 1; }

echo "=== rebooting to trigger auto-logon + tray autostart ==="
az vm restart -g "$RG" -n "$VM" -o none

# poll tray-alive briefly; the bug is that it never appears, so cap the wait and then DUMP state.
echo "=== polling tray-alive (auto-logon Session 1) ==="
alive_ps='$m="C:\ProgramData\UFOAgent\tasks\tray-alive"; if(Test-Path $m){"TRAY age="+[int]((Get-Date)-(Get-Item $m).LastWriteTime).TotalSeconds+"s"}else{"TRAY none"}'
tray_ok=0
tdeadline=$(( $(date +%s) + 6 * 60 ))
while [ "$(date +%s)" -lt "$tdeadline" ]; do
  out="$(rc "$alive_ps")"; echo "  $out"
  # Extract the digits between "age=" and "s", portably: BSD sed (macOS, where this driver runs)
  # doesn't support \+, and the run-command output carries CRLF — so do it with bash + tr.
  age="${out##*age=}"; age="${age%%s*}"; age="$(printf '%s' "$age" | tr -dc '0-9')"
  if [ -n "$age" ] && [ "$age" -le 90 ]; then tray_ok=1; break; fi
  sleep 20
done

echo "===================== LIVE TRAY DIAGNOSTIC ====================="
rc '
"--- ufoagent processes (is the tray running?) ---"
Get-Process ufoagent -ErrorAction SilentlyContinue | Select-Object Id,SessionId,StartTime,Path | Format-Table -Auto | Out-String
"--- explorer (the shell that processes the Startup folder) ---"
Get-Process explorer -ErrorAction SilentlyContinue | Select-Object Id,SessionId | Format-Table -Auto | Out-String
"--- {commonstartup} shortcut ---"
$lnk="C:\ProgramData\Microsoft\Windows\Start Menu\Programs\StartUp\UFOAgent.lnk"
"present: " + (Test-Path $lnk)
if(Test-Path $lnk){ $sh=New-Object -ComObject WScript.Shell; $s=$sh.CreateShortcut($lnk); "target: "+$s.TargetPath+" "+$s.Arguments }
"--- sessions ---"
quser 2>$null | Out-String
"--- tray-alive marker ---"
$m="C:\ProgramData\UFOAgent\tasks\tray-alive"; if(Test-Path $m){"present, age="+[int]((Get-Date)-(Get-Item $m).LastWriteTime).TotalSeconds+"s"}else{"absent"}
"--- ufoagent.log tail (tray launch / errors) ---"
Get-Content "C:\ProgramData\UFOAgent\logs\ufoagent.log" -Tail 50 -ErrorAction SilentlyContinue | Out-String
'

# De-risk the gui-e2e Session-1 GUI self-test cheaply (no provisioning): does the schtasks /IT bridge
# actually drive a GUI in Session 1 post-reboot and let us verify it via UIAutomation? This is exactly
# gui-e2e step 6 — prove it here in ~12 min before paying for the full 45-min run.
echo "=== Session-1 GUI self-test (gui-e2e step 6 mechanism) ==="
gui_out="$(rc "$(cat "$HERE/session1-gui-selftest.ps1")")"
echo "$gui_out"
if printf '%s\n' "$gui_out" | grep -q 'SESSION1-GUI OK'; then echo "SELF-TEST: OK"; else echo "SELF-TEST: NOT OK"; fi
echo "================================================================"

# ICON EXPERIMENT: the service spawns the tray via CreateProcessAsUser and its Shell_NotifyIcon
# (the visible icon) fails with E_FAIL → it runs headless. The GitHub runner launches the tray as a
# NORMAL in-session process (Start-Process) and the icon works. So: does launching the tray IN-SESSION
# (schtasks /IT, like the runner) build the icon here? If yes, the fix is the launch mechanism.
echo "=== ICON EXPERIMENT: in-session (schtasks) launch vs service CreateProcessAsUser ==="
rc '
$log = "C:\ProgramData\UFOAgent\logs\ufoagent.log"
"service-spawn (CreateProcessAsUser) icon-unavailable so far: " + ([bool](Select-String -Path $log -Pattern "system-tray UI unavailable" -Quiet))
# Stop the service so it stops re-spawning, and kill existing trays, so only OUR launch is in play.
Stop-Service UFOAgent -Force -EA SilentlyContinue; Start-Sleep 3
Get-Process ufoagent -EA SilentlyContinue | Stop-Process -Force -EA SilentlyContinue; Start-Sleep 2
$before = (Get-Content $log -EA SilentlyContinue).Count
# Launch the tray IN-SESSION via an interactive scheduled task (the proven Session-0->1 bridge).
# Use a no-spaces launcher .cmd so the spaced exe path needs no nested-quote escaping in /TR.
Set-Content -Path C:\tray-launch.cmd -Value "start """" ""C:\Program Files\UFOAgent\ufoagent.exe"" tray" -Encoding Ascii
schtasks /Create /TN TrayIcon /TR "C:\tray-launch.cmd" /SC ONCE /ST 23:57 /RU ufoadmin /IT /RL HIGHEST /F | Out-Null
schtasks /Run /TN TrayIcon | Out-Null
Start-Sleep 25
$after = Get-Content $log -EA SilentlyContinue | Select-Object -Skip $before
$newFail = [bool]($after | Select-String "system-tray UI unavailable" -Quiet)
$trayProcs = (Get-Process ufoagent -EA SilentlyContinue | Where-Object { $_.SessionId -eq 1 }).Count
"in-session launch: new icon-unavailable warning=$newFail ; session-1 tray procs=$trayProcs"
if (-not $newFail -and $trayProcs -ge 1) { "ICON-RESULT: IN-SESSION LAUNCH BUILT THE ICON => fix = launch in-session (logon task), not CreateProcessAsUser" }
elseif ($newFail) { "ICON-RESULT: in-session launch ALSO E_FAILed Shell_NotifyIcon => environmental, not the launch mechanism" }
else { "ICON-RESULT: inconclusive (no session-1 tray running)" }
schtasks /Delete /TN TrayIcon /F 2>$null | Out-Null
'
echo "================================================================"

if [ "$tray_ok" -eq 1 ]; then echo "=== TRAY-PROBE: tray came alive (age<=90s) ==="; exit 0
else echo "=== TRAY-PROBE: tray did NOT come alive (see diagnostic above) ==="; exit 1; fi
