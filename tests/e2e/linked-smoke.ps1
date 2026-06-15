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

# Environments reached the control plane: the repair round-trip above proves the DO has processed
# this node's `hello` (which carries the environment report). So a run_task for an environment the
# node doesn't have must be gated server-side (400) — if the report were missing, gateRunTask would
# allow it (null = old agent) and we'd get a 200. This proves report -> CP -> gate end to end.
Write-Host 'asserting run_task is gated to reported environments (ufo3 should be rejected)...'
$gated = $false
try {
  Send-NodeCommand 'run_task' -Request 'noop' -Env 'ufo3' | Out-Null
} catch {
  $code = $_.Exception.Response.StatusCode.value__
  $msg = $_.ErrorDetails.Message
  if ($code -eq 400 -and $msg -match 'ufo3') { $gated = $true; Write-Host "gate rejected ufo3 as expected: $msg" }
  else { throw "run_task(ufo3) failed unexpectedly: status=$code msg=$msg" }
}
if (-not $gated) { throw 'run_task for a missing env (ufo3) was accepted — the node did not report environments' }

Write-Host 'linked smoke OK: WS hello+environments -> DO -> command round-trip -> env gate'
