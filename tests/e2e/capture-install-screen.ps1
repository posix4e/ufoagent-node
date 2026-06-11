# Screenshot the REAL Inno wizard for the site (launched with no silent flags), then kill it
# before any file copy — install.ps1 does the actual silent install right after. This is the
# genuine first screen a user sees, published like the task shots.
$ErrorActionPreference = 'Continue'
. "$PSScriptRoot/helpers.ps1"
$setup = (Get-ChildItem -Recurse dl -Filter ufoagent-setup.exe | Select-Object -First 1).FullName
if (-not $setup) { throw 'installer artifact missing' }
Add-MpPreference -ExclusionPath (Join-Path (Get-Location) 'dl') -ErrorAction SilentlyContinue
(New-Object -ComObject Shell.Application).MinimizeAll()
$p = Start-Process $setup -PassThru
Start-Sleep -Seconds 8
Save-Shot 'install-screen'
# Inno runs as a loader + extracted .tmp child — kill both before they copy anything.
Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue
Get-Process | Where-Object { $_.Name -like '*ufoagent-setup*' } | Stop-Process -Force -ErrorAction SilentlyContinue
