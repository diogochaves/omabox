-- `omabox lua` (issue #40, NOTES finding 106): an agent's Lua, evaluated in the box's Hyprland, and
-- what it returns handed back. `hyprctl eval` answers only "ok" or "error: MESSAGE", so the answer
-- travels as the message of an error raised here on purpose: "omabox-lua-ok:" and a JSON array of
-- the values returned, or "omabox-lua-error:" and the Lua error. No file, so calls at once never
-- meet. JSON escapes every control character, NUL included (the message is a C string on the way).
-- The host sends this file as the body of a function called with the source as its one argument.
local src = ...
local OK, ERR = "omabox-lua-ok:", "omabox-lua-error:"

-- An expression (`hl.get_cursor_pos()`) or statements (`local w = ...; return w.title`), as
-- hyprctl eval itself takes them. The statement's error is the one said when neither compiles.
local fn, err = load("return " .. src, "=lua", "t")
if not fn then fn, err = load(src, "=lua", "t") end
if not fn then error(ERR .. err, 0) end
local res = table.pack(pcall(fn))
if not res[1] then error(ERR .. tostring(res[2]), 0) end

-- Hyprland's objects (a window, a workspace, a monitor) are userdata whose fields cannot be listed
-- from Lua; Hyprland's own type stubs name them (`---@class HL.Window`, `---@field address string`).
-- Read once per Hyprland, on the first userdata met; without the stubs a userdata is its tostring.
local function class_fields(name)
  if omabox_lua_fields == nil then
    omabox_lua_fields = {}
    local f = io.open("/usr/share/hypr/stubs/hl.meta.lua")
    if f then
      local cls
      for line in f:lines() do
        local c = line:match("^%-%-%-@class%s+([%w_.]+)")
        if c then
          cls = c; omabox_lua_fields[c] = {}
        elseif cls then
          local k, t = line:match("^%-%-%-@field%s+([%w_]+)%??%s+(.*)$")
          if k and not t:match("^fun%(") then table.insert(omabox_lua_fields[cls], k)
          elseif not line:match("^%-%-%-") then cls = nil end
        end
      end
      f:close()
    end
  end
  return omabox_lua_fields[name]
end

local function str(s)
  local esc = { ['"'] = '\\"', ["\\"] = "\\\\", ["\n"] = "\\n", ["\t"] = "\\t", ["\r"] = "\\r" }
  return '"' .. s:gsub('[%c"\\]', function(c) return esc[c] or string.format("\\u%04x", c:byte()) end) .. '"'
end

local function num(v)
  if math.type(v) == "integer" then return string.format("%d", v) end
  if v ~= v or v == math.huge or v == -math.huge then return str(tostring(v)) end   -- no JSON for these
  return tostring(v)   -- as Lua prints it: 960.0, 0.1, 1e+300
end

-- A table on the current path again is a cycle; a userdata inside an expanded one (a window's
-- workspace, that workspace's monitor...) is its tostring, as `hyprctl -j clients` names them.
local path = {}
local function enc(v, depth, expand)
  local t = type(v)
  if t == "nil" then return "null"
  elseif t == "boolean" then return tostring(v)
  elseif t == "number" then return num(v)
  elseif t == "string" then return str(v)
  elseif t == "table" then
    if path[v] then return str("<cycle>") end
    if depth >= 32 then return str("<too deep>") end
    path[v] = true
    local keys, n = {}, 0
    for k in pairs(v) do n = n + 1; keys[n] = k end
    local seq = n == #v
    for _, k in ipairs(keys) do
      if math.type(k) ~= "integer" or k < 1 or k > n then seq = false; break end
    end
    local out = {}
    if seq then
      for i = 1, n do out[i] = enc(v[i], depth + 1, expand) end
      path[v] = nil
      return "[" .. table.concat(out, ",") .. "]"
    end
    table.sort(keys, function(a, b) return tostring(a) < tostring(b) end)
    for i, k in ipairs(keys) do out[i] = str(tostring(k)) .. ":" .. enc(v[k], depth + 1, expand) end
    path[v] = nil
    return "{" .. table.concat(out, ",") .. "}"
  elseif t == "userdata" and expand then
    local mt = getmetatable(v)
    local fields = type(mt) == "table" and type(rawget(mt, "__name")) == "string" and class_fields(rawget(mt, "__name"))
    if not fields then return str(tostring(v)) end
    local out = {}
    for _, k in ipairs(fields) do
      local ok, fv = pcall(function() return v[k] end)
      if ok and type(fv) ~= "function" then out[#out + 1] = str(k) .. ":" .. enc(fv, depth + 1, false) end
    end
    return "{" .. table.concat(out, ",") .. "}"
  end
  return str(tostring(v))   -- a function, a thread, a userdata not expanded
end

local out = {}
for i = 2, res.n do out[i - 1] = enc(res[i], 0, true) end
error(OK .. "[" .. table.concat(out, ",") .. "]", 0)
