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
# The Omarchy shell only, by its recorded pid: a quickshell a project runs in the box is not ours to
# kill. Waited for 5 s at most, then SIGKILLed.
pidfile=$XDG_RUNTIME_DIR/omabox-shell.pid
# Asked to quit first: Omarchy's launcher, which runs the shell here too (#151), starts it again when
# it dies of a signal, not when it quits (finding 131). One that will not quit: its launcher is stopped
# first (it then stops the shell), or it would start another next to the new one.
if old=$(cat "$pidfile" 2>/dev/null) && [ "$(cat "/proc/$old/comm" 2>/dev/null)" = quickshell ]; then
  if ! timeout 5 /usr/bin/quickshell kill --pid "$old" >/dev/null 2>&1; then
    parent=$(sed 's/.*) . \([0-9]*\).*/\1/' "/proc/$old/stat" 2>/dev/null) || parent=""
    if [ -n "$parent" ] && grep -qa omarchy-launch-shell "/proc/$parent/cmdline" 2>/dev/null; then kill "$parent"
    else kill "$old"; fi
  fi
  for _ in $(seq 100); do kill -0 "$old" 2>/dev/null || break; sleep 0.05; done
  kill -KILL "$old" 2>/dev/null
fi
# A fresh log each start, before the new pid is there (`restart-shell` reads this log for a crash once
# it is), written in append mode: a line the box's Hyprland adds (the crash dialog it closes, finding
# 183) is not overwritten by the shell's next one.
: > "$HOME/shell.log"
# Run as Omarchy runs it (#151): omarchy-launch-shell sets the file watcher off itself and starts the
# shell again when it exits non-zero, which Quickshell does not do within 10 s of its start; a box was
# left with no bar where a user's desktop gets one back. Its systemd-cat, the box's stand-in, sends the
# shell's output here and writes each new shell's pid (it execs quickshell), followed across relaunches.
rm -f "$pidfile"
launcher=$OMARCHY_PATH/bin/omarchy-launch-shell
[ ! -x "$launcher" ] || exec "$launcher" >> "$HOME/shell.log" 2>&1
# An Omarchy tree without it (`up --omarchy` of an older one): the shell itself, as before.
echo $$ > "$pidfile"
QS_DISABLE_FILE_WATCHER=1 QS_NO_RELOAD_POPUP=1 exec /usr/bin/quickshell -n -p "$OMARCHY_PATH/shell" >> "$HOME/shell.log" 2>&1
