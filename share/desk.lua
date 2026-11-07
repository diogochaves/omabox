-- `up --desktop desk` (#321, finding 702): the user's own Hyprland config, the box HOME's
-- ~/.config/hypr/hyprland.lua, loaded as their desktop loads it, on Omarchy's base or a plain one
-- (OMABOX_DESK_BASE, fixed at up: finding 708). The box's copy of share/desk.lua, run by hyprland.lua,
-- which then sets what every box gets over it (the screen and monitors, debug, the cursor's buffer,
-- autoreload, Xwayland, the pointer, the crash hook, the start).
--
-- Before the config, an autostart policy (share/autostart.lua, the box's copy): each command it
-- starts through hl.exec_cmd, os.execute or io.popen is judged, and what a box must not or cannot
-- run is dropped (idle and lock daemons, the user's systemd session, system-bus applets, what the box
-- runs itself, Omarchy's own session work, the shell the box's slot starts); each decision is a line
-- in ~/autostart.log (`omabox log autostart`): PHASE VERDICT CLASS VIA CMD  (WHY). PHASE is load (the
-- first load), start (the hyprland.start handlers), reload (a later load: `hyprctl reload`) or later
-- (after the start, or after a reload's load: a bind's Lua, which runs as the user asked, unjudged).
-- Binds that name a command (hl.dsp.exec_cmd) are never wrapped: a key pressed in the box does what
-- it does on the desk. What omabox itself starts goes through omabox_exec (hyprland.lua), unjudged.
local run = os.getenv("XDG_RUNTIME_DIR") or ""
local home = os.getenv("HOME") or ""
local base = os.getenv("OMABOX_DESK_BASE") == "omarchy" and "omarchy" or "plain"
local policy = dofile(run .. "/omabox-autostart.lua")
-- The desk shell's slot (#321 step 4): take(cmd) for a shell's command (true: the slot runs it, the
-- config does not), after_start() once the config's start handlers have run.
local deskshell = dofile(run .. "/omabox-desk-shell.lua")
local conf = os.getenv("XDG_CONFIG_HOME") or (home .. "/.config")
local hypr = conf .. "/hypr"
-- As Hyprland reads a config's modules: beside it, and ~/.config's (`require("hypr.x")`).
package.path = hypr .. "/?.lua;" .. hypr .. "/?/init.lua;" .. conf .. "/?.lua;" .. package.path

local function note(...)
  local f = io.open(home .. "/autostart.log", "a")
  if f then f:write(policy.line(...), "\n"); f:close() end
end
-- Hyprland's own log, as a config error (hyprland.log, ERR from omabox ]).
local function hypr_err(text)
  local f = io.open(run .. "/hypr/" .. (os.getenv("HYPRLAND_INSTANCE_SIGNATURE") or "") .. "/hyprland.log", "a")
  if f then f:write("ERR from omabox ]: " .. text:gsub("%s+", " ") .. "\n"); f:close() end
end

-- The phase: a load before the start is the first one (omabox.env is written at the end of the
-- start), any other a reload; a reload starts a fresh Lua state, so nothing here outlives a load.
local phase = "load"
do
  local f = io.open(run .. "/omabox.env")
  if f then f:close(); phase = "reload" end
end
-- hyprland.lua's: "later" at the end of its start handler, the last registered; "parsed" at the end
-- of its load (a reload's load is over: what runs now is a bind's).
function omabox_desk_phase(p)
  if p == "later" or (p == "parsed" and phase == "reload") then phase = "later" end
end

-- Whether CMD, started through VIA, runs; said in ~/autostart.log either way.
local function allowed(cmd, via)
  if type(cmd) ~= "string" then return true end
  local verdict, class, why, part = policy.judge(cmd, via, os.getenv)
  if phase == "later" then
    note(phase, "run", class, via, cmd, verdict == "drop" and "after the start: the user's own doing" or nil)
    return true
  end
  -- A shell's command: the slot's first (step 4), before the policy's own drop.
  if class == "shell" and deskshell.take(cmd) then verdict, why, part = "drop", "the box's shell slot runs it", nil end
  note(phase, verdict, class, via, cmd, why, part)
  return verdict == "run"
end

local cfg = hypr .. "/hyprland.lua"
local have = io.open(cfg)
if not have then
  -- No Lua config (none, or only a hyprland.conf, which a Lua Hyprland does not read): the base's
  -- own desktop instead, as a box of it runs (plain.lua or omarchy.lua).
  local conf_only = io.open(hypr .. "/hyprland.conf")
  if conf_only then conf_only:close() end
  note(phase, "none", "config", "dofile", cfg,
    (conf_only and "only a hyprland.conf: " or "no hyprland.lua: ") .. "omabox's " .. base .. " desktop instead")
  dofile(run .. "/omabox-" .. base .. ".lua")
else
  have:close()
  local raw_exec, raw_execute, raw_popen = omabox_exec or hl.exec_cmd, os.execute, io.popen
  hl.exec_cmd = function(cmd, ...)
    if allowed(cmd, "exec_cmd") then return raw_exec(cmd, ...) end
  end
  os.execute = function(cmd, ...)
    if cmd == nil then return raw_execute() end -- (is there a shell: not a command)
    if allowed(cmd, "execute") then return raw_execute(cmd, ...) end
    return true, "exit", 0
  end
  io.popen = function(cmd, mode, ...)
    if allowed(cmd, "popen") then return raw_popen(cmd, mode, ...) end
    -- (what it would have read: nothing; written: taken and thrown away)
    return io.open("/dev/null", (mode or "r"):sub(1, 1) == "w" and "w" or "r")
  end
  -- The start's phase, from a handler registered before the config's own.
  hl.on("hyprland.start", function() phase = "start" end)
  if base == "omarchy" then
    -- Omarchy's base: the config loads Omarchy's bootstrap and defaults itself. As an Omarchy box,
    -- without Omarchy's session autostart (default.hypr.autostart: systemd/dbus env import, the
    -- shell, first-run, monitor-watch, udiskie, herdr, post-boot; the box's shell.sh starts
    -- Omarchy's shell), and without the user's hypr.monitors (#246: rules for real screens, and
    -- GDK_SCALE; the box's monitors are omabox's). bootstrap.lua empties package.loaded of both
    -- prefixes, so they are marked again after it, and package.preload holds them whatever loads it.
    local function mark()
      package.loaded["default.hypr.autostart"] = true
      package.loaded["hypr.monitors"] = true
    end
    package.preload["default.hypr.autostart"] = function() return true end
    package.preload["hypr.monitors"] = function() return true end
    mark()
    local raw_dofile = dofile
    local function after(...) mark(); return ... end
    dofile = function(path, ...)
      if type(path) == "string" and path:match("/default/hypr/bootstrap%.lua$") then return after(raw_dofile(path, ...)) end
      return raw_dofile(path, ...)
    end
  end
  -- The config itself; an error in it is a config error: the box still starts, with what loaded.
  local ok, err = pcall(dofile, cfg)
  if not ok then
    local e = tostring(err):gsub("%s+", " ")
    hypr_err(cfg .. ": " .. e)
    note(phase, "error", "config", "dofile", cfg, e)
  end
end
-- The slot's guess once the config's start handlers have run (a handler registered after theirs).
hl.on("hyprland.start", function() deskshell.after_start() end)

-- The desk's overrides: what every box needs over a desktop's config (hyprland.lua sets the rest
-- after this). omabox's stand-ins first on PATH, ahead of Omarchy's bin on its base (finding 149).
do
  local standins = "/opt/omabox/share/bin"
  local kept, omarchy_bin = { standins }, nil
  if base == "omarchy" then
    local ok, paths = pcall(require, "default.hypr.paths")
    local o = ok and type(paths) == "table" and paths.omarchy_path or os.getenv("OMARCHY_PATH") or "/usr/share/omarchy"
    omarchy_bin = o .. "/bin"
    table.insert(kept, omarchy_bin)
  end
  for entry in (os.getenv("PATH") or "/usr/bin"):gmatch("[^:]+") do
    if entry ~= standins and entry ~= omarchy_bin then table.insert(kept, entry) end
  end
  hl.env("PATH", table.concat(kept, ":"))
end
-- plain.lua's every-box settings: no update news or donation prompt, no guiutils notice, a window
-- on the workspace in front (`omabox run` starts everything from one process), no zoom when an
-- output appears (finding 670).
hl.config({
  ecosystem = { no_update_news = true, no_donation_nag = true },
  misc = { disable_hyprland_guiutils_check = true, initial_workspace_tracking = 0 },
})
hl.animation({ leaf = "monitorAdded", enabled = false })
