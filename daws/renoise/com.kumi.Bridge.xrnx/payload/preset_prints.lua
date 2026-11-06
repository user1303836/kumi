-- Pure preset fingerprints, from one immutable string. The synchronous paths retain Lua's fast substitutions; the
-- background path performs bounded byte steps, including finding and replacing the fields, without building a second
-- whole preset. Neither path reads Renoise. Keep the textual matching (including malformed XML), not an XML parser's
-- interpretation: the host's prints and existing change records use these exact bytes.
local M = {}
M.VIEW_STATE = { "IsMaximized", "ShowDevices", "CurrentlyVisibleChain", "MonitorVisibleChain", "PluginEditorWindowPosition" }
M.NUMBER_BYTES = 256

local ordinary = { "SelectedPresetIsModified", "IsSelected" }
for _, tag in ipairs(M.VIEW_STATE) do ordinary[#ordinary + 1] = tag end

function M.aims_inside(xml)
  for value in xml:gmatch("DestEffect>%s*<Value>(-?[%d%.]+)</Value>") do
    if tonumber(value) >= 0 then return true end
  end
  return false
end

local function plain_now(text)
  for _, tag in ipairs(ordinary) do text = text:gsub("<" .. tag .. ">[^<]*</" .. tag .. ">", "<" .. tag .. "/>") end
  return text
end

function M.shape_now(xml, print_)
  local shape = xml:gsub("<Value>[^<]*</Value>", "<Value/>"):gsub("<ParameterChunk>.-</ParameterChunk>", "<ParameterChunk/>")
  return print_(plain_now(shape))
end

function M.content_now(xml, print_)
  local content = M.aims_inside(xml) and xml:gsub("<Value>[^<]*</Value>", "<Value/>") or xml
  return print_(plain_now(content))
end

local function rules(values, chunks)
  local root = {}
  local function add(tag, chunk)
    local opening, node = "<" .. tag .. ">", root
    for k = 1, #opening do
      local c = opening:sub(k, k)
      node[c] = node[c] or {}
      node = node[c]
    end
    node.rule = { opening = opening, closing = "</" .. tag .. ">", replacement = "<" .. tag .. "/>", chunk = chunk }
  end
  if values then add("Value") end
  if chunks then add("ParameterChunk", true) end
  for _, tag in ipairs(ordinary) do add(tag) end
  return root
end
local shape_rules, content_rules, moving_rules = rules(true, true), rules(false, false), rules(true, false)

local M32, byte, format = 4294967296, string.byte, string.format
local function emit(s, c)
  local value = byte(c)
  s.a, s.b = (s.a * 33 + value) % M32, (s.b * 65599 + value) % M32
  s.length = s.length + 1
end

local function normalizer(xml, root)
  return { xml = xml, root = root, i = 1, mode = "scan", a = 5381, b = 0, length = 0 }
end

local function output(s, first, last, next_i)
  s.mode, s.out_i, s.out_last, s.next_i = "raw", first, last, next_i
end

-- Each step inspects or emits at most one byte. The trie, numeric conversion (at most NUMBER_BYTES), final format and
-- state changes are bounded independently of input length. Failed plain matches contain no '<' before their probe;
-- their raw prefix can therefore be emitted without overlooking another replacement.
local function normalize_step(s)
  local xml, n = s.xml, #s.xml
  if s.mode == "raw" then
    if s.out_i <= s.out_last then emit(s, xml:sub(s.out_i, s.out_i)) s.out_i = s.out_i + 1
    else s.mode, s.i = "scan", s.next_i end
  elseif s.mode == "replacement" then
    if s.out_i <= #s.rule.replacement then
      emit(s, s.rule.replacement:sub(s.out_i, s.out_i))
      s.out_i = s.out_i + 1
    else s.mode = "scan" end
  elseif s.mode == "scan" then
    if s.i > n then return format("%08x%08x:%d", s.a, s.b, s.length) end
    local c = xml:sub(s.i, s.i)
    if c == "<" then s.mode, s.start, s.node = "opening", s.i, s.root
    else emit(s, c) s.i = s.i + 1 end
  elseif s.mode == "opening" then
    if s.i > n then output(s, s.start, n, n + 1) return end
    local node = s.node[xml:sub(s.i, s.i)]
    if not node then output(s, s.start, s.i - 1, s.i) return end
    s.node, s.i = node, s.i + 1
    if node.rule then
      s.rule = node.rule
      if s.rule.chunk then
        if s.no_chunks then output(s, s.start, s.i - 1, s.i)
        else s.mode, s.matched = "chunk", 0 end
      else s.mode = "plain" end
    end
  elseif s.mode == "plain" then
    if s.i > n then output(s, s.start, n, n + 1)
    elseif xml:sub(s.i, s.i) == "<" then s.mode, s.probe, s.matched = "closing", s.i, 0
    else s.i = s.i + 1 end
  elseif s.mode == "closing" then
    if s.i > n then output(s, s.start, n, n + 1) return end
    if xml:sub(s.i, s.i) ~= s.rule.closing:sub(s.matched + 1, s.matched + 1) then
      output(s, s.start, s.probe - 1, s.probe)
    else
      s.i, s.matched = s.i + 1, s.matched + 1
      if s.matched == #s.rule.closing then s.mode, s.out_i = "replacement", 1 end
    end
  elseif s.mode == "chunk" then
    if s.i > n then
      -- No closing delimiter occurs anywhere after this opening, so every later chunk opening also fails. Rescan its
      -- contents for the plain replacements, as the sequential gsubs do, without repeatedly searching the same tail.
      s.no_chunks = true
      output(s, s.start, s.start + #s.rule.opening - 1, s.start + #s.rule.opening)
      return
    end
    local c = xml:sub(s.i, s.i)
    if c == s.rule.closing:sub(s.matched + 1, s.matched + 1) then s.matched = s.matched + 1
    else s.matched = c == "<" and 1 or 0 end
    s.i = s.i + 1
    if s.matched == #s.rule.closing then s.mode, s.out_i = "replacement", 1 end
  end
end

local DEST, VALUE, CLOSE = "DestEffect>", "<Value>", "</Value>"
local function restart_aim(s, c)
  s.mode, s.matched = "dest", c == "D" and 1 or 0
end
local function digit(c) return c ~= "" and c:match("[%d%.]") ~= nil end

local function aim_step(s)
  if s.i > #s.xml then return true, false end
  local c = s.xml:sub(s.i, s.i)
  if s.mode == "dest" then
    if c == DEST:sub(s.matched + 1, s.matched + 1) then
      s.matched = s.matched + 1
      if s.matched == #DEST then s.mode, s.matched = "space", 0 end
    else restart_aim(s, c) end
  elseif s.mode == "space" then
    if not c:match("%s") then
      if c == "<" then s.mode, s.matched = "value", 1 else restart_aim(s, c) end
    end
  elseif s.mode == "value" then
    if c == VALUE:sub(s.matched + 1, s.matched + 1) then
      s.matched = s.matched + 1
      if s.matched == #VALUE then s.mode, s.number_start, s.numbers = "number", s.i + 1, 0 end
    else restart_aim(s, c) end
  elseif s.mode == "number" then
    if digit(c) then
      s.numbers = s.numbers + 1
    elseif c == "-" and s.i == s.number_start then
      -- The optional minus is part of the capture but not its required digit/dot run.
    elseif c == "<" and s.numbers > 0 then
      s.mode, s.matched, s.number_end = "close", 1, s.i - 1
    else restart_aim(s, c) end
    if s.mode == "number" and s.i - s.number_start + 1 > M.NUMBER_BYTES then return true, nil, true end
  elseif s.mode == "close" then
    if c == CLOSE:sub(s.matched + 1, s.matched + 1) then
      s.matched = s.matched + 1
      if s.matched == #CLOSE then
        -- Keep tonumber's exact edge cases and the old error for malformed numeric captures. An unusually long
        -- capture takes the explicit synchronous fallback instead of one unbounded conversion in a background slice.
        if tonumber(s.xml:sub(s.number_start, s.number_end)) >= 0 then return true, true end
        s.mode, s.matched = "dest", 0
      end
    else restart_aim(s, c) end
  end
  s.i = s.i + 1
end

local Processor = {}
Processor.__index = Processor
function M.new(xml)
  assert(type(xml) == "string", "preset data is text")
  return setmetatable({ xml = xml, phase = "aim", state = { xml = xml, i = 1, mode = "dest", matched = 0 }, work = 0 }, Processor)
end

-- At most `most` bounded byte/state operations, whether the clock moves or not. A result is returned only when both
-- complete fingerprints exist. `synchronous` asks the caller to retain the old synchronous check for that exact text.
function Processor:advance(most)
  assert(type(most) == "number" and most >= 1 and most == math.floor(most), "a positive work budget")
  local used = 0
  while used < most and not self.result do
    used = used + 1
    if self.phase == "aim" then
      local done, moving, synchronous = aim_step(self.state)
      if done then
        if synchronous then self.result = { synchronous = true }
        else
          self.moving, self.phase, self.state = moving, "shape", normalizer(self.xml, shape_rules)
        end
      end
    else
      local printed = normalize_step(self.state)
      if printed then
        if self.phase == "shape" then
          self.shape, self.phase, self.state = printed, "content", normalizer(self.xml, self.moving and moving_rules or content_rules)
        else self.result = { shape = self.shape, content = printed } end
      end
    end
  end
  self.work = self.work + used
  return self.result, used
end

return M
