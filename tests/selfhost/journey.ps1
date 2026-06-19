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
$script:lastBambuStatus = ''
$script:bambuProcessName = 'bambu-studio'
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
  foreach ($allowed in @('"bambu-studio.cmd"', '"bambustudio.exe"')) {
    if ($cliRaw -notmatch [regex]::Escape($allowed)) {
      throw "UFO CLI launcher allow-list missing: $allowed"
    }
  }
  Write-Host 'managed UFO config: USE_MCP=False, local GUI MCP servers, MCP loaders patched, Bambu launcher allowed'
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

function Write-CommandResultProgress([string]$phase, $command) {
  if (-not $command -or -not $command.result) { return }
  $summary = (($command.result -replace '\s+', ' ').Trim())
  if ($summary.Length -gt 220) { $summary = $summary.Substring(0, 220) + '...' }
  Write-ProgressEvent 'phase_update' $phase "UFO result: $summary"
}

function Get-BambuExe {
  $candidates = @(
    "$env:ProgramFiles\Bambu Studio\bambu-studio.exe",
    "$env:ProgramFiles\Bambu Studio\BambuStudio.exe",
    "${env:ProgramFiles(x86)}\Bambu Studio\bambu-studio.exe",
    "${env:ProgramFiles(x86)}\Bambu Studio\BambuStudio.exe"
  )
  $found = $candidates | Where-Object { Test-Path $_ } | Select-Object -First 1
  if ($found) { return $found }

  $portable = Join-Path $ROOT 'apps\BambuStudio'
  if (Test-Path $portable) {
    Get-ChildItem $portable -Recurse -File -Include 'bambu-studio.exe', 'BambuStudio.exe' -ErrorAction SilentlyContinue |
      Select-Object -First 1 -ExpandProperty FullName
  }
}

function Install-BambuShortcut([string]$exe) {
  $targets = @(
    "$env:PUBLIC\Desktop\Bambu Studio.lnk",
    "$env:ProgramData\Microsoft\Windows\Start Menu\Programs\Bambu Studio.lnk"
  )
  $shell = New-Object -ComObject WScript.Shell
  foreach ($target in $targets) {
    New-Item -ItemType Directory -Force (Split-Path $target) | Out-Null
    $lnk = $shell.CreateShortcut($target)
    $lnk.TargetPath = $exe
    $lnk.WorkingDirectory = Split-Path $exe
    $lnk.Description = 'Bambu Studio'
    $lnk.Save()
  }
}

function Install-BambuLaunchShim([string]$exe) {
  $cmd = Join-Path $env:WINDIR 'bambu-studio.cmd'
  $body = @(
    '@echo off',
    ('start "" /D "' + (Split-Path $exe) + '" "' + $exe + '" %* ^<NUL ^>NUL 2^>NUL')
  )
  Set-Content -Path $cmd -Value $body -Encoding Ascii
}

function Install-BambuStudio {
  $existing = Get-BambuExe
  if ($existing) {
    Install-BambuShortcut $existing
    Install-BambuLaunchShim $existing
    Write-Host "Bambu Studio already installed: $existing"
    return $existing
  }
  [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
  Write-ProgressEvent 'phase_update' 'bambu' 'resolving latest Bambu Studio release'
  $rel = Invoke-RestMethod -UseBasicParsing -Headers @{ 'User-Agent' = 'ufoagent-e2e' } -Uri 'https://api.github.com/repos/bambulab/BambuStudio/releases/latest'
  $asset = $rel.assets | Where-Object { $_.name -match '^Bambu_Studio_win-.*\.zip$' } | Select-Object -First 1
  if (-not $asset) { throw 'latest Bambu Studio release has no Windows zip asset' }
  $archive = Join-Path $ROOT 'dl\bambu-studio.zip'
  $dest = Join-Path $ROOT 'apps\BambuStudio'
  New-Item -ItemType Directory -Force -Path (Split-Path $archive), $dest | Out-Null
  Write-Host "Bambu Studio archive: $($asset.name) $($asset.size) bytes"
  Write-ProgressEvent 'phase_update' 'bambu' "downloading $($asset.name) ($([math]::Round($asset.size / 1MB)) MB)"
  Invoke-WebRequest -UseBasicParsing -Headers @{ 'User-Agent' = 'ufoagent-e2e' } -Uri $asset.browser_download_url -OutFile $archive
  Write-ProgressEvent 'phase_update' 'bambu' 'extracting Bambu Studio'
  if (Test-Path $dest) { Remove-Item $dest -Recurse -Force }
  New-Item -ItemType Directory -Force $dest | Out-Null
  Expand-Archive -Path $archive -DestinationPath $dest -Force
  $exe = Get-BambuExe
  if (-not $exe) { throw 'Bambu Studio archive extracted but app executable was not found' }
  Install-BambuShortcut $exe
  Install-BambuLaunchShim $exe
  Write-ProgressEvent 'phase_update' 'bambu' 'Bambu Studio ready'
  $exe
}

function Get-BambuProcess {
  Get-Process 'bambu-studio', 'BambuStudio' -ErrorAction SilentlyContinue
}

function Stop-BambuStudio {
  Get-BambuProcess | Stop-Process -Force -ErrorAction SilentlyContinue
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

function Write-ProgressEvent([string]$kind, [string]$phase = '', [string]$detail = '') {
  $obj = [ordered]@{ ts = (Now); event = $kind }
  if ($phase) { $obj.phase = $phase }
  if ($detail) { $obj.detail = $detail }
  ([pscustomobject]$obj | ConvertTo-Json -Compress) | Add-Content (Join-Path $OUT 'progress.ndjson') -Encoding Ascii
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
  $resp = Send-NodeCommand 'run_task' 'Open Notepad and type the message: hello from ufoagent'
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
    [bool](Get-Process notepad -ErrorAction SilentlyContinue)
  }
  if (-not $opened) {
    $c = if ($script:remoteId) { Get-NodeCommand $script:remoteId } else { $null }
    if ($c) { Write-Host "run_task command result: status=$($c.status) result=$($c.result)" }
    Show-FileTail 'agent log' $AgentLog 40
    throw 'remote run_task did not open Notepad'
  }
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
  $null = Wait-CommandTerminal $script:remoteId 'remote run_task' 300
}

# 5) BAMBU STUDIO - real third-party Windows app path. This catches AppAgent/config regressions that
# a Notepad smoke test will not, and produces the website's concrete app demo.
$script:bambuId = $null
Phase 'bambu' {
  if (-not $haveAdm) { Write-Host 'no CI admin token: skipping Bambu Studio task'; return 'SKIP' }
  Stop-UfoWindows
  Stop-BambuStudio
  $bambu = Install-BambuStudio
  $script:bambuProcessName = [IO.Path]::GetFileNameWithoutExtension($bambu)
  Write-Host "Bambu Studio installed: $bambu"
  Minimize-OwnConsole
  Write-ProgressEvent 'phase_update' 'bambu' 'sending Bambu Studio run_task'
  $resp = Send-NodeCommand 'run_task' 'Use the run_shell tool with exactly this command: bambu-studio.cmd. Then wait until the Bambu Studio main window is visible.'
  Write-Host "sent Bambu run_task: id=$($resp.id) status=$($resp.status)"
  Write-ProgressEvent 'phase_update' 'bambu' "command queued: $($resp.id)"
  $script:bambuId = $resp.id
  $opened = Wait-For -TimeoutSec 420 -PollSec 5 -StreamAgentLog -Condition {
    $c = Get-NodeCommand $script:bambuId
    if ($c -and $c.status -and $c.status -ne $script:lastBambuStatus) {
      Write-ProgressEvent 'phase_update' 'bambu' "command status: $($c.status)"
      $script:lastBambuStatus = $c.status
    }
    if ($c -and $c.status -eq 'failed') { throw "Bambu run_task command failed: $($c.result)" }
    [bool](Get-BambuProcess | Where-Object { $_.MainWindowHandle -ne 0 })
  }
  if (-not $opened) {
    $c = if ($script:bambuId) { Get-NodeCommand $script:bambuId } else { $null }
    if ($c) { Write-Host "Bambu run_task command result: status=$($c.status) result=$($c.result)" }
    Show-FileTail 'agent log' $AgentLog 60
    throw 'Bambu Studio did not open'
  }
  Set-Crop $script:bambuProcessName
  Start-Sleep 4
  $bambuDone = Wait-CommandTerminal $script:bambuId 'Bambu Studio run_task' 300
  Write-CommandResultProgress 'bambu' $bambuDone
}

# 6) DASHBOARD - capture this node's REAL desktop via the live `screenshot` command, then open the
#    dashboard (a CI-token preview of the REAL CI-tenant data, since the headless VM browser can't do
#    GitHub OAuth) on this node's detail and record it. On trusted CI this phase is required: do not
#    publish a misleading desktop gif if the browser/dashboard did not open.
Phase 'dashboard' {
  if (-not $haveAdm) { Write-Host 'no CI admin token: skipping dashboard phase'; return 'SKIP' }
  $edge = @("$env:ProgramFiles\Microsoft\Edge\Application\msedge.exe", "${env:ProgramFiles(x86)}\Microsoft\Edge\Application\msedge.exe") |
    Where-Object { Test-Path $_ } | Select-Object -First 1
  if (-not $edge) { throw 'Microsoft Edge not found; cannot capture mission control' }

  # Capture this node's desktop through the REAL screenshot command. Bambu Studio remains open so
  # the live desktop image has concrete app work visible.
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
}
Get-Process msedge -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Stop-UfoWindows
Stop-BambuStudio

# 7) ACTIVITY SUMMARY - the tray/taskbar recap of what this node has been doing. This is the final
#    story beat: after real remote work, open the tray menu and show the on-device LLM summary.
Phase 'activity' {
  if (-not $haveTok) { Write-Host 'not linked: skipping activity summary'; return 'SKIP' }
  $dlg = Open-TrayActivitySummary
  Write-Host 'activity summary dialog opened from tray'
  Start-Sleep 8
  Close-ActivityDialog $dlg
}

Write-Result 'PASS'
Write-ProgressEvent 'journey_done'
Write-Host 'JOURNEY DONE: PASS'
