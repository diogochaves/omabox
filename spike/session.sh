# Runs inside the sandbox, under a private D-Bus.
echo "$DBUS_SESSION_BUS_ADDRESS" > /run/user/1000/dbus-address
mkdir -p /home/sbx/.config/labwc
export WLR_BACKENDS=headless WLR_HEADLESS_OUTPUTS=1 WLR_LIBINPUT_NO_DEVICES=1 WLR_RENDER_DRM_DEVICE=/dev/dri/renderD128
exec labwc -s "env -u WLR_BACKENDS -u WLR_LIBINPUT_NO_DEVICES -u WLR_HEADLESS_OUTPUTS -u WLR_RENDER_DRM_DEVICE OMARCHY_PATH=/usr/share/omarchy LD_LIBRARY_PATH=/opt/omabox/lib HYPRLAND_NO_SD_NOTIFY=1 HYPRLAND_NO_CRASHREPORTER=1 Hyprland --config /spike/hyprland.lua" > /home/sbx/labwc.log 2>&1
