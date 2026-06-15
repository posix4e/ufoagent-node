# Wait for the installer's background bootstrap to auto-provision UFO2 (zip path, no git),
# streaming the agent log live. Fails fast if bootstrap logs its failure verdict.
$ErrorActionPreference = 'Stop'
. "$PSScriptRoot/helpers.ps1"
$ok = Wait-For -TimeoutSec 720 -PollSec 10 -StreamAgentLog -Condition {
  $st = & $Exe status | Out-String
  if (($st -notmatch 'ufo_home:\s+\(unset\)') -and ($st -notmatch 'python:\s+\(unset\)')) { Write-Host $st; return $true }
  if ((Test-Path $AgentLog) -and (Select-String -Path $AgentLog -Pattern 'UFO2 setup failed' -Quiet)) {
    throw 'bootstrap reported failure (see [agent] log above)'
  }
  $false
}
if (-not $ok) {
  Write-Host '::error::installer did not auto-provision UFO2 within 12 minutes'
  if (Test-Path $AgentLog) { Get-Content $AgentLog | Write-Host }
  throw 'auto-provision timed out'
}
Write-Host 'installer auto-provisioned UFO2 (no git) OK'

# Environments-v1 (agent half): bootstrap records ufo2 installing -> ready in a marker file, and the
# running service re-probes each tick and reports the change. Assert both on this real install — the
# marker is ground truth; the log line proves the service's reporting path (this is what fills the
# dashboard chip and feeds the run_task gate). Both lag the config write by a tick, so poll briefly.
$EnvMarker = "$env:ProgramData\UFOAgent\envs\ufo2.json"
$ready = Wait-For -TimeoutSec 120 -PollSec 5 -Condition {
  (Test-Path $EnvMarker) -and (((Get-Content $EnvMarker -Raw | ConvertFrom-Json).state) -eq 'ready')
}
if (-not $ready) {
  if (Test-Path $EnvMarker) { Write-Host "env marker: $(Get-Content $EnvMarker -Raw)" }
  throw 'ufo2 env marker never reached state=ready after provisioning'
}
Write-Host "ufo2 env marker = ready: $(Get-Content $EnvMarker -Raw)"

$reported = Wait-For -TimeoutSec 120 -PollSec 5 -StreamAgentLog -Condition {
  (Test-Path $AgentLog) -and (Select-String -Path $AgentLog -Pattern 'environments changed -> ufo2=ready' -Quiet)
}
if (-not $reported) { throw 'service never logged the ufo2 installing->ready transition' }
Write-Host 'service observed + reported ufo2 -> ready'

# Cold-box proof: win32ui must import via the provisioned venv — i.e. the agent actually installed
# the VC++ runtime (mfc140u.dll) we removed before install. bootstrap's verify_ufo_imports already
# gates this (a failure would have surfaced as 'UFO2 setup failed' above), but assert it explicitly
# so the cold-box → provisioned → importable chain is unmistakable in the log. This is the check that
# would have caught the win32ui/MFC crash that "full e2e green" shipped.
$py = (Get-Content "$env:ProgramData\UFOAgent\config.json" -Raw | ConvertFrom-Json).python
if (-not $py) { throw 'config.python not set after provisioning' }
& $py -c "import win32ui, pywinauto; print('win32ui + pywinauto import OK via ' + __import__('sys').executable)"
if ($LASTEXITCODE -ne 0) { throw "win32ui failed to import via the provisioned venv ($py) — the agent did not provision the VC++ runtime" }
Write-Host 'cold-box OK: win32ui imports via the provisioned venv (agent installed the VC++ runtime)'
