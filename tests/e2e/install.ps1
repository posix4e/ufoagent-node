# Silent install with the Inno log streamed live. Git is stripped from PATH first so the
# installer's background bootstrap must take the no-git zip path (the real customer scenario).
$ErrorActionPreference = 'Stop'
. "$PSScriptRoot/helpers.ps1"
$setup = (Get-ChildItem -Recurse dl -Filter ufoagent-setup.exe | Select-Object -First 1).FullName
if (-not $setup) { throw 'installer artifact missing' }
Add-MpPreference -ExclusionPath "$env:ProgramFiles\UFOAgent", (Join-Path (Get-Location) 'dl') -ErrorAction SilentlyContinue
$env:PATH = (($env:PATH -split ';') | Where-Object { $_ -and -not (Test-Path (Join-Path $_ 'git.exe')) }) -join ';'
if (Get-Command git -ErrorAction SilentlyContinue) { Write-Host 'note: git still resolvable' } else { Write-Host 'git removed from PATH' }
# Cold-box: drop the MFC runtime (mfc140u.dll) so the agent's bootstrap MUST install the VC++
# redistributable for win32ui to load — the real fresh-VM scenario the GitHub runner otherwise masks
# (it ships VC++ via Visual Studio). Runner is ephemeral, so renaming a System32 DLL is safe; the
# agent's install_vc_redist restores it, and verify_ufo_imports fails the build if it doesn't.
foreach ($d in @("$env:WINDIR\System32\mfc140u.dll", "$env:WINDIR\SysWOW64\mfc140u.dll")) {
  if (Test-Path $d) { try { Rename-Item $d "$d.coldbox.bak" -Force -ErrorAction Stop; Write-Host "cold-box: removed $d" } catch { Write-Host "::warning::could not remove $d ($($_.Exception.Message)) — MFC test less faithful this run" } }
  else { Write-Host "cold-box: already absent $d" }
}
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
