# Silent install with the Inno log streamed live. Git is stripped from PATH first so the
# installer's background bootstrap must take the no-git zip path (the real customer scenario).
$ErrorActionPreference = 'Stop'
. "$PSScriptRoot/helpers.ps1"
$setup = (Get-ChildItem -Recurse dl -Filter ufoagent-setup.exe | Select-Object -First 1).FullName
if (-not $setup) { throw 'installer artifact missing' }
Add-MpPreference -ExclusionPath "$env:ProgramFiles\UFOAgent", (Join-Path (Get-Location) 'dl') -ErrorAction SilentlyContinue
$env:PATH = (($env:PATH -split ';') | Where-Object { $_ -and -not (Test-Path (Join-Path $_ 'git.exe')) }) -join ';'
if (Get-Command git -ErrorAction SilentlyContinue) { Write-Host 'note: git still resolvable' } else { Write-Host 'git removed from PATH' }
# NOTE: we do NOT cold-box the MFC runtime here. Deleting mfc140u.dll on the runner creates a
# "redist registered but file missing" state that vc_redist /repair can't reliably restore (MSI
# repair is keypath-based) — so it falsely failed. On a real fresh VM the redist isn't registered
# and the agent's /install works. The VC++ provisioning is validated on real VMs + by
# verify_ufo_imports in bootstrap; a faithful cold-VM e2e is tracked as a follow-up.
$log = Join-Path $env:RUNNER_TEMP 'inno-install.log'
"INNO_LOG=$log" | Out-File -FilePath $env:GITHUB_ENV -Append # the diagnostics step reads it
Write-Host "installing: $setup"
$p = Start-Process "$setup" -ArgumentList '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/NOCANCEL', "/LOG=$log" -PassThru
# Stream the Inno log while the installer runs (its own log, not the agent's — plain loop).
$seen = 0
$deadline = (Get-Date).AddMinutes(5)
while (-not $p.HasExited -and (Get-Date) -lt $deadline) {
  Start-Sleep -Seconds 5
  if (Test-Path $log) {
    $lines = Get-Content $log
    if ($lines.Count -gt $seen) { $lines[$seen..($lines.Count - 1)] | ForEach-Object { Write-Host "  [inno] $_" }; $seen = $lines.Count }
  }
}
if (-not $p.HasExited) {
  Write-Host '::error::installer did not finish within 5 minutes'
  if (Test-Path $log) { Get-Content $log | Write-Host }
  $p | Stop-Process -Force -ErrorAction SilentlyContinue
  throw 'installer hung'
}
if (-not (Test-Path "$env:ProgramFiles\UFOAgent\ufoagent.exe")) { throw 'ufoagent.exe was not installed' }
Write-Host "installer exit code: $($p.ExitCode)"
