#!/usr/bin/env bash
set -euo pipefail

if [ "${OMABOX_WAYLAND_SCREEN:-0}" = 1 ]; then
  printf '%s\n' "$WAYLAND_DISPLAY" > "$XDG_RUNTIME_DIR/omabox-parent-display"
  /usr/bin/wlr-randr --output HEADLESS-1 --custom-mode "${OMABOX_SIZE}Hz"
fi

exec env -u WLR_BACKENDS -u WLR_LIBINPUT_NO_DEVICES -u WLR_HEADLESS_OUTPUTS -u WLR_RENDER_DRM_DEVICE -u DISPLAY \
  OMARCHY_PATH=/usr/share/omarchy LD_LIBRARY_PATH=/opt/omabox/lib HYPRLAND_NO_SD_NOTIFY=1 HYPRLAND_NO_CRASHREPORTER=1 \
  /usr/bin/Hyprland --config /opt/omabox/share/hyprland.lua
