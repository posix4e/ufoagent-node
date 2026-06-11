# Local-path run_task: `ufoagent run` on the box itself. Same no-faking bar as the remote path:
# Notepad must open, the message must be typed (UIAutomation), and UFO2 finishes on its own so
# cmd_run records DONE (exit 0). UFO2's own stdout/stderr are captured for diagnosis.
$ErrorActionPreference = 'Stop'
. "$PSScriptRoot/helpers.ps1"
Stop-UfoWindows
$rout = "$env:RUNNER_TEMP\local-run.out.txt"; $rerr = "$env:RUNNER_TEMP\local-run.err.txt"
# Record this run too (assembled into the site's local-task.gif).
Start-FrameRecorder 'local'
# Pass the command line as a SINGLE quoted string — Start-Process -ArgumentList with an array
# does NOT quote the spaced request, so clap saw 'Open'/'the'/'Notepad'/... as stray positional
# args and bailed ("try --help"). -PassThru so we can wait for UFO2 to finish, not kill it early.
$proc = Start-Process $Exe -ArgumentList 'run --task adhoc -r "Open Notepad and type the message: hello from ufoagent"' -RedirectStandardOutput $rout -RedirectStandardError $rerr -PassThru

$opened = Wait-For -TimeoutSec 360 -StreamAgentLog -Condition { [bool](Get-Process notepad -ErrorAction SilentlyContinue) }
if (-not $opened) {
  Save-Shot '11-local-notepad'
  Show-FileTail 'ufoagent run stdout' $rout
  Show-FileTail 'ufoagent run stderr' $rerr
  Show-FileTail 'agent log' $AgentLog 40
  throw 'local `ufoagent run` did not actually open Notepad (UFO2 did not perform the task)'
}
Write-Host 'local run_task: UFO2 opened Notepad via `ufoagent run`'
Save-Shot '12-local-opening'

$typed = Wait-NotepadTyped
Save-Shot '11-local-notepad'
Stop-FrameRecorder 'local' # the action is over; don't record the wait-for-exit tail
Assert-TypedVerdict $typed 'local run_task'

# Let UFO2 finish so the exit code is its own verdict; dump its output either way.
if (-not $proc.WaitForExit(240000)) { Write-Host '::warning::local UFO2 did not finish in 4 min — killing' }
else { Write-Host "local run exit code: $($proc.ExitCode)" }
Show-FileTail 'UFO2 stdout (tail)' $rout
Show-FileTail 'UFO2 stderr (tail)' $rerr
Stop-UfoWindows
