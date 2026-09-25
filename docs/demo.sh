#!/usr/bin/env bash
# The README's screenshots and demo video, made with omabox itself. A box plays the desktop (Omarchy's
# stock bar and a stock theme, the omabox widget in the bar, a terminal) and a second box runs inside
# it, driven from that terminal as an agent would drive one. Nothing here touches the real desktop:
# the recording is wf-recorder inside the stage box.
#
#   docs/demo.sh [--video] [OUT_DIR]     default OUT_DIR: docs/media (preview.png goes to the repo root)
#
# Needs: the omabox tools built (install.sh), ImageMagick (magick) for the widget picture, and for
# --video wf-recorder and ffmpeg (pacman -S wf-recorder ffmpeg).
set -euo pipefail

ROOT=$(cd "$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")/.." && pwd)
CLI=$ROOT/bin/omabox
STAGE=omabox-demo
VIDEO=0
[ "${1:-}" = --video ] && { VIDEO=1; shift; }
OUT=${1:-$ROOT/docs/media}
mkdir -p "$OUT"

ob() { "$CLI" "$@"; }
stage() { ob run -b "$STAGE" -- "$@"; }
clients() { ob hyprctl -b "$STAGE" -j clients; }
has_client() { clients | jq -e --arg c "$1" 'any(.[]; .class == $c)' >/dev/null; }
until_ok() { local n=$(( $1 * 5 )); shift; while ! "$@" 2>/dev/null; do n=$((n - 1)); [ $n -gt 0 ] || { echo "demo: timed out on: $*" >&2; exit 1; }; sleep 0.2; done; }
inner_up() { stage omabox ls --json | jq -e --arg n "$1" 'any(.[]; .name == $n and .state == "up")' >/dev/null; }
shot() { ob shot -b "$STAGE" -o "$1" >/dev/null; }

# Type a command into the stage's terminal at a human pace (three characters per key event), then Enter.
say() {
  local s=$1 i
  for ((i = 0; i < ${#s}; i += 3)); do ob keys -b "$STAGE" -t "${s:i:3}" >/dev/null; done
  sleep 0.4
  ob keys -b "$STAGE" Return >/dev/null
}

cleanup() { [ -n "${KEEP:-}" ] || ob down "$STAGE" >/dev/null 2>&1 || true; }
trap cleanup EXIT

# The stage: 1280x720 so text stays legible scaled down in a README, an isolated network (nothing in
# it needs one), Omarchy's stock bar and theme rather than this machine's.
ob up "$STAGE" --size 1280x720 --stock-bar --plugin "$ROOT/plugin" --net isolated --idle 0 >/dev/null
H=$(ob path "$STAGE")/home
ln -sfn "$CLI" "$H/.local/bin/omabox"   # the widget and the terminal run this omabox, in the stage
stage omarchy-theme-set tokyo-night >/dev/null 2>&1
# Peek and interactive windows open next to the terminal instead of on workspace 9 (a setting), and
# the terminal keeps the narrow third of the screen.
stage omabox config workspace 1 >/dev/null
ob hyprctl -b "$STAGE" eval 'hl.config({ dwindle = { default_split_ratio = 0.7 } })' >/dev/null
ob keys -b "$STAGE" super+Return >/dev/null
until_ok 10 has_client foot
sleep 1
ob keys -b "$STAGE" -t 'export OMABOX=agent; clear' Return >/dev/null
sleep 1

if [ $VIDEO = 1 ]; then
  stage bash -c 'rm -f ~/demo.mp4; setsid wf-recorder -y -f ~/demo.mp4 >~/wf-recorder.log 2>&1 &'
  sleep 1.5
fi

say '# an agent starts a desktop of its own'
sleep 0.6
say 'omabox up --size 1280x720'
until_ok 30 inner_up agent
until_ok 30 bash -c "! '$CLI' run -b '$STAGE' -- pgrep -f 'omabox up --size' >/dev/null"
sleep 1.2
say 'omabox peek'                     # a live, view-only window of it, for you
until_ok 10 has_client omabox-peek
sleep 1.8
say 'omabox run -- omarchy-theme-set catppuccin-latte'
sleep 3.5
say 'omabox keys super+alt+space'     # Omarchy's menu, in the agent's box
sleep 2.5
say 'omabox keys Escape'
sleep 0.8
say 'omabox run -d -- nautilus'       # an app, on the box's own (fake) HOME
until_ok 10 bash -c "'$CLI' run -b '$STAGE' -- omabox hyprctl -b agent -j clients | jq -e 'any(.[]; .class == \"org.gnome.Nautilus\")' >/dev/null"
sleep 2
say 'omabox shot'
sleep 1.5
# The widget: the box in the bar
ob click -b "$STAGE" 1259 12 >/dev/null
sleep 2.5
shot "$ROOT/preview.png"
ob keys -b "$STAGE" Escape >/dev/null
sleep 0.5
ob click -b "$STAGE" 220 660 >/dev/null   # back to the terminal
sleep 0.5
say 'omabox down'
until_ok 10 bash -c "! '$CLI' hyprctl -b '$STAGE' -j clients | jq -e 'any(.[]; .class == \"omabox-peek\")' >/dev/null"
sleep 1.5
# An interactive box, from the widget: a whole Omarchy desktop in a window you use
ob click -b "$STAGE" 1259 12 >/dev/null
sleep 1.5
ob click -b "$STAGE" 1249 64 >/dev/null   # the gear: Settings
ob pointer -b "$STAGE" -- move 640 600 >/dev/null   # off the panel
sleep 2
shot "$OUT/settings.png"
ob keys -b "$STAGE" Escape >/dev/null
sleep 1
ob keys -b "$STAGE" n n >/dev/null        # New interactive box (n twice)
until_ok 30 has_client aquamarine
sleep 4
ob click -b "$STAGE" 860 400 >/dev/null
ob keys -b "$STAGE" super+alt+Escape >/dev/null   # SUPER keys go to the box now
sleep 0.5
ob keys -b "$STAGE" super+alt+space >/dev/null   # Omarchy's menu, in the interactive box
sleep 2.5
shot "$OUT/interactive.png"
sleep 1

if [ $VIDEO = 1 ]; then
  stage pkill -INT -x wf-recorder || true
  # It finishes on its next frame, and a still screen sends none: move the pointer until it is done.
  for i in $(seq 60); do
    stage pgrep -x wf-recorder >/dev/null || break
    [ "$i" -lt 60 ] || { echo "demo: wf-recorder did not finish" >&2; exit 1; }
    ob pointer -b "$STAGE" -- move $((600 + i % 2 * 20)) 700 >/dev/null
    sleep 0.5
  done
  # wf-recorder writes a frame only on damage: make it a steady 30 fps and hold the last frame.
  ffmpeg -v error -y -i "$H/demo.mp4" -vf "fps=30,tpad=stop_mode=clone:stop_duration=2" \
    -c:v libx264 -preset slow -crf 26 -pix_fmt yuv420p -movflags +faststart "$OUT/demo.mp4"
fi
stage omabox down --all >/dev/null 2>&1 || true

# The widget's two faces side by side (the list with the agent's box, from preview.png, and Settings),
# cropped from the stage's top right corner where the panel opens: its size at 1280x720.
magick "$ROOT/preview.png" -crop 404x206+873+29 +repage "$OUT/.a.png"
magick "$OUT/settings.png" -crop 404x356+873+29 +repage "$OUT/.b.png"
magick "$OUT/.a.png" -size 24x1 xc:none "$OUT/.b.png" -background none -gravity north +append "$OUT/widget.png"
rm -f "$OUT/.a.png" "$OUT/.b.png" "$OUT/settings.png"
echo "demo: $ROOT/preview.png and $OUT/"
