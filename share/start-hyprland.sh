#!/usr/bin/env bash
set -euo pipefail

# The X display locks session.sh held while labwc started, so it made no Xwayland (#211): the box's X
# displays are free again, for Hyprland's Xwayland (`up --xwayland`) or an X server run in the box.
rm -f /tmp/.X{0..32}-lock

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
# An interactive box's monitors in its layout (hyprland.lua writes the file), for omabox's aquamarine
# build to put the pointer in a monitor's window on that monitor (finding 238); others ignore it.
[ "${OMABOX_INTERACTIVE:-0}" != 1 ] || lib+=(AQ_WAYLAND_LAYOUT="$XDG_RUNTIME_DIR/omabox.layout")
# And a class of its own for each window `monitor add` makes (finding 429): omabox writes
# omabox.appid.NAME before it asks for output NAME, and its host rule matches that class alone. Any
# other window keeps class aquamarine. omabox.appid-files says this Hyprland has it: `monitor add`
# refuses a box without (one an older omabox started, whose start-hyprland.sh set no such env, even
# on a library with +appid: box.json's version alone cannot tell).
if [ "${OMABOX_INTERACTIVE:-0}" = 1 ]; then
  lib+=(AQ_WAYLAND_APP_ID_FILES="$XDG_RUNTIME_DIR/omabox.appid.")
  : > "$XDG_RUNTIME_DIR/omabox.appid-files"
fi
# The box's own copy of omabox's config (#140): share/ is the checkout's or package's, live, and a
# change to it (a pull, an upgrade) reloaded every running box, dropping what agents had added in Lua.
# Autoreload is off too (hyprland.lua); `omabox reload` refreshes the copy and reloads it. Written as
# reload writes it (cat, not cp), so it is the box's to change whatever share/'s mode: cp kept a
# read-only install's 0444, and a write to the copy in the box was refused (finding 528).
cfg=$XDG_RUNTIME_DIR/omabox-hyprland.lua
cat /opt/omabox/share/hyprland.lua > "$cfg.new" && mv -f "$cfg.new" "$cfg"
# The version this Hyprland is, as a desk's HOME has it after its first session (finding 626): with
# none, the box's Hyprland wrote 0.0.0, and the next Hyprland started in the box (a test's nested
# compositor, one run by hand) took itself for an upgrade and opened Hyprland's update news over its
# first frames. A HOME that has the file (a save's) keeps its own.
seen=${XDG_DATA_HOME:-$HOME/.local/share}/hyprland/lastVersion
if [ ! -e "$seen" ]; then
  ver=$("${OMABOX_HYPRLAND:-/usr/bin/Hyprland}" --version 2>/dev/null | sed -n '1s/^Hyprland \([0-9][0-9.]*\) .*/\1/p') || ver=""
  [ -z "$ver" ] || { mkdir -p "${seen%/*}" && printf '%s' "$ver" > "$seen"; } || true
fi
exec env -u WLR_BACKENDS -u WLR_LIBINPUT_NO_DEVICES -u WLR_HEADLESS_OUTPUTS -u WLR_RENDER_DRM_DEVICE -u WLR_SCENE_DISABLE_VISIBILITY -u DISPLAY -u OMABOX_HYPRLAND \
  OMARCHY_PATH="${OMABOX_OMARCHY:-/usr/share/omarchy}" "${lib[@]}" HYPRLAND_NO_SD_NOTIFY=1 HYPRLAND_NO_CRASHREPORTER=1 \
  "${OMABOX_HYPRLAND:-/usr/bin/Hyprland}" --config "$cfg"
