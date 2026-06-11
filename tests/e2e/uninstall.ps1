# Always-run teardown: stop any agent processes and run the Inno uninstaller silently.
Get-Process ufoagent -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
$u = "$env:ProgramFiles\UFOAgent\unins000.exe"
if (Test-Path $u) { Start-Process $u -ArgumentList '/VERYSILENT' -Wait; Write-Host 'uninstalled' }
