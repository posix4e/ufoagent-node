# Shared e2e helpers, dot-sourced by the step scripts:  . "$PSScriptRoot/helpers.ps1"
# One copy instead of per-step pastes (the duplicated Get-NotepadText diverged once already).

$script:Shots = Join-Path $env:RUNNER_TEMP 'shots'
New-Item -ItemType Directory -Force -Path $script:Shots | Out-Null

# Shared paths every step needs.
$script:Exe      = "$env:ProgramFiles\UFOAgent\ufoagent.exe"
$script:AgentLog = "$env:ProgramData\UFOAgent\logs\ufoagent.log"
$script:AgentLogSeen = 0

# Print agent-log lines that appeared since the last call — live progress in the step output.
function Show-NewAgentLog {
  if (-not (Test-Path $script:AgentLog)) { return }
  $lines = Get-Content $script:AgentLog
  if ($lines.Count -gt $script:AgentLogSeen) {
    $lines[$script:AgentLogSeen..($lines.Count - 1)] | ForEach-Object { Write-Host "  [agent] $_" }
    $script:AgentLogSeen = $lines.Count
  }
}

# THE poll loop (the steps used to paste 11 copies of this): run $Condition every $PollSec until
# it returns truthy or $TimeoutSec lapses; returns $true/$false. -StreamAgentLog tails ufoagent.log
# between polls. A throw inside $Condition propagates (used for fail-fast checks).
function Wait-For([scriptblock]$Condition, [int]$TimeoutSec, [int]$PollSec = 5, [switch]$StreamAgentLog) {
  $deadline = (Get-Date).AddSeconds($TimeoutSec)
  while ((Get-Date) -lt $deadline) {
    if ($StreamAgentLog) { Show-NewAgentLog }
    if (& $Condition) { return $true }
    Start-Sleep -Seconds $PollSec
  }
  return $false
}

# Clear UFO2's desktop debris (and the deferred bootstrap console) so screenshots are legible and
# the notepad.exe assertion starts fresh.
function Stop-UfoWindows {
  Get-Process | Where-Object { $_.MainWindowTitle -like '*UFOAgent setup*' } | Stop-Process -Force -ErrorAction SilentlyContinue
  Get-Process notepad, python, pythonw -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
}

# Control-plane command API (CI service token, valid only for the CI tenant's nodes).
function Get-ApiHeaders { @{ authorization = "Bearer $env:CI_ADMIN_TOKEN"; 'content-type' = 'application/json' } }
function Get-ApiBase { "https://app.ufoagent.xyz/api/agents/$env:CI_AGENT_ID/commands" }
function Send-NodeCommand([string]$Kind, [string]$Request, [string]$Env) {
  $body = @{ kind = $Kind }; if ($Request) { $body.request = $Request }; if ($Env) { $body.env = $Env }
  Invoke-RestMethod -Method POST -Uri (Get-ApiBase) -Headers (Get-ApiHeaders) -Body ($body | ConvertTo-Json -Compress)
}
function Get-NodeCommand([string]$Id) {
  (Invoke-RestMethod -Uri (Get-ApiBase) -Headers (Get-ApiHeaders)).commands | Where-Object { $_.id -eq $Id }
}

function Show-FileTail([string]$Label, [string]$Path, [int]$Lines = 50) {
  Write-Host "--- $Label ---"
  if (Test-Path $Path) { Get-Content $Path -Tail $Lines | Write-Host }
}

# Hidden desktop frame recorder (record-frames.ps1): frames land in $Shots/frames-<name>;
# assemble-gifs.ps1 turns them into <name>-task.gif after the tasks pass. Stop = sentinel file.
function Start-FrameRecorder([string]$Name, [double]$IntervalSec = 2) {
  $stop = Join-Path $env:RUNNER_TEMP "stop-$Name-rec"
  Remove-Item $stop -ErrorAction SilentlyContinue
  Start-Process pwsh -WindowStyle Hidden -ArgumentList '-NoProfile', '-File', (Join-Path $PSScriptRoot 'record-frames.ps1'),
    '-OutDir', (Join-Path $script:Shots "frames-$Name"), '-StopFile', $stop, '-IntervalSec', $IntervalSec
}
function Stop-FrameRecorder([string]$Name) {
  New-Item -ItemType File -Path (Join-Path $env:RUNNER_TEMP "stop-$Name-rec") -Force | Out-Null
}

# Wait for UFO2 to type $Want into Notepad (read back via UIAutomation, no faking).
# Returns @{ Typed = <full text or $null>; ReadOk = <could the document be read at all> }.
function Wait-NotepadTyped([string]$Want = 'ufoagent', [int]$TimeoutSec = 90, [switch]$StreamAgentLog) {
  $out = @{ Typed = $null; ReadOk = $false }
  $deadline = (Get-Date).AddSeconds($TimeoutSec)
  while ((Get-Date) -lt $deadline) {
    if ($StreamAgentLog) { Show-NewAgentLog }
    $txt = Get-NotepadText
    if ($null -ne $txt) { $out.ReadOk = $true; if ($txt -match $Want) { $out.Typed = $txt; break } }
    Start-Sleep -Seconds 5
  }
  $out
}

# Verdict on the typed-text check: pass loudly; warn if UIA couldn't read the document at all
# (the screenshot still carries the evidence); fail if it was readable but the message never came.
function Assert-TypedVerdict($R, [string]$Context) {
  if ($R.Typed) { Write-Host "real task performed — UFO2 typed into Notepad: $($R.Typed.Trim())" }
  elseif (-not $R.ReadOk) { Dump-NotepadTree; Write-Host '::warning::could not read Notepad text via UIAutomation; relying on the screenshot for the typed message' }
  else { Show-FileTail 'agent log' $script:AgentLog; throw "${Context}: Notepad opened but UFO2 never typed the message (no 'ufoagent' in the document)" }
}

# Full-desktop screenshot into the shared shots dir (uploaded as the tray-screenshots artifact;
# curated names are published to the `screenshots` branch on green main).
function Save-Shot([string]$Name) {
  try {
    Add-Type -AssemblyName System.Windows.Forms,System.Drawing
    $b = [System.Windows.Forms.SystemInformation]::VirtualScreen
    $bmp = New-Object System.Drawing.Bitmap($b.Width, $b.Height)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen($b.X, $b.Y, 0, 0, $bmp.Size)
    $bmp.Save((Join-Path $script:Shots "$Name.png")); $g.Dispose(); $bmp.Dispose()
    Write-Host "screenshot: $Name.png"
  } catch { Write-Host "screenshot $Name failed: $($_.Exception.Message)" }
}

# Read Notepad's text back via UIAutomation to verify UFO2 actually TYPED the message (not just
# opened the app). Classic Notepad (this image): the editor is ClassName 'Edit' exposed as a Pane,
# with the typed text in its Name (and ValuePattern) — read that first; modern Notepad falls back
# to a Document/Edit control exposing TextPattern. Returns $null only if truly unreadable.
function Get-NotepadText {
  try {
    Add-Type -AssemblyName UIAutomationClient,UIAutomationTypes -ErrorAction Stop
    $np = Get-Process notepad -ErrorAction SilentlyContinue | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1
    if (-not $np) { return $null }
    $ae = [System.Windows.Automation.AutomationElement]::FromHandle($np.MainWindowHandle)
    if (-not $ae) { return $null }
    foreach ($el in $ae.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition)) {
      if ($el.Current.ClassName -eq 'Edit') {
        try { $vp = $el.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern); if ($vp -and $vp.Current.Value) { return $vp.Current.Value } } catch {}
        if ($el.Current.Name) { return $el.Current.Name }
      }
    }
    foreach ($ct in @([System.Windows.Automation.ControlType]::Document, [System.Windows.Automation.ControlType]::Edit)) {
      $el = $ae.FindFirst([System.Windows.Automation.TreeScope]::Descendants,
              (New-Object System.Windows.Automation.PropertyCondition([System.Windows.Automation.AutomationElement]::ControlTypeProperty, $ct)))
      if (-not $el) { continue }
      try { $tp = $el.GetCurrentPattern([System.Windows.Automation.TextPattern]::Pattern); if ($tp) { return $tp.DocumentRange.GetText(-1) } } catch {}
    }
    return $null
  } catch { return $null }
}

# On a failed read, dump Notepad's control tree so the real structure is visible in the log.
function Dump-NotepadTree {
  try {
    $np = Get-Process notepad -ErrorAction SilentlyContinue | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1
    if (-not $np) { Write-Host 'Notepad tree: no notepad window'; return }
    $ae = [System.Windows.Automation.AutomationElement]::FromHandle($np.MainWindowHandle)
    Write-Host '--- Notepad UIA tree ---'
    $ae.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition) |
      ForEach-Object { Write-Host ("  [{0}] class='{1}' name='{2}'" -f $_.Current.ControlType.ProgrammaticName, $_.Current.ClassName, $_.Current.Name) }
  } catch { Write-Host "Dump-NotepadTree error: $($_.Exception.Message)" }
}
