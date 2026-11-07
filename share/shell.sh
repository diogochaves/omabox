#!/usr/bin/env bash
# Starts (or restarts) the box's shell inside a box: Omarchy's, a plain box's `up --shell CMD`, or a
# desk box's own (#321).
# Launched by Hyprland, so it has the session env.
# The file watcher stays off so a screenshot never catches a half-reloaded shell; `omabox restart-shell`
# is the explicit reload.
# What Omarchy's autostart does: give the session bus (and the user manager's) the compositor's env.
"$(dirname "${BASH_SOURCE[0]}")/session-env.sh"
pidfile=$XDG_RUNTIME_DIR/omabox-shell.pid
# `up --desktop plain --shell CMD` (#247): a shell of the user's, not Omarchy's. CMD runs in this
# process, so its pid is the one recorded, with the mark and token files `up` and `restart-shell` read
# as for Omarchy's, and under a slot of its own (#438): OMABOX_SHELL_SLOT in its environment, which
# all it starts inherits, whatever it does with its session or its parent: a launcher that starts the
# shell and exits (`caelestia shell -d`; `qs -d` calls setsid) leaves its shell in the slot, and omabox
# counts the shell gone when nothing of the slot is left (the slot is in omabox-shell.slot). The last
# start's slot and the tree of each of its processes (`dms run` starts quickshell) are stopped, TERM
# then KILL after 5 s. `up --quickshell`'s build is `qs` and `quickshell` on PATH.
cmd=${OMABOX_SHELL_CMD:-}
# A desk box (#321, finding 703) runs the shell its user's config starts in the same slot: the command
# share/desk-shell.lua took from the config (or its guess), in omabox-shell.cmd. Started from there by
# desk-shell.lua (`take`) and by `restart-shell` (its token); never by hyprland.lua's own start, which
# has no word: the slot is the config's to fill, and starting it twice would stop the one just taken.
# Nothing in the file (the config started none): nothing to start. `up --shell CMD` overrides it.
if [ "${OMABOX_DESKTOP:-}" = desk ] && [ -z "$cmd" ]; then
  [ -z "${1:-}" ] || cmd=$(cat "$XDG_RUNTIME_DIR/omabox-shell.cmd" 2>/dev/null) || cmd=""
  [ -n "$cmd" ] || exit 0
fi
if [ -n "$cmd" ]; then
  slotfile=$XDG_RUNTIME_DIR/omabox-shell.slot
  tree() { local c; for c in $(pgrep -P "$1"); do tree "$c"; done; echo "$1"; }
  # The processes of slot $1, crash dialogs and all (this script is none: Hyprland started it).
  slot_pids() {
    [[ $1 =~ ^[A-Za-z0-9._-]+$ ]] || return 0
    grep -lsFxz -e "OMABOX_SHELL_SLOT=$1" /proc/[0-9]*/environ | sed -n 's|^/proc/\([0-9]*\)/environ$|\1|p'
  }
  old=$(cat "$pidfile" 2>/dev/null) || old=""
  oldslot=$(cat "$slotfile" 2>/dev/null) || oldslot=""
  pids=""
  [[ ! $old =~ ^[0-9]+$ ]] || [ ! -d "/proc/$old" ] || pids=$(tree "$old")
  for p in $(slot_pids "$oldslot"); do pids="$pids $(tree "$p")"; done
  # (Each asked once; what the slot started meanwhile too, a launcher stopped as it forked.)
  alive=$pids sent=" "
  deadline=$((SECONDS + 5))
  while [ -n "$alive" ] && [ $SECONDS -lt $deadline ]; do
    for p in $alive; do [[ $sent == *" $p "* ]] || { kill "$p" 2>/dev/null; sent="$sent$p "; }; done
    sleep 0.05
    alive=""; for p in $pids $(slot_pids "$oldslot"); do [ -d "/proc/$p" ] && alive="$alive $p"; done
  done
  # shellcheck disable=SC2086 # pid lists
  [ -z "$alive" ] || kill -KILL $alive 2>/dev/null
  rm -f "$pidfile" "$slotfile"
  : > "$HOME/shell.log"
  sh -c 'echo $$' > "$XDG_RUNTIME_DIR/omabox-shell.mark"
  echo "${1:-}" > "$XDG_RUNTIME_DIR/omabox-shell.start"
  # (This process's pid and the time in µs: no later start's, and no old one's whose pid came again.)
  slot=$$-${EPOCHREALTIME//[!0-9]/}
  echo "$slot" > "$slotfile"
  echo $$ > "$pidfile"
  OMABOX_SHELL_SLOT=$slot exec sh -c "$cmd" >> "$HOME/shell.log" 2>&1
fi
# The Omarchy shell only: its recorded pid and any other instance of its config, which the new one's
# `-n` would meet ("An instance of this configuration is already running", finding 248). A quickshell
# a project runs in the box on a config of its own is not ours to kill.
config=$OMARCHY_PATH/shell
# The box's quickshell: `up --quickshell`'s build when share/session.sh linked one (finding 358).
qs=$XDG_RUNTIME_DIR/omabox-quickshell/quickshell
[ -x "$qs" ] || qs=/usr/bin/quickshell
shells() {
  { cat "$pidfile" 2>/dev/null; echo
    "$qs" list -j -p "$config" --any-display 2>/dev/null | jq -r '.[].pid' 2>/dev/null
  } | sort -un | while read -r p; do
    st=$(cat "/proc/$p/stat" 2>/dev/null) || continue
    case $st in "$p (quickshell) "[!Z]*) echo "$p" ;; esac   # (not one exited and not reaped yet)
  done
}
launchers() { pgrep -f '^[^ ]*bash [^ ]*/omarchy-launch-shell$'; }
# Asked to quit first: Omarchy's launcher, which runs the shell here too (#151), starts it again when
# it dies of a signal, not when it quits (finding 131). One that will not quit: its launcher is stopped
# first (it then stops the shell), or it would start another next to the new one.
for old in $(shells); do
  timeout 5 "$qs" kill --pid "$old" >/dev/null 2>&1 && continue
  parent=$(sed 's/.*) . \([0-9]*\).*/\1/' "/proc/$old/stat" 2>/dev/null) || parent=""
  if [ -n "$parent" ] && grep -qa omarchy-launch-shell "/proc/$parent/cmdline" 2>/dev/null; then kill "$parent"
  else kill "$old"; fi
done
# Then until no launcher and no shell is left, 5 s at most, then SIGKILLed. A launcher still there is
# one whose shell died of a signal (a crash, a kill) as the edits landed: it starts the shell again a
# second later, next to the new one, which then exits as a duplicate, or is the one met (248). Stopped,
# its trap ends it at its next turn without starting one.
deadline=$((SECONDS + 5))
while [ $SECONDS -lt $deadline ]; do
  l=$(launchers) s=$(shells)
  [ -n "$l$s" ] || break
  # shellcheck disable=SC2086 # pid lists
  if [ -n "$l" ]; then kill $l; else kill $s; fi 2>/dev/null
  sleep 0.05
done
# shellcheck disable=SC2046 # pid lists
kill -KILL $(launchers) $(shells) 2>/dev/null
# A fresh log each start, before the new pid is there (`restart-shell` reads this log for a crash once
# it is), written in append mode: a line the box's Hyprland adds (the crash dialog it closes, finding
# 183) is not overwritten by the shell's next one.
: > "$HOME/shell.log"
# Run as Omarchy runs it (#151): omarchy-launch-shell sets the file watcher off itself and starts the
# shell again when it exits non-zero, which Quickshell does not do within 10 s of its start; a box was
# left with no bar where a user's desktop gets one back. Its systemd-cat, the box's stand-in, sends the
# shell's output here and writes each new shell's pid (it execs quickshell), followed across relaunches.
rm -f "$pidfile"
# The box's newest pid now, for `restart-shell`: everything of this start comes after it (a pid
# namespace hands pids out in order), and a crash below it is an old shell's, stopped as it loaded (an
# old launcher's relaunch: a SEGV when stopped mid-load on a slow GPU), not this start failing (255).
sh -c 'echo $$' > "$XDG_RUNTIME_DIR/omabox-shell.mark"
# `restart-shell` passes a token and waits for it here: a pid written after it is this start's, not
# one an old launcher's relaunch wrote while the old shell was being stopped (finding 248).
echo "${1:-}" > "$XDG_RUNTIME_DIR/omabox-shell.start"
# Omarchy 4.5's boot intro (#291, finding 415): a shell that finds no mark of this session in
# background-intro.session-id takes its start for a new login and covers every screen, the bar too,
# in black on the Overlay layer until omarchy-theme-bg-boot-intro returns: up to 5 s on a theme with
# intros (it waits for an OWE daemon no box runs), 10 s at most. The first shot after `up` was that
# black. A box's start is no login anyone watches: marked as had, as a shell restart finds it, the
# shell shows the desktop at once. (The same id the shell and the intro script read.)
mkdir -p "$HOME/.local/state/omarchy" 2>/dev/null &&
  printf '%s\n' "${OMARCHY_SESSION_ID:-$HYPRLAND_INSTANCE_SIGNATURE}" > "$HOME/.local/state/omarchy/background-intro.session-id" 2>/dev/null
launcher=$OMARCHY_PATH/bin/omarchy-launch-shell
[ ! -x "$launcher" ] || exec "$launcher" >> "$HOME/shell.log" 2>&1
# An Omarchy tree without it (`up --omarchy` of an older one): the shell itself, as before.
echo $$ > "$pidfile"
QS_DISABLE_FILE_WATCHER=1 QS_NO_RELOAD_POPUP=1 exec "$qs" --log-times -n -p "$OMARCHY_PATH/shell" >> "$HOME/shell.log" 2>&1
