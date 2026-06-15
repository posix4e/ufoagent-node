# Dashboard-path run_task: enqueue via the command API -> DO -> WS -> tray -> UFO2 drives the
# real desktop. NO faking: Notepad must open, the message must actually be typed (read back via
# UIAutomation), and UFO2 finishes on its own so the node records the task as DONE.
$ErrorActionPreference = 'Stop'
. "$PSScriptRoot/helpers.ps1"
Stop-UfoWindows # legible screenshots + a fresh notepad.exe assertion
# Record the whole run as frames (hidden helper, 1 shot / 2s) — assembled into the site's
# remote-task.gif after both tasks pass.
Start-FrameRecorder 'remote'
# Defaults to env=ufo2 server-side; this enqueue would 400 (env gate) unless the agent reported
# ufo2=ready — so reaching the task at all is the positive half of the environments-v1 gate.
$resp = Send-NodeCommand 'run_task' 'Open Notepad and type the message: hello from ufoagent'
Write-Host "enqueued run_task: id=$($resp.id) status=$($resp.status)"
$tlog = "$env:ProgramData\UFOAgent\tasks\logs\$($resp.id).txt"

$opened = Wait-For -TimeoutSec 300 -StreamAgentLog -Condition { [bool](Get-Process notepad -ErrorAction SilentlyContinue) }
if (-not $opened) {
  Save-Shot '10-remote-notepad'
  Show-FileTail 'agent log' $AgentLog
  Show-FileTail 'task transcript' $tlog 60
  throw 'UFO2 did not open Notepad — run_task did not actually perform the task'
}
Write-Host 'UFO2 opened Notepad on the desktop (notepad.exe running)'
Save-Shot '09-remote-opening'

# The real task: the message must be TYPED. Screenshot after, so the shot shows the text.
$typed = Wait-NotepadTyped -StreamAgentLog
Save-Shot '10-remote-notepad'
Stop-FrameRecorder 'remote' # the action is over; don't record the wait-for-exit tail
Assert-TypedVerdict $typed 'remote run_task'

# Let UFO2 FINISH on its own so the node records DONE (exit 0), keeping the on-node history —
# and the LLM activity recap built from it — accurate. Kill only if it over-runs.
$final = $null
$null = Wait-For -TimeoutSec 240 -StreamAgentLog -Condition {
  $cmd = Get-NodeCommand $resp.id
  if ($cmd -and ($cmd.status -eq 'done' -or $cmd.status -eq 'failed')) { $script:final = $cmd; return $true }
  $false
}
if ($final) { Write-Host "run_task result: status=$($final.status)" } else { Write-Host '::warning::run_task still pending after 4 min — cleaning up' }
Show-FileTail 'UFO2 transcript (tail)' $tlog
Stop-UfoWindows
