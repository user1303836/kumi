-- What FOCUS shows (contract §4 and §5): focus, snapshot and the bodies. Like the rows, everything is addressed by index
-- and looked up again where it is used, so a job can stop between slices and start from nothing.

local import = ...
local json = import("json")
local refs = import("refs")
local reads = import("reads")
local rows = import("rows")

local fail, the_song, track_count, hex, integer, array = rows.fail, rows.the_song, rows.track_count, rows.hex,
  rows.integer, rows.array
local track_type, key_tracking_name, group_parents = rows.track_type, rows.key_tracking_name, rows.group_parents
local song_row, track_row, device_row, instrument_row, sample_row, phrase_row, line_row = rows.song_row, rows.track_row,
  rows.device_row, rows.instrument_row, rows.sample_row, rows.phrase_row, rows.line_row

local M = {}

-- What a body is cut to, in lines, when nothing says otherwise (and at most).
M.BODY_LINES = 33
M.MAX_BODY_LINES = 128
-- What the lines of a body may come to (a page, less the rest of the answer): reading stops where it is filled.
M.BODY_BYTES = reads.PAGE_BUDGET - 8192
-- Samples and phrases an instrument body lists at most.
M.MAX_BODY_ROWS = 255
-- Tracks a snapshot lists at most (`read` has the rest).
M.SNAPSHOT_TRACKS = 200
-- Tracks a mixer body shows by default and at most.
M.MIXER_STRIPS = 24
M.MAX_MIXER_STRIPS = 64

-- Selection, transport, view

local function selection_of(song, epoch)
  local track = song:track(song.selected_track_index)
  local selection = {
    trackRef = refs.make(epoch, "track", song.selected_track_index),
    sequenceRef = refs.make(epoch, "seq", song.selected_sequence_index),
    patternRef = refs.make(epoch, "pattern", song.selected_pattern_index),
    instrumentRef = refs.make(epoch, "instrument", song.selected_instrument_index),
    -- What FOCUS's header shows
    track = { name = track.name, color = hex(track.color), type = track_type(track) },
    patternName = song:pattern(song.selected_pattern_index).name,
    instrumentName = song:instrument(song.selected_instrument_index).name,
    sequence = song.selected_sequence_index,
    -- (lines by Renoise's numbers, from 0)
    line = song.selected_line_index - 1,
  }
  local sample, phrase, device = song.selected_sample_index, song.selected_phrase_index, song.selected_track_device_index
  if sample > 0 then selection.sampleRef = refs.make(epoch, "sample", song.selected_instrument_index, sample) end
  if phrase > 0 then selection.phraseRef = refs.make(epoch, "phrase", song.selected_instrument_index, phrase) end
  if device > 0 then
    selection.deviceRef = refs.make(epoch, "device", song.selected_track_index, device)
    selection.deviceName = track:device(device).display_name
  end
  local note_column, effect_column = song.selected_note_column_index, song.selected_effect_column_index
  selection.cursor = {
    sequence = song.selected_sequence_index,
    line = song.selected_line_index - 1,
    track = song.selected_track_index,
    column = note_column > 0 and note_column or effect_column,
    columnKind = note_column > 0 and "note" or "effect",
  }
  local block = song.selection_in_pattern
  if block then
    selection.range = {
      startLine = block.start_line - 1,
      endLine = block.end_line - 1,
      startTrack = block.start_track,
      endTrack = block.end_track,
    }
  end
  return selection
end

-- (M5 m5-renoise: set by the bridge; Renoise reads as playing while it renders, which Kumi's own render isn't)
M.rendering = function() return false end

-- The play position while it plays, else the edit cursor.
local function transport_of(song)
  local transport = song.transport
  local playing = transport.playing and not M.rendering()
  local sequence, line, beat
  if playing then
    local at = transport.playback_pos
    sequence, line, beat = at.sequence, at.line, transport.playback_pos_beats
  else
    -- (the beat is Renoise's own: it follows the song's LPB changes (ZLxx), and costs nothing however long the sequence is)
    sequence, line, beat = song.selected_sequence_index, song.selected_line_index, transport.edit_pos_beats
  end
  local per_bar = transport.metronome_beats_per_bar
  return {
    playing = playing,
    recording = transport.edit_mode,
    editMode = transport.edit_mode,
    followPlayer = transport.follow_player,
    beat = beat,
    tempo = transport.bpm,
    bpm = transport.bpm,
    lpb = transport.lpb,
    tpl = transport.tpl,
    beatsPerBar = per_bar > 0 and per_bar or 4,
    loop = {
      on = transport.loop_pattern or transport.loop_block_enabled or transport.loop_sequence_start > 0,
      start = transport.loop_start_beats,
      ["end"] = transport.loop_end_beats,
      pattern = transport.loop_pattern,
      block = transport.loop_block_enabled,
    },
    sequence = sequence,
    line = line - 1,
  }
end

-- Which pane the producer is looking at: the middle frame. The pattern matrix and the DSP chain are panes beside it, not
-- views of their own, so they are flags.
local function view_of(window)
  local W = renoise.ApplicationWindow
  local frame = window.active_middle_frame
  if frame == W.MIDDLE_FRAME_PATTERN_EDITOR then return "pattern" end
  if frame == W.MIDDLE_FRAME_MIXER then return "mixer" end
  if frame == W.MIDDLE_FRAME_INSTRUMENT_PHRASE_EDITOR then return "phrase" end
  if frame == W.MIDDLE_FRAME_INSTRUMENT_SAMPLE_EDITOR then return "sample" end
  return "instrument"
end

function M.focus(epoch, changed)
  local song = the_song()
  local window = renoise.app().window
  return {
    epoch = epoch,
    view = view_of(window),
    matrixVisible = window.pattern_matrix_is_visible,
    dspVisible = window.lower_frame_is_visible and window.active_lower_frame == renoise.ApplicationWindow.LOWER_FRAME_TRACK_DSPS,
    selection = selection_of(song, epoch),
    transport = transport_of(song),
    changed = changed,
  }
end

-- snapshot {}: what a read of everything small gives at once; big sections (patterns, lines) are read in pages.
function M.snapshot(job, epoch, changed)
  local song = the_song()
  local count = track_count(song)
  local parents = group_parents(song)
  local tracks = {}
  for i = 1, math.min(count, M.SNAPSHOT_TRACKS) do
    job.slice()
    tracks[i] = track_row(the_song(), epoch, i, parents)
  end
  local result = {
    epoch = epoch,
    project = song_row(song),
    transport = transport_of(song),
    tracks = array(tracks),
    selection = selection_of(song, epoch),
    setup = rows.setup(song),
    changed = changed,
  }
  if count > M.SNAPSHOT_TRACKS then result.moreTracks = true end
  -- (a snapshot says it has more tracks than it lists with `moreTracks`, whatever the reason)
  reads.bound(result, { "tracks" })
  if result.truncated then
    result.truncated = nil
    result.moreTracks = true
  end
  return result
end

-- Bodies

local function window_of(params, default_center, total)
  local window = params.window
  local from, to
  if window ~= nil then
    if type(window) ~= "table" then fail("bad-params", "window must be {from, to}") end
    from = integer(window.from, "window.from", 1, 100000)
    to = integer(window.to, "window.to", 1, 100000)
    if not from or not to then fail("bad-params", "window needs both from and to") end
  end
  if not from then
    from = math.max(1, default_center - 16)
    to = from + M.BODY_LINES - 1
  end
  if to < from then fail("bad-params", "window.to is before window.from") end
  from = math.min(from, total)
  to = math.min(to, total, from + M.MAX_BODY_LINES - 1)
  return from, to
end

-- The pattern behind a body: the ref, or the one at the playhead when following, else the selected one.
local function pattern_for_body(song, params, epoch)
  if params.ref ~= nil then return refs.expect(params.ref, "pattern", 1, epoch)[1] end
  local transport = song.transport
  if transport.follow_player and transport.playing then
    return song.sequencer:pattern(transport.playback_pos.sequence)
  end
  return song.selected_pattern_index
end

-- The tracks of a grid, cut from the end of the window down to what a page holds. `entries` hold every row read, each
-- with its line number (and, where the body encoded the rows as it measured them, each row's text under `texts`, sent as
-- it is); `result` is the answer so far (without `tracks` and `window`).
local function fit(result, entries, from, to, cut)
  while true do
    local shown = {}
    for t, entry in ipairs(entries) do
      local kept = {}
      for k, row in ipairs(entry.rows) do
        if row.line <= to then
          local text = entry.texts and entry.texts[k]
          kept[#kept + 1] = text and json.raw(text) or row
        end
      end
      shown[t] = {
        ref = entry.ref, name = entry.name, color = entry.color, noteColumns = entry.noteColumns,
        effectColumns = entry.effectColumns, lines = array(kept), aliasOf = entry.aliasOf, aliasRef = entry.aliasRef,
        mutedColumns = entry.mutedColumns, automation = entry.automation,
      }
    end
    result.window = { from = from, to = to }
    result.tracks = array(shown)
    result.truncated = cut or nil
    if to <= from or #json.encode(result) <= reads.PAGE_BUDGET then return result end
    to = math.max(from, from + math.floor((to - from + 1) * 0.6) - 1)
    cut = true
  end
end

-- M4 (n-song): a pattern track's automation lanes in the window, cheaply (the track's list of envelopes, no search through its
-- devices): each its device's and parameter's names, play mode and the points from `from` to `to` (at most LANE_POINTS).
M.LANES = 8
M.LANE_POINTS = 32
local PLAYMODE_NAMES = { "points", "linear", "curves" }
function M.lanes(pattern_track, from, to)
  local out = {}
  for _, automation in ipairs(pattern_track.automation) do
    if #out >= M.LANES then break end
    local device, parameter = automation.dest_device, automation.dest_parameter
    if device and parameter then
      local points = {}
      for _, point in ipairs(automation.points) do
        if point.time >= from and point.time < to + 1 and #points < M.LANE_POINTS then points[#points + 1] = { point.time, point.value } end
      end
      out[#out + 1] = {
        name = (device.display_name ~= "" and device.display_name or device.name) .. " " .. parameter.name,
        playmode = PLAYMODE_NAMES[automation.playmode] or "linear", points = array(points),
      }
    end
  end
  if #out == 0 then return nil end
  return array(out)
end

local function pattern_body(job, params, epoch)
  local song = the_song()
  local pattern_at = pattern_for_body(song, params, epoch)
  if pattern_at > #song.patterns then fail("bad-params", "no pattern " .. pattern_at) end
  local lines = song:pattern(pattern_at).number_of_lines
  local transport = song.transport
  local follows = transport.follow_player and transport.playing
  local center = follows and transport.playback_pos.line or song.selected_line_index
  local from, to = window_of(params, center, lines)
  local tracks = track_count(song)
  -- Lines are read across all the tracks, one at a time, and reading stops where what was read fills a page: the window is
  -- cut there (`truncated`), with nothing read that would be thrown away. A line is a slice, so a wide song doesn't hold
  -- Renoise's GUI for the whole body.
  local entries, shape = {}, {}
  local pattern = song:pattern(pattern_at)
  local bytes = 1024
  for t = 1, tracks do
    local track = song:track(t)
    local pattern_track = pattern:track(t)
    local alias = pattern_track.alias_pattern_index
    shape[t] = { track.visible_note_columns, track.visible_effect_columns }
    entries[t] = {
      ref = refs.make(epoch, "track", t), name = track.name, color = hex(track.color),
      noteColumns = shape[t][1], effectColumns = shape[t][2], rows = {}, texts = {},
      aliasOf = alias > 0 and alias or nil, aliasRef = alias > 0 and refs.make(epoch, "pattern", alias) or nil,
    }
    entries[t].mutedColumns = rows.column_state(track)
    entries[t].automation = M.lanes(pattern_track, from, to)
    bytes = bytes + 200 + #entries[t].name + (entries[t].automation and #json.encode(entries[t].automation) or 0)
  end
  local hidden = params.hidden == true
  local cut = false
  -- (the pattern's tracks looked up once a slice: a line is the one call to Renoise a row makes, until the job waits)
  local live_tracks
  for i = from, to do
    if job.slice() or not live_tracks then
      local live_pattern = the_song():pattern(pattern_at)
      live_tracks = {}
      for t = 1, tracks do live_tracks[t] = live_pattern:track(t) end
    end
    for t = 1, tracks do
      local row = line_row(live_tracks[t]:line(i), i, shape[t][1], shape[t][2], hidden)
      if row then
        -- (each row encoded once, here where it is measured: `fit` sends the texts)
        local entry, text = entries[t], json.encode(row, { max_depth = json.ROW_DEPTH })
        entry.rows[#entry.rows + 1] = row
        entry.texts[#entry.rows] = text or false
        bytes = bytes + #(text or json.encode(row) or "") + 1
      end
    end
    if bytes > M.BODY_BYTES and i < to then
      to, cut = math.max(from, i - 1), true
      break
    end
  end
  local live = the_song()
  local at = live.transport.playback_pos
  local result = {
    epoch = epoch,
    ref = refs.make(epoch, "pattern", pattern_at),
    name = live:pattern(pattern_at).name,
    lines = lines,
    from = from,
    lpb = live.transport.lpb,
  }
  -- The edit cursor, when this is the pattern it is in; the play position, when it is playing in it.
  if live.selected_pattern_index == pattern_at then result.cursor = live.selected_line_index end
  if live.transport.playing and live.sequencer:pattern(at.sequence) == pattern_at then result.playing = at.line end
  return fit(result, entries, from, to, cut)
end

local function matrix_body(job, params, epoch)
  local song = the_song()
  local tracks = track_count(song)
  local names, track_refs = {}, {}
  for t = 1, tracks do
    names[t] = song:track(t).name
    track_refs[t] = refs.make(epoch, "track", t)
  end
  local slots = #song.sequencer.pattern_sequence
  local list = {}
  for s = 1, slots do
    job.slice()
    local live = the_song()
    local sequencer = live.sequencer
    local pattern_at = sequencer:pattern(s)
    local pattern = live:pattern(pattern_at)
    local aliases, muted, empty, colors = {}, {}, {}, {}
    for t = 1, tracks do
      local pattern_track = pattern:track(t)
      local alias = pattern_track.alias_pattern_index
      if alias > 0 then aliases[tostring(t)] = alias end
      if sequencer:track_sequence_slot_is_muted(t, s) then muted[#muted + 1] = t end
      -- (what the matrix draws as an empty block)
      if pattern_track.is_empty then empty[#empty + 1] = t end
      -- M4 (n-song): the slot's colours (its pattern's tracks')
      local color = pattern_track.color
      if color then colors[tostring(t)] = hex(color) end
    end
    list[s] = {
      ref = refs.make(epoch, "seq", s),
      pattern = pattern_at,
      name = pattern.name,
      section = sequencer:sequence_is_start_of_section(s) and sequencer:sequence_section_name(s) or json.null,
      aliases = next(aliases) and aliases or json.object(),
      mutedTracks = array(muted),
      emptyTracks = array(empty),
    }
    if next(colors) then list[s].colors = colors end
  end
  local live = the_song()
  local transport = live.transport
  local result = {
    epoch = epoch,
    tracks = array(names),
    trackRefs = array(track_refs),
    slots = array(list),
    current = transport.playing and transport.playback_pos.sequence or live.selected_sequence_index,
  }
  -- Cut to the budget, keeping the slots around the current one (`from` is the first shown); the rest is read as sequence and
  -- pattern rows.
  local from, to = 1, slots
  while #json.encode(result) > reads.PAGE_BUDGET and to > from do
    local size = math.max(1, math.floor((to - from + 1) * 0.7))
    from = math.max(1, math.min(result.current - math.floor(size / 2), slots - size + 1))
    to = from + size - 1
    local shown = {}
    for s = from, to do shown[#shown + 1] = list[s] end
    result.slots = array(shown)
    result.truncated = true
    result.slotCount = slots
    result.from = from
  end
  return result
end

local function phrase_body(job, params, epoch)
  local at = refs.expect(params.ref, "phrase", 2, epoch)
  local song = the_song()
  local instrument_at, phrase_at = at[1], at[2]
  if instrument_at > #song.instruments or phrase_at > #song:instrument(instrument_at).phrases then
    fail("bad-params", "no such phrase")
  end
  local phrase = song:instrument(instrument_at):phrase(phrase_at)
  local lines = phrase.number_of_lines
  local selected = song.selected_instrument_index == instrument_at and song.selected_phrase_index == phrase_at
  local from, to = window_of(params, selected and song.selected_phrase_line_index or 17, lines)
  local note_columns, effect_columns = phrase.visible_note_columns, phrase.visible_effect_columns
  local read = {}
  -- (the phrase looked up once a slice: a line is the one call to Renoise a row makes, until the job waits)
  local live
  for i = from, to do
    if job.slice() or not live then live = the_song():instrument(instrument_at):phrase(phrase_at) end
    local row = line_row(live:line(i), i, note_columns, effect_columns, params.hidden == true)
    if row then read[#read + 1] = row end
  end
  -- The same grid as a pattern's, of one track.
  local entries = { { ref = refs.make(epoch, "phrase", instrument_at, phrase_at), name = phrase.name, color = json.null,
    noteColumns = note_columns, effectColumns = effect_columns, rows = read } }
  local result = {
    epoch = epoch,
    ref = refs.make(epoch, "phrase", instrument_at, phrase_at),
    instrumentRef = refs.make(epoch, "instrument", instrument_at),
    name = phrase.name,
    lines = lines,
    from = from,
    lpb = phrase.lpb,
    keyTracking = key_tracking_name(phrase.key_tracking),
    looping = phrase.looping,
  }
  if selected then
    local cursor = the_song().selected_phrase_line_index
    if cursor > 0 then result.cursor = cursor end
  end
  return fit(result, entries, from, to, false)
end

-- An instrument's samples and their keyzones (the keyzone map: each zone's notes and velocities), its macros and its phrases with
-- their keys (n-instruments: what plays the zones and the phrases, and how the zones overlap).
local function instrument_body(job, params, epoch)
  local at = refs.expect(params.ref, "instrument", 1, epoch)[1]
  local song = the_song()
  if at > #song.instruments then fail("bad-params", "no instrument " .. at) end
  local instrument = song:instrument(at)
  local keyzones, phrases, ranges = {}, {}, {}
  local sample_count, phrase_count = #instrument.samples, #instrument.phrases
  for s = 1, math.min(sample_count, M.MAX_BODY_ROWS) do
    job.slice()
    local row = sample_row(the_song(), epoch, at, s)
    keyzones[s] = {
      sample = row.name, sampleRef = row.ref, noteRange = row.noteRange, velocityRange = row.velocityRange,
      baseNote = row.baseNote,
    }
    if row.layer ~= "note-on" then keyzones[s].layer = row.layer end
    if row.slice then keyzones[s].slice = true end
    if row.muteGroup and row.muteGroup > 0 then keyzones[s].muteGroup = row.muteGroup end
  end
  for p = 1, math.min(phrase_count, M.MAX_BODY_ROWS) do
    job.slice()
    local phrase = the_song():instrument(at):phrase(p)
    phrases[p] = phrase.name
    local mapping = phrase.mapping
    ranges[p] = mapping and array({ mapping.note_range[1], mapping.note_range[2] }) or json.null
  end
  local row = instrument_row(the_song(), epoch, at)
  local result = {
    epoch = epoch,
    ref = row.ref,
    name = row.name,
    plugin = row.plugin,
    keyzones = array(keyzones),
    macros = row.macros,
    phrases = array(phrases),
    -- n-instruments
    kind = row.kind,
    overlap = row.overlap,
    phrasePlayback = row.phrasePlayback,
    phraseRanges = array(ranges),
    modsets = row.modsetCount,
    chains = row.chainCount,
  }
  if row.phraseProgramRef then result.phraseProgramRef, result.phraseProgram = row.phraseProgramRef, the_song():instrument(at).phrase_program end
  if sample_count > M.MAX_BODY_ROWS or phrase_count > M.MAX_BODY_ROWS then result.truncated = true end
  return reads.bound(result, { "keyzones", "phrases", "phraseRanges" })
end

-- The chain of a track. Flat until M4 reads the Doofers' and Splitters' XML: they are marked as containers.
local function dsp_body(job, params, epoch)
  local at = refs.expect(params.ref, "track", 1, epoch)[1]
  local song = the_song()
  if at > track_count(song) then fail("bad-params", "no track " .. at) end
  local nodes = {}
  for d = 1, #song:track(at).devices do
    job.slice()
    local row = device_row(the_song(), epoch, at, d)
    nodes[d] = {
      ref = row.ref, name = row.displayName, enabled = row.active, plugin = row.plugin,
      container = row.isDoofer or row.isSplitter, childNodes = json.array(),
    }
  end
  return reads.bound({ epoch = epoch, ref = refs.make(epoch, "track", at), name = song:track(at).name, nodes = array(nodes) }, { "nodes" })
end

-- The mixer: a page of the tracks' strips around the selected one (`limit` of them), each with its post-FX fader in dB, its
-- panning (-1 to 1), and whether it is muted (Renoise's mute state) or soloed, as the mixer shows them. Renoise's Lua API
-- gives no meters, so a strip has no level (`levels` false says so).
local function mixer_body(job, params, epoch)
  local limit = params.limit == nil and M.MIXER_STRIPS or integer(params.limit, "limit", 1, M.MAX_MIXER_STRIPS)
  local song = the_song()
  local tracks = track_count(song)
  local from = math.max(1, math.min(song.selected_track_index - math.floor(limit / 2), tracks - limit + 1))
  local strips = {}
  for t = from, math.min(tracks, from + limit - 1) do
    job.slice()
    local track = the_song():track(t)
    local strip = {
      ref = refs.make(epoch, "track", t), name = track.name, type = track_type(track), color = hex(track.color),
      volumeDb = rows.db(track.postfx_volume.value), pan = (track.postfx_panning.value - 0.5) * 2,
      selected = t == the_song().selected_track_index,
    }
    if track.type ~= renoise.Track.TRACK_TYPE_MASTER then
      strip.mute = rows.mute_name(track.mute_state)
      strip.solo = track.solo_state
    end
    strips[#strips + 1] = strip
  end
  return reads.bound({ epoch = epoch, strips = array(strips), trackCount = tracks, from = from - 1, levels = false }, { "strips" })
end

-- focus_body {kind, ref?, window?}
function M.focus_body(job, params, epoch)
  if type(params) ~= "table" then fail("bad-params", "focus_body needs a kind") end
  local kind = params.kind
  if kind == "pattern" then return pattern_body(job, params, epoch) end
  if kind == "matrix" then return matrix_body(job, params, epoch) end
  if kind == "phrase" then return phrase_body(job, params, epoch) end
  if kind == "instrument" then return instrument_body(job, params, epoch) end
  if kind == "dsp" then return dsp_body(job, params, epoch) end
  if kind == "mixer" then return mixer_body(job, params, epoch) end
  fail("bad-params", "unknown body " .. tostring(kind) .. " (pattern, matrix, phrase, instrument, dsp, mixer)")
end

return M
