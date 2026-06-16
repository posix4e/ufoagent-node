# Runs via run-command (Session 0). Captures a screenshot of the OPEN tray menu in the interactive
# Session 1 — visual proof the 🛸 icon renders + its menu works on a real auto-logon VM. Uses the same
# recipe as the proven FlaUI TrayTest (tests/gui/TrayTest), in pure PowerShell UIAutomation so no .NET
# SDK is needed on the bare box: open the "Show Hidden Icons" overflow, find the UFOAgent tray-icon
# button, right-click it, screenshot, and read back the menu items to verify.
#
# Bridges Session 0 -> 1 with an interactive scheduled task (the proven bridge). Writes the shots to
# C:\shots\*.png and a verdict to C:\shot-verdict.txt, which the driver reads + uploads.
$ErrorActionPreference = 'Stop'
$user = if ($env:GUI_USER) { $env:GUI_USER } else { 'ufoadmin' }
Remove-Item C:\shot-verdict.txt -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path C:\shots | Out-Null

$worker = @'
$ErrorActionPreference = "SilentlyContinue"
Add-Type -AssemblyName System.Windows.Forms, System.Drawing, UIAutomationClient, UIAutomationTypes
Add-Type @"
using System; using System.Runtime.InteropServices;
public class M {
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint x, uint y, uint d, IntPtr e);
  public static void Click(int x,int y,bool right){ SetCursorPos(x,y);
    uint dn = right?0x08u:0x02u, up = right?0x10u:0x04u;
    mouse_event(dn,0,0,0,IntPtr.Zero); mouse_event(up,0,0,0,IntPtr.Zero); }
}
"@
$A = [System.Windows.Automation.AutomationElement]
$root = $A::RootElement
function Shot($name) {
  $b = [System.Windows.Forms.SystemInformation]::VirtualScreen
  $bmp = New-Object System.Drawing.Bitmap $b.Width, $b.Height
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $g.CopyFromScreen($b.X, $b.Y, 0, 0, $bmp.Size)
  $bmp.Save("C:\shots\$name.png"); $g.Dispose(); $bmp.Dispose()
}
function Buttons($needle) {
  $cond = New-Object System.Windows.Automation.PropertyCondition($A::ControlTypeProperty, [System.Windows.Automation.ControlType]::Button)
  $root.FindAll([System.Windows.Automation.TreeScope]::Descendants, $cond) | Where-Object { $_.Current.Name -like "*$needle*" }
}
function ClickCenter($el, $right) {
  $r = $el.Current.BoundingRectangle
  [M]::Click([int]($r.X + $r.Width / 2), [int]($r.Y + $r.Height / 2), $right)
}
Start-Sleep -Seconds 3
Shot "00-desktop"
# Open the notification-area overflow (on Server 2025 the icon lives in this flyout).
$chev = Buttons "Show Hidden Icons" | Select-Object -First 1
if (-not $chev) { $chev = Buttons "hidden icons" | Select-Object -First 1 }
if ($chev) { ClickCenter $chev $false; Start-Sleep -Seconds 2; Shot "01-overflow" }
# Find the UFOAgent tray-icon BUTTON (not the 'UFOAgent setup' console window).
$icon = Buttons "UFOAgent" | Select-Object -First 1
if (-not $icon) { "FAIL: UFOAgent tray icon button not found" | Set-Content C:\shot-verdict.txt; Shot "FAIL-no-icon"; return }
ClickCenter $icon $true   # right-click opens the menu
Start-Sleep -Seconds 2
Shot "02-menu"
# Read the menu items back to confirm it's OUR menu. The desktop-wide MenuItem enumeration also picks
# up other windows' menus (e.g. Server Manager), so require several DISTINCTIVE UFOAgent items rather
# than any single (ambiguous) word.
$mcond = New-Object System.Windows.Automation.PropertyCondition($A::ControlTypeProperty, [System.Windows.Automation.ControlType]::MenuItem)
$items = $root.FindAll([System.Windows.Automation.TreeScope]::Descendants, $mcond) | ForEach-Object { $_.Current.Name } | Where-Object { $_ }
$joined = ($items -join " | ")
$ours = @('Re-link', 'Repair', 'Run a task', 'been doing', 'View log', 'Open dashboard', 'Quit')
$hits = @($ours | Where-Object { $joined -match [regex]::Escape($_) }).Count
if ($hits -ge 4) { "PASS ($hits/7 UFOAgent menu items) menu=[$joined]" | Set-Content C:\shot-verdict.txt }
else { "FAIL only $hits/7 UFOAgent menu items found: [$joined]" | Set-Content C:\shot-verdict.txt }
'@
Set-Content -Path 'C:\screenshot-worker.ps1' -Value $worker -Encoding UTF8

Write-Output "creating interactive screenshot task as $user ..."
schtasks /Create /TN GuiScreenshot /TR "powershell.exe -NoProfile -ExecutionPolicy Bypass -File C:\screenshot-worker.ps1" /SC ONCE /ST 23:56 /RU $user /IT /RL HIGHEST /F | Out-Null
schtasks /Run /TN GuiScreenshot | Out-Null

$deadline = (Get-Date).AddSeconds(120)
while ((Get-Date) -lt $deadline) { if (Test-Path C:\shot-verdict.txt) { break }; Start-Sleep -Seconds 3 }
schtasks /Delete /TN GuiScreenshot /F 2>$null | Out-Null

if (Test-Path C:\shot-verdict.txt) {
  $v = (Get-Content C:\shot-verdict.txt -Raw).Trim()
  Write-Output "VERDICT: $v"
  Write-Output ("SHOTS: " + ((Get-ChildItem C:\shots\*.png -ErrorAction SilentlyContinue | ForEach-Object { $_.Name }) -join ', '))
  if ($v -like 'PASS*') { Write-Output 'SCREENSHOT OK' }
} else {
  Write-Output 'VERDICT: (none — the interactive screenshot task did not run in Session 1)'
}
