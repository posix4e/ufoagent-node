# Drive the tray menu with FlaUI (Run a task -> PowerShell prompt; View log -> Notepad; "What's
# this node been doing?" -> the native LLM recap dialog), then assert: the actions landed in the
# log, AND a tray-spawned action ACTUALLY produced a visible window. That window check is the
# regression guard — the tray is console-less, so before the ShellExecute fix every console child
# (Run a task / Link / Repair) launched with no window and the menu silently did nothing, yet the
# log line still appeared. A logged-but-invisible action is the exact bug; assert the window exists.
$ErrorActionPreference = 'Stop'
. "$PSScriptRoot/helpers.ps1"
Get-Process powershell -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue # clean slate
dotnet run --project tests/gui/TrayTest -- "$Exe" "$env:RUNNER_TEMP\shots" --attach
foreach ($pat in @('tray: menu action . run a task', 'tray: menu action . view log', 'tray: menu action . activity summary')) {
  $ok = Wait-For -TimeoutSec 30 -PollSec 2 -Condition {
    (Test-Path $AgentLog) -and (Select-String -Path $AgentLog -Pattern $pat -Quiet)
  }
  if (-not $ok) { Show-FileTail 'agent log' $AgentLog 40; throw "tray action did not show up in the log: $pat" }
  Write-Host "ok: $pat"
}
# Regression guard: 'Run a task' spawns a PowerShell prompt (Read-Host) from the console-less tray.
# It must have a VISIBLE window. With the old console-less spawn it had none -> this fails.
$prompt = Wait-For -TimeoutSec 30 -PollSec 2 -Condition {
  [bool](Get-Process powershell -ErrorAction SilentlyContinue | Where-Object { $_.MainWindowHandle -ne 0 })
}
if (-not $prompt) {
  Show-FileTail 'agent log' $AgentLog 40
  throw "Run a task logged its action but opened NO visible window (the console-less tray-spawn bug)"
}
Write-Host 'ok: Run a task opened a visible prompt window (tray child windows are visible)'
Get-Process powershell -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue # dismiss the prompt
$summary = & $Exe activity | Out-String
Write-Host '=== activity summary ==='; Write-Host $summary
if ([string]::IsNullOrWhiteSpace($summary)) { throw 'activity summary was empty' }
if ($summary -match 'summary unavailable') { throw 'activity fell back to the raw listing (LLM path did not run)' }
if ($summary -match "hasn't run any commands yet") { throw 'activity found no command history (expected remote + local run_task entries)' }
Write-Host 'activity summary is a real on-device LLM recap'
