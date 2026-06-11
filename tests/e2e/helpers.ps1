# Shared e2e helpers, dot-sourced by ci.yml steps:  . ./tests/e2e/helpers.ps1
# One copy instead of per-step pastes (the duplicated Get-NotepadText diverged once already).

$script:Shots = Join-Path $env:RUNNER_TEMP 'shots'
New-Item -ItemType Directory -Force -Path $script:Shots | Out-Null

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
