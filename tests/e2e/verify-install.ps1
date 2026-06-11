# The installed exe answers the standard commands and the service is running.
$ErrorActionPreference = 'Stop'
. "$PSScriptRoot/helpers.ps1"
Start-Sleep -Seconds 5
$svc = sc.exe query UFOAgent | Out-String; Write-Host $svc
if ($svc -notmatch 'RUNNING|START_PENDING') { throw 'UFOAgent service is not running' }
Write-Host '--- version ---';     & $Exe version
Write-Host '--- configure ---';   & $Exe configure --control-plane https://app.ufoagent.xyz
Write-Host '--- status ---';      $st = & $Exe status | Out-String; Write-Host $st
if ($st -notmatch 'app\.ufoagent\.xyz') { throw 'configure did not persist' }
Write-Host '--- update ---';      & $Exe update
Write-Host '--- link --help ---'; & $Exe link --help | Out-Null
$r = Invoke-WebRequest 'https://app.ufoagent.xyz/v1/agent/version' -UseBasicParsing -TimeoutSec 20
if ($r.StatusCode -ne 200) { throw 'control plane unreachable from the node' }
Write-Host 'standard commands OK'
