-- JSON for the bridge: exact numbers, full escaping, valid UTF-8 out, explicit empty arrays,
-- and an encoder that can run in time slices.

local byte, char, find, format, sub, gsub = string.byte, string.char, string.find, string.format, string.sub, string.gsub
local concat, sort = table.concat, table.sort
local floor, huge = math.floor, math.huge
local type, pairs, next, tonumber, getmetatable, setmetatable = type, pairs, next, tonumber, getmetatable, setmetatable
local clock = os.clock

local M = {}

M.null = setmetatable({}, { __tostring = function() return "null" end })

local array_mt = { __jsontype = "array" }
local object_mt = { __jsontype = "object" }

-- Marks a table as a JSON array, so an empty one encodes as [] (an unmarked empty table is {}).
function M.array(t)
  return setmetatable(t or {}, array_mt)
end

function M.object(t)
  return setmetatable(t or {}, object_mt)
end

function M.is_array(t)
  return getmetatable(t) == array_mt
end

-- A text this encoder made of a value, put into a document as it is where the value would go. A page measures each row it
-- takes by encoding it once, and sends the texts: the frame then costs no second encoding of every row, and holds the same
-- bytes it did when the rows were encoded again. (The text is trusted as a whole: its own nesting isn't counted again.)
local raw_mt = { __jsontype = "raw" }
function M.raw(text)
  return setmetatable({ text = text }, raw_mt)
end

function M.is_raw(t)
  return getmetatable(t) == raw_mt
end

-- The deepest a document nests, the outermost object or array counting as the first level: the same limit as
-- crates/daw-bridge (MAX_JSON_DEPTH), so a line one side writes is never one the other refuses.
M.MAX_DEPTH = 64
-- The deepest a row encoded on its own (a page's item, a grid's row, a digest's record) may nest: its text goes into the
-- document as it is, where its own levels aren't counted again, and the document around it is at most this many levels
-- (the answer, items, a track's rows). A row deeper than this is sent as a table and counted with the rest.
M.ROW_DEPTH = M.MAX_DEPTH - 8

local REPLACEMENT = "\239\191\189"

-- Returns the end of the valid sequence that starts at i, or nil and the end of its maximal invalid
-- subpart (each subpart becomes one U+FFFD, as Rust's from_utf8_lossy does).
local function utf8_sequence(s, i, c)
  local need, lo, hi
  if c >= 0xC2 and c <= 0xDF then need, lo, hi = 1, 0x80, 0xBF
  elseif c == 0xE0 then need, lo, hi = 2, 0xA0, 0xBF
  elseif c == 0xED then need, lo, hi = 2, 0x80, 0x9F
  elseif c >= 0xE1 and c <= 0xEF then need, lo, hi = 2, 0x80, 0xBF
  elseif c == 0xF0 then need, lo, hi = 3, 0x90, 0xBF
  elseif c == 0xF4 then need, lo, hi = 3, 0x80, 0x8F
  elseif c >= 0xF1 and c <= 0xF3 then need, lo, hi = 3, 0x80, 0xBF
  else return nil, i end
  for k = 1, need do
    local d = byte(s, i + k)
    if not d or d < lo or d > hi then return nil, i + k - 1 end
    lo, hi = 0x80, 0xBF
  end
  return i + need
end

-- Valid UTF-8 comes back unchanged; each invalid part becomes U+FFFD.
local function repair_utf8(s)
  local i = find(s, "[\128-\255]")
  if not i then return s end
  local out, n, start = nil, 0, 1
  while i do
    local last, bad_end = utf8_sequence(s, i, byte(s, i))
    if last then
      i = find(s, "[\128-\255]", last + 1)
    else
      out = out or {}
      out[n + 1] = sub(s, start, i - 1)
      out[n + 2] = REPLACEMENT
      n = n + 2
      start = bad_end + 1
      i = find(s, "[\128-\255]", start)
    end
  end
  if not out then return s end
  out[n + 1] = sub(s, start)
  return concat(out)
end
M.repair_utf8 = repair_utf8

local function valid_utf8(s)
  local i = find(s, "[\128-\255]")
  while i do
    local last = utf8_sequence(s, i, byte(s, i))
    if not last then return false end
    i = find(s, "[\128-\255]", last + 1)
  end
  return true
end
M.valid_utf8 = valid_utf8

local ESCAPES = { ['"'] = '\\"', ["\\"] = "\\\\", ["\b"] = "\\b", ["\f"] = "\\f", ["\n"] = "\\n", ["\r"] = "\\r", ["\t"] = "\\t" }
for i = 0, 31 do
  local c = char(i)
  if not ESCAPES[c] then ESCAPES[c] = format("\\u%04x", i) end
end

local function encode_string(s)
  if find(s, '[%z\1-\31"\\\128-\255]') then
    s = gsub(repair_utf8(s), '[%z\1-\31"\\]', ESCAPES)
  end
  return '"' .. s .. '"'
end
M.encode_string = encode_string

local MAX_EXACT = 2 ^ 53

-- Integers print as integers; other numbers in the fewest digits (at most 17) that read back
-- exactly. NaN and infinities are null. (An integer by %.0f, not %d: a Lua 5.1 built with a 32-bit
-- long, as on Windows, formats %d through one, which can't hold 2^31 or more.)
local function encode_number(v)
  if v ~= v or v == huge or v == -huge then return "null" end
  if v == floor(v) and v >= -MAX_EXACT and v <= MAX_EXACT then
    if v == 0 then return "0" end
    return format("%.0f", v)
  end
  local s = format("%.15g", v)
  if tonumber(s) ~= v then
    s = format("%.16g", v)
    if tonumber(s) ~= v then s = format("%.17g", v) end
  end
  return s
end
M.encode_number = encode_number

local function encode_error(message)
  error({ json_error = message }, 0)
end

-- A list has the keys 1 to #t and no others: a hole would make # unreliable and lose items silently.
local function check_list(t, n)
  local count = 0
  for k in pairs(t) do
    if type(k) ~= "number" or k < 1 or k > n or k ~= floor(k) then
      encode_error("table mixes list items and keys")
    end
    count = count + 1
  end
  if count ~= n then encode_error("list has holes") end
end

-- "array" or "object"; errors on mixed or sparse tables (marked as lists or not).
local function table_kind(t)
  local mt = getmetatable(t)
  if mt == object_mt then return "object" end
  local n = #t
  if mt == array_mt then
    check_list(t, n)
    return "array"
  end
  if n == 0 then
    local k = next(t)
    if k == nil or type(k) == "string" then return "object" end
    encode_error("table has keys that are not strings")
  end
  check_list(t, n)
  return "array"
end

local SLICE_CHECK_EVERY = 256

local function new_encoder(options)
  local buf, n = {}, 0
  local sort_keys = options and options.sort_keys
  local max_depth = options and options.max_depth or M.MAX_DEPTH
  local on_tick = options and options.on_tick
  local ticks = 0

  local encode_value

  local function encode_table(t, depth)
    if depth > max_depth then encode_error("too deeply nested") end
    if table_kind(t) == "array" then
      local count = #t
      if count == 0 then n = n + 1; buf[n] = "[]"; return end
      n = n + 1; buf[n] = "["
      for i = 1, count do
        if i > 1 then n = n + 1; buf[n] = "," end
        encode_value(t[i], depth + 1)
      end
      n = n + 1; buf[n] = "]"
      return
    end
    local keys, count = {}, 0
    for k in pairs(t) do
      if type(k) ~= "string" then encode_error("object key is not a string") end
      count = count + 1
      keys[count] = k
    end
    if count == 0 then n = n + 1; buf[n] = "{}"; return end
    if sort_keys then sort(keys) end
    n = n + 1; buf[n] = "{"
    for i = 1, count do
      local k = keys[i]
      if i > 1 then n = n + 1; buf[n] = "," end
      n = n + 1; buf[n] = encode_string(k)
      n = n + 1; buf[n] = ":"
      encode_value(t[k], depth + 1)
    end
    n = n + 1; buf[n] = "}"
  end

  encode_value = function(v, depth)
    if on_tick then
      ticks = ticks + 1
      if ticks >= SLICE_CHECK_EVERY then ticks = 0; on_tick() end
    end
    local kind = type(v)
    if kind == "string" then
      n = n + 1; buf[n] = encode_string(v)
    elseif kind == "number" then
      n = n + 1; buf[n] = encode_number(v)
    elseif kind == "boolean" then
      n = n + 1; buf[n] = v and "true" or "false"
    elseif v == nil or v == M.null then
      n = n + 1; buf[n] = "null"
    elseif kind == "table" then
      if getmetatable(v) == raw_mt then
        n = n + 1; buf[n] = v.text
      else
        encode_table(v, depth)
      end
    else
      encode_error("cannot encode a " .. kind)
    end
  end

  return function(value)
    encode_value(value, 1)
    return concat(buf, "", 1, n)
  end
end

local function unwrap_error(err)
  if type(err) == "table" and err.json_error then return err.json_error end
  error(err, 0)
end

-- Returns the text, or nil and why.
function M.encode(value, options)
  local ok, result = pcall(new_encoder(options), value)
  if ok then return result end
  return nil, unwrap_error(result)
end

-- An encoder that runs a little at a time: job:step(budget_ms) returns true and the text (or
-- false and nil while it has more to do, or nil and an error).
function M.sliced_encoder(value, options)
  local deadline = 0
  local opts = {}
  for k, v in pairs(options or {}) do opts[k] = v end
  opts.on_tick = function()
    if clock() >= deadline then coroutine.yield() end
  end
  local encode = new_encoder(opts)
  local co = coroutine.create(function() return encode(value) end)
  local job = { slices = 0 }
  function job.step(_, budget_ms)
    deadline = clock() + (budget_ms or 10) / 1000
    job.slices = job.slices + 1
    local ok, result = coroutine.resume(co)
    if not ok then
      if type(result) == "table" and result.json_error then return nil, result.json_error end
      return nil, tostring(result)
    end
    if coroutine.status(co) == "dead" then return true, result end
    return false, nil
  end
  return job
end

-- Decoding

local function decode_error(s, i, message)
  error({ json_error = format("%s at byte %d", message, i) }, 0)
end

local ESCAPE_CHARS = { [34] = '"', [92] = "\\", [47] = "/", [98] = "\b", [102] = "\f", [110] = "\n", [114] = "\r", [116] = "\t" }

local function utf8_char(cp)
  if cp < 0x80 then return char(cp) end
  if cp < 0x800 then return char(0xC0 + floor(cp / 64), 0x80 + cp % 64) end
  if cp < 0x10000 then return char(0xE0 + floor(cp / 4096), 0x80 + floor(cp / 64) % 64, 0x80 + cp % 64) end
  return char(0xF0 + floor(cp / 262144), 0x80 + floor(cp / 4096) % 64, 0x80 + floor(cp / 64) % 64, 0x80 + cp % 64)
end

local function read_hex4(s, i)
  local h = sub(s, i, i + 3)
  if not find(h, "^%x%x%x%x$") then decode_error(s, i, "invalid \\u escape") end
  return tonumber(h, 16)
end

local function parse_string(s, i)
  local parts, n = nil, 0
  local j = i + 1
  while true do
    local k = find(s, '[%z\1-\31"\\]', j)
    if not k then decode_error(s, i, "unterminated string") end
    local c = byte(s, k)
    if c == 34 then
      local tail = sub(s, j, k - 1)
      if not valid_utf8(tail) then decode_error(s, j, "invalid UTF-8") end
      if not parts then return tail, k + 1 end
      parts[n + 1] = tail
      return concat(parts, "", 1, n + 1), k + 1
    elseif c == 92 then
      parts = parts or {}
      local chunk = sub(s, j, k - 1)
      if not valid_utf8(chunk) then decode_error(s, j, "invalid UTF-8") end
      n = n + 1; parts[n] = chunk
      local e = byte(s, k + 1)
      if e == 117 then
        local cp = read_hex4(s, k + 2)
        j = k + 6
        -- A surrogate escape must be half of a pair: a lone one is refused, as serde_json refuses it.
        if cp >= 0xD800 and cp <= 0xDBFF then
          local low = byte(s, j) == 92 and byte(s, j + 1) == 117 and read_hex4(s, j + 2)
          if not (low and low >= 0xDC00 and low <= 0xDFFF) then decode_error(s, k, "lone surrogate") end
          cp = 0x10000 + (cp - 0xD800) * 1024 + (low - 0xDC00)
          j = j + 6
        elseif cp >= 0xDC00 and cp <= 0xDFFF then
          decode_error(s, k, "lone surrogate")
        end
        n = n + 1; parts[n] = utf8_char(cp)
      else
        local r = ESCAPE_CHARS[e]
        if not r then decode_error(s, k, "invalid escape") end
        n = n + 1; parts[n] = r
        j = k + 2
      end
    else
      decode_error(s, k, "control character in string")
    end
  end
end

local function parse_number(s, i)
  local j = i
  if byte(s, j) == 45 then j = j + 1 end
  local c = byte(s, j)
  if c == 48 then
    j = j + 1
  elseif c and c >= 49 and c <= 57 then
    local _, e = find(s, "^%d+", j)
    j = e + 1
  else
    decode_error(s, i, "invalid number")
  end
  if byte(s, j) == 46 then
    local _, e = find(s, "^%d+", j + 1)
    if not e then decode_error(s, j, "invalid number") end
    j = e + 1
  end
  c = byte(s, j)
  if c == 101 or c == 69 then
    j = j + 1
    c = byte(s, j)
    if c == 43 or c == 45 then j = j + 1 end
    local _, e = find(s, "^%d+", j)
    if not e then decode_error(s, j, "invalid number") end
    j = e + 1
  end
  local number = tonumber(sub(s, i, j - 1))
  if number == math.huge or number == -math.huge then decode_error(s, i, "number out of range") end
  return number, j
end

local parse_value

local function skip(s, i)
  return find(s, "[^ \t\r\n]", i) or #s + 1
end

local function parse_array(s, i, depth, null)
  local out, n = setmetatable({}, array_mt), 0
  i = skip(s, i + 1)
  if byte(s, i) == 93 then return out, i + 1 end
  while true do
    local v
    v, i = parse_value(s, i, depth + 1, null)
    n = n + 1; out[n] = v
    i = skip(s, i)
    local c = byte(s, i)
    if c == 93 then return out, i + 1 end
    if c ~= 44 then decode_error(s, i, "expected ',' or ']'") end
    i = skip(s, i + 1)
  end
end

-- top: when given, the document's own members that are numbers get their text recorded in it, by name.
local function parse_object(s, i, depth, null, top)
  local out = {}
  i = skip(s, i + 1)
  if byte(s, i) == 125 then return out, i + 1 end
  while true do
    if byte(s, i) ~= 34 then decode_error(s, i, "expected a string key") end
    local key
    key, i = parse_string(s, i)
    i = skip(s, i)
    if byte(s, i) ~= 58 then decode_error(s, i, "expected ':'") end
    local v
    local first = skip(s, i + 1)
    v, i = parse_value(s, first, depth + 1, null)
    out[key] = v
    if top then
      local lead = byte(s, first)
      top[key] = (lead == 45 or (lead and lead >= 48 and lead <= 57)) and sub(s, first, i - 1) or nil
    end
    i = skip(s, i)
    local c = byte(s, i)
    if c == 125 then return out, i + 1 end
    if c ~= 44 then decode_error(s, i, "expected ',' or '}'") end
    i = skip(s, i + 1)
  end
end

-- depth: the nesting level a container at i would have (the document's outermost is 1).
parse_value = function(s, i, depth, null, top)
  local c = byte(s, i)
  if c == 123 or c == 91 then
    if depth > M.MAX_DEPTH then decode_error(s, i, "too deeply nested") end
    if c == 123 then return parse_object(s, i, depth, null, top) end
    return parse_array(s, i, depth, null)
  end
  if c == 34 then return parse_string(s, i) end
  if c == 45 or (c and c >= 48 and c <= 57) then return parse_number(s, i) end
  if c == 116 and sub(s, i, i + 3) == "true" then return true, i + 4 end
  if c == 102 and sub(s, i, i + 4) == "false" then return false, i + 5 end
  if c == 110 and sub(s, i, i + 3) == "null" then return null, i + 4 end
  if not c then decode_error(s, i, "unexpected end") end
  decode_error(s, i, "unexpected character")
end

-- Returns the value, or nil and why. null decodes to json.null, or to nil with options.null_as_nil. With
-- options.numbers (a table), the number members of a top-level object are also recorded there as the text they were
-- written as, by name (1.0 and 1 are the same number to Lua, and not the same to a u64 field).
function M.decode(s, options)
  if type(s) ~= "string" then return nil, "not a string" end
  local null = (not (options and options.null_as_nil)) and M.null or nil
  local ok, value = pcall(function()
    local v, j = parse_value(s, skip(s, 1), 1, null, options and options.numbers)
    j = skip(s, j)
    if j <= #s then decode_error(s, j, "trailing characters") end
    return v, j
  end)
  if ok then return value end
  return nil, unwrap_error(value)
end

return M
