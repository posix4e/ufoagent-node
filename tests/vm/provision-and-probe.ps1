# Runs ON the bare VM via `az vm run-command` (Session 0). Installs the beta, waits for the
# installer's background provisioning to finish by polling the env marker, then asserts UFO2's GUI
# stack actually imports — the cold-start readiness verdict the GitHub runner can't make.
#
# Why poll the marker (not wait on the bootstrap process): the installer launches `bootstrap --pause`,
# which blocks on "Press Enter" forever in this headless console. So we (a) detect completion via the
# marker (written before the pause) and (b) kill the lingering bootstrap console before exiting, so
# run-command's process tree is clean and the call returns instead of hanging on the pause.
#
# Windows PowerShell 5.1 compatible. Prints "COLD-PROVISION PASS" only when everything holds; the
# driver greps for that token. Throws (text surfaces in run-command output) on any failure.
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

$marker = 'C:\ProgramData\UFOAgent\envs\ufo2.json'
$log = 'C:\ProgramData\UFOAgent\logs\ufoagent.log'
$cfgPath = 'C:\ProgramData\UFOAgent\config.json'
$timeoutMin = if ($env:PROVISION_TIMEOUT_MIN) { [int]$env:PROVISION_TIMEOUT_MIN } else { 25 }
$betaUrl = if ($env:UFOAGENT_BETA_URL) { $env:UFOAGENT_BETA_URL } else { 'https://github.com/ufoagent/ufoagent-node/releases/download/beta/ufoagent-setup.exe' }

# 1) install the agent (the installer kicks UFO2 provisioning in the background)
$exe = "$env:TEMP\ufoagent-setup.exe"
Write-Output "downloading: $betaUrl"
Invoke-WebRequest -UseBasicParsing $betaUrl -OutFile $exe
Write-Output ("installer bytes: " + (Get-Item $exe).Length)
Write-Output 'installing /VERYSILENT ...'
Start-Process $exe -ArgumentList '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/NOCANCEL' -Wait
if (-not (Test-Path "$env:ProgramFiles\UFOAgent\ufoagent.exe")) { throw 'ufoagent.exe was not installed' }
Write-Output ("installed: " + (& "$env:ProgramFiles\UFOAgent\ufoagent.exe" version))

# 2) poll the env marker until terminal (state printed only on change to keep output small)
$deadline = (Get-Date).AddMinutes($timeoutMin)
$state = ''
$last = ''
while ((Get-Date) -lt $deadline) {
  if (Test-Path $marker) {
    try { $state = (Get-Content $marker -Raw | ConvertFrom-Json).state } catch { $state = 'unreadable' }
  } elseif ((Test-Path $log) -and (Select-String -Path $log -Pattern 'UFO2 setup failed' -Quiet)) {
    $state = 'setup-failed'
  } else { $state = 'no-marker' }
  if ($state -ne $last) { Write-Output ("provisioning state: $state"); $last = $state }
  if ($state -in @('ready', 'broken', 'setup-failed')) { break }
  Start-Sleep -Seconds 30
}

# 3) kill the lingering `bootstrap --pause` console (and any agent procs) so run-command's process
#    tree is clean and this call returns. Provisioning is already terminal by here.
Get-Process ufoagent -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue

# 4) cold-start readiness verdict
Write-Output "final marker state: $state"
if ($state -ne 'ready') {
  if (Test-Path $marker) { Write-Output ('marker: ' + (Get-Content $marker -Raw)) }
  if (Test-Path $log) { Write-Output '--- agent log tail ---'; Get-Content $log -Tail 40 }
  throw "env did not reach ready (state=$state)"
}
$cfg = Get-Content $cfgPath -Raw | ConvertFrom-Json
$py = $cfg.python
Write-Output "venv python: $py"
if (-not $py -or -not (Test-Path $py)) { throw "config.python missing: $py" }

# THE bug class: UFO2's GUI stack must import (fails on a bare VM unless the agent provisioned the
# VC++/MFC runtime + ran pywin32 post-install). This is what shipped green on the contaminated runner.
& $py -c "import win32ui, pywinauto; print('win32ui + pywinauto import OK')"
if ($LASTEXITCODE -ne 0) { throw 'win32ui/pywinauto import FAILED on the bare VM (VC++/MFC not provisioned)' }

# The agent must have auto-installed Python — a bare Windows Server has none.
if (-not (Test-Path "$env:ProgramFiles\Python311\python.exe")) {
  throw 'agent did not auto-install Python (ProgramFiles\Python311\python.exe missing)'
}
Write-Output 'agent auto-installed Python: OK'

Write-Output 'COLD-PROVISION PASS'
