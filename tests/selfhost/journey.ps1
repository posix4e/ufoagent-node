# The ONE end-to-end journey, run inside the VM's interactive desktop session. Walks the whole user story
# linearly, asserting each step, and exports UFO's own per-task trajectories for website assets. Emits:
#   out\phases.json  - [{label,start,end,ok,error}] guest epoch-ms phase boundaries for diagnosis
#   out\result.json  - {status: RUNNING|PASS|FAIL, phases}
#   out\ufo          - harvested UFO logs, traces, transcripts, and manifests per run_task phase
# Reuses the proven assertion logic from tests\e2e (staged alongside as .\e2e). Kept ASCII so Windows
# PowerShell 5.1 parses it cleanly.
$ErrorActionPreference = 'Stop'
$ROOT = $PSScriptRoot
$E2E  = Join-Path $ROOT 'e2e'
$OUT  = Join-Path $ROOT 'out'
$HARVEST = Join-Path $ROOT 'harvest_ufo.py'
$env:RUNNER_TEMP = $ROOT                      # helpers.ps1 derives $Shots from this
New-Item -ItemType Directory -Force $OUT, (Join-Path $ROOT 'shots'), (Join-Path $ROOT 'dl') | Out-Null
if (Test-Path (Join-Path $ROOT 'env.ps1')) { . (Join-Path $ROOT 'env.ps1') }   # CI tokens, if staged
. (Join-Path $E2E 'helpers.ps1')
Add-Type -AssemblyName System.Windows.Forms, System.Drawing

# Win32 helpers for foreground-window checks. SW_RESTORE+SetForeground brings target windows to the front
# before the live screenshot command captures mission-control recaps.
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
$script:lastInstallDetail = ''
$script:lastRemoteStatus = ''
$script:lastThirdPartyStatus = ''
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

function Assert-NativeUfoConfig {
  $system = 'C:\ProgramData\UFOAgent\ufo\config\ufo\system.yaml'
  if (-not (Test-Path $system)) { throw "UFO system config missing: $system" }
  $raw = Get-Content $system -Raw
  if ($raw -notmatch '(?m)^\s*USE_MCP:\s*True\b') {
    throw 'UFO native config did not keep USE_MCP enabled'
  }
  if ($raw -match '(?m)^\s*USE_MCP:\s*False\b') {
    throw 'UFO native config was overridden with USE_MCP=False'
  }
  if ($raw -notmatch '(?m)^\s*MCP_FALLBACK_TO_UI:\s*True\b') {
    throw 'UFO native config did not keep MCP_FALLBACK_TO_UI enabled'
  }
  if ($raw -notmatch '(?m)^\s*SAFE_GUARD:\s*False\b') {
    throw 'UFO unattended mode did not disable SAFE_GUARD'
  }
  $mcp = 'C:\ProgramData\UFOAgent\ufo\config\ufo\mcp.yaml'
  if (-not (Test-Path $mcp)) { throw "UFO MCP config missing: $mcp" }
  $mcpRaw = Get-Content $mcp -Raw
  if ($mcpRaw -match 'Managed by ufoagent') {
    throw 'UFO MCP config is still using the old managed override'
  }
  foreach ($required in @('namespace: UICollector', 'namespace: HostUIExecutor', 'namespace: AppUIExecutor', 'namespace: CommandLineExecutor')) {
    if ($mcpRaw -notmatch [regex]::Escape($required)) {
      throw "UFO native MCP config missing server: $required"
    }
  }
  $shellExecutorCount = ([regex]::Matches($mcpRaw, [regex]::Escape('namespace: CommandLineExecutor'))).Count
  if ($shellExecutorCount -lt 2) {
    throw 'UFO native MCP config does not expose CommandLineExecutor to both HostAgent and AppAgent'
  }
  $cli = 'C:\ProgramData\UFOAgent\ufo\ufo\client\mcp\local_servers\cli_mcp_server.py'
  if (-not (Test-Path $cli)) { throw "UFO CLI MCP server missing: $cli" }
  $cliRaw = Get-Content $cli -Raw
  if ($cliRaw -notmatch 'Managed by ufoagent: allow all CLI MCP commands for unattended installs') {
    throw 'UFO CLI MCP allowlist patch is missing'
  }
  if ($cliRaw -notmatch 'Managed by ufoagent: run CLI MCP commands through cmd.exe for real shell semantics') {
    throw 'UFO CLI MCP real-shell patch is missing'
  }
  $ui = 'C:\ProgramData\UFOAgent\ufo\ufo\client\mcp\local_servers\ui_mcp_server.py'
  if (-not (Test-Path $ui)) { throw "UFO UI MCP server missing: $ui" }
  $uiRaw = Get-Content $ui -Raw
  if ($uiRaw -notmatch 'Managed by ufoagent: make GUI action primitives tolerant and on-screen') {
    throw 'UFO UI action primitive patch is missing'
  }
  if ($uiRaw -notmatch 'Managed by ufoagent: send keyboard input directly to focused windows when no control target exists') {
    throw 'UFO UI direct keyboard input patch is missing'
  }
  if ($uiRaw -notmatch 'Managed by ufoagent: treat keyboard_input text key tokens as keystrokes') {
    throw 'UFO UI key-token keyboard patch is missing'
  }
  if ($uiRaw -notmatch 'Managed by ufoagent: click relative coordinates with a direct mouse fallback') {
    throw 'UFO UI coordinate-click fallback patch is missing'
  }
  if ($uiRaw -notmatch 'Managed by ufoagent: keep AppAgent perception on the foreground top-level window') {
    throw 'UFO UI foreground-window awareness patch is missing'
  }
  if ($uiRaw -notmatch 'Managed by ufoagent: ignore console shells as foreground AppAgent targets') {
    throw 'UFO UI foreground console-filter patch is missing'
  }
  foreach ($required in @('text: Annotated[', '_ufoagent_restore_window_for_actions(ui_state.selected_app_window)', 'window.maximize()', 'App window screenshot too small, treating as invalid', '_ufoagent_keyboard_input_to_foreground', '_ufoagent_text_contains_key_tokens', 'pyperclip.copy(keys)', '_ufoagent_click_relative_coordinates', '_ufoagent_pyautogui.click', '_ufoagent_sync_selected_window_to_foreground(ui_state)', 'Desktop(backend=backend).active()', 'ConsoleWindowClass')) {
    if ($uiRaw -notmatch [regex]::Escape($required)) {
      throw "UFO UI action primitive patch missing expected behavior: $required"
    }
  }

  $legacyChecks = @(
    [pscustomobject]@{ Path = 'C:\ProgramData\UFOAgent\ufo\ufo\agents\agent\host_agent.py'; Marker = 'Managed by ufoagent: honor USE_MCP=False'; Label = 'MCP loader skip patch' }
    [pscustomobject]@{ Path = 'C:\ProgramData\UFOAgent\ufo\ufo\agents\agent\app_agent.py'; Marker = 'Managed by ufoagent: honor USE_MCP=False'; Label = 'MCP loader skip patch' }
    [pscustomobject]@{ Path = 'C:\ProgramData\UFOAgent\ufo\ufo\agents\agent\app_agent.py'; Marker = 'Managed by ufoagent: auto-approve confirmation in unattended mode'; Label = 'AppAgent confirmation source patch' }
    [pscustomobject]@{ Path = 'C:\ProgramData\UFOAgent\ufo\ufo\client\mcp\local_servers\ui_mcp_server.py'; Marker = 'Managed by ufoagent: allow focused-app keyboard input'; Label = 'UI keyboard fallback patch' }
    [pscustomobject]@{ Path = 'C:\ProgramData\UFOAgent\ufo\ufo\automator\app_apis\shell\shell_client.py'; Marker = 'Managed by ufoagent: allow unrestricted run_shell commands'; Label = 'shell command allowlist patch' }
    [pscustomobject]@{ Path = 'C:\ProgramData\UFOAgent\ufo\ufo\automator\app_apis\shell\shell_client.py'; Marker = 'Managed by ufoagent: allow unrestricted run_shell paths'; Label = 'shell path allowlist patch' }
    [pscustomobject]@{ Path = 'C:\ProgramData\UFOAgent\ufo\ufo\client\mcp\local_servers\cli_mcp_server.py'; Marker = 'Managed by ufoagent: allow unrestricted CLI launcher commands'; Label = 'CLI command allowlist patch' }
    [pscustomobject]@{ Path = 'C:\ProgramData\UFOAgent\ufo\ufo\client\mcp\local_servers\cli_mcp_server.py'; Marker = 'Managed by ufoagent: launch unrestricted commands without waiting for GUI apps'; Label = 'CLI launcher run_shell patch' }
    [pscustomobject]@{ Path = 'C:\ProgramData\UFOAgent\ufo\ufo\prompts\share\base\host_agent.yaml'; Marker = 'Managed by ufoagent: generic runtime remediation'; Label = 'HostAgent remediation prompt patch' }
    [pscustomobject]@{ Path = 'C:\ProgramData\UFOAgent\ufo\ufo\prompts\share\base\app_agent.yaml'; Marker = 'Managed by ufoagent: generic runtime remediation'; Label = 'AppAgent remediation prompt patch' }
  )
  foreach ($check in $legacyChecks) {
    if (-not (Test-Path $check.Path)) { throw "UFO file missing: $($check.Path)" }
    $fileRaw = Get-Content $check.Path -Raw
    if ($fileRaw -match [regex]::Escape($check.Marker)) {
      throw "legacy managed UFO patch remains ($($check.Label)): $($check.Path)"
    }
  }

  Write-Host 'native UFO config: USE_MCP=True, MCP fallback enabled, native MCP map intact, SAFE_GUARD=False, CLI commands allowed, GUI primitives tolerant/on-screen'
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

function Invoke-UfoRunTask([string]$label, [string]$request, [int]$TimeoutSec = 900, [int]$PollSec = 5) {
  $resp = Send-NodeCommand 'run_task' $request
  Write-Host "sent ${label} run_task: id=$($resp.id) status=$($resp.status)"
  Write-ProgressEvent 'phase_update' $label "command queued: $($resp.id)"
  $id = $resp.id
  $script:lastRunTaskStatus = $null
  $done = Wait-For -TimeoutSec $TimeoutSec -PollSec $PollSec -StreamAgentLog -Condition {
    $c = Get-NodeCommand $id
    if ($c -and $c.status -and $c.status -ne $script:lastRunTaskStatus) {
      Write-ProgressEvent 'phase_update' $label "command status: $($c.status)"
      $script:lastRunTaskStatus = $c.status
    }
    if ($c -and $c.status -eq 'failed') { throw "${label} run_task failed: $($c.result)" }
    $c -and $c.status -eq 'done'
  }
  $final = if ($id) { Get-NodeCommand $id } else { $null }
  if (-not $done -or -not $final) {
    if ($final) { Write-Host "${label} run_task result: status=$($final.status) result=$($final.result)" }
    Show-FileTail 'agent log' $AgentLog 120
    throw "${label} run_task did not finish cleanly"
  }
  Write-CommandResultProgress $label $final
  [pscustomobject]@{ Id = $id; Result = $final }
}

function Export-UfoTrajectory([string]$label, [string]$id, [string]$request = '') {
  if (-not $id) { throw "cannot harvest UFO trajectory for ${label}: missing command id" }
  if (-not $request) { throw "cannot harvest UFO trajectory for ${label}: missing request text" }
  if (-not (Test-Path $HARVEST)) { throw "UFO harvest script missing: $HARVEST" }
  $py = 'C:\ProgramData\UFOAgent\ufo\.venv\Scripts\python.exe'
  $ufoHome = 'C:\ProgramData\UFOAgent\ufo'
  if (-not (Test-Path $py)) { throw "UFO Python missing: $py" }
  if (-not (Test-Path $ufoHome)) { throw "UFO home missing: $ufoHome" }
  $harvestRoot = Join-Path $OUT 'ufo'
  New-Item -ItemType Directory -Force $harvestRoot | Out-Null
  $requestFile = Join-Path $OUT "ufo-request-$label.txt"
  [IO.File]::WriteAllText($requestFile, $request, (New-Object System.Text.UTF8Encoding $false))
  Write-ProgressEvent 'phase_update' $label "harvesting UFO trajectory for command $id"
  & $py $HARVEST export --label $label --task-id $id --ufo-home $ufoHome --log-name 'adhoc' --request-file $requestFile --out $harvestRoot
  if ($LASTEXITCODE -ne 0) { throw "UFO trajectory harvest failed for ${label}: exit $LASTEXITCODE" }
  Write-ProgressEvent 'phase_update' $label "harvested UFO trajectory for command $id"
}

function Invoke-NodeScreenshotToFile([string]$label, [string]$outFile) {
  $shot = Send-NodeCommand 'screenshot'
  Write-Host "sent ${label} screenshot: id=$($shot.id) status=$($shot.status)"
  Write-ProgressEvent 'phase_update' 'dashboard' "${label} screenshot command queued: $($shot.id)"
  $done = Wait-For -TimeoutSec 90 -StreamAgentLog -Condition {
    $c = Get-NodeCommand $shot.id
    $c -and ($c.status -eq 'done' -or $c.status -eq 'failed')
  }
  $c = if ($shot.id) { Get-NodeCommand $shot.id } else { $null }
  if (-not $done -or -not $c -or $c.status -ne 'done') {
    throw "${label} screenshot command failed: status=$($c.status) result=$($c.result)"
  }
  $shotUrl = "https://app.ufoagent.xyz/api/agents/$env:CI_AGENT_ID/screenshot/latest"
  Invoke-WebRequest -UseBasicParsing -Headers (Get-ApiHeaders) -Uri $shotUrl -OutFile $outFile
  if (-not (Test-Path $outFile)) { throw "${label} screenshot was not saved: $outFile" }
  Write-ProgressEvent 'phase_update' 'dashboard' "saved ${label} screenshot"
  $c
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

function Get-BambuStudioInstaller([datetime]$Since) {
  $roots = @(
    "$env:USERPROFILE\Downloads",
    "$env:PUBLIC\Downloads",
    "$env:TEMP",
    (Join-Path $ROOT 'Downloads')
  ) | Where-Object { $_ -and (Test-Path $_) } | Select-Object -Unique
  $candidates = foreach ($root in $roots) {
    Get-ChildItem -Path $root -File -ErrorAction SilentlyContinue |
      Where-Object {
        $_.Length -gt 1000000 -and
        $_.LastWriteTime -ge $Since -and
        $_.Extension -match '^\.(exe|msi)$' -and
        $_.Name -match '(?i)bambu'
      }
  }
  $candidates | Sort-Object LastWriteTime -Descending | Select-Object -First 1
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

function Stop-Brave {
  Get-Process 'brave', 'BraveBrowserStandaloneSetup', 'BraveBrowserStandaloneSilentSetup' -ErrorAction SilentlyContinue |
    Stop-Process -Force -ErrorAction SilentlyContinue
}

function Stop-BambuStudio {
  Get-Process -ErrorAction SilentlyContinue |
    Where-Object { $_.ProcessName -match '(?i)bambu|Bambu_Studio' } |
    Stop-Process -Force -ErrorAction SilentlyContinue
}

function Get-BambuStudioProcess {
  Get-Process -ErrorAction SilentlyContinue |
    Where-Object { $_.MainWindowHandle -ne 0 -and $_.ProcessName -match '(?i)bambu' -and $_.ProcessName -notmatch '(?i)setup|unins|crash' } |
    Select-Object -First 1
}

function Get-UiaText($el, [int]$MaxItems = 160) {
  $names = New-Object System.Collections.Generic.List[string]
  if (-not $el) { return '' }
  try {
    if ($el.Current.Name) { $names.Add($el.Current.Name) }
    foreach ($child in (Get-UiaElements $el)) {
      if ($names.Count -ge $MaxItems) { break }
      try {
        $name = $child.Current.Name + ''
        if ($name.Trim()) { $names.Add($name.Trim()) }
      } catch {}
    }
  } catch {}
  ($names | Select-Object -Unique) -join ' '
}

function Find-BambuStudioGraphicsError {
  $root = [System.Windows.Automation.AutomationElement]::RootElement
  $wins = try { $root.FindAll([System.Windows.Automation.TreeScope]::Children, [System.Windows.Automation.Condition]::TrueCondition) } catch { @() }
  foreach ($win in $wins) {
    $text = Get-UiaText $win
    if (-not $text) { continue }
    if ($text -match '(?i)(bambu|studio|opengl|graphics|graphic|gpu|driver)' -and $text -match '(?i)(opengl|graphics|graphic|gpu|driver)' -and $text -match '(?i)(error|failed|unable|unsupported|not supported|problem)') {
      return $text
    }
  }
  $null
}

function Assert-BambuStudioLaunchesClean([int]$TimeoutSec = 240) {
  $exe = Get-BambuStudioExe
  if (-not $exe) { throw 'Bambu Studio executable was not found after install' }
  Stop-BambuStudio
  Write-ProgressEvent 'phase_update' 'thirdparty' 'verifying Bambu Studio clean launch'
  Start-Process -FilePath $exe -WorkingDirectory (Split-Path $exe)
  $deadline = (Get-Date).AddSeconds($TimeoutSec)
  while ((Get-Date) -lt $deadline) {
    $err = Find-BambuStudioGraphicsError
    if ($err) {
      throw "Bambu Studio showed an OpenGL/graphics error dialog: $err"
    }
    $p = Get-BambuStudioProcess
    if ($p) {
      [Win32]::ShowWindow($p.MainWindowHandle, 9) | Out-Null
      [Win32]::SetForegroundWindow($p.MainWindowHandle) | Out-Null
      Start-Sleep -Milliseconds 800
      $err = Find-BambuStudioGraphicsError
      if ($err) {
        throw "Bambu Studio showed an OpenGL/graphics error dialog: $err"
      }
      Write-ProgressEvent 'phase_update' 'thirdparty' 'Bambu Studio launched without graphics error dialog'
      return $true
    }
    Start-Sleep -Seconds 5
  }
  throw 'Bambu Studio did not open a usable window during clean-launch verification'
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

function Add-SharedTextLine([string]$Path, [string]$Line) {
  $bytes = [System.Text.Encoding]::ASCII.GetBytes($Line + [Environment]::NewLine)
  $last = $null
  for ($i = 0; $i -lt 50; $i++) {
    try {
      $fs = [System.IO.File]::Open($Path, [System.IO.FileMode]::Append, [System.IO.FileAccess]::Write, [System.IO.FileShare]::ReadWrite)
      try {
        $fs.Write($bytes, 0, $bytes.Length)
        return $true
      } finally {
        $fs.Dispose()
      }
    } catch [System.IO.IOException] {
      $last = $_.Exception.Message
      Start-Sleep -Milliseconds 100
    }
  }
  Write-Host "progress write skipped after retries: $last"
  $false
}

function Write-ProgressEvent([string]$kind, [string]$phase = '', [string]$detail = '') {
  $obj = [ordered]@{ ts = (Now); event = $kind }
  if ($phase) { $obj.phase = $phase }
  if ($detail) { $obj.detail = $detail }
  $line = ([pscustomobject]$obj | ConvertTo-Json -Compress)
  [void](Add-SharedTextLine (Join-Path $OUT 'progress.ndjson') $line)
}

function Write-Result($status) {
  [pscustomobject]@{ status = $status; ended = (Now); phases = $phases } |
    ConvertTo-Json -Depth 6 | Set-Content (Join-Path $OUT 'result.json') -Encoding Ascii
}
function Phase([string]$name, [scriptblock]$body) {
  Write-Host "=== phase: $name ==="
  Write-ProgressEvent 'phase_start' $name
  $s = Now; $ok = $true; $err = ''; $skip = $false
  try { $skip = (& $body) -eq 'SKIP' } catch { $ok = $false; $err = "$($_.Exception.Message)" }
  $e = Now
  $dur = '{0:n1}s' -f (($e - $s) / 1000.0)
  $obj = [ordered]@{ label = $name; start = $s; end = $e; ok = $ok; skipped = $skip; error = $err }
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
  $installMarkerFreshAfter = [DateTimeOffset]::UtcNow.ToUnixTimeSeconds() - 1
  Start-Process $setup -ArgumentList '/SILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/NOCANCEL'
  Write-ProgressEvent 'phase_update' 'install' 'installer launched; waiting for UFO2 ready'
  Write-ProgressEvent 'phase_update' 'install' 'waiting for fresh UFO2 provisioning marker'
  $ready = Wait-For -TimeoutSec 600 -PollSec 5 -StreamAgentLog -Condition {
    if (Test-Path $marker) {
      $m = Get-Content $marker -Raw | ConvertFrom-Json
      $fresh = $m.updated_at -and ([int64]$m.updated_at -ge $installMarkerFreshAfter)
      if (-not $fresh) { return $false }
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
  Write-ProgressEvent 'phase_update' 'install' 'validating native UFO config'
  Assert-NativeUfoConfig
  Write-Host 'install + provision: UFO2 ready'
}
if ($env:UFOAGENT_E2E_STOP_AFTER_INSTALL -eq '1') {
  Write-ProgressEvent 'journey_done' '' 'provisioned snapshot ready'
  Write-Result 'PASS'
  exit 0
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

# 3) LINK - show the REAL QR/pairing screen, then link functionally via the CI token.
$script:linkProc = $null
Phase 'link' {
  $script:linkProc = Start-Process $Exe -ArgumentList 'link', '--force' -PassThru -WindowStyle Normal
  Start-Sleep -Seconds 9
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
  Minimize-OwnConsole
  $script:remotePrompt = @"
Open Notepad and type exactly:
hello from ufoagent
"@
  $resp = Send-NodeCommand 'run_task' $script:remotePrompt
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
    if ($notepadOpen) { return $true }
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
  $typed = Wait-NotepadTyped -StreamAgentLog
  if ($typed.Typed) {
    Assert-TypedVerdict $typed 'remote run_task'
  } elseif (Test-UfoTranscriptTyped $script:remoteId 'hello from ufoagent') {
    Write-Host 'remote run_task: UFO transcript shows set_edit_text typed hello from ufoagent'
  } else {
    Assert-TypedVerdict $typed 'remote run_task'
  }
  Start-Sleep 1
  if ($script:remoteId) {
    $null = Wait-CommandTerminal $script:remoteId 'remote run_task' 300
    Export-UfoTrajectory 'remote' $script:remoteId $script:remotePrompt
  }
}

# 5) THIRD-PARTY APP CHAIN - UFO uses the desktop to install Brave, then uses Brave to download
# and install Bambu Studio. The harness does not pre-download either installer or supply commands;
# it only asserts outcomes after each objective-sized task.
$script:thirdPartyIds = @{}
Phase 'thirdparty' {
  if (-not $haveAdm) { Write-Host 'no CI admin token: skipping third-party app task'; return 'SKIP' }
  Stop-UfoWindows
  Stop-Brave
  Stop-BambuStudio
  Minimize-OwnConsole
  $thirdPartyStarted = (Get-Date).AddMinutes(-2)
  $thirdPartyTaskTimeoutSec = 900

  Write-ProgressEvent 'phase_update' 'thirdparty' 'sending Brave install run_task'
  $bravePrompt = @"
Install Brave Browser on this Windows desktop.

Finish only after Brave Browser is installed and can be launched normally.
"@
  $braveRun = Invoke-UfoRunTask 'thirdparty-brave-install' $bravePrompt $thirdPartyTaskTimeoutSec
  $script:thirdPartyIds['brave-install'] = $braveRun.Id
  Export-UfoTrajectory 'thirdparty-brave-install' $braveRun.Id $bravePrompt

  $brave = Get-BraveExe
  if (-not $brave) { throw 'Brave install run_task finished but Brave Browser was not installed' }
  Write-ProgressEvent 'phase_update' 'thirdparty' "Brave ready: $brave"

  Minimize-OwnConsole
  Write-ProgressEvent 'phase_update' 'thirdparty' 'sending Bambu installer download run_task'
  $bambuDownloadPrompt = @"
Use Brave Browser to download the latest Windows Bambu Studio installer for Windows.

Finish only after the installer file is downloaded and ready to run.
"@
  $downloadRun = Invoke-UfoRunTask 'thirdparty-bambu-download' $bambuDownloadPrompt $thirdPartyTaskTimeoutSec
  $script:thirdPartyIds['bambu-download'] = $downloadRun.Id
  Export-UfoTrajectory 'thirdparty-bambu-download' $downloadRun.Id $bambuDownloadPrompt
  $bambuInstaller = Get-BambuStudioInstaller $thirdPartyStarted
  if (-not $bambuInstaller) { throw 'Bambu download run_task finished but no fresh Bambu Studio installer was found' }
  Write-ProgressEvent 'phase_update' 'thirdparty' "Bambu installer ready: $($bambuInstaller.FullName)"

  Minimize-OwnConsole
  Write-ProgressEvent 'phase_update' 'thirdparty' 'sending Bambu install run_task'
  $bambuInstallPrompt = @"
Install Bambu Studio from the downloaded Windows installer.

Finish only after Bambu Studio is installed on this desktop.
"@
  $installRun = Invoke-UfoRunTask 'thirdparty-bambu-install' $bambuInstallPrompt $thirdPartyTaskTimeoutSec
  $script:thirdPartyIds['bambu-install'] = $installRun.Id
  Export-UfoTrajectory 'thirdparty-bambu-install' $installRun.Id $bambuInstallPrompt
  if (-not (Test-BambuStudioInstalled)) { throw 'Bambu install run_task finished but Bambu Studio was not installed' }
  $bambuExe = Get-BambuStudioExe
  if ($bambuExe) { Write-Host "Bambu Studio installed: $bambuExe" } else { Write-Host 'Bambu Studio installed: uninstall entry or shortcut detected' }

  Minimize-OwnConsole
  Write-ProgressEvent 'phase_update' 'thirdparty' 'sending Bambu clean-launch run_task'
  $bambuLaunchPrompt = @"
Launch Bambu Studio.

If a graphics or OpenGL error appears, diagnose and resolve the missing capability, then retry.

Finish only after Bambu Studio is running in a normal usable window with no error dialog visible.
"@
  $launchRun = Invoke-UfoRunTask 'thirdparty-bambu-launch' $bambuLaunchPrompt $thirdPartyTaskTimeoutSec
  $script:thirdPartyIds['bambu-launch'] = $launchRun.Id
  Export-UfoTrajectory 'thirdparty-bambu-launch' $launchRun.Id $bambuLaunchPrompt
  Assert-BambuStudioLaunchesClean
  Start-Sleep 4
}

# 6) DASHBOARD - capture this node's REAL desktop via the live `screenshot` command, then open the
#    dashboard (a CI-token preview of the REAL CI-tenant data, since the headless VM browser can't do
#    GitHub OAuth) on this node's detail and capture the mission-control recap. On trusted CI this phase
#    is required: do not publish a misleading desktop image if the browser/dashboard did not open.
Phase 'dashboard' {
  if (-not $haveAdm) { Write-Host 'no CI admin token: skipping dashboard phase'; return 'SKIP' }
  $edge = @("$env:ProgramFiles\Microsoft\Edge\Application\msedge.exe", "${env:ProgramFiles(x86)}\Microsoft\Edge\Application\msedge.exe") |
    Where-Object { Test-Path $_ } | Select-Object -First 1
  if (-not $edge) { throw 'Microsoft Edge not found; cannot capture mission control' }

  # Capture this node's desktop through the REAL screenshot command. Brave/Bambu remains open so
  # the live desktop image has concrete third-party app work visible.
  $null = Invoke-NodeScreenshotToFile 'node-desktop' (Join-Path $OUT 'node-desktop.png')
  Stop-UfoWindows   # clear Notepad before showing the browser

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
  Focus-Proc 'msedge'
  # Only a real Edge window should be foreground; the launcher console is ~880px wide, the maximized
  # Edge app window is near full screen.
  $missionRect = Get-FgRect
  if (-not $missionRect -or $missionRect.w -lt 1000) {
    $w = if ($missionRect) { $missionRect.w } else { 0 }
    Get-Process msedge -EA SilentlyContinue | Stop-Process -Force -EA SilentlyContinue
    throw "mission control was not foreground or was too small (w=$w)"
  }
  Start-Sleep 2

  # End the same moving proof clip with the tray/taskbar recap of what this node has been doing.
  if (-not $haveTok) { Write-Host 'not linked: skipping activity summary inside dashboard clip'; return }
  $dlg = Open-TrayActivitySummary
  Write-Host 'activity summary dialog opened from tray'
  Start-Sleep 8
  $null = Invoke-NodeScreenshotToFile 'mission-control' (Join-Path $OUT 'mission-control.png')
}
Get-Process msedge -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Stop-UfoWindows
Stop-Brave
Stop-BambuStudio

Write-Result 'PASS'
Write-ProgressEvent 'journey_done'
Write-Host 'JOURNEY DONE: PASS'
