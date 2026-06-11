# Start the tray (the run_task executor in Session 1) and wait until it advertises liveness —
# the marker the service checks before queuing a GUI task.
$ErrorActionPreference = 'Stop'
. "$PSScriptRoot/helpers.ps1"
Start-Process $Exe -ArgumentList 'tray'
$alive = "$env:ProgramData\UFOAgent\tasks\tray-alive"
if (-not (Wait-For -TimeoutSec 120 -PollSec 2 -Condition { Test-Path $alive })) {
  throw 'tray did not start (no tray-alive marker)'
}
Write-Host 'tray running; tray-alive present'
