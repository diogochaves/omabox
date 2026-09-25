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
# The Omarchy shell only, by the pid this script recorded (exec keeps it): a quickshell a project runs
# in the box is not ours to kill. Waited for 5 s at most, then SIGKILLed.
pidfile=$XDG_RUNTIME_DIR/omabox-shell.pid
if old=$(cat "$pidfile" 2>/dev/null) && [ "$(cat "/proc/$old/comm" 2>/dev/null)" = quickshell ]; then
  kill "$old"
  for _ in $(seq 100); do kill -0 "$old" 2>/dev/null || break; sleep 0.05; done
  kill -KILL "$old" 2>/dev/null
fi
echo $$ > "$pidfile"
QS_DISABLE_FILE_WATCHER=1 QS_NO_RELOAD_POPUP=1 exec /usr/bin/quickshell -n -p "$OMARCHY_PATH/shell" > "$HOME/shell.log" 2>&1
