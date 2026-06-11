# Desktop frame recorder for the CI task runs. Snapshots the whole virtual screen into OutDir
# as 0000.png, 0001.png, ... every IntervalSec until StopFile appears (or the job dies). The
# frames are assembled into the site's remote-task.gif / local-task.gif after the tasks pass.
# Runs hidden (Start-Process -WindowStyle Hidden) so the recorder itself never appears in shot.
param(
  [Parameter(Mandatory = $true)][string]$OutDir,
  [Parameter(Mandatory = $true)][string]$StopFile,
  [double]$IntervalSec = 2
)
$ErrorActionPreference = 'Continue'
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$i = 0
while (-not (Test-Path $StopFile)) {
  try {
    $b = [System.Windows.Forms.SystemInformation]::VirtualScreen
    $bmp = New-Object System.Drawing.Bitmap($b.Width, $b.Height)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen($b.X, $b.Y, 0, 0, $bmp.Size)
    $bmp.Save((Join-Path $OutDir ('{0:d4}.png' -f $i)))
    $g.Dispose(); $bmp.Dispose()
    $i++
  } catch {}
  Start-Sleep -Milliseconds ([int]($IntervalSec * 1000))
}
