-- omabox's Omarchy desktop: Omarchy's real defaults, minus its session autostart. The box's copy of
-- this file (#140; start-hyprland.sh and `omabox reload` write it with hyprland.lua's), run first by
-- hyprland.lua, which then sets what every box gets. A desk box on Omarchy's base without a Lua
-- config of its own runs it too (desk.lua, #321).
dofile((os.getenv("OMARCHY_PATH") or "/usr/share/omarchy") .. "/default/hypr/bootstrap.lua")
package.loaded["default.hypr.autostart"] = true -- skip: systemd/dbus env import, first-run, monitor-watch, udiskie
require("default.hypr.omarchy")
-- The box HOME's own ~/.config/hypr modules (#246), where stock ~/.config/hypr/hyprland.lua loads them
-- (after Omarchy's defaults, before the toggles), so an edit there and `hyprctl reload` do what they do
-- on a desktop (a plugin's "add this to bindings.lua"). Before everything omabox sets in hyprland.lua,
-- which wins: PATH, the screen, debug/misc/xwayland, the hidden pointer. Not ~/.config/hypr/hyprland.lua
-- itself (this file stands in for it; a desk box runs that one, #321). Skipped: autostart (as
-- default.hypr.autostart), and monitors: a new user's is not comments only (GDK_SCALE=2 and a catch-all
-- rule at scale auto), and its rules are for real screens; the box's are omabox's (`--size`,
-- `--monitor`, `mode`). Hyprland's require keeps a module's error from ending the load: it is a config
-- error (`hyprctl configerrors`; no red bar in a box, hyprland.lua), which `up`, `reload` and `hyprctl
-- reload` say. The pcall is for a require that raises instead (another build): the rest of the config
-- still loads, and the error goes to Hyprland's log. A module with no file is skipped (a HOME without
-- them, from an Omarchy with no /etc/skel: not an error).
for _, m in ipairs({ "hypr.input", "hypr.bindings", "hypr.looknfeel" }) do
  local ok, err = true, nil
  if package.searchpath(m, package.path) then ok, err = pcall(require, m) end
  if not ok then
    local run = os.getenv("XDG_RUNTIME_DIR") or ""
    local log = io.open(run .. "/hypr/" .. (os.getenv("HYPRLAND_INSTANCE_SIGNATURE") or "") .. "/hyprland.log", "a")
    if log then log:write("ERR from omabox ]: require(\"" .. m .. "\"): " .. tostring(err):gsub("%s+", " ") .. "\n"); log:close() end
  end
end
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
-- As Omarchy's stock ~/.config/hypr/hyprland.lua ends, after the user modules (above): the toggle
-- flags and workspace layouts from ~/.local/state, so Omarchy's toggles work in a box.
require("default.hypr.toggles")
