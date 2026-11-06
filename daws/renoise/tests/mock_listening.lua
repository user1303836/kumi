-- What the mock Renoise does for listening (M5), as Renoise 3.5.4 was measured to do it (m5/renoise.md):
--   song:render   renders in the background (`rendering` true, `transport.playing` true meanwhile, playback stopped after), takes
--                 end_pos inclusive, raises for a line out of range or a second render; the test ends it with `finish_render`,
--                 which writes the WAV (cut short with `fraction`) and calls back (unless `no_callback`).
--   cancel_rendering  is recorded as a crash: it leaves Renoise's undo dead, so the tool must never call it.
--   solo          is mock_song.lua's (`M.solo_set`): it rewrites mute states, and a solo is exclusive.
--   samples       have a sample buffer (`buffer = {rate, frames, channels}` in their data) that save_as writes, and slices.
--   transport     plays from start_at at the song's tempo (`playback_pos_beats` moves with the mock clock), follow mode drags the
--                 edit position, and the song's length in beats is told.

return function(M, object)

local function u16(n) return string.char(n % 256, math.floor(n / 256) % 256) end
local function u32(n)
  return string.char(n % 256, math.floor(n / 256) % 256, math.floor(n / 65536) % 256, math.floor(n / 16777216) % 256)
end

-- A WAV of silence: 32-bit float, as Renoise writes them.
function M.write_wav(path, frames, rate, channels)
  channels = channels or 2
  local size = frames * channels * 4
  local f = assert(io.open(path, "wb"))
  f:write("RIFF", u32(36 + size), "WAVE", "fmt ", u32(16), u16(3), u16(channels), u32(rate), u32(rate * channels * 4),
    u16(channels * 4), u16(32), "data", u32(size))
  local chunk = string.rep("\0", 65536)
  local left = size
  while left > 0 do
    local n = math.min(left, #chunk)
    f:write(n == #chunk and chunk or chunk:sub(1, n))
    left = left - n
  end
  f:close()
end

-- The seconds a render from `start` to `stop` (inclusive) lasts, and the BPM and LPB it ends at, as Renoise 3.5.4 renders
-- (measured): it starts at the last ZT and ZL before `start` (as playback from the song's start would have them), but for a kind
-- with none before it, at the song's FIRST command of that kind anywhere in the sequence, else the transport's; then every
-- command it meets applies from its line.
function M.render_seconds(start, stop)
  local d = M.data
  local song = d.song
  local function commands(s, l)
    local p = d.sequence[s]
    local found = {}
    for t = 1, #d.tracks do
      local effects = ((d.patterns[p].tracks[t] or {}).lines or {})[l]
      effects = effects and effects.effects or {}
      for c = 1, d.tracks[t].effect_columns or 1 do
        local e = effects[c]
        if e and e.number and e.number ~= 0 then
          local text = song:pattern(p):track(t):line(l):effect_column(c).number_string
          if text == "ZL" and e.amount > 0 then found.lpb = e.amount elseif text == "ZT" and e.amount >= 32 then found.bpm = e.amount end
        end
      end
    end
    return found
  end
  local first = {}
  for s = 1, #d.sequence do
    for l = 1, d.patterns[d.sequence[s]].lines do
      local c = commands(s, l)
      first.lpb, first.bpm = first.lpb or c.lpb, first.bpm or c.bpm
    end
  end
  local lpb, bpm
  for s = 1, start.sequence do
    for l = 1, d.patterns[d.sequence[s]].lines do
      if s == start.sequence and l >= start.line then break end
      local c = commands(s, l)
      lpb, bpm = c.lpb or lpb, c.bpm or bpm
    end
  end
  lpb = lpb or first.lpb or d.transport_fields.lpb
  bpm = bpm or first.bpm or d.transport_fields.bpm
  local seconds = 0
  for s = start.sequence, stop.sequence do
    for l = (s == start.sequence) and start.line or 1, d.patterns[d.sequence[s]].lines do
      local c = commands(s, l)
      lpb, bpm = c.lpb or lpb, c.bpm or bpm
      seconds = seconds + 60 / (bpm * lpb)
      if s == stop.sequence and l == stop.line then return seconds, bpm, lpb end
    end
  end
  return seconds, bpm, lpb
end

local LOGIC = "std::logic_error: "

local function move_edit_position(d, position)
  local fields = d.song_fields
  local sequence, pattern = fields.selected_sequence_index, fields.selected_pattern_index
  d.transport_fields.edit_pos = { sequence = position.sequence, line = position.line }
  fields.selected_sequence_index, fields.selected_line_index = position.sequence, position.line
  fields.selected_pattern_index = d.sequence[position.sequence]
  if sequence ~= fields.selected_sequence_index then M.fire(fields.selected_sequence_index_observable) end
  if pattern ~= fields.selected_pattern_index then M.fire(fields.selected_pattern_index_observable) end
end

local function install_render(song)
  local d = M.data
  local fields = M.impl(song)
  fields.rendering_progress = 0
  fields.render = function(_, options, path, done)
    if fields.rendering then error(LOGIC .. "'rendering already is in progress. can only start one render process.'", 2) end
    if type(options) ~= "table" or type(path) ~= "string" or type(done) ~= "function" then error("mock: render(options, path, done)", 2) end
    for _, position in ipairs({ options.start_pos, options.end_pos }) do
      local p = d.sequence[position.sequence]
      if not p or position.line < 1 or position.line > d.patterns[p].lines then
        error(LOGIC .. "'invalid start_pos or end_pos 'line'. line must be [1 - pattern_length].'", 2)
      end
    end
    if M.render_refuses then return false, M.render_refuses end
    local record = { path = path, options = options, done = done, tracks = {}, rate = options.sample_rate or 44100 }
    for k, t in ipairs(d.tracks) do record.tracks[k] = { mute = t.mute_state, solo = t.solo == true, name = t.name, kind = t.kind } end
    record.seconds, record.bpm, record.lpb = M.render_seconds(options.start_pos, options.end_pos)
    record.was_playing = d.transport_fields.playing
    M.renders[#M.renders + 1] = record
    M.pending_render = record
    fields.rendering, fields.rendering_progress = true, 0
    -- (Renoise reads as playing while it renders; playback is stopped once it ends)
    d.transport_fields.playing = true
    M.fire(d.transport_fields.playing_observable)
    return true
  end
  fields.cancel_rendering = function()
    M.crashes[#M.crashes + 1] = "cancel_rendering (it leaves Renoise's undo dead until the app loses focus: measured)"
    fields.rendering = false
  end
end

-- The render in flight ends: `fraction` of its frames written (a render cut short still calls back: measured after a cancel),
-- `no_callback` (it ends with none), `no_file`.
function M.finish_render(options)
  options = options or {}
  local record = assert(M.pending_render, "mock: no render in flight")
  M.pending_render = nil
  local d = M.data
  local frames = math.floor(record.seconds * record.rate + 0.5)
  if options.fraction then frames = math.floor(frames * options.fraction) end
  if not options.no_file then M.write_wav(record.path, frames, record.rate, 2) end
  record.frames = frames
  local fields = M.impl(d.song)
  fields.rendering, fields.rendering_progress = false, 0
  d.transport_fields.playing = false
  M.fire(d.transport_fields.playing_observable)
  -- (the render played the song's tempo commands into the transport, as playback does: no undo step)
  local tf = d.transport_fields
  if tf.bpm ~= record.bpm or tf.lpb ~= record.lpb then
    tf.bpm, tf.lpb = record.bpm, record.lpb
    M.fire(tf.bpm_observable)
    M.fire(tf.lpb_observable)
  end
  -- Row 13, Renoise 3.5.4: follow-player left the edit cursor at the rendered window's start.
  if tf.follow_player then move_edit_position(d, record.options.start_pos) end
  if not options.no_callback then
    local ok, err = M.run_callback(record.done)
    if not ok then M.errors[#M.errors + 1] = "render callback: " .. tostring(err) end
  end
  return record
end

function M.render_progress(value)
  M.impl(M.data.song).rendering_progress = value
end

-- Renoise's solo is mock_song.lua's (M4 n-song, measured); a producer's solo button, for the tests.
function M.solo_write(i, value) M.solo_set(M.data.tracks[i], value == true) end

-- Samples

-- A sample's buffer saved as a WAV the bridge can read back (its header says its length), as Renoise does; a folder that isn't
-- there gets Renoise's own error dialog and false.
function M.save_wav(sample, path)
  local buffer = sample.buffer
  local dir = path:match("^(.*)[/\\][^/\\]+$")
  if not buffer or (dir and not io.exists(dir)) then
    M.modals[#M.modals + 1] = { "save_as", path }
    M.errors[#M.errors + 1] = "modal: save_as failed"
    return false
  end
  M.write_wav(path, buffer.frames, buffer.rate, buffer.channels)
  M.saved_samples[#M.saved_samples + 1] = path
  return true
end

function M.sample_extras(sample, fields, getters)
  local buffer = sample.buffer
  if fields.sample_buffer == nil then
    fields.sample_buffer = object("renoise.SampleBuffer", {
      has_sample_data = buffer ~= nil, sample_rate = buffer and buffer.rate or 44100, number_of_frames = buffer and buffer.frames or 0,
      number_of_channels = buffer and buffer.channels or 2, bit_depth = buffer and buffer.bits or 32, read_only = sample.slice_alias == true,
      save_as = function(_, path, format)
        if format ~= "wav" and format ~= "flac" then error("mock: save_as wants wav or flac", 2) end
        local dir = path:match("^(.*)[/\\][^/\\]+$")
        if not buffer or (dir and not io.exists(dir)) then
          -- (Renoise shows its own error dialog)
          M.modals[#M.modals + 1] = { "save_as", path }
          M.errors[#M.errors + 1] = "modal: save_as failed"
          return false
        end
        M.write_wav(path, buffer.frames, buffer.rate, buffer.channels)
        M.saved_samples[#M.saved_samples + 1] = path
        return true
      end,
    })
  end
  if getters.is_slice_alias == nil then getters.is_slice_alias = function() return sample.slice_alias == true end end
  if fields.slice_markers == nil then fields.slice_markers = sample.markers or {} end
end

-- The transport

function M.transport_extras(tf, getters, setters, d)
  tf.timing_model = tf.timing_model or 2
  tf.edit_pos = tf.edit_pos or { sequence = 1, line = 1 }
  setters.edit_pos = function(position) move_edit_position(d, position) end
  local function beat_of(position)
    local lines = 0
    for s = 1, position.sequence - 1 do lines = lines + d.patterns[d.sequence[s]].lines end
    return (lines + position.line - 1) / tf.lpb
  end
  local function position_of(beat)
    local line = math.floor(beat * tf.lpb + 1e-9)
    for s = 1, #d.sequence do
      local lines = d.patterns[d.sequence[s]].lines
      if line < lines then return { sequence = s, line = line + 1 } end
      line = line - lines
    end
    return { sequence = #d.sequence, line = d.patterns[d.sequence[#d.sequence]].lines }
  end
  getters.song_length_beats = function()
    local lines = 0
    for s = 1, #d.sequence do lines = lines + d.patterns[d.sequence[s]].lines end
    return lines / tf.lpb
  end
  getters.playback_pos_beats = function()
    if tf.playing and d.play_start then return d.play_start.beat + (M.time - d.play_start.time) / 1000 * tf.bpm / 60 end
    return tf.playback_pos_beats
  end
  local start_at, stop = tf.start_at, tf.stop
  tf.start_at = function(self, position)
    start_at(self, position)
    local at = type(position) == "table" and position or { sequence = d.song_fields.selected_sequence_index, line = position }
    d.play_start = { time = M.time, beat = beat_of(at) }
  end
  tf.stop = function(self)
    -- (follow mode drags the edit position to where playback stopped)
    if d.play_start and tf.follow_player then tf.edit_pos = position_of(getters.playback_pos_beats()) end
    stop(self)
    d.play_start = nil
  end
end

-- Each test starts with no render, and with the transport's constants Renoise has.
local reset = M.reset
function M.reset(options)
  reset(options)
  M.renders, M.pending_render, M.render_refuses, M.saved_samples, M.copies_from = {}, nil, nil, {}, 0
  local transport = M.impl(renoise.Transport)
  transport.TIMING_MODEL_SPEED, transport.TIMING_MODEL_LPB = 1, 2
  return M
end

local make_song = M.make_song
function M.make_song(options)
  local song = make_song(options)
  install_render(song)
  return song
end

end
