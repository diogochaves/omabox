-- t_desk_lua's desk (#321, finding 702): a Hyprland config of a desk without Omarchy, seeded at the
-- box HOME's ~/.config/hypr. Each thing it does is one the box keeps, overrides or drops. Its
-- programs are the stubs in fixture/bin, which only write a line to ~/fixture-ran.
require("fixture.binds")
local bin = os.getenv("HOME") .. "/.config/hypr/fixture/bin"
-- Kept: the user's look and input. Overridden by the box: the red bar, the cursor's buffer, and a
-- catch-all monitor rule (the box's screen is omabox's).
hl.config({
  general = { gaps_out = 17 },
  input = { follow_mouse = 0 },
  debug = { suppress_errors = false },
  cursor = { use_cpu_buffer = 1 },
})
hl.monitor({ output = "", mode = "3840x2160@60", position = "auto", scale = 2 })
-- At parse time, each load (load, then reload): run.
os.execute('echo parse >> "$HOME/fixture-ran"')
local p = io.popen("echo desk-popen")
omabox_t321_popen = p and p:read("l")
if p then p:close() end
-- Dropped, as a write: what is written goes nowhere, and the handle still takes it.
local w = io.popen(bin .. "/hypridle --from-popen", "w")
omabox_t321_popen_w = w and w:write("x") and "writable" or "not"
if w then w:close() end
-- At the start: the idle daemon dropped (through uwsm-app, as Omarchy's o.launch), the bar run.
hl.on("hyprland.start", function()
  hl.exec_cmd("uwsm-app -- " .. bin .. "/hypridle")
  hl.exec_cmd(bin .. "/fixture-bar")
end)
