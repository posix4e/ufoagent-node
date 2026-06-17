#!/usr/bin/env bash
# Slice the ONE continuous recording into per-phase marketing gifs. Frames are named frame-<host-epoch-ms>.png;
# phases.json carries GUEST epoch-ms boundaries, so add `skew` to convert guest->host before selecting.
# Same ffmpeg recipe the old assemble-gifs used (4fps timelapse, mpdecimate drops idle, 960px, 128 colors).
set -uo pipefail
REC="$1"; PHASES="$2"; SKEW="$3"; OUT="$4"
mkdir -p "$OUT"
command -v ffmpeg >/dev/null || { echo "ffmpeg missing on host"; exit 1; }
command -v jq >/dev/null || { echo "jq missing on host"; exit 1; }

# phase label -> published gif basename (link kept for archive; the site uses the robot for link)
gifname(){ case "$1" in
  install)  echo install-screen;;
  link)     echo link-screen;;
  local)    echo local-task;;
  remote)   echo remote-task;;
  activity) echo activity-summary;;
  *)        echo "$1";; esac; }

# PowerShell ConvertTo-Json unwraps a single-element array to a bare object; normalize to an array.
[ -s "$PHASES" ] || { echo "no phases.json (journey produced none)"; exit 0; }
PH=$(jq -c 'if type=="array" then . else [.] end' "$PHASES" 2>/dev/null || echo '[]')
n=$(echo "$PH" | jq 'length' 2>/dev/null || echo 0); n=${n:-0}
[ "$n" -gt 0 ] 2>/dev/null || { echo "no phases"; exit 0; }
for i in $(seq 0 $((n-1))); do
  label=$(echo "$PH" | jq -r ".[$i].label")
  gs=$(echo "$PH" | jq -r ".[$i].start"); ge=$(echo "$PH" | jq -r ".[$i].end")
  hs=$(( gs + SKEW )); he=$(( ge + SKEW ))
  d="$OUT/frames-$label"; rm -rf "$d"; mkdir -p "$d"; k=0
  for f in "$REC"/frame-*.png; do
    [ -e "$f" ] || continue
    ms=$(basename "$f"); ms=${ms#frame-}; ms=${ms%.png}
    if [ "$ms" -ge "$hs" ] && [ "$ms" -le "$he" ]; then
      cp "$f" "$(printf '%s/%04d.png' "$d" "$k")"; k=$((k+1))
    fi
  done
  if [ "$k" -lt 4 ]; then echo "  $label: only $k frames in window - skip"; continue; fi
  # a representative still (last frame of the phase) + the gif
  cp "$(printf '%s/%04d.png' "$d" $((k-1)))" "$OUT/$(gifname "$label").png" 2>/dev/null || true
  gif="$OUT/$(gifname "$label").gif"
  ffmpeg -y -framerate 4 -i "$d/%04d.png" -vf "mpdecimate,setpts=N/(4*TB),scale=960:-2:flags=lanczos,split[a][b];[a]palettegen=max_colors=128[p];[b][p]paletteuse=dither=bayer:bayer_scale=3" -frames:v 240 -loop 0 "$gif" >/dev/null 2>&1
  if [ -f "$gif" ]; then echo "  $(gifname "$label").gif  ($k frames, $(du -h "$gif" | cut -f1))"; else echo "  $label: ffmpeg produced no gif"; fi
done
