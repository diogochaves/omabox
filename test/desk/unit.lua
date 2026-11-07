-- t_unit_autostart's driver (#321, finding 702), run on the host's lua: share/autostart.lua's
-- verdicts, and share/desk.lua's wrappers around a stub `hl` (nothing reaches a real Hyprland; what
-- a wrapper lets through to os.execute or io.popen is harmless, and what it drops names a path that
-- is not there). Prints a line per check: NAME, a tab, what was wanted, a tab, what came.
-- lua unit.lua SHARE TMP
local share, tmp = arg[1], arg[2]
local A = dofile(share .. "/autostart.lua")
local function out(name, want, got) print(name .. "\t" .. tostring(want) .. "\t" .. tostring(got)) end

-- The table: each command's verdict and class, with no flags of the box's.
local rows = {
  { "qs -c bar", "run shell" }, { "quickshell -p /x", "run shell" }, { "dms run", "run shell" },
  { "caelestia shell -d", "run shell" }, { "noctalia-shell", "run shell" }, { "waybar", "run shell" },
  { "ags run", "run shell" }, { "hyprpanel", "run shell" }, { "eww daemon", "run shell" }, { "ironbar", "run shell" },
  { "omarchy-launch-shell", "run shell" },
  { "hypridle", "drop idle" }, { "swayidle -w", "drop idle" }, { "xidlehook --timer 1 x ''", "drop idle" },
  { "hyprlock", "drop lock" }, { "swaylock -f", "drop lock" }, { "gtklock", "drop lock" },
  { "loginctl lock-session", "drop lock" },
  { "systemctl --user start hyprpolkitagent", "drop session" }, { "uwsm finalize", "drop session" },
  { "uwsm start hyprland.desktop", "drop session" },
  { "dbus-update-activation-environment --systemd --all", "drop session" },
  { "/usr/lib/polkit-kde-authentication-agent-1", "drop sysbus" }, { "nm-applet --indicator", "drop sysbus" },
  { "blueman-applet", "drop sysbus" }, { "udiskie --automount", "drop sysbus" },
  { "omarchy-powerprofiles-init", "drop sysbus" }, { "kdeconnectd", "drop sysbus" },
  { "gnome-keyring-daemon --start", "drop box" }, { "pipewire", "drop box" }, { "wireplumber", "drop box" },
  { "/usr/lib/xdg-desktop-portal-hyprland", "drop box" }, { "hyprpm reload -n", "drop box" },
  { "omarchy-provision-first-run", "drop omarchy" }, { "omarchy-hyprland-monitor-watch", "drop omarchy" },
  { "omarchy-hook post-boot", "drop omarchy" }, { "herdr server", "drop omarchy" },
  { "hyprpaper", "run look" }, { "swww-daemon", "run look" }, { "mako", "run look" }, { "swaync", "run look" },
  { "wl-paste --type text --watch cliphist store", "run look" }, { "hyprctl setcursor Bibata 24", "run look" },
  { "firefox", "run other" }, { "systemctl suspend", "run other" }, { "omarchy-hook other", "run other" },
  -- Wrappers, parts and shells (the plan's cases).
  { "uwsm-app -- hypridle", "drop idle" }, { "FOO=1 setsid -f nm-applet", "drop sysbus" },
  { "sleep 2 && omarchy-hook post-boot", "drop omarchy" }, { "sh -c 'hypridle & waybar'", "drop idle" },
  { "myhypridle", "run other" }, { "myhypridle-watcher --x", "run other" }, { "/opt/x/hypridle", "drop idle" },
  { "env A=1 nohup hypridle", "drop idle" }, { "exec hyprlock", "drop lock" },
  { "systemd-run --user -u x -- hypridle", "drop idle" }, { "app2unit -- swayidle", "drop idle" },
  { "uwsm app -- waybar", "run shell" }, { "sleep 1; waybar", "run shell" },
  { "echo 'a && hypridle'", "run other" }, { 'bash -lc "sleep 1 | hyprlock"', "drop lock" },
  { "true || hypridle", "drop idle" },
}
for _, r in ipairs(rows) do
  local v, c = A.judge(r[1], "exec_cmd", {})
  out("judge " .. r[1], r[2], v .. " " .. c)
end
local v, c = A.judge("cat /etc/hostname", "popen", {})
out("judge a popen read: query", "run query", v .. " " .. c)
v, c = A.judge("hypridle", "popen", {})
out("judge a popen of hypridle: dropped", "drop idle", v .. " " .. c)

-- The box's flags change verdicts.
local function vc(cmd, env) local a, b = A.judge(cmd, "exec_cmd", env); return a .. " " .. b end
out("OMABOX_SYSTEMD=1: systemctl --user runs", "run session", vc("systemctl --user start x", { OMABOX_SYSTEMD = "1" }))
out("OMABOX_SYSTEMD=1: uwsm finalize runs", "run session", vc("uwsm finalize", { OMABOX_SYSTEMD = "1" }))
out("OMABOX_SYSTEMD=1: dbus-update still dropped (session-env.sh does it)", "drop session",
  vc("dbus-update-activation-environment --systemd", { OMABOX_SYSTEMD = "1" }))
out("OMABOX_POLKIT=1: a polkit agent runs", "run sysbus", vc("hyprpolkitagent", { OMABOX_POLKIT = "1" }))
out("OMABOX_SHELL_CMD: that shell dropped (the slot starts it)", "drop shell", vc("uwsm-app -- dms run", { OMABOX_SHELL_CMD = "dms run" }))
out("OMABOX_SHELL_CMD: another shell runs", "run shell", vc("waybar", { OMABOX_SHELL_CMD = "dms run" }))
out("OMABOX_DESK_BASE=omarchy: omarchy-launch-shell dropped (shell.sh starts it)", "drop shell",
  vc("omarchy-launch-shell", { OMABOX_DESK_BASE = "omarchy" }))
out("env as a function (os.getenv's shape)", "drop shell",
  vc("waybar", function(k) return k == "OMABOX_SHELL_CMD" and "waybar" or nil end))

-- The log's line.
local _, cl, why, part = A.judge("sleep 2 && omarchy-hook post-boot", "exec_cmd", {})
out("line: a dropped part named", "start drop omarchy exec_cmd sleep 2 && omarchy-hook post-boot  (omarchy-hook post-boot: "
  .. "Omarchy's own session work, not a box's)", A.line("start", "drop", cl, "exec_cmd", "sleep 2 && omarchy-hook post-boot", why, part))
out("line: a run, no reason", "load run other execute true", A.line("load", "run", "other", "execute", "true"))
out("line: one line for a command of two", "load run other exec_cmd a\\nb", A.line("load", "run", "other", "exec_cmd", "a\nb"))

-- desk.lua's wrappers around a stub hl, in a stand-in runtime dir and HOME.
local run, home = tmp .. "/run", tmp .. "/home"
os.execute("mkdir -p '" .. run .. "' '" .. home .. "/.config/hypr'")
for _, f in ipairs({ "autostart", "desk-shell", "plain", "omarchy" }) do
  local i = assert(io.open(share .. "/" .. f .. ".lua")); local s = i:read("a"); i:close()
  local o = assert(io.open(run .. "/omabox-" .. f .. ".lua", "w")); o:write(s); o:close()
end
local cfg = assert(io.open(home .. "/.config/hypr/hyprland.lua", "w"))
cfg:write([[
hl.exec_cmd("/nonexistent-omabox/hypridle")
hl.exec_cmd("/nonexistent-omabox/fixture-bar --x")
t_execute = { os.execute("/nonexistent-omabox/hyprlock") }
t_execute_ran = os.execute("true")
local w = io.popen("/nonexistent-omabox/hypridle", "w")
t_popen_w = w and w:write("x") and "writable" or "not"
if w then w:close() end
local r = io.popen("echo read-me")
t_popen_r = r and r:read("l")
if r then r:close() end
hl.on("hyprland.start", function() hl.exec_cmd("uwsm-app -- /nonexistent-omabox/swayidle") end)
]])
cfg:close()
local calls, handlers = {}, {}
hl = {
  exec_cmd = function(cmd) table.insert(calls, cmd) end,
  on = function(ev, fn) if ev == "hyprland.start" then table.insert(handlers, fn) end end,
  config = function() end, env = function() end, animation = function() end,
}
omabox_exec = hl.exec_cmd
local getenv = os.getenv
local fake = { XDG_RUNTIME_DIR = run, HOME = home, XDG_CONFIG_HOME = home .. "/.config", OMABOX_DESK_BASE = "plain" }
os.getenv = function(k) if fake[k] ~= nil then return fake[k] end return getenv(k) end
dofile(share .. "/desk.lua")
out("a dropped exec_cmd never reaches Hyprland, a run one does", "/nonexistent-omabox/fixture-bar --x", table.concat(calls, "|"))
out("a dropped os.execute says it ran", "true exit 0", table.concat({ tostring(t_execute[1]), tostring(t_execute[2]), tostring(t_execute[3]) }, " "))
out("a run os.execute runs", "true", tostring(t_execute_ran))
out("a dropped io.popen \"w\" takes writes", "writable", t_popen_w)
out("a run io.popen reads", "read-me", t_popen_r)
for _, fn in ipairs(handlers) do fn() end
out("at the start: dropped too", "/nonexistent-omabox/fixture-bar --x", table.concat(calls, "|"))
omabox_desk_phase("later")
hl.exec_cmd("/nonexistent-omabox/hypridle --bind")
out("later (a bind's Lua): passed through", "/nonexistent-omabox/fixture-bar --x|/nonexistent-omabox/hypridle --bind", table.concat(calls, "|"))
omabox_exec("/opt/omabox/x")
local f = assert(io.open(home .. "/autostart.log")); local log = f:read("a"); f:close()
out("the log, a line each, omabox_exec's not in it", table.concat({
  "load drop idle exec_cmd /nonexistent-omabox/hypridle  (idle: a box has no input and would lock itself (finding 10))",
  "load run other exec_cmd /nonexistent-omabox/fixture-bar --x",
  "load drop lock execute /nonexistent-omabox/hyprlock  (lock: a box takes no real password, and nothing would unlock it)",
  "load run other execute true",
  "load drop idle popen /nonexistent-omabox/hypridle  (idle: a box has no input and would lock itself (finding 10))",
  "load run query popen echo read-me",
  "start drop idle exec_cmd uwsm-app -- /nonexistent-omabox/swayidle  (idle: a box has no input and would lock itself (finding 10))",
  "later run idle exec_cmd /nonexistent-omabox/hypridle --bind  (after the start: the user's own doing)",
}, "|"), (log:gsub("\n$", ""):gsub("\n", "|")))
os.getenv = getenv
