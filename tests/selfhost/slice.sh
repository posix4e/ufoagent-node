#!/usr/bin/env bash
# Slice the one continuous host recording into website-contract assets using capture markers.
# Frames are named frame-<host-epoch-ms>.png; markers.json carries GUEST epoch-ms capture
# points plus the foreground crop rect. Add `skew` to convert guest->host.
set -euo pipefail

REC="$1"; MARKERS="$2"; SKEW="$3"; OUT="$4"
mkdir -p "$OUT"
command -v ffmpeg >/dev/null || { echo "ffmpeg missing on host"; exit 1; }
command -v ffprobe >/dev/null || { echo "ffprobe missing on host"; exit 1; }
command -v jq >/dev/null || { echo "jq missing on host"; exit 1; }

required=(install-screen link-screen third-party-app mission-control)

gha_escape(){
  local s="${1:-}"
  s=${s//'%'/'%25'}; s=${s//$'\r'/'%0D'}; s=${s//$'\n'/'%0A'}
  printf '%s' "$s"
}

emit_slice_progress(){
  local msg="$1"
  echo "  $msg"
  if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
    printf -- '- %s %s\n' "$(date -u +%H:%M:%SZ)" "$msg" >> "$GITHUB_STEP_SUMMARY"
  fi
  if [ -n "${GITHUB_ACTIONS:-}" ]; then
    printf '::notice title=Selfhost e2e::%s\n' "$(gha_escape "$msg")"
  fi
}

window_pre_ms(){
  case "$1" in
    third-party-app) echo 300000;;
    *) echo 0;;
  esac
}

window_post_ms(){
  case "$1" in
    install-screen) echo 180000;;
    link-screen) echo 45000;;
    third-party-app) echo 30000;;
    mission-control) echo 120000;;
    *) echo 60000;;
  esac
}

gray_average(){
  local png="$1"
  ffmpeg -v error -i "$png" -vf 'scale=1:1,format=gray' -frames:v 1 -f rawvideo - 2>/dev/null |
    od -An -tu1 | tr -d '[:space:]'
}

assert_nonblack_png(){
  local png="$1" avg
  [ -s "$png" ] || return 1
  avg=$(gray_average "$png" || true)
  [[ "$avg" =~ ^[0-9]+$ ]] || return 1
  [ "$avg" -gt 5 ]
}

[ -s "$MARKERS" ] || { echo "no markers.json (journey produced no capture markers)"; exit 1; }
M=$(jq -c 'if type=="array" then . else [.] end | sort_by(.ts)' "$MARKERS")
n=$(echo "$M" | jq 'length')
[ "$n" -gt 0 ] || { echo "no capture markers"; exit 1; }

dups=$(echo "$M" | jq -r 'group_by(.label)[] | select(length > 1) | .[0].label' | paste -sd, -)
[ -z "$dups" ] || { echo "duplicate capture labels in markers.json: $dups"; exit 1; }

first=$(ls "$REC"/frame-*.png 2>/dev/null | head -1 || true)
last=$(ls "$REC"/frame-*.png 2>/dev/null | tail -1 || true)
[ -n "$first" ] && [ -n "$last" ] || { echo "recording has no frames"; exit 1; }
first_ms=$(basename "$first"); first_ms=${first_ms#frame-}; first_ms=${first_ms%.png}
last_ms=$(basename "$last"); last_ms=${last_ms#frame-}; last_ms=${last_ms%.png}

FW=1280; FH=800
d=$(ffprobe -v error -select_streams v:0 -show_entries stream=width,height -of csv=s=x:p=0 "$first" 2>/dev/null || true)
[ -n "$d" ] && { FW=${d%x*}; FH=${d#*x}; }

for i in $(seq 0 $((n-1))); do
  label=$(echo "$M" | jq -r ".[$i].label")
  ts=$(echo "$M" | jq -r ".[$i].ts")
  pre=$(window_pre_ms "$label")
  post=$(window_post_ms "$label")
  hs=$(( ts + SKEW - pre ))
  he=$(( ts + SKEW + post ))
  [ "$hs" -lt "$first_ms" ] && hs="$first_ms"
  [ "$he" -gt "$last_ms" ] && he="$last_ms"
  [ "$he" -gt "$hs" ] || { echo "marker $label maps to an empty frame window"; exit 1; }

  emit_slice_progress "slice started: $label"
  d="$OUT/frames-$label"; rm -rf "$d"; mkdir -p "$d"; k=0
  for f in "$REC"/frame-*.png; do
    [ -e "$f" ] || continue
    ms=$(basename "$f"); ms=${ms#frame-}; ms=${ms%.png}
    if [ "$ms" -ge "$hs" ] && [ "$ms" -le "$he" ]; then
      cp "$f" "$(printf '%s/%04d.png' "$d" "$k")"
      k=$((k+1))
    fi
  done
  [ "$k" -ge 4 ] || { echo "slice $label only had $k frames"; exit 1; }

  cropf=""
  crop=$(echo "$M" | jq -c ".[$i].fg_rect // empty")
  if [ -n "$crop" ]; then
    cx=$(echo "$crop" | jq -r '.x|floor'); cy=$(echo "$crop" | jq -r '.y|floor')
    cw=$(echo "$crop" | jq -r '.w|floor'); ch=$(echo "$crop" | jq -r '.h|floor')
    [ "$cx" -lt 0 ] && cx=0; [ "$cy" -lt 0 ] && cy=0
    [ $((cx+cw)) -gt "$FW" ] && cw=$((FW-cx))
    [ $((cy+ch)) -gt "$FH" ] && ch=$((FH-cy))
    cw=$((cw - cw%2)); ch=$((ch - ch%2))
    [ "$cw" -ge 120 ] && [ "$ch" -ge 120 ] && cropf="crop=${cw}:${ch}:${cx}:${cy},"
  fi

  target=240
  colors=128
  case "$label" in
    install-screen) target=120; colors=64;;
    link-screen) target=120;;
    mission-control) target=240;;
    third-party-app) target=240;;
  esac
  step=$(( (k + target - 1) / target ))
  [ "$step" -lt 1 ] && step=1
  selectf=""
  [ "$step" -gt 1 ] && selectf="select='not(mod(n\\,$step))',"

  vf="${cropf}${selectf}mpdecimate,setpts=N/(4*TB),scale=min(900\\,iw):-2:flags=lanczos,split[a][b];[a]palettegen=max_colors=${colors}[p];[b][p]paletteuse=dither=bayer:bayer_scale=3"
  gif="$OUT/$label.gif"
  png="$OUT/$label.png"
  emit_slice_progress "slice building: $label.gif ($k frames, step=$step)"
  ffmpeg -y -framerate 4 -i "$d/%04d.png" -vf "$vf" -frames:v "$target" -loop 0 "$gif" >/dev/null 2>&1

  still_index=$((k-1))
  if [ -n "$cropf" ]; then
    ffmpeg -y -i "$(printf '%s/%04d.png' "$d" "$still_index")" -vf "${cropf%,}" "$png" >/dev/null 2>&1
  else
    ffmpeg -y -i "$(printf '%s/%04d.png' "$d" "$still_index")" "$png" >/dev/null 2>&1
  fi
  [ -s "$gif" ] || { echo "slice failed: $label.gif was not produced"; exit 1; }
  assert_nonblack_png "$png" || { echo "slice failed: $label.png appears black or unreadable"; exit 1; }
  emit_slice_progress "slice done: $label.gif ($k frames, crop=[${cropf:-full}], $(du -h "$gif" | cut -f1))"
done

for f in "${required[@]}"; do
  [ -s "$OUT/$f.gif" ] || { echo "missing required capture asset: $f.gif"; exit 1; }
  [ -s "$OUT/$f.png" ] || { echo "missing required capture asset: $f.png"; exit 1; }
  assert_nonblack_png "$OUT/$f.png" || { echo "required capture asset appears black: $f.png"; exit 1; }
done
