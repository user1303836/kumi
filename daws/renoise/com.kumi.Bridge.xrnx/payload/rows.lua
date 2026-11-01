-- The song as rows (contract §3). Everything here is addressed by index and looked up again where it is used: nothing
-- keeps a Renoise object from one slice of a job to the next (the song can change between slices).
-- Rows number things as Renoise shows them: lines, patterns, instruments and samples from 0 (a Lua index less one); tracks,
-- phrases, sequence slots and places in a chain from 1, as their Lua index.

local import = ...
local json = import("json")
local refs = import("refs")
local status = import("status")

local M = {}

local function fail(code, message)
  error({ code = code, message = message }, 0)
end

local function the_song()
  local song = renoise.song()
  if not song then fail("no-song", "Renoise has no song open") end
  return song
end

local function track_count(song)
  return song.sequencer_track_count + 1 + song.send_track_count
end

local function hex(color)
  if type(color) ~= "table" or not color[1] then return json.null end
  return string.format("#%02x%02x%02x", color[1], color[2], color[3])
end

local function db(linear)
  if type(linear) == "number" and linear > 0 then return 20 * math.log10(linear) end
  return json.null
end

---@return integer?
local function integer(value, name, low, high)
  if value == nil then return nil end
  if type(value) ~= "number" or value ~= math.floor(value) or value < low or value > high then
    fail("bad-params", name .. " must be a whole number from " .. low .. " to " .. high)
  end
  return value
end

local function array(items)
  return json.array(items)
end

-- Names for Renoise's enums, read from the API's own constants.
local function track_type(song_track)
  local T = renoise.Track
  local t = song_track.type
  if t == T.TRACK_TYPE_SEQUENCER then return "sequencer" end
  if t == T.TRACK_TYPE_GROUP then return "group" end
  if t == T.TRACK_TYPE_SEND then return "send" end
  return "master"
end

local function mute_name(state)
  local T = renoise.Track
  if state == T.MUTE_STATE_OFF then return "off" end
  if state == T.MUTE_STATE_MUTED then return "muted" end
  return "active"
end

local function loop_name(mode)
  local S = renoise.Sample
  if mode == S.LOOP_MODE_FORWARD then return "forward" end
  if mode == S.LOOP_MODE_REVERSE then return "reverse" end
  if mode == S.LOOP_MODE_PING_PONG then return "pingpong" end
  return "off"
end

local function layer_name(layer)
  local I = renoise.Instrument
  if layer == I.LAYER_NOTE_OFF then return "note-off" end
  if layer == I.LAYER_NOTE_DISABLED then return "disabled" end
  return "note-on"
end

local function key_tracking_name(mode)
  local P = renoise.InstrumentPhrase
  if mode == P.KEY_TRACKING_TRANSPOSE then return "transpose" end
  if mode == P.KEY_TRACKING_OFFSET then return "offset" end
  return "none"
end

-- Group tracks hold the tracks just before them in the song's order, and Renoise can't compare two track objects, so
-- who is in which group is worked out from the order and each group's member count: parent[i] = the group's index.
local function group_parents(song)
  local parents = {}
  local T = renoise.Track
  -- Marks the group's direct children (its `members` are all of its descendants, just before it) and returns the
  -- position before its whole subtree.
  local function place(group)
    local first = group - #song:track(group).members
    local at = group - 1
    while at >= first and at >= 1 do
      parents[at] = group
      if song:track(at).type == T.TRACK_TYPE_GROUP then at = place(at) else at = at - 1 end
    end
    return first - 1
  end
  local at = song.sequencer_track_count
  while at >= 1 do
    if song:track(at).type == T.TRACK_TYPE_GROUP then at = place(at) else at = at - 1 end
  end
  return parents
end

-- Rows

-- Set by the bridge: the song's `id` and `identity` (it knows the pid and which document this is).
---@type fun(song: renoise.Song): table
M.song_extra = function(_) return {} end

local function song_row(song)
  local transport = song.transport
  local path = song.file_name
  local row = {
    name = status.cap(song.name or ""),
    path = (path and path ~= "") and path or json.null,
    bpm = transport.bpm,
    lpb = transport.lpb,
    tpl = transport.tpl,
    patternCount = #song.patterns,
    sequenceLength = #song.sequencer.pattern_sequence,
    playing = transport.playing,
    editMode = transport.edit_mode,
    followPlayer = transport.follow_player,
  }
  for k, v in pairs(M.song_extra(song)) do row[k] = v end
  M.song_settings(song, row)
  return row
end

-- M4 (n-song): the settings Kumi's song kinds change, as they take them.
local MOST_COMMENTS_SHOWN = 1000
function M.song_settings(song, row)
  local transport = song.transport
  local amounts = {}
  for i, v in ipairs(transport.groove_amounts) do amounts[i] = v end
  row.groove = { on = transport.groove_enabled, amounts = array(amounts) }
  row.metronome = {
    on = transport.metronome_enabled, beatsPerBar = transport.metronome_beats_per_bar, linesPerBeat = transport.metronome_lines_per_beat,
    precount = transport.metronome_precount_enabled and transport.metronome_precount_bars or false, volumeDb = db(transport.metronome_volume),
  }
  row.editing = {
    editStep = transport.edit_step, octave = transport.octave,
    quantize = transport.record_quantize_enabled and transport.record_quantize_lines or false,
    wrap = transport.wrapped_pattern_edit, singleTrack = transport.single_track_edit_mode,
    keyboardVelocity = transport.keyboard_velocity_enabled and transport.keyboard_velocity or false,
    recordTo = transport.record_parameter_mode == renoise.Transport.RECORD_PARAMETER_MODE_AUTOMATION and "automation" or "pattern",
  }
  row.headroomDb = db(transport.track_headroom)
  row.artist = status.cap(song.artist or "")
  local comments = table.concat(song.comments, "\n")
  if #comments > MOST_COMMENTS_SHOWN then
    -- (cut on a character's start: the text stays UTF-8)
    local cut = MOST_COMMENTS_SHOWN
    while cut > 0 and comments:byte(cut + 1) and comments:byte(cut + 1) >= 0x80 and comments:byte(cut + 1) < 0xC0 do cut = cut - 1 end
    row.comments, row.commentsCut = comments:sub(1, cut), true
  else
    row.comments = comments
  end
end

-- (m7) The producer's setup, read on each look: how long the song's patterns usually are, its groups and their colours,
-- and the tools installed (Paketti's among them). The song row has the rest (BPM, LPB, TPL, edit step, octave).
M.SETUP_SLOTS = 512
M.SETUP_GROUPS = 32
M.SETUP_TOOLS = 40
local OWN_TOOL = "com.kumi.Bridge"
function M.setup(song)
  local counts, usual, most, lengths = {}, nil, 0, 0
  local sequence = song.sequencer.pattern_sequence
  for s = 1, math.min(#sequence, M.SETUP_SLOTS) do
    local lines = song:pattern(sequence[s]).number_of_lines
    if not counts[lines] then lengths = lengths + 1 end
    counts[lines] = (counts[lines] or 0) + 1
    -- (a tie goes to the length that got there first)
    if counts[lines] > most then usual, most = lines, counts[lines] end
  end
  local groups = {}
  for i = 1, song.sequencer_track_count do
    local track = song:track(i)
    if track.type == renoise.Track.TRACK_TYPE_GROUP and #groups < M.SETUP_GROUPS then
      groups[#groups + 1] = { name = status.cap(track.name), color = hex(track.color), tracks = #track.members }
    end
  end
  -- (Renoise lists each tool as a table of its manifest's fields: name, id, enabled…)
  local tools, paketti = {}, false
  for _, entry in pairs(renoise.app().installed_tools or {}) do
    local tool = entry --[[@as table]]
    if type(tool) == "table" and tool.id ~= OWN_TOOL and tool.enabled ~= false then
      local id, name = tostring(tool.id or ""), tostring(tool.name or "")
      if (id .. " " .. name):lower():find("paketti", 1, true) then paketti = true end
      if #tools < M.SETUP_TOOLS then tools[#tools + 1] = status.cap(name ~= "" and name or id) end
    end
  end
  table.sort(tools)
  return {
    patternLines = usual or json.null,
    patternLinesVary = lengths > 1 or nil,
    groups = array(groups),
    tools = array(tools),
    paketti = paketti,
  }
end

-- The note columns of a track that are muted (their numbers) and the ones that are named ({"2": "Pad"}); nil for none.
local function column_state(track)
  local muted, names
  for c = 1, track.max_note_columns do
    if track:column_is_muted(c) then
      muted = muted or {}
      muted[#muted + 1] = c
    end
    local name = track:column_name(c)
    if name ~= "" then
      names = names or {}
      names[tostring(c)] = name
    end
  end
  return muted and array(muted) or nil, names
end

local function track_row(song, epoch, i, parents)
  local track = song:track(i)
  local is_master = track.type == renoise.Track.TRACK_TYPE_MASTER
  local row = {
    ref = refs.make(epoch, "track", i),
    index = i,
    name = track.name,
    type = track_type(track),
    color = hex(track.color),
    colorBlend = track.color_blend,
    -- (volumeDb and pan are the pre-FX fader and pan, as they were; the post-FX ones are the fader a producer rides)
    volumeDb = db(track.prefx_volume.value),
    pan = (track.prefx_panning.value - 0.5) * 2,
    preVolumeDb = db(track.prefx_volume.value),
    prePan = (track.prefx_panning.value - 0.5) * 2,
    preWidth = track.prefx_width.value,
    postVolumeDb = db(track.postfx_volume.value),
    postPan = (track.postfx_panning.value - 0.5) * 2,
    deviceCount = #track.devices,
    noteColumns = track.visible_note_columns,
    effectColumns = track.visible_effect_columns,
    groupRef = parents and parents[i] and refs.make(epoch, "track", parents[i]) or json.null,
  }
  if not is_master then
    row.mute = mute_name(track.mute_state)
    row.solo = track.solo_state
  end
  row.mutedColumns, row.columnNames = column_state(track)
  M.track_settings(track, row)
  return row
end

-- M4 (n-song): what Kumi's track kinds change, as they take it: the collapse, the width in percent, the output routing and
-- delay, and which sub-columns show.
function M.track_settings(track, row)
  local T = renoise.Track
  row.collapsed = track.collapsed
  if track.type == T.TRACK_TYPE_GROUP then row.groupCollapsed = track.group_collapsed end
  row.width = track.prefx_width.value / track.prefx_width.value_max * 100
  row.outputRouting = track.output_routing
  if track.type == T.TRACK_TYPE_SEQUENCER then row.outputDelay = track.output_delay end
  if track.type == T.TRACK_TYPE_SEQUENCER and track.max_note_columns > 0 then
    row.subColumns = {
      volume = track.volume_column_visible, panning = track.panning_column_visible, delay = track.delay_column_visible,
      sampleEffects = track.sample_effects_column_visible,
    }
  end
end

local function sequence_row(song, epoch, s, tracks)
  local sequencer = song.sequencer
  local muted = {}
  for t = 1, tracks do
    if sequencer:track_sequence_slot_is_muted(t, s) then muted[#muted + 1] = t end
  end
  return {
    ref = refs.make(epoch, "seq", s),
    patternRef = refs.make(epoch, "pattern", sequencer:pattern(s)),
    pattern = sequencer:pattern(s) - 1,
    section = sequencer:sequence_is_start_of_section(s) and sequencer:sequence_section_name(s) or json.null,
    mutedTracks = array(muted),
  }
end

local function pattern_row(song, epoch, p, tracks)
  local pattern = song:pattern(p)
  local aliases, colors = {}, {}
  for t = 1, tracks do
    local pattern_track = pattern:track(t)
    local alias = pattern_track.alias_pattern_index
    if alias > 0 then aliases[tostring(t)] = alias - 1 end
    -- M4 (n-song): the slot colours of the pattern's tracks in the matrix
    local color = pattern_track.color
    if color then colors[tostring(t)] = hex(color) end
  end
  local row = {
    ref = refs.make(epoch, "pattern", p),
    number = p - 1,
    name = pattern.name,
    lines = pattern.number_of_lines,
    aliases = next(aliases) and aliases or json.object(),
  }
  if next(colors) then row.colors = colors end
  return row
end

local function macros_of(instrument)
  local macros = {}
  for k = 1, #instrument.macros do
    local macro = instrument:macro(k)
    macros[k] = { name = macro.name, value = macro.value }
    -- (n-instruments: how many parameters each is mapped to; the mappings themselves are the instrument's XML)
    local mapped = #macro.mappings
    if mapped > 0 then macros[k].mappings = mapped end
  end
  return array(macros)
end

local function midi_type_name(kind)
  local P = renoise.InstrumentMidiOutputProperties
  if kind == P.TYPE_LINE_IN_RET then return "line-in" end
  if kind == P.TYPE_INTERNAL then return "internal" end
  return "external"
end

-- n-instruments: Renoise's numbered settings by the names Kumi's changes take.
local OVERLAP_NAMES = { [0] = "all", [1] = "cycle", [2] = "random" }
local PLAYBACK_NAMES = { "off", "program", "keymap" }
local QUANTIZE_NAMES = { "none", "line", "beat", "bar" }
local KEY_NAMES = { "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B" }
local NNA_NAMES = { "cut", "off", "continue" }
local SYNC_NAMES = { "repitch", "percussion", "texture" }
local INTERPOLATION_NAMES = { "none", "linear", "cubic", "sinc" }

-- What the instrument plays through: its samples, a plug-in, or a MIDI port (its settings under `midi`); and its own settings.
local function instrument_row(song, epoch, i)
  local instrument = song:instrument(i)
  local plugin = instrument.plugin_properties
  local out = instrument.midi_output_properties
  local kind = "sample"
  if plugin.plugin_loaded then
    kind = "plugin"
  elseif instrument.active_tab == renoise.Instrument.TAB_EXT_MIDI then
    kind = "midi"
  end
  local trigger = instrument.trigger_options
  local row = {
    ref = refs.make(epoch, "instrument", i),
    index = i - 1,
    name = instrument.name,
    kind = kind,
    sampleCount = #instrument.samples,
    phraseCount = #instrument.phrases,
    plugin = plugin.plugin_loaded and plugin.plugin_device.name or json.null,
    macros = macros_of(instrument),
    -- n-instruments
    volumeDb = db(instrument.volume),
    transpose = instrument.transpose,
    overlap = OVERLAP_NAMES[instrument.sample_mapping_overlap_mode] or instrument.sample_mapping_overlap_mode,
    phrasePlayback = PLAYBACK_NAMES[instrument.phrase_playback_mode] or instrument.phrase_playback_mode,
    modsetCount = #instrument.sample_modulation_sets,
    chainCount = #instrument.sample_device_chains,
    quantize = QUANTIZE_NAMES[trigger.quantize] or trigger.quantize,
    monophonic = trigger.monophonic,
  }
  local program = instrument.phrase_program
  if program >= 1 and program <= #instrument.phrases then row.phraseProgramRef = refs.make(epoch, "phrase", i, program) end
  if trigger.monophonic then row.glide = trigger.monophonic_glide end
  if trigger.scale_mode ~= "None" then row.scale, row.scaleKey = trigger.scale_mode, KEY_NAMES[trigger.scale_key] end
  if plugin.plugin_loaded then
    local device = plugin.plugin_device
    row.pluginPath = device.device_path
    row.pluginParamCount = #device.parameters
    local preset = device.active_preset
    if preset and preset > 0 and preset <= #device.presets then row.pluginPreset = device:preset(preset) end
    row.pluginChannel, row.pluginTranspose = plugin.channel, plugin.transpose
  end
  if kind == "midi" then
    row.midi = {
      device = out.device_name, type = midi_type_name(out.instrument_type), channel = out.channel, program = out.program,
      bank = out.bank, transpose = out.transpose, delay = out.delay, duration = out.duration,
    }
  end
  return row
end

local function range(pair)
  return array({ pair[1], pair[2] })
end

local function sample_row(song, epoch, i, s)
  local instrument = song:instrument(i)
  local sample = instrument:sample(s)
  local mapping = sample.sample_mapping
  local row = {
    ref = refs.make(epoch, "sample", i, s),
    index = s - 1,
    name = sample.name,
    baseNote = mapping.base_note,
    noteRange = range(mapping.note_range),
    velocityRange = range(mapping.velocity_range),
    layer = layer_name(mapping.layer),
    mapKeyToPitch = mapping.map_key_to_pitch,
    mapVelocityToVolume = mapping.map_velocity_to_volume,
    loop = loop_name(sample.loop_mode),
    muteGroup = sample.mute_group,
    -- n-instruments: every setting sample-set takes, by its names
    volumeDb = db(sample.volume),
    panning = sample.panning * 2 - 1,
    transpose = sample.transpose,
    fineTune = sample.fine_tune,
    nna = NNA_NAMES[sample.new_note_action] or sample.new_note_action,
    interpolation = INTERPOLATION_NAMES[sample.interpolation_mode] or sample.interpolation_mode,
  }
  if sample.loop_mode ~= renoise.Sample.LOOP_MODE_OFF then
    row.loopStart, row.loopEnd, row.loopRelease = sample.loop_start, sample.loop_end, sample.loop_release
  end
  if sample.beat_sync_enabled then row.beatSync, row.beatSyncLines, row.beatSyncMode = true, sample.beat_sync_lines, SYNC_NAMES[sample.beat_sync_mode] end
  if sample.oneshot then row.oneshot = true end
  if sample.autoseek then row.autoseek = true end
  if sample.autofade then row.autofade = true end
  if sample.oversample_enabled then row.oversample = true end
  if mapping.read_only then row.keyzoneReadOnly = true end
  if sample.is_slice_alias then row.slice = true end
  local markers = sample.slice_markers
  if #markers > 0 then row.sliceMarkers = array(markers) end
  local set, chain = sample.modulation_set_index, sample.device_chain_index
  row.modulationSetRef = set > 0 and refs.make(epoch, "modset", i, set) or json.null
  row.fxChainRef = chain > 0 and refs.make(epoch, "chain", i, chain) or json.null
  local buffer = sample.sample_buffer
  if buffer.has_sample_data then
    row.frames, row.rate, row.channels, row.bits = buffer.number_of_frames, buffer.sample_rate, buffer.number_of_channels, buffer.bit_depth
  else
    row.frames = 0
  end
  return row
end

local PLAY_NAMES = { "pattern", "script" }

local function phrase_row(song, epoch, i, p)
  local phrase = song:instrument(i):phrase(p)
  local row = {
    ref = refs.make(epoch, "phrase", i, p),
    index = p,
    name = phrase.name,
    lines = phrase.number_of_lines,
    lpb = phrase.lpb,
    keyTracking = key_tracking_name(phrase.key_tracking),
    looping = phrase.looping,
    -- n-instruments
    playback = PLAY_NAMES[phrase.playback_mode] or phrase.playback_mode,
    baseNote = phrase.base_note,
    loopStart = phrase.loop_start - 1,
    loopEnd = phrase.loop_end - 1,
    shuffle = phrase.shuffle,
    autoseek = phrase.autoseek,
    noteColumns = phrase.visible_note_columns,
    effectColumns = phrase.visible_effect_columns,
  }
  local mapping = phrase.mapping
  if mapping then
    row.keyRange = range(mapping.note_range)
    row.keyTrackingInKeymap, row.baseNoteInKeymap = key_tracking_name(mapping.key_tracking), mapping.base_note
  else
    row.keyRange = json.null
  end
  if phrase.playback_mode == renoise.InstrumentPhrase.PLAY_SCRIPT then
    local script = phrase.script
    row.scriptLines = #script.paragraphs
    if script.compile_error ~= "" then row.compileError = script.compile_error end
    if script.runtime_error ~= "" then row.runtimeError = script.runtime_error end
    if script.pending_changes > 0 then row.uncommitted = true end
  end
  return row
end

-- n-instruments: an instrument's modulation sets, with their devices (and the devices' parameters, which are few), and its sample
-- FX chains with their devices (their parameters are read as `param` rows of the device).
local TARGET_NAMES = { "volume", "panning", "pitch", "cutoff", "resonance", "drive" }
local OPERATOR_NAMES = { "add", "subtract", "multiply", "divide" }
local INPUT_FIELDS = { volume = "volume_input", panning = "panning_input", pitch = "pitch_input", cutoff = "cutoff_input",
  resonance = "resonance_input", drive = "drive_input" }
local TARGET_WORDS = { "Volume ", "Panning ", "Pitch ", "Cutoff ", "Resonance ", "Drive " }

local function moddevice_row(epoch, i, m, d, device)
  local params = {}
  for k = 1, #device.parameters do
    local parameter = device:parameter(k)
    params[k] = { index = k, name = parameter.name, value = parameter.value, valueString = parameter.value_string, min = parameter.value_min, max = parameter.value_max }
  end
  local kind = device.name
  for _, word in ipairs(TARGET_WORDS) do
    if kind:sub(1, #word) == word then kind = kind:sub(#word + 1) end
  end
  local row = {
    ref = refs.make(epoch, "moddevice", i, m, d), index = d, name = device.display_name, type = kind,
    target = TARGET_NAMES[device.target] or device.target, operator = OPERATOR_NAMES[device.operator] or device.operator,
    bipolar = device.bipolar, active = device.is_active, params = array(params),
  }
  if device.tempo_sync_switching_allowed then row.tempoSynced = device.tempo_synced end
  local ok, points = pcall(function() return device.points end)
  if ok and type(points) == "table" then
    row.points = #points
    row.length = device.length
  end
  return row
end

local function modset_row(song, epoch, i, m)
  local set = song:instrument(i):sample_modulation_set(m)
  local inputs = {}
  for key, field in pairs(INPUT_FIELDS) do inputs[key] = set[field].value end
  local devices = {}
  for d = 1, #set.devices do devices[d] = moddevice_row(epoch, i, m, d, set:device(d)) end
  return {
    ref = refs.make(epoch, "modset", i, m), index = m, name = set.name, pitchRange = set.pitch_range, filterType = set.filter_type,
    filterVersion = set.filter_version, inputs = inputs, devices = array(devices),
  }
end

local function chain_row(song, epoch, i, c)
  local chain = song:instrument(i):sample_device_chain(c)
  local devices = {}
  for d = 1, #chain.devices do
    local device = chain:device(d)
    local preset = device.active_preset
    devices[d] = {
      ref = refs.make(epoch, "chaindevice", i, c, d), index = d, name = device.display_name ~= "" and device.display_name or device.name,
      path = device.device_path, active = device.is_active, paramCount = #device.parameters,
      preset = (preset and preset > 0 and preset <= #device.presets) and device:preset(preset) or json.null,
    }
  end
  return { ref = refs.make(epoch, "chain", i, c), index = c, name = chain.name, outputRouting = chain.output_routing, devices = array(devices) }
end

local function parameter_row(reference, k, parameter)
  return {
    ref = reference, index = k, name = parameter.name, value = parameter.value, valueString = parameter.value_string,
    min = parameter.value_min, max = parameter.value_max,
  }
end

local function device_row(song, epoch, t, d)
  local device = song:track(t):device(d)
  local path = device.device_path or ""
  local preset = device.active_preset
  return {
    ref = refs.make(epoch, "device", t, d),
    index = d,
    name = device.name,
    displayName = device.display_name,
    path = path,
    active = device.is_active,
    preset = (preset and preset > 0) and device:preset(preset) or json.null,
    paramCount = #device.parameters,
    plugin = path:find("^Audio/Effects/Native/") == nil,
    -- (the native ones only: a plug-in of that name is not a container; the paths are Renoise's, measured in 3.5.4)
    isDoofer = path == "Audio/Effects/Native/Doofer",
    isSplitter = path == "Audio/Effects/Native/Splitter",
  }
end

local function param_row(song, epoch, t, d, k)
  local parameter = song:track(t):device(d):parameter(k)
  return {
    ref = refs.make(epoch, "param", t, d, k),
    index = k,
    name = parameter.name,
    value = parameter.value,
    valueString = parameter.value_string,
    min = parameter.value_min,
    max = parameter.value_max,
  }
end

local MAX_NOTE_COLUMNS, MAX_EFFECT_COLUMNS = 12, 8

-- The columns with something in them, in Renoise's own text: note columns first..last, then effect columns.
local function filled_columns(line, first_note, last_note, first_effect, last_effect)
  local notes, effects = {}, {}
  for c = first_note, last_note do
    local column = line:note_column(c)
    if not column.is_empty then
      local note = { col = c, note = column.note_string }
      local text = column.instrument_string
      if text ~= ".." then note.instrument = text end
      text = column.volume_string
      if text ~= ".." then note.volume = text end
      text = column.panning_string
      if text ~= ".." then note.panning = text end
      text = column.delay_string
      if text ~= ".." then note.delay = text end
      if column.effect_number_value ~= 0 or column.effect_amount_value ~= 0 then
        note.fx = column.effect_number_string .. column.effect_amount_string
      end
      notes[#notes + 1] = note
    end
  end
  for c = first_effect, last_effect do
    local column = line:effect_column(c)
    if not column.is_empty then effects[#effects + 1] = { col = c, number = column.number_string, amount = column.amount_string } end
  end
  return notes, effects
end

-- A pattern line (or a phrase's) as the tracker shows it, in Renoise's own text; nil when it is empty. Only columns
-- with something in them are listed, each with its number, and a field the tracker shows as ".." is left out.
-- Data in a column the track doesn't show is under `hidden` ({notes?, effects?}). A line with nothing in the shown columns
-- is looked through for it, since the line isn't empty; one that has something is only when `scan_hidden` (reading every
-- column of every line costs about six times as much).
local function line_row(line, number, note_columns, effect_columns, scan_hidden)
  if line.is_empty then return nil end
  local notes, effects = filled_columns(line, 1, note_columns, 1, effect_columns)
  local row = { line = number }
  if #notes > 0 then row.notes = array(notes) end
  if #effects > 0 then row.effects = array(effects) end
  if scan_hidden or (#notes == 0 and #effects == 0) then
    local hidden_notes, hidden_effects =
      filled_columns(line, note_columns + 1, MAX_NOTE_COLUMNS, effect_columns + 1, MAX_EFFECT_COLUMNS)
    if #hidden_notes > 0 or #hidden_effects > 0 then
      row.hidden = {}
      if #hidden_notes > 0 then row.hidden.notes = array(hidden_notes) end
      if #hidden_effects > 0 then row.hidden.effects = array(hidden_effects) end
    end
  end
  if row.notes == nil and row.effects == nil and row.hidden == nil then return nil end
  return row
end

-- Every solo taken away (Renoise turns every track Active); false when one stays. The setter toggles a track's own solo, so `false`
-- on a track soloed only along with another (a group's member) solos it on its own first (measured, 3.5.4): hence the rounds, of
-- which a solo takes two at most.
M.SOLO_ROUNDS = 64
function M.unsolo_all(song)
  for _ = 1, M.SOLO_ROUNDS do
    local first
    for t = 1, M.track_count(song) do
      local track = song:track(t)
      if track.type ~= renoise.Track.TRACK_TYPE_MASTER and track.solo_state then
        first = track
        break
      end
    end
    if not first then return true end
    first.solo_state = false
  end
  return false
end

-- Each of a pattern track's automations with the device and parameter it automates, in the track's order:
-- fn(d, k, device, parameter, automation). A track's parameters run to hundreds and its automations are few, so none reads no
-- parameter, and the walk ends at the last. (An alias is walked whole: whether its list counts what `find_automation` finds in
-- the track it shows hasn't been measured.)
function M.each_automation(track, pattern_track, fn)
  local left = pattern_track.is_alias and math.huge or #pattern_track.automation
  for d = 1, #track.devices do
    if left == 0 then return end
    local device = track:device(d)
    for k = 1, #device.parameters do
      local parameter = device:parameter(k)
      if parameter.is_automated then
        local automation = pattern_track:find_automation(parameter)
        if automation then
          fn(d, k, device, parameter, automation)
          left = left - 1
          if left == 0 then return end
        end
      end
    end
  end
end

-- M4 (n-song): the automation of a pattern's track, as the automation-write kind takes it: per automated parameter its ref,
-- names, play mode and points (time in lines from 0, value 0 to 1 of the parameter's range).
local PLAYMODE_NAMES = { "points", "linear", "curves" }
function M.envelopes(song, epoch, p, t)
  local track, pattern_track, out = song:track(t), song:pattern(p):track(t), {}
  M.each_automation(track, pattern_track, function(d, k, device, parameter, automation)
    local points = {}
    for i, point in ipairs(automation.points) do points[i] = { time = point.time - 1, value = point.value, scaling = point.scaling } end
    table.sort(points, function(a, b) return a.time < b.time end)
    out[#out + 1] = {
      ref = refs.make(epoch, "param", t, d, k), device = device.display_name ~= "" and device.display_name or device.name,
      parameter = parameter.name, playmode = PLAYMODE_NAMES[automation.playmode] or "linear", points = array(points),
      valueMin = parameter.value_min, valueMax = parameter.value_max,
    }
  end)
  return out
end

M.fail = fail
M.the_song = the_song
M.track_count = track_count
M.hex = hex
M.integer = integer
M.array = array
M.track_type = track_type
M.db, M.mute_name = db, mute_name
M.key_tracking_name = key_tracking_name
M.group_parents = group_parents
M.song_row = song_row
M.track_row = track_row
M.sequence_row = sequence_row
M.pattern_row = pattern_row
M.instrument_row = instrument_row
M.sample_row = sample_row
M.phrase_row = phrase_row
M.device_row = device_row
M.modset_row, M.chain_row, M.parameter_row = modset_row, chain_row, parameter_row -- (n-instruments)
M.param_row = param_row
M.line_row = line_row
M.column_state = column_state

return M
