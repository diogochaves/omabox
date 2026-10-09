#!/usr/bin/env bash
# Starts (or restarts) the Omarchy shell inside a box. Launched by Hyprland, so it has the session env.
# The file watcher stays off so a screenshot never catches a half-reloaded shell; `omabox restart-shell`
# is the explicit reload.
# What Omarchy's autostart does: give the session bus the compositor's env, or D-Bus-activated apps
# (Files, the portals) start without WAYLAND_DISPLAY and die.
dbus-update-activation-environment --all
# And the user manager's, with `up --systemd`: units that open windows need WAYLAND_DISPLAY too. A
# unit bus-activated before this (the portal, in an interactive box) failed without it and may have hit
# its start limit: clear that, so the next request starts it with the session's environment.
if [ -S "$XDG_RUNTIME_DIR/systemd/private" ]; then
  dbus-update-activation-environment --systemd --all
  systemctl --user reset-failed
fi
# The Omarchy shell only: its recorded pid and any other instance of its config, which the new one's
# `-n` would meet ("An instance of this configuration is already running", finding 248). A quickshell
# a project runs in the box on a config of its own is not ours to kill.
pidfile=$XDG_RUNTIME_DIR/omabox-shell.pid
config=$OMARCHY_PATH/shell
shells() {
  { cat "$pidfile" 2>/dev/null; echo
    /usr/bin/quickshell list -j -p "$config" --any-display 2>/dev/null | jq -r '.[].pid' 2>/dev/null
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
  timeout 5 /usr/bin/quickshell kill --pid "$old" >/dev/null 2>&1 && continue
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
# `restart-shell` passes a token and waits for it here: a pid written after it is this start's, not
# one an old launcher's relaunch wrote while the old shell was being stopped (finding 248).
echo "${1:-}" > "$XDG_RUNTIME_DIR/omabox-shell.start"
launcher=$OMARCHY_PATH/bin/omarchy-launch-shell
[ ! -x "$launcher" ] || exec "$launcher" >> "$HOME/shell.log" 2>&1
# An Omarchy tree without it (`up --omarchy` of an older one): the shell itself, as before.
echo $$ > "$pidfile"
QS_DISABLE_FILE_WATCHER=1 QS_NO_RELOAD_POPUP=1 exec /usr/bin/quickshell -n -p "$OMARCHY_PATH/shell" >> "$HOME/shell.log" 2>&1
