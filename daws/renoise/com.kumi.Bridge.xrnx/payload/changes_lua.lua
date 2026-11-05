-- Lua run in Renoise for Kumi (run-lua): a chunk run in one callback named by the change's description, so Renoise's own
-- undo takes back whatever it did, whole. Kumi can't take it back: it doesn't know what the chunk changed.
--
-- A chunk runs with the whole of Renoise's API (`renoise`, and `song` for renoise.song()), `print` collected for the answer,
-- for at most `timeoutMs` (500 by default, 800 at most: Renoise's watchdog speaks up after about 10 s, and a callback should
-- stay well under a second). A chunk that runs past it is stopped where it is: what it changed so far stays. The limit is a
-- count hook that stops only the chunk's own code (the tool's notifiers run whole: see `guard` in bridge.lua), and the JIT is
-- off while the chunk runs, so code it loads itself is counted too; the chunk can't take the hook off or turn the JIT back
-- on (debug.sethook and jit.on raise while it runs). A long call into C (a string pattern that backtracks) is not stopped.

local import = ...
local refs = import("refs")
local base = import("changes_base")
local json = import("json")

local bad, fail = base.bad, base.fail
local M = {}

M.DEFAULT_MS, M.MOST_MS = 500, 800
local MOST_CODE = 256 * 1024
local MOST_PRINTED = 200
local MOST_LINE = 4096
local MOST_RESULT = 256 * 1024

-- What a chunk may write, read from its text (its comments and strings blanked first): "none" when it sets no field but
-- of tables it made itself and calls only methods that read; "unkept" when all it sets or calls is what Renoise's undo
-- doesn't keep (mute, solo, selection, transport); "any" otherwise. Only "none" is taken as a chunk that changed nothing.
local READS = {}
for name in ([[track pattern instrument sample phrase line lines_in_range note_column effect_column device parameter
  column_is_muted column_name sequence_is_start_of_section sequence_section_name track_sequence_slot_is_muted find_automation
  sample_modulation_set sample_device_chain can_undo can_redo is_undo_redoing has_line_notifier has_notifier
  format sub match gmatch find lower upper len rep gsub byte reverse]]):gmatch("%S+") do READS[name] = true end
local UNKEPT_FIELDS = {}
for name in ([[mute_state solo_state selected_track_index selected_instrument_index selected_line_index selected_sequence_index
  selected_sample_index selected_phrase_index selected_track_device_index selected_note_column_index selected_effect_column_index
  follow_player edit_mode edit_step octave loop_pattern loop_block_enabled loop_range loop_sequence_range metronome_enabled
  metronome_precount_enabled metronome_precount_bars record_quantize_enabled record_quantize_lines wrapped_pattern_edit
  single_track_edit_mode keyboard_velocity_enabled keyboard_velocity record_parameter_mode playback_pos playback_pos_beats
  edit_pos edit_pos_beats loop_block_range_coeff octave_enabled selection_in_pattern selection_in_phrase
  selected_phrase_line_index selected_phrase_note_column_index selected_phrase_effect_column_index
  selected_sample_device_chain_index selected_sample_device_index selected_sample_modulation_set_index]]):gmatch("%S+") do UNKEPT_FIELDS[name] = true end
-- (mute and solo, notes played, playback and its schedule, the block loop and the selection: none is a step of Renoise's undo)
local UNKEPT_METHODS = {}
for name in ([[start stop start_at panic show_status mute unmute solo trigger_instrument_note_on trigger_instrument_note_off
  trigger_sample_note_on trigger_sample_note_off trigger_pattern_line trigger_sequence add_scheduled_sequence
  set_scheduled_sequence loop_block_move_forwards loop_block_move_backwards select_next_track
  select_previous_track]]):gmatch("%S+") do UNKEPT_METHODS[name] = true end

-- The code with its comments blanked and its strings emptied (so nothing in them reads as code).
local function blanked(code)
  local out, i, n = {}, 1, #code
  while i <= n do
    local at = code:find("[%-%[\"']", i)
    if not at then out[#out + 1] = code:sub(i) break end
    out[#out + 1] = code:sub(i, at - 1)
    local c = code:sub(at, at)
    local long = code:match("^%[(=*)%[", c == "-" and at + 2 or at)
    if c == "-" and code:sub(at + 1, at + 1) ~= "-" then
      out[#out + 1], i = "-", at + 1
    elseif c == "-" then
      local close = long and code:find("]" .. long .. "]", at + 4 + #long, true)
      i = long and (close and close + #long + 2 or n + 1) or (code:find("\n", at, true) or n + 1)
      out[#out + 1] = " "
    elseif c == "[" and not long then
      out[#out + 1], i = "[", at + 1
    elseif c == "[" then
      local close = code:find("]" .. long .. "]", at + 2 + #long, true)
      out[#out + 1], i = '""', close and close + #long + 2 or n + 1
    else
      local j = at + 1
      while j <= n do
        local d = code:sub(j, j)
        if d == "\\" then j = j + 2 elseif d == c or d == "\n" then break else j = j + 1 end
      end
      out[#out + 1], i = '""', j + 1
    end
  end
  return table.concat(out)
end

-- The target of an assignment that ends at `e`: its root name ("?" when it is a call's result) and the field it sets, or
-- nil for a plain name or a table constructor's key; and where the text before it ends.
local function target_at(text, e)
  local i, field = e, nil
  while i >= 1 do
    local c = text:sub(i, i)
    if c == "]" then
      local depth = 0
      repeat
        local d = text:sub(i, i)
        if d == "]" then depth = depth + 1 elseif d == "[" then depth = depth - 1 end
        i = i - 1
      until depth == 0 or i < 1
      field = field or "[]"
      while i >= 1 and text:sub(i, i):match("%s") do i = i - 1 end
      if not text:sub(i, i):match("[%w_%]%)]") then return nil, nil, i end
    elseif c:match("[%w_]") then
      local last = i
      while i >= 1 and text:sub(i, i):match("[%w_]") do i = i - 1 end
      local name = text:sub(i + 1, last)
      local j = i
      while j >= 1 and text:sub(j, j):match("%s") do j = j - 1 end
      if text:sub(j, j) == "." and text:sub(j - 1, j - 1) ~= "." then
        field = field or name
        i = j - 1
        while i >= 1 and text:sub(i, i):match("%s") do i = i - 1 end
      else
        if field == nil or name:match("^%d") then return nil, nil, j end
        return name, field, j
      end
    else
      return field and "?" or nil, field, i
    end
  end
  return field and "?" or nil, field, 0
end

local function writes_of(code)
  local text = blanked(code)
  -- (code it loads or runs itself, or tables it reaches round: anything)
  for _, word in ipairs({ "load", "dofile", "require", "rawset", "setmetatable", "setfenv", "debug" }) do
    if text:find(word, 1, true) then return "any" end
  end
  local made, class = {}, "none"
  for name in text:gmatch("local%s+([%a_][%w_]*)%s*=%s*{") do made[name] = true end
  local function saw(unkept)
    if not unkept then class = "any" elseif class == "none" then class = "unkept" end
  end
  for name in text:gmatch(":%s*([%a_][%w_]*)%s*[%(\"{]") do
    if UNKEPT_METHODS[name] then saw(true) elseif not READS[name] then saw(false) end
  end
  for at in text:gmatch("()=") do
    if text:sub(at + 1, at + 1) ~= "=" and not text:sub(at - 1, at - 1):match("[=~<>]") then
      local e = at - 1
      while e >= 1 do
        while e >= 1 and text:sub(e, e):match("%s") do e = e - 1 end
        local root, field, before = target_at(text, e)
        if root and not made[root] then saw(UNKEPT_FIELDS[field] == true) end
        if text:sub(before, before) ~= "," then break end
        e = before - 1
      end
    end
    if class == "any" then break end
  end
  return class
end
M.writes_of = writes_of

local function first_line(code)
  for line in code:gmatch("[^\n]+") do
    local text = line:gsub("^%s+", ""):gsub("%s+$", "")
    if text ~= "" then
      if #text > 60 then text = text:sub(1, 59) .. "\226\128\166" end
      return text
    end
  end
  return "(nothing)"
end

local function compile(input)
  local code = input.code
  if type(code) ~= "string" or code == "" then bad("code: the Lua to run") end
  if #code > MOST_CODE then bad("code is longer than 256 KB") end
  local chunk, err = loadstring(code, "=kumi")
  if not chunk then bad("the Lua doesn't compile: " .. tostring(err)) end
  local limit = input.timeoutMs
  if limit == nil then limit = M.DEFAULT_MS end
  if type(limit) ~= "number" or limit < 1 or limit > M.MOST_MS then bad(string.format("timeoutMs is from 1 to %d", M.MOST_MS)) end
  return chunk, limit
end

-- A value as JSON can carry it: tables copied (to a depth), Renoise's objects and functions as their names.
local function plain(value, depth)
  local kind = type(value)
  -- (a text past the bound isn't copied or encoded: its length says what it was)
  if kind == "string" and #value > MOST_RESULT then return string.format("(a text of %d bytes, not sent)", #value) end
  if kind == "nil" or kind == "boolean" or kind == "string" then return value end
  if kind == "number" then return (value == value and value ~= math.huge and value ~= -math.huge) and value or tostring(value) end
  if kind ~= "table" or depth > 6 then return tostring(value) end
  -- (a list 1..n stays a list; any other table is an object with its keys as text: kept as they were, a list with gaps,
  -- a 0 key or names among the numbers would make no JSON at all)
  local count, top, list = 0, 0, true
  for k in pairs(value) do
    count = count + 1
    if list and type(k) == "number" and k >= 1 and k % 1 == 0 then top = math.max(top, k) else list = false end
  end
  local out = {}
  if list and top == count then
    for i = 1, math.min(count, 1000) do out[i] = plain(value[i], depth + 1) end
    return out
  end
  local n = 0
  for k, v in pairs(value) do
    n = n + 1
    if n > 1000 then break end
    out[tostring(k)] = plain(v, depth + 1)
  end
  return out
end

-- A value's text for an answer, which nothing in it can make raise (an error object's own __tostring may).
local function said(value)
  local ok, text = pcall(tostring, value)
  return ok and text or "(a value Kumi can't read)"
end

-- Lua's own functions: an error raised in code one of them called (table.sort's comparator, pcall's function) goes back
-- to the Lua that called it. Any other C function (Renoise's, firing notifiers) may catch it itself: it shows an error and
-- disables the notifier.
local LIBRARY = {}
for _, t in ipairs({ _G, string, table, math, os, io, coroutine, debug, rawget(_G, "bit"), rawget(_G, "jit"), package }) do
  if type(t) == "table" then
    for _, v in pairs(t) do
      if type(v) == "function" then LIBRARY[v] = true end
    end
  end
end

-- Whether a stop raised in the code the count hook interrupted (the hook's caller, `level` 2 from it) reaches `run`, the
-- function run-lua's pcall runs the chunk in: only through the chunk's code and Lua's functions, never through one of the
-- tool's guards (its notifiers run whole) or a C function of Renoise's. Code outside `run` (run-lua's own, after the chunk
-- ended) is never stopped; a coroutine the chunk made is stopped where it is: its error goes to its resumer.
local function stoppable(level, run, thread, is_guard)
  while true do
    local info = debug.getinfo(level + 1, "fS")
    if not info then return coroutine.running() ~= thread end
    local fn = info.func
    if fn == run then return true end
    if is_guard(fn) or (info.what == "C" and not LIBRARY[fn]) then return false end
    level = level + 1
  end
end

local NOTE = "Only Renoise's own undo can take back what it did (Edit > Undo); Kumi can't."
local UNKEPT_NOTE = "What it changed (mute, solo, selection or transport) is nothing Renoise's undo takes back, and Kumi can't either."

local function preview(song, env, input)
  compile(input)
  return {
    target = { ref = refs.make(env.epoch(), "song", 1), kind = "song" }, prior = {}, fence = {},
    display = { title = "Ran Lua: " .. first_line(input.code), note = NOTE },
  }
end

local function apply(song, env, input, fence, desc)
  local chunk, limit = compile(input)
  local printed = {}
  local scope = setmetatable({
    song = song,
    print = function(...)
      if #printed >= MOST_PRINTED then return end
      local parts = {}
      for i = 1, select("#", ...) do parts[i] = tostring((select(i, ...))) end
      local text = table.concat(parts, "\t")
      -- (a line past the bound is cut, so a big print never fills the answer)
      if #text > MOST_LINE then text = text:sub(1, MOST_LINE) .. string.format("\226\128\166 (%d bytes in all)", #text) end
      printed[#printed + 1] = text
    end,
  }, { __index = _G })
  setfenv(chunk, scope)
  local writes = writes_of(input.code)
  local before = env.changes and env.changes() or 0
  local deadline = env.now_ms() + limit
  song:describe_undo(desc)
  -- (a loop the JIT compiles never sees the hook, the chunk's own or one it loads: the JIT is off while it runs)
  local jitted = jit and jit.status and jit.status()
  if jitted then
    jit.off()
    jit.flush()
  end
  -- (inside the tool's own notifiers, their guards included, the stop waits for the chunk's code: they run whole)
  local in_notifier = env.in_notifier or function() return false end
  local is_guard = env.is_guard or function() return false end
  local thread = coroutine.running()
  -- (the chunk runs under a frame of the tool's that stays on the stack whatever the chunk does: its own `return f()` is a
  -- tail call, which takes the chunk's frame away)
  local function run() return (chunk()) end
  -- (past the deadline where a stop can't land, inside a notifier, the hook looks again at every instruction: a loop whose
  -- turns all take the same count would otherwise meet the 1000-count hook in the same notifier every time, and never stop)
  local late, every = false, false
  -- (the chunk can't take its time limit off: while it runs, debug.sethook and jit.on raise, as REAPER's wrapper does, so
  -- it can neither clear the hook nor turn the JIT back on, whose compiled loops the hook never sees; the tool's own hook
  -- uses the real ones, and both are put back after it, however it ended)
  local sethook, jit_on = debug.sethook, jit and jit.on
  local function refused(name)
    return function() error(name .. " would take off Kumi's time limit while the chunk runs: Kumi refuses it", 2) end
  end
  local function hook()
    if not late then
      if env.now_ms() <= deadline then return end
      late = true
    end
    if not in_notifier() and stoppable(2, run, thread, is_guard) then
      error(string.format("Kumi stopped it: it ran longer than %d ms", limit), 2)
    end
    if not every then
      every = true
      sethook(hook, "", 1)
    end
  end
  sethook(hook, "", 1000)
  debug.sethook = refused("debug.sethook")
  if jit_on then jit.on = refused("jit.on") end
  local ok, result = pcall(run)
  debug.sethook = sethook
  if jit_on then jit.on = jit_on end
  sethook()
  if jitted then jit.on() end
  local told = (env.changes and env.changes() or 0) ~= before
  -- (what its text can't write didn't change; what the tool was told of did; the rest may have)
  local changed = writes == "any" or (writes == "unkept" and told) or (writes == "none" and told)
  local unkept = writes == "unkept"
  local applied = { changed = changed, printed = printed, writes = writes }
  local note = unkept and UNKEPT_NOTE or NOTE
  if changed and not unkept and not told then note = "It may have changed the song: Kumi can't see all a chunk does. " .. NOTE end
  local display = { title = "Ran Lua: " .. first_line(input.code), note = note }
  -- (the chunk has run: nothing from here raises, or the answer would say it wrote nothing and a retry would run it again)
  if ok then
    local read, value = pcall(plain, result, 0)
    if not read then value = "(a value Kumi couldn't read: " .. said(value) .. ")" end
    local text, why = json.encode({ v = value })
    if not text then
      value = "(a value Kumi couldn't send: " .. said(why) .. ")"
    elseif #text > MOST_RESULT then
      value = "(a value of more than 256 KB, not sent)"
    end
    applied.result = value
  else
    applied.failed = { message = said(result) }
    display.note = string.format("It stopped with an error (%s)%s. %s", said(result),
      changed and "; what it changed before that stays" or "", unkept and UNKEPT_NOTE or NOTE)
  end
  if not changed then display.unchanged = true end
  return {
    applied = applied, display = display, target = { ref = refs.make(env.epoch(), "song", 1), kind = "song" },
    -- (Renoise makes a step only of what its undo keeps: none when nothing changed, or only mute, solo, selection or transport)
    undo = { desc = desc, entry = changed and not unkept }, prior = {},
  }
end

local function restore()
  fail("daw-only", "Kumi can't take back what a Lua chunk did: Renoise's own undo can (Edit > Undo)")
end

base.register({ kind = "run-lua", custom = true, preview = preview, apply = apply, restore = restore })

return M
