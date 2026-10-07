#!/usr/bin/env bash
set -euo pipefail

if [ "${OMABOX_WAYLAND_SCREEN:-0}" = 1 ]; then
  printf '%s\n' "$WAYLAND_DISPLAY" > "$XDG_RUNTIME_DIR/omabox-parent-display"
  /usr/bin/wlr-randr --output HEADLESS-1 --custom-mode "${OMABOX_SIZE}Hz"
fi

# OMABOX_HYPRLAND: `omabox up --hyprland PATH`, a build of the user's (NOTES finding 116), mounted
# read-only at its own path; hyprctl and the rest stay the installed ones. OMABOX_OMARCHY: `up
# --omarchy DIR`'s tree, as OMARCHY_PATH (finding 135).
# /opt/omabox/lib: a private aquamarine with the fix for nested Wayland outputs, when omabox mounted
# one (finding 125); without it, the system's.
lib=(); [ ! -d /opt/omabox/lib ] || lib=(LD_LIBRARY_PATH=/opt/omabox/lib)
# The box's own copy of omabox's config (#140): share/ is the checkout's or package's, live, and a
# change to it (a pull, an upgrade) reloaded every running box, dropping what agents had added in Lua.
# Autoreload is off too (hyprland.lua); `omabox reload` refreshes the copy and reloads it.
cfg=$XDG_RUNTIME_DIR/omabox-hyprland.lua
cp /opt/omabox/share/hyprland.lua "$cfg.new" && mv -f "$cfg.new" "$cfg"
exec env -u WLR_BACKENDS -u WLR_LIBINPUT_NO_DEVICES -u WLR_HEADLESS_OUTPUTS -u WLR_RENDER_DRM_DEVICE -u WLR_SCENE_DISABLE_VISIBILITY -u DISPLAY -u OMABOX_HYPRLAND \
  OMARCHY_PATH="${OMABOX_OMARCHY:-/usr/share/omarchy}" "${lib[@]}" HYPRLAND_NO_SD_NOTIFY=1 HYPRLAND_NO_CRASHREPORTER=1 \
  "${OMABOX_HYPRLAND:-/usr/bin/Hyprland}" --config "$cfg"
