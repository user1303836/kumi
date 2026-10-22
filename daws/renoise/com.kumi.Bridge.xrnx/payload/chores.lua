-- Scripts kept for a repeated chore (#401): a chunk Kumi wrote and ran once, kept once the producer said yes, as a file in
-- Kumi's bridge folder (bridges/renoise/scripts), an entry in the Tools menu (Tools > Kumi > its name) and a keyboard shortcut
-- the producer can set (Global > Kumi > its name, in Renoise's Keys preferences). A run is one callback: one step of Renoise's
-- undo. The bridge registers the kept ones when it starts; removing one takes its entry, its shortcut and its file away.

local import = ...
local log = import("log")

local M = {}

M.MOST_CODE = 200 * 1024
local MOST_NAME = 80
local MOST_DESCRIPTION = 500
-- The first line of a kept file, which names it.
local HEADER = "-- Kumi: "

-- Kumi's folder of kept scripts: beside the discovery folder, in Kumi's own bridge folder (not Renoise's versioned
-- preferences, nor the tool's bundle, which an update replaces).
function M.folder(cfg)
  local discovery = cfg and cfg.discovery_dir
  if type(discovery) ~= "string" then return nil end
  local root = discovery:match("^(.*)[/\\][^/\\]+[/\\]?$")
  return root and (root .. "/scripts") or nil
end

local function bad(message) error({ code = "bad-params", message = message }, 0) end
local function fail(message) error({ code = "failed", message = message }, 0) end

local function slug_of(params)
  local slug = type(params) == "table" and params.slug or nil
  if type(slug) ~= "string" or slug == "" or #slug > 64 or slug:find("[^a-z0-9%-]") then bad("slug: a-z, 0-9 and -, 1 to 64 of them") end
  return slug
end

local function line_of(params, key, most, required)
  local text = params[key]
  if text == nil and not required then return "" end
  if type(text) ~= "string" then bad(key .. ": one line of text") end
  text = text:gsub("^%s+", ""):gsub("%s+$", "")
  if (required and text == "") or #text > most * 4 or text:find("%c") then bad(string.format("%s: one line of at most %d characters", key, most)) end
  return text
end

-- Its label in the menu and the Keys preferences: Renoise splits those names at ':'.
local function label(name) return (name:gsub(":", " -")) end
function M.entry(name) return "Main Menu:Tools:Kumi:" .. label(name) end
function M.binding(name) return "Global:Kumi:" .. label(name) end

-- The file Renoise runs: what Kumi's run-lua gives a chunk (`song`), then the chunk.
local function wrapped(name, description, code)
  local about = description ~= "" and ("-- " .. description .. "\n") or ""
  return HEADER .. name .. "\n" .. about .. "-- Kept by Kumi (its scripts tool removes it). A run is one step of Renoise's undo.\n"
    .. "local song = renoise.song()\n" .. code .. "\n"
end

-- What a menu entry or a shortcut runs: the file as it is now, an error on the status bar and Renoise's console.
local function runner(path, name)
  return function()
    local file = io.open(path, "rb")
    if not file then
      renoise.app():show_status("Kumi: " .. name .. " is no longer kept")
      return
    end
    local text = file:read("*a")
    file:close()
    local chunk, err = loadstring(text, "=" .. name)
    local ok = chunk ~= nil
    if chunk then ok, err = pcall(chunk) end
    if not ok then
      print("Kumi: " .. name .. ": " .. tostring(err))
      renoise.app():show_status("Kumi: " .. name .. " stopped: " .. tostring(err))
    end
  end
end

-- Its menu entry and shortcut, made again (a reload's closures replaced by this payload's).
local function register(path, name)
  local tool = renoise.tool()
  local entry, binding = M.entry(name), M.binding(name)
  if tool:has_menu_entry(entry) then tool:remove_menu_entry(entry) end
  tool:add_menu_entry({ name = entry, invoke = runner(path, name) })
  if tool:has_keybinding(binding) then tool:remove_keybinding(binding) end
  tool:add_keybinding({ name = binding, invoke = function(repeated) if not repeated then runner(path, name)() end end })
end

local function unregister(name)
  local tool = renoise.tool()
  local entry, binding = M.entry(name), M.binding(name)
  if tool:has_menu_entry(entry) then tool:remove_menu_entry(entry) end
  if tool:has_keybinding(binding) then tool:remove_keybinding(binding) end
end

-- A kept file's name, from its first line.
local function name_in(path)
  local file = io.open(path, "rb")
  if not file then return nil end
  local first = file:read("*l") or ""
  file:close()
  local name = first:sub(1, #HEADER) == HEADER and first:sub(#HEADER + 1):gsub("%s+$", "") or nil
  return name ~= "" and name or nil
end

-- Every kept script registered: when the bridge starts. What doesn't register is logged, never raised.
function M.register_all(cfg)
  local dir = M.folder(cfg)
  if not dir or not io.exists(dir) then return 0 end
  local count = 0
  for _, file in ipairs(os.filenames(dir, { "*.lua" })) do
    local path = dir .. "/" .. file
    local name = name_in(path)
    if name then
      local ok, err = pcall(register, path, name)
      if ok then count = count + 1 else log.write("error", "kept script " .. file .. ": " .. tostring(err)) end
    end
  end
  return count
end

-- `script_install {slug, name, description?, code, replace?}`: kept, in the menu and the Keys preferences.
function M.install(params, cfg)
  local slug = slug_of(params)
  local name = line_of(params, "name", MOST_NAME, true)
  local description = line_of(params, "description", MOST_DESCRIPTION, false)
  local code = params.code
  if type(code) ~= "string" or not code:find("%S") or #code > M.MOST_CODE then bad("code: the Lua to keep, at most 200 KB") end
  if not loadstring(code, "=" .. name) then bad("the Lua doesn't compile: run it once with run_lua first") end
  local dir = M.folder(cfg)
  if not dir then return fail("Kumi's bridge folder isn't known (the bridge has no reference): nothing was kept") end
  if not io.exists(dir) then os.mkdir(dir) end
  local path = dir .. "/" .. slug .. ".lua"
  local before = io.exists(path) and name_in(path) or nil
  if io.exists(path) and params.replace ~= true then fail("a script is kept as " .. path .. " already: replace: true writes over it") end
  local file, err = io.open(path, "wb")
  if not file then return fail("couldn't write " .. path .. ": " .. tostring(err)) end
  file:write(wrapped(name, description, code))
  file:close()
  if before and before ~= name then unregister(before) end
  register(path, name)
  local menu, shortcut = "Tools > Kumi > " .. label(name), "Global > Kumi > " .. label(name)
  return { file = path, menu = menu, shortcut = shortcut,
    where = menu .. " in Renoise's menu, and a shortcut to set in its Keys preferences (" .. shortcut .. ")" }
end

-- `script_remove {slug}`: its menu entry, shortcut and file taken away.
function M.remove(params, cfg)
  local slug = slug_of(params)
  local dir = M.folder(cfg)
  local path = dir and (dir .. "/" .. slug .. ".lua")
  if not path or not io.exists(path) then error({ code = "gone", message = "no script is kept as " .. tostring(path) }, 0) end
  local name = name_in(path)
  if name then unregister(name) end
  local ok, err = os.remove(path)
  if not ok then fail("couldn't delete " .. path .. ": " .. tostring(err)) end
  return { removed = path }
end

return M
