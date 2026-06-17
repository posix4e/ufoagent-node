#!/usr/bin/env bash
# Capture a screenshot of the rendered tray menu on a real auto-logon VM — visual proof the 🛸 icon
# works (the fix), and the first Phase-3 marketing-style artifact. Provisioning is irrelevant to the
# tray UI, so this skips it (~12 min): install → autologon → reboot → the SYSTEM service launches the
# tray IN-SESSION (icon renders) → an interactive Session-1 task opens the menu + screenshots it →
# pull the PNGs off the no-public-IP VM via an ephemeral storage blob (VM uploads, driver downloads).
#
# Usage:  UFOAGENT_BETA_URL=<icon-fix installer> tests/vm/tray-screenshot.sh
# Env:    LOCATION (westus2) SIZE (Standard_D2s_v5) RG (auto) OUT (./tray-shots)
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
LOCATION="${LOCATION:-westus2}"
SIZE="${SIZE:-Standard_D2s_v5}"
IMAGE="${IMAGE:-MicrosoftWindowsServer:WindowsServer:2025-datacenter-azure-edition:latest}"
STAMP="$(date +%Y%m%d-%H%M%S)"
RG="${RG:-ufo-shot-${STAMP}-$$}"
VM="shot"
ADMIN_USER="ufoadmin"
ADMIN_PASS="Shot1!$(openssl rand -hex 10)Zz"
OUT="${OUT:-$PWD/tray-shots}"
SA="ufoshot$(date +%H%M%S)$RANDOM"; SA="$(echo "$SA" | tr -cd 'a-z0-9' | cut -c1-24)"

cleanup() { echo "=== teardown: deleting RG $RG ==="; az group delete -n "$RG" --yes --no-wait >/dev/null 2>&1 || true; }
trap cleanup EXIT

rc() {
  az vm run-command invoke -g "$RG" -n "$VM" --command-id RunPowerShellScript \
    --scripts "$1" --query "value[0].message" -o tsv 2>/dev/null || true
}

echo "=== creating RG $RG ($LOCATION) + bare VM ($SIZE) ==="
az group create -n "$RG" -l "$LOCATION" -o none
az vm create -g "$RG" -n "$VM" --image "$IMAGE" --size "$SIZE" \
  --admin-username "$ADMIN_USER" --admin-password "$ADMIN_PASS" \
  --public-ip-address "" --nsg "" -o none
echo "VM up."

# Ephemeral storage to ferry the PNGs off the no-IP VM: the VM uploads with a write SAS; the driver
# downloads with the account key.
echo "=== creating ephemeral storage $SA ==="
az storage account create -n "$SA" -g "$RG" -l "$LOCATION" --sku Standard_LRS --kind StorageV2 --allow-blob-public-access false -o none
KEY="$(az storage account keys list -n "$SA" -g "$RG" --query "[0].value" -o tsv)"
az storage container create -n shots --account-name "$SA" --account-key "$KEY" -o none
EXPIRY="$(date -u -v+2H +%Y-%m-%dT%H:%MZ 2>/dev/null || date -u -d '+2 hours' +%Y-%m-%dT%H:%MZ)"
WSAS="$(az storage container generate-sas -n shots --account-name "$SA" --account-key "$KEY" --permissions cw --expiry "$EXPIRY" --https-only -o tsv)"
BASE="https://${SA}.blob.core.windows.net/shots"

# 1) install (skip provisioning) + autologon, forwarding creds + the icon-fix installer URL.
echo "=== install icon-fix beta + autologon ==="
prelude="\$env:GUI_USER='$ADMIN_USER'; \$env:GUI_PASS='$ADMIN_PASS';"
[ -n "${UFOAGENT_BETA_URL:-}" ] && prelude="$prelude \$env:UFOAGENT_BETA_URL='$UFOAGENT_BETA_URL';"
install_out="$(rc "$prelude
$(cat "$HERE/tray-probe-install.ps1")")"
echo "$install_out" | tail -4
printf '%s\n' "$install_out" | grep -q 'TRAY-PROBE-INSTALL OK' || { echo "install/autologon failed"; exit 1; }

# 2) reboot → service launches the tray in-session (icon renders).
echo "=== rebooting ==="
az vm restart -g "$RG" -n "$VM" -o none

# 3) wait for the tray to come alive in Session 1.
echo "=== waiting for tray-alive ==="
alive_ps='$m="C:\ProgramData\UFOAgent\tasks\tray-alive"; if(Test-Path $m){"TRAY age="+[int]((Get-Date)-(Get-Item $m).LastWriteTime).TotalSeconds+"s"}else{"TRAY none"}'
tray_ok=0; tdeadline=$(( $(date +%s) + 10 * 60 ))
while [ "$(date +%s)" -lt "$tdeadline" ]; do
  out="$(rc "$alive_ps")"; echo "  $out"
  age="${out##*age=}"; age="${age%%s*}"; age="$(printf '%s' "$age" | tr -dc '0-9')"
  if [ -n "$age" ] && [ "$age" -le 90 ]; then tray_ok=1; break; fi
  sleep 20
done
[ "$tray_ok" -eq 1 ] || { echo "tray never came alive"; exit 1; }

# 4) open the menu + screenshot it (interactive Session-1 task).
echo "=== capturing the tray menu in Session 1 ==="
shot_out="$(rc "\$env:GUI_USER='$ADMIN_USER';
$(cat "$HERE/session1-screenshot.ps1")")"
echo "$shot_out"

# 5) VM uploads the PNGs to the blob; driver downloads them.
echo "=== uploading shots from the VM ==="
rc "\$sas='$WSAS'; \$base='$BASE'
Get-ChildItem C:\\shots\\*.png -ErrorAction SilentlyContinue | ForEach-Object {
  try { Invoke-WebRequest -Method Put -InFile \$_.FullName -Headers @{'x-ms-blob-type'='BlockBlob'} -Uri (\"\$base/\$(\$_.Name)?\$sas\") -UseBasicParsing | Out-Null; Write-Output ('uploaded ' + \$_.Name) } catch { Write-Output ('upload failed ' + \$_.Name + ': ' + \$_.Exception.Message) }
}"

echo "=== downloading shots -> $OUT ==="
mkdir -p "$OUT"
az storage blob download-batch -d "$OUT" -s shots --account-name "$SA" --account-key "$KEY" -o none 2>/dev/null || true
ls -la "$OUT" 2>/dev/null

if printf '%s\n' "$shot_out" | grep -q 'SCREENSHOT OK'; then
  echo "=== TRAY-SCREENSHOT: PASS (menu captured) ==="; exit 0
else
  echo "=== TRAY-SCREENSHOT: shots pulled but menu verdict not PASS (see above) ==="; exit 1
fi
