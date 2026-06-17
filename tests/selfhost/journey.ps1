# The ONE end-to-end journey, run inside the VM's interactive desktop session while the HOST screen-records
# the display. Walks the whole user story linearly, asserting each step; the recording is sliced into the
# per-phase marketing gifs by phase boundaries. Emits:
#   out\phases.json  - [{label,start,end,ok,error}] guest epoch-ms boundaries (host slices by these)
#   out\result.json  - {status: RUNNING|PASS|FAIL, phases}
# Reuses the proven assertion logic from tests\e2e (staged alongside as .\e2e). The host records, so this
# does NOT start any in-guest frame recorder. Kept ASCII so Windows PowerShell 5.1 parses it cleanly.
$ErrorActionPreference = 'Stop'
$ROOT = $PSScriptRoot
$E2E  = Join-Path $ROOT 'e2e'
$OUT  = Join-Path $ROOT 'out'
$env:RUNNER_TEMP = $ROOT                      # helpers.ps1 derives $Shots from this
New-Item -ItemType Directory -Force $OUT, (Join-Path $ROOT 'shots'), (Join-Path $ROOT 'dl') | Out-Null
if (Test-Path (Join-Path $ROOT 'env.ps1')) { . (Join-Path $ROOT 'env.ps1') }   # CI tokens, if staged
. (Join-Path $E2E 'helpers.ps1')
Add-Type -AssemblyName System.Windows.Forms, System.Drawing

$BetaUrl  = if ($env:UFOAGENT_BETA_URL) { $env:UFOAGENT_BETA_URL } else { 'https://github.com/ufoagent/ufoagent-node/releases/download/beta/ufoagent-setup.exe' }
$marker   = 'C:\ProgramData\UFOAgent\envs\ufo2.json'
$haveTok  = [bool]$env:CI_AGENT_TOKEN
$haveAdm  = [bool]($env:CI_ADMIN_TOKEN -and $env:CI_AGENT_ID)

$phases = New-Object System.Collections.ArrayList
function Now { [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds() }
function Write-Result($status) {
  [pscustomobject]@{ status = $status; ended = (Now); phases = $phases } |
    ConvertTo-Json -Depth 6 | Set-Content (Join-Path $OUT 'result.json') -Encoding Ascii
}
function Phase([string]$name, [scriptblock]$body) {
  Write-Host "=== phase: $name ==="
  $s = Now; $ok = $true; $err = ''; $skip = $false
  try { $skip = (& $body) -eq 'SKIP' } catch { $ok = $false; $err = "$($_.Exception.Message)" }
  $e = Now
  [void]$phases.Add([pscustomobject]@{ label = $name; start = $s; end = $e; ok = $ok; skipped = $skip; error = $err })
  ($phases | ConvertTo-Json -Depth 4) | Set-Content (Join-Path $OUT 'phases.json') -Encoding Ascii
  if (-not $ok) { Write-Host "PHASE FAILED: $name : $err"; Write-Result 'FAIL'; throw "phase '$name' failed: $err" }
}
[pscustomobject]@{ status = 'RUNNING'; started = (Now) } | ConvertTo-Json | Set-Content (Join-Path $OUT 'result.json') -Encoding Ascii

# 1) INSTALL - run the REAL installer in this interactive session; /SILENT shows the progress + the
#    visible "UFOAgent setup" console streaming the uv provisioning. Detached (the bootstrap --pause
#    console hangs), poll the env marker to ready. This is the faithful install the user actually sees.
Phase 'install' {
  [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
  $setup = Join-Path $ROOT 'dl\ufoagent-setup.exe'
  Invoke-WebRequest -UseBasicParsing $BetaUrl -OutFile $setup
  Write-Host "installer: $((Get-Item $setup).Length) bytes"
  Start-Process $setup -ArgumentList '/SILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/NOCANCEL'
  $ready = Wait-For -TimeoutSec 600 -PollSec 5 -StreamAgentLog -Condition {
    if (Test-Path $marker) { if ((Get-Content $marker -Raw | ConvertFrom-Json).state -in @('ready', 'broken')) { return $true } }
    $false
  }
  if (-not $ready) { throw 'provisioning did not reach a terminal state in 10m' }
  $state = (Get-Content $marker -Raw | ConvertFrom-Json).state
  if ($state -ne 'ready') { throw "provisioning state=$state (expected ready)" }
  Write-Host 'install + provision: UFO2 ready'
}

# 2) TRAY - the installer launched the tray in this session; confirm it is alive.
Phase 'tray' {
  $tm = 'C:\ProgramData\UFOAgent\tasks\tray-alive'
  $alive = Wait-For -TimeoutSec 150 -PollSec 5 -Condition {
    (Test-Path $tm) -and (((Get-Date) - (Get-Item $tm).LastWriteTime).TotalSeconds -le 120)
  }
  if (-not $alive) { throw 'tray did not come alive in this session' }
  Write-Host 'tray alive in the interactive session'; Start-Sleep 2
}

# 3) LINK - show the REAL QR/pairing screen for the recording, then link functionally via the
#    pre-provisioned CI token (the QR auto-approve is intentionally out of scope; see plan).
Phase 'link' {
  $lp = Start-Process $Exe -ArgumentList 'link', '--force' -PassThru -WindowStyle Normal
  Start-Sleep -Seconds 9
  if ($haveTok) {
    & $Exe configure --agent-token $env:CI_AGENT_TOKEN
    $connected = Wait-For -TimeoutSec 180 -StreamAgentLog -Condition {
      (Test-Path $AgentLog) -and (Select-String -Path $AgentLog -Pattern 'ws: connected to control plane' -Quiet)
    }
    Stop-Process -Id $lp.Id -Force -ErrorAction SilentlyContinue
    if (-not $connected) { throw 'agent never connected to the control plane over WebSocket' }
    Write-Host 'linked + WS connected'
  } else {
    Stop-Process -Id $lp.Id -Force -ErrorAction SilentlyContinue
    Write-Host 'no CI token: showed the QR screen only (functional link skipped)'; return 'SKIP'
  }
}

# 4) LOCAL TASK - `ufoagent run` on the box; Notepad must open and the message must be typed (UIA).
Phase 'local' {
  Stop-UfoWindows
  $rout = Join-Path $ROOT 'local-run.out.txt'; $rerr = Join-Path $ROOT 'local-run.err.txt'
  $proc = Start-Process $Exe -ArgumentList 'run --task adhoc -r "Open Notepad and type the message: hello from ufoagent"' -RedirectStandardOutput $rout -RedirectStandardError $rerr -PassThru
  $opened = Wait-For -TimeoutSec 360 -StreamAgentLog -Condition { [bool](Get-Process notepad -ErrorAction SilentlyContinue) }
  if (-not $opened) { Show-FileTail 'run stdout' $rout; Show-FileTail 'run stderr' $rerr; Show-FileTail 'agent log' $AgentLog 40; throw 'local run_task did not open Notepad' }
  Assert-TypedVerdict (Wait-NotepadTyped) 'local run_task'
  if (-not $proc.WaitForExit(240000)) { Write-Host '::warning::local UFO2 did not finish in 4m' } else { Write-Host "local run exit=$($proc.ExitCode)" }
  Stop-UfoWindows
}

# 5) REMOTE TASK - enqueue via the control-plane command API -> WS -> tray -> UFO2.
Phase 'remote' {
  if (-not $haveAdm) { Write-Host 'no CI admin token: skipping remote task'; return 'SKIP' }
  Stop-UfoWindows
  $resp = Send-NodeCommand 'run_task' 'Open Notepad and type the message: hello from ufoagent'
  Write-Host "enqueued run_task: id=$($resp.id) status=$($resp.status)"
  $opened = Wait-For -TimeoutSec 300 -StreamAgentLog -Condition { [bool](Get-Process notepad -ErrorAction SilentlyContinue) }
  if (-not $opened) { Show-FileTail 'agent log' $AgentLog 40; throw 'remote run_task did not open Notepad' }
  Assert-TypedVerdict (Wait-NotepadTyped -StreamAgentLog) 'remote run_task'
  $null = Wait-For -TimeoutSec 240 -StreamAgentLog -Condition { $c = Get-NodeCommand $resp.id; $c -and ($c.status -eq 'done' -or $c.status -eq 'failed') }
  Stop-UfoWindows
}

# 6) ACTIVITY - the on-device LLM recap of what the node has been doing.
Phase 'activity' {
  if (-not $haveTok) { Write-Host 'no CI token: skipping activity recap (needs a linked node)'; return 'SKIP' }
  $summary = & $Exe activity | Out-String
  Write-Host '=== activity summary ==='; Write-Host $summary
  if ([string]::IsNullOrWhiteSpace($summary)) { throw 'activity summary was empty' }
  if ($summary -match 'summary unavailable') { throw 'activity fell back to the raw listing (LLM path did not run)' }
  Start-Sleep 3
}

Write-Result 'PASS'
Write-Host 'JOURNEY DONE: PASS'
