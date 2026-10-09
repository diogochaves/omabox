-- omabox nested Hyprland: real Omarchy defaults, minus Omarchy's session autostart.
-- Settings come from the environment `omabox up` sets: OMABOX_SIZE (WxH@HZ), OMABOX_INTERACTIVE, OMABOX_XWAYLAND,
-- OMABOX_AUTORELOAD.
dofile((os.getenv("OMARCHY_PATH") or "/usr/share/omarchy") .. "/default/hypr/bootstrap.lua")
package.loaded["default.hypr.autostart"] = true -- skip: systemd/dbus env import, first-run, monitor-watch, udiskie
require("default.hypr.omarchy")
-- Omarchy's envs.lua puts its own bin first for everything Hyprland starts (the bar, binds,
-- terminals): omabox's stand-ins (omarchy-version, the browser policy) go ahead of it again, as on
-- every other box PATH (finding 149).
do
  local standins = "/opt/omabox/share/bin"
  local omarchy_bin = require("default.hypr.paths").omarchy_path .. "/bin"
  local kept = { standins, omarchy_bin }
  for entry in (os.getenv("PATH") or "/usr/bin"):gmatch("[^:]+") do
    if entry ~= standins and entry ~= omarchy_bin then table.insert(kept, entry) end
  end
  hl.env("PATH", table.concat(kept, ":"))
end
-- As Omarchy's stock ~/.config/hypr/hyprland.lua ends (its hypr.* user modules are comments only):
-- the toggle flags and workspace layouts from ~/.local/state, so Omarchy's toggles work in a box.
require("default.hypr.toggles")

local interactive = os.getenv("OMABOX_INTERACTIVE") == "1"

-- More monitors (#122, #123): `omabox monitor add` (and `up --monitor`) make each one and write it to
-- omabox.monitors, "NAME MODE PLACE SCALE" a line, in the order made. MODE and SCALE as Hyprland made
-- them; an interactive box's monitor keeps the MODE and SCALE asked for (#174; `preferred`, its
-- window's size, in a box started before). PLACE as asked (#161): `right` or `below` the line before
-- it (the first: the main screen), or XxY. Placed here, on every load (a reload keeps them) and
-- whenever omabox asks (omabox_place_monitors(), after `mode`, `monitor add` and `monitor remove`): a
-- main screen of another size moves the ones placed next to it; an XxY one stays. A line whose
-- monitor is not there keeps its place, so the others stay: a headless one `omabox output drop` took
-- away (#163) comes back to it (its rule set here makes no output), and an interactive box's new
-- window opens in it (omabox writes the line first). A line of a box started before #174 whose window
-- is gone is skipped: the next goes next to the one before it. Read after the box's own screen rule,
-- which they override for their output.
-- An interactive box with monitors shows each as a view (#174): its window on the user's desktop is
-- the monitor scaled down, all by one factor, a display-settings picture of the layout (omabox lays
-- the windows out). The monitor's mode is its window's pixels (aquamarine's Wayland output takes the
-- size of its window, finding 28), its scale those pixels over the size asked for (below 1 on a
-- smaller window), so its size in the layout, the one the bar and apps lay out on, is the size asked
-- for: the main screen's `--size`, a monitor's SPEC (over its scale). A window the user resizes
-- changes its view's scale, not the monitor's place; along a side whose aspect changed the monitor
-- gets smaller (the whole of it stays in the window).
local main_screen, main_mode -- set below: the main screen's name (nil: the first other), its mode before it exists
-- The monitors as placed, in logical pixels as Hyprland lays them out: the main screen first ({ name,
-- x, y, w, h, m = the live monitor }), then each line's ({ ..., l = the line }); whether the box has
-- views, their scale (the main window's), and the function that gives a window's.
local function omabox_layout(more)
  local lines, extra = {}, {}
  local function add(line)
    local n, m, p, s = line:match("^([%w_-]+) (%S+) (%S+) ([%d.]+)$")
    if n then table.insert(lines, { name = n, mode = m, place = p, scale = tonumber(s) }); extra[n] = true end
  end
  local mf = io.open((os.getenv("XDG_RUNTIME_DIR") or "") .. "/omabox.monitors")
  if mf then
    for line in mf:lines() do add(line) end
    mf:close()
  end
  if more then add(more) end
  local views = false
  for _, l in ipairs(lines) do if interactive and l.mode:match("^%d+x%d+") then views = true end end
  local live, main = {}, nil
  for _, m in ipairs(hl.get_monitors()) do
    live[m.name] = m
    if not main and not extra[m.name] and m.name ~= "FALLBACK" and (not main_screen or m.name == main_screen) then main = m end
  end
  local prev
  if main and not views then
    prev = { x = main.x, y = main.y, w = math.floor(main.width / main.scale), h = math.floor(main.height / main.scale) }
  elseif main_mode then
    -- Views: the size asked for. A headless box with no main screen yet, or dropped (`omabox output
    -- drop`, #163): its mode as the config sets it, omabox.mode read again (an `omabox mode` since
    -- this load set it live, and the next load reads it).
    local mode = main_mode
    local f = not interactive and io.open((os.getenv("XDG_RUNTIME_DIR") or "") .. "/omabox.mode")
    if f then mode = f:read("l") or mode; f:close() end
    local w, h = mode:match("^(%d+)x(%d+)")
    if w then prev = { x = 0, y = 0, w = tonumber(w), h = tonumber(h) } end
  end
  -- A view's scale: its window's pixels over the size asked for, with the whole of it in the window.
  local function view(m, w, h) return math.floor(math.max(m.width / w, m.height / h) * 100000 + 0.5) / 100000 end
  local f = views and main and prev and view(main, prev.w, prev.h) or nil
  local out = { { name = main and main.name, x = 0, y = 0, w = prev and prev.w, h = prev and prev.h, m = main } }
  for _, l in ipairs(lines) do
    local W, H = l.mode:match("^(%d+)x(%d+)")
    local m = live[l.name]
    if W or m then
      local w, h
      if W then w, h = math.floor(tonumber(W) / l.scale), math.floor(tonumber(H) / l.scale)
      else w, h = math.floor(m.width / m.scale), math.floor(m.height / m.scale) end
      local x, y = l.place:match("^(%d+)x(%d+)$")
      if x then x, y = tonumber(x), tonumber(y)
      elseif prev and l.place == "right" then x, y = prev.x + prev.w, prev.y
      elseif prev and l.place == "below" then x, y = prev.x, prev.y + prev.h end
      table.insert(out, { name = l.name, x = x, y = y, w = w, h = h, m = m, l = l })
      prev = x and { x = x, y = y, w = w, h = h } or nil
      -- No main window (closed): the views share one scale, a live one's.
      if views and W and m and not f then f = view(m, w, h) end
    end
  end
  return out, views, f, view
end
function omabox_place_monitors()
  local out, views, f, view = omabox_layout()
  for i, b in ipairs(out) do
    local l = b.l
    if i == 1 then
      -- An interactive box's main window: a view while it has monitors, else its window's size.
      if interactive and b.m then
        hl.monitor({ output = b.name, mode = "preferred", position = "0x0", scale = views and f or 1 })
      end
    else
      local pos = b.x and (b.x .. "x" .. b.y) or (l.place == "below" and "auto-down" or "auto-right")
      if views and l.mode:match("^%d+x%d+") then
        -- A window not there yet (omabox makes it next) opens at the views' scale.
        hl.monitor({ output = l.name, mode = "preferred", position = pos, scale = b.m and view(b.m, b.w, b.h) or f or 1 })
      else
        hl.monitor({ output = l.name, mode = l.mode, position = pos, scale = l.scale })
      end
    end
  end
end
-- The layout omabox lays an interactive box's windows out by (#174), as omabox_place_monitors places
-- it: "NAME X Y W H" a monitor, the main screen as `main`, comma-separated; one with no place (none
-- before it) left out. With MORE, a line as omabox.monitors has them, as if it were there last (a
-- monitor about to be made). Raised as an error: `hyprctl eval` passes back nothing else.
function omabox_monitor_layout(more)
  local rows = {}
  for i, b in ipairs((omabox_layout(more))) do
    if b.x and b.w then table.insert(rows, (i == 1 and "main" or b.name) .. " " .. b.x .. " " .. b.y .. " " .. b.w .. " " .. b.h) end
  end
  error("omabox-layout:" .. table.concat(rows, ","), 0)
end

if interactive then
  -- The window on the user's desktop is the screen; it follows that window's size. Any name: after a
  -- close with confirm-close on, the window the box opens again is WAYLAND-2 (finding 70).
  hl.monitor({ output = "", mode = "preferred", position = "0x0", scale = 1 })
  -- With monitors, the main screen's size is --size's (a view, above).
  main_mode = os.getenv("OMABOX_SIZE") or "1920x1080@60"
  -- Closing that window only removes the output; end the box with it instead of idling screenless.
  -- With confirm-close on (omabox writes the flag; `omabox config` changes it live), the first close
  -- opens a new window and asks instead; a close while it asks is the answer. "Asking" is a file, not
  -- a Lua global: the new window's output has a new name, the resize watch below reloads, and a reload
  -- starts a fresh Lua state. The generation keeps this to the latest load's handler, in case a reload
  -- keeps the earlier ones.
  omabox_close_gen = (omabox_close_gen or 0) + 1
  local gen = omabox_close_gen
  local run = os.getenv("XDG_RUNTIME_DIR") or ""
  local function read(path)
    local f = io.open(path)
    if not f then return nil end
    local v = f:read("l")
    f:close()
    return v
  end
  -- A new output after the last one went (the window confirm-close opens) shows a new, empty
  -- workspace, not the one the box showed (issue #24): Hyprland parks the workspaces on its FALLBACK
  -- monitor meanwhile, and gives the new output the first free one. By the time the output is gone
  -- no workspace is active any more, so follow the one shown here, write it down when the last output
  -- goes (a file: the resize watch reloads, twice, before the new output is complete), and focus it
  -- again once the new output holds the workspaces (FALLBACK gone), windows on the others kept.
  local shown
  local function track(w)
    if w and not w.special and w.monitor and w.monitor.name ~= "FALLBACK" then
      shown = w.id > 0 and tostring(w.id) or ("name:" .. w.name)
    end
  end
  track(hl.get_active_workspace())
  hl.on("workspace.active", function(w) if gen == omabox_close_gen then track(w) end end)
  local function restore()
    local ws = read(run .. "/omabox.workspace")
    local mons = hl.get_monitors()
    if not ws or #mons == 0 then return end
    for _, m in ipairs(mons) do if m.name == "FALLBACK" then return end end
    os.remove(run .. "/omabox.workspace")
    hl.dispatch(hl.dsp.focus({ workspace = ws }))
  end
  hl.on("monitor.added", function() if gen == omabox_close_gen then restore() end end)
  -- Each monitor's box in the layout, for omabox's aquamarine (AQ_WAYLAND_LAYOUT, finding 238): the
  -- host's pointer in a monitor's window comes as a point of that window, and Hyprland 0.56 places it
  -- over the box around every monitor, not over that one; with this, aquamarine gives it in the
  -- layout's terms. Logical sizes as Hyprland rounds them. Written anew at every layout change.
  local function write_layout()
    local rows = {}
    for _, m in ipairs(hl.get_monitors()) do
      local w, h = m.width / m.scale, m.height / m.scale
      if m.transform % 2 == 1 then w, h = h, w end
      -- (Positions rounded too: `%d` raises on a float with a fraction, which `disable_scale_checks`
      -- can give an auto-placed monitor, and the handler would die with the file left stale.)
      table.insert(rows, string.format("%s %d %d %d %d", m.name, math.floor(m.x + 0.5), math.floor(m.y + 0.5), math.floor(w + 0.5), math.floor(h + 0.5)))
    end
    local f = io.open(run .. "/omabox.layout.new", "w")
    if not f then return end
    f:write(table.concat(rows, "\n") .. "\n")
    f:close()
    os.rename(run .. "/omabox.layout.new", run .. "/omabox.layout")
  end
  write_layout()
  hl.on("monitor.layout_changed", function() if gen == omabox_close_gen then write_layout() end end)
  hl.on("monitor.removed", function()
    if gen ~= omabox_close_gen then return end
    if #hl.get_monitors() > 0 then restore(); return end
    if shown then
      local f = io.open(run .. "/omabox.workspace", "w")
      if f then f:write(shown .. "\n"); f:close() end
    end
    if read(run .. "/omabox.confirm-close") == "on" and not read(run .. "/omabox.close-asking") then
      local f = io.open(run .. "/omabox.close-asking", "w")
      if f then f:write("1\n"); f:close() end
      hl.exec_cmd("/opt/omabox/share/confirm-close.sh")
    else
      -- Closed on purpose: the box's reaper on the host then clears it, instead of leaving it
      -- "dead" as a crash would be (with its logs) (finding 71).
      local f = io.open(run .. "/omabox.closed", "w")
      if f then f:write("1\n"); f:close() end
      hl.dispatch(hl.dsp.exit())
    end
  end)
  -- Resizing the window changes the output's mode, but Hyprland 0.56 fires no event and does not
  -- re-arrange for it: bar and wallpaper keep the old size until something else commits (NOTES
  -- finding 28). Only a config reload fixes it, so watch the size and reload when it changes.
  -- Global guard: one timer, however many reloads.
  -- Every monitor's: an interactive box can have a window per monitor (#123).
  -- A view's scale (#174) follows its window's size too: the reload places them. A reload starts a
  -- fresh Lua state, and with it a new timer: the sizes it compares with are the ones this load
  -- placed by, so a size that changes between the load and the timer's first tick is not missed (it
  -- was: a window resized just after a reload kept its view at the old scale).
  if not omabox_resize_timer then
    local function sizes()
      local parts = {}
      for _, m in ipairs(hl.get_monitors()) do table.insert(parts, m.name .. " " .. m.width .. "x" .. m.height) end
      return #parts > 0 and table.concat(parts, ",") or nil
    end
    local last = sizes()
    omabox_resize_timer = hl.timer(function()
      local now = sizes()
      if not now then return end
      if last and now ~= last then hl.exec_cmd("/usr/bin/hyprctl reload") end
      last = now
    end, { timeout = 250, type = "repeat" })
  end
else
  -- NVIDIA's GBM driver cannot allocate the linear buffers aquamarine asks for on a synthetic
  -- headless output. Use the private labwc Wayland output as the screen on that driver.
  local waylandScreen = os.getenv("OMABOX_WAYLAND_SCREEN") == "1"
  if not waylandScreen then hl.monitor({ output = "WAYLAND-1", disabled = true }) end
  -- OMABOX_SIZE is WxH@HZ; `omabox mode` writes a later choice to omabox.mode so a reload keeps it.
  local mode = os.getenv("OMABOX_SIZE") or "1920x1080@60"
  local f = io.open((os.getenv("XDG_RUNTIME_DIR") or "") .. "/omabox.mode")
  if f then mode = f:read("l") or mode; f:close() end
  if not mode:find("@") then mode = mode .. "@60" end
  main_screen, main_mode = waylandScreen and "WAYLAND-1" or "HEADLESS-2", mode
  hl.monitor({ output = main_screen, mode = mode, position = "0x0", scale = 1 })
end
hl.config({
  -- An interactive box's views (#174) are at any scale, a window's pixels over the size asked for,
  -- which Hyprland would otherwise round to one that divides the window into whole pixels (a 1000x562
  -- window at 0.5208 became 0.5: a 2000x1124 monitor). Its size in the layout is rounded instead.
  debug = { vfr = true, disable_logs = false, disable_scale_checks = interactive },
  -- No reload because a file changed (#140): a box changes when the agent asks (`hyprctl reload`,
  -- `omabox reload`), not when omabox or Omarchy is updated under it. `up --autoreload` keeps it on,
  -- as on a desktop, for a project that has to see what a file change does (a reload loop).
  misc = { disable_watchdog_warning = true, disable_autoreload = os.getenv("OMABOX_AUTORELOAD") ~= "1" },
  xwayland = { enabled = os.getenv("OMABOX_XWAYLAND") == "1" },
})
omabox_place_monitors()

-- Quickshell's crash reporter (#126, NOTES finding 183): when the shell crashes, Quickshell writes a
-- report under ~/.cache/quickshell/crashes, then opens a dialog that takes focus. Nobody reads it in a
-- box, and it took the keys meant for the app under test (Return on it opened a browser). It is
-- closed as it opens; the report stays, and `up`/`restart-shell` name it. Told apart by its process's
-- environment, which only the reporter has, never by its class: every Quickshell window, a project's
-- own included, is org.quickshell. (Not QS_DISABLE_CRASH_HANDLER: that writes no report at all.)
-- Global guard: one handler, however many reloads.
if not omabox_crash_hook then
  omabox_crash_hook = hl.on("window.open", function(w)
    if not w or w.class ~= "org.quickshell" or type(w.pid) ~= "number" then return end
    local f = io.open("/proc/" .. math.floor(w.pid) .. "/environ", "rb")
    if not f then return end
    local env = f:read("a") or ""
    f:close()
    if not env:find("__QUICKSHELL_CRASH_DUMP_PID=", 1, true) then return end
    local pid = math.floor(w.pid)
    hl.exec_cmd("kill " .. pid .. "; echo \"omabox: closed the shell's crash dialog (pid " .. pid ..
      "); the report is in ~/.cache/quickshell/crashes\" >> \"$HOME/shell.log\"")
  end)
end

hl.on("hyprland.start", function()
  -- The box's events, from the start, for `omabox events` (NOTES finding 108): a line each, stamped.
  hl.exec_cmd('/opt/omabox/bin/omabox-events "$HOME/events.log"')
  if not interactive then
    if os.getenv("OMABOX_WAYLAND_SCREEN") ~= "1" then
      hl.exec_cmd("/usr/bin/hyprctl output create headless HEADLESS-2")
    end
    -- A headless box has no input devices, and with none on the seat Hyprland drops focus changes:
    -- a shell panel then never gets the keys of a later `omabox keys`, nor a pointer the click of a
    -- later `omabox click` on the same spot (NOTES finding 41). Keep an idle keyboard and pointer.
    hl.exec_cmd("/opt/omabox/bin/omabox-keyboard --hold")
    hl.exec_cmd("/opt/omabox/bin/omabox-pointer --hold")
  end
  -- The Omarchy shell, unless `up --no-shell`: then a bare compositor, no bar, tray or notifications.
  if os.getenv("OMABOX_SHELL") ~= "0" then hl.exec_cmd("/opt/omabox/share/shell.sh") end
  -- What `omabox run` gives commands, so they see the box like anything Hyprland launched.
  -- Written last and renamed into place: its presence means Hyprland is up.
  hl.exec_cmd("env -0 > $XDG_RUNTIME_DIR/omabox.env.tmp && mv $XDG_RUNTIME_DIR/omabox.env.tmp $XDG_RUNTIME_DIR/omabox.env")
end)
