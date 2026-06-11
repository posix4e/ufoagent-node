# Linked smoke: hand the pre-provisioned CI node token to the installed agent, watch it connect
# over WebSocket, then drive a real repair command through the live control plane
# (enqueue -> DO push -> agent WS -> execute -> result).
$ErrorActionPreference = 'Stop'
. "$PSScriptRoot/helpers.ps1"
# The service's WS loop polls the store every 30s and connects as soon as the token appears.
& $Exe configure --agent-token $env:CI_AGENT_TOKEN
$connected = Wait-For -TimeoutSec 180 -StreamAgentLog -Condition {
  (Test-Path $AgentLog) -and (Select-String -Path $AgentLog -Pattern 'ws: connected to control plane' -Quiet)
}
if (-not $connected) { throw 'agent never connected to the control plane over WebSocket' }
Write-Host 'WS connected - enqueueing a repair command via the control plane API...'
$resp = Send-NodeCommand 'repair'
Write-Host "enqueued: id=$($resp.id) status=$($resp.status)"
$done = Wait-For -TimeoutSec 120 -StreamAgentLog -Condition {
  $cmd = Get-NodeCommand $resp.id
  if ($cmd -and $cmd.status -eq 'failed') { throw "command failed: $($cmd.result)" }
  if ($cmd -and $cmd.status -eq 'done') { Write-Host "command done: $($cmd.result)"; return $true }
  $false
}
if (-not $done) { throw 'repair command did not reach done within 2 minutes' }
Write-Host 'linked smoke OK: enqueue -> DO push -> agent WS -> execute -> result'
