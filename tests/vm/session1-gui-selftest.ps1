# Runs via run-command (Session 0). De-risks the headful bridge WITHOUT UFO2/LLM: proves we can drive
# a GUI in the auto-logged-in interactive Session 1 and read it back via UIAutomation — exactly the
# mechanism the real run_task verification will use (UFO2 typing into Notepad, verified the same way).
#
# How: run-command is Session 0 (no desktop). So we write a Session-1 script that opens Notepad, types
# a marker, and reads it back via UIAutomation; then run it as an INTERACTIVE scheduled task (/IT) so
# it executes in the logged-on user's Session 1. We poll the verdict file it writes and print it.
$ErrorActionPreference = 'Stop'
$user = if ($env:GUI_USER) { $env:GUI_USER } else { 'ufoadmin' }
$verdict = 'C:\gui-verdict.txt'
Remove-Item $verdict -ErrorAction SilentlyContinue

# The Session-1 worker: open Notepad, type the marker (SendKeys), then UIAutomation-read it back.
$worker = @'
$ErrorActionPreference = "SilentlyContinue"
Add-Type -AssemblyName System.Windows.Forms, UIAutomationClient, UIAutomationTypes
Get-Process notepad -EA SilentlyContinue | Stop-Process -Force -EA SilentlyContinue
Start-Process notepad.exe
Start-Sleep -Seconds 4
[System.Windows.Forms.SendKeys]::SendWait("hello from ufoagent")
Start-Sleep -Seconds 2
function Read-Notepad {
  $np = Get-Process notepad -EA SilentlyContinue | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1
  if (-not $np) { return $null }
  $ae = [System.Windows.Automation.AutomationElement]::FromHandle($np.MainWindowHandle)
  if (-not $ae) { return $null }
  foreach ($el in $ae.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition)) {
    if ($el.Current.ClassName -eq "Edit") {
      try { $vp = $el.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern); if ($vp.Current.Value) { return $vp.Current.Value } } catch {}
      if ($el.Current.Name) { return $el.Current.Name }
    }
  }
  foreach ($ct in @([System.Windows.Automation.ControlType]::Document, [System.Windows.Automation.ControlType]::Edit)) {
    $el = $ae.FindFirst([System.Windows.Automation.TreeScope]::Descendants, (New-Object System.Windows.Automation.PropertyCondition([System.Windows.Automation.AutomationElement]::ControlTypeProperty, $ct)))
    if ($el) { try { $tp = $el.GetCurrentPattern([System.Windows.Automation.TextPattern]::Pattern); if ($tp) { return $tp.DocumentRange.GetText(-1) } } catch {} }
  }
  return $null
}
$t = Read-Notepad
$sid = (Get-Process notepad -EA SilentlyContinue | Select-Object -First 1).SessionId
if ($t -and ($t -match "ufoagent")) { "PASS session=$sid text=" + $t.Trim() | Set-Content C:\gui-verdict.txt }
elseif ($null -ne $t) { "FAIL session=$sid text=" + $t.Trim() | Set-Content C:\gui-verdict.txt }
else { "FAIL session=$sid could not read Notepad via UIAutomation" | Set-Content C:\gui-verdict.txt }
Get-Process notepad -EA SilentlyContinue | Stop-Process -Force -EA SilentlyContinue
'@
Set-Content -Path 'C:\gui-worker.ps1' -Value $worker -Encoding UTF8

Write-Output "creating interactive scheduled task as $user ..."
schtasks /Create /TN GuiSelfTest /TR "powershell.exe -NoProfile -ExecutionPolicy Bypass -File C:\gui-worker.ps1" /SC ONCE /ST 23:59 /RU $user /IT /RL HIGHEST /F | Out-Null
schtasks /Run /TN GuiSelfTest | Out-Null

Write-Output "waiting for the Session-1 worker to write its verdict ..."
$deadline = (Get-Date).AddSeconds(120)
while ((Get-Date) -lt $deadline) { if (Test-Path $verdict) { break }; Start-Sleep -Seconds 3 }
schtasks /Delete /TN GuiSelfTest /F 2>$null | Out-Null

if (Test-Path $verdict) {
  $v = (Get-Content $verdict -Raw).Trim()
  Write-Output "VERDICT: $v"
  if ($v -like 'PASS*') { Write-Output 'SESSION1-GUI OK' }
} else {
  Write-Output 'VERDICT: (none — the interactive task did not run in Session 1; auto-logon/desktop issue)'
}
