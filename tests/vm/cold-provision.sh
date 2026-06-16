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
SIZE="${SIZE:-Standard_D2s_v5}"
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

# One headless run-command does it all: install the beta, poll the env marker to terminal, kill the
# lingering `bootstrap --pause` console, then probe. run-command returns when the SCRIPT exits.
# (The script's own ~25m poll budget bounds it; the run-command extension caps at 90m.)
echo "=== install + provision + probe (headless run-command) ==="
result="$(az vm run-command invoke -g "$RG" -n "$VM" --command-id RunPowerShellScript \
  --scripts @"$HERE/provision-and-probe.ps1" \
  --query "value[0].message" -o tsv)"
echo "$result"

if printf '%s\n' "$result" | grep -q 'COLD-PROVISION PASS'; then
  echo "=== COLD-PROVISION: PASS ==="
  exit 0
else
  echo "=== COLD-PROVISION: FAIL ==="
  exit 1
fi
