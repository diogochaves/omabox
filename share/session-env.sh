#!/usr/bin/env bash
# Gives the box's session bus, and its user manager with `up --systemd`, the compositor's environment,
# as Omarchy's autostart does: D-Bus-activated apps (Files, the portals) and units that open windows
# need WAYLAND_DISPLAY, or they start without it and die. Run by shell.sh before a shell starts, and by
# hyprland.lua itself in a box with no shell (`up --no-shell`, a plain box without --shell), which got
# none before (finding 663). Launched by Hyprland, so it has the session env.
set -uo pipefail
dbus-update-activation-environment --all
# A unit bus-activated before this (the portal, in an interactive box) failed without it and may have
# hit its start limit: clear that, so the next request starts it with the session's environment.
if [ -S "$XDG_RUNTIME_DIR/systemd/private" ]; then
  dbus-update-activation-environment --systemd --all
  systemctl --user reset-failed
fi
exit 0
