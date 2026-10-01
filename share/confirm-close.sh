#!/usr/bin/env bash
# An interactive box's window was closed with confirm-close on (NOTES finding 70). Hyprland's Lua
# (hyprland.lua) runs this instead of exiting: open a new window, then ask. The new window lands on
# the user's current workspace, where they just closed the old one. Closing it again while this asks
# is a yes (hyprland.lua exits on a close while omabox.close-asking exists); "Keep it running" or
# Escape is a no.
set -uo pipefail

# Ending on purpose: omabox.closed tells the box's reaper to clear it (finding 71).
quit() { echo 1 > "$XDG_RUNTIME_DIR/omabox.closed"; hyprctl dispatch 'hl.dsp.exit()' >/dev/null 2>&1; exit 0; }
keep() { rm -f "$XDG_RUNTIME_DIR/omabox.close-asking"; exit 0; }

# A host that cannot give a new window (the session is ending): nothing to ask on, so end.
hyprctl output create wayland >/dev/null 2>&1 || quit
# The host's exec rule (render_unfocused) went with the old window: this one is not drawn while
# hidden. omabox.reopened lets `shot` say so (finding 90).
echo 1 > "$XDG_RUNTIME_DIR/omabox.reopened"
# Hyprland's FALLBACK stands in while it has no output: not a window.
monitors() { [ "$(hyprctl -j monitors 2>/dev/null | jq '[.[] | select(.name != "FALLBACK")] | length')" -gt 0 ] 2>/dev/null; }
for _ in $(seq 50); do
  monitors && break
  sleep 0.1
done
# No window came (an aquamarine without the fix for nested Wayland outputs, finding 125; omabox
# refuses confirm-close there, but the library can change under a running box): the user closed the
# only window, so end the box rather than leave it running with none.
monitors || quit

msg="Close the window again to shut this box down"
hyprctl notify 1 8000 0 "$msg" >/dev/null 2>&1 || true

# The Omarchy shell's menu, when the box runs it (not with up --no-shell): the notification alone
# then, and the next close ends the box.
command -v omarchy-menu-select >/dev/null && [ "${OMABOX_SHELL:-1}" != 0 ] || exit 0
sleep 0.5   # the shell re-lays out onto the new output first
choice=$(omarchy-menu-select "Shut down this box?" "Shut down" "Keep it running" 2>/dev/null) || keep
[ "$choice" = "Shut down" ] && quit
keep
