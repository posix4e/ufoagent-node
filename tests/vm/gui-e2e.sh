#!/usr/bin/env bash
# Headful GUI e2e — proves the auto-logon → interactive Session 1 → GUI-drive → UIAutomation-verify
# bridge works on a REAL bare Windows VM, the mechanism the credentialed run_task verification reuses.
#
# Pipeline (all on one scratch VM, no public IP, driven through `az vm run-command`):
#   1. create a bare WS2025 VM
#   2. install + cold-provision UFO2  (reuse provision-and-probe.ps1 — the win32ui/MFC fidelity)
#   3. `ufoagent autologon --user <admin> --password <pw>`  (writes HKLM Winlogon; run-command is SYSTEM)
#   4. `az vm restart`  → boot auto-logs the admin into Session 1, installer's {commonstartup} shortcut
#      starts the tray
#   5. poll for a FRESH tasks\tray-alive  (mtime touched post-reboot ⇒ the tray is live in Session 1)
#   6. run session1-gui-selftest.ps1: Session-0 run-command spawns an INTERACTIVE scheduled task in
#      Session 1 that opens Notepad, types a marker, UIAutomation-reads it back, writes a verdict
#   7. assert "SESSION1-GUI OK"; teardown
#
# This deliberately uses NO UFO2/LLM credential — it validates the headful MECHANICS locally. The full
# credentialed run_task GUI (UFO2 actually driving Notepad) is exercised in CI, where CI_AGENT_TOKEN
# exists. Uses the ambient `az login`.
#
# Usage:  tests/vm/gui-e2e.sh
# Env:    LOCATION (westus2) SIZE (Standard_D4s_v5) RG (auto) UFOAGENT_BETA_URL
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
LOCATION="${LOCATION:-westus2}"
SIZE="${SIZE:-Standard_D4s_v5}"   # 4 cores — cold provisioning (Python + torch + UFO2) is CPU-bound
IMAGE="${IMAGE:-MicrosoftWindowsServer:WindowsServer:2025-datacenter-azure-edition:latest}"
STAMP="$(date +%Y%m%d-%H%M%S)"
RG="${RG:-ufo-guitest-${STAMP}-$$}"
VM="guitest"
ADMIN_USER="ufoadmin"   # MUST match session1-gui-selftest.ps1's GUI_USER default
ADMIN_PASS="Gui1!$(openssl rand -hex 10)Zz"   # meets Windows complexity; fixed once assigned

cleanup() { echo "=== teardown: deleting RG $RG ==="; az group delete -n "$RG" --yes --no-wait >/dev/null 2>&1 || true; }
trap cleanup EXIT

# Run a SHORT PowerShell snippet on the VM via the legacy run-command and echo its message. Fine for
# the quick steps here (autologon, marker poll, the self-test's own 120s internal wait); the long
# install+provision step uses the MANAGED run-command instead (below) because it outlives the legacy
# extension's provisioning timeout.
rc() {  # rc <powershell-inline>
  az vm run-command invoke -g "$RG" -n "$VM" --command-id RunPowerShellScript \
    --scripts "$1" --query "value[0].message" -o tsv 2>/dev/null || true
}

echo "=== creating RG $RG ($LOCATION) ==="
az group create -n "$RG" -l "$LOCATION" -o none

echo "=== creating bare Windows VM ($SIZE, $IMAGE; no public IP) ==="
az vm create -g "$RG" -n "$VM" --image "$IMAGE" --size "$SIZE" \
  --admin-username "$ADMIN_USER" --admin-password "$ADMIN_PASS" \
  --public-ip-address "" --nsg "" -o none
echo "VM up."

# 1) install + cold-provision via the MANAGED run-command (polled) — same as cold-provision.sh.
RC="guiprovision"
echo "=== install + cold-provision (managed run-command, polled) ==="
az vm run-command create -g "$RG" --vm-name "$VM" --name "$RC" \
  --script @"$HERE/provision-and-probe.ps1" --timeout-in-seconds 3600 --no-wait -o none

deadline=$(( $(date +%s) + 65 * 60 ))
state="Running"
while [ "$(date +%s)" -lt "$deadline" ]; do
  state="$(az vm run-command show -g "$RG" --vm-name "$VM" --run-command-name "$RC" \
    --instance-view --query "instanceView.executionState" -o tsv 2>/dev/null || echo Unknown)"
  echo "  provision state: ${state:-Unknown}"
  case "$state" in Succeeded | Failed | TimedOut | Canceled | Error) break ;; esac
  sleep 30
done
provision_out="$(az vm run-command show -g "$RG" --vm-name "$VM" --run-command-name "$RC" \
  --instance-view --query "instanceView.output" -o tsv 2>/dev/null)"
echo "----- provision output (executionState=$state) -----"
echo "$provision_out"
if ! printf '%s\n' "$provision_out" | grep -q 'COLD-PROVISION PASS'; then
  echo "=== GUI-E2E: FAIL (cold provisioning did not pass) ==="
  exit 1
fi
echo "cold provisioning: PASS"

# 2) enable auto-logon for the admin (writes HKLM Winlogon; run-command runs as SYSTEM). Does NOT take
#    effect until the next boot — so right now there is still no interactive session.
echo "=== enabling auto-logon for $ADMIN_USER ==="
rc "& \"\$env:ProgramFiles\\UFOAgent\\ufoagent.exe\" autologon --user $ADMIN_USER --password '$ADMIN_PASS'"

# 3) NEGATIVE CONTROL (fail-first): a fresh Azure VM with nobody logged on has no interactive Session 1,
#    so the GUI bridge CANNOT work yet — the interactive scheduled task has no desktop to run on and no
#    verdict gets written. Run the self-test now and require that it does NOT pass. This proves the
#    harness genuinely detects a dead Session 1 (a test that always prints OK proves nothing) and that
#    the auto-logon we're about to do is the load-bearing step.
echo "=== negative control: Session-1 GUI self-test BEFORE auto-logon takes effect (expect FAIL) ==="
neg_out="$(rc "$(cat "$HERE/session1-gui-selftest.ps1")")"
echo "----- negative-control output -----"
echo "$neg_out"
if printf '%s\n' "$neg_out" | grep -q 'SESSION1-GUI OK'; then
  echo "=== GUI-E2E: FAIL (negative control PASSED — harness is vacuous: it reported OK with no Session 1) ==="
  exit 1
fi
echo "negative control failed as expected (no interactive session before auto-logon): good"

# 4) reboot → boot auto-logs $ADMIN_USER into Session 1; the installer's {commonstartup} shortcut
#    starts the tray there.
echo "=== rebooting to trigger auto-logon + tray autostart ==="
az vm restart -g "$RG" -n "$VM" -o none

# 5) poll for a FRESH tray-alive marker. It may already exist (stale) from before provisioning killed
#    the procs, so we gate on mtime AGE, not existence — a recent touch proves the tray is live in the
#    auto-logged-in Session 1. run-command may fail transiently while the VM finishes booting; tolerate.
echo "=== waiting for the tray to come alive in Session 1 (auto-logon) ==="
alive_ps='$m="C:\ProgramData\UFOAgent\tasks\tray-alive"; if(Test-Path $m){"TRAY age="+[int]((Get-Date)-(Get-Item $m).LastWriteTime).TotalSeconds+"s"}else{"TRAY none"}'
tray_ok=0
tdeadline=$(( $(date +%s) + 10 * 60 ))
while [ "$(date +%s)" -lt "$tdeadline" ]; do
  out="$(rc "$alive_ps")"
  echo "  $out"
  age="$(printf '%s' "$out" | sed -n 's/.*TRAY age=\([0-9]\+\)s.*/\1/p')"
  if [ -n "$age" ] && [ "$age" -le 90 ]; then tray_ok=1; break; fi
  sleep 20
done
if [ "$tray_ok" -ne 1 ]; then
  echo "----- diagnostic: auto-logon / session state -----"
  rc 'query session 2>$null; "---"; quser 2>$null; "---"; Get-ItemProperty "HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon" | Select-Object AutoAdminLogon,DefaultUserName | Format-List'
  echo "=== GUI-E2E: FAIL (tray never came alive in Session 1 — auto-logon/desktop issue) ==="
  exit 1
fi
echo "tray is live in Session 1: OK"

# 6) POSITIVE: now that auto-logon created Session 1 and the tray is live, the same self-test must pass
#    — UFO2-style GUI drive + UIAutomation verify (the self-test bridges Session 0→1 via an interactive
#    scheduled task). Its internal poll waits up to 120s, so give the invoke headroom.
echo "=== positive: Session-1 GUI self-test AFTER auto-logon (expect PASS) ==="
gui_out="$(rc "$(cat "$HERE/session1-gui-selftest.ps1")")"
echo "----- gui self-test output -----"
echo "$gui_out"

if printf '%s\n' "$gui_out" | grep -q 'SESSION1-GUI OK'; then
  echo "=== GUI-E2E: PASS ==="
  exit 0
else
  echo "=== GUI-E2E: FAIL (Session-1 GUI drive/verify did not pass) ==="
  exit 1
fi
