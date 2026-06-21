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
  [DllImport("kernel32.dll")] public static extern IntPtr GetConsoleWindow();
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int n);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int X, int Y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint flags, uint dx, uint dy, uint data, UIntPtr extraInfo);
}
"@
Add-Type -AssemblyName UIAutomationClient,UIAutomationTypes

# Declutter: WS2025 auto-opens Server Manager + an Azure-Arc nag that otherwise sit behind every shot.
# (The cold snapshot also disables its auto-open; this is the belt-and-suspenders.)
Get-Process ServerManager -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue

$BetaUrl  = if ($env:UFOAGENT_BETA_URL) { $env:UFOAGENT_BETA_URL } else { 'https://github.com/ufoagent/ufoagent-node/releases/download/beta/ufoagent-setup.exe' }
$marker   = 'C:\ProgramData\UFOAgent\envs\ufo2.json'
$haveTok  = [bool]$env:CI_AGENT_TOKEN
$haveAdm  = [bool]($env:CI_ADMIN_TOKEN -and $env:CI_AGENT_ID)

$phases = New-Object System.Collections.ArrayList
$script:curCrop = $null
$script:lastInstallDetail = ''
$script:lastRemoteStatus = ''
$script:lastBraveStatus = ''
$script:lastBambuStatus = ''
$script:BraveDownloadUrl = 'https://github.com/brave/brave-browser/releases/latest/download/BraveBrowserStandaloneSilentSetup.exe'
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
function Minimize-OwnConsole {
  $h = [Win32]::GetConsoleWindow()
  if ($h -ne [IntPtr]::Zero) {
    [Win32]::ShowWindow($h, 6) | Out-Null
    Start-Sleep -Milliseconds 800
  }
}
# Capture the crop for the current phase: focus $proc (if given), then record the foreground window rect.
function Set-Crop([string]$proc = $null) { if ($proc) { Focus-Proc $proc }; $script:curCrop = Get-FgRect }

function Get-UiaRect($el) {
  if (-not $el) { return $null }
  try {
    $r = $el.Current.BoundingRectangle
    if ($r.Width -gt 80 -and $r.Height -gt 80) {
      return [ordered]@{ x = [int]$r.X; y = [int]$r.Y; w = [int]$r.Width; h = [int]$r.Height }
    }
  } catch {}
  $null
}

function Get-UiaElements($root) {
  try { return $root.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition) }
  catch { return @() }
}

function Find-UiaByName([string]$needle, [string]$controlType = $null, [switch]$ChildrenOnly) {
  $root = [System.Windows.Automation.AutomationElement]::RootElement
  $scope = if ($ChildrenOnly) { [System.Windows.Automation.TreeScope]::Children } else { [System.Windows.Automation.TreeScope]::Descendants }
  $all = try { $root.FindAll($scope, [System.Windows.Automation.Condition]::TrueCondition) } catch { @() }
  foreach ($el in $all) {
    try {
      if ($controlType -and $el.Current.ControlType.ProgrammaticName -ne "ControlType.$controlType") { continue }
      if (($el.Current.Name + '') -like "*$needle*") { return $el }
    } catch {}
  }
  $null
}

function Invoke-UiaElement($el) {
  if (-not $el) { return $false }
  try {
    $p = $el.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern)
    if ($p) { $p.Invoke(); return $true }
  } catch {}
  try {
    $r = $el.Current.BoundingRectangle
    [Win32]::SetCursorPos([int]($r.X + ($r.Width / 2)), [int]($r.Y + ($r.Height / 2))) | Out-Null
    [Win32]::mouse_event(0x0002, 0, 0, 0, [UIntPtr]::Zero)
    [Win32]::mouse_event(0x0004, 0, 0, 0, [UIntPtr]::Zero)
    return $true
  } catch { return $false }
}

function Dismiss-UfoSetupConsole {
  Write-ProgressEvent 'phase_update' 'install' 'closing installer setup prompt'
  $win = Find-UiaByName 'UFOAgent setup' 'Window' -ChildrenOnly
  if ($win) {
    if (-not $script:curCrop) { $script:curCrop = Get-UiaRect $win }
    try {
      [Win32]::SetForegroundWindow([IntPtr]$win.Current.NativeWindowHandle) | Out-Null
      Start-Sleep -Milliseconds 500
      [System.Windows.Forms.SendKeys]::SendWait('{ENTER}')
      Start-Sleep -Seconds 1
    } catch {}
  }
  Get-Process -ErrorAction SilentlyContinue |
    Where-Object { $_.MainWindowTitle -like '*UFOAgent setup*' } |
    ForEach-Object {
      try {
        if (-not $_.CloseMainWindow()) { $_ | Stop-Process -Force -ErrorAction SilentlyContinue }
        Start-Sleep -Milliseconds 500
        if (-not $_.HasExited) { $_ | Stop-Process -Force -ErrorAction SilentlyContinue }
      } catch {}
    }
}

function RightClick-UiaElement($el) {
  if (-not $el) { return $false }
  try {
    $r = $el.Current.BoundingRectangle
    [Win32]::SetCursorPos([int]($r.X + ($r.Width / 2)), [int]($r.Y + ($r.Height / 2))) | Out-Null
    [Win32]::mouse_event(0x0008, 0, 0, 0, [UIntPtr]::Zero)
    [Win32]::mouse_event(0x0010, 0, 0, 0, [UIntPtr]::Zero)
    return $true
  } catch { return $false }
}

function Assert-ManagedUfoConfig {
  $system = 'C:\ProgramData\UFOAgent\ufo\config\ufo\system.yaml'
  if (-not (Test-Path $system)) { throw "UFO system config missing: $system" }
  $raw = Get-Content $system -Raw
  if ($raw -notmatch '(?m)^\s*USE_MCP:\s*False\b') {
    throw 'managed UFO config did not disable USE_MCP'
  }
  $mcp = 'C:\ProgramData\UFOAgent\ufo\config\ufo\mcp.yaml'
  if (-not (Test-Path $mcp)) { throw "UFO MCP config missing: $mcp" }
  $mcpRaw = Get-Content $mcp -Raw
  if ($mcpRaw -notmatch 'Managed by ufoagent') {
    throw 'managed UFO MCP config was not written'
  }
  foreach ($required in @('namespace: UICollector', 'namespace: HostUIExecutor', 'namespace: AppUIExecutor', 'namespace: CommandLineExecutor')) {
    if ($mcpRaw -notmatch [regex]::Escape($required)) {
      throw "managed UFO MCP config missing UI server: $required"
    }
  }
  foreach ($blocked in @('WordCOMExecutor', 'ExcelCOMExecutor', 'PowerPointCOMExecutor', 'type: http')) {
    if ($mcpRaw -match [regex]::Escape($blocked)) {
      throw "managed UFO MCP config exposes blocked server: $blocked"
    }
  }
  foreach ($py in @(
      'C:\ProgramData\UFOAgent\ufo\ufo\agents\agent\host_agent.py',
      'C:\ProgramData\UFOAgent\ufo\ufo\agents\agent\app_agent.py'
    )) {
    if (-not (Test-Path $py)) { throw "UFO MCP loader missing: $py" }
    $pyRaw = Get-Content $py -Raw
    if ($pyRaw -notmatch 'Managed by ufoagent: honor USE_MCP=False') {
      throw "UFO MCP loader was not patched to honor USE_MCP=False: $py"
    }
  }
  $cli = 'C:\ProgramData\UFOAgent\ufo\ufo\client\mcp\local_servers\cli_mcp_server.py'
  if (-not (Test-Path $cli)) { throw "UFO CLI launcher missing: $cli" }
  $cliRaw = Get-Content $cli -Raw
  foreach ($required in @('Managed by ufoagent: permit every CLI command', 'def _is_cli_command_allowed(command_str: str) -> bool:', 'return True')) {
    if ($cliRaw -notmatch [regex]::Escape($required)) {
      throw "UFO CLI launcher unrestricted policy missing: $required"
    }
  }
  foreach ($blocked in @('any(base == allowed.lower() for allowed in ALLOWED_CLI_COMMANDS)', 'pattern.search(command_str)')) {
    if ($cliRaw -match [regex]::Escape($blocked)) {
      throw "UFO CLI launcher still contains blocking policy: $blocked"
    }
  }
  Write-Host 'managed UFO config: USE_MCP=False, local GUI MCP servers, MCP loaders patched, CLI unrestricted'
}

function Wait-CommandTerminal([string]$id, [string]$label, [int]$TimeoutSec = 300) {
  $done = Wait-For -TimeoutSec $TimeoutSec -PollSec 5 -StreamAgentLog -Condition {
    $c = Get-NodeCommand $id
    $c -and ($c.status -eq 'done' -or $c.status -eq 'failed')
  }
  $c = if ($id) { Get-NodeCommand $id } else { $null }
  if (-not $done -or -not $c) { throw "$label did not finish in ${TimeoutSec}s" }
  Write-Host "$label result: status=$($c.status)"
  if ($c.status -ne 'done') { throw "$label failed: $($c.result)" }
  $c
}

function Wait-CommandTerminalAfterState([string]$id, [string]$label, [string]$phase, [int]$TimeoutSec = 120) {
  $done = Wait-For -TimeoutSec $TimeoutSec -PollSec 5 -StreamAgentLog -Condition {
    $c = Get-NodeCommand $id
    $c -and ($c.status -eq 'done' -or $c.status -eq 'failed')
  }
  $c = if ($id) { Get-NodeCommand $id } else { $null }
  if ($done -and $c) {
    Write-Host "$label result after state verified: status=$($c.status)"
    if ($c.status -ne 'done') {
      $summary = Get-CommandResultSummary $c
      if ($summary) { Write-ProgressEvent 'phase_update' $phase "$label failed after state verified: $summary" }
      throw "$label failed: $($c.result)"
    }
    return $c
  }
  $status = if ($c -and $c.status) { $c.status } else { 'missing' }
  Write-Host "::warning::$label still $status after state verified"
  Write-ProgressEvent 'phase_update' $phase "$label still $status after state verified"
  $c
}

function Get-CommandResultSummary($command, [int]$MaxLen = 220) {
  if (-not $command -or -not $command.result) { return }
  $summary = (($command.result + '') -replace '[^\x09\x0A\x0D\x20-\x7E]', ' ' -replace '\s+', ' ').Trim()
  if ($summary.Length -gt $MaxLen) { $summary = $summary.Substring(0, $MaxLen) + '...' }
  $summary
}

function Write-CommandResultProgress([string]$phase, $command) {
  $summary = Get-CommandResultSummary $command
  if (-not $summary) { return }
  Write-ProgressEvent 'phase_update' $phase "UFO result: $summary"
}

function Get-NodeCommands {
  $resp = Invoke-RestMethod -Uri (Get-ApiBase) -Headers (Get-ApiHeaders)
  if ($resp -and $resp.commands) { return @($resp.commands) }
  @()
}

function Get-CommandIdSet {
  $ids = @{}
  foreach ($cmd in (Get-NodeCommands)) {
    if ($cmd.id) { $ids[$cmd.id] = $true }
  }
  $ids
}

function Count-InterjectRedispatches([hashtable]$BeforeIds) {
  $count = 0
  foreach ($cmd in (Get-NodeCommands)) {
    if (-not $cmd.id -or $BeforeIds.ContainsKey($cmd.id)) { continue }
    if (($cmd.kind + '') -ne 'run_task') { continue }
    $args = $cmd.args
    $hasOverseerArgs = $args -and ($args.PSObject.Properties.Name -contains 'overseer') -and $args.overseer
    $staleTarget = (($cmd.result + '') -match 'overseer interject target was no longer running')
    if ($hasOverseerArgs -or $staleTarget) { $count++ }
  }
  $count
}

function Stop-OverseerInterventionRun {
  Stop-UfoWindows
  Get-CimInstance Win32_Process -ErrorAction SilentlyContinue |
    Where-Object {
      (($_.CommandLine + '') -match 'ProgramData\\UFOAgent\\ufo') -and
      (($_.Name + '') -match '^(python|pythonw|cmd|powershell|pwsh)\.exe$')
    } |
    ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
}

function Save-UfoTrajectorySnapshot([string]$phase, [string]$cmdId, $command = $null) {
  if (-not $cmdId) { return }
  $src = 'C:\ProgramData\UFOAgent\ufo\logs\adhoc'
  $responseLog = Join-Path $src 'response.log'
  if (-not (Test-Path $responseLog)) {
    Write-ProgressEvent 'phase_update' $phase "no UFO response.log to harvest for $cmdId"
    return
  }
  $safe = (($phase + '-' + $cmdId) -replace '[^A-Za-z0-9_.-]', '_')
  $root = Join-Path $OUT 'ufo-trajectories'
  $dst = Join-Path $root $safe
  Remove-Item $dst -Recurse -Force -ErrorAction SilentlyContinue
  New-Item -ItemType Directory -Force $dst | Out-Null
  Copy-Item (Join-Path $src '*') $dst -Recurse -Force -ErrorAction SilentlyContinue

  $cmdStatus = $null
  $cmdResult = $null
  if ($command) {
    $cmdStatus = $command.status
    $cmdResult = $command.result
  }
  $meta = [ordered]@{
    phase = $phase
    cmd_id = $cmdId
    captured_at = (Now)
    status = $cmdStatus
    result = $cmdResult
  }
  ([pscustomobject]$meta | ConvertTo-Json -Depth 5) | Set-Content (Join-Path $dst 'meta.json') -Encoding Ascii

  if ($haveAdm) {
    try {
      $trajUrl = "https://app.ufoagent.xyz/api/agents/$env:CI_AGENT_ID/trajectories/$cmdId"
      Invoke-WebRequest -UseBasicParsing -Headers (Get-ApiHeaders) -Uri $trajUrl -OutFile (Join-Path $dst 'control-plane-trajectory.json')
    } catch {
      Write-Host "::warning::could not save control-plane trajectory for ${phase}/${cmdId}: $($_.Exception.Message)"
    }
  }
  Write-ProgressEvent 'phase_update' $phase "harvested UFO trajectory for $cmdId"
}

function Quote-PsString([string]$value) {
  "'" + $value.Replace("'", "''") + "'"
}

function Install-UfoAgentLauncher {
  $dir = Join-Path $env:ProgramData 'UFOAgent\launchers'
  New-Item -ItemType Directory -Force $dir | Out-Null
  $ps1 = Join-Path $env:ProgramData 'UFOAgent\ufoagent-launch.ps1'
  $psBody = @(
    'param(',
    '  [Parameter(Mandatory=$true, Position=0)][string]$AppId,',
    '  [Parameter(ValueFromRemainingArguments=$true)][string[]]$AppArgs',
    ')',
    'if ($AppId -notmatch ''^[A-Za-z0-9_.-]+$'') { Write-Error "ufoagent-launch: invalid app id: $AppId"; exit 2 }',
    '$script = Join-Path $env:ProgramData ("UFOAgent\launchers\" + $AppId + ".ps1")',
    'if (-not (Test-Path $script)) { Write-Error "ufoagent-launch: unknown app: $AppId"; exit 2 }',
    '$eventPath = Join-Path $env:ProgramData ''UFOAgent\launcher-events.ndjson''',
    '$event = [ordered]@{ ts = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds(); app = $AppId; args = $AppArgs }',
    '([pscustomobject]$event | ConvertTo-Json -Compress) | Add-Content -Path $eventPath -Encoding Ascii',
    '& $script @AppArgs',
    'if ($LASTEXITCODE -ne $null) { exit $LASTEXITCODE }',
    'exit 0'
  )
  Set-Content -Path $ps1 -Value $psBody -Encoding Ascii

  $cmd = Join-Path $env:WINDIR 'ufoagent-launch.cmd'
  $body = @(
    '@echo off',
    'powershell -NoProfile -ExecutionPolicy Bypass -File "%ProgramData%\UFOAgent\ufoagent-launch.ps1" %*',
    'exit /b %ERRORLEVEL%'
  )
  Set-Content -Path $cmd -Value $body -Encoding Ascii
}

function Register-UfoAgentLaunchApp([string]$id, [string]$exe) {
  Install-UfoAgentLauncher
  $dir = Join-Path $env:ProgramData 'UFOAgent\launchers'
  New-Item -ItemType Directory -Force $dir | Out-Null
  $script = Join-Path $dir "$id.ps1"
  $exeQ = Quote-PsString $exe
  $dirQ = Quote-PsString (Split-Path $exe)
  $body = @(
    'param([Parameter(ValueFromRemainingArguments=$true)][string[]]$AppArgs)',
    ('$exe = ' + $exeQ),
    ('$dir = ' + $dirQ),
    'if ($AppArgs -and $AppArgs.Count -gt 0) {',
    '  Start-Process -FilePath $exe -WorkingDirectory $dir -ArgumentList $AppArgs',
    '} else {',
    '  Start-Process -FilePath $exe -WorkingDirectory $dir',
    '}',
    'exit 0'
  )
  Set-Content -Path $script -Value $body -Encoding Ascii
}

function Register-UfoAgentLaunchScript([string]$id, [string[]]$body) {
  Install-UfoAgentLauncher
  $dir = Join-Path $env:ProgramData 'UFOAgent\launchers'
  New-Item -ItemType Directory -Force $dir | Out-Null
  Set-Content -Path (Join-Path $dir "$id.ps1") -Value $body -Encoding Ascii
}

function Clear-LauncherEvents {
  Remove-Item (Join-Path $env:ProgramData 'UFOAgent\launcher-events.ndjson') -Force -ErrorAction SilentlyContinue
}

function Get-LauncherEvents {
  $path = Join-Path $env:ProgramData 'UFOAgent\launcher-events.ndjson'
  if (-not (Test-Path $path)) { return @() }
  $events = @()
  foreach ($line in (Get-Content $path -ErrorAction SilentlyContinue)) {
    if (-not $line) { continue }
    try { $events += ($line | ConvertFrom-Json) } catch {}
  }
  $events
}

function Test-LauncherEvent([string]$id) {
  foreach ($event in (Get-LauncherEvents)) {
    if (($event.app + '') -eq $id) { return $true }
  }
  $false
}

function Assert-LauncherEvent([string]$id, [string]$context) {
  if (Test-LauncherEvent $id) {
    Write-ProgressEvent 'phase_update' $context "launcher used: $id"
    return
  }
  $seen = ((Get-LauncherEvents | ForEach-Object { $_.app }) -join ', ')
  if (-not $seen) { $seen = 'none' }
  throw "$context did not invoke generic launcher id '$id' (seen: $seen)"
}

function Get-EdgeExe {
  @(
    "$env:ProgramFiles\Microsoft\Edge\Application\msedge.exe",
    "${env:ProgramFiles(x86)}\Microsoft\Edge\Application\msedge.exe"
  ) | Where-Object { Test-Path $_ } | Select-Object -First 1
}

function Get-NotepadExe {
  @(
    "$env:WINDIR\System32\notepad.exe",
    "$env:WINDIR\SysWOW64\notepad.exe"
  ) | Where-Object { Test-Path $_ } | Select-Object -First 1
}

function Get-DownloadsDir {
  Join-Path $env:USERPROFILE 'Downloads'
}

function Get-BraveExe {
  $candidates = @(
    "$env:ProgramFiles\BraveSoftware\Brave-Browser\Application\brave.exe",
    "${env:ProgramFiles(x86)}\BraveSoftware\Brave-Browser\Application\brave.exe",
    "$env:LOCALAPPDATA\BraveSoftware\Brave-Browser\Application\brave.exe"
  )
  $found = $candidates | Where-Object { Test-Path $_ } | Select-Object -First 1
  if ($found) { return $found }
  foreach ($entry in (Get-UninstallEntries 'Brave')) {
    foreach ($base in @($entry.InstallLocation, $entry.DisplayIcon)) {
      if (-not $base) { continue }
      $clean = (($base + '') -replace ',.*$', '').Trim('"')
      $path = if ($clean -match '\.exe$') { $clean } else { Join-Path $clean 'brave.exe' }
      if (Test-Path $path) { return $path }
    }
  }
}

function Get-UninstallEntries([string]$displayName) {
  $roots = @(
    'HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*',
    'HKLM:\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\*',
    'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*'
  )
  foreach ($root in $roots) {
    Get-ItemProperty -Path $root -ErrorAction SilentlyContinue |
      Where-Object { ($_.DisplayName + '') -like "*$displayName*" }
  }
}

function Get-BambuStudioExe {
  $candidates = @(
    "$env:ProgramFiles\Bambu Studio\bambu-studio.exe",
    "$env:ProgramFiles\Bambu Studio\BambuStudio.exe",
    "$env:ProgramFiles\Bambu Studio\Bambu Studio.exe",
    "${env:ProgramFiles(x86)}\Bambu Studio\bambu-studio.exe",
    "${env:ProgramFiles(x86)}\Bambu Studio\BambuStudio.exe",
    "${env:ProgramFiles(x86)}\Bambu Studio\Bambu Studio.exe",
    "$env:LOCALAPPDATA\Programs\Bambu Studio\bambu-studio.exe",
    "$env:LOCALAPPDATA\Programs\Bambu Studio\BambuStudio.exe",
    "$env:LOCALAPPDATA\Programs\Bambu Studio\Bambu Studio.exe"
  )
  $found = $candidates | Where-Object { Test-Path $_ } | Select-Object -First 1
  if ($found) { return $found }
  foreach ($entry in (Get-UninstallEntries 'Bambu Studio')) {
    $iconDir = $null
    if ($entry.DisplayIcon) {
      $iconPath = (($entry.DisplayIcon + '') -replace ',.*$', '').Trim('"')
      if ($iconPath) { $iconDir = Split-Path $iconPath -ErrorAction SilentlyContinue }
    }
    foreach ($base in @($entry.InstallLocation, $iconDir)) {
      if (-not $base -or -not (Test-Path $base)) { continue }
      $exe = Get-ChildItem $base -Recurse -File -Include '*.exe' -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -match '(?i)bambu.*studio|bambu-studio|bambustudio' -and $_.Name -notmatch '(?i)unins|uninstall|crash' } |
        Select-Object -First 1 -ExpandProperty FullName
      if ($exe) { return $exe }
    }
  }
}

function Test-BambuStudioInstalled {
  if (Get-BambuStudioExe) { return $true }
  if (Get-UninstallEntries 'Bambu Studio') { return $true }
  $shortcutRoots = @(
    "$env:ProgramData\Microsoft\Windows\Start Menu\Programs",
    "$env:APPDATA\Microsoft\Windows\Start Menu\Programs",
    "$env:PUBLIC\Desktop"
  )
  foreach ($root in $shortcutRoots) {
    if ((Test-Path $root) -and (Get-ChildItem $root -Recurse -Filter '*Bambu*Studio*.lnk' -ErrorAction SilentlyContinue | Select-Object -First 1)) {
      return $true
    }
  }
  $false
}

function Get-BraveProcess {
  Get-Process 'brave' -ErrorAction SilentlyContinue
}

function Register-BraveInstallerLauncher {
  $downloadsQ = Quote-PsString (Get-DownloadsDir)
  $body = @(
    '$ErrorActionPreference = ''Stop''',
    ('$downloads = ' + $downloadsQ),
    'if (-not (Test-Path $downloads)) { Write-Error "Brave installer launcher: downloads folder missing: $downloads"; exit 3 }',
    '$installer = Get-ChildItem -Path $downloads -File -Filter ''BraveBrowserStandalone*Setup*.exe'' -ErrorAction SilentlyContinue | Sort-Object LastWriteTime -Descending | Select-Object -First 1',
    'if (-not $installer) { Write-Error "Brave installer launcher: Brave setup exe was not found in $downloads"; exit 3 }',
    'Write-Host "Brave installer launcher: running $($installer.FullName)"',
    '$p = Start-Process -FilePath $installer.FullName -WorkingDirectory $installer.DirectoryName -Wait -PassThru',
    '$candidates = @("$env:ProgramFiles\BraveSoftware\Brave-Browser\Application\brave.exe", "${env:ProgramFiles(x86)}\BraveSoftware\Brave-Browser\Application\brave.exe", "$env:LOCALAPPDATA\BraveSoftware\Brave-Browser\Application\brave.exe")',
    '$brave = $null',
    'for ($i = 0; $i -lt 60 -and -not $brave; $i++) { $brave = $candidates | Where-Object { Test-Path $_ } | Select-Object -First 1; if (-not $brave) { Start-Sleep -Seconds 2 } }',
    'if ($p.ExitCode -ne 0 -and -not $brave) { Write-Error "Brave installer exited $($p.ExitCode)"; exit $p.ExitCode }',
    'if (-not $brave) { Write-Error "Brave installer completed but brave.exe was not found"; exit 4 }',
    'Start-Process -FilePath $brave -ArgumentList @(''--no-first-run'', ''--no-default-browser-check'', ''about:blank'')',
    'exit 0'
  )
  Register-UfoAgentLaunchScript 'brave-setup' $body
}

function Stop-Brave {
  Get-Process 'brave', 'BraveBrowserStandaloneSetup', 'BraveBrowserStandaloneSilentSetup' -ErrorAction SilentlyContinue |
    Stop-Process -Force -ErrorAction SilentlyContinue
}

function Resolve-BambuStudioInstallerUrl {
  [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
  Write-ProgressEvent 'phase_update' 'thirdparty' 'resolving latest Bambu Studio release'
  $rel = Invoke-RestMethod -UseBasicParsing -Headers @{ 'User-Agent' = 'ufoagent-e2e' } -Uri 'https://api.github.com/repos/bambulab/BambuStudio/releases/latest'
  $asset = $rel.assets | Where-Object { $_.name -match '^Bambu_Studio_win-.*\.exe$' } | Select-Object -First 1
  if (-not $asset) { throw 'latest Bambu Studio release has no Windows exe asset' }
  Write-Host "Bambu Studio installer asset: $($asset.name) $($asset.size) bytes"
  Write-ProgressEvent 'phase_update' 'thirdparty' "latest Bambu installer: $($asset.name) ($([math]::Round($asset.size / 1MB)) MB)"
  $asset.browser_download_url
}

function Get-BambuInstallerDownload {
  $downloads = Get-DownloadsDir
  if (-not (Test-Path $downloads)) { return $null }
  Get-ChildItem -Path $downloads -File -Filter 'Bambu_Studio_win-*.exe' -ErrorAction SilentlyContinue |
    Where-Object { $_.Length -gt 100MB } |
    Sort-Object LastWriteTime -Descending |
    Select-Object -First 1
}

function Test-StableDownload($file) {
  if (-not $file -or -not (Test-Path $file.FullName)) { return $false }
  $len = $file.Length
  $mtime = $file.LastWriteTimeUtc
  Start-Sleep -Seconds 2
  $again = Get-Item $file.FullName -ErrorAction SilentlyContinue
  [bool]($again -and $again.Length -eq $len -and $again.LastWriteTimeUtc -eq $mtime)
}

function Register-BambuInstallerLauncher {
  $downloadsQ = Quote-PsString (Get-DownloadsDir)
  $body = @(
    '$ErrorActionPreference = ''Stop''',
    ('$downloads = ' + $downloadsQ),
    'if (-not (Test-Path $downloads)) { Write-Error "Bambu installer launcher: downloads folder missing: $downloads"; exit 3 }',
    '$installer = Get-ChildItem -Path $downloads -File -Filter ''Bambu_Studio_win-*.exe'' -ErrorAction SilentlyContinue | Sort-Object LastWriteTime -Descending | Select-Object -First 1',
    'if (-not $installer) { Write-Error "Bambu installer launcher: Bambu Studio Windows installer was not found in $downloads"; exit 3 }',
    'Write-Host "Bambu installer launcher: running $($installer.FullName)"',
    '$p = Start-Process -FilePath $installer.FullName -WorkingDirectory $installer.DirectoryName -ArgumentList ''/S'' -PassThru',
    'if (-not $p.WaitForExit(900000)) { try { $p.Kill() } catch {}; Write-Error "Bambu installer timed out"; exit 5 }',
    'if ($p.ExitCode -ne 0) { Write-Error "Bambu installer exited $($p.ExitCode)"; exit $p.ExitCode }',
    '$candidates = @("$env:ProgramFiles\Bambu Studio\bambu-studio.exe", "$env:ProgramFiles\Bambu Studio\BambuStudio.exe", "$env:ProgramFiles\Bambu Studio\Bambu Studio.exe", "${env:ProgramFiles(x86)}\Bambu Studio\bambu-studio.exe", "${env:ProgramFiles(x86)}\Bambu Studio\BambuStudio.exe", "${env:ProgramFiles(x86)}\Bambu Studio\Bambu Studio.exe", "$env:LOCALAPPDATA\Programs\Bambu Studio\bambu-studio.exe", "$env:LOCALAPPDATA\Programs\Bambu Studio\BambuStudio.exe", "$env:LOCALAPPDATA\Programs\Bambu Studio\Bambu Studio.exe")',
    '$bambu = $null',
    'for ($i = 0; $i -lt 90 -and -not $bambu; $i++) { $bambu = $candidates | Where-Object { Test-Path $_ } | Select-Object -First 1; if (-not $bambu) { Start-Sleep -Seconds 2 } }',
    'if ($bambu) { Start-Process -FilePath $bambu; exit 0 }',
    '$roots = @(''HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*'', ''HKLM:\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\*'', ''HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*'')',
    '$entry = Get-ItemProperty -Path $roots -ErrorAction SilentlyContinue | Where-Object { ($_.DisplayName + "") -like "*Bambu Studio*" } | Select-Object -First 1',
    'if ($entry) { exit 0 }',
    'Write-Error "Bambu installer completed but Bambu Studio was not found"; exit 4'
  )
  Register-UfoAgentLaunchScript 'bambu-setup' $body
}

function Stop-BambuStudio {
  Get-Process -ErrorAction SilentlyContinue |
    Where-Object { $_.ProcessName -match '(?i)bambu|Bambu_Studio' } |
    Stop-Process -Force -ErrorAction SilentlyContinue
}

function Set-CropAnyProcess([string[]]$names) {
  foreach ($name in $names) {
    $p = Get-Process $name -ErrorAction SilentlyContinue | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1
    if ($p) {
      [Win32]::ShowWindow($p.MainWindowHandle, 9) | Out-Null
      [Win32]::SetForegroundWindow($p.MainWindowHandle) | Out-Null
      Start-Sleep -Milliseconds 500
      $script:curCrop = Get-FgRect
      if ($script:curCrop) { return }
    }
  }
  $script:curCrop = Get-FgRect
}

function Test-UfoTranscriptTyped([string]$id, [string]$want) {
  if (-not $id) { return $false }
  $path = "C:\ProgramData\UFOAgent\tasks\logs\$id.txt"
  if (-not (Test-Path $path)) { return $false }
  $raw = Get-Content $path -Raw -ErrorAction SilentlyContinue
  [bool]($raw -and $raw.Contains($want) -and $raw.Contains('set_edit_text') -and $raw.Contains('SUCCESS'))
}

function Open-TrayActivitySummary {
  $hidden = Find-UiaByName 'Show Hidden Icons' 'Button'
  if (-not $hidden) { $hidden = Find-UiaByName 'hidden icons' 'Button' }
  if ($hidden) { [void](Invoke-UiaElement $hidden); Start-Sleep -Milliseconds 1500 }
  $icon = Find-UiaByName 'UFOAgent' 'Button'
  if (-not $icon) { throw 'UFOAgent tray icon button not found' }
  if (-not (RightClick-UiaElement $icon)) { throw 'could not right-click UFOAgent tray icon' }
  Start-Sleep -Milliseconds 1500
  $item = Find-UiaByName 'been doing' 'MenuItem'
  if (-not $item) { throw "tray menu item containing 'been doing' not found" }
  if (-not (Invoke-UiaElement $item)) { throw 'could not invoke activity summary menu item' }
  $dlg = $null
  for ($i = 0; $i -lt 90 -and -not $dlg; $i++) {
    Start-Sleep -Seconds 1
    $dlg = Find-UiaByName 'been doing' 'Window' -ChildrenOnly
  }
  if (-not $dlg) { throw 'activity summary dialog never appeared' }
  try { [Win32]::SetForegroundWindow([IntPtr]$dlg.Current.NativeWindowHandle) | Out-Null } catch {}
  Start-Sleep -Milliseconds 800
  $script:curCrop = Get-UiaRect $dlg
  if (-not $script:curCrop) { $script:curCrop = Get-FgRect }
  $dlg
}

function Close-ActivityDialog($dlg) {
  if (-not $dlg) { return }
  try {
    foreach ($el in (Get-UiaElements $dlg)) {
      if ($el.Current.ControlType.ProgrammaticName -eq 'ControlType.Button' -and (($el.Current.Name + '') -like '*OK*')) {
        [void](Invoke-UiaElement $el); return
      }
    }
  } catch {}
}

function Add-TelemetryLine([string]$path, [string]$line) {
  $bytes = [System.Text.Encoding]::ASCII.GetBytes($line + [Environment]::NewLine)
  for ($i = 0; $i -lt 10; $i++) {
    $fs = $null
    try {
      $fs = [System.IO.File]::Open(
        $path,
        [System.IO.FileMode]::Append,
        [System.IO.FileAccess]::Write,
        [System.IO.FileShare]::ReadWrite
      )
      $fs.Write($bytes, 0, $bytes.Length)
      return
    } catch [System.IO.IOException] {
      Start-Sleep -Milliseconds 50
    } catch {
      Write-Host "progress telemetry write skipped: $($_.Exception.Message)"
      return
    } finally {
      if ($fs) { $fs.Dispose() }
    }
  }
  Write-Host "progress telemetry write skipped after retries"
}

function Write-ProgressEvent([string]$kind, [string]$phase = '', [string]$detail = '') {
  $obj = [ordered]@{ ts = (Now); event = $kind }
  if ($phase) { $obj.phase = $phase }
  if ($detail) { $obj.detail = $detail }
  Add-TelemetryLine (Join-Path $OUT 'progress.ndjson') ([pscustomobject]$obj | ConvertTo-Json -Compress)
}

function Write-Result($status) {
  [pscustomobject]@{ status = $status; ended = (Now); phases = $phases } |
    ConvertTo-Json -Depth 6 | Set-Content (Join-Path $OUT 'result.json') -Encoding Ascii
}
function Phase([string]$name, [scriptblock]$body) {
  Write-Host "=== phase: $name ==="
  Write-ProgressEvent 'phase_start' $name
  $script:curCrop = $null
  $s = Now; $ok = $true; $err = ''; $skip = $false
  try { $skip = (& $body) -eq 'SKIP' } catch { $ok = $false; $err = "$($_.Exception.Message)" }
  $e = Now
  $dur = '{0:n1}s' -f (($e - $s) / 1000.0)
  $obj = [ordered]@{ label = $name; start = $s; end = $e; ok = $ok; skipped = $skip; error = $err }
  if ($script:curCrop) { $obj.crop = $script:curCrop }
  [void]$phases.Add([pscustomobject]$obj)
  ($phases | ConvertTo-Json -Depth 4) | Set-Content (Join-Path $OUT 'phases.json') -Encoding Ascii
  if ($ok -and $skip) {
    Write-ProgressEvent 'phase_skipped' $name $dur
  } elseif ($ok) {
    Write-ProgressEvent 'phase_done' $name $dur
  } else {
    Write-ProgressEvent 'phase_failed' $name $err
  }
  if (-not $ok) { Write-Host "PHASE FAILED: $name : $err"; Write-Result 'FAIL'; throw "phase '$name' failed: $err" }
}
[pscustomobject]@{ status = 'RUNNING'; started = (Now) } | ConvertTo-Json | Set-Content (Join-Path $OUT 'result.json') -Encoding Ascii
Write-ProgressEvent 'journey_start'

# 1) INSTALL - the REAL installer in this session; /SILENT shows the visible "UFOAgent setup" console
#    streaming the uv provisioning. Detached (bootstrap --pause hangs); poll the marker to ready.
Phase 'install' {
  [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
  $setup = Join-Path $ROOT 'dl\ufoagent-setup.exe'
  $installer = if ($env:UFOAGENT_INSTALLER_PATH -and -not $env:UFOAGENT_BETA_URL) { $env:UFOAGENT_INSTALLER_PATH } else { $null }
  if ($installer) {
    if (-not (Test-Path $installer)) { throw "staged installer not found: $installer" }
    Write-ProgressEvent 'phase_update' 'install' 'using staged installer artifact'
    if ($installer -ne $setup) { Copy-Item $installer $setup -Force }
  } else {
    Write-ProgressEvent 'phase_update' 'install' 'downloading installer'
    Invoke-WebRequest -UseBasicParsing $BetaUrl -OutFile $setup
  }
  Write-Host "installer: $((Get-Item $setup).Length) bytes"
  Write-ProgressEvent 'phase_update' 'install' "installer ready: $((Get-Item $setup).Length) bytes"
  Start-Process $setup -ArgumentList '/SILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/NOCANCEL'
  Write-ProgressEvent 'phase_update' 'install' 'installer launched; waiting for UFO2 ready'
  $ready = Wait-For -TimeoutSec 600 -PollSec 5 -StreamAgentLog -Condition {
    if (Test-Path $marker) {
      $m = Get-Content $marker -Raw | ConvertFrom-Json
      $detail = if ($m.detail) { "$($m.state): $($m.detail)" } else { "$($m.state)" }
      if ($detail -and $detail -ne $script:lastInstallDetail) {
        Write-ProgressEvent 'phase_update' 'install' $detail
        $script:lastInstallDetail = $detail
      }
      if ($m.state -in @('ready', 'broken')) {
        Dismiss-UfoSetupConsole
        return $true
      }
    }
    $false
  }
  if (-not $ready) { throw 'provisioning did not reach a terminal state in 10m' }
  $state = (Get-Content $marker -Raw | ConvertFrom-Json).state
  if ($state -ne 'ready') { throw "provisioning state=$state (expected ready)" }
  Write-ProgressEvent 'phase_update' 'install' 'validating managed UFO config'
  Assert-ManagedUfoConfig
  if (-not $script:curCrop) { Set-Crop }
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

# 4) REMOTE TASK - command API -> WS -> login-session agent -> UFO2.
Phase 'remote' {
  if (-not $haveAdm) { Write-Host 'no CI admin token: skipping remote task'; return 'SKIP' }
  Stop-UfoWindows
  $notepad = Get-NotepadExe
  if (-not $notepad) { throw 'Notepad executable not found' }
  Register-UfoAgentLaunchApp 'notepad' $notepad
  Clear-LauncherEvents
  Minimize-OwnConsole
  $remotePrompt = @"
Open Notepad through the shell launcher, then type text.

Use the CommandLineExecutor run_shell tool directly. Do not use AppUIExecutor to type the command into Command Prompt, PowerShell, Run, or any terminal window.

The run_shell bash_command must be exactly:
ufoagent-launch.cmd notepad

After Notepad is visible, type exactly:
hello from ufoagent
"@
  $resp = Send-NodeCommand 'run_task' $remotePrompt
  Write-Host "sent run_task: id=$($resp.id) status=$($resp.status)"
  Write-ProgressEvent 'phase_update' 'remote' "command queued: $($resp.id)"
  $script:remoteId = $resp.id
  $opened = Wait-For -TimeoutSec 300 -StreamAgentLog -Condition {
    $c = Get-NodeCommand $script:remoteId
    if ($c -and $c.status -and $c.status -ne $script:lastRemoteStatus) {
      Write-ProgressEvent 'phase_update' 'remote' "command status: $($c.status)"
      $script:lastRemoteStatus = $c.status
    }
    if ($c -and $c.status -eq 'failed') { throw "run_task command failed: $($c.result)" }
    $notepadOpen = [bool](Get-Process notepad -ErrorAction SilentlyContinue)
    if ($notepadOpen -and (Test-LauncherEvent 'notepad')) { return $true }
    if ($notepadOpen -and $c -and $c.status -eq 'done') {
      throw "remote run_task opened Notepad without invoking generic launcher id 'notepad'"
    }
    if ($c -and $c.status -eq 'done') {
      $summary = Get-CommandResultSummary $c 420
      if ($summary) {
        Write-ProgressEvent 'phase_update' 'remote' "UFO result: $summary"
        throw "remote run_task finished without opening Notepad; UFO result: $summary"
      }
      throw 'remote run_task finished without opening Notepad'
    }
    $false
  }
  if (-not $opened) {
    $c = if ($script:remoteId) { Get-NodeCommand $script:remoteId } else { $null }
    if ($c) { Write-Host "run_task command result: status=$($c.status) result=$($c.result)" }
    Show-FileTail 'agent log' $AgentLog 40
    throw 'remote run_task did not open Notepad'
  }
  Assert-LauncherEvent 'notepad' 'remote'
  $typed = Wait-NotepadTyped -StreamAgentLog
  if ($typed.Typed) {
    Assert-TypedVerdict $typed 'remote run_task'
  } elseif (Test-UfoTranscriptTyped $script:remoteId 'hello from ufoagent') {
    Write-Host 'remote run_task: UFO transcript shows set_edit_text typed hello from ufoagent'
  } else {
    Assert-TypedVerdict $typed 'remote run_task'
  }
  Set-Crop 'notepad'; Start-Sleep 1
}
if ($haveAdm -and $script:remoteId) {
  $remoteDone = Wait-CommandTerminal $script:remoteId 'remote run_task' 300
  Save-UfoTrajectorySnapshot 'remote' $script:remoteId $remoteDone
}

# 5) THIRD-PARTY APP CHAIN - UFO uses the desktop to install Brave, then uses Brave to install Bambu
# Studio. The harness does not pre-download either installer; it only asserts the resulting apps exist.
$script:thirdPartyId = $null
Phase 'thirdparty' {
  if (-not $haveAdm) { Write-Host 'no CI admin token: skipping third-party app task'; return 'SKIP' }
  Stop-UfoWindows
  Stop-Brave
  Stop-BambuStudio
  Clear-LauncherEvents
  $edge = Get-EdgeExe
  if (-not $edge) { throw 'Microsoft Edge not found; cannot have UFO download Brave' }
  Register-UfoAgentLaunchApp 'edge' $edge
  Register-BraveInstallerLauncher
  Minimize-OwnConsole

  $brave = Get-BraveExe
  $braveWasInstalled = [bool]$brave
  if (-not $braveWasInstalled) {
    Write-ProgressEvent 'phase_update' 'thirdparty' 'sending Brave install run_task'
    $bravePrompt = @"
Install Brave Browser.

When a step says run_shell, call the CommandLineExecutor run_shell tool directly. Do not type these commands into Command Prompt, PowerShell, Run, or any terminal window.

Step 1: use the run_shell tool with exactly this command:
ufoagent-launch.cmd edge --no-first-run --no-default-browser-check $($script:BraveDownloadUrl)

Step 2: in Microsoft Edge, wait for the Brave installer download to finish. If Edge asks, keep or allow the download.

Step 3: use the run_shell tool with exactly this command:
ufoagent-launch.cmd brave-setup

Wait until Brave Browser is installed and a Brave window is visible. Do not stop after only downloading the installer.
"@
    $resp = Send-NodeCommand 'run_task' $bravePrompt
    Write-Host "sent Brave install run_task: id=$($resp.id) status=$($resp.status)"
    Write-ProgressEvent 'phase_update' 'thirdparty' "Brave command queued: $($resp.id)"
    $script:thirdPartyId = $resp.id
    $braveReady = Wait-For -TimeoutSec 900 -PollSec 5 -StreamAgentLog -Condition {
      $c = Get-NodeCommand $script:thirdPartyId
      if ($c -and $c.status -and $c.status -ne $script:lastBraveStatus) {
        Write-ProgressEvent 'phase_update' 'thirdparty' "Brave command status: $($c.status)"
        $script:lastBraveStatus = $c.status
      }
      if ($c -and $c.status -eq 'failed') { throw "Brave install run_task failed: $($c.result)" }
      [bool](Get-BraveExe)
    }
    if (-not $braveReady) {
      $c = if ($script:thirdPartyId) { Get-NodeCommand $script:thirdPartyId } else { $null }
      if ($c) { Write-Host "Brave install run_task result: status=$($c.status) result=$($c.result)" }
      Show-FileTail 'agent log' $AgentLog 80
      throw 'UFO did not install Brave'
    }
    $braveDone = Wait-CommandTerminalAfterState $script:thirdPartyId 'Brave install run_task' 'thirdparty' 120
    Write-CommandResultProgress 'thirdparty' $braveDone
    Save-UfoTrajectorySnapshot 'brave' $script:thirdPartyId $braveDone
    Assert-LauncherEvent 'edge' 'thirdparty'
    Assert-LauncherEvent 'brave-setup' 'thirdparty'
    $brave = Get-BraveExe
  } else {
    Write-Host "Brave already installed: $brave"
  }
  if (-not $brave) { throw 'Brave install completed but brave.exe was not found' }
  Register-UfoAgentLaunchApp 'brave' $brave
  Write-ProgressEvent 'phase_update' 'thirdparty' "Brave ready: $brave"

  $bambuWasInstalled = Test-BambuStudioInstalled
  if (-not $bambuWasInstalled) {
    $bambuUrl = Resolve-BambuStudioInstallerUrl
    Register-BambuInstallerLauncher
    Write-ProgressEvent 'phase_update' 'thirdparty' 'sending Bambu Studio download run_task'
    $bambuDownloadPrompt = @"
Download the Bambu Studio Windows installer using Brave.

When a step says run_shell, call the CommandLineExecutor run_shell tool directly. Do not type these commands into Command Prompt, PowerShell, Run, or any terminal window.

Step 1: use the run_shell tool with exactly this command:
ufoagent-launch.cmd brave --no-first-run --no-default-browser-check $bambuUrl

Step 2: in Brave, wait for the Bambu Studio Windows installer download to finish. If Brave asks, keep or allow the download. Download the .exe installer, not the zip. Stop once the download is complete.
"@
    $resp = Send-NodeCommand 'run_task' $bambuDownloadPrompt
    Write-Host "sent Bambu Studio download run_task: id=$($resp.id) status=$($resp.status)"
    Write-ProgressEvent 'phase_update' 'thirdparty' "Bambu download command queued: $($resp.id)"
    $bambuDownloadId = $resp.id
    $script:thirdPartyId = $bambuDownloadId
    $bambuDownloaded = Wait-For -TimeoutSec 900 -PollSec 10 -StreamAgentLog -Condition {
      $c = Get-NodeCommand $bambuDownloadId
      if ($c -and $c.status -and $c.status -ne $script:lastBambuStatus) {
        Write-ProgressEvent 'phase_update' 'thirdparty' "Bambu download command status: $($c.status)"
        $script:lastBambuStatus = $c.status
      }
      $installer = Get-BambuInstallerDownload
      if ($installer -and (Test-StableDownload $installer)) { return $true }
      if ($c -and $c.status -eq 'failed') { throw "Bambu Studio download run_task failed: $($c.result)" }
      $false
    }
    if (-not $bambuDownloaded) {
      $c = if ($bambuDownloadId) { Get-NodeCommand $bambuDownloadId } else { $null }
      if ($c) { Write-Host "Bambu Studio download run_task result: status=$($c.status) result=$($c.result)" }
      Show-FileTail 'agent log' $AgentLog 100
      throw 'UFO did not download the Bambu Studio installer'
    }
    $bambuDownloadDone = Wait-CommandTerminalAfterState $bambuDownloadId 'Bambu Studio download run_task' 'thirdparty' 60
    Write-CommandResultProgress 'thirdparty' $bambuDownloadDone
    Save-UfoTrajectorySnapshot 'bambu-download' $bambuDownloadId $bambuDownloadDone
    Assert-LauncherEvent 'brave' 'thirdparty'

    $installer = Get-BambuInstallerDownload
    if (-not $installer) { throw 'Bambu Studio installer download disappeared before setup' }
    Write-ProgressEvent 'phase_update' 'thirdparty' "Bambu installer downloaded: $($installer.Name)"
    Write-ProgressEvent 'phase_update' 'thirdparty' 'sending Bambu Studio setup run_task'
    $bambuSetupPrompt = @"
Install the already-downloaded Bambu Studio Windows installer.

When a step says run_shell, call the CommandLineExecutor run_shell tool directly. Do not type this command into Command Prompt, PowerShell, Run, or any terminal window.

Use the run_shell tool with exactly this command:
ufoagent-launch.cmd bambu-setup

Wait until Bambu Studio is installed. If Bambu Studio opens with an OpenGL or graphics error, leave that dialog visible and consider the install complete.
"@
    $resp = Send-NodeCommand 'run_task' $bambuSetupPrompt
    Write-Host "sent Bambu Studio setup run_task: id=$($resp.id) status=$($resp.status)"
    Write-ProgressEvent 'phase_update' 'thirdparty' "Bambu setup command queued: $($resp.id)"
    $bambuSetupId = $resp.id
    $script:thirdPartyId = $bambuSetupId
    $script:lastBambuStatus = ''
    $bambuReady = Wait-For -TimeoutSec 900 -PollSec 10 -StreamAgentLog -Condition {
      $c = Get-NodeCommand $bambuSetupId
      if ($c -and $c.status -and $c.status -ne $script:lastBambuStatus) {
        Write-ProgressEvent 'phase_update' 'thirdparty' "Bambu setup command status: $($c.status)"
        $script:lastBambuStatus = $c.status
      }
      $installed = Test-BambuStudioInstalled
      if ($c -and $c.status -eq 'failed') { throw "Bambu Studio setup run_task failed: $($c.result)" }
      $installed
    }
    if (-not $bambuReady) {
      $c = if ($bambuSetupId) { Get-NodeCommand $bambuSetupId } else { $null }
      if ($c) { Write-Host "Bambu Studio setup run_task result: status=$($c.status) result=$($c.result)" }
      Show-FileTail 'agent log' $AgentLog 100
      throw 'UFO did not install Bambu Studio'
    }
    $bambuSetupDone = Wait-CommandTerminalAfterState $bambuSetupId 'Bambu Studio setup run_task' 'thirdparty' 120
    Write-CommandResultProgress 'thirdparty' $bambuSetupDone
    Save-UfoTrajectorySnapshot 'bambu-setup' $bambuSetupId $bambuSetupDone
    Assert-LauncherEvent 'bambu-setup' 'thirdparty'
  } else {
    Write-Host 'Bambu Studio already installed'
  }
  $bambuExe = Get-BambuStudioExe
  if ($bambuExe) { Write-Host "Bambu Studio installed: $bambuExe" } else { Write-Host 'Bambu Studio installed: uninstall entry or shortcut detected' }
  Set-CropAnyProcess @('Bambu Studio', 'BambuStudio', 'bambu-studio', 'brave')
  Start-Sleep 4
}

# 6) OVERSEER INTERVENTION - deliberately impossible task that must trigger live halt/interject.
$script:overseerInterveneId = $null
$script:lastOverseerInterveneStatus = ''
$script:lastOverseerStormCount = -1
Phase 'overseer-intervene' {
  if (-not $haveAdm) { Write-Host 'no CI admin token: skipping overseer intervention phase'; return 'SKIP' }
  Stop-OverseerInterventionRun
  $before = Get-CommandIdSet
  $stormLimit = 2
  $taskUrl = 'http://127.0.0.1:9/ufoagent-impossible-e2e-application.exe'
  $prompt = @"
Install and launch the Windows application named UFOAgent Impossible E2E App from this URL:
$taskUrl

This URL is intentionally unreachable. Do not claim success unless the application is actually installed and launched. If the download fails, report that it failed; do not invent a workaround or mark the task complete.
"@
  try {
    $resp = Send-NodeCommand 'run_task' $prompt
    Write-Host "sent overseer intervention run_task: id=$($resp.id) status=$($resp.status)"
    Write-ProgressEvent 'phase_update' 'overseer-intervene' "command queued: $($resp.id)"
    $script:overseerInterveneId = $resp.id

    $intervened = Wait-For -TimeoutSec 240 -PollSec 5 -StreamAgentLog -Condition {
      $c = Get-NodeCommand $script:overseerInterveneId
      if ($c -and $c.status -and $c.status -ne $script:lastOverseerInterveneStatus) {
        Write-ProgressEvent 'phase_update' 'overseer-intervene' "command status: $($c.status)"
        $script:lastOverseerInterveneStatus = $c.status
      }
      $count = Count-InterjectRedispatches $before
      if ($count -ne $script:lastOverseerStormCount) {
        Write-ProgressEvent 'phase_update' 'overseer-intervene' "overseer re-dispatch count: $count"
        $script:lastOverseerStormCount = $count
      }
      if ($count -gt $stormLimit) {
        throw "overseer interject storm detected ($count re-dispatches) - loop protection missing"
      }
      if ($count -gt 0) { return $true }
      if ($c -and $c.status -eq 'halted') { return $true }
      if ($c -and ($c.status -eq 'done' -or $c.status -eq 'failed')) {
        throw "overseer did not intervene before failing task finished: status=$($c.status) result=$($c.result)"
      }
      $false
    }
    if (-not $intervened) { throw 'overseer did not intervene within 240s on the deliberately failing task' }

    Write-ProgressEvent 'phase_update' 'overseer-intervene' 'intervention observed; watching for storm'
    [void](Wait-For -TimeoutSec 90 -PollSec 5 -StreamAgentLog -Condition {
      $count = Count-InterjectRedispatches $before
      if ($count -ne $script:lastOverseerStormCount) {
        Write-ProgressEvent 'phase_update' 'overseer-intervene' "overseer re-dispatch count: $count"
        $script:lastOverseerStormCount = $count
      }
      if ($count -gt $stormLimit) {
        throw "overseer interject storm detected ($count re-dispatches) - loop protection missing"
      }
      $false
    })
    Write-ProgressEvent 'phase_update' 'overseer-intervene' "storm check passed with $script:lastOverseerStormCount re-dispatches"
  } finally {
    Stop-OverseerInterventionRun
  }
}

# 7) DASHBOARD - capture this node's REAL desktop via the live `screenshot` command, then open the
#    dashboard (a CI-token preview of the REAL CI-tenant data, since the headless VM browser can't do
#    GitHub OAuth) on this node's detail and record it. On trusted CI this phase is required: do not
#    publish a misleading desktop gif if the browser/dashboard did not open.
Phase 'dashboard' {
  if (-not $haveAdm) { Write-Host 'no CI admin token: skipping dashboard phase'; return 'SKIP' }
  $edge = @("$env:ProgramFiles\Microsoft\Edge\Application\msedge.exe", "${env:ProgramFiles(x86)}\Microsoft\Edge\Application\msedge.exe") |
    Where-Object { Test-Path $_ } | Select-Object -First 1
  if (-not $edge) { throw 'Microsoft Edge not found; cannot capture mission control' }

  # Capture this node's desktop through the REAL screenshot command. Brave/Bambu remains open so
  # the live desktop image has concrete third-party app work visible.
  $shot = Send-NodeCommand 'screenshot'
  Write-Host "sent screenshot: id=$($shot.id) status=$($shot.status)"
  $done = Wait-For -TimeoutSec 90 -StreamAgentLog -Condition { $c = Get-NodeCommand $shot.id; $c -and ($c.status -eq 'done' -or $c.status -eq 'failed') }
  $c = if ($shot.id) { Get-NodeCommand $shot.id } else { $null }
  if (-not $done -or -not $c -or $c.status -ne 'done') {
    throw "screenshot command failed: status=$($c.status) result=$($c.result)"
  }
  Write-Host "screenshot captured: $($c.result)"
  Stop-UfoWindows   # clear the remote-task Notepad before showing the browser

  # Save the raw captured desktop still (published next to the gifs; powers dashboard previews).
  $shotUrl = "https://app.ufoagent.xyz/api/agents/$env:CI_AGENT_ID/screenshot/latest"
  try { Invoke-WebRequest -UseBasicParsing -Headers (Get-ApiHeaders) -Uri $shotUrl -OutFile (Join-Path $OUT 'node-desktop.png'); Write-Host 'saved node-desktop.png' }
  catch { Write-Host "could not save node-desktop.png: $($_.Exception.Message)" }

  # Open mission control (real data via the CI-token preview) on this node's detail and record it.
  #     App mode = no toolbar/address bar, so the token in the URL never appears in the gif. A FRESH
  #     dedicated profile (+ --no-first-run) is what makes the app window actually appear on a clean box;
  #     --inprivate (the previous attempt) silently produced no window.
  $cp = 'https://app.ufoagent.xyz/preview/ci?token=' + [Uri]::EscapeDataString($env:CI_ADMIN_TOKEN) + '&node=' + [Uri]::EscapeDataString($env:CI_AGENT_ID)
  Get-Process msedge -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
  $eprof = Join-Path $ROOT 'edge-app'; Remove-Item $eprof -Recurse -Force -ErrorAction SilentlyContinue
  # --force-device-scale-factor=1 pins 1:1 rendering: the VM's display scaling otherwise reflowed the
  # layout (collapsed the screenshot column, clipped the status values) in the first capture.
  Start-Process $edge -ArgumentList '--no-first-run', '--no-default-browser-check', '--disable-sync',
    '--disable-features=msEdgeWelcomeExperience', '--force-device-scale-factor=1', "--user-data-dir=$eprof",
    '--start-maximized', '--window-size=1280,800', ('--app=' + $cp)
  $win = Wait-For -TimeoutSec 30 -PollSec 2 -Condition {
    [bool](Get-Process msedge -ErrorAction SilentlyContinue | Where-Object { $_.MainWindowHandle -ne 0 })
  }
  if (-not $win) {
    Get-Process msedge -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
    throw 'Edge never opened a mission control window'
  }
  Start-Sleep 14   # let fonts + the inlined screenshot fully load and the layout settle before cropping
  Set-Crop 'msedge'
  # Only a real Edge window should anchor the crop; the launcher console is ~880px wide, the maximized
  # Edge app window is near full screen. If the crop looks like the console, drop it (full-frame gif).
  if (-not $script:curCrop -or $script:curCrop.w -lt 1000) {
    $w = if ($script:curCrop) { $script:curCrop.w } else { 0 }
    $script:curCrop = $null
    Get-Process msedge -EA SilentlyContinue | Stop-Process -Force -EA SilentlyContinue
    throw "mission control was not foreground or was too small (w=$w)"
  }
  Start-Sleep 2

  # End the same moving proof clip with the tray/taskbar recap of what this node has been doing.
  # Use full-frame cropping so the dashboard, taskbar interaction, menu, and dialog remain contiguous.
  if (-not $haveTok) { Write-Host 'not linked: skipping activity summary inside dashboard clip'; return }
  $script:curCrop = $null
  $dlg = Open-TrayActivitySummary
  $script:curCrop = $null
  Write-Host 'activity summary dialog opened from tray'
  Start-Sleep 8
}
Get-Process msedge -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Stop-UfoWindows
Stop-Brave
Stop-BambuStudio

Write-Result 'PASS'
Write-ProgressEvent 'journey_done'
Write-Host 'JOURNEY DONE: PASS'
