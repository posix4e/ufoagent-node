# The ONE end-to-end journey, run inside the VM's interactive desktop session while the HOST screen-records
# the display. Walks the whole user story linearly, asserting each step; the recording is sliced into the
# per-phase marketing gifs by phase boundaries. Emits:
#   out\phases.json  - [{label,start,end,ok,error,crop:{x,y,w,h}}] guest epoch-ms boundaries + the window
#                      rect to crop each gif to (host slices + crops by these)
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

# Win32 for per-phase window cropping: capture the target window's rect so the host crops each gif to it
# (the full 1280x800 desktop buries the real content). SW_RESTORE+SetForeground brings it to the front first.
Add-Type @"
using System; using System.Runtime.InteropServices;
public struct RECT { public int Left, Top, Right, Bottom; }
public static class Win32 {
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int n);
}
"@

# Declutter: WS2025 auto-opens Server Manager + an Azure-Arc nag that otherwise sit behind every shot.
# (The cold snapshot also disables its auto-open; this is the belt-and-suspenders.)
Get-Process ServerManager -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue

$BetaUrl  = if ($env:UFOAGENT_BETA_URL) { $env:UFOAGENT_BETA_URL } else { 'https://github.com/ufoagent/ufoagent-node/releases/download/beta/ufoagent-setup.exe' }
$marker   = 'C:\ProgramData\UFOAgent\envs\ufo2.json'
$haveTok  = [bool]$env:CI_AGENT_TOKEN
$haveAdm  = [bool]($env:CI_ADMIN_TOKEN -and $env:CI_AGENT_ID)

$phases = New-Object System.Collections.ArrayList
$script:curCrop = $null
function Now { [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds() }
function Get-FgRect {
  $h = [Win32]::GetForegroundWindow(); $r = New-Object RECT
  if ($h -ne [IntPtr]::Zero -and [Win32]::GetWindowRect($h, [ref]$r)) {
    $w = $r.Right - $r.Left; $ht = $r.Bottom - $r.Top
    if ($w -gt 80 -and $ht -gt 80) { return [ordered]@{ x = $r.Left; y = $r.Top; w = $w; h = $ht } }
  }
  $null
}
function Focus-Proc([string]$name) {
  $p = Get-Process $name -ErrorAction SilentlyContinue | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1
  if ($p) { [Win32]::ShowWindow($p.MainWindowHandle, 9) | Out-Null; [Win32]::SetForegroundWindow($p.MainWindowHandle) | Out-Null; Start-Sleep -Milliseconds 500 }
}
# Capture the crop for the current phase: focus $proc (if given), then record the foreground window rect.
function Set-Crop([string]$proc = $null) { if ($proc) { Focus-Proc $proc }; $script:curCrop = Get-FgRect }

function Write-Result($status) {
  [pscustomobject]@{ status = $status; ended = (Now); phases = $phases } |
    ConvertTo-Json -Depth 6 | Set-Content (Join-Path $OUT 'result.json') -Encoding Ascii
}
function Phase([string]$name, [scriptblock]$body) {
  Write-Host "=== phase: $name ==="
  $script:curCrop = $null
  $s = Now; $ok = $true; $err = ''; $skip = $false
  try { $skip = (& $body) -eq 'SKIP' } catch { $ok = $false; $err = "$($_.Exception.Message)" }
  $e = Now
  $obj = [ordered]@{ label = $name; start = $s; end = $e; ok = $ok; skipped = $skip; error = $err }
  if ($script:curCrop) { $obj.crop = $script:curCrop }
  [void]$phases.Add([pscustomobject]$obj)
  ($phases | ConvertTo-Json -Depth 4) | Set-Content (Join-Path $OUT 'phases.json') -Encoding Ascii
  if (-not $ok) { Write-Host "PHASE FAILED: $name : $err"; Write-Result 'FAIL'; throw "phase '$name' failed: $err" }
}
[pscustomobject]@{ status = 'RUNNING'; started = (Now) } | ConvertTo-Json | Set-Content (Join-Path $OUT 'result.json') -Encoding Ascii

# 1) INSTALL - the REAL installer in this session; /SILENT shows the visible "UFOAgent setup" console
#    streaming the uv provisioning. Detached (bootstrap --pause hangs); poll the marker to ready.
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
  Set-Crop   # the "UFOAgent setup" console is foreground here
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

# 3) LINK - show the REAL QR/pairing screen for the recording, then link functionally via the CI token.
$script:linkProc = $null
Phase 'link' {
  $script:linkProc = Start-Process $Exe -ArgumentList 'link', '--force' -PassThru -WindowStyle Normal
  Start-Sleep -Seconds 9
  Set-Crop   # the link/QR console is foreground
  if ($haveTok) {
    & $Exe configure --agent-token $env:CI_AGENT_TOKEN
    $connected = Wait-For -TimeoutSec 180 -StreamAgentLog -Condition {
      (Test-Path $AgentLog) -and (Select-String -Path $AgentLog -Pattern 'ws: connected to control plane' -Quiet)
    }
    if (-not $connected) { throw 'agent never connected to the control plane over WebSocket' }
    Write-Host 'linked + WS connected'
  } else {
    Write-Host 'no CI token: showed the QR screen only (functional link skipped)'; return 'SKIP'
  }
}
Stop-Process -Id $script:linkProc.Id -Force -ErrorAction SilentlyContinue

# 4) LOCAL TASK - `ufoagent run`; Notepad must open + the message typed (UIA). The timed phase ENDS on the
#    typed-Notepad frame (clean gif + still); UFO2 is left to finish AFTER the phase window.
$script:localProc = $null
Phase 'local' {
  Stop-UfoWindows
  $rout = Join-Path $ROOT 'local-run.out.txt'; $rerr = Join-Path $ROOT 'local-run.err.txt'
  $script:localProc = Start-Process $Exe -ArgumentList 'run --task adhoc -r "Open Notepad and type the message: hello from ufoagent"' -RedirectStandardOutput $rout -RedirectStandardError $rerr -PassThru
  $opened = Wait-For -TimeoutSec 360 -StreamAgentLog -Condition { [bool](Get-Process notepad -ErrorAction SilentlyContinue) }
  if (-not $opened) { Show-FileTail 'run stdout' $rout; Show-FileTail 'run stderr' $rerr; Show-FileTail 'agent log' $AgentLog 40; throw 'local run_task did not open Notepad' }
  Assert-TypedVerdict (Wait-NotepadTyped) 'local run_task'
  Set-Crop 'notepad'; Start-Sleep 1   # end frame = Notepad with the typed message
}
if ($script:localProc) { $null = $script:localProc.WaitForExit(240000) }   # let UFO2 finish (outside the timed window)

# 5) REMOTE TASK - enqueue via the control-plane command API -> WS -> tray -> UFO2. Same clean end frame.
Phase 'remote' {
  if (-not $haveAdm) { Write-Host 'no CI admin token: skipping remote task'; return 'SKIP' }
  Stop-UfoWindows
  $resp = Send-NodeCommand 'run_task' 'Open Notepad and type the message: hello from ufoagent'
  Write-Host "enqueued run_task: id=$($resp.id) status=$($resp.status)"
  $script:remoteId = $resp.id
  $opened = Wait-For -TimeoutSec 300 -StreamAgentLog -Condition { [bool](Get-Process notepad -ErrorAction SilentlyContinue) }
  if (-not $opened) { Show-FileTail 'agent log' $AgentLog 40; throw 'remote run_task did not open Notepad' }
  Assert-TypedVerdict (Wait-NotepadTyped -StreamAgentLog) 'remote run_task'
  Set-Crop 'notepad'; Start-Sleep 1
}
if ($haveAdm -and $script:remoteId) {
  $null = Wait-For -TimeoutSec 240 -StreamAgentLog -Condition { $c = Get-NodeCommand $script:remoteId; $c -and ($c.status -eq 'done' -or $c.status -eq 'failed') }
  Stop-UfoWindows
}

# 6) ACTIVITY - the on-device LLM recap. Show it on screen (Notepad) so the gif captures the real recap
#    text, not whatever window happened to be foreground.
Phase 'activity' {
  if (-not $haveTok) { Write-Host 'no CI token: skipping activity recap (needs a linked node)'; return 'SKIP' }
  $summary = & $Exe activity | Out-String
  Write-Host '=== activity summary ==='; Write-Host $summary
  if ([string]::IsNullOrWhiteSpace($summary)) { throw 'activity summary was empty' }
  if ($summary -match 'summary unavailable') { throw 'activity fell back to the raw listing (LLM path did not run)' }
  Stop-UfoWindows
  $script:recap = Join-Path $ROOT 'activity-recap.txt'
  "What's this node been doing?`r`n`r`n$($summary.Trim())" | Set-Content $script:recap -Encoding Ascii
  Start-Process notepad $script:recap
  $shown = Wait-For -TimeoutSec 30 -PollSec 2 -Condition { [bool](Get-Process notepad -ErrorAction SilentlyContinue) }
  if (-not $shown) { throw 'recap Notepad did not open' }
  Set-Crop 'notepad'; Start-Sleep 3
}
Stop-UfoWindows

# 7) DASHBOARD - the mission-control view itself. Capture this node's REAL desktop via the live
#    `screenshot` command (tray -> GDI -> R2), then open the dashboard (a CI-token preview of the REAL
#    CI-tenant data, since the headless VM browser can't do GitHub OAuth) on this node's detail and
#    record it. Also save the raw captured desktop still for the website demo + /preview. Non-fatal:
#    if the installed (beta) agent predates the screenshot feature, the phase SKIPs rather than failing
#    the journey, so the existing gifs keep flowing until a beta with the feature ships.
Phase 'dashboard' {
  if (-not $haveAdm) { Write-Host 'no CI admin token: skipping dashboard phase'; return 'SKIP' }
  $edge = @("$env:ProgramFiles\Microsoft\Edge\Application\msedge.exe", "${env:ProgramFiles(x86)}\Microsoft\Edge\Application\msedge.exe") |
    Where-Object { Test-Path $_ } | Select-Object -First 1
  if (-not $edge) { Write-Host 'no Microsoft Edge on this box: skipping dashboard phase'; return 'SKIP' }

  # Put something real on the desktop so the captured screenshot shows the node working.
  if ($script:recap -and (Test-Path $script:recap)) { Start-Process notepad $script:recap; Start-Sleep 2 }

  # 7a) capture this node's desktop through the REAL screenshot command.
  $shot = Send-NodeCommand 'screenshot'
  Write-Host "enqueued screenshot: id=$($shot.id) status=$($shot.status)"
  $done = Wait-For -TimeoutSec 90 -StreamAgentLog -Condition { $c = Get-NodeCommand $shot.id; $c -and ($c.status -eq 'done' -or $c.status -eq 'failed') }
  $c = if ($shot.id) { Get-NodeCommand $shot.id } else { $null }
  if (-not $done -or -not $c -or $c.status -ne 'done') {
    Write-Host "screenshot not available (installed agent may predate the feature): status=$($c.status) result=$($c.result)"
    Stop-UfoWindows; return 'SKIP'
  }
  Write-Host "screenshot captured: $($c.result)"
  Stop-UfoWindows   # clear the recap notepad before showing the browser

  # 7b) save the raw captured desktop still (published next to the gifs; powers the website demo + /preview).
  $shotUrl = "https://app.ufoagent.xyz/api/agents/$env:CI_AGENT_ID/screenshot/latest"
  try { Invoke-WebRequest -UseBasicParsing -Headers (Get-ApiHeaders) -Uri $shotUrl -OutFile (Join-Path $OUT 'node-desktop.png'); Write-Host 'saved node-desktop.png' }
  catch { Write-Host "could not save node-desktop.png: $($_.Exception.Message)" }

  # 7c) open mission control (real data via the CI-token preview) on this node's detail and record it.
  #     App mode = no toolbar/address bar, so the token in the URL never appears in the gif; InPrivate
  #     avoids first-run/profile nags.
  $cp = 'https://app.ufoagent.xyz/preview/ci?token=' + [Uri]::EscapeDataString($env:CI_ADMIN_TOKEN) + '&node=' + [Uri]::EscapeDataString($env:CI_AGENT_ID)
  Start-Process $edge -ArgumentList '--inprivate', '--no-first-run', '--no-default-browser-check', '--window-size=1280,800', ('--app=' + $cp)
  Start-Sleep 12   # let the page load + the inlined screenshot render
  Set-Crop 'msedge'
  Start-Sleep 2
}
Get-Process msedge -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Stop-UfoWindows

Write-Result 'PASS'
Write-Host 'JOURNEY DONE: PASS'
