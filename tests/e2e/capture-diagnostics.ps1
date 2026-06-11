# Always-run diagnostics: dump + collect the Inno log, agent log, config, and service state.
. "$PSScriptRoot/helpers.ps1"
$diag = Join-Path $env:RUNNER_TEMP 'diag'; New-Item -ItemType Directory -Force -Path $diag | Out-Null
$pd = "$env:ProgramData\UFOAgent"
Write-Host '===== Inno install log ====='
if ($env:INNO_LOG -and (Test-Path $env:INNO_LOG)) { Get-Content $env:INNO_LOG | Write-Host; Copy-Item $env:INNO_LOG $diag -ErrorAction SilentlyContinue }
Write-Host '===== agent log (tail) ====='
if (Test-Path $AgentLog) { Get-Content $AgentLog -Tail 200 | Write-Host; Copy-Item $AgentLog $diag -ErrorAction SilentlyContinue }
Write-Host '===== config.json ====='
$cfg = Join-Path $pd 'config.json'
if (Test-Path $cfg) { Get-Content $cfg | Write-Host; Copy-Item $cfg $diag -ErrorAction SilentlyContinue }
Write-Host '===== service ====='
sc.exe query UFOAgent | Out-String | Write-Host
