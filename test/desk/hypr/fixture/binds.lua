-- t_desk_lua's binds (#321): a bind that names a command is never judged (the lock's stub runs as
-- on the desk), and a bind's Lua after the start runs as asked (phase later).
local bin = os.getenv("HOME") .. "/.config/hypr/fixture/bin"
hl.bind("SUPER + F7", hl.dsp.exec_cmd(bin .. "/hyprlock"))
hl.bind("SUPER + F8", function() hl.exec_cmd(bin .. "/hypridle --from-bind") end)
