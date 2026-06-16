# FAST tray-autostart probe (pre-reboot half). Runs on the bare VM via run-command (Session 0).
# Installs the beta but DELIBERATELY skips the ~30-min UFO2 provisioning — the tray-autostart bug is
# independent of provisioning, so this turns a ~45-min iteration into ~12-min. Then sets auto-logon so
# the driver's reboot lands us in an interactive Session 1 where we can observe whether the installer's
# {commonstartup} shortcut actually brings the tray to life.
#
# Env: GUI_USER (admin to auto-logon), GUI_PASS (their password)
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

$exe = "$env:ProgramFiles\UFOAgent\ufoagent.exe"
$startup = "$env:ProgramData\Microsoft\Windows\Start Menu\Programs\StartUp\UFOAgent.lnk"
$betaUrl = if ($env:UFOAGENT_BETA_URL) { $env:UFOAGENT_BETA_URL } else { 'https://github.com/ufoagent/ufoagent-node/releases/download/beta/ufoagent-setup.exe' }
$user = if ($env:GUI_USER) { $env:GUI_USER } else { 'ufoadmin' }
$pass = $env:GUI_PASS

# install — detached (its [Run] launches `bootstrap --pause`, which blocks on Read-Host headless).
$setup = "$env:TEMP\ufoagent-setup.exe"
Write-Output "downloading: $betaUrl"
Invoke-WebRequest -UseBasicParsing $betaUrl -OutFile $setup
Write-Output 'launching installer /VERYSILENT (detached) ...'
Start-Process $setup -ArgumentList '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/NOCANCEL'
$d = (Get-Date).AddMinutes(5)
while (-not (Test-Path $exe) -and (Get-Date) -lt $d) { Start-Sleep -Seconds 5 }
if (-not (Test-Path $exe)) { throw 'ufoagent.exe was not installed within 5 min' }
Write-Output ("installed: " + (& $exe version))

# confirm the {commonstartup} shortcut the tray autostart relies on actually got created.
$d = (Get-Date).AddMinutes(1)
while (-not (Test-Path $startup) -and (Get-Date) -lt $d) { Start-Sleep -Seconds 2 }
Write-Output ("startup shortcut present: " + (Test-Path $startup) + "  ($startup)")

# stop the install-time procs (the Session-0 tray + the hung --pause console + the agent) so nothing
# lingers; the reboot is what re-launches the tray, in the interactive session, via the shortcut.
1..6 | ForEach-Object { Get-Process ufoagent, ufoagent-setup -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue; Start-Sleep -Seconds 1 }

# enable auto-logon (HKLM; run-command is SYSTEM). Takes effect on the driver's next reboot.
# (autologon also suppresses the first-logon privacy OOBE page that would otherwise steal foreground.)
if (-not $pass) { throw 'GUI_PASS not provided' }
& $exe autologon --user $user --password $pass
Write-Output "TRAY-PROBE-INSTALL OK (user=$user)"
