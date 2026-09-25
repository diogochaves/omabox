export XDG_RUNTIME_DIR=/run/user/1000 HOME=/home/sbx
now(){ date +%s%N; }
t0=$(now)
weston --backend=headless --renderer=gl --width=1920 --height=1080 --socket=wl-outer --config=/spike/weston.ini --idle-time=0 > /home/sbx/weston.log 2>&1 &
for i in $(seq 100); do [ -S $XDG_RUNTIME_DIR/wl-outer ] && break; sleep 0.1; done
echo "weston up $(( ($(now)-t0)/1000000 ))ms"
WAYLAND_DISPLAY=wl-outer HYPRLAND_NO_SD_NOTIFY=1 HYPRLAND_NO_CRASHREPORTER=1 Hyprland --config /spike/hyprland.lua > /home/sbx/hl.log 2>&1 &
for i in $(seq 150); do ls $XDG_RUNTIME_DIR/hypr/*/.socket.sock >/dev/null 2>&1 && break; sleep 0.1; done
echo "hyprland socket $(( ($(now)-t0)/1000000 ))ms"; sleep 2
export HYPRLAND_INSTANCE_SIGNATURE=$(ls $XDG_RUNTIME_DIR/hypr | head -1)
ls $XDG_RUNTIME_DIR
hyprctl monitors | head -3
inner=$(cd $XDG_RUNTIME_DIR; ls | grep -E '^wayland-[0-9]+$' | head -1); echo inner=$inner
WAYLAND_DISPLAY=$inner grim /home/sbx/inner.png && echo INNER_GRIM_OK
for p in weston Hyprland; do echo "$p rss_kb=$(ps -o rss= -C $p | head -1)"; done
kill %2 %1; wait
grep -iE 'renderer|gl_renderer|EGL.*vendor|GL_RENDERER' /home/sbx/weston.log | head -3
grep -iE 'error|fail|crit|terminate|what' /home/sbx/hl.log | head -10
