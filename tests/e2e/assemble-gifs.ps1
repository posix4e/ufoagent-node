# Assemble the task frame recordings into the site's timelapse GIFs. Runs on PRs too (the
# publish step stays main-only), so a PR's tray-screenshots artifact shows exactly what would
# ship. Frames come from record-frames.ps1, started/stopped by the task scripts.
$ErrorActionPreference = 'Continue'
. "$PSScriptRoot/helpers.ps1"
if (-not (Get-Command ffmpeg -ErrorAction SilentlyContinue)) { choco install ffmpeg --no-progress -y | Out-Null }
foreach ($t in @('remote', 'local')) {
  $dir = Join-Path $Shots "frames-$t"
  $n = (Get-ChildItem $dir -Filter *.png -ErrorAction SilentlyContinue | Measure-Object).Count
  if ($n -lt 6) { Write-Host "::warning::frames-$t has only $n frames - skipping gif"; continue }
  # mpdecimate drops the long idle stretches (UFO2 planning), so the gif is mostly motion:
  # console activity, Notepad opening, the message being typed. ~4 fps timelapse, 960px wide,
  # 128-color palette; -frames:v caps a worst case at ~60s of gif.
  $gif = Join-Path $Shots "$t-task.gif"
  ffmpeg -y -framerate 4 -i "$dir\%04d.png" -vf "mpdecimate,setpts=N/(4*TB),scale=960:-2:flags=lanczos,split[a][b];[a]palettegen=max_colors=128[p];[b][p]paletteuse=dither=bayer:bayer_scale=3" -frames:v 240 -loop 0 $gif 2>&1 | Select-Object -Last 4
  if (Test-Path $gif) { Write-Host ("gif: {0}-task.gif ({1:n1} MB from {2} frames)" -f $t, ((Get-Item $gif).Length / 1MB), $n) }
  else { Write-Host "::warning::ffmpeg produced no $t-task.gif" }
}
