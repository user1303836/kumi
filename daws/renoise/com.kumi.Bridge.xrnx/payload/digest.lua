-- digest {cursor?, values?, content?}: the song as records for catch-up and watch_me (M6), which Kumi's host keeps and
-- compares, a page at a time. Renoise's objects have no lasting identity, so records carry what tells them apart (a name
-- that isn't Renoise's own numbering, what they hold, their place) and the host matches them by that.
--
-- Records: each track (kind, mixer, routing, colour, columns, group, device count), its devices after the mixer (path,
-- on/off, a fingerprint of the parameters that aren't automated), each pattern (length, each track's lines summed up as
-- "n notes, n effects #fingerprint", its automation) and each instrument (samples, plug-in, phrases, macros, modulation
-- and effects). The song's own values (BPM, LPB, TPL, name, the sequence) come with the first page.
--
-- Bounded: pages of about PAGE_BYTES; pattern lines read up to LINES_BUDGET for the whole digest (the rest aren't
-- compared: null), parameter values (`values`, watch_me) up to VALUES_BUDGET; `content: false` (watch_me's looks) reads
-- no lines at all. A job: each pattern and track in a slice of its own.

local import = ...
local json = import("json")
local refs = import("refs")
local rows = import("rows")

local bit = require("bit")
local bxor, lshift, tobit, tohex = bit.bxor, bit.lshift, bit.tobit, bit.tohex
local byte, format = string.byte, string.format

local M = {}

M.LINES_BUDGET = 300000
M.VALUES_BUDGET = 100000
M.PAGE_BYTES = 192 * 1024

local function fail(code, message)
  error({ code = code, message = message }, 0)
end

-- 32-bit FNV-1a: a fingerprint only this tool compares with itself.
local function new_print()
  return { h = -2128831035 }
end

local function feed(print, text)
  local h = print.h
  for i = 1, #text do
    h = bxor(h, byte(text, i))
    h = tobit(lshift(h, 24) + lshift(h, 8) + h * 147)
  end
  h = bxor(h, 31)
  print.h = tobit(lshift(h, 24) + lshift(h, 8) + h * 147)
end

local function feed_number(print, value)
  feed(print, format("%.4f", type(value) == "number" and value or 0))
end

local function hex(print)
  return tohex(print.h)
end

local function round(value, places)
  if type(value) ~= "number" or value ~= value then return json.null end
  local scale = 10 ^ places
  return math.floor(value * scale + 0.5) / scale
end

local function db(linear)
  if type(linear) == "number" and linear > 0 then return round(20 * math.log10(linear), 2) end
  return -200
end

-- The names Renoise numbers itself follow the track's place: they tell nothing of which one it is (measured, 3.5.4).
local DEFAULT_NAME = { sequencer = "^Track %d+$", send = "^S%d+$", group = "^Group %d+$" }

local function mute_word(state)
  local T = renoise.Track
  if state == T.MUTE_STATE_OFF then return "off" end
  if state == T.MUTE_STATE_MUTED then return "muted" end
  return "active"
end

-- A pan as Kumi's track kinds take it (-1 to 1, 0 the centre), and a width in percent: what a recipe sets again; null when
-- automation moves it (it follows the automation at the edit position).
local function pan_of(parameter)
  if parameter.is_automated then return json.null end
  return round((parameter.value - 0.5) * 2, 4)
end

local function width_of(parameter)
  if parameter.is_automated or not parameter.value_max or parameter.value_max == 0 then return json.null end
  return round(parameter.value / parameter.value_max * 100, 2)
end

-- Where a digest goes on from: `g1.<epoch>.<phase>.<index>.<lines>.<values>`, phase 1 the tracks, 2 the patterns, 3 the
-- instruments; the budgets as they were when that phase's item began.
local function cursor_text(c)
  return format("g1.%s.%d.%d.%d.%d", c.epoch, c.phase, c.index, c.lines, c.values)
end

local function parse_cursor(text)
  if type(text) ~= "string" then return nil end
  local epoch, phase, index, lines, values = text:match("^g1%.([%x]+)%.(%d)%.(%d+)%.(%d+)%.(%d+)$")
  if not epoch then return nil end
  return { epoch = epoch, phase = tonumber(phase), index = tonumber(index), lines = tonumber(lines), values = tonumber(values) }
end

-- The labels a pattern's lines are kept under: each track's name, told apart when two share it.
local function track_labels(song, count)
  local labels, seen = {}, {}
  for t = 1, count do
    local name = song:track(t).name
    if name == nil or name == "" then name = format("Track %02d", t) end
    seen[name] = (seen[name] or 0) + 1
    labels[t] = seen[name] > 1 and (name .. " (" .. seen[name] .. ")") or name
  end
  return labels
end

local function track_records(job, song, t, epoch, parents, budget, out)
  local track = song:track(t)
  local kind = rows.track_type(track)
  local name = track.name or ""
  local pattern = DEFAULT_NAME[kind]
  local default = name == "" or (pattern ~= nil and name:match(pattern) ~= nil)
  local key = "track#/" .. t
  local mixer = {
    volume = track.prefx_volume.is_automated and json.null or db(track.prefx_volume.value),
    pan = pan_of(track.prefx_panning),
    width = width_of(track.prefx_width),
    postVolume = track.postfx_volume.is_automated and json.null or db(track.postfx_volume.value),
    postPan = pan_of(track.postfx_panning),
  }
  if kind ~= "master" then
    mixer.mute = mute_word(track.mute_state)
    mixer.solo = track.solo_state
  end
  local routing = { output = track.output_routing }
  if kind == "sequencer" then routing.delay = track.output_delay end
  local group = parents[t] and song:track(parents[t]).name or json.null
  out[#out + 1] = {
    k = "track",
    n = (name ~= "" and name) or format("Track %02d", t),
    dn = default or nil,
    o = t,
    f = {
      kind = kind,
      mixer = mixer,
      routing = routing,
      color = rows.hex(track.color),
      columns = { notes = track.visible_note_columns, effects = track.visible_effect_columns },
      group = { name = group },
      devices = { count = #track.devices - 1 },
    },
  }
  -- Its devices after the mixer (the mixer's parameters are the track's own, above).
  for d = 2, #track.devices do
    local device = track:device(d)
    local print, values = new_print(), {}
    local parameters = device.parameters
    local listed = budget.values_on and budget.values + #parameters <= M.VALUES_BUDGET
    for k = 1, #parameters do
      local parameter = parameters[k]
      if parameter.is_automated then
        if listed then values[k] = json.null end
      else
        feed_number(print, parameter.value)
        if listed then values[k] = round(parameter.value, 6) end
      end
    end
    if listed then budget.values = budget.values + #parameters end
    local record = {
      k = "device",
      n = device.display_name,
      o = d,
      p = key,
      f = { path = device.device_path or "", active = device.is_active, parameters = { count = #parameters, parametersHash = hex(print) } },
    }
    if listed then record.v = json.array(values) end
    if budget.values_on then record.r = refs.make(epoch, "device", t, d) end
    out[#out + 1] = record
  end
  job.slice()
end

-- A pattern track's lines summed up as "n notes, n effects #fingerprint" (the same lines, the same text), `slice` called every
-- 64 lines (a job's, or none). (#401: the jam's pattern reads compare patterns by it.)
function M.lines_print(pattern_track, track, length, slice)
  local print, notes, effects = new_print(), 0, 0
  local note_columns, effect_columns = track.visible_note_columns, track.visible_effect_columns
  for l = 1, length do
    local line = pattern_track:line(l)
    if not line.is_empty then
      feed_number(print, l)
      for c = 1, note_columns do
        local column = line:note_column(c)
        if not column.is_empty then
          notes = notes + ((column.note_value < 120) and 1 or 0)
          feed(print, format("%d:%d %d %d %d %d %d %d", c, column.note_value, column.instrument_value, column.volume_value,
            column.panning_value, column.delay_value, column.effect_number_value, column.effect_amount_value))
        end
      end
      for c = 1, effect_columns do
        local column = line:effect_column(c)
        if not column.is_empty then
          effects = effects + 1
          feed(print, format("e%d:%d %d", c, column.number_value, column.amount_value))
        end
      end
    end
    if slice and l % 64 == 0 then slice() end
  end
  return format("%d notes, %d effects #%s", notes, effects, hex(print))
end

local function pattern_record(job, song, p, count, labels, budget)
  local pattern = song:pattern(p)
  local length = pattern.number_of_lines
  local lines = {}
  local automation = new_print()
  local automated = 0
  for t = 1, count do
    local pattern_track = pattern:track(t)
    local track = song:track(t)
    if pattern_track.alias_pattern_index > 0 then
      lines[labels[t]] = format("alias of Pattern %02d", pattern_track.alias_pattern_index - 1)
    elseif not pattern_track.is_empty then
      if not budget.content or budget.lines + length > M.LINES_BUDGET then
        lines[labels[t]] = json.null
        if budget.content then budget.over = true end
      else
        budget.lines = budget.lines + length
        lines[labels[t]] = M.lines_print(pattern_track, track, length, job.slice)
      end
    end
    for _, entry in ipairs(pattern_track.automation) do
      automated = automated + 1
      local parameter = entry.dest_parameter
      feed(automation, format("%d:%s:%d", t, parameter and parameter.name or "", entry.playmode))
      for _, point in ipairs(entry.points) do
        feed_number(automation, point.time)
        feed_number(automation, point.value)
      end
    end
  end
  local name = pattern.name or ""
  job.slice()
  return {
    k = "pattern",
    n = name ~= "" and name or format("Pattern %02d", p - 1),
    dn = name == "" or nil,
    o = p,
    f = {
      length = length,
      lines = next(lines) and lines or json.object(),
      automation = { count = automated, automationHash = hex(automation) },
    },
  }
end

local function instrument_record(job, song, i)
  local instrument = song:instrument(i)
  local samples = new_print()
  for s = 1, #instrument.samples do
    local sample = instrument:sample(s)
    local mapping = sample.sample_mapping
    feed(samples, sample.name or "")
    feed_number(samples, mapping.base_note)
    feed(samples, format("%d-%d %d-%d", mapping.note_range[1], mapping.note_range[2], mapping.velocity_range[1], mapping.velocity_range[2]))
    for _, value in ipairs({ sample.volume, sample.panning, sample.transpose, sample.fine_tune, sample.loop_mode, sample.loop_start,
      sample.loop_end, sample.mute_group or 0 }) do
      feed_number(samples, value)
    end
    local buffer = sample.sample_buffer
    if buffer and buffer.has_sample_data then
      feed_number(samples, buffer.number_of_frames)
      feed_number(samples, buffer.sample_rate)
    end
  end
  local phrases = new_print()
  for k = 1, #instrument.phrases do
    local phrase = instrument:phrase(k)
    feed(phrases, phrase.name or "")
    feed_number(phrases, phrase.number_of_lines)
    feed_number(phrases, phrase.lpb)
  end
  local macros = new_print()
  for k = 1, #instrument.macros do
    local macro = instrument:macro(k)
    feed(macros, macro.name or "")
    feed_number(macros, macro.value)
  end
  local plugin = instrument.plugin_properties
  local name = instrument.name or ""
  job.slice()
  return {
    k = "instrument",
    n = name ~= "" and name or format("Instrument %02X", i - 1),
    dn = name == "" or nil,
    o = i,
    f = {
      samples = { count = #instrument.samples, samplesHash = hex(samples) },
      plugin = plugin.plugin_loaded and plugin.plugin_device.name or "",
      phrases = { count = #instrument.phrases, phrasesHash = hex(phrases) },
      macros = { macrosHash = hex(macros) },
      modulation = { count = #instrument.sample_modulation_sets },
      effects = { count = #instrument.sample_device_chains },
    },
  }
end

-- The song's own values: tempo, lines per beat, ticks per line, its name, and its sequence (patterns, sections, mutes).
local function facts(song, count)
  local sequencer = song.sequencer
  local sequence = sequencer.pattern_sequence
  local print = new_print()
  for s = 1, #sequence do
    feed_number(print, sequence[s])
    if sequencer:sequence_is_start_of_section(s) then feed(print, "section:" .. (sequencer:sequence_section_name(s) or "")) end
    for t = 1, count do
      if sequencer:track_sequence_slot_is_muted(t, s) then feed(print, "m" .. t) end
    end
  end
  local transport = song.transport
  return {
    bpm = transport.bpm,
    lpb = transport.lpb,
    tpl = transport.tpl,
    name = song.name or "",
    sequence = { slots = #sequence, sequenceHash = hex(print) },
  }
end

-- One page of the digest. `extra` gives the project row (the bridge's song id and identity) and the change counter.
function M.digest(job, params, epoch, extra)
  local song = rows.the_song()
  local count = rows.track_count(song)
  local values_on = params.values == true
  local content = params.content ~= false
  local cursor = { epoch = epoch, phase = 1, index = 1, lines = 0, values = 0 }
  if params.cursor ~= nil then
    local parsed = parse_cursor(params.cursor)
    if not parsed then fail("bad-params", "cursor: the nextCursor an earlier page gave, unchanged") end
    ---@cast parsed -nil
    if parsed.epoch ~= epoch then fail("stale", "the song changed since the first page: read again") end
    cursor = parsed
  end
  local first = params.cursor == nil
  local budget = { lines = cursor.lines, values = cursor.values, values_on = values_on, content = content, over = false }
  local records, bytes, next_cursor = {}, 0, nil
  local parents = rows.group_parents(song)
  local labels = track_labels(song, count)
  -- Adds what one item gave; false (and the cursor to go on from) when the page is full. (Each record is encoded once, here
  -- where it is measured, and goes into the page as that text: the frame encodes no record again.)
  local function take(made, phase, index, before)
    local size, texts = 0, {}
    for k, record in ipairs(made) do
      local text = json.encode(record, { max_depth = json.ROW_DEPTH })
      texts[k] = text or false
      size = size + #(text or json.encode(record) or "") + 1
    end
    if #records > 0 and bytes + size > M.PAGE_BYTES then
      next_cursor = { epoch = epoch, phase = phase, index = index, lines = before.lines, values = before.values }
      return false
    end
    for k = 1, #made do records[#records + 1] = texts[k] and json.raw(texts[k]) or made[k] end
    bytes = bytes + size
    return true
  end
  local phases = {
    { total = count, make = function(index, made) track_records(job, rows.the_song(), index, epoch, parents, budget, made) end },
    { total = #song.patterns, make = function(index, made) made[1] = pattern_record(job, rows.the_song(), index, count, labels, budget) end },
    { total = #song.instruments, make = function(index, made) made[1] = instrument_record(job, rows.the_song(), index) end },
  }
  local phase, index = cursor.phase, cursor.index
  while phase <= #phases and not next_cursor do
    while index <= phases[phase].total do
      local before = { lines = budget.lines, values = budget.values }
      local made = {}
      phases[phase].make(index, made)
      if not take(made, phase, index, before) then
        budget.lines, budget.values = before.lines, before.values
        break
      end
      index = index + 1
    end
    if not next_cursor then phase, index = phase + 1, 1 end
  end
  local answer = { epoch = epoch, records = json.array(records), changed = extra.changed }
  if next_cursor then answer.nextCursor = cursor_text(next_cursor) end
  local bounded = {}
  if first then
    song = rows.the_song()
    answer.project = extra.project
    answer.facts = facts(song, count)
    answer.summaries = { device = "devices" }
    if not content then bounded.pattern = "lines not read (a quick look)" end
  end
  if budget.over then bounded.lines = format("more than %d pattern lines: the rest aren't compared", M.LINES_BUDGET) end
  answer.bounded = next(bounded) and bounded or json.object()
  return answer
end

M.parse_cursor = parse_cursor
M.cursor_text = cursor_text

return M
