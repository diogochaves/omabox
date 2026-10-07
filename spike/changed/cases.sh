#!/usr/bin/env bash
# Phase 1 of #145 (finding 243): look-after-action cases, scripted in a box, each a before and an
# after frame of what an agent would shoot (the window; the screen for a drag ghost, a notification
# or the launcher), measured by measure.sh. Each group wants a fresh box of its own (the windows tile):
#   cases.sh BOX OUTDIR probe|nautilus|shell
set -uo pipefail   # (not -e: a --wait that times out still leaves a frame to measure)
[ -z "${TRACE:-}" ] || set -x
here=$(cd "$(dirname "$0")" && pwd)
ob() { "$here/../../bin/omabox" "$@"; }
B=$1 O=$2 G=$3
mkdir -p "$O"
W=title:^Probe$
# shot NAME [ARGS]: a shot (the probe window's unless ARGS), the screen settled first.
shot() {
  local n=$1; shift; [ $# -gt 0 ] || set -- -w "$W"
  ob wait -b "$B" still --quiet 300ms >/dev/null 2>&1 || true
  ob shot -b "$B" "$@" -o "$O/$n.png" >/dev/null 2>&1
  # A screen shot shows the pointer: where it was, for measure.sh to leave out (as wait does).
  [ "$1" = -w ] || ob hyprctl -b "$B" cursorpos | tr -d , > "$O/$n.cursor"
}
pair() { "$here/measure.sh" "$1" "$O/$1-0.png" "$O/$1-1.png" "${MARGIN:-16}"; }
park() { ob pointer -b "$B" -- move 1900 1060 >/dev/null 2>&1; }

probe() {
  ob run -b "$B" -d -q --replace --wait -- /usr/bin/python3 "$here/probe-app.py" >/dev/null 2>&1
  park
  # A header-bar menu opens (a popover).
  shot menu-0; ob click -b "$B" -w "$W" --wait 1781 22 >/dev/null 2>&1; shot menu-1; pair menu
  ob keys -b "$B" Escape >/dev/null 2>&1
  # A switch flips; a check box ticks.
  shot toggle-0; ob click -b "$B" -w "$W" --wait 321 75 >/dev/null 2>&1; shot toggle-1; pair toggle
  shot check-0; ob click -b "$B" -w "$W" --wait 370 75 >/dev/null 2>&1; shot check-1; pair check
  # Typing into a field: its focus ring, the text, the caret; then more text into it.
  shot type-0; ob click -b "$B" -w "$W" 600 119 >/dev/null 2>&1
  ob keys -b "$B" --wait -t "invoice 2026" >/dev/null 2>&1; shot type-1; pair type
  shot typemore-0; ob keys -b "$B" --wait -t " march" >/dev/null 2>&1; shot typemore-1; pair typemore
  # A progress bar fills a step (the button's pressed state too).
  shot fill-0; ob click -b "$B" -w "$W" --wait 1804 167 >/dev/null 2>&1; shot fill-1; pair fill
  # A drag's ghost, mid-drag: a drag icon surface, only in a screen shot (the pointer moves too).
  local ax ay; read -r ax ay < <(ob windows -b "$B" --json | jq -r '.[] | select(.title == "Probe") | "\(.at[0]) \(.at[1])"')
  park; shot drag-0 -g "0,0 1920x1080"
  ob drag -b "$B" --shot "$O/drag-1.png" --hold 400ms $((ax + 367)) $((ay + 258)) $((ax + 560)) $((ay + 420)) >/dev/null 2>&1
  ob hyprctl -b "$B" cursorpos | tr -d , > "$O/drag-1.cursor"
  pair drag
  # ...the window shot mid-drag has no ghost; dropped, the target's label changes.
  shot drop-0; ob drag -b "$B" --hold 400ms $((ax + 367)) $((ay + 258)) $((ax + 677)) $((ay + 258)) >/dev/null 2>&1; shot drop-1; pair drop
  # Three actions since the last shot: one box spans them all.
  shot seq-0
  ob click -b "$B" -w "$W" --wait 321 75 >/dev/null 2>&1
  ob click -b "$B" -w "$W" --wait 1804 167 >/dev/null 2>&1
  ob click -b "$B" -w "$W" --wait 30 293 >/dev/null 2>&1
  shot seq-1; pair seq
  # Nothing done, a field focused: the caret blinks (GTK: 1.2 s a cycle, for 10 s after the last key).
  ob click -b "$B" -w "$W" 900 119 >/dev/null 2>&1
  ob shot -b "$B" -w "$W" --burst 6 --every 250ms --diff -o "$O/caret" 2>/dev/null | sed 's/^/caret: /'
}

nautilus() {
  ob run -b "$B" -d -q --replace --wait -- nautilus --new-window /usr/share/omarchy >/dev/null 2>&1
  park; sleep 1
  # A file's context menu (the status bar's "selected" toast too).
  shot nmenu-0 -w nautilus; ob click -b "$B" -w nautilus --wait 808 120 right >/dev/null 2>&1; shot nmenu-1 -w nautilus; pair nmenu
  ob keys -b "$B" Escape >/dev/null 2>&1
  # Another file selected.
  shot nsel-0 -w nautilus; ob click -b "$B" -w nautilus --wait 1282 120 >/dev/null 2>&1; shot nsel-1 -w nautilus; pair nsel
  # The main menu.
  shot nmain-0 -w nautilus; ob click -b "$B" -w nautilus --wait 216 22 >/dev/null 2>&1; shot nmain-1 -w nautilus; pair nmain
  ob keys -b "$B" Escape >/dev/null 2>&1
  # A file dragged: the icon ghost, mid-drag (screen).
  local ax ay; read -r ax ay < <(ob windows -b "$B" --json | jq -r '.[] | select(.class == "org.gnome.Nautilus") | "\(.at[0]) \(.at[1])"')
  park; shot ndrag-0 -g "0,0 1920x1080"
  ob drag -b "$B" --shot "$O/ndrag-1.png" --hold 500ms $((ax + 808)) $((ay + 256)) $((ax + 1100)) $((ay + 600)) >/dev/null 2>&1
  ob hyprctl -b "$B" cursorpos | tr -d , > "$O/ndrag-1.cursor"
  pair ndrag
  ob keys -b "$B" Escape >/dev/null 2>&1
  # Grid to list view: most of the window.
  shot nview-0 -w nautilus; ob keys -b "$B" -w nautilus ctrl+1 >/dev/null 2>&1; sleep 1; shot nview-1 -w nautilus; pair nview
}

shell() {
  park
  # A notification pops up (screen).
  shot notify-0 -g "0,0 1920x1080"; ob run -b "$B" -- notify-send "Build finished" "3 tests failed" >/dev/null 2>&1
  sleep 1; shot notify-1 -g "0,0 1920x1080"; pair notify
  # The launcher opens (screen).
  sleep 6; shot launch-0 -g "0,0 1920x1080"; ob keys -b "$B" super+space >/dev/null 2>&1; sleep 1; shot launch-1 -g "0,0 1920x1080"; pair launch
  # ...and a query typed into it.
  shot query-0 -g "0,0 1920x1080"; ob keys -b "$B" --wait -t "files" >/dev/null 2>&1; shot query-1 -g "0,0 1920x1080"; pair query
  ob keys -b "$B" Escape >/dev/null 2>&1
}

"$G"
