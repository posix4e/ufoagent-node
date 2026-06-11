# Drive the tray menu with FlaUI (Run a task -> PowerShell prompt; View log -> Notepad; "What's
# this node been doing?" -> the native LLM recap dialog). The regression guard lives in TrayTest:
# it asserts 'Run a task' actually produced a VISIBLE PowerShell window (exit 4 if not) — before the
# ShellExecute fix the console-less tray spawned it with no window and the menu silently did nothing,
# yet the log line still appeared. Here we also confirm each action reached the log.
$ErrorActionPreference = 'Stop'
. "$PSScriptRoot/helpers.ps1"
Get-Process powershell -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue # clean slate
dotnet run --project tests/gui/TrayTest -- "$Exe" "$env:RUNNER_TEMP\shots" --attach
if ($LASTEXITCODE -ne 0) { Show-FileTail 'agent log' $AgentLog 40; throw "TrayTest failed (exit $LASTEXITCODE) — a tray action did not produce its window" }
foreach ($pat in @('tray: menu action . run a task', 'tray: menu action . view log', 'tray: menu action . activity summary')) {
  $ok = Wait-For -TimeoutSec 30 -PollSec 2 -Condition {
    (Test-Path $AgentLog) -and (Select-String -Path $AgentLog -Pattern $pat -Quiet)
  }
  if (-not $ok) { Show-FileTail 'agent log' $AgentLog 40; throw "tray action did not show up in the log: $pat" }
  Write-Host "ok: $pat"
}
Get-Process powershell -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue # dismiss the prompt
$summary = & $Exe activity | Out-String
Write-Host '=== activity summary ==='; Write-Host $summary
if ([string]::IsNullOrWhiteSpace($summary)) { throw 'activity summary was empty' }
if ($summary -match 'summary unavailable') { throw 'activity fell back to the raw listing (LLM path did not run)' }
if ($summary -match "hasn't run any commands yet") { throw 'activity found no command history (expected remote + local run_task entries)' }
Write-Host 'activity summary is a real on-device LLM recap'
