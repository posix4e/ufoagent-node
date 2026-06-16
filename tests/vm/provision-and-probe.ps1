# Runs ON the bare VM via the MANAGED `az vm run-command` (Session 0). Installs the beta, waits for
# the installer's background bootstrap to provision UFO2, then asserts UFO2's GUI stack imports — the
# cold-start readiness verdict the GitHub runner can't make (it preinstalls VC++/MFC + Python).
#
# IMPORTANT (don't -Wait the installer): the installer's [Run] launches `ufoagent bootstrap --pause`,
# which blocks on Read-Host in this headless Session-0 console. With -Wait the installer never exits
# and the whole run-command hangs. So we launch it DETACHED, poll the env marker the bootstrap writes,
# then kill the lingering `--pause` console (+ installer + agent) so the run-command returns cleanly.
#
# Windows PowerShell 5.1 compatible. Prints "COLD-PROVISION PASS" only when everything holds.
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

$exe = "$env:ProgramFiles\UFOAgent\ufoagent.exe"
$marker = 'C:\ProgramData\UFOAgent\envs\ufo2.json'
$log = 'C:\ProgramData\UFOAgent\logs\ufoagent.log'
$cfgPath = 'C:\ProgramData\UFOAgent\config.json'
$betaUrl = if ($env:UFOAGENT_BETA_URL) { $env:UFOAGENT_BETA_URL } else { 'https://github.com/ufoagent/ufoagent-node/releases/download/beta/ufoagent-setup.exe' }

# 1) install — launch DETACHED (no -Wait; see header).
$setup = "$env:TEMP\ufoagent-setup.exe"
Write-Output "downloading: $betaUrl"
Invoke-WebRequest -UseBasicParsing $betaUrl -OutFile $setup
Write-Output ("installer bytes: " + (Get-Item $setup).Length)
Write-Output 'launching installer /VERYSILENT (detached) ...'
Start-Process $setup -ArgumentList '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/NOCANCEL'
$d = (Get-Date).AddMinutes(5)
while (-not (Test-Path $exe) -and (Get-Date) -lt $d) { Start-Sleep -Seconds 5 }
if (-not (Test-Path $exe)) { throw 'ufoagent.exe was not installed within 5 min' }
Write-Output ("installed: " + (& $exe version))

# 2) poll the env marker until the installer's background bootstrap finishes provisioning UFO2
#    (installs Python + UFO2 + the VC++/MFC runtime, then verify_ufo_imports marks ready/broken).
Write-Output 'waiting for provisioning ...'
$state = 'no-marker'; $last = ''
$d = (Get-Date).AddMinutes(40)
while ((Get-Date) -lt $d) {
  if (Test-Path $marker) { try { $state = (Get-Content $marker -Raw | ConvertFrom-Json).state } catch { $state = 'unreadable' } }
  elseif ((Test-Path $log) -and (Select-String -Path $log -Pattern 'UFO2 setup failed' -Quiet)) { $state = 'setup-failed' }
  if ($state -ne $last) { Write-Output "  provisioning: $state"; $last = $state }
  if ($state -in @('ready', 'broken', 'setup-failed')) { break }
  Start-Sleep -Seconds 20
}

# 3) stop the hung `bootstrap --pause` console + installer + agent procs so nothing lingers for the
#    run-command to wait on (provisioning is terminal by here).
1..6 | ForEach-Object { Get-Process ufoagent, ufoagent-setup -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue; Start-Sleep -Seconds 1 }

# 4) cold-start readiness verdict
Write-Output "final marker state: $state"
if ($state -ne 'ready') {
  if (Test-Path $marker) { Write-Output ('marker: ' + (Get-Content $marker -Raw)) }
  if (Test-Path $log) { Write-Output '--- log tail ---'; Get-Content $log -Tail 40 }
  throw "env did not reach ready (state=$state)"
}
$cfg = Get-Content $cfgPath -Raw | ConvertFrom-Json
$py = $cfg.python
Write-Output "venv python: $py"
if (-not $py -or -not (Test-Path $py)) { throw "config.python missing: $py" }

# THE bug class: UFO2's GUI stack must import (fails on a bare VM unless the agent provisioned the
# VC++/MFC runtime + ran pywin32 post-install).
& $py -c "import win32ui, pywinauto; print('win32ui + pywinauto import OK')"
if ($LASTEXITCODE -ne 0) { throw 'win32ui/pywinauto import FAILED on the bare VM (VC++/MFC not provisioned)' }

# The agent must have auto-installed Python — a bare Windows Server has none.
if (-not (Test-Path "$env:ProgramFiles\Python311\python.exe")) {
  throw 'agent did not auto-install Python (ProgramFiles\Python311\python.exe missing)'
}
Write-Output 'agent auto-installed Python: OK'

Write-Output 'COLD-PROVISION PASS'
