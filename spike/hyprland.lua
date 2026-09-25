-- omabox nested Hyprland: real Omarchy defaults, minus Omarchy's session autostart.
dofile((os.getenv("OMARCHY_PATH") or "/usr/share/omarchy") .. "/default/hypr/bootstrap.lua")
package.loaded["default.hypr.autostart"] = true -- skip: systemd/dbus env import, first-run, monitor-watch, udiskie
require("default.hypr.omarchy")

-- WAYLAND-1 is only the bootstrap window inside labwc; the real screen is a headless output.
hl.monitor({ output = "WAYLAND-1", disabled = true })
hl.monitor({ output = "HEADLESS-2", mode = (os.getenv("OMABOX_MODE") or "1920x1080") .. "@60", position = "0x0", scale = 1 })
hl.config({ debug = { vfr = true, disable_logs = false }, misc = { disable_watchdog_warning = true } })

hl.on("hyprland.start", function()
  hl.exec_cmd("hyprctl output create headless HEADLESS-2")
  hl.exec_cmd("wayvnc -d -r -R -o HEADLESS-2 127.0.0.1 ${OMABOX_VNC_PORT:-5909} > $HOME/wayvnc.log 2>&1")
  hl.exec_cmd("QS_DISABLE_FILE_WATCHER=1 QS_NO_RELOAD_POPUP=1 quickshell -n -p $OMARCHY_PATH/shell > $HOME/shell.log 2>&1")
end)
