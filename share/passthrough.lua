-- The host side of interactive mode (NOTES findings 26, 29, 117, 118). Unlike the rest of share/,
-- this runs in the USER'S Hyprland: `omabox` sends it with hyprctl eval (install_passthrough), as the
-- body of a function called with (boxes dir, toggle key, theme colours file). Runtime-only, never
-- written to their config: a config reload drops it (and every Lua global), and an interactive box's
-- reaper sends it again within 2 s (keys_ensure). Versioned: a newer omabox replaces an older one's
-- hooks.
--
-- The submap "omabox" binds only the toggle key, so every other key (SUPER binds included) reaches the
-- focused box. Two ways into it:
--   one-shot: the toggle key. It ends by itself, so nobody is stuck with SUPER swallowed: when focus
--     goes to a window that is not a box, or on the first key pressed while the pointer is off every
--     box window (clicking the host bar leaves the box focused, so focus alone does not tell; that
--     first key, usually SUPER, still reaches the box, the rest are the host's).
--   keys-to-box (sticky, issue #22): `omabox keys-to-box on` leaves <boxes>/NAME/keys-to-box. While
--     it is there, that box's window taking focus enters the submap and focus going anywhere else
--     leaves it: focus alone decides, the key rule above does not apply. The toggle key gets the keys
--     back until the window loses focus and gets it again (a second press gives them to the box).
-- While the submap is on with a box window focused, that window's border takes the theme's attention
-- colour (its red, the bar's "active" modules' colour) and <boxes>/.keys names the box (the bar
-- widget lights its icon); both go back when the submap ends.
local boxes, key, colors = ...
local VERSION = 3
if (omabox_pass_version or 0) >= VERSION then return end
for _, sub in ipairs(omabox_pass_subs or {}) do sub:remove() end
-- An older omabox's binds in this Hyprland: replaced below (binding again would add a second one).
if omabox_passthrough then hl.unbind(key) end
omabox_passthrough = true
omabox_pass_version = VERSION

local TAG = "omabox-keys"

-- The box a window is, by name. Every interactive box's window has class aquamarine (and that title);
-- its client's pid is the box's outer bwrap (omabox-wlfd connects to this Hyprland, then execs it),
-- whose command line binds <boxes>/NAME/run into the box. The pid stays when the box's output is
-- recreated (a confirm-close keep, issue #24): the connection is the same. "" for a nested Hyprland
-- that is not one of these boxes, nil for any other window.
local NUL = string.char(0)
local run_pat = NUL .. boxes:gsub("[%^%$%(%)%%%.%[%]%*%+%-%?]", "%%%0") .. "/([%w][%w_.%-]*)/run" .. NUL
local function box_of(w)
  if not (w and w.class == "aquamarine") then return nil end
  local f = io.open("/proc/" .. tostring(w.pid) .. "/cmdline", "rb")
  if not f then return "" end
  local s = f:read("a") or ""
  f:close()
  return s:match(run_pat) or ""
end

-- Whether a window is a box with keys-to-box on.
local function sticky(w)
  local name = box_of(w)
  if not name or name == "" then return false end
  local f = io.open(boxes .. "/" .. name .. "/keys-to-box")
  if not f then return false end
  f:close()
  return true
end

local function over_box()
  local c = hl.get_cursor_pos()
  if not c then return false end
  for _, w in ipairs(hl.get_windows({ class = "aquamarine" })) do
    local a, s = w.at, w.size
    if w.mapped and not w.hidden and w.workspace and w.workspace.visible
      and c.x >= a.x and c.x < a.x + s.x and c.y >= a.y and c.y < a.y + s.y then return true end
  end
  return false
end

-- The border: a window rule on a tag of ours, the tag set and removed on the one window. (A tag change
-- re-applies rules at once; the tag outlives a reload, the rule does not, so a reinstall clears it.)
local color = "rgb(ff5555)" -- Hyprland's own urgent red, when the theme names none
local cf = colors and io.open(colors)
if cf then
  for line in cf:lines() do
    local hex = line:match("^%s*red%s*=%s*[\"']#(%x%x%x%x%x%x)[\"']")
    if hex then color = "rgb(" .. hex:lower() .. ")"; break end
  end
  cf:close()
end
hl.window_rule({ name = TAG, match = { tag = TAG }, border_color = color })
local function tag(addr, t) pcall(hl.dispatch, hl.dsp.window.tag({ tag = t, window = "address:" .. addr })) end

-- <boxes>/.keys: the name of the box the keys go to, or an empty line. Written only on a change,
-- renamed into place (the widget watches it).
local function say(name)
  if omabox_keys_said == name then return end
  local tmp = boxes .. "/.keys.tmp"
  local f = io.open(tmp, "w")
  if not f then return end
  f:write(name, "\n")
  f:close()
  if os.rename(tmp, boxes .. "/.keys") then omabox_keys_said = name end
end

-- What the submap and focus say now: the border on the focused box window (W, as check has it; nil:
-- as Hyprland says) while the submap is on.
local function mark(w)
  if w == nil then w = hl.get_active_window() end
  if hl.get_current_submap() ~= "omabox" or not (w and w.class == "aquamarine") then w = nil end
  local addr = w and w.address or nil
  if omabox_keys_marked ~= addr then
    if omabox_keys_marked then tag(omabox_keys_marked, "-" .. TAG) end
    if addr then tag(addr, "+" .. TAG) end
    omabox_keys_marked = addr
  end
  say(w and box_of(w) or "")
end

-- How the submap was entered: "sticky" (focus on a keys-to-box box) or "once" (the toggle key).
local function enter(how)
  omabox_keys_how = how
  if hl.get_current_submap() ~= "omabox" then hl.dispatch(hl.dsp.submap("omabox")) end
end
local function leave()
  omabox_keys_how = nil
  if hl.get_current_submap() == "omabox" then hl.dispatch(hl.dsp.submap("reset")) end
end

-- Focus on W (false: on no window; nil: the window focused now, as Hyprland says): off every box, the
-- submap ends (both modes); on a keys-to-box box it starts, unless the toggle key got out on that
-- window; on another box, a sticky submap ends, a one-shot one stays (as it always has, box to box).
-- (window.active says false itself: while a closing window's event runs, Hyprland still names it.)
local function check(w)
  if w == nil then w = hl.get_active_window() end
  w = w or nil
  if not (w and w.address == omabox_keys_paused) then omabox_keys_paused = nil end
  if not (w and w.class == "aquamarine") then leave()
  elseif sticky(w) then
    if not omabox_keys_paused then enter("sticky") end
  elseif omabox_keys_how == "sticky" then leave() end
  mark(w or false)
end
-- For `omabox keys-to-box` after it changed a box's file: a fresh start for the focused window.
function omabox_keys_changed()
  omabox_keys_paused = nil
  check()
end

hl.define_submap("omabox", function()
  hl.bind(key, function()
    local w = hl.get_active_window()
    omabox_keys_paused = w and w.address or nil
    leave()
  end, { description = "omabox: stop passing keys to the box" })
end)
hl.bind(key, function()
  omabox_keys_paused = nil
  enter(sticky(hl.get_active_window()) and "sticky" or "once")
end, { description = "omabox: pass keys to the box" })

-- A callback's error would be logged nowhere and end nothing: kept from breaking the hooks' caller.
omabox_pass_subs = {
  hl.on("window.active", function(w) pcall(check, w or false) end),
  hl.on("keybinds.submap", function() pcall(mark) end),
  hl.on("input.keyboard.key", function(_, _, state)
    if state ~= 1 or hl.get_current_submap() ~= "omabox" then return end
    pcall(function()
      if omabox_keys_how ~= "sticky" and not over_box() then leave() end
    end)
  end),
}

-- A reload keeps the submap it was in and the tags on windows, but not what they meant: start over
-- from what focus says now.
for _, w in ipairs(hl.get_windows({ class = "aquamarine" })) do tag(w.address, "-" .. TAG) end
omabox_keys_marked, omabox_keys_said, omabox_keys_how = nil, nil, nil
if hl.get_current_submap() == "omabox" then
  local w = hl.get_active_window()
  omabox_keys_how = sticky(w) and "sticky" or "once"
end
check()
