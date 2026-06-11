# Drive the tray menu with FlaUI (View log -> Notepad; "What's this node been doing?" -> the
# native dialog with the LLM recap), assert both actions land in the agent log, then run
# `ufoagent activity` directly and assert it's a GENUINE LLM recap — not the fallback listing.
$ErrorActionPreference = 'Stop'
. "$PSScriptRoot/helpers.ps1"
dotnet run --project tests/gui/TrayTest -- "$Exe" "$env:RUNNER_TEMP\shots" --attach
foreach ($pat in @('tray: menu action . view log', 'tray: menu action . activity summary')) {
  $ok = Wait-For -TimeoutSec 30 -PollSec 2 -Condition {
    (Test-Path $AgentLog) -and (Select-String -Path $AgentLog -Pattern $pat -Quiet)
  }
  if (-not $ok) { Show-FileTail 'agent log' $AgentLog 40; throw "tray action did not show up in the log: $pat" }
  Write-Host "ok: $pat"
}
$summary = & $Exe activity | Out-String
Write-Host '=== activity summary ==='; Write-Host $summary
if ([string]::IsNullOrWhiteSpace($summary)) { throw 'activity summary was empty' }
if ($summary -match 'summary unavailable') { throw 'activity fell back to the raw listing (LLM path did not run)' }
if ($summary -match "hasn't run any commands yet") { throw 'activity found no command history (expected remote + local run_task entries)' }
Write-Host 'activity summary is a real on-device LLM recap'
