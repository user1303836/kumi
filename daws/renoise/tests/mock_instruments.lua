-- The mock song's instruments (M4, stream n-instruments): instruments with their samples (settings, keyzones, slices, a
-- buffer), phrases (lines, mappings, scripts), modulation sets and their devices, sample FX chains and their devices, a
-- plug-in or MIDI output, and the files Renoise loads and saves them as. Everything is plain data behind strict objects,
-- like the rest of the mock; each write through the API fires its observable and records its undo step.
--
-- What Renoise does that the tool depends on (measured where marked): an instrument inserted, deleted or swapped renumbers the
-- notes of every pattern that play the instruments after it (a note whose own instrument is deleted is cleared: lab
-- 2026-10-19); a sample list's insert, delete and swap are told as list operations; slice markers make read-only alias samples.

return function(M, object, h)

local line, device, out_of_range, list, note_data = h.line, h.device, h.out_of_range, h.list, h.note_data

-- Observables kept in the data (the proxies are made anew on every read)
local function obs(t, key)
  t.obs = t.obs or {}
  t.obs[key] = t.obs[key] or M.observable()
  return t.obs[key]
end
M.obs = obs

local function same(a, b)
  if type(a) == "table" and type(b) == "table" then
    if #a ~= #b then return false end
    for k = 1, #a do if a[k] ~= b[k] then return false end end
    return true
  end
  return a == b
end

local function copy(v)
  if type(v) ~= "table" then return v end
  local out = {}
  for k, x in pairs(v) do out[k] = copy(x) end
  return out
end

-- A field written through the API: changed, its observable fired, an undo step recorded (unless Renoise makes none).
local function write(t, field, value, not_a_step)
  local old = t[field]
  if same(old, value) then return end
  value = copy(value)
  t[field] = value
  M.fire(obs(t, field))
  if not not_a_step then
    M.record(function() t[field] = old; M.fire(obs(t, field)) end, function() t[field] = value; M.fire(obs(t, field)) end)
  end
end

local function check_range(field, value, low, high, whole)
  if type(value) ~= "number" or value ~= value or value < low or value > high or (whole and value ~= math.floor(value)) then
    error(string.format("invalid value for '%s': %s (valid: %s to %s)", field, tostring(value), tostring(low), tostring(high)), 3)
  end
end

-- getters/setters for plain fields of `t`: spec = { field = {default, check(value) or nil, not_a_step?} }
local function fields_of(t, spec)
  local getters, setters, plain = {}, {}, {}
  for field, s in pairs(spec) do
    getters[field] = function()
      local value = t[field]
      if value == nil then value = s[1] end
      return type(value) == "table" and copy(value) or value
    end
    setters[field] = function(value)
      if s[2] then s[2](value) end
      if t[field] == nil then t[field] = copy(s[1]) end
      write(t, field, value, s[3])
    end
    plain[field .. "_observable"] = obs(t, field)
  end
  return getters, setters, plain
end

local function range(low, high, whole)
  return function(value) check_range("value", value, low, high, whole) end
end

local function boolean(value)
  if type(value) ~= "boolean" then error("bad argument: boolean expected", 3) end
end

local function text_value(value)
  if type(value) ~= "string" then error("bad argument: string expected", 3) end
end

-- Data

local SAMPLE_DEFAULTS = {
  name = "", volume = 1, panning = 0.5, transpose = 0, fine_tune = 0, beat_sync_enabled = false, beat_sync_lines = 16, beat_sync_mode = 1,
  interpolation_mode = 3, oversample_enabled = false, new_note_action = 1, oneshot = false, mute_group = 0, autoseek = false, autofade = false,
  loop_mode = 1, loop_release = false, loop_start = 1, loop_end = 1, modulation_set_index = 0, device_chain_index = 0,
  base_note = 48, note_range = { 0, 119 }, velocity_range = { 0, 127 }, layer = 1, map_key_to_pitch = true, map_velocity_to_volume = true,
}

function M.sample_data(spec)
  local sample = {}
  for k, v in pairs(SAMPLE_DEFAULTS) do sample[k] = copy(v) end
  for k, v in pairs(spec or {}) do sample[k] = copy(v) end
  sample.markers = sample.markers or {}
  -- (a buffer: what audio it holds is `content`, which a save writes and a load reads back)
  if sample.frames then
    sample.buffer = { frames = sample.frames, rate = sample.rate or 44100, channels = sample.channels or 1, bits = sample.bits or 16, content = sample.content or ("audio:" .. sample.name) }
    sample.frames, sample.rate, sample.channels, sample.bits, sample.content = nil, nil, nil, nil, nil
    if sample.loop_end == 1 then sample.loop_end = sample.buffer.frames end
  end
  return sample
end

local function phrase_data(spec)
  local phrase = {
    name = "", lines = 16, lpb = 4, key_tracking = 1, looping = true, loop_start = 1, loop_end = 16, base_note = 48, shuffle = 0,
    autoseek = false, note_columns = 1, effect_columns = 1, instrument_column_visible = true, volume_column_visible = true,
    panning_column_visible = false, delay_column_visible = false, sample_effects_column_visible = false, playback_mode = 1,
    script = { paragraphs = { "-- a phrase script", "return pattern {}" }, committed = nil, compile_error = "", runtime_error = "", pending = 0 },
    data = {}, notifiers = {},
  }
  for k, v in pairs(spec or {}) do phrase[k] = copy(v) end
  if spec and spec.lines and not spec.loop_end then phrase.loop_end = spec.lines end
  return phrase
end
M.phrase_data = phrase_data

-- The modulation devices Renoise has, and their parameters (names as Renoise shows them).
M.MODULATION_DEVICES = {
  ["Modulation/AHDSR"] = { name = "AHDSR", params = { "Attack", "Hold", "Decay", "Sustain", "Release", "Attack Scaling", "Decay Scaling", "Release Scaling" }, sync = true },
  ["Modulation/Envelope"] = { name = "Envelope", params = {}, points = true, sync = true },
  ["Modulation/Fader"] = { name = "Fader", params = { "From", "To", "Duration", "Delay" }, sync = true },
  ["Modulation/Key Tracking"] = { name = "Key Tracking", params = { "Min", "Max" } },
  ["Modulation/LFO"] = { name = "LFO", params = { "Phase", "Frequency", "Amount", "Delay" }, sync = true },
  ["Modulation/Operand"] = { name = "Operand", params = { "Value" } },
  ["Modulation/Stepper"] = { name = "Stepper", params = {}, points = true, sync = true },
  ["Modulation/Velocity Tracking"] = { name = "Velocity Tracking", params = { "Min", "Max" } },
}
local MODULATION_PATHS = {}
for path in pairs(M.MODULATION_DEVICES) do MODULATION_PATHS[#MODULATION_PATHS + 1] = path end
table.sort(MODULATION_PATHS)

-- (a device's name is "<Target> <Type>": "Volume AHDSR", lab)
local TARGET_NAMES = { "Volume", "Panning", "Pitch", "Cutoff", "Resonance", "Drive" }

local function modulation_device_data(path, target)
  local kind = M.MODULATION_DEVICES[path]
  local params = {}
  for k, name in ipairs(kind.params) do params[k] = { name = name, value = 0.5, min = 0, max = 1 } end
  local name = TARGET_NAMES[target] .. " " .. kind.name
  local stepper = path:find("Stepper", 1, true) ~= nil
  return {
    path = path, name = name, target = target, operator = target == 1 and 3 or 1, bipolar = false, tempo_synced = false, active = true,
    display_name = name, maximized = true,
    params = params, points = kind.points and { { time = 1, value = 1, scaling = 0 }, { time = 64, value = 0.5, scaling = 0 }, { time = 128, value = 0, scaling = 0 } } or nil,
    length = kind.points and (stepper and 16 or 513) or nil, play_mode = kind.points and 2 or nil, loop_mode = 1, loop_start = 1, loop_end = 64,
    sustain_enabled = false, sustain_position = 1, fade_amount = 0, play_step = 0,
  }
end
M.modulation_device_data = modulation_device_data

local function set_data(spec)
  local set = {
    name = "Set 01", pitch_range = 12, filter_version = 3, filter_type = "None",
    inputs = { volume = 1, panning = 0, pitch = 0, cutoff = 63.5, resonance = 63.5, drive = 0 }, devices = {},
  }
  for k, v in pairs(spec or {}) do set[k] = copy(v) end
  return set
end
M.set_data = set_data

local function chain_data(spec)
  local chain = {
    name = "FX Chain 01", output_routing = "Current Track",
    devices = { { name = "Mixer", path = "Audio/Effects/Native/InstrumentVolPan", params = {
      { name = "Panning", value = 0.5, min = 0, max = 1 }, { name = "Volume", value = 1, min = 0, max = 1.4125 },
      { name = "Width", value = 0, min = 0, max = 1 } }, active = true } },
  }
  for k, v in pairs(spec or {}) do chain[k] = copy(v) end
  return chain
end
M.chain_data = chain_data

local function macro_data(name)
  return { name = name, value = 0.5, mappings = {} }
end

function M.instrument_data(spec)
  spec = spec or {}
  local instrument = {
    name = spec.name or "", volume = 1, transpose = 0, overlap = 0, phrase_playback_mode = 2, phrase_program = 0,
    tab = spec.tab or 1, comments = {},
    trigger = { quantize = 1, monophonic = false, monophonic_glide = 0, scale_mode = "None", scale_key = 1 },
    samples = {}, phrases = {}, mappings = {}, sets = {}, chains = {}, macros = {},
    pitchbend = macro_data("Pitchbend"), modwheel = macro_data("Mod Wheel"), pressure = macro_data("Pressure"),
    plugin = spec.plugin and copy(spec.plugin) or nil,
    plugin_props = { channel = 1, transpose = 0, volume = 1, auto_suspend = false },
    midi = {
      device_name = (spec.midi or {}).device or "", instrument_type = (spec.midi or {}).type or 1, channel = (spec.midi or {}).channel or 1,
      program = (spec.midi or {}).program or 0, bank = (spec.midi or {}).bank or 0, transpose = (spec.midi or {}).transpose or 0,
      delay = (spec.midi or {}).delay or 0, duration = (spec.midi or {}).duration or 8000,
    },
  }
  for k = 1, 8 do instrument.macros[k] = macro_data("Macro " .. k) end
  for k = 1, spec.samples or 0 do instrument.samples[k] = M.sample_data({ name = "Sample " .. k }) end
  for k, s in ipairs(spec.sample_specs or {}) do instrument.samples[k] = M.sample_data(s) end
  for k = 1, spec.phrases or 0 do instrument.phrases[k] = phrase_data({ name = "Phrase " .. k }) end
  -- (a new instrument has one modulation set: lab)
  for k = 1, spec.sets == nil and 1 or spec.sets do instrument.sets[k] = set_data({ name = string.format("Set %02d", k) }) end
  for k = 1, spec.chains or 0 do instrument.chains[k] = chain_data({ name = "FX " .. k }) end
  for k, v in pairs(spec.fields or {}) do instrument[k] = copy(v) end
  -- (the M3 tests' plug-in: a device of that name)
  if instrument.plugin and not instrument.plugin.path then instrument.plugin.path = "Audio/Generators/VST3/" .. (instrument.plugin.name or "Plug") end
  return instrument
end

-- Pattern data that names instruments: renumbered as Renoise renumbers it.

local function each_note(fn)
  for p, pattern in ipairs(M.data.patterns) do
    for t, pattern_track in pairs(pattern.tracks) do
      for k, l in pairs(pattern_track.lines or {}) do
        for c, note in pairs(l.notes or {}) do fn(note, l, c, p, t, k) end
      end
    end
  end
end

-- (a test's Renoise whose renumbering of notes and 0Z commands tells the patterns' line notifiers, as an edit does: not
-- measured, #361 N-N17)
local function tell_lines(p, t, k)
  if not M.renumber_touches then return end
  local pattern = M.data.patterns[p]
  local notifiers = {}
  for i, fn in ipairs(pattern.notifiers or {}) do notifiers[i] = fn end
  for _, fn in ipairs(notifiers) do
    local ok, err = pcall(fn, { pattern = p, track = t, line = k })
    if not ok then M.errors[#M.errors + 1] = "notifier: " .. tostring(err) end
  end
end

-- The 0Z commands that name an instrument's phrases, on lines where a note plays it: a phrase deleted at `ph` clears those that
-- name it and moves the later ones down; one made at `ph` moves those from it up, and never fills a cleared one (lab 2026-10-21).
local ZERO_Z = 256 + 35 -- (the mock's value for the command 0Z: 256 + 36 * "0" + "Z")
local function renumber_phrases(instrument, type_, ph)
  local v
  for k, candidate in ipairs(M.data.instruments) do if candidate == instrument then v = k - 1 end end
  if v == nil then return end
  for p, pattern in ipairs(M.data.patterns) do
    for t, pattern_track in pairs(pattern.tracks) do
      for k, l in pairs(pattern_track.lines or {}) do
        local plays = false
        for _, note in pairs(l.notes or {}) do if note.instrument == v then plays = true end end
        if plays then
          local function move(get, set)
            local number, amount = get()
            if number ~= ZERO_Z then return end
            if type_ == "remove" and amount == ph then set(0, 0) tell_lines(p, t, k)
            elseif type_ == "remove" and amount > ph then set(number, amount - 1) tell_lines(p, t, k)
            elseif type_ == "insert" and amount >= ph then set(number, amount + 1) tell_lines(p, t, k) end
          end
          for _, effect in pairs(l.effects or {}) do
            move(function() return effect.number, effect.amount end, function(n, a) effect.number, effect.amount = n, a end)
          end
          for _, note in pairs(l.notes or {}) do
            move(function() return note.fxn, note.fxa end, function(n, a) note.fxn, note.fxa = n, a end)
          end
        end
      end
    end
  end
end

local function renumber(type_, a, b)
  -- (instrument values count from 0; 255 is none)
  each_note(function(note, l, c, p, t, k)
    local v = note.instrument
    if v == 255 or v == nil then return end
    if type_ == "insert" then
      if v >= a - 1 then note.instrument = v + 1 tell_lines(p, t, k) end
    elseif type_ == "remove" then
      if v == a - 1 then l.notes[c] = note_data({}) tell_lines(p, t, k) elseif v > a - 1 then note.instrument = v - 1 tell_lines(p, t, k) end
    elseif type_ == "swap" then
      if v == a - 1 then note.instrument = b - 1 tell_lines(p, t, k) elseif v == b - 1 then note.instrument = a - 1 tell_lines(p, t, k) end
    end
  end)
end

-- What every note of the song plays, as (pattern, track, line, column) = value: an undo puts the cells back exactly.
local function notes_snapshot()
  local out = {}
  for p, pattern in ipairs(M.data.patterns) do
    for t, pattern_track in pairs(pattern.tracks) do
      for k, l in pairs(pattern_track.lines or {}) do
        for c, note in pairs(l.notes or {}) do out[#out + 1] = { p, t, k, c, copy(note) } end
      end
    end
  end
  return out
end

local function notes_restore(snapshot)
  for _, pattern in ipairs(M.data.patterns) do
    for _, pattern_track in pairs(pattern.tracks) do
      for _, l in pairs(pattern_track.lines or {}) do l.notes = {} end
    end
  end
  for _, entry in ipairs(snapshot) do
    local pattern = M.data.patterns[entry[1]]
    if pattern then
      pattern.tracks[entry[2]] = pattern.tracks[entry[2]] or { alias = 0, lines = {} }
      local lines = pattern.tracks[entry[2]].lines
      lines[entry[3]] = lines[entry[3]] or { notes = {}, effects = {} }
      lines[entry[3]].notes = lines[entry[3]].notes or {}
      lines[entry[3]].notes[entry[4]] = copy(entry[5])
    end
  end
end

-- The instrument list: an insert, delete or swap, as the API or the producer makes it (`record` for the API's undo step).
function M.instrument_list_op(type_, a, b, spec, record)
  local d = M.data
  local instruments = d.instruments
  local before = notes_snapshot()
  local made
  if type_ == "insert" then
    if a < 1 or a > #instruments + 1 then out_of_range("instrument", a) end
    if #instruments >= 255 then error("can't have more than 255 instruments in a song", 3) end
    made = M.instrument_data(spec)
    table.insert(instruments, a, made)
    renumber("insert", a)
    M.fire(d.song_fields.instruments_observable, { type = "insert", index = a })
  elseif type_ == "remove" then
    if not instruments[a] then out_of_range("instrument", a) end
    if #instruments == 1 then error("Renoise needs at least one instrument", 3) end
    made = table.remove(instruments, a)
    renumber("remove", a)
    M.fire(d.song_fields.instruments_observable, { type = "remove", index = a })
  else
    if not instruments[a] or not instruments[b] then out_of_range("instrument", a) end
    if a == b then return end
    instruments[a], instruments[b] = instruments[b], instruments[a]
    renumber("swap", a, b)
    M.fire(d.song_fields.instruments_observable, { type = "swap", index1 = a, index2 = b })
  end
  local selected = d.song_fields.selected_instrument_index
  if selected > #instruments then d.song_fields.selected_instrument_index = #instruments end
  if record then
    local after = notes_snapshot()
    M.record(function()
      if type_ == "insert" then
        table.remove(instruments, a)
        M.fire(d.song_fields.instruments_observable, { type = "remove", index = a })
      elseif type_ == "remove" then
        table.insert(instruments, a, made)
        M.fire(d.song_fields.instruments_observable, { type = "insert", index = a })
      else
        instruments[a], instruments[b] = instruments[b], instruments[a]
        M.fire(d.song_fields.instruments_observable, { type = "swap", index1 = a, index2 = b })
      end
      notes_restore(before)
    end, function()
      if type_ == "insert" then
        table.insert(instruments, a, made)
        M.fire(d.song_fields.instruments_observable, { type = "insert", index = a })
      elseif type_ == "remove" then
        table.remove(instruments, a)
        M.fire(d.song_fields.instruments_observable, { type = "remove", index = a })
      else
        instruments[a], instruments[b] = instruments[b], instruments[a]
        M.fire(d.song_fields.instruments_observable, { type = "swap", index1 = a, index2 = b })
      end
      notes_restore(after)
    end)
  end
  return made
end

-- A list inside an instrument (its samples, phrases, sets, chains) or inside a set or chain (its devices): an insert, delete
-- or swap told to `observable`, recorded for undo. `fixed` is how many entries at the front can't move (a chain's mixer).
-- (`shown`: what the notifier adds to each place: a modulation set's devices are told one higher, lab)
local function list_op(items, observable, type_, a, b, made, fixed, shown)
  fixed, shown = fixed or 0, shown or 0
  local function fire(op)
    if op.index then op.index = op.index + shown end
    if op.index1 then op.index1, op.index2 = op.index1 + shown, op.index2 + shown end
    M.fire(observable, op)
  end
  if type_ == "insert" then
    if a < 1 + fixed or a > #items + 1 then out_of_range("index", a) end
    table.insert(items, a, made)
    fire({ type = "insert", index = a })
  elseif type_ == "remove" then
    if a <= fixed or not items[a] then out_of_range("index", a) end
    made = table.remove(items, a)
    fire({ type = "remove", index = a })
  else
    if a <= fixed or b <= fixed or not items[a] or not items[b] then out_of_range("index", a) end
    items[a], items[b] = items[b], items[a]
    fire({ type = "swap", index1 = a, index2 = b })
  end
  M.record(function()
    if type_ == "insert" then
      for k, x in ipairs(items) do if x == made then table.remove(items, k) fire({ type = "remove", index = k }) break end end
    elseif type_ == "remove" then
      table.insert(items, math.min(a, #items + 1), made)
      fire({ type = "insert", index = math.min(a, #items) })
    else
      items[a], items[b] = items[b], items[a]
      fire({ type = "swap", index1 = a, index2 = b })
    end
  end, function()
    if type_ == "insert" then
      table.insert(items, math.min(a, #items + 1), made)
      fire({ type = "insert", index = math.min(a, #items) })
    elseif type_ == "remove" then
      for k, x in ipairs(items) do if x == made then table.remove(items, k) fire({ type = "remove", index = k }) break end end
    else
      items[a], items[b] = items[b], items[a]
      fire({ type = "swap", index1 = a, index2 = b })
    end
  end)
  return made
end
M.list_op = list_op

-- Proxies

local function parameters_of(data, set_value)
  local out = {}
  for k, p in ipairs(data.params or {}) do
    out[k] = h.parameter(p, {
      value = function(value)
        if type(value) ~= "number" then error("bad argument: number expected", 2) end
        if value < (p.min or 0) or value > (p.max or 1) then error("invalid value: out of range", 2) end
        local old = p.value
        if old == value then return end
        p.value = value
        M.fire(p.value_observable)
        M.record(function() p.value = old; M.fire(p.value_observable) end, function() p.value = value; M.fire(p.value_observable) end)
        if set_value then set_value(p) end
      end,
      value_string = function(text)
        local number = tonumber(tostring(text):match("-?%d+%.?%d*"))
        if not number then return end -- (Renoise ignores text without a number: lab 2026-10-19)
        local old = p.value
        number = math.max(p.min or 0, math.min(p.max or 1, number))
        if old == number then return end
        p.value = number
        M.fire(p.value_observable)
        M.record(function() p.value = old; M.fire(p.value_observable) end, function() p.value = number; M.fire(p.value_observable) end)
      end,
    })
  end
  return out
end

local function buffer_object(sample)
  local function b() return sample.buffer end
  local function need() if not b() then error("sample buffer has no sample data", 3) end return b() end
  return object("renoise.SampleBuffer", {
    CHANNEL_LEFT = 1, CHANNEL_RIGHT = 2, CHANNEL_LEFT_AND_RIGHT = 3,
    load_from = function(_, path)
      if sample.slice_alias then error("sample buffer is read-only", 2) end
      local loaded = M.read_audio(path)
      if not loaded then
        -- (Renoise shows the error in a dialog and answers false)
        M.dialogs = (M.dialogs or 0) + 1
        return false
      end
      local old_buffer, old_name, old_loop = sample.buffer, sample.name, { sample.loop_start, sample.loop_end }
      loaded.name = nil
      sample.buffer = loaded
      -- (Renoise keeps the sample's name: lab)
      sample.loop_start, sample.loop_end = 1, loaded.frames
      M.fire(obs(sample, "buffer"))
      local new_name = sample.name
      M.record(function()
        sample.buffer, sample.name, sample.loop_start, sample.loop_end = old_buffer, old_name, old_loop[1], old_loop[2]
        M.fire(obs(sample, "buffer"))
      end, function()
        sample.buffer, sample.name, sample.loop_start, sample.loop_end = loaded, new_name, 1, loaded.frames
        M.fire(obs(sample, "buffer"))
      end)
      return true
    end,
    save_as = function(_, path, format)
      if format ~= "wav" and format ~= "flac" then error("invalid format", 2) end
      -- (M5: a WAV export is a real file the bridge reads back, as Renoise writes it; see mock_listening.lua)
      if format == "wav" and M.save_wav then return M.save_wav(sample, path) end
      local buffer = need()
      M.write_file(path, { kind = "audio", buffer = buffer, name = sample.name })
      return true
    end,
    create_sample_data = function(_, rate, bits, channels, frames)
      local old = sample.buffer
      sample.buffer = { frames = frames, rate = rate, channels = channels, bits = bits, content = "silence:" .. frames }
      M.fire(obs(sample, "buffer"))
      local new = sample.buffer
      M.record(function() sample.buffer = old; M.fire(obs(sample, "buffer")) end, function() sample.buffer = new; M.fire(obs(sample, "buffer")) end)
      return true
    end,
    -- (a value of the audio: made from what the buffer holds and the frame, the same on every read)
    sample_data = function(_, channel, frame)
      local buffer = need()
      if frame < 1 or frame > buffer.frames or channel < 1 or channel > buffer.channels then error("frame or channel out of range", 2) end
      -- (what a script wrote, then an edit a test makes at a few frames, as the sample editor would: buffer.edits[frame] = value)
      local written = buffer.values and buffer.values[channel]
      if written and written[frame] then return written[frame] end
      if buffer.edits and buffer.edits[frame] then return buffer.edits[frame] end
      local h = 7
      for i = 1, #buffer.content do h = (h * 31 + buffer.content:byte(i)) % 1000003 end
      return math.sin(h + frame * 0.001 + channel)
    end,
    -- (a script's writes: prepared, written, then finalized as one undo step; Renoise clips a value to [-1, 1])
    prepare_sample_data_changes = function()
      local buffer = need()
      buffer.prepared = { values = copy(buffer.values or {}) }
    end,
    set_sample_data = function(_, channel, frame, value)
      local buffer = need()
      if frame < 1 or frame > buffer.frames or channel < 1 or channel > buffer.channels then error("frame or channel out of range", 2) end
      if type(value) ~= "number" or value ~= value then error("a sample value is a number", 2) end
      if not buffer.prepared then error("set_sample_data before prepare_sample_data_changes", 2) end
      buffer.values = buffer.values or {}
      buffer.values[channel] = buffer.values[channel] or {}
      buffer.values[channel][frame] = math.max(-1, math.min(1, value))
    end,
    finalize_sample_data_changes = function()
      local buffer = need()
      if not buffer.prepared then error("finalize_sample_data_changes before prepare_sample_data_changes", 2) end
      local old, new = buffer.prepared.values, copy(buffer.values or {})
      buffer.prepared = nil
      M.fire(obs(sample, "buffer"))
      M.record(function() buffer.values = copy(old); M.fire(obs(sample, "buffer")) end, function() buffer.values = copy(new); M.fire(obs(sample, "buffer")) end)
    end,
    delete_sample_data = function()
      local old = sample.buffer
      sample.buffer = nil
      M.fire(obs(sample, "buffer"))
      M.record(function() sample.buffer = old; M.fire(obs(sample, "buffer")) end, function() sample.buffer = nil; M.fire(obs(sample, "buffer")) end)
    end,
  }, {
    has_sample_data = function() return b() ~= nil end,
    read_only = function() return sample.slice_alias == true end,
    number_of_frames = function() return need().frames end,
    sample_rate = function() return need().rate end,
    bit_depth = function() return need().bits end,
    number_of_channels = function() return need().channels end,
  })
end

local MAPPING_SPEC = {
  base_note = { 48, range(0, 119, true) },
  note_range = { { 0, 119 }, function(v)
    if type(v) ~= "table" or type(v[1]) ~= "number" or type(v[2]) ~= "number" or v[1] < 0 or v[2] > 119 or v[1] > v[2] then error("invalid note range", 3) end
  end },
  velocity_range = { { 0, 127 }, function(v)
    if type(v) ~= "table" or type(v[1]) ~= "number" or type(v[2]) ~= "number" or v[1] < 0 or v[2] > 127 or v[1] > v[2] then error("invalid velocity range", 3) end
  end },
  layer = { 1, range(0, 2, true) },
  map_key_to_pitch = { true, boolean },
  map_velocity_to_volume = { true, boolean },
}

local SAMPLE_SPEC = {
  name = { "", text_value }, volume = { 1, range(0, 4) }, panning = { 0.5, range(0, 1) }, transpose = { 0, range(-120, 120, true) },
  fine_tune = { 0, range(-127, 127, true) }, beat_sync_enabled = { false, boolean }, beat_sync_lines = { 16, range(1, 512, true) },
  beat_sync_mode = { 1, range(1, 3, true) }, interpolation_mode = { 3, range(1, 4, true) }, oversample_enabled = { false, boolean },
  new_note_action = { 1, range(1, 3, true) }, oneshot = { false, boolean }, mute_group = { 0, range(0, 15, true) },
  autoseek = { false, boolean }, autofade = { false, boolean }, loop_mode = { 1, range(1, 4, true) }, loop_release = { false, boolean },
}

local function sample_object(instrument, s)
  local sample = instrument.samples[s]
  local getters, setters, fields = fields_of(sample, SAMPLE_SPEC)
  -- (loop points are frames of the buffer: 1 to its length, the start before the end)
  for _, field in ipairs({ "loop_start", "loop_end" }) do
    getters[field] = function() return sample[field] end
    setters[field] = function(value)
      local frames = sample.buffer and sample.buffer.frames or 1
      check_range(field, value, 1, frames, true)
      if field == "loop_start" and value > sample.loop_end then error("loop_start must be before loop_end", 2) end
      if field == "loop_end" and value < sample.loop_start then error("loop_end must be after loop_start", 2) end
      write(sample, field, value)
    end
    fields[field .. "_observable"] = obs(sample, field)
  end
  -- (an index into the instrument's sets or chains, 0 for none)
  for field, key in pairs({ modulation_set_index = "sets", device_chain_index = "chains" }) do
    getters[field] = function() return sample[field] end
    setters[field] = function(value)
      check_range(field, value, 0, #instrument[key], true)
      write(sample, field, value)
    end
    fields[field .. "_observable"] = obs(sample, field)
  end
  getters.is_slice_alias = function() return sample.slice_alias == true end
  getters.slice_markers = function() return copy(sample.markers) end
  setters.slice_markers = function(value) M.set_markers(instrument, value, false) end
  fields.slice_markers_observable = obs(sample, "markers")
  fields.sample_buffer_observable = obs(sample, "buffer")
  -- (an alias of a slice is read-only: its keyzone follows the master sample; its own settings can be written: lab 2026-10-19)
  local mapping_getters, mapping_setters, mapping_fields = fields_of(sample, MAPPING_SPEC)
  for field, set in pairs(mapping_setters) do
    mapping_setters[field] = function(value)
      if sample.slice_alias or (s == 1 and #sample.markers > 0) then error("sample mapping is read-only", 2) end
      set(value)
    end
  end
  mapping_getters.read_only = function() return sample.slice_alias == true or (s == 1 and #sample.markers > 0) end
  fields.sample_mapping = object("renoise.SampleMapping", mapping_fields, mapping_getters, mapping_setters)
  fields.sample_buffer = buffer_object(sample)
  fields.insert_slice_marker = function(_, at) M.slice_op(instrument, "insert", at) end
  fields.delete_slice_marker = function(_, at) M.slice_op(instrument, "remove", at) end
  fields.move_slice_marker = function(_, from, to) M.slice_op(instrument, "move", from, to) end
  fields.clear = function() for field, spec in pairs(SAMPLE_SPEC) do setters[field](spec[1]) end end
  fields.copy_from = function(_, other)
    local from = M.impl(other).__data
    for k, v in pairs(from) do if k ~= "obs" then write(sample, k, copy(v)) end end
  end
  fields.__data = sample
  -- (M5 m5-renoise: what listening adds to a sample; see mock_listening.lua)
  if M.sample_extras then M.sample_extras(sample, fields, getters, setters) end
  return object("renoise.Sample", fields, getters, setters)
end
M.sample_object = sample_object

-- Slices: markers in the first sample make read-only alias samples after it, one per marker.
local function rebuild_aliases(instrument)
  local master = instrument.samples[1]
  local kept = {}
  for k = 2, #instrument.samples do
    if instrument.samples[k].slice_alias then kept[instrument.samples[k].marker] = instrument.samples[k] end
  end
  local others = {}
  for k = 2, #instrument.samples do
    if not instrument.samples[k].slice_alias then others[#others + 1] = instrument.samples[k] end
  end
  local rebuilt = { master }
  for k, at in ipairs(master.markers) do
    local alias = kept[at] or M.sample_data({ name = string.format("%s (S#%02d)", master.name, k), base_note = 49 + k - 1 })
    alias.slice_alias, alias.marker = true, at
    alias.note_range = { master.base_note + k, master.base_note + k }
    alias.base_note = master.base_note + k
    alias.buffer = master.buffer and { frames = (master.markers[k + 1] or master.buffer.frames) - at, rate = master.buffer.rate, channels = master.buffer.channels, bits = master.buffer.bits, content = master.buffer.content .. "@" .. at } or nil
    rebuilt[#rebuilt + 1] = alias
  end
  if #master.markers > 0 then others = {} end
  for _, other in ipairs(others) do rebuilt[#rebuilt + 1] = other end
  return rebuilt
end

function M.set_markers(instrument, markers, remap)
  local master = instrument.samples[1]
  local before_samples, before_markers = list(instrument.samples), copy(master.markers)
  table.sort(markers)
  master.markers = copy(markers)
  local rebuilt = rebuild_aliases(instrument)
  -- (told as removes of the old aliases and inserts of the new, as a list does)
  for k = #instrument.samples, 2, -1 do
    table.remove(instrument.samples, k)
    M.fire(instrument.samples_observable, { type = "remove", index = k })
  end
  for k = 2, #rebuilt do
    instrument.samples[k] = rebuilt[k]
    M.fire(instrument.samples_observable, { type = "insert", index = k })
  end
  M.fire(obs(master, "markers"))
  local after_samples, after_markers = list(instrument.samples), copy(master.markers)
  M.record(function()
    instrument.samples, master.markers = list(before_samples), copy(before_markers)
    M.fire(instrument.samples_observable)
    M.fire(obs(master, "markers"))
  end, function()
    instrument.samples, master.markers = list(after_samples), copy(after_markers)
    M.fire(instrument.samples_observable)
    M.fire(obs(master, "markers"))
  end)
  M.slices_remapped = (M.slices_remapped or 0) + (remap and 1 or 0)
end

function M.slice_op(instrument, type_, a, b)
  local master = instrument.samples[1]
  if not master.buffer then error("slices need sample data", 3) end
  local markers = copy(master.markers)
  if type_ == "insert" then
    if a < 1 or a > master.buffer.frames then error("marker position out of range", 3) end
    for _, at in ipairs(markers) do if at == a then error("a marker exists at that position", 3) end end
    if #markers >= 255 then error("too many slices", 3) end
    markers[#markers + 1] = a
  else
    local found
    for k, at in ipairs(markers) do if at == a then found = k end end
    if not found then error("no slice marker at that position", 3) end
    if type_ == "remove" then
      table.remove(markers, found)
    else
      if b < 1 or b > master.buffer.frames then error("marker position out of range", 3) end
      markers[found] = b
      -- (the alias of the moved slice keeps its settings)
      for k = 2, #instrument.samples do
        if instrument.samples[k].slice_alias and instrument.samples[k].marker == a then instrument.samples[k].marker = b end
      end
    end
  end
  M.set_markers(instrument, markers, true)
end

local function phrase_line(phrase, k)
  return line(phrase.data[k], {
    current = function() return phrase.data[k] end,
    ensure = function()
      phrase.data[k] = phrase.data[k] or { notes = {}, effects = {} }
      return phrase.data[k]
    end,
    changed = function()
      for _, fn in ipairs(list(phrase.notifiers)) do
        local ok, err = pcall(fn, { line = k })
        if not ok then M.errors[#M.errors + 1] = "notifier: " .. tostring(err) end
      end
    end,
  })
end

local PHRASE_SPEC = {
  name = { "", text_value }, lpb = { 4, range(1, 256, true) }, looping = { true, boolean },
  key_tracking = { 1, range(1, 3, true) }, base_note = { 48, range(0, 119, true) }, shuffle = { 0, range(0, 1) }, autoseek = { false, boolean },
  instrument_column_visible = { true, boolean }, volume_column_visible = { true, boolean }, panning_column_visible = { false, boolean },
  delay_column_visible = { false, boolean }, sample_effects_column_visible = { false, boolean }, playback_mode = { 1, range(1, 2, true) },
}

local function script_object(phrase)
  local script = phrase.script
  return object("renoise.InstrumentPhraseScript", {
    paragraphs_observable = obs(script, "paragraphs"), paragraphs_assignment_observable = obs(script, "paragraphs_assignment"),
    compile_error_observable = obs(script, "compile_error"), runtime_error_observable = obs(script, "runtime_error"),
    pending_changes_observable = obs(script, "pending"), committed_observable = obs(script, "committed"),
    parameters = {}, parameters_observable = obs(script, "parameters"), editor_visible = false, editor_visible_observable = obs(script, "editor"),
    commit = function()
      -- (Renoise compiles a committed script later, in its player: M.compile_script runs it when the test says)
      M.fire(obs(script, "committed"))
      script.pending = 0
      M.pending_compiles = M.pending_compiles or {}
      M.pending_compiles[#M.pending_compiles + 1] = script
    end,
    render_to_pattern = function(_, options, done)
      if type(options) == "function" then done, options = options, {} end
      -- (a render Renoise refuses at once answers false and why, and calls nothing back: M.render_refuses sets the why)
      if M.render_refuses then return false, M.render_refuses end
      M.pending_renders = M.pending_renders or {}
      M.pending_renders[#M.pending_renders + 1] = { phrase = phrase, options = options, done = done, lines = phrase.lines }
      -- (Renoise rewrites the phrase's length by itself before the tool's next callback, well before it answers: lab 2026-10-19)
      M.idle_work = M.idle_work or {}
      M.idle_work[#M.idle_work + 1] = function()
        phrase.lines = math.min(512, phrase.lines * 2)
        M.fire(obs(phrase, "number_of_lines"))
      end
      return true
    end,
  }, {
    paragraphs = function() return copy(script.paragraphs) end,
    compile_error = function() return script.compile_error end,
    runtime_error = function() return script.runtime_error end,
    pending_changes = function() return script.pending end,
  }, {
    paragraphs = function(value)
      if type(value) ~= "table" then error("bad argument: table expected", 2) end
      local old = copy(script.paragraphs)
      script.paragraphs = copy(value)
      script.pending = script.pending + 1
      M.fire(obs(script, "paragraphs"))
      local new = copy(value)
      -- (Renoise's undo and redo put the text back uncommitted: the player keeps what was committed, lab "pending 2")
      M.record(function() script.paragraphs = copy(old); script.pending = script.pending + 1; M.fire(obs(script, "paragraphs")) end,
        function() script.paragraphs = copy(new); script.pending = script.pending + 1; M.fire(obs(script, "paragraphs")) end)
    end,
  })
end

-- What Renoise's player does with committed scripts (the test runs it): a script with "error(" fails at runtime, one without
-- `return` fails to compile; the others compile.
function M.compile_scripts()
  for _, script in ipairs(M.pending_compiles or {}) do
    local text = table.concat(script.committed_text or script.paragraphs, "\n")
    script.committed = copy(script.paragraphs)
    local compile, runtime = "", ""
    if not text:find("return", 1, true) then compile = "[string \"script\"]:1: 'return' expected"
    elseif text:find("error(", 1, true) then runtime = "[string \"script\"]:1: boom" end
    if script.compile_error ~= compile then script.compile_error = compile M.fire(obs(script, "compile_error")) end
    if script.runtime_error ~= runtime then script.runtime_error = runtime M.fire(obs(script, "runtime_error")) end
  end
  M.pending_compiles = {}
end

-- A render of a script to the phrase's lines: notes on every 4th line, in its own undo step (as Renoise's callback does), which
-- also takes back the length Renoise rewrote and the switch to playing the lines (one undo reverts the whole render: lab).
function M.finish_renders(fail)
  for _, render in ipairs(M.pending_renders or {}) do
    if fail then
      render.done("rendering failed", 0, 0)
    else
      local phrase = render.phrase
      M.callback_begin()
      local count = 0
      for k = 1, phrase.lines, 4 do
        local l = phrase_line(phrase, k)
        l:note_column(1).note_string = "C-4"
        count = count + 1
      end
      local before, after = render.lines, phrase.lines
      M.record(function() phrase.lines = before; M.fire(obs(phrase, "number_of_lines")) end,
        function() phrase.lines = after; M.fire(obs(phrase, "number_of_lines")) end)
      write(phrase, "playback_mode", 1)
      M.callback_end()
      render.done(nil, count, 0)
    end
  end
  M.pending_renders = {}
end

local function mapping_object(instrument, m)
  local mapping = instrument.mappings[m]
  local getters, setters, fields = fields_of(mapping, {
    key_tracking = { 1, range(1, 3, true) }, base_note = { 48, range(0, 119, true) }, looping = { true, boolean },
    loop_start = { 1, range(1, 512, true) }, loop_end = { 16, range(1, 512, true) },
  })
  -- (mappings may not overlap: a range is refused when it reaches a neighbour's)
  getters.note_range = function() return copy(mapping.note_range) end
  setters.note_range = function(value)
    local prev, nxt = instrument.mappings[m - 1], instrument.mappings[m + 1]
    if value[1] > value[2] or (prev and value[1] <= prev.note_range[2]) or (nxt and value[2] >= nxt.note_range[1]) then
      error("phrase mappings may not overlap", 2)
    end
    write(mapping, "note_range", value)
  end
  fields.note_range_observable = obs(mapping, "note_range")
  -- (the mapping's key tracking, base note and loop are its own, not its phrase's: lab)
  getters.phrase = function()
    for k, p in ipairs(instrument.phrases) do if p == mapping.phrase then return M.phrase_proxy(instrument, k) end end
    error("mapping without phrase", 2)
  end
  return object("renoise.InstrumentPhraseMapping", fields, getters, setters)
end

function M.phrase_proxy(instrument, ph)
  local phrase = instrument.phrases[ph]
  local getters, setters, fields = fields_of(phrase, PHRASE_SPEC)
  -- (a phrase switched to play its script takes its default name, "Phrase 01": lab 2026-10-19)
  setters.playback_mode = function(value)
    check_range("playback_mode", value, 1, 2, true)
    if value == 2 and phrase.playback_mode ~= 2 then
      for k, candidate in ipairs(instrument.phrases) do
        if candidate == phrase then write(phrase, "name", string.format("Phrase %02d", k)) end
      end
    end
    write(phrase, "playback_mode", value)
  end
  getters.number_of_lines = function() return phrase.lines end
  setters.number_of_lines = function(value)
    check_range("number_of_lines", value, 1, 512, true)
    write(phrase, "lines", value)
    -- (Renoise keeps the loop inside the phrase)
    if phrase.loop_end > value then write(phrase, "loop_end", value) end
    if phrase.loop_start > value then write(phrase, "loop_start", value) end
    M.fire(obs(phrase, "number_of_lines"))
  end
  fields.number_of_lines_observable = obs(phrase, "number_of_lines")
  getters.visible_note_columns = function() return phrase.note_columns end
  setters.visible_note_columns = function(value) check_range("visible_note_columns", value, 1, 12, true) write(phrase, "note_columns", value) M.fire(obs(phrase, "visible_note_columns")) end
  fields.visible_note_columns_observable = obs(phrase, "visible_note_columns")
  getters.visible_effect_columns = function() return phrase.effect_columns end
  setters.visible_effect_columns = function(value) check_range("visible_effect_columns", value, 0, 8, true) write(phrase, "effect_columns", value) M.fire(obs(phrase, "visible_effect_columns")) end
  fields.visible_effect_columns_observable = obs(phrase, "visible_effect_columns")
  for _, field in ipairs({ "loop_start", "loop_end" }) do
    getters[field] = function() return phrase[field] end
    setters[field] = function(value)
      check_range(field, value, 1, phrase.lines, true)
      if field == "loop_start" and value > phrase.loop_end then error("loop_start must not be after loop_end", 2) end
      if field == "loop_end" and value < phrase.loop_start then error("loop_end must not be before loop_start", 2) end
      write(phrase, field, value)
    end
    fields[field .. "_observable"] = obs(phrase, field)
  end
  getters.is_empty = function()
    for _, l in pairs(phrase.data) do if not line(l).is_empty then return false end end
    return true
  end
  fields.is_empty_observable = obs(phrase, "is_empty")
  getters.mapping = function()
    for m, mapping in ipairs(instrument.mappings) do if mapping.phrase == phrase then return mapping_object(instrument, m) end end
    return nil
  end
  fields.script = script_object(phrase)
  fields.line = function(_, k)
    if type(k) ~= "number" or k < 1 or k > 512 then out_of_range("phrase line", k) end
    return phrase_line(phrase, k)
  end
  fields.column_is_muted = function(_, c) return (phrase.muted_columns or {})[c] == true end
  fields.column_name = function(_, c) return (phrase.column_names or {})[c] or "" end
  fields.column_is_muted_observable = function(_, c) return obs(phrase, "muted" .. c) end
  fields.column_name_observable = function(_, c) return obs(phrase, "colname" .. c) end
  fields.set_column_is_muted = function(_, c, muted)
    phrase.muted_columns = phrase.muted_columns or {}
    write(phrase.muted_columns, c, muted or nil)
  end
  fields.set_column_name = function(_, c, name)
    phrase.column_names = phrase.column_names or {}
    write(phrase.column_names, c, name ~= "" and name or nil)
  end
  fields.clear = function()
    local old = phrase.data
    phrase.data = {}
    for _, fn in ipairs(list(phrase.notifiers)) do pcall(fn, { line = 1 }) end
    M.record(function() phrase.data = old end, function() phrase.data = {} end)
  end
  fields.copy_from = function(_, other)
    local from = M.impl(other).__data
    for k, v in pairs(from) do if k ~= "obs" and k ~= "notifiers" then write(phrase, k, copy(v)) end end
    for _, fn in ipairs(list(phrase.notifiers)) do pcall(fn, { line = 1 }) end
  end
  fields.add_line_notifier = function(_, fn)
    for _, existing in ipairs(phrase.notifiers) do if existing == fn then error("the notifier was already added", 2) end end
    M.notifier_adds = (M.notifier_adds or 0) + 1
    phrase.notifiers[#phrase.notifiers + 1] = fn
  end
  fields.remove_line_notifier = function(_, fn)
    for n, existing in ipairs(phrase.notifiers) do if existing == fn then table.remove(phrase.notifiers, n) return end end
    error("the notifier was not added", 2)
  end
  fields.has_line_notifier = function(_, fn)
    for _, existing in ipairs(phrase.notifiers) do if existing == fn then return true end end
    return false
  end
  fields.__data = phrase
  return object("renoise.InstrumentPhrase", fields, getters, setters)
end

local function modulation_device_object(set, k)
  local data = set.devices[k]
  local kind = M.MODULATION_DEVICES[data.path]
  local getters, setters, fields = fields_of(data, {
    operator = { 1, range(1, 4, true) }, bipolar = { false, boolean }, tempo_synced = { false, boolean },
    display_name = { data.name, text_value }, is_maximized = { true, boolean },
  })
  getters.is_active = function() return data.active ~= false end
  setters.is_active = function(value) boolean(value) write(data, "active", value) end
  fields.is_active_observable = obs(data, "active")
  fields.name, fields.short_name, fields.target = data.name, data.name, data.target
  fields.tempo_sync_switching_allowed = kind.sync == true
  fields.parameters = parameters_of(data)
  fields.parameter = function(_, i)
    if not fields.parameters[i] then out_of_range("parameter", i) end
    return fields.parameters[i]
  end
  fields.copy_from = function(_, other)
    local from = M.impl(other).__data
    if from.path ~= data.path then error("devices must be of the same type", 2) end
    for key, v in pairs(from) do if key ~= "obs" and key ~= "target" then data[key] = copy(v) end end
    for i, p in ipairs(data.params) do p.value_observable = nil end
  end
  fields.__data = data
  local class = "renoise.SampleModulationDevice"
  if kind.points then
    class = data.path:find("Stepper") and "renoise.SampleStepperModulationDevice" or "renoise.SampleEnvelopeModulationDevice"
    getters.points = function() return copy(data.points) end
  local _ = kind
    setters.points = function(value)
      local seen = {}
      for _, point in ipairs(value) do
        if seen[point.time] then error("points may not share a time", 2) end
        seen[point.time] = true
        check_range("time", point.time, 1, data.length, true)
      end
      local sorted = copy(value)
      table.sort(sorted, function(a, b) return a.time < b.time end)
      for _, point in ipairs(sorted) do point.scaling = point.scaling or 0 end
      write(data, "points", sorted)
    end
    fields.points_observable = obs(data, "points")
    local extra_getters, extra_setters, extra = fields_of(data, {
      length = { 64, range(class == "renoise.SampleStepperModulationDevice" and 1 or 6, class == "renoise.SampleStepperModulationDevice" and 256 or 6144, true) },
      play_mode = { 2, range(1, 3, true) },
    })
    for key, fn in pairs(extra_getters) do getters[key] = fn end
    for key, fn in pairs(extra_setters) do setters[key] = fn end
    for key, value in pairs(extra) do fields[key] = value end
    if class == "renoise.SampleEnvelopeModulationDevice" then
      local env_getters, env_setters, env = fields_of(data, {
        loop_mode = { 1, range(1, 4, true) }, loop_start = { 1, range(1, 6144, true) }, loop_end = { 64, range(1, 6144, true) },
        sustain_enabled = { false, boolean }, sustain_position = { 1, range(1, 6144, true) }, fade_amount = { 0, range(0, 4095, true) },
      })
      for key, fn in pairs(env_getters) do getters[key] = fn end
      for key, fn in pairs(env_setters) do setters[key] = fn end
      for key, value in pairs(env) do fields[key] = value end
    else
      getters.play_step = function() return data.play_step or 0 end
      setters.play_step = function(value) check_range("play_step", value, -1, 16, true) write(data, "play_step", value) end
      fields.play_step_observable = obs(data, "play_step")
    end
  end
  -- (the plain fields of a type that aren't parameters: 3.5's API types)
  local TYPED = {
    ["Modulation/LFO"] = { "renoise.SampleLfoModulationDevice", "mode", 1, 4 },
    ["Modulation/Velocity Tracking"] = { "renoise.SampleVelocityTrackingModulationDevice", "mode", 1, 2 },
    ["Modulation/Fader"] = { "renoise.SampleFaderModulationDevice", "scaling", 3, 5 },
  }
  local typed = TYPED[data.path]
  if typed then
    class = typed[1]
    local typed_getters, typed_setters, typed_fields = fields_of(data, { [typed[2]] = { typed[3], range(1, typed[4], true) } })
    for key, fn in pairs(typed_getters) do getters[key] = fn end
    for key, fn in pairs(typed_setters) do setters[key] = fn end
    for key, value in pairs(typed_fields) do fields[key] = value end
  end
  return object(class, fields, getters, setters)
end

local INPUTS = { volume_input = "volume", panning_input = "panning", pitch_input = "pitch", cutoff_input = "cutoff", resonance_input = "resonance", drive_input = "drive" }
-- (each input's range, as Renoise has it: lab)
local INPUT_RANGES = { volume = { 0, 1 }, panning = { -1, 1 }, pitch = { -1, 1 }, cutoff = { 0, 127 }, resonance = { 0, 127 }, drive = { 0, 127 } }

local function set_object(instrument, m)
  local set = instrument.sets[m]
  set.devices_observable = set.devices_observable or M.observable()
  local getters, setters, fields = fields_of(set, { name = { "Modulation", text_value }, pitch_range = { 12, range(1, 96, true) } })
  getters.filter_type = function() return set.filter_type end
  setters.filter_type = function(value)
    local ok = false
    for _, name in ipairs(M.FILTER_TYPES) do if name == value then ok = true end end
    if not ok then error("invalid filter type: " .. tostring(value), 2) end
    write(set, "filter_type", value)
  end
  fields.filter_type_observable = obs(set, "filter_type")
  fields.filter_version, fields.filter_version_observable = set.filter_version, obs(set, "filter_version")
  fields.available_filter_types = copy(M.FILTER_TYPES)
  fields.available_devices = copy(MODULATION_PATHS)
  fields.devices_observable = set.devices_observable
  for member, key in pairs(INPUTS) do
    set.input_params = set.input_params or {}
    set.input_params[key] = set.input_params[key] or { name = key:sub(1, 1):upper() .. key:sub(2), min = INPUT_RANGES[key][1], max = INPUT_RANGES[key][2] }
    local holder = set.input_params[key]
    holder.value = set.inputs[key]
    fields[member] = h.parameter(holder, {
      value = function(value)
        check_range(member, value, INPUT_RANGES[key][1], INPUT_RANGES[key][2])
        local old = set.inputs[key]
        if old == value then return end
        set.inputs[key], holder.value = value, value
        M.fire(holder.value_observable)
        M.record(function() set.inputs[key], holder.value = old, old end, function() set.inputs[key], holder.value = value, value end)
      end,
    })
  end
  getters.devices = function()
    local out = {}
    for k in ipairs(set.devices) do out[k] = modulation_device_object(set, k) end
    return out
  end
  fields.device = function(_, k)
    if not set.devices[k] then out_of_range("device", k) end
    return modulation_device_object(set, k)
  end
  fields.insert_device_at = function(_, path, target, index)
    if not M.MODULATION_DEVICES[path] then error("unknown modulation device: " .. tostring(path), 2) end
    check_range("target", target, 1, 6, true)
    if index < 1 or index > #set.devices + 1 then out_of_range("device", index) end
    list_op(set.devices, set.devices_observable, "insert", index, nil, modulation_device_data(path, target), 0, 1)
    return modulation_device_object(set, index)
  end
  fields.delete_device_at = function(_, index)
    if not set.devices[index] then out_of_range("device", index) end
    list_op(set.devices, set.devices_observable, "remove", index, nil, nil, 0, 1)
  end
  fields.init = function() end
  fields.copy_from = function() end
  fields.upgrade_filter_version = function() end
  fields.__data = set
  return object("renoise.SampleModulationSet", fields, getters, setters)
end

M.FILTER_TYPES = { "None", "LP Clean", "LP K35", "LP Moog", "LP Diode", "HP Clean", "HP K35", "HP Moog", "BP Clean", "BP K35", "BP Moog",
  "BandPass", "BandStop", "Vowel", "Comb", "Decimator", "Dist Shape", "Dist Fold", "AM Sine", "AM Triangle", "AM Saw", "AM Pulse" }

local function chain_object(instrument, c)
  local chain = instrument.chains[c]
  chain.devices_observable = chain.devices_observable or M.observable()
  local getters, setters, fields = fields_of(chain, { name = { "FX", text_value } })
  getters.output_routing = function() return chain.output_routing end
  setters.output_routing = function(value)
    local ok = false
    for _, name in ipairs(M.chain_routings()) do if name == value then ok = true end end
    if not ok then error("invalid output routing: " .. tostring(value), 2) end
    write(chain, "output_routing", value)
  end
  fields.output_routing_observable = obs(chain, "output_routing")
  getters.available_output_routings = function() return M.chain_routings() end
  getters.available_devices = function()
    local paths = {}
    for path in pairs(M.device_catalog or {}) do paths[#paths + 1] = path end
    table.sort(paths)
    return paths
  end
  -- (a sample FX chain has no available_device_infos: lab)
  fields.devices_observable = chain.devices_observable
  getters.devices = function()
    local out = {}
    for k, data in ipairs(chain.devices) do out[k] = device(data) end
    return out
  end
  fields.device = function(_, k)
    if not chain.devices[k] then out_of_range("device", k) end
    return device(chain.devices[k])
  end
  fields.insert_device_at = function(_, path, index)
    if index < 2 or index > #chain.devices + 1 then out_of_range("device", index) end
    if path:find("Missing", 1, true) or not (M.device_catalog or {})[path] then error("std::logic_error: 'unknown device " .. path .. "'", 2) end
    local data = { name = path:match("([^/]+)$") or path, path = path, params = {}, active = true }
    for k, template in ipairs((M.device_catalog or {})[path] or {}) do
      data.params[k] = { name = template.name, value = template.value, min = template.min, max = template.max }
    end
    list_op(chain.devices, chain.devices_observable, "insert", index, nil, data, 1)
    return device(data)
  end
  fields.delete_device_at = function(_, index)
    if index == 1 then error("the mixer device can't be deleted", 2) end
    list_op(chain.devices, chain.devices_observable, "remove", index, nil, nil, 1)
  end
  fields.swap_devices_at = function(_, a, b)
    if a == 1 or b == 1 then error("the mixer device must stay first", 2) end
    list_op(chain.devices, chain.devices_observable, "swap", a, b, nil, 1)
  end
  fields.__data = chain
  return object("renoise.SampleDeviceChain", fields, getters, setters)
end

-- Where a chain may send its sound: the current track, or a sequencer track as "NN: name" (lab).
function M.chain_routings()
  local out = { "Current Track" }
  for t, data in ipairs(M.data.tracks) do
    if data.kind == "sequencer" then out[#out + 1] = string.format("%02d: %s", t, data.name) end
  end
  return out
end

-- The plug-ins Renoise found (tests add theirs to M.plugin_catalog: path -> {name, params, presets}).
local function plugin_infos()
  local infos = {}
  for path, spec in pairs(M.plugin_catalog or {}) do
    infos[#infos + 1] = { path = path, name = spec.name, short_name = spec.name, favorite_name = spec.name, is_favorite = false, is_bridged = false }
  end
  table.sort(infos, function(a, b) return a.path < b.path end)
  return infos
end

local function plugin_device_object(instrument)
  local plugin = instrument.plugin
  plugin.params = plugin.params or {}
  local params = parameters_of(plugin)
  return object("renoise.InstrumentPluginDevice", {
    name = plugin.name, short_name = plugin.name, device_path = plugin.path, presets = copy(plugin.presets or {}),
    active_preset_observable = obs(plugin, "preset"), parameters = params, external_editor_available = true, external_editor_visible = false,
    preset = function(_, i)
      if not (plugin.presets or {})[i] then out_of_range("preset", i) end
      return plugin.presets[i]
    end,
    parameter = function(_, i)
      if not params[i] then out_of_range("parameter", i) end
      return params[i]
    end,
  }, {
    active_preset = function() return plugin.preset or 0 end,
    active_preset_data = function() return plugin.preset_data or "" end,
  }, {
    active_preset = function(value)
      check_range("active_preset", value, 0, #(plugin.presets or {}), true)
      -- (a plug-in that keeps its program reads none chosen, its state as it was: Apple's DLSMusicDevice, lab)
      if plugin.keeps_program then return end
      write(plugin, "preset", value, plugin.preset_not_a_step)
      -- (a program sets the plug-in's parameters: the mock moves the first to the program's number)
      if plugin.params[1] and value > 0 then plugin.params[1].value = math.min(plugin.params[1].max or 1, value / 10) end
    end,
    active_preset_data = function(value) write(plugin, "preset_data", value) end,
  })
end

local function plugin_props_object(instrument)
  local props = instrument.plugin_props
  local getters, setters, fields = fields_of(props, {
    channel = { 1, range(1, 16, true) }, transpose = { 0, range(-120, 120, true) }, volume = { 1, range(0, 4) },
    -- (auto-suspend is no undo step: lab)
    auto_suspend = { false, boolean, true },
  })
  fields.plugin_device_observable = obs(instrument, "plugin")
  fields.alias_instrument_index, fields.alias_fx_track_index, fields.alias_fx_device_index, fields.midi_output_routing_index = 0, 0, 0, 0
  fields.alias_instrument_index_observable, fields.alias_fx_track_index_observable = obs(props, "alias_i"), obs(props, "alias_t")
  fields.alias_fx_device_index_observable, fields.midi_output_routing_index_observable = obs(props, "alias_d"), obs(props, "routing")
  getters.available_plugins = function()
    local out = {}
    for k, info in ipairs(plugin_infos()) do out[k] = info.path end
    return out
  end
  getters.available_plugin_infos = plugin_infos
  getters.plugin_loaded = function() return instrument.plugin ~= nil end
  getters.plugin_device = function()
    if not instrument.plugin then return nil end
    return plugin_device_object(instrument)
  end
  fields.load_plugin = function(_, path)
    local old = instrument.plugin
    -- (a plug-in Renoise lets go of the old one for, then fails to load: M.plugin_unloads_and_fails names it)
    if M.plugin_unloads_and_fails and path == M.plugin_unloads_and_fails then
      instrument.plugin = nil
      M.fire(obs(instrument, "plugin"))
      return false
    end
    local made
    if path ~= "" then
      local spec = (M.plugin_catalog or {})[path]
      if not spec then return false end
      made = { path = path, name = spec.name, presets = copy(spec.presets or {}), preset = 0, preset_data = spec.preset_data or ("<plugin " .. spec.name .. ">"), params = {} }
      for k, p in ipairs(spec.params or {}) do made.params[k] = { name = p.name, value = p.value or 0, min = p.min or 0, max = p.max or 1 } end
    end
    instrument.plugin = made
    M.fire(obs(instrument, "plugin"))
    M.record(function() instrument.plugin = old; M.fire(obs(instrument, "plugin")) end, function() instrument.plugin = made; M.fire(obs(instrument, "plugin")) end)
    -- (Renoise names the instrument after the plug-in it loads, and leaves it unnamed when the plug-in goes: lab 2026-10-19)
    local kind = path:match("^Audio/Generators/([^/]+)/") or "VST"
    write(instrument, "name", made and (kind .. ": " .. made.name) or "")
    return true
  end
  return object("renoise.InstrumentPluginProperties", fields, getters, setters)
end

local function midi_object(instrument)
  local midi = instrument.midi
  local getters, setters, fields = fields_of(midi, {
    instrument_type = { 1, range(1, 3, true) }, channel = { 1, range(1, 16, true) }, transpose = { 0, range(-120, 120, true) },
    program = { 0, range(0, 128, true) }, bank = { 0, range(0, 65536, true) }, delay = { 0, range(0, 100, true) }, duration = { 8000, range(1, 8000, true) },
  })
  getters.device_name = function() return midi.device_name end
  setters.device_name = function(value)
    local ok = value == ""
    for _, name in ipairs(M.midi_outputs or {}) do if name == value then ok = true end end
    if not ok then error("invalid device name: " .. tostring(value), 2) end
    write(midi, "device_name", value, M.midi_not_steps)
  end
  fields.device_name_observable = obs(midi, "device_name")
  fields.TYPE_EXTERNAL, fields.TYPE_LINE_IN_RET, fields.TYPE_INTERNAL = 1, 2, 3
  return object("renoise.InstrumentMidiOutputProperties", fields, getters, setters)
end

local function macro_object(macro)
  local getters, setters, fields = fields_of(macro, { name = { "", text_value }, value = { 0, range(0, 1) } })
  getters.value_string = function() return string.format("%d %%", math.floor(macro.value * 100 + 0.5)) end
  fields.value_string_observable = obs(macro, "value_string")
  fields.mappings_observable = obs(macro, "mappings")
  -- (a mapping a test gives: {name (or param) of the parameter, min, max, scaling}; the API can't make one, but writes its
  -- range and scaling, each an undo step: M4 n-devices)
  local function mapping_object(m)
    local parameter = h.parameter({ name = m.name or m.param or "Param", value = 0, min = 0, max = 1 }, {})
    local get, set, watched = fields_of(m, { min = { 0, range(0, 1) }, max = { 1, range(0, 1) }, scaling = { 3, range(1, 5, true) } })
    return object("renoise.InstrumentMacroMapping", {
      parameter = parameter,
      parameter_min_observable = watched.min_observable, parameter_max_observable = watched.max_observable,
      parameter_scaling_observable = watched.scaling_observable,
    }, { parameter_min = get.min, parameter_max = get.max, parameter_scaling = get.scaling },
      { parameter_min = set.min, parameter_max = set.max, parameter_scaling = set.scaling })
  end
  getters.mappings = function()
    local out = {}
    for k, m in ipairs(macro.mappings or {}) do out[k] = mapping_object(m) end
    return out
  end
  fields.mapping = function(_, i)
    if not (macro.mappings or {})[i] then out_of_range("mapping", i) end
    return mapping_object(macro.mappings[i])
  end
  return object("renoise.InstrumentMacro", fields, getters, setters)
end

local function trigger_object(instrument)
  local trigger = instrument.trigger
  local getters, setters, fields = fields_of(trigger, {
    quantize = { 1, range(1, 4, true) }, monophonic = { false, boolean }, monophonic_glide = { 0, range(0, 255, true) },
    scale_key = { 1, range(1, 12, true) },
  })
  getters.scale_mode = function() return trigger.scale_mode end
  setters.scale_mode = function(value)
    local ok = false
    for _, name in ipairs(M.SCALES) do if name == value then ok = true end end
    if not ok then error("invalid scale mode: " .. tostring(value), 2) end
    write(trigger, "scale_mode", value)
  end
  fields.scale_mode_observable = obs(trigger, "scale_mode")
  fields.available_scale_modes = copy(M.SCALES)
  fields.QUANTIZE_NONE, fields.QUANTIZE_LINE, fields.QUANTIZE_BEAT, fields.QUANTIZE_BAR = 1, 2, 3, 4
  return object("renoise.InstrumentTriggerOptions", fields, getters, setters)
end

M.SCALES = { "None", "Natural Major", "Natural Minor", "Pentatonic Major", "Pentatonic Minor", "Dorian", "Phrygian", "Lydian", "Mixolydian", "Blues" }

local INSTRUMENT_SPEC = {
  name = { "", text_value }, volume = { 1, range(0, 1.9952623149689) }, transpose = { 0, range(-120, 120, true) },
}

function M.instrument_proxy(i)
  local d = M.data
  local instrument = type(i) == "number" and d.instruments[i]
  if not instrument then out_of_range("instrument", i) end
  for _, key in ipairs({ "samples", "phrases", "sets", "chains", "mappings" }) do
    instrument[key .. "_observable"] = instrument[key .. "_observable"] or M.observable()
  end
  local getters, setters, fields = fields_of(instrument, INSTRUMENT_SPEC)
  -- (the M3 code's observables keep their names)
  fields.name_observable = obs(instrument, "name")
  fields.samples_observable, fields.phrases_observable = instrument.samples_observable, instrument.phrases_observable
  fields.sample_modulation_sets_observable, fields.sample_device_chains_observable = instrument.sets_observable, instrument.chains_observable
  fields.phrase_mappings_observable, fields.sample_mappings_observable = instrument.mappings_observable, obs(instrument, "sample_mappings")
  getters.sample_mapping_overlap_mode = function() return instrument.overlap end
  setters.sample_mapping_overlap_mode = function(value) check_range("overlap", value, 0, 2, true) write(instrument, "overlap", value) end
  fields.sample_mapping_overlap_mode_observable = obs(instrument, "overlap")
  getters.phrase_playback_mode = function() return instrument.phrase_playback_mode end
  setters.phrase_playback_mode = function(value)
    check_range("phrase_playback_mode", value, 1, 3, true)
    write(instrument, "phrase_playback_mode", value)
    -- (keymap is program 127; off is 0; program mode keeps a phrase's number, or none: 0)
    local program = value == 3 and 127 or (value == 1 and 0 or (instrument.phrase_program >= 1 and instrument.phrase_program <= 126 and instrument.phrase_program or 0))
    write(instrument, "phrase_program", program)
  end
  fields.phrase_playback_mode_observable = obs(instrument, "phrase_playback_mode")
  getters.phrase_program = function() return instrument.phrase_program end
  setters.phrase_program = function(value)
    check_range("phrase_program", value, 0, 127, true)
    write(instrument, "phrase_program", value)
    -- (the program sets the mode: 0 off, 127 keymap, a phrase program mode; lab pending for 0 in program mode)
    write(instrument, "phrase_playback_mode", value == 0 and 1 or (value == 127 and 3 or 2))
  end
  fields.phrase_program_observable = obs(instrument, "phrase_program")
  getters.active_tab = function() return instrument.tab or 1 end
  setters.active_tab = function(value) check_range("active_tab", value, 1, 3, true) write(instrument, "tab", value, true) end
  fields.active_tab_observable = obs(instrument, "tab")
  fields.comments, fields.comments_observable, fields.comments_assignment_observable = {}, obs(instrument, "comments"), obs(instrument, "comments_a")
  fields.macros_visible, fields.macros_visible_observable = false, obs(instrument, "macros_visible")
  fields.phrase_editor_visible, fields.phrase_editor_visible_observable = false, obs(instrument, "phrase_editor_visible")
  fields.show_comments_after_loading, fields.show_comments_after_loading_observable = false, obs(instrument, "show_comments")
  fields.trigger_options = trigger_object(instrument)
  fields.midi_output_properties = midi_object(instrument)
  fields.plugin_properties = plugin_props_object(instrument)
  fields.pitchbend_macro, fields.modulation_wheel_macro, fields.channel_pressure_macro =
    macro_object(instrument.pitchbend), macro_object(instrument.modwheel), macro_object(instrument.pressure)
  fields.macro = function(_, k)
    if not instrument.macros[k] then out_of_range("macro", k) end
    return macro_object(instrument.macros[k])
  end
  getters.macros = function()
    local out = {}
    for k, macro in ipairs(instrument.macros) do out[k] = macro_object(macro) end
    return out
  end
  getters.samples = function()
    local out = {}
    for s in ipairs(instrument.samples) do out[s] = sample_object(instrument, s) end
    return out
  end
  getters.phrases = function()
    local out = {}
    for ph in ipairs(instrument.phrases) do out[ph] = M.phrase_proxy(instrument, ph) end
    return out
  end
  getters.phrase_mappings = function()
    local out = {}
    for m in ipairs(instrument.mappings) do out[m] = mapping_object(instrument, m) end
    return out
  end
  getters.sample_mappings = function()
    local on, off = {}, {}
    for s, sample in ipairs(instrument.samples) do
      local proxy = sample_object(instrument, s).sample_mapping
      if sample.layer == 2 then off[#off + 1] = proxy else on[#on + 1] = proxy end
    end
    return { on, off }
  end
  getters.sample_modulation_sets = function()
    local out = {}
    for m in ipairs(instrument.sets) do out[m] = set_object(instrument, m) end
    return out
  end
  getters.sample_device_chains = function()
    local out = {}
    for c in ipairs(instrument.chains) do out[c] = chain_object(instrument, c) end
    return out
  end
  fields.sample = function(_, s)
    if not instrument.samples[s] then out_of_range("sample", s) end
    return sample_object(instrument, s)
  end
  fields.phrase = function(_, ph)
    if not instrument.phrases[ph] then out_of_range("phrase", ph) end
    return M.phrase_proxy(instrument, ph)
  end
  fields.phrase_mapping = function(_, m)
    if not instrument.mappings[m] then out_of_range("phrase mapping", m) end
    return mapping_object(instrument, m)
  end
  fields.sample_mapping = function(_, layer, k)
    local list_ = getters.sample_mappings()[layer]
    if not list_ or not list_[k] then out_of_range("sample mapping", k) end
    return list_[k]
  end
  fields.sample_modulation_set = function(_, m)
    if not instrument.sets[m] then out_of_range("modulation set", m) end
    return set_object(instrument, m)
  end
  fields.sample_device_chain = function(_, c)
    if not instrument.chains[c] then out_of_range("device chain", c) end
    return chain_object(instrument, c)
  end
  fields.insert_sample_at = function(_, s)
    if instrument.samples[1] and #instrument.samples[1].markers > 0 then error("can't insert samples into a sliced instrument", 2) end
    if s < 1 or s > #instrument.samples + 1 then out_of_range("sample", s) end
    -- (a new sample takes the whole keyboard, or the next key when the instrument is a drum kit: one key per sample)
    local spec = { name = "" }
    local drumkit = #instrument.samples > 0
    for _, sample in ipairs(instrument.samples) do
      if sample.note_range[1] ~= sample.note_range[2] then drumkit = false end
    end
    if drumkit then
      local top = 0
      for _, sample in ipairs(instrument.samples) do top = math.max(top, sample.note_range[2]) end
      spec.note_range, spec.base_note = { math.min(119, top + 1), math.min(119, top + 1) }, math.min(119, top + 1)
    end
    -- (a new sample uses the instrument's first set: lab)
    if #instrument.sets > 0 then spec.modulation_set_index = 1 end
    list_op(instrument.samples, instrument.samples_observable, "insert", s, nil, M.sample_data(spec))
    return sample_object(instrument, s)
  end
  fields.delete_sample_at = function(_, s)
    if not instrument.samples[s] then out_of_range("sample", s) end
    if instrument.samples[s].slice_alias then error("slice aliases can't be deleted: delete the slice marker", 2) end
    if s == 1 and #instrument.samples[1].markers > 0 then
      -- (the master of slices goes with its aliases)
      M.set_markers(instrument, {}, false)
    end
    list_op(instrument.samples, instrument.samples_observable, "remove", s)
    -- (a sample's sets and chains are numbers: nothing to renumber here)
    -- (an instrument left with no samples loses its name: lab 2026-10-19)
    if #instrument.samples == 0 then write(instrument, "name", "") end
  end
  fields.swap_samples_at = function(_, a, b)
    if (instrument.samples[a] or {}).slice_alias or (instrument.samples[b] or {}).slice_alias then error("slice aliases can't be moved", 2) end
    list_op(instrument.samples, instrument.samples_observable, "swap", a, b)
  end
  fields.insert_phrase_at = function(_, ph)
    if #instrument.phrases >= 126 then error("can't have more than 126 phrases", 2) end
    if ph < 1 or ph > #instrument.phrases + 1 then out_of_range("phrase", ph) end
    -- (Renoise's new phrase: lab)
    renumber_phrases(instrument, "insert", ph)
    list_op(instrument.phrases, instrument.phrases_observable, "insert", ph, nil, phrase_data({ name = string.format("Phrase %02d", #instrument.phrases + 1),
      lpb = M.data.transport_fields.lpb, lines = 32, loop_end = 32, key_tracking = 2, autoseek = true, note_columns = 4, instrument_column_visible = false }))
    return M.phrase_proxy(instrument, ph)
  end
  fields.delete_phrase_at = function(_, ph)
    if not instrument.phrases[ph] then out_of_range("phrase", ph) end
    local phrase = instrument.phrases[ph]
    for m, mapping in ipairs(instrument.mappings) do
      if mapping.phrase == phrase then list_op(instrument.mappings, instrument.mappings_observable, "remove", m) break end
    end
    renumber_phrases(instrument, "remove", ph)
    list_op(instrument.phrases, instrument.phrases_observable, "remove", ph)
    -- (measured, 3.5.4: the program moves off a phrase deleted, 2 to 1, and down with the phrases after it; an insert leaves
    -- it where it is: M7 lab 2026-10-21)
    local program = instrument.phrase_program
    if program == ph then
      write(instrument, "phrase_program", #instrument.phrases == 0 and 0 or math.max(1, ph - 1))
    elseif program > ph then
      write(instrument, "phrase_program", program - 1)
    end
    -- (a test's Renoise that moves the program as it was also seen to, off its phrase: program 2, delete 4 -> 3, lab 2026-10-21)
    if M.program_after_delete then
      local moved = M.program_after_delete(program, ph, #instrument.phrases)
      if moved then write(instrument, "phrase_program", moved) end
    end
  end
  fields.can_insert_phrase_mapping_at = function(_, m) return m >= 1 and m <= #instrument.mappings + 1 end
  fields.insert_phrase_mapping_at = function(_, m, phrase_proxy)
    local phrase = M.impl(phrase_proxy).__data
    for _, mapping in ipairs(instrument.mappings) do
      if mapping.phrase == phrase then error("the phrase has a mapping already", 2) end
    end
    if m < 1 or m > #instrument.mappings + 1 then out_of_range("phrase mapping", m) end
    -- (the free range between the mappings before and after it)
    local low = instrument.mappings[m - 1] and instrument.mappings[m - 1].note_range[2] + 1 or 0
    local high = instrument.mappings[m] and instrument.mappings[m].note_range[1] - 1 or 119
    if low > high then error("no room for a phrase mapping there", 2) end
    list_op(instrument.mappings, instrument.mappings_observable, "insert", m, nil, { phrase = phrase, note_range = { low, high },
      key_tracking = phrase.key_tracking, base_note = phrase.base_note, looping = phrase.looping, loop_start = phrase.loop_start, loop_end = phrase.loop_end })
    return mapping_object(instrument, m)
  end
  fields.delete_phrase_mapping_at = function(_, m)
    if not instrument.mappings[m] then out_of_range("phrase mapping", m) end
    list_op(instrument.mappings, instrument.mappings_observable, "remove", m)
  end
  fields.insert_sample_modulation_set_at = function(_, m)
    if m < 1 or m > #instrument.sets + 1 then out_of_range("modulation set", m) end
    list_op(instrument.sets, instrument.sets_observable, "insert", m, nil, set_data({ name = string.format("Set %02d", #instrument.sets + 1) }))
    M.renumber_assignments(instrument, "modulation_set_index", "insert", m)
    return set_object(instrument, m)
  end
  fields.delete_sample_modulation_set_at = function(_, m)
    if not instrument.sets[m] then out_of_range("modulation set", m) end
    M.renumber_assignments(instrument, "modulation_set_index", "remove", m)
    list_op(instrument.sets, instrument.sets_observable, "remove", m)
  end
  fields.swap_sample_modulation_sets_at = function(_, a, b)
    list_op(instrument.sets, instrument.sets_observable, "swap", a, b)
    M.renumber_assignments(instrument, "modulation_set_index", "swap", a, b)
  end
  fields.insert_sample_device_chain_at = function(_, c)
    if c < 1 or c > #instrument.chains + 1 then out_of_range("device chain", c) end
    list_op(instrument.chains, instrument.chains_observable, "insert", c, nil, chain_data({ name = string.format("FX Chain %02d", #instrument.chains + 1) }))
    M.renumber_assignments(instrument, "device_chain_index", "insert", c)
    return chain_object(instrument, c)
  end
  fields.delete_sample_device_chain_at = function(_, c)
    if not instrument.chains[c] then out_of_range("device chain", c) end
    M.renumber_assignments(instrument, "device_chain_index", "remove", c)
    list_op(instrument.chains, instrument.chains_observable, "remove", c)
  end
  fields.swap_sample_device_chains_at = function(_, a, b)
    list_op(instrument.chains, instrument.chains_observable, "swap", a, b)
    M.renumber_assignments(instrument, "device_chain_index", "swap", a, b)
  end
  fields.clear = function()
    -- (what clear() leaves: no samples, phrases or chains, one set, no name: lab)
    M.replace_instrument(i, M.instrument_data({}))
  end
  fields.copy_from = function(_, other)
    local from = M.impl(other).__data
    M.replace_instrument(i, M.instrument_copy(from))
  end
  fields.midi_input_properties = object("renoise.InstrumentMidiInputProperties", {
    device_name = "", channel = 0, note_range = { 0, 119 }, assigned_track = 0, device_name_observable = obs(instrument, "midi_in"),
    channel_observable = obs(instrument, "midi_in_ch"), note_range_observable = obs(instrument, "midi_in_nr"), assigned_track_observable = obs(instrument, "midi_in_t"),
  })
  fields.__data = instrument
  return object("renoise.Instrument", fields, getters, setters)
end

-- A sample's set or chain is a number: Renoise renumbers them with the list (assumed as instruments are; lab pending).
function M.renumber_assignments(instrument, field, type_, a, b)
  for _, sample in ipairs(instrument.samples) do
    local v = sample[field]
    local new = v
    if type_ == "insert" then
      if v >= a then new = v + 1 end
    elseif type_ == "remove" then
      if v == a then new = 0 elseif v > a then new = v - 1 end
    elseif type_ == "swap" then
      if v == a then new = b elseif v == b then new = a end
    end
    if new ~= v then write(sample, field, new) end
  end
end

-- An instrument with everything copied (a duplicate, or what a file holds).
function M.instrument_copy(from)
  local strip
  strip = function(v, seen)
    if type(v) ~= "table" then return v end
    if seen[v] then return seen[v] end
    local out = {}
    seen[v] = out
    for k, x in pairs(v) do
      if k ~= "obs" and k ~= "notifiers" and not (type(k) == "string" and k:match("_observable$")) then out[k] = strip(x, seen) end
    end
    return out
  end
  local made = strip(from, {})
  for _, phrase in ipairs(made.phrases or {}) do phrase.notifiers = {} end
  return made
end

-- The instrument at `i` takes another's contents (a load, a clear, a copy): its lists are told as emptied and filled.
function M.replace_instrument(i, contents)
  local instrument = M.data.instruments[i]
  local before = M.instrument_copy(instrument)
  local function put(from)
    for _, key in ipairs({ "samples", "phrases", "sets", "chains", "mappings" }) do
      local items = instrument[key]
      for k = #items, 1, -1 do
        table.remove(items, k)
        M.fire(instrument[key .. "_observable"], { type = "remove", index = k })
      end
    end
    for k, v in pairs(from) do
      if k ~= "obs" and k ~= "samples" and k ~= "phrases" and k ~= "sets" and k ~= "chains" and k ~= "mappings" and not (type(k) == "string" and k:match("_observable$")) then
        instrument[k] = copy(v)
      end
    end
    for _, key in ipairs({ "samples", "phrases", "sets", "chains", "mappings" }) do
      for k, item in ipairs(from[key] or {}) do
        instrument[key][k] = item
        M.fire(instrument[key .. "_observable"], { type = "insert", index = k })
        -- (and, as Renoise copies a set or a chain, the device at 1 a set's list doesn't show, and a chain's mixer: lab)
        if (key == "sets" or key == "chains") and item.devices_observable then M.fire(item.devices_observable, { type = "insert", index = 1 }) end
      end
    end
    for _, phrase in ipairs(instrument.phrases) do phrase.notifiers = phrase.notifiers or {} end
    -- (mappings name their phrases: a copy's mappings name the copy's phrases)
    M.fire(obs(instrument, "name"))
    M.fire(obs(instrument, "plugin"))
  end
  local after = M.instrument_copy(contents)
  put(contents)
  M.record(function() put(M.instrument_copy(before)) end, function() put(M.instrument_copy(after)) end)
end

-- Files: what the mock's Renoise saves and loads (a Lua table, written as JSON-like text the mock reads back).

local function serialize(v)
  local t = type(v)
  if t == "table" then
    local parts = {}
    for k, x in pairs(v) do
      local key = type(k) == "number" and ("[" .. k .. "]") or ("[" .. string.format("%q", k) .. "]")
      parts[#parts + 1] = key .. "=" .. serialize(x)
    end
    return "{" .. table.concat(parts, ",") .. "}"
  elseif t == "string" then
    return string.format("%q", v)
  end
  return tostring(v)
end

function M.write_file(path, value)
  local f = io.open(path, "wb")
  if not f then return false end
  -- (an instrument file is a zip: it starts as one, and its central directory at its end names Instrument.xml)
  f:write((value.kind == "instrument" and "PK\3\4" or "") .. "MOCKRENOISE " .. serialize(value) .. (value.kind == "instrument" and "\n--PK\1\2 Instrument.xml" or ""))
  f:close()
  M.files_written = (M.files_written or 0) + 1
  return true
end

function M.read_file(path)
  local f = io.open(path, "rb")
  if not f then return nil end
  local text = f:read("*a")
  f:close()
  text = text:gsub("\n%-%-PK\1\2 Instrument%.xml$", "")
  local body = text:match("^MOCKRENOISE (.*)$") or text:match("^PK\3\4MOCKRENOISE (.*)$")
  if not body then return nil, text end
  local chunk = loadstring("return " .. body)
  return chunk and chunk() or nil
end

-- Audio a buffer can load: what a buffer saved, or a WAV/FLAC/AIFF file the test made (its size stands in for its length).
function M.read_audio(path)
  local value, raw = M.read_file(path)
  if value and value.kind == "audio" then
    local buffer = copy(value.buffer)
    buffer.name = value.name
    return buffer
  end
  -- (a file whose contents Renoise can't read, though its first bytes look right: `broken` in it)
  if raw and path:lower():match("%.[wfa][ali][vfa]c?f?$") and not raw:find("broken", 1, true) and not raw:find("not a wave", 1, true) then
    local name = path:match("([^/\\]+)%.[^.]+$")
    return { frames = math.max(1, #raw), rate = 44100, channels = 1, bits = 16, content = "file:" .. path, name = name }
  end
  return nil
end

-- renoise.app()'s instrument files, into or from the selected instrument. An .xrni replaces the instrument: a new one is put after
-- it and the old one taken out (insert@i+1, remove@i, no notes renumbered: lab); a multi-sample loads into the same instrument;
-- anything else load_instrument doesn't know shows "Unknown instrument type!" in a dialog (lab).
function M.app_load_instrument(path, how)
  local d = M.data
  local song = d.song_fields
  local i = song.selected_instrument_index
  local value, raw = M.read_file(path)
  if how == "multi" then
    if not (raw and path:lower():match("%.sfz$")) then M.dialogs = (M.dialogs or 0) + 1 return false end
    local contents = M.instrument_copy(d.instruments[i])
    contents.name = path:match("([^/\\]+)%.sfz$")
    contents.samples = {}
    for region in raw:gmatch("<region>([^\n]*)") do
      local file = region:match("sample=(%S+)")
      local lo, hi = tonumber(region:match("lokey=(%d+)")), tonumber(region:match("hikey=(%d+)"))
      local center = tonumber(region:match("pitch_keycenter=(%d+)"))
      contents.samples[#contents.samples + 1] = M.sample_data({ name = file and file:match("([^/]+)%.%w+$") or "Sample", frames = 100,
        note_range = { lo or 0, hi or 119 }, base_note = center or lo or 48, content = "sfz:" .. tostring(file) })
    end
    M.replace_instrument(i, contents)
    return true
  end
  if not (value and value.kind == "instrument") then
    M.dialogs = (M.dialogs or 0) + 1
    return false
  end
  local instruments = d.instruments
  local old = instruments[i]
  local made = M.instrument_copy(value.instrument)
  -- (into the last instrument, Renoise first adds an empty one after it, and leaves it: lab, 2026-10-20)
  local extra = i == #instruments and M.instrument_data({}) or nil
  local function swap_in()
    if extra then
      table.insert(instruments, i + 1, extra)
      M.fire(song.instruments_observable, { type = "insert", index = i + 1 })
    end
    table.insert(instruments, i + 1, made)
    M.fire(song.instruments_observable, { type = "insert", index = i + 1 })
    table.remove(instruments, i)
    M.fire(song.instruments_observable, { type = "remove", index = i })
  end
  local function swap_out()
    if extra then
      table.insert(instruments, i, old)
      M.fire(song.instruments_observable, { type = "insert", index = i })
      for _ = 1, 2 do
        table.remove(instruments, i + 1)
        M.fire(song.instruments_observable, { type = "remove", index = i + 1 })
      end
      return
    end
    -- (Renoise's undo tells the old one back at i, then the loaded one's remove after it: lab 2026-10-20)
    table.insert(instruments, i, old)
    M.fire(song.instruments_observable, { type = "insert", index = i })
    table.remove(instruments, i + 1)
    M.fire(song.instruments_observable, { type = "remove", index = i + 1 })
  end
  swap_in()
  M.record(swap_out, swap_in)
  return true
end

function M.app_save_instrument(path)
  local song = M.data.song_fields
  local instrument = M.data.instruments[song.selected_instrument_index]
  if not instrument then error("no instrument selected", 2) end
  return M.write_file(path, { kind = "instrument", instrument = M.instrument_copy(instrument) })
end

end
