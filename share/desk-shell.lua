-- A desk box's shell slot (#321 step 4, NOTES finding 703): the user's own shell, as a desk-shells/
-- profile finds it, run in the box's slot (share/shell.sh: its pid, restart-shell, its crash) instead
-- of by the user's config. desk.lua calls take(cmd) for each command the user's config starts, before
-- its drop table, and after_start() once the user's config has run, before the box's environment is
-- written (omabox.env: `up` reads the slot's files once that is there).
--
-- take(cmd) -> taken, why: true when the slot took the command, so the config does not run it (why
-- says what became of it, for desk.lua's log); false for a command that starts no shell omabox knows.
-- The first shell the config starts runs in the slot; a second one is dropped (one bar, not two);
-- with `up --shell CMD` (OMABOX_SHELL_CMD) the slot runs CMD and the config's shell is dropped; with
-- `up --no-shell` it is dropped too.
-- after_start(): nothing taken, the slot starts the guess `up` made (OMABOX_DESK_SHELL: a profile,
-- found by its systemd unit, say, which the config's `systemctl --user` would have started) by its
-- `start`; no guess: the slot is marked none, and `up` says so.
--
-- What the slot runs goes in $XDG_RUNTIME_DIR/omabox-shell.cmd, written here before shell.sh starts
-- (no quoting on the way), and how it got there in omabox-desk-shell: `config`, `guess`, or `none`.
-- Both are the box's files: `up` and `ls` read them as such (plain, one line, bounded). A Hyprland
-- reload parses the config again: a command whose slot already runs (the file is there) is dropped,
-- not started twice.
--
-- M.dir: the profiles' folder (the suite's unit test points it at its own); M.exec: what starts the
-- slot, desk.lua's own unwrapped exec when it sets one (hl.exec_cmd otherwise).
local M = { dir = "/opt/omabox/share/desk-shells", exec = nil }

local function run_dir()
  return os.getenv("XDG_RUNTIME_DIR") or ""
end

-- words(s): the shell command s split into its parts (on && || ; | & and newlines), each a list of
-- its words, quotes taken off as sh takes them (single, double, a backslash). Enough to read what a
-- config starts; anything stranger reads as words that match no profile.
function M.parts(s)
  local parts, words, w, have = {}, {}, {}, false
  local i, n = 1, #s
  local function word()
    if have then words[#words + 1] = table.concat(w) end
    w, have = {}, false
  end
  local function part()
    word()
    if #words > 0 then parts[#parts + 1] = words end
    words = {}
  end
  while i <= n do
    local c = s:sub(i, i)
    if c == "'" then
      local j = s:find("'", i + 1, true) or (n + 1)
      w[#w + 1] = s:sub(i + 1, j - 1)
      have, i = true, j + 1
    elseif c == '"' then
      i = i + 1
      while i <= n and s:sub(i, i) ~= '"' do
        local d = s:sub(i, i)
        if d == "\\" and i < n and s:sub(i + 1, i + 1):match('[\\"$`]') then
          i = i + 1
          d = s:sub(i, i)
        end
        w[#w + 1] = d
        i = i + 1
      end
      have, i = true, i + 1
    elseif c == "\\" then
      if i < n and s:sub(i + 1, i + 1) ~= "\n" then w[#w + 1] = s:sub(i + 1, i + 1); have = true end
      i = i + 2
    elseif c == ";" or c == "|" or c == "&" or c == "\n" or c == "(" or c == ")" then
      part()
      i = i + 1
    elseif c:match("%s") then
      word()
      i = i + 1
    else
      w[#w + 1] = c
      have, i = true, i + 1
    end
  end
  part()
  return parts
end

local function base(w)
  return (w:gsub("^.*/", ""))
end

-- strip(words): what a part runs once what only starts it is taken off: A=B words, exec, env (its
-- -i, -u NAME and A=B), setsid and nohup, uwsm app / uwsm-app / app2unit (to their --), systemd-run
-- (to its --). A list of parts: sh -c / bash -c 'SCRIPT' is SCRIPT's parts, each stripped.
function M.strip(words)
  local i = 1
  local function opts()
    while words[i] and words[i]:sub(1, 1) == "-" and words[i] ~= "--" do i = i + 1 end
    if words[i] == "--" then i = i + 1 end
  end
  while words[i] do
    local w = words[i]
    local b = base(w)
    if w:match("^[%a_][%w_]*=") then
      i = i + 1
    elseif b == "exec" or b == "nohup" then
      i = i + 1
    elseif b == "setsid" then
      i = i + 1
      while words[i] and words[i]:sub(1, 1) == "-" do i = i + 1 end
    elseif b == "env" then
      i = i + 1
      while words[i] do
        local e = words[i]
        if e == "-u" or e == "--unset" then i = i + 2
        elseif e:sub(1, 1) == "-" or e:match("^[%a_][%w_]*=") then i = i + 1
        else break end
      end
    elseif b == "uwsm" and words[i + 1] == "app" then
      i = i + 2
      opts()
    elseif b == "uwsm-app" or b == "app2unit" then
      i = i + 1
      opts()
    elseif b == "systemd-run" then
      local j = i + 1
      while words[j] and words[j] ~= "--" do j = j + 1 end
      if not words[j] then break end
      i = j + 1
    elseif (b == "sh" or b == "bash" or b == "dash" or b == "zsh") and words[i + 1]
      and words[i + 1]:match("^%-%a*c%a*$") and words[i + 2] then
      local out = {}
      for _, p in ipairs(M.parts(words[i + 2])) do
        for _, q in ipairs(M.strip(p)) do out[#out + 1] = q end
      end
      return out
    else
      break
    end
  end
  local rest = {}
  for k = i, #words do rest[#rest + 1] = words[k] end
  if #rest == 0 then return {} end
  return { rest }
end

-- The profiles, read once: { name, start, pats = { {words}, wild } } in name order.
local loaded
function M.profiles()
  if loaded then return loaded end
  loaded = {}
  local dir = M.dir
  local ls = io.popen("ls -1 -- '" .. dir:gsub("'", "'\\''") .. "' 2>/dev/null")
  local names = {}
  if ls then
    for n in ls:lines() do
      if n:match("^[a-z0-9][a-z0-9-]*$") then names[#names + 1] = n end
    end
    ls:close()
  end
  table.sort(names)
  for _, name in ipairs(names) do
    local f = io.open(dir .. "/" .. name)
    if f then
      local p = { name = name, start = nil, pats = {} }
      for line in f:lines() do
        local key, val = line:match("^%s*(%S+)%s+(.-)%s*$")
        if key == "start" or key == "match" then
          if key == "start" then p.start = val end
          local ws = {}
          for x in val:gmatch("%S+") do ws[#ws + 1] = x end
          local wild = false
          for _, x in ipairs(ws) do if x:match("^{.*}$") then wild = true end end
          p.pats[#p.pats + 1] = { words = ws, wild = wild }
        end
      end
      f:close()
      loaded[#loaded + 1] = p
    end
  end
  return loaded
end

-- profile_for(cmd): the profile whose start or match a part of cmd begins with (its program by its
-- name, then its words in order, `{name}` any one word); a pattern with no `{...}` first, so a named
-- Quickshell config's own profile wins over the generic one. nil: none.
function M.profile_for(cmd)
  local runs = {}
  for _, p in ipairs(M.parts(cmd)) do
    for _, q in ipairs(M.strip(p)) do runs[#runs + 1] = q end
  end
  local function fits(argv, pat)
    if #argv < #pat or base(argv[1]) ~= pat[1] then return false end
    for k = 2, #pat do
      if not pat[k]:match("^{.*}$") and argv[k] ~= pat[k] then return false end
    end
    return true
  end
  for _, wild in ipairs({ false, true }) do
    for _, p in ipairs(M.profiles()) do
      for _, pat in ipairs(p.pats) do
        if pat.wild == wild then
          for _, argv in ipairs(runs) do
            if fits(argv, pat.words) then return p end
          end
        end
      end
    end
  end
  return nil
end

local function read(name)
  local f = io.open(run_dir() .. "/" .. name)
  if not f then return nil end
  local v = f:read("*a") or ""
  f:close()
  return v
end

-- (renamed into place: `up` never reads half of one)
local function write(name, text)
  local p = run_dir() .. "/" .. name
  local f = io.open(p .. ".new", "w")
  if not f then return false end
  f:write(text, "\n")
  f:close()
  return os.rename(p .. ".new", p)
end

local function one_line(s)
  s = (s or ""):gsub("[%c]+", " ")
  if #s > 200 then s = s:sub(1, 200) .. "..." end
  return s
end

local taken
local function start(cmd, from)
  taken = cmd
  write("omabox-shell.cmd", cmd)
  write("omabox-desk-shell", from)
  -- (`take`: shell.sh's token, which a desk box's slot needs to start from the file; the token of a
  -- restart-shell is its own)
  local exec = M.exec or hl.exec_cmd
  exec("/opt/omabox/share/shell.sh take")
end

function M.take(cmd)
  if type(cmd) ~= "string" then return false end
  local p = M.profile_for(cmd)
  if not p then return false end
  if os.getenv("OMABOX_SHELL") == "0" then return true, "up --no-shell: no shell" end
  local over = os.getenv("OMABOX_SHELL_CMD") or ""
  if over ~= "" then return true, "a second shell: the slot runs " .. one_line(over) .. " (up --shell)" end
  local now = taken or (read("omabox-shell.cmd") or ""):gsub("\n$", "")
  if now ~= "" then
    if now == cmd then return true, "the slot runs it already" end
    return true, "a second shell: the slot runs " .. one_line(now)
  end
  start(cmd, "config")
  return true, "the slot runs it (" .. p.name .. ")"
end

function M.after_start()
  if os.getenv("OMABOX_SHELL") == "0" or (os.getenv("OMABOX_SHELL_CMD") or "") ~= "" then return end
  if taken or read("omabox-shell.cmd") then return end
  local guess = os.getenv("OMABOX_DESK_SHELL") or ""
  for _, p in ipairs(M.profiles()) do
    if p.name == guess and p.start then
      start(p.start, "guess")
      return
    end
  end
  write("omabox-desk-shell", "none")
end

return M
