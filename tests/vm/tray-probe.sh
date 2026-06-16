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
  age="$(printf '%s' "$out" | sed -n 's/.*TRAY age=\([0-9]\+\)s.*/\1/p')"
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

# DECISIVE: does the tray binary come alive when launched IN Session 1 via an interactive task? If yes,
# the binary is fine and the bug is purely the autostart MECHANISM (Startup folder not processed under
# auto-logon) → fix = logon scheduled task or service-spawns-tray. If still absent, the tray itself
# fails to build its icon in this session → different fix.
echo "=== DECISIVE: launch tray via interactive scheduled task in Session 1 ==="
rc '
$u="ufoadmin"
schtasks /Create /TN TrayLaunch /TR "\"C:\Program Files\UFOAgent\ufoagent.exe\" tray" /SC ONCE /ST 23:58 /RU $u /IT /RL HIGHEST /F | Out-Null
schtasks /Run /TN TrayLaunch | Out-Null
Start-Sleep -Seconds 25
$m="C:\ProgramData\UFOAgent\tasks\tray-alive"
if(Test-Path $m){"AFTER-TASK: tray-alive PRESENT, age="+[int]((Get-Date)-(Get-Item $m).LastWriteTime).TotalSeconds+"s  => tray binary works in-session; autostart MECHANISM is the bug"}
else{"AFTER-TASK: tray-alive still ABSENT => the tray binary itself failed to come up in Session 1"}
"--- ufoagent procs now ---"
Get-Process ufoagent -EA SilentlyContinue | Select-Object Id,SessionId | Format-Table -Auto | Out-String
"--- log tail ---"
Get-Content "C:\ProgramData\UFOAgent\logs\ufoagent.log" -Tail 18 -EA SilentlyContinue | Out-String
schtasks /Delete /TN TrayLaunch /F 2>$null | Out-Null
'
echo "================================================================"

if [ "$tray_ok" -eq 1 ]; then echo "=== TRAY-PROBE: tray came alive (age<=90s) ==="; exit 0
else echo "=== TRAY-PROBE: tray did NOT come alive (see diagnostic above) ==="; exit 1; fi
