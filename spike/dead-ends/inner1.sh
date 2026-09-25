export XDG_RUNTIME_DIR=/run/user/1000 HOME=/home/sbx WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 WLR_RENDER_DRM_DEVICE=/dev/dri/renderD128
sway -c /spike/sway.conf > /home/sbx/sway.log 2>&1 &
for i in $(seq 50); do ls $XDG_RUNTIME_DIR/wayland-* >/dev/null 2>&1 && break; sleep 0.1; done
ls $XDG_RUNTIME_DIR; export WAYLAND_DISPLAY=$(cd $XDG_RUNTIME_DIR; ls | grep -E '^wayland-[0-9]+$' | head -1)
SWAYSOCK=$(ls $XDG_RUNTIME_DIR/sway-ipc.* | head -1) swaymsg -t get_outputs | head -5
grim /home/sbx/sway.png && echo GRIM_OK
kill %1; wait; tail -5 /home/sbx/sway.log
