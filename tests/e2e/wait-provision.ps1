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
