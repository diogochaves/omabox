export XDG_RUNTIME_DIR=/run/user/1000 HOME=/home/sbx
WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 WLR_RENDER_DRM_DEVICE=/dev/dri/renderD128 sway -c /spike/sway.conf > /home/sbx/sway.log 2>&1 &
for i in $(seq 50); do [ -S $XDG_RUNTIME_DIR/wayland-1 ] && break; sleep 0.1; done
grep -iE 'renderer|gles|vulkan|pixman' /home/sbx/sway.log | head -3
t0=$(date +%s.%N)
WAYLAND_DISPLAY=wayland-1 HYPRLAND_NO_SD_NOTIFY=1 HYPRLAND_NO_CRASHREPORTER=1 AQ_DRM_DEVICES=/dev/dri/renderD128 Hyprland --config /spike/hyprland.lua > /home/sbx/hl.log 2>&1 &
for i in $(seq 100); do ls $XDG_RUNTIME_DIR/hypr/*/.socket.sock >/dev/null 2>&1 && break; sleep 0.1; done
sleep 2; echo "hl up in $(echo "$(date +%s.%N)-$t0" | bc)s"
export HYPRLAND_INSTANCE_SIGNATURE=$(ls $XDG_RUNTIME_DIR/hypr | head -1)
ls $XDG_RUNTIME_DIR
hyprctl monitors | head -4
WAYLAND_DISPLAY=wayland-1 grim /home/sbx/outer.png && echo OUTER_GRIM_OK
WAYLAND_DISPLAY=wayland-2 grim /home/sbx/inner.png && echo INNER_GRIM_OK
ps -o rss=,comm= -p $(pgrep -d, -x sway),$(pgrep -d, -x Hyprland)
kill %2 %1; wait; grep -iE 'error|fail|crit' /home/sbx/hl.log | grep -v '^\s*$' | head -15
