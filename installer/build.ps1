# Local build: ufoagentd.exe (PyInstaller) -> ufoagent-setup.exe (Inno Setup).
# Usage:  pwsh installer\build.ps1 -Version 0.1.0
param([string]$Version = "0.1.0")
$ErrorActionPreference = "Stop"

Write-Host "==> Installing build deps"
python -m pip install --upgrade pip pyinstaller pywin32

Write-Host "==> Freezing ufoagentd.exe"
pyinstaller build/ufoagentd.spec --noconfirm --distpath dist --workpath build/_work

Write-Host "==> Compiling installer (Inno Setup)"
$iscc = "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe"
& $iscc "/DAppVersion=$Version" installer\ufoagent.iss

Write-Host "==> Done. Output: installer\Output\ufoagent-setup.exe"
