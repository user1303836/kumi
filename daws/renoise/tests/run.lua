-- Runs the bridge's Lua tests on stock LuaJIT against the mock Renoise API.
-- Usage: luajit run.lua <temp dir> [name filter]

local tests_dir = arg[0]:match("^(.*[/\\])") or "./"
local temp_root = assert(arg[1], "usage: luajit run.lua <temp dir> [filter]")
local filter = arg[2]

local T = {
  tests_dir = tests_dir,
  tool_dir = tests_dir .. "../com.kumi.Bridge.xrnx/",
  temp_root = temp_root,
  list = {},
}

function T.test(name, fn)
  T.list[#T.list + 1] = { name = name, fn = fn }
end

local function show(v)
  if type(v) == "string" then return string.format("%q", v) end
  return tostring(v)
end

function T.eq(actual, expected, message)
  if actual ~= expected then
    error((message and message .. ": " or "") .. "expected " .. show(expected) .. ", got " .. show(actual), 2)
  end
end

function T.ok(value, message)
  if not value then error(message or "expected a true value", 2) end
  return value
end

function T.raises(fn, pattern)
  local ok, err = pcall(fn)
  if ok then error("expected an error", 2) end
  if pattern and not tostring(err):find(pattern) then
    error("error " .. show(tostring(err)) .. " doesn't match " .. show(pattern), 2)
  end
end

-- Numbers match within a few ulps: libm's log10/pow differ in the last bits between macOS (where the vectors are
-- blessed) and Linux CI.
local function close(a, b)
  return type(a) == "number" and math.abs(a - b) <= 1e-12 * math.max(1, math.abs(a), math.abs(b))
end

-- Deep equality for decoded JSON.
function T.same(a, b, path)
  path = path or "value"
  if type(a) ~= type(b) then error(path .. ": " .. type(a) .. " vs " .. type(b), 2) end
  if type(a) ~= "table" then
    if a ~= b and not close(a, b) then error(path .. ": " .. show(a) .. " vs " .. show(b), 2) end
    return
  end
  for k, v in pairs(a) do T.same(v, b[k], path .. "." .. tostring(k)) end
  for k in pairs(b) do
    if a[k] == nil then error(path .. "." .. tostring(k) .. " is missing", 2) end
  end
end

local counter = 0
function T.temp_dir(name)
  counter = counter + 1
  local dir = temp_root .. "/" .. counter .. "-" .. (name or "t"):gsub("[^%w]", "-")
  assert(os.execute("mkdir -p '" .. dir .. "'") == 0)
  return dir
end

-- Payload modules share one instance each, as they do in main.lua.
local loaded = {}
function T.load_payload_module(name)
  local function import(module)
    if loaded[module] == nil then
      loaded[module] = assert(loadfile(T.tool_dir .. "payload/" .. module .. ".lua"))(import)
    end
    return loaded[module]
  end
  return import(name)
end

-- A payload module loaded on its own as a Lua 5.1 built with a 32-bit long runs it, as on Windows: string.format's
-- integer conversions (%d, %i, %c, %o, %u, %x, %X) and tonumber(text, base) go through a long there, and a number one
-- can't hold is undefined (MSVC prints %d of 2^31 as -2147483648). Here it fails, by name. The modules it imports are
-- the usual ones.
function T.load_with_32_bit_long(name)
  local real_format, real_tonumber = string.format, tonumber
  local function fits(value, unsigned)
    if type(value) ~= "number" then return true end
    if unsigned then return value >= 0 and value < 2 ^ 32 end
    return value >= -2 ^ 31 and value < 2 ^ 31
  end
  local function format(spec, ...)
    local at, n = 1, 0
    while true do
      local percent = spec:find("%", at, true)
      if not percent then break end
      if spec:sub(percent + 1, percent + 1) == "%" then
        at = percent + 2
      else
        local _, last, conversion = spec:find("^[-+ #0]*%d*%.?%d*(%a)", percent + 1)
        n = n + 1
        if conversion and conversion:find("^[dic]$") and not fits(select(n, ...), false) or
          conversion and conversion:find("^[ouxX]$") and not fits(select(n, ...), true) then
          error(real_format("a 32-bit long can't hold %.17g (string.format %%%s)", select(n, ...), conversion), 2)
        end
        at = (last or percent) + 1
      end
    end
    return real_format(spec, ...)
  end
  local function tonumber(text, base)
    local value = real_tonumber(text, base)
    if base and base ~= 10 and value and not fits(value, true) then
      error(real_format("a 32-bit long can't hold %s (tonumber in base %d)", tostring(text), base), 2)
    end
    return value
  end
  local chunk = assert(loadfile(T.tool_dir .. "payload/" .. name .. ".lua"))
  setfenv(chunk, setmetatable({ string = setmetatable({ format = format }, { __index = string }), tonumber = tonumber }, { __index = _G }))
  return chunk(T.load_payload_module)
end

T.mock = assert(loadfile(tests_dir .. "mock_renoise.lua"))(tests_dir)

for _, file in ipairs({ "test_json.lua", "test_sha256.lua", "test_wire.lua", "test_vectors.lua", "test_mock.lua", "test_bridge.lua", "test_reads.lua", "test_retire.lua", "test_changes.lua", "test_review.lua",
  -- M4 (n-song)
  "test_song.lua", "test_track_settings.lua", "test_patterns.lua", "test_sequence.lua", "test_automation.lua", "test_lines.lua", "test_actions.lua", "test_matrix_m4.lua", "test_solo.lua",
  -- n-instruments
  "test_instruments.lua", "test_samples.lua", "test_modfx.lua", "test_phrases.lua", "test_instrument_reads.lua", "test_matrix_instruments.lua", "test_fences_instruments.lua", "test_continuity_instruments.lua",
  -- n-devices
  "test_devices_m4.lua",
  -- #401: scripts kept for a chore
  "test_chores.lua",
  -- M5 (listening)
  "test_listening.lua", "test_scratch.lua",
  -- M6 (m6-library)
  "test_kit.lua",
  -- M6 (m6-catchup)
  "test_digest.lua",
  -- M7 (variations)
  "test_variations.lua" }) do
  local chunk = loadfile(tests_dir .. file)
  if chunk then chunk(T) end
end

local passed, failed = 0, 0
for _, t in ipairs(T.list) do
  if not filter or t.name:find(filter, 1, true) then
    local ok, err = xpcall(t.fn, debug.traceback)
    if ok and T.mock.crashes and #T.mock.crashes > 0 then
      ok, err = false, "a call that ends Renoise's server or crashes Renoise: " .. table.concat(T.mock.crashes, " | ")
    end
    if ok then
      passed = passed + 1
    else
      failed = failed + 1
      io.write("FAIL ", t.name, "\n  ", tostring(err):gsub("\n", "\n  "), "\n")
    end
  end
end
io.write(string.format("%d passed, %d failed\n", passed, failed))
os.exit(failed == 0 and 0 or 1)
