export XDG_RUNTIME_DIR=/run/user/1000 HOME=/home/sbx
now(){ date +%s%N; }
parent=$1
export WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 WLR_RENDER_DRM_DEVICE=/dev/dri/renderD128
HL="env -u WLR_BACKENDS HYPRLAND_NO_SD_NOTIFY=1 HYPRLAND_NO_CRASHREPORTER=1 Hyprland --config /spike/hyprland.lua"
t0=$(now)
case $parent in
  labwc) mkdir -p /home/sbx/.config/labwc; labwc -s "sh -c '$HL > /home/sbx/hl.log 2>&1'" > /home/sbx/parent.log 2>&1 & ;;
  cage)  cage -- sh -c "$HL > /home/sbx/hl.log 2>&1" > /home/sbx/parent.log 2>&1 & ;;
esac
for i in $(seq 150); do ls $XDG_RUNTIME_DIR/hypr/*/.socket.sock >/dev/null 2>&1 && break; sleep 0.1; done
echo "[$parent] hyprland socket $(( ($(now)-t0)/1000000 ))ms"; sleep 2
export HYPRLAND_INSTANCE_SIGNATURE=$(ls $XDG_RUNTIME_DIR/hypr | head -1)
ls $XDG_RUNTIME_DIR | tr '\n' ' '; echo
hyprctl monitors | head -3
inner=$(cd $XDG_RUNTIME_DIR; ls | grep -E '^wayland-[0-9]+$' | sort | tail -1); echo inner=$inner
WAYLAND_DISPLAY=$inner grim /home/sbx/inner-$parent.png && echo INNER_GRIM_OK
for p in labwc cage Hyprland; do r=$(ps -o rss= -C $p | head -1); [ -n "$r" ] && echo "$p rss_kb=$r"; done
grep -iE 'invalid version|terminate|what\(\)|CRIT' /home/sbx/hl.log | head -5
