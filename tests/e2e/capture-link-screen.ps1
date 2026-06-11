# Screenshot the REAL pairing screen for the site: `link --force` mints a fresh code + QR in its
# own console. The code is never approved here so it simply expires; the stored node token is
# untouched (link only writes the token on approval).
$ErrorActionPreference = 'Continue'
. "$PSScriptRoot/helpers.ps1"
(New-Object -ComObject Shell.Application).MinimizeAll()
Start-Process $Exe -ArgumentList 'link', '--force'
Start-Sleep -Seconds 10
Save-Shot 'link-screen'
# Kill ONLY the link invocation, never the service.
Get-CimInstance Win32_Process -Filter "Name='ufoagent.exe'" | Where-Object { $_.CommandLine -match '\slink\b' } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
