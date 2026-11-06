#!/usr/bin/env bash
# test/distros-boxes.sh OUT: the box half of `test/distros.sh --boxes` (#247), run as the container's
# user once install.sh passed there; not for a host (it refuses outside a podman container). A plain
# box (`up --desktop plain`): no config errors, foot started by `run -d`, `wait window`, `windows`,
# `keys` typed into it and run, SUPER+RETURN's terminal, a shot, `down` leaving no box folder, HOME
# or process; then, where Quickshell is packaged, a plain box whose shell is a one-PanelWindow bar
# (`--shell 'qs -p BAR'`): its layer drawn, a shot, down. Prints `ok`/`FAIL` per step and a last
# `boxes: N passed, M failed` line; the shots go to OUT (plain.png, shell.png), and a failed box's
# logs to OUT/kept-<box>/. Exit 1 when a step failed.
set -uo pipefail

[ -e /run/.containerenv ] || [ "${OMABOX_DISTROS_VM:-}" = 1 ] ||
  { echo "distros-boxes.sh: runs inside test/distros.sh's container or test/distros-vm.sh's VM only" >&2; exit 2; }
out=${1:?usage: distros-boxes.sh OUT}
mkdir -p "$out"
pass=0 fail=0
# What this user runs before any box (a VM's systemd --user and SSH session): not left by a box.
before=" $(ps -u "$(id -u)" -o pid= | tr -s ' \n' '  ') "

step() {
  local what=$1; shift
  if "$@"; then echo "ok    $what"; pass=$((pass + 1))
  else echo "FAIL  $what (exit $?)"; fail=$((fail + 1)); return 1; fi
}
same() { [ "$2" = "$3" ] || { echo "      $1: want '$2', got '$3'"; return 1; }; }
# keep BOX: its box.log, HOME logs and Hyprland's log, before down removes them, and its screen as
# it is now (a step that failed: what was on it).
keep() {
  local d=${XDG_RUNTIME_DIR:?}/omabox/$1 k=$out/kept-$1
  mkdir -p "$k/home"
  cp -P "$d/box.log" "$k/" 2>/dev/null
  cp -P "$d"/home/*.log "$k/home/" 2>/dev/null
  cp -P "$d"/run/hypr/*/hyprland.log "$k/" 2>/dev/null
  omabox shot -b "$1" -o "$k/screen-$(date +%s%N).png" >/dev/null 2>&1
  return 0
}
# gone BOX: down left nothing of it: no folder, no HOME, no process of this user but ours.
gone() {
  local left
  [ ! -e "$XDG_RUNTIME_DIR/omabox/$1" ] || { echo "      its folder is left"; return 1; }
  [ ! -e "${XDG_CACHE_HOME:-$HOME/.cache}/omabox/$1" ] || { echo "      its HOME is left"; return 1; }
  left=$(ps -u "$(id -u)" -o pid=,stat=,args= |
    awk -v me=$$ -v before="$before" '$1 != me && $2 !~ /^Z/ && $3 !~ /^(ps|awk|bash|runuser)$/ && !index(before, " " $1 " ")')
  [ -z "$left" ] || { echo "      left running:"; echo "        ${left//$'\n'/$'\n'        }"; return 1; }
}

# record_steps BOX (#442): where wf-recorder and ffmpeg are installed, record in BOX with the
# distribution's own ffmpeg (libx264, or openh264 where it has none: Fedora's ffmpeg-free,
# openSUSE's): a take, its contact sheet, a take with --cut-idle, and two monitors as one video.
# Each an H.264 MP4 that this ffmpeg reads.
record_steps() {
  local b=$1 v=$out/rec.mp4 o
  if ! command -v wf-recorder >/dev/null || ! command -v ffmpeg >/dev/null; then
    echo "      (no wf-recorder or ffmpeg: no record)"; return 0
  fi
  echo "      record's encoder: $(omabox doctor 2>&1 | grep -o "record's H.264 by [a-z0-9]*" || echo none)"
  o=$(omabox record -b "$b" -o "$v" --for 2s 2>&1)
  echo "      ${o//$'\n'/$'\n'      }"
  step "record --for 2s" same file "$v" "$(tail -n 1 <<<"$o")" || keep "$b"
  step "...an H.264 video at 30 fps" same video "h264 30/1" \
    "$(ffprobe -v error -select_streams v:0 -show_entries stream=codec_name,r_frame_rate -of csv=p=0 "$v" 2>&1 | tr , ' ')"
  step "record sheet" omabox record -b "$b" sheet "$v" -o "$out/rec.sheet.png" --frames 3
  step "record --cut-idle" omabox record -b "$b" -o "$out/cut.mp4" --cut-idle 300ms --for 2s
  step "...an H.264 video" same video h264 \
    "$(ffprobe -v error -select_streams v:0 -show_entries stream=codec_name -of csv=p=0 "$out/cut.mp4" 2>&1)"
  if step "monitor add" omabox monitor -b "$b" add 800x600; then
    step "record --monitor all" omabox record -b "$b" -o "$out/all.mp4" --monitor all --for 1s || keep "$b"
    # (the 800x600 right of the main screen: the video as wide as both)
    step "...one H.264 video of both" same video "h264 $(omabox monitor -b "$b" list --json | jq '[.[] | .x + (.mode | split("x")[0] | tonumber)] | max')" \
      "$(ffprobe -v error -select_streams v:0 -show_entries stream=codec_name,width -of csv=p=0 "$out/all.mp4" 2>&1 | tr , ' ')"
    omabox monitor -b "$b" remove "$(omabox monitor -b "$b" list --json | jq -r '.[-1].name')" >/dev/null 2>&1
  fi
}

echo "==> a plain box"
B=c1
if step "up --desktop plain" omabox up "$B" --desktop plain; then
  step "no config errors" same configerrors "" "$(omabox hyprctl -b "$B" configerrors | tr -d '[:space:]')" || keep "$B"
  step "the box's Hyprland" sh -c "omabox hyprctl -b $B version | head -n 1"
  step "run -d foot" omabox run -b "$B" -d --wait -- foot -a c1-term || keep "$B"
  step "wait window" omabox wait -b "$B" --timeout 10s window c1-term || keep "$B"
  step "windows lists it" sh -c "omabox windows -b $B | grep -q c1-term"
  step "keys typed into it" omabox keys -b "$B" --window c1-term --wait -t 'echo plain-typed > /tmp/c1-typed'
  omabox keys -b "$B" --window c1-term --wait Return >/dev/null
  step "...and run" omabox wait -b "$B" --timeout 10s until -- grep -qx plain-typed /tmp/c1-typed || keep "$B"
  step "a shot" omabox shot -b "$B" -o "$out/plain.png"
  step "...not empty" test -s "$out/plain.png"
  step "SUPER+RETURN opens a terminal" omabox keys -b "$B" --wait super+Return
  step "...a second window" same windows 2 "$(omabox windows -b "$B" --json | jq length)" || keep "$B"
  record_steps "$B"
  step "down" omabox down "$B"
  step "...leaves nothing" gone "$B"
else
  keep "$B"; omabox down "$B" >/dev/null 2>&1
fi

if command -v qs >/dev/null; then
  echo "==> a plain box with a Quickshell bar as its shell"
  B=c2 bar=$HOME/bar
  mkdir -p "$bar"
  cat > "$bar/shell.qml" <<'QML'
import QtQuick
import Quickshell
import Quickshell.Wayland
PanelWindow {
  WlrLayershell.namespace: "c2-bar"
  anchors { top: true; left: true; right: true }
  implicitHeight: 30
  color: "#336699"
  Text { anchors.centerIn: parent; text: "plain bar"; color: "white" }
}
QML
  if step "up --desktop plain --shell 'qs -p BAR'" omabox up "$B" --desktop plain --ro-bind "$bar" --shell "qs -p $bar"; then
    step "its layer is drawn" omabox wait -b "$B" --timeout 5s layer c2-bar || keep "$B"
    step "ls names its shell" sh -c "omabox ls | grep -qF 'shell: qs -p $bar'"
    step "a shot" omabox shot -b "$B" -o "$out/shell.png"
    step "...not empty" test -s "$out/shell.png"
    step "down" omabox down "$B"
    step "...leaves nothing" gone "$B"
  else
    keep "$B"; omabox down "$B" >/dev/null 2>&1
  fi
else
  echo "==> no Quickshell here: no box with a shell"
fi

echo "boxes: $pass passed, $fail failed"
[ "$fail" = 0 ]
