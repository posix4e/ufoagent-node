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

# The Session-1 worker: open Notepad, focus it, type the marker, then read it back via the CLIPBOARD
# (Select-All + Copy). Clipboard read-back is version-independent — unlike UIAutomation it doesn't
# depend on Notepad's control tree (Windows Server 2025 ships the modern Notepad, whose UIA tree
# differs from the classic Edit control), and it proves the keystrokes actually reached the app.
$worker = @'
$ErrorActionPreference = "SilentlyContinue"
Add-Type -AssemblyName System.Windows.Forms
$marker = "hello from ufoagent"
Get-Process notepad -EA SilentlyContinue | Stop-Process -Force -EA SilentlyContinue
Start-Sleep -Seconds 1
$proc = Start-Process notepad.exe -PassThru
# Wait for the editor window to exist (modern Notepad's first cold launch can take several seconds).
$np = $null
for ($i = 0; $i -lt 30; $i++) {
  $np = Get-Process -Id $proc.Id -EA SilentlyContinue | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1
  if ($np) { break }
  Start-Sleep -Milliseconds 500
}
$sid = $proc.SessionId
# Focus the window so SendKeys lands in the editor, then type + select-all + copy.
$wsh = New-Object -ComObject WScript.Shell
$wsh.AppActivate($proc.Id) | Out-Null
Start-Sleep -Seconds 1
Set-Clipboard -Value ""
[System.Windows.Forms.SendKeys]::SendWait($marker)
Start-Sleep -Seconds 1
[System.Windows.Forms.SendKeys]::SendWait("^a^c")
Start-Sleep -Seconds 1
$got = (Get-Clipboard -Raw)
if ($got -and ($got -match "ufoagent")) { "PASS session=$sid text=" + $got.Trim() | Set-Content C:\gui-verdict.txt }
elseif (-not $np) { "FAIL session=$sid Notepad window never appeared" | Set-Content C:\gui-verdict.txt }
else { "FAIL session=$sid clipboard=[" + ("$got").Trim() + "]" | Set-Content C:\gui-verdict.txt }
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
