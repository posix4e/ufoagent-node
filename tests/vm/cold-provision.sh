#!/usr/bin/env bash
# Faithful cold-start integration test. Provisions the agent on a BARE Windows Server VM (no Visual
# Studio, no Python, no VC++/MFC runtime) — the real customer environment the GitHub runner can't
# represent because it preinstalls all of that. Creates a scratch VM, runs install+provision+probe in
# one headless `az vm run-command` (see tests/vm/provision-and-probe.ps1), then tears the VM down.
# Uses the ambient `az login`; no public IP (drives the VM through run-command).
#
# Usage:  tests/vm/cold-provision.sh
# Env:    LOCATION (westus2) SIZE (Standard_D2s_v5) RG (auto) UFOAGENT_BETA_URL
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
LOCATION="${LOCATION:-westus2}"
SIZE="${SIZE:-Standard_D4s_v5}"   # 4 cores — fresh-VM provisioning (Python + torch + UFO2) is CPU-bound
IMAGE="${IMAGE:-MicrosoftWindowsServer:WindowsServer:2025-datacenter-azure-edition:latest}"
STAMP="$(date +%Y%m%d-%H%M%S)"
RG="${RG:-ufo-coldtest-${STAMP}-$$}"
VM="coldtest"
ADMIN_USER="ufoadmin"
ADMIN_PASS="Cold1!$(openssl rand -hex 10)Zz"   # meets Windows complexity

cleanup() { echo "=== teardown: deleting RG $RG ==="; az group delete -n "$RG" --yes --no-wait >/dev/null 2>&1 || true; }
trap cleanup EXIT

echo "=== creating RG $RG ($LOCATION) ==="
az group create -n "$RG" -l "$LOCATION" -o none

echo "=== creating bare Windows VM ($SIZE, $IMAGE; no public IP) ==="
az vm create -g "$RG" -n "$VM" --image "$IMAGE" --size "$SIZE" \
  --admin-username "$ADMIN_USER" --admin-password "$ADMIN_PASS" \
  --public-ip-address "" --nsg "" -o none
echo "VM up."

# Install + provision + probe in one script, via the MANAGED run-command (`az vm run-command create`)
# — NOT the legacy `invoke`, whose RunCommandWindows extension has a provisioning timeout that a
# ~20-min bootstrap blows past (VMExtensionProvisioningTimeout). The managed one takes
# --timeout-in-seconds and we poll its executionState, so long provisioning is fine.
RC="coldprobe"
echo "=== install + provision + probe (managed run-command, polled) ==="
az vm run-command create -g "$RG" --vm-name "$VM" --name "$RC" \
  --script @"$HERE/provision-and-probe.ps1" --timeout-in-seconds 3600 --no-wait -o none

deadline=$(( $(date +%s) + 65 * 60 ))
state="Running"
while [ "$(date +%s)" -lt "$deadline" ]; do
  state="$(az vm run-command show -g "$RG" --vm-name "$VM" --run-command-name "$RC" \
    --instance-view --query "instanceView.executionState" -o tsv 2>/dev/null || echo Unknown)"
  echo "  run-command state: ${state:-Unknown}"
  case "$state" in Succeeded | Failed | TimedOut | Canceled | Error) break ;; esac
  sleep 30
done

result="$(az vm run-command show -g "$RG" --vm-name "$VM" --run-command-name "$RC" \
  --instance-view --query "instanceView.output" -o tsv 2>/dev/null)"
rcerr="$(az vm run-command show -g "$RG" --vm-name "$VM" --run-command-name "$RC" \
  --instance-view --query "instanceView.error" -o tsv 2>/dev/null)"
echo "----- run-command output (executionState=$state) -----"
echo "$result"
[ -n "$rcerr" ] && { echo "----- stderr -----"; echo "$rcerr"; }

# On anything but a clean pass, dump where provisioning actually got to (the run-command output is
# truncated/buffered, so this reads the marker + agent-log tail directly) before teardown.
if ! printf '%s\n' "$result" | grep -q 'COLD-PROVISION PASS'; then
  echo "----- on-failure diagnostic (marker + log tail) -----"
  az vm run-command invoke -g "$RG" -n "$VM" --command-id RunPowerShellScript --scripts \
    'if(Test-Path "C:\ProgramData\UFOAgent\envs\ufo2.json"){"MARKER: "+(Get-Content "C:\ProgramData\UFOAgent\envs\ufo2.json" -Raw)}else{"MARKER: none"}; "PY311: "+(Test-Path "C:\Program Files\Python311\python.exe"); "--- log tail ---"; Get-Content "C:\ProgramData\UFOAgent\logs\ufoagent.log" -Tail 25 -ErrorAction SilentlyContinue' \
    --query "value[0].message" -o tsv 2>/dev/null || echo "(diagnostic unavailable)"
fi

if printf '%s\n' "$result" | grep -q 'COLD-PROVISION PASS'; then
  echo "=== COLD-PROVISION: PASS ==="
  exit 0
else
  echo "=== COLD-PROVISION: FAIL (executionState=$state) ==="
  exit 1
fi
