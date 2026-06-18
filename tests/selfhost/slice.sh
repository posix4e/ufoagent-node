#!/usr/bin/env bash
# Slice the ONE continuous recording into per-phase marketing gifs. Frames are named frame-<host-epoch-ms>.png;
# phases.json carries GUEST epoch-ms boundaries (+ a per-phase `crop` window rect), so add `skew` to convert
# guest->host before selecting, and crop each gif to its window so the content isn't lost in the full desktop.
set -uo pipefail
REC="$1"; PHASES="$2"; SKEW="$3"; OUT="$4"
mkdir -p "$OUT"
command -v ffmpeg >/dev/null || { echo "ffmpeg missing on host"; exit 1; }
command -v jq >/dev/null || { echo "jq missing on host"; exit 1; }

gifname(){ case "$1" in
  install)   echo install-screen;;
  link)      echo link-screen;;
  remote)    echo remote-task;;
  dashboard) echo mission-control;;
  *)         echo "$1";; esac; }

# PowerShell ConvertTo-Json unwraps a single-element array to a bare object; normalize to an array.
[ -s "$PHASES" ] || { echo "no phases.json (journey produced none)"; exit 0; }
PH=$(jq -c 'if type=="array" then . else [.] end' "$PHASES" 2>/dev/null || echo '[]')
n=$(echo "$PH" | jq 'length' 2>/dev/null || echo 0); n=${n:-0}
[ "$n" -gt 0 ] 2>/dev/null || { echo "no phases"; exit 0; }

# source frame dims (for clamping crops) - read from the first frame
first=$(ls "$REC"/frame-*.png 2>/dev/null | head -1)
FW=1280; FH=800
if [ -n "$first" ]; then d=$(ffprobe -v error -select_streams v:0 -show_entries stream=width,height -of csv=s=x:p=0 "$first" 2>/dev/null); [ -n "$d" ] && { FW=${d%x*}; FH=${d#*x}; }; fi

for i in $(seq 0 $((n-1))); do
  label=$(echo "$PH" | jq -r ".[$i].label")
  ok=$(echo "$PH" | jq -r ".[$i].ok // false")
  skipped=$(echo "$PH" | jq -r ".[$i].skipped // false")
  if [ "$ok" != "true" ] || [ "$skipped" = "true" ]; then
    echo "  $label: skipped/failed phase - no asset"
    continue
  fi
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

  # crop filter from the phase's window rect (clamped to the frame, even dims); empty = full frame
  cropf=""
  crop=$(echo "$PH" | jq -c ".[$i].crop // empty")
  if [ -n "$crop" ]; then
    cx=$(echo "$crop"|jq -r '.x|floor'); cy=$(echo "$crop"|jq -r '.y|floor')
    cw=$(echo "$crop"|jq -r '.w|floor'); ch=$(echo "$crop"|jq -r '.h|floor')
    [ "$cx" -lt 0 ] && cx=0; [ "$cy" -lt 0 ] && cy=0
    [ $((cx+cw)) -gt "$FW" ] && cw=$((FW-cx)); [ $((cy+ch)) -gt "$FH" ] && ch=$((FH-cy))
    cw=$((cw - cw%2)); ch=$((ch - ch%2))
    [ "$cw" -ge 120 ] && [ "$ch" -ge 120 ] && cropf="crop=${cw}:${ch}:${cx}:${cy},"
  fi

  # per-phase tuning: install is a long provisioning timelapse - cap it short + fewer colors so it stays small
  case "$label" in
    install) cap=60; colors=64;;
    *)       cap=240; colors=128;;
  esac

  vf="${cropf}mpdecimate,setpts=N/(4*TB),scale=min(900\\,iw):-2:flags=lanczos,split[a][b];[a]palettegen=max_colors=${colors}[p];[b][p]paletteuse=dither=bayer:bayer_scale=3"
  gif="$OUT/$(gifname "$label").gif"
  ffmpeg -y -framerate 4 -i "$d/%04d.png" -vf "$vf" -frames:v "$cap" -loop 0 "$gif" >/dev/null 2>&1
  # representative still = last in-window frame, cropped to match
  png="$OUT/$(gifname "$label").png"
  ffmpeg -y -i "$(printf '%s/%04d.png' "$d" $((k-1)))" ${cropf:+-vf ${cropf%,}} "$png" >/dev/null 2>&1 || cp "$(printf '%s/%04d.png' "$d" $((k-1)))" "$png" 2>/dev/null || true
  if [ -f "$gif" ]; then echo "  $(gifname "$label").gif  ($k frames, crop=[${cropf:-full}], $(du -h "$gif" | cut -f1))"; else echo "  $label: ffmpeg produced no gif"; fi
done
