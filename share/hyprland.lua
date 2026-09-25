-- omabox nested Hyprland: real Omarchy defaults, minus Omarchy's session autostart.
-- Settings come from the environment `omabox up` sets: OMABOX_SIZE (WxH@HZ), OMABOX_INTERACTIVE, OMABOX_XWAYLAND.
dofile((os.getenv("OMARCHY_PATH") or "/usr/share/omarchy") .. "/default/hypr/bootstrap.lua")
package.loaded["default.hypr.autostart"] = true -- skip: systemd/dbus env import, first-run, monitor-watch, udiskie
require("default.hypr.omarchy")
-- As Omarchy's stock ~/.config/hypr/hyprland.lua ends (its hypr.* user modules are comments only):
-- the toggle flags and workspace layouts from ~/.local/state, so Omarchy's toggles work in a box.
require("default.hypr.toggles")

local interactive = os.getenv("OMABOX_INTERACTIVE") == "1"

if interactive then
  -- The window on the user's desktop is the screen; it follows that window's size. Any name: after a
  -- close with confirm-close on, the window the box opens again is WAYLAND-2 (finding 70).
  hl.monitor({ output = "", mode = "preferred", position = "0x0", scale = 1 })
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
  hl.on("monitor.removed", function()
    if gen ~= omabox_close_gen or #hl.get_monitors() > 0 then return end
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
  if not omabox_resize_timer then
    local last
    omabox_resize_timer = hl.timer(function()
      local m = hl.get_monitors()[1]
      if not m then return end
      local now = m.name .. " " .. m.width .. "x" .. m.height
      if last and now ~= last then hl.exec_cmd("/usr/bin/hyprctl reload") end
      last = now
    end, { timeout = 250, type = "repeat" })
  end
else
  -- WAYLAND-1 is only the bootstrap window inside labwc; the real screen is a headless output.
  hl.monitor({ output = "WAYLAND-1", disabled = true })
  -- OMABOX_SIZE is WxH@HZ; `omabox mode` writes a later choice to omabox.mode so a reload keeps it.
  local mode = os.getenv("OMABOX_SIZE") or "1920x1080@60"
  local f = io.open((os.getenv("XDG_RUNTIME_DIR") or "") .. "/omabox.mode")
  if f then mode = f:read("l") or mode; f:close() end
  if not mode:find("@") then mode = mode .. "@60" end
  hl.monitor({ output = "HEADLESS-2", mode = mode, position = "0x0", scale = 1 })
end
hl.config({
  debug = { vfr = true, disable_logs = false },
  misc = { disable_watchdog_warning = true },
  xwayland = { enabled = os.getenv("OMABOX_XWAYLAND") == "1" },
})

hl.on("hyprland.start", function()
  if not interactive then
    hl.exec_cmd("/usr/bin/hyprctl output create headless HEADLESS-2")
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
