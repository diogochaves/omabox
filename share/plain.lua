-- omabox's own small Hyprland config for `up --desktop plain` (#247): a box with nothing of Omarchy's,
-- for any Hyprland with a Lua config. Hyprland's own defaults otherwise; hyprland.lua then sets the
-- screen, monitors, debug and the startup as for every box. No bar: a plain box runs no shell.

-- The keyboard layout the machine was set up with, as an Omarchy box gets it (Omarchy reads the same
-- file): systemd-localed's /etc/vconsole.conf, else Debian's /etc/default/keyboard; the layout and its
-- variant only. Neither there: Hyprland's own default, us.
local function xkb()
  for _, path in ipairs({ "/etc/vconsole.conf", "/etc/default/keyboard" }) do
    local f = io.open(path)
    if f then
      local v = {}
      for line in f:lines() do
        local key, value = line:match("^%s*([%w_]+)%s*=%s*(.-)%s*$")
        if key then v[key] = value:gsub("%s+#.*$", ""):gsub('^"(.*)"$', "%1"):gsub("^'(.*)'$", "%1") end
      end
      f:close()
      if v.XKBLAYOUT and v.XKBLAYOUT ~= "" then return v.XKBLAYOUT, v.XKBVARIANT or "" end
    end
  end
  return "us", ""
end
local layout, variant = xkb()

-- The colour where no surface is: a clean box's sentinel (`up --clean`, #247), as omabox.clean says
-- it ("background COLOUR"; any of it inside a window is a hole), else a dark one.
local function background()
  local f = io.open((os.getenv("XDG_RUNTIME_DIR") or "") .. "/omabox.clean")
  if not f then return "rgb(1a1b26)" end
  local colour
  for l in f:lines() do colour = l:match("^background (%S+)$") or colour end
  f:close()
  return colour or "rgb(1a1b26)"
end

hl.config({
  -- No update news or donation prompt over the first frames (Omarchy turns them off too).
  ecosystem = { no_update_news = true, no_donation_nag = true },
  general = { gaps_in = 4, gaps_out = 8, border_size = 2 },
  misc = {
    -- No "hyprland-guiutils is not installed" over the first frames either: Debian's and openSUSE's
    -- Hyprland come without it (Arch's depends on it), and its notice's shrinking bar kept the screen
    -- changing for its 10 s, so `run --wait` never saw it settle.
    disable_hyprland_guiutils_check = true,
    disable_hyprland_logo = true,
    disable_splash_rendering = true,
    background_color = background(),
    focus_on_activate = true,
  },
  input = { kb_layout = layout, kb_variant = variant, follow_mouse = 1 },
})
-- The two binds a bare desktop needs to be used by hand (an interactive box): a terminal (the
-- session's $TERMINAL), and closing a window. Anything else is the agent's (`omabox keys`, `omabox
-- hyprctl dispatch`).
hl.bind("SUPER + RETURN", hl.dsp.exec_cmd(os.getenv("TERMINAL") or "xdg-terminal-exec"))
hl.bind("SUPER + W", hl.dsp.window.close())
