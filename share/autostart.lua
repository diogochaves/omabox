-- A desk box's autostart policy (#321, finding 702): what of a command the user's Hyprland config
-- starts runs in the box. Pure Lua, nothing of Hyprland's (the suite runs it on the host's lua):
-- desk.lua asks judge() for each command it would run (hl.exec_cmd, os.execute, io.popen) and logs
-- the answer to ~/autostart.log with line().
--
-- A command is split into its parts (&& || ; | & and newlines, outside quotes); each part is read as
-- its words, past what only wraps another command: VAR=x, exec, env, setsid, nohup, uwsm-app --,
-- `uwsm app --`, app2unit --, systemd-run --user ... --; `sh -c '...'` (bash, dash, zsh) is judged
-- by what it runs. The program is told by its name (argv0's basename), never by a substring: a
-- `myhypridle-watcher` is not hypridle. Any part dropped drops the whole command; the log names it.
local M = {}

-- What a box drops, and why, by class.
local WHY = {
  idle = "idle: a box has no input and would lock itself (finding 10)",
  lock = "lock: a box takes no real password, and nothing would unlock it",
  session = "the user's session: a box has no user manager (up --systemd gives one)",
  dbus_env = "the session's environment: the box's own session-env.sh sends it",
  sysbus = "a system-bus applet: a box has no system bus",
  polkit = "a polkit agent: a box has no system bus (up --polkit runs one)",
  box = "the box runs its own",
  omarchy = "Omarchy's own session work, not a box's",
  slot = "the box's shell: up --shell starts it in the shell slot",
  omarchy_shell = "Omarchy's shell: the box's shell.sh starts it",
}

-- By program name: its class, and (for a class that drops) why. A function takes the words and the
-- environment and returns class, why (nil: runs).
local NAMES = {}
local function name(class, why, ...)
  for _, n in ipairs({ ... }) do NAMES[n] = { class, why } end
end
name("idle", WHY.idle, "hypridle", "swayidle", "xidlehook")
name("lock", WHY.lock, "hyprlock", "swaylock", "gtklock")
name("session", WHY.dbus_env, "dbus-update-activation-environment")
name("sysbus", WHY.sysbus, "nm-applet", "blueman-applet", "udiskie", "omarchy-powerprofiles-init", "kdeconnectd",
  "kdeconnect-indicator")
name("box", WHY.box, "gnome-keyring-daemon", "pipewire", "pipewire-pulse", "wireplumber")
name("omarchy", WHY.omarchy, "omarchy-provision-first-run", "omarchy-hyprland-monitor-watch")
name("shell", nil, "qs", "quickshell", "noctalia-shell", "waybar", "ags", "astal", "hyprpanel", "eww", "ironbar",
  "omarchy-launch-shell")
name("look", nil, "hyprpaper", "swaybg", "swww-daemon", "swww", "awww-daemon", "mpvpaper", "wpaperd", "hyprsunset",
  "mako", "dunst", "swaync", "fnott", "hyprctl")

-- The words of a part as a shell reads them: quotes and backslashes taken off. Good enough for a
-- config's commands; not a shell (no expansion, no redirections understood).
local function words(s)
  local out, cur, have, i, n = {}, {}, false, 1, #s
  while i <= n do
    local c = s:sub(i, i)
    if c == "'" then
      local j = s:find("'", i + 1, true) or (n + 1)
      table.insert(cur, s:sub(i + 1, j - 1)); have = true; i = j + 1
    elseif c == '"' then
      local j = i + 1
      while j <= n and s:sub(j, j) ~= '"' do
        if s:sub(j, j) == "\\" then j = j + 1 end
        table.insert(cur, s:sub(j, j)); j = j + 1
      end
      have = true; i = j + 1
    elseif c == "\\" then
      table.insert(cur, s:sub(i + 1, i + 1)); have = true; i = i + 2
    elseif c:match("%s") then
      if have then table.insert(out, table.concat(cur)); cur, have = {}, false end
      i = i + 1
    else
      table.insert(cur, c); have = true; i = i + 1
    end
  end
  if have then table.insert(out, table.concat(cur)) end
  return out
end
M.words = words

-- The parts of a command line: split on && || ; | & and newlines outside quotes (and outside a
-- $(...) or `...`, kept whole), each trimmed, the empty ones left out.
local function parts(s)
  local out, start, i, n, q, depth = {}, 1, 1, #s, nil, 0
  local function cut(at, skip)
    local p = s:sub(start, at - 1):match("^%s*(.-)%s*$")
    if p ~= "" then table.insert(out, p) end
    start = at + skip
  end
  while i <= n do
    local c = s:sub(i, i)
    if q then
      if c == "\\" and q == '"' then i = i + 1
      elseif c == q then q = nil end
    elseif c == "\\" then i = i + 1
    elseif c == "'" or c == '"' or c == "`" then q = c
    elseif s:sub(i, i + 1) == "$(" then depth = depth + 1; i = i + 1
    elseif c == ")" and depth > 0 then depth = depth - 1
    elseif depth == 0 then
      local two = s:sub(i, i + 1)
      if two == "&&" or two == "||" then cut(i, 2); i = i + 1
      elseif c == ";" or c == "|" or c == "&" or c == "\n" then cut(i, 1) end
    end
    i = i + 1
  end
  cut(n + 1, 0)
  return out
end
M.parts = parts

local function base(w) return (w or ""):match("([^/]*)$") end

-- A part's words past its wrappers: what actually runs. VAR=x, exec, env [VAR=x...], nohup, setsid
-- [options], uwsm-app [options] [--], `uwsm app [options] [--]`, app2unit [options] [--], systemd-run
-- [options] [--]; one inside another as often as written.
local function unwrap(w)
  local i = 1
  local function opts(stop_at_dashdash)
    while w[i] and w[i]:sub(1, 1) == "-" do
      if w[i] == "--" and stop_at_dashdash then i = i + 1; return end
      i = i + 1
    end
  end
  while w[i] do
    local b = base(w[i])
    if w[i]:match("^[%a_][%w_]*=") then i = i + 1
    elseif b == "exec" or b == "nohup" then i = i + 1
    elseif b == "env" then
      i = i + 1
      while w[i] and (w[i]:match("^[%a_][%w_]*=") or w[i]:sub(1, 1) == "-") do i = i + 1 end
    elseif b == "setsid" then i = i + 1; opts(true)
    elseif b == "uwsm-app" or b == "app2unit" then i = i + 1; opts(true)
    elseif b == "uwsm" and w[i + 1] == "app" then i = i + 2; opts(true)
    elseif b == "systemd-run" then
      i = i + 1
      -- (its options with a value: -p X, -u X, --unit X...; one form of each is enough here)
      while w[i] and w[i]:sub(1, 1) == "-" do
        if w[i] == "--" then i = i + 1; break end
        local o = w[i]
        i = i + 1
        if (o == "-p" or o == "-u" or o == "-E" or o == "-d" or o == "--unit" or o == "--property"
            or o == "--setenv" or o == "--slice" or o == "--description") and w[i] then i = i + 1 end
      end
    else break end
  end
  local out = {}
  for j = i, #w do table.insert(out, w[j]) end
  return out
end
M.unwrap = unwrap

-- The class of one part's program (words past wrappers), and why it drops (nil: it runs).
local function classify(w, env)
  local b = base(w[1])
  if b == "" then return "other", nil end
  if b == "systemctl" then
    for _, a in ipairs(w) do
      if a == "--user" then
        if env.OMABOX_SYSTEMD == "1" then return "session", nil end
        return "session", WHY.session
      end
    end
    return "other", nil
  end
  if b == "uwsm" and (w[2] == "start" or w[2] == "finalize") then
    if env.OMABOX_SYSTEMD == "1" then return "session", nil end
    return "session", WHY.session
  end
  if b == "loginctl" and w[2] == "lock-session" then return "lock", WHY.lock end
  if b == "hyprpm" and w[2] == "reload" then return "box", WHY.box end
  if b:match("^xdg%-desktop%-portal") then return "box", WHY.box end
  if b:match("polkit") then
    if env.OMABOX_POLKIT == "1" then return "sysbus", nil end
    return "sysbus", WHY.polkit
  end
  if b == "omarchy-hook" and w[2] == "post-boot" then return "omarchy", WHY.omarchy end
  if b == "herdr" and w[2] == "server" then return "omarchy", WHY.omarchy end
  if b == "dms" and w[2] == "run" then return "shell", nil end
  if b == "caelestia" and w[2] == "shell" then return "shell", nil end
  if b == "wl-paste" then
    for _, a in ipairs(w) do if a == "--watch" or a == "-w" then return "look", nil end end
    return "other", nil
  end
  if b == "omarchy-launch-shell" and env.OMABOX_DESK_BASE == "omarchy" then return "shell", WHY.omarchy_shell end
  local e = NAMES[b]
  if e then return e[1], e[2] end
  return "other", nil
end

-- The program's words as a comparable line: argv0's basename, then its arguments.
local function key(w)
  if #w == 0 then return "" end
  local k = { base(w[1]) }
  for j = 2, #w do table.insert(k, w[j]) end
  return table.concat(k, " ")
end

-- judge(cmd, via, env) -> verdict ("run" or "drop"), class, why, part: what the box does with CMD
-- (a string; anything else runs, unjudged), started through VIA (exec_cmd, execute or popen). ENV:
-- the box's OMABOX_SYSTEMD, OMABOX_POLKIT, OMABOX_SHELL_CMD and OMABOX_DESK_BASE (a table, or
-- os.getenv). CLASS is the first part's that is not `other` (a popen with none: `query`); PART, the
-- part that dropped it.
function M.judge(cmd, via, env, depth)
  env = env or {}
  if type(env) == "function" then
    local get = env
    env = setmetatable({}, { __index = function(_, k) return get(k) end })
  end
  depth = depth or 0
  if type(cmd) ~= "string" then return "run", "other", nil, nil end
  local slot = env.OMABOX_SHELL_CMD or ""
  local slot_key = slot ~= "" and key(unwrap(words(slot))) or nil
  local class
  for _, p in ipairs(parts(cmd)) do
    local w = unwrap(words(p))
    local b = base(w[1])
    -- sh -c 'CMD': judged by CMD (an option before -c, as `sh -ec`, is fine).
    local inner
    if (b == "sh" or b == "bash" or b == "dash" or b == "zsh") and depth < 4 then
      for j = 2, #w do
        if w[j]:match("^%-%a*c%a*$") then inner = w[j + 1]; break end
        if w[j]:sub(1, 1) ~= "-" then break end
      end
    end
    local v, c, why, part
    if inner then
      v, c, why, part = M.judge(inner, via, env, depth + 1)
      if c == "query" then c = "other" end
    else
      c, why = classify(w, env)
      if c == "shell" and slot_key and key(w) == slot_key then why = WHY.slot end
      v, part = why and "drop" or "run", p
    end
    if v == "drop" then return v, c, why, part end
    if c ~= "other" and not class then class = c end
  end
  if not class then class = via == "popen" and "query" or "other" end
  return "run", class, nil, nil
end

-- One line of ~/autostart.log: PHASE VERDICT CLASS VIA CMD, and (WHY) two spaces after it; a
-- dropped command's names the part that dropped it. Newlines in CMD shown as \n: one line each.
function M.line(phase, verdict, class, via, cmd, why, part)
  local c = tostring(cmd):gsub("\n", "\\n")
  local w = why
  if w and part and part ~= cmd then w = part:gsub("\n", "\\n") .. ": " .. w end
  return phase .. " " .. verdict .. " " .. (class or "-") .. " " .. (via or "-") .. " " .. c .. (w and ("  (" .. w .. ")") or "")
end

return M
