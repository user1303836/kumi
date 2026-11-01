-- The tool's reads (read, snapshot, focus, focus_body) against a song in the mock Renoise, with a Lua client playing
-- Kumi's host: it gives credit back as it reads, as the real host does.
local T = ...
local mock = T.mock
local json = T.load_payload_module("json")
local test, eq, ok = T.test, T.eq, T.ok
local H = T.harness

local MAX_LINE = 256 * 1024

-- A song with groups (Mix holds Drums, which holds Kick and Hat, and Bass), a send, three patterns, a four slot sequence
-- and two instruments.
local function demo(extra)
  local spec = {
    name = "Demo", file_name = "/s/demo.xrns", bpm = 128, lpb = 4,
    tracks = {
      { name = "Kick", color = { 255, 0, 0 } },
      { name = "Hat" },
      { name = "Drums", kind = "group", members = 2 },
      { name = "Bass", note_columns = 2 },
      { name = "Mix", kind = "group", members = 4, effect_columns = 2 },
    },
    send_tracks = 1,
    patterns = 3, pattern_lines = 32, sequence_length = 4, sequence = { 1, 2, 1, 3 },
    instruments = 2,
    instrument_specs = { { name = "Kit", samples = 2, phrases = 1 }, { name = "Pad", samples = 1, plugin = { name = "Serum" } } },
  }
  for k, v in pairs(extra or {}) do spec[k] = v end
  return spec
end

local next_id = 1000

-- What a host does as it reads: takes what the socket holds and gives credit back for exactly those bytes (a line not
-- yet whole too: an answer longer than the credit goes on that way), until the grant releases no more.
local function drain(peer)
  local total = 0
  repeat
    local taken = peer:take()
    if #taken > 0 then H.send(peer, { type = "credit", bytes = #taken }) end
    total = total + #taken
  until #taken == 0
  return total
end

-- Sends a request and runs the clock until its response comes, giving credit back for every line read. Returns the
-- response, the other frames that came meanwhile, and the longest line.
local function call(peer, method, params, ticks)
  next_id = next_id + 1
  local id = next_id
  H.send(peer, { type = "request", id = id, method = method, params = params or {} })
  local response, others, longest = nil, {}, 0
  for _ = 1, ticks or 600 do
    drain(peer)
    for _, line in ipairs(peer:frames(nil)) do
      longest = math.max(longest, #line + 1)
      local frame = assert(json.decode(line))
      if frame.type == "response" and frame.id == id then response = frame else others[#others + 1] = frame end
    end
    if response then break end
    mock.advance(10)
  end
  return response, others, longest
end

local function setup(options, song)
  options = options or {}
  options.song = song or demo()
  local env = H.setup(options)
  local peer = H.connect(env)
  H.frames(peer)
  return env, peer
end

local function epoch_of(peer)
  return call(peer, "focus").result.epoch
end

local function ref(peer, kind, ...)
  return epoch_of(peer) .. ":" .. kind .. ":" .. table.concat({ ... }, ".")
end

local function read(peer, kind, params)
  params = params or {}
  params.kind = kind
  local response = call(peer, "read", params)
  ok(response, "an answer to read " .. kind)
  return response
end

local function rows(peer, kind, params)
  local response = read(peer, kind, params)
  ok(response.result, "read " .. kind .. " failed: " .. json.encode(response.error or {}))
  return response.result.items, response.result
end

local function near(actual, expected)
  ok(type(actual) == "number" and math.abs(actual - expected) < 0.001, "expected about " .. expected .. ", got " .. tostring(actual))
end

local function failure(response, code)
  ok(response.error, "expected an error")
  eq(response.error.code, code, "error code")
end

local function fill_demo()
  local ops = mock.ops
  ops.set_note(1, 1, 1, 1, { note = 48, instrument = 0 })
  ops.set_note(1, 1, 5, 1, { note = 51, instrument = 0, volume = 64, fxn = 10, fxa = 32 })
  ops.set_note(1, 4, 3, 1, { note = 36, instrument = 1 })
  ops.set_note(1, 4, 3, 2, { note = 120 })
  ops.set_effect(1, 5, 2, 1, 10, 64)
  ops.set_note(2, 1, 1, 1, { note = 60, instrument = 0 })
  ops.alias(2, 2, 1)
  ops.section(1, "Intro")
  ops.mute_slot(2, 3, true)
  local data = mock.data
  data.tracks[1].devices = {
    { name = "EQ 5", presets = { "Flat", "Bright" }, preset = 2,
      params = { { name = "Gain", value = 0.5 }, { name = "Freq", value = 0.25, min = 0, max = 1 } } },
    { name = "Doofer", params = {} },
  }
  data.tracks[2].volume, data.tracks[2].panning = 0.5, 1
  data.instruments[1].macros[1].name, data.instruments[1].macros[1].value = "Cutoff", 0.75
  data.instruments[1].phrases[1].data[3] = { notes = { [1] = { note = 48, instrument = 255, volume = 255, panning = 255, delay = 0, fxn = 0, fxa = 0 } } }
end

test("reads: song", function()
  local env, peer = setup()
  local items, result = rows(peer, "song")
  eq(#items, 1)
  ok(result.epoch:match("^%x+$"), "an epoch in hex")
  eq(items[1].name, "Demo")
  ok(items[1].id:match("^%x+$") and #items[1].id == 32, "the song's id")
  ok(items[1].identity:match("^4242:%d+$"), "the pid and which opening of a song this is: " .. items[1].identity)
  eq(items[1].path, "/s/demo.xrns")
  eq(items[1].bpm, 128)
  eq(items[1].lpb, 4)
  eq(items[1].tpl, 6)
  eq(items[1].patternCount, 3)
  eq(items[1].sequenceLength, 4)
  eq(items[1].playing, false)
  H.no_dialogs()
end)

test("reads: tracks are rows with their groups, colours and levels", function()
  local env, peer = setup()
  fill_demo()
  local items = rows(peer, "track")
  eq(#items, 7, "five tracks, the master and a send")
  local E = epoch_of(peer)
  eq(items[1].ref, E .. ":track:1")
  eq(items[1].type, "sequencer")
  eq(items[1].color, "#ff0000")
  eq(items[2].color, "#29a699", "the colour Renoise gave it")
  eq(items[2].colorBlend, 0)
  eq(items[1].groupRef, E .. ":track:3", "Kick is in Drums")
  eq(items[2].groupRef, E .. ":track:3")
  eq(items[3].type, "group")
  eq(items[3].groupRef, E .. ":track:5", "Drums is in Mix")
  eq(items[4].groupRef, E .. ":track:5")
  eq(items[5].groupRef, json.null, "Mix is at the top")
  eq(items[6].type, "master")
  eq(items[6].mute, nil, "the master can't be muted")
  eq(items[7].type, "send")
  eq(items[1].mute, "active")
  eq(items[1].volumeDb, 0)
  ok(math.abs(items[2].volumeDb + 6.0206) < 0.001, "0.5 is about -6 dB")
  eq(items[2].pan, 1)
  eq(items[4].noteColumns, 2)
  eq(items[5].effectColumns, 2)
  eq(items[1].deviceCount, 2)
  H.no_dialogs()
end)

test("reads: the mixer body has each track's post-FX fader, panning, mute and solo, and says it has no meters", function()
  local env, peer = setup()
  fill_demo()
  local body = call(peer, "focus_body", { kind = "mixer" }).result
  local E = epoch_of(peer)
  eq(#body.strips, 7, "every track, the master and the send")
  eq((body.from), 0)
  eq(body.trackCount, 7)
  eq(body.levels, false, "Renoise's API gives no meters")
  eq(body.strips[1].ref, E .. ":track:1")
  eq(body.strips[1].mute, "active")
  eq(body.strips[1].selected, true)
  eq(type(body.strips[1].volumeDb), "number")
  eq(type(body.strips[1].pan), "number")
  eq(body.strips[6].type, "master")
  eq(body.strips[6].mute, nil, "the master can't be muted")
  -- A page of two around the selected track, and a limit past the most refused.
  local page = call(peer, "focus_body", { kind = "mixer", limit = 2 }).result
  eq(#page.strips, 2)
  eq(page.from, 0)
  eq(call(peer, "focus_body", { kind = "mixer", limit = 65 }).error.code, "bad-params")
  H.no_dialogs()
end)

test("reads: tracks page by limit and cursor, and a cursor goes stale with the epoch", function()
  local env, peer = setup()
  local items, result = rows(peer, "track", { limit = 3 })
  eq(#items, 3)
  eq(items[3].index, 3)
  ok(result.nextCursor, "more to come")
  items, result = rows(peer, "track", { limit = 3, cursor = result.nextCursor })
  eq(items[1].index, 4)
  eq(#items, 3)
  local cursor = result.nextCursor
  items, result = rows(peer, "track", { limit = 3, cursor = cursor })
  eq(#items, 1)
  eq(result.nextCursor, nil, "the last page has no cursor")
  mock.ops.insert_pattern()
  H.tick(2)
  failure(read(peer, "track", { cursor = cursor }), "stale")
  failure(read(peer, "track", { cursor = "nonsense" }), "bad-params")
  H.no_dialogs()
end)

test("reads: the sequence, patterns and their aliases", function()
  local env, peer = setup()
  fill_demo()
  local E = epoch_of(peer)
  local sequence = rows(peer, "sequence")
  eq(#sequence, 4)
  eq(sequence[1].ref, E .. ":seq:1")
  eq(sequence[1].patternRef, E .. ":pattern:1")
  eq(sequence[2].patternRef, E .. ":pattern:2")
  eq(sequence[3].patternRef, E .. ":pattern:1")
  eq(sequence[1].section, "Intro")
  eq(sequence[2].section, json.null)
  eq(#sequence[3].mutedTracks, 1)
  eq(sequence[3].mutedTracks[1], 2)
  local patterns = rows(peer, "pattern")
  eq(#patterns, 3)
  eq(patterns[1].lines, 32)
  eq(patterns[1].number, 0, "pattern 00, as Renoise numbers it")
  eq(sequence[2].pattern, 1, "slot 2 plays pattern 01")
  eq(patterns[2].aliases["2"], 0, "track 2 of pattern 01 is a copy of pattern 00's")
  eq(next(patterns[1].aliases), nil)
  H.no_dialogs()
end)

test("reads: lines are the tracker's text, empty ones are left out", function()
  local env, peer = setup()
  fill_demo()
  local E = epoch_of(peer)
  local items = rows(peer, "line", { parent = E .. ":pattern:1", track = 1 })
  eq(#items, 2)
  eq(items[1].line, 0, "lines by Renoise's numbers, from 00")
  eq(items[1].notes[1].note, "C-4")
  eq(items[1].notes[1].instrument, "00")
  eq(items[1].notes[1].volume, nil, "an empty field is left out")
  eq(items[1].notes[1].fx, nil, "no effect, no field")
  eq(items[1].effects, nil)
  eq(items[2].line, 4)
  eq(items[2].notes[1].note, "D#4")
  eq(items[2].notes[1].volume, "40")
  eq(items[2].notes[1].fx, "0A20")
  -- Another track by ref, two note columns, one of them a note off
  items = rows(peer, "line", { parent = E .. ":pattern:1", track = E .. ":track:4" })
  eq(#items, 1)
  eq(#items[1].notes, 2)
  eq(items[1].notes[1].col, 1)
  eq(items[1].notes[2].note, "OFF")
  -- Effect columns
  items = rows(peer, "line", { parent = E .. ":pattern:1", track = 5 })
  eq(#items, 1)
  eq(items[1].line, 1)
  eq(items[1].effects[1].number, "0A")
  eq(items[1].effects[1].amount, "40")
  -- Pages of rows
  local first, result = rows(peer, "line", { parent = E .. ":pattern:1", track = 1, limit = 1 })
  eq(#first, 1)
  ok(result.nextCursor)
  local second, last = rows(peer, "line", { parent = E .. ":pattern:1", track = 1, limit = 1, cursor = result.nextCursor })
  eq(second[1].line, 4)
  eq(last.nextCursor, nil)
  H.no_dialogs()
end)

test("reads: line needs a pattern and a track, and refuses what isn't one", function()
  local env, peer = setup()
  local E = epoch_of(peer)
  failure(read(peer, "line", { track = 1 }), "bad-params")
  failure(read(peer, "line", { parent = E .. ":pattern:1" }), "bad-params")
  failure(read(peer, "line", { parent = E .. ":pattern:9", track = 1 }), "bad-params")
  failure(read(peer, "line", { parent = E .. ":track:1", track = 1 }), "bad-params")
  failure(read(peer, "line", { parent = E .. ":pattern:1", track = 99 }), "bad-params")
  failure(read(peer, "line", { parent = "0:pattern:1", track = 1 }), "stale")
  failure(read(peer, "nonsense"), "bad-params")
  failure(read(peer, "track", { limit = 0 }), "bad-params")
  failure(read(peer, "track", { limit = 201 }), "bad-params")
  failure(read(peer, "track", { limit = 1.5 }), "bad-params")
  H.no_dialogs()
end)

test("reads: instruments, samples, phrases, devices and parameters", function()
  local env, peer = setup()
  fill_demo()
  local E = epoch_of(peer)
  local instruments = rows(peer, "instrument")
  eq(#instruments, 2)
  eq(instruments[1].ref, E .. ":instrument:1")
  eq(instruments[1].name, "Kit")
  eq(instruments[1].sampleCount, 2)
  eq(instruments[1].phraseCount, 1)
  eq(instruments[1].plugin, json.null)
  eq(instruments[1].macros[1].name, "Cutoff")
  eq(instruments[1].macros[1].value, 0.75)
  eq(#instruments[1].macros, 8)
  eq(instruments[2].plugin, "Serum")
  local samples = rows(peer, "sample", { parent = E .. ":instrument:1" })
  eq(#samples, 2)
  eq(samples[2].ref, E .. ":sample:1.2")
  eq(samples[1].baseNote, 48)
  eq(samples[1].noteRange[2], 119)
  eq(samples[1].velocityRange[2], 127)
  eq(samples[1].loop, "off")
  eq(samples[1].muteGroup, 0)
  local phrases = rows(peer, "phrase", { parent = E .. ":instrument:1" })
  eq(#phrases, 1)
  eq(phrases[1].ref, E .. ":phrase:1.1")
  eq(phrases[1].lines, 16)
  eq(phrases[1].keyTracking, "none")
  eq(phrases[1].looping, true)
  local devices = rows(peer, "device", { parent = E .. ":track:1" })
  eq(#devices, 2)
  eq(devices[1].ref, E .. ":device:1.1")
  eq(devices[1].name, "EQ 5")
  eq(devices[1].preset, "Bright")
  eq(devices[1].paramCount, 2)
  eq(devices[1].isDoofer, false)
  eq(devices[2].isDoofer, true)
  eq(devices[2].preset, json.null)
  local params = rows(peer, "param", { parent = E .. ":device:1.1" })
  eq(#params, 2)
  eq(params[1].ref, E .. ":param:1.1.1")
  eq(params[1].name, "Gain")
  eq(params[1].value, 0.5)
  eq(params[1].valueString, "0.50")
  eq(params[2].min, 0)
  eq(params[2].max, 1)
  failure(read(peer, "sample", { parent = E .. ":instrument:9" }), "bad-params")
  failure(read(peer, "param", { parent = E .. ":device:1.9" }), "bad-params")
  failure(read(peer, "device", { parent = E .. ":track:99" }), "bad-params")
  H.no_dialogs()
end)

test("reads: snapshot gathers the small things", function()
  local env, peer = setup()
  fill_demo()
  mock.ops.select("selected_track_index", 4)
  local response = call(peer, "snapshot")
  local s = response.result
  local E = s.epoch
  eq(s.project.name, "Demo")
  eq(s.transport.playing, false)
  eq(s.transport.bpm, 128)
  eq(#s.tracks, 7)
  eq(s.tracks[1].ref, E .. ":track:1")
  eq(s.selection.trackRef, E .. ":track:4")
  eq(s.selection.patternRef, E .. ":pattern:1")
  eq(s.selection.instrumentRef, E .. ":instrument:1")
  eq(s.selection.sampleRef, nil, "no sample selected")
  eq(s.selection.cursor.line, 0)
  eq(s.selection.cursor.track, 4)
  ok(math.type == nil and type(s.changed) == "number", "the change counter")
  eq(s.moreTracks, nil)
  H.no_dialogs()
end)

test("reads: the snapshot carries the producer's setup: usual pattern length, groups and colours, tools", function()
  local env, peer = setup()
  fill_demo()
  local s = call(peer, "snapshot").result
  eq(s.setup.patternLines, 32)
  eq(s.setup.patternLinesVary, nil)
  eq(#s.setup.groups, 2)
  eq(s.setup.groups[1].name, "Drums")
  eq(s.setup.groups[1].tracks, 2)
  eq(s.setup.groups[2].name, "Mix")
  eq(s.setup.groups[2].tracks, 4)
  ok(s.setup.groups[1].color:match("^#%x%x%x%x%x%x$"), "a group's colour")
  eq(#s.setup.tools, 0, "Kumi's own tool isn't listed")
  eq(s.setup.paketti, false)
  -- A song whose patterns mostly run 64 lines, with Paketti and another tool installed (one switched off).
  local song = renoise.song()
  song:pattern(1).number_of_lines = 64
  local tools = renoise.app().installed_tools
  tools[#tools + 1] = { name = "Paketti", id = "org.lackluster.Paketti", enabled = true }
  tools[#tools + 1] = { name = "Slices to Pattern", id = "com.example.Slices", enabled = true }
  tools[#tools + 1] = { name = "Off Tool", id = "com.example.Off", enabled = false }
  s = call(peer, "snapshot").result
  eq(s.setup.patternLines, 64, "the length most slots play (slots 1 and 3 play pattern 1)")
  eq(s.setup.patternLinesVary, true)
  T.same(s.setup.tools, { "Paketti", "Slices to Pattern" })
  eq(s.setup.paketti, true)
  H.no_dialogs()
end)

test("reads: focus is cheap, names the view and follows the selection and the transport", function()
  local env, peer = setup()
  fill_demo()
  local f = call(peer, "focus").result
  local E = f.epoch
  eq(f.view, "pattern")
  eq(f.matrixVisible, false)
  eq(f.dspVisible, false)
  eq(f.selection.trackRef, E .. ":track:1")
  eq(f.selection.sequenceRef, E .. ":seq:1")
  eq(f.selection.cursor.sequence, 1)
  eq(f.selection.cursor.columnKind, "note")
  -- What FOCUS's header shows
  eq(f.selection.track.name, "Kick")
  eq(f.selection.track.color, "#ff0000")
  eq(f.selection.track.type, "sequencer")
  eq(f.selection.patternName, "")
  eq(f.selection.instrumentName, "Kit")
  eq(f.selection.sequence, 1)
  eq(f.selection.line, 0, "Renoise's line 00")
  eq(f.selection.deviceName, nil, "no device selected")
  eq(f.transport.playing, false)
  eq(f.transport.recording, false)
  eq(f.transport.tempo, 128)
  eq(f.transport.beatsPerBar, 4, "Renoise's own guess is a bar of four")
  eq(f.transport.beat, 0, "stopped at the edit cursor, line 1 of slot 1")
  eq(f.transport.sequence, 1)
  eq(f.transport.line, 0)
  eq(f.transport.loop.on, false)
  eq(f.transport.loop.start, 0)
  eq(f.transport.loop["end"], 16)
  for frame, view in pairs({ [2] = "mixer", [3] = "phrase", [5] = "sample", [4] = "instrument", [9] = "instrument", [1] = "pattern" }) do
    mock.window.active_middle_frame = frame
    eq(call(peer, "focus").result.view, view, "middle frame " .. frame)
  end
  mock.window.pattern_matrix_is_visible = true
  mock.window.lower_frame_is_visible = true
  f = call(peer, "focus").result
  eq(f.matrixVisible, true)
  eq(f.dspVisible, true)
  mock.window.active_lower_frame = 2
  eq(call(peer, "focus").result.dspVisible, false, "the automation pane isn't the DSP chain")
  mock.ops.select_sample(2)
  mock.ops.select_phrase(1)
  mock.ops.select_device(2)
  mock.ops.select("selected_line_index", 9)
  mock.ops.select("selected_note_column_index", 0)
  mock.ops.select("selected_effect_column_index", 2)
  f = call(peer, "focus").result
  eq(f.selection.sampleRef, E .. ":sample:1.2")
  eq(f.selection.phraseRef, E .. ":phrase:1.1")
  eq(f.selection.deviceRef, E .. ":device:1.2")
  eq(f.selection.deviceName, "Doofer")
  eq(f.selection.cursor.line, 8)
  eq(f.selection.cursor.column, 2)
  eq(f.selection.cursor.columnKind, "effect")
  eq(f.transport.beat, 2, "line 9 at 4 lines to the beat is two beats in")
  -- Playing: the play position, in beats; a loop of the pattern, or a block
  mock.ops.transport("playing", true)
  mock.ops.transport("loop_pattern", true)
  mock.ops.transport("metronome_beats_per_bar", 3)
  mock.ops.play_at(3, 17, 9.5)
  f = call(peer, "focus").result
  eq(f.transport.playing, true)
  eq(f.transport.sequence, 3)
  eq(f.transport.line, 16, "Renoise's line 16 is its 17th")
  eq(f.transport.beat, 9.5)
  eq(f.transport.beatsPerBar, 3)
  eq(f.transport.loop.on, true)
  eq(f.transport.loop.pattern, true)
  H.no_dialogs()
end)

test("reads: focus counts changes, and only the ones that happen", function()
  local env, peer = setup()
  fill_demo()
  H.tick(2)
  local base = call(peer, "focus").result.changed
  H.tick(5)
  eq(call(peer, "focus").result.changed, base, "an idle song doesn't count")
  mock.ops.set_note(1, 2, 7, 1, { note = 40 })
  local after = call(peer, "focus").result.changed
  ok(after > base, "a line edit counts")
  mock.ops.rename_track(2, "Closed hat")
  local renamed = call(peer, "focus").result.changed
  ok(renamed > after, "a rename counts")
  mock.ops.mute_track(2, 3)
  ok(call(peer, "focus").result.changed > renamed, "a mute counts")
  H.no_dialogs()
end)

test("reads: the pattern body is a window of lines around the cursor", function()
  local env, peer = setup()
  fill_demo()
  local E = epoch_of(peer)
  local body = call(peer, "focus_body", { kind = "pattern" }).result
  eq(body.ref, E .. ":pattern:1")
  eq(body.name, "")
  eq(body.lines, 32)
  eq(body.lpb, 4)
  eq(body.from, 1)
  eq(body.window.from, 1)
  eq(body.window.to, 32, "33 lines from line 1, cut at the pattern's end")
  eq(#body.tracks, 7)
  eq(body.tracks[1].ref, E .. ":track:1")
  eq(body.tracks[1].name, "Kick")
  eq(body.tracks[1].color, "#ff0000")
  eq(#body.tracks[1].lines, 2)
  eq(body.tracks[1].lines[2].notes[1].note, "D#4")
  eq(body.tracks[4].noteColumns, 2)
  eq(body.cursor, 1, "the edit cursor is in this pattern")
  eq(body.playing, nil, "and nothing plays")
  -- A window of its own, on another pattern: no cursor there
  body = call(peer, "focus_body", { kind = "pattern", ref = E .. ":pattern:2", window = { from = 1, to = 4 } }).result
  eq(body.ref, E .. ":pattern:2")
  eq(body.window.to, 4)
  eq(#body.tracks[1].lines, 1)
  eq(body.cursor, nil, "the cursor is in pattern 1")
  body = call(peer, "focus_body", { kind = "pattern", window = { from = 3, to = 6 } }).result
  eq(#body.tracks[1].lines, 1, "only line 5 is in 3..6")
  eq(body.tracks[1].lines[1].line, 5)
  eq(body.from, 3)
  -- Following the playhead: the pattern under it, and the line it is on
  mock.ops.transport("follow_player", true)
  mock.ops.transport("playing", true)
  mock.ops.play_at(2, 20)
  body = call(peer, "focus_body", { kind = "pattern" }).result
  eq(body.ref, E .. ":pattern:2", "the pattern under the playhead")
  eq(body.playing, 20)
  eq(body.cursor, nil)
  failure(call(peer, "focus_body", { kind = "pattern", window = { from = 5, to = 2 } }), "bad-params")
  failure(call(peer, "focus_body", { kind = "pattern", ref = E .. ":pattern:9" }), "bad-params")
  failure(call(peer, "focus_body", { kind = "nonsense" }), "bad-params")
  H.no_dialogs()
end)

test("reads: the matrix, phrase, instrument and dsp bodies", function()
  local env, peer = setup()
  fill_demo()
  local E = epoch_of(peer)
  local matrix = call(peer, "focus_body", { kind = "matrix" }).result
  eq(#matrix.tracks, 7)
  eq(matrix.tracks[3], "Drums", "the names of the tracks")
  eq(matrix.trackRefs[3], E .. ":track:3")
  eq(#matrix.slots, 4)
  eq(matrix.slots[1].ref, E .. ":seq:1")
  eq(matrix.slots[1].pattern, 1, "the pattern's number")
  eq(matrix.slots[1].section, "Intro")
  eq(matrix.slots[2].section, json.null)
  eq(matrix.slots[2].aliases["2"], 1)
  -- (review U9) the tracks with nothing in the slot's pattern, which the matrix draws as empty blocks
  eq(table.concat(matrix.slots[1].emptyTracks, ","), "2,3,6,7")
  eq(matrix.slots[3].mutedTracks[1], 2)
  eq(matrix.current, 1, "the selected slot when nothing plays")
  mock.ops.transport("playing", true)
  mock.ops.play_at(3, 1)
  eq(call(peer, "focus_body", { kind = "matrix" }).result.current, 3, "the playing slot")
  local phrase = call(peer, "focus_body", { kind = "phrase", ref = E .. ":phrase:1.1" }).result
  eq(phrase.ref, E .. ":phrase:1.1")
  eq(phrase.instrumentRef, E .. ":instrument:1")
  eq(phrase.lines, 16)
  eq(phrase.lpb, 4)
  eq(#phrase.tracks, 1, "the grid of a pattern, of one track")
  eq(#phrase.tracks[1].lines, 1)
  eq(phrase.tracks[1].lines[1].line, 3)
  eq(phrase.tracks[1].lines[1].notes[1].note, "C-4")
  local instrument = call(peer, "focus_body", { kind = "instrument", ref = E .. ":instrument:1" }).result
  eq(instrument.name, "Kit")
  eq(#instrument.keyzones, 2)
  eq(instrument.keyzones[1].sample, "Sample 1", "the sample's name")
  eq(instrument.keyzones[1].sampleRef, E .. ":sample:1.1")
  eq(instrument.keyzones[1].noteRange[1], 0)
  eq(instrument.keyzones[1].baseNote, 48)
  eq(instrument.phrases[1], "Phrase 1", "phrases by name")
  eq(instrument.macros[1].name, "Cutoff")
  local dsp = call(peer, "focus_body", { kind = "dsp", ref = E .. ":track:1" }).result
  eq(dsp.ref, E .. ":track:1")
  eq(#dsp.nodes, 2)
  eq(dsp.nodes[1].ref, E .. ":device:1.1")
  eq(dsp.nodes[1].name, "EQ 5")
  eq(dsp.nodes[1].enabled, true)
  eq(dsp.nodes[1].plugin, false)
  eq(dsp.nodes[1].container, false)
  eq(dsp.nodes[2].container, true, "a Doofer")
  eq(#dsp.nodes[2].childNodes, 0)
  failure(call(peer, "focus_body", { kind = "phrase", ref = E .. ":phrase:1.9" }), "bad-params")
  failure(call(peer, "focus_body", { kind = "instrument", ref = E .. ":instrument:9" }), "bad-params")
  failure(call(peer, "focus_body", { kind = "dsp", ref = E .. ":track:99" }), "bad-params")
  failure(call(peer, "focus_body", { kind = "dsp", ref = E .. ":device:1.1" }), "bad-params")
  H.no_dialogs()
end)

test("reads: nothing keeps reading when Renoise has no song", function()
  local env, peer = setup()
  mock.song = nil
  failure(read(peer, "song"), "no-song")
  failure(call(peer, "focus"), "no-song")
  failure(call(peer, "snapshot"), "no-song")
  H.no_dialogs()
end)

-- A big song: 40 tracks of 12 note columns and 8 effect columns, one 300 line pattern with every cell full.
local function heavy(options)
  local tracks = {}
  for i = 1, options.tracks do tracks[i] = { name = "T" .. i, note_columns = 12, effect_columns = 8 } end
  return demo({ tracks = tracks, send_tracks = 0, patterns = 1, pattern_lines = options.lines, sequence_length = 1, sequence = { 1 } })
end

local function fill_cells(tracks, lines)
  for t = 1, tracks do
    for k = 1, lines do
      for c = 1, 12 do mock.ops.set_note(1, t, k, c, { note = 48 + c, instrument = c, volume = 64, panning = 32, delay = 16, fxn = 10, fxa = 32 }) end
      for c = 1, 8 do mock.ops.set_effect(1, t, k, c, 10 + c, 64) end
    end
  end
end

test("reads: a page never goes over 256 KiB, and the pages together are every row", function()
  local env, peer = setup(nil, heavy({ tracks = 1, lines = 300 }))
  fill_cells(1, 300)
  local E = epoch_of(peer)
  local seen, cursor, pages = {}, nil, 0
  repeat
    local response, _, longest = call(peer, "read", { kind = "line", parent = E .. ":pattern:1", track = 1, limit = 200, cursor = cursor })
    ok(response.result, json.encode(response.error or {}))
    ok(longest <= MAX_LINE, "a page of " .. longest .. " bytes")
    pages = pages + 1
    ok(#response.result.items >= 1, "every page has a row")
    for _, item in ipairs(response.result.items) do seen[item.line] = true end
    cursor = response.result.nextCursor
  until not cursor or pages > 20
  ok(pages >= 2, "it took more than one page: " .. pages)
  for k = 0, 299 do ok(seen[k], "line " .. k .. " was read") end
  H.no_dialogs()
end)

test("reads: a body too big for a page is cut to a smaller window and says so", function()
  local env, peer = setup(nil, heavy({ tracks = 12, lines = 128 }))
  fill_cells(12, 40)
  local E = epoch_of(peer)
  local response, _, longest = call(peer, "focus_body", { kind = "pattern", window = { from = 1, to = 40 } })
  ok(response.result, json.encode(response.error or {}))
  ok(longest <= MAX_LINE, "an answer of " .. longest .. " bytes")
  eq(response.result.truncated, true)
  ok(response.result.window.to < 40, "the window was cut: " .. response.result.window.to)
  eq(response.result.window.from, 1)
  eq(#response.result.tracks, 13, "twelve tracks and the master")
  H.no_dialogs()
end)

test("reads: a long read is sliced over the timers and keeps its connection's answers in order", function()
  local env, peer = setup(nil, heavy({ tracks = 60, lines = 32 }))
  local timers = #mock.timers
  mock.clock_step_ms = 1 -- every look at the clock costs a millisecond: a slice is a few rows
  local other = H.connect(env)
  H.frames(other)
  H.send(peer, { type = "request", id = 1, method = "read", params = { kind = "track", limit = 60 } })
  H.send(peer, { type = "request", id = 2, method = "read", params = { kind = "song" } })
  H.send(other, { type = "request", id = 3, method = "ping" })
  -- Not done at once; the other connection is not made to wait
  local answered = {}
  local order = {}
  for _ = 1, 400 do
    for _, f in ipairs(H.frames(peer)) do if f.type == "response" then order[#order + 1] = f.id end end
    for _, f in ipairs(H.frames(other)) do if f.type == "response" then answered[f.id] = #order end end
    if #order >= 2 then break end
    mock.advance(10)
  end
  eq(order[1], 1, "the read first")
  eq(order[2], 2, "then the read that waited behind it")
  eq(answered[3], 0, "the other connection's ping was answered while the read was still going")
  eq(#mock.timers, timers, "the fast timer is gone when no read is pending")
  mock.clock_step_ms = 0
  H.no_dialogs()
end)

test("reads: the fast timer runs only while a read is pending", function()
  local env, peer = setup(nil, heavy({ tracks = 60, lines = 32 }))
  local timers = #mock.timers
  mock.clock_step_ms = 1
  H.send(peer, { type = "request", id = 1, method = "read", params = { kind = "track", limit = 60 } })
  eq(#mock.timers, timers + 1, "a read is pending: the second timer")
  for _ = 1, 400 do
    mock.advance(10)
    if #mock.timers == timers then break end
  end
  eq(#mock.timers, timers, "and gone after")
  mock.clock_step_ms = 0
  local got = H.frames(peer)
  eq(got[#got].id, 1)
  H.no_dialogs()
end)

test("reads: a song switched in the middle of a read ends it with epoch-changed", function()
  local env, peer = setup(nil, heavy({ tracks = 60, lines = 32 }))
  mock.clock_step_ms = 1
  H.send(peer, { type = "request", id = 1, method = "read", params = { kind = "track", limit = 60 } })
  mock.advance(10)
  mock.load_song(demo({ name = "Another" }))
  local response
  for _ = 1, 100 do
    mock.advance(10)
    for _, f in ipairs(H.frames(peer)) do if f.type == "response" and f.id == 1 then response = f end end
    if response then break end
  end
  ok(response, "answered")
  failure(response, "epoch-changed")
  mock.clock_step_ms = 0
  -- And the next read is of the new song, in a new epoch
  eq(rows(peer, "song")[1].name, "Another")
  H.no_dialogs()
end)

test("reads: a track inserted in the middle of a read ends it with changed", function()
  local env, peer = setup(nil, heavy({ tracks = 60, lines = 32 }))
  mock.clock_step_ms = 1
  H.send(peer, { type = "request", id = 1, method = "read", params = { kind = "track", limit = 60 } })
  mock.advance(10)
  mock.ops.insert_track(2, { name = "New" })
  local response
  for _ = 1, 100 do
    mock.advance(10)
    for _, f in ipairs(H.frames(peer)) do if f.type == "response" and f.id == 1 then response = f end end
    if response then break end
  end
  ok(response, "answered")
  failure(response, "changed")
  mock.clock_step_ms = 0
  H.no_dialogs()
end)

test("reads: a connection that goes away takes its read with it", function()
  local env, peer = setup(nil, heavy({ tracks = 60, lines = 32 }))
  local timers = #mock.timers
  mock.clock_step_ms = 1
  H.send(peer, { type = "request", id = 1, method = "read", params = { kind = "track", limit = 60 } })
  eq(#mock.timers, timers + 1)
  peer:close()
  for _ = 1, 20 do mock.advance(50) end
  eq(#mock.timers, timers, "no read left to run")
  mock.clock_step_ms = 0
  H.no_dialogs()
end)

test("reads: a reload ends the reads in progress, and the connection goes on", function()
  local env, peer = setup(nil, heavy({ tracks = 60, lines = 32 }))
  local other = H.connect(env)
  H.frames(other)
  mock.clock_step_ms = 1
  H.send(peer, { type = "request", id = 1, method = "read", params = { kind = "track", limit = 60 } })
  H.send(peer, { type = "request", id = 2, method = "read", params = { kind = "song" } })
  mock.advance(10)
  H.send(other, { type = "request", id = 3, method = "renoise.reload" })
  local got = {}
  for _ = 1, 100 do
    mock.advance(10)
    for _, f in ipairs(H.frames(peer)) do if f.type == "response" then got[f.id] = f end end
    if got[2] then break end
  end
  mock.clock_step_ms = 0
  ok(got[1], "the read was answered")
  failure(got[1], "reloading")
  ok(got[2], "and so was the read that waited behind it")
  failure(got[2], "reloading")
  eq(rows(peer, "song")[1].name, "Demo", "the new payload reads")
  H.no_dialogs()
end)

test("reads: a host that stops reading is held to the credit and gets its pages when it reads", function()
  local env, peer = setup(nil, heavy({ tracks = 1, lines = 300 }))
  fill_cells(1, 300)
  local E = epoch_of(peer)
  mock.socket_buffer_bytes = 600 * 1024
  peer:stop_reading()
  for id = 1, 6 do
    H.send(peer, { type = "request", id = id, method = "read", params = { kind = "line", parent = E .. ":pattern:1", track = 1, limit = 200 } })
  end
  for _ = 1, 100 do mock.advance(50) end
  ok(peer.unread <= 64 * 1024, "no more than the floor went out: " .. peer.unread)
  eq(mock.blocked_sends, 0, "nothing was sent into a full socket")
  ok(not H.retired(peer), "the host wasn't retired")
  peer:resume_reading()
  local answers = {}
  for _ = 1, 400 do
    drain(peer)
    for _, line in ipairs(peer:frames(nil)) do
      local frame = assert(json.decode(line))
      if frame.type == "response" then answers[frame.id] = frame end
    end
    local count = 0
    for _ in pairs(answers) do count = count + 1 end
    if count == 6 then break end
    mock.advance(10)
  end
  for id = 1, 6 do ok(answers[id] and answers[id].result, "answer " .. id) end
  H.no_dialogs()
end)

test("reads: a track inserted or removed is told with what was made and what is gone, and the epoch stays", function()
  local env, peer = setup()
  local before = epoch_of(peer)
  mock.ops.insert_track(2, { name = "New" })
  H.tick(2)
  local got = H.events(peer, "tracks")
  eq(#got, 1)
  eq(got[1].data.count, 8)
  eq(got[1].data.made[1], 2)
  eq(#got[1].data.gone, 0)
  eq(epoch_of(peer), before, "inserting a track doesn't move the epoch")
  mock.ops.remove_track(2)
  H.tick(2)
  got = H.events(peer, "tracks")
  eq(#got, 1)
  eq(got[1].data.count, 7)
  eq(#got[1].data.made, 0)
  eq(got[1].data.gone[1], 2)
  -- Two in one tick, in order
  mock.ops.insert_track(1, {})
  mock.ops.insert_track(1, {})
  H.tick(2)
  got = H.events(peer, "tracks")
  eq(#got, 2)
  eq(got[1].data.count, 8)
  eq(got[2].data.count, 9)
  eq(epoch_of(peer), before)
  H.no_dialogs()
end)

test("reads: a new track is watched like the others", function()
  local env, peer = setup()
  mock.ops.insert_track(2, { name = "New" })
  H.tick(2)
  local base = call(peer, "focus").result.changed
  mock.ops.rename_track(2, "Renamed")
  ok(call(peer, "focus").result.changed > base, "the new track's rename counts")
  H.no_dialogs()
end)

test("reads: what the tool can't map moves the epoch", function()
  local env, peer = setup()
  local function moved(change, what)
    local before = epoch_of(peer)
    change()
    H.tick(2)
    local after = epoch_of(peer)
    ok(after ~= before, what .. " moves the epoch")
    return before
  end
  local old = moved(function() mock.ops.swap_tracks(1, 2) end, "swapping tracks")
  moved(function() mock.ops.insert_pattern() end, "a new pattern")
  moved(function() mock.ops.insert_sequence_slot(1) end, "a new sequence slot")
  moved(function() mock.ops.insert_instrument({ name = "New" }) end, "a new instrument")
  moved(function() mock.ops.insert_sample(1) end, "a new sample")
  moved(function() mock.ops.insert_device(1, { name = "Gate", params = {} }) end, "a new device")
  failure(read(peer, "track", { cursor = old .. ".0.2" }), "stale")
  failure(read(peer, "device", { parent = old .. ":track:1" }), "stale")
  -- (and what it watched afresh still counts)
  local base = call(peer, "focus").result.changed
  mock.ops.set_note(1, 1, 1, 1, { note = 50 })
  ok(call(peer, "focus").result.changed > base)
  H.no_dialogs()
end)

test("reads: a song opened moves the epoch and the identity", function()
  local env, peer = setup()
  local before = epoch_of(peer)
  local first = rows(peer, "song")[1]
  mock.load_song(demo({ name = "Next", file_name = "/s/next.xrns" }))
  H.tick(2)
  ok(epoch_of(peer) ~= before)
  local next_song = rows(peer, "song")[1]
  eq(next_song.name, "Next")
  local function serial(row) return tonumber(row.identity:match("^4242:(%d+)$")) end
  eq(serial(next_song), serial(first) + 1, "another opening")
  ok(next_song.id ~= first.id, "and another song")
  H.no_dialogs()
end)

test("reads: 64 tracks and 100 patterns", function()
  local tracks = {}
  for i = 1, 64 do tracks[i] = { name = "T" .. i } end
  local env, peer = setup(nil, demo({ tracks = tracks, send_tracks = 2, patterns = 100, sequence_length = 100, sequence = nil }))
  local E = epoch_of(peer)
  local all = 0
  local cursor
  repeat
    local response, _, longest = call(peer, "read", { kind = "pattern", limit = 200, cursor = cursor })
    ok(longest <= MAX_LINE)
    all = all + #response.result.items
    cursor = response.result.nextCursor
  until not cursor
  eq(all, 100)
  local items = rows(peer, "track", { limit = 200 })
  eq(#items, 67)
  eq(#rows(peer, "sequence", { limit = 200 }), 100)
  local snapshot = call(peer, "snapshot").result
  eq(#snapshot.tracks, 67)
  H.no_dialogs()
end)

test("reads: a window of a sparse pattern isn't cut, however many cells it has", function()
  local tracks = {}
  for i = 1, 64 do tracks[i] = { name = "T" .. i, note_columns = 4, effect_columns = 1 } end
  local env, peer = setup(nil, demo({ tracks = tracks, send_tracks = 0, pattern_lines = 128 }))
  mock.ops.set_note(1, 3, 2, 1, { note = 48 })
  mock.lines_read = 0
  local body = call(peer, "focus_body", { kind = "pattern", window = { from = 1, to = 100 } }).result
  eq(body.window.from, 1)
  eq(body.window.to, 100)
  eq(body.truncated, nil)
  eq(body.tracks[3].lines[1].line, 2, "what is inside the window is there")
  eq(mock.lines_read, 65 * 100, "every line of the window, on every track and the master")
  H.no_dialogs()
end)

test("reads: a window of a dense pattern is read only as far as a page holds, line by line", function()
  local tracks = {}
  for i = 1, 64 do tracks[i] = { name = "T" .. i, note_columns = 4, effect_columns = 1 } end
  local env, peer = setup(nil, demo({ tracks = tracks, send_tracks = 0, pattern_lines = 128 }))
  for line = 1, 100 do
    for t = 1, 64 do
      for column = 1, 4 do
        mock.ops.set_note(1, t, line, column, { note = 48, instrument = 1, volume = 64, panning = 40, delay = 16, fxn = 10, fxa = 32 })
      end
    end
  end
  mock.lines_read = 0
  local response = call(peer, "focus_body", { kind = "pattern", window = { from = 1, to = 100 } })
  local body = response.result
  eq(body.truncated, true)
  ok(body.window.to >= 1 and body.window.to < 100, "the window was cut: " .. body.window.to)
  ok(#json.encode(response) <= MAX_LINE, "and it fits a page")
  eq(#body.tracks[64].lines, body.window.to, "every line of the window is there on the last track too")
  ok(mock.lines_read <= 65 * (body.window.to + 1), "and no line was read past the one that filled the page: " .. mock.lines_read)
  H.no_dialogs()
end)

test("reads: with the timers stopped (a modal dialog) a read goes on with the messages that still come", function()
  local env, peer = setup(nil, heavy({ tracks = 60, lines = 32 }))
  mock.clock_step_ms = 1
  mock.stall_timers = true
  H.send(peer, { type = "request", id = 1, method = "read", params = { kind = "track", limit = 60 } })
  local done
  for _ = 1, 200 do
    mock.advance(120) -- the clock moves, no timer runs
    H.send(peer, { type = "request", id = 2, method = "ping" })
    for _, f in ipairs(H.frames(peer)) do
      if f.type == "response" and f.id == 1 then done = f end
    end
    if done then break end
  end
  mock.stall_timers = false
  mock.clock_step_ms = 0
  ok(done and done.result, "the read finished on messages alone")
  eq(#done.result.items, 60)
  H.no_dialogs()
end)

-- Kumi shortens refs by key name: a ref under any other key would reach the model long.
local function stray_refs(value, key, path, found)
  if type(value) == "string" then
    if value:match("^%x+:%a+:[%d%.]+$") then
      local allowed = key == "ref" or key == "parent" or (key and (key:match("Ref$") or key:match("Refs$")))
      if not allowed then found[#found + 1] = path .. " = " .. value end
    end
  elseif type(value) == "table" then
    for k, v in pairs(value) do
      -- (the elements of a list are under the list's key)
      local child_key = type(k) == "number" and key or k
      stray_refs(v, child_key, path .. "." .. tostring(k), found)
    end
  end
  return found
end

test("reads: a ref is only ever under ref, parent, a Ref key or a Refs key", function()
  local env, peer = setup()
  fill_demo()
  mock.ops.select_sample(2)
  mock.ops.select_phrase(1)
  mock.ops.select_device(2)
  local E = epoch_of(peer)
  local answers = {
    song = call(peer, "read", { kind = "song" }),
    track = call(peer, "read", { kind = "track" }),
    sequence = call(peer, "read", { kind = "sequence" }),
    pattern = call(peer, "read", { kind = "pattern" }),
    line = call(peer, "read", { kind = "line", parent = E .. ":pattern:1", track = 1 }),
    instrument = call(peer, "read", { kind = "instrument" }),
    sample = call(peer, "read", { kind = "sample", parent = E .. ":instrument:1" }),
    phrase = call(peer, "read", { kind = "phrase", parent = E .. ":instrument:1" }),
    device = call(peer, "read", { kind = "device", parent = E .. ":track:1" }),
    param = call(peer, "read", { kind = "param", parent = E .. ":device:1.1" }),
    paged = call(peer, "read", { kind = "track", limit = 2 }),
    snapshot = call(peer, "snapshot"),
    focus = call(peer, "focus"),
    pattern_body = call(peer, "focus_body", { kind = "pattern" }),
    matrix_body = call(peer, "focus_body", { kind = "matrix" }),
    phrase_body = call(peer, "focus_body", { kind = "phrase", ref = E .. ":phrase:1.1" }),
    instrument_body = call(peer, "focus_body", { kind = "instrument", ref = E .. ":instrument:1" }),
    dsp_body = call(peer, "focus_body", { kind = "dsp", ref = E .. ":track:1" }),
  }
  local refs_seen = 0
  for name, answer in pairs(answers) do
    ok(answer.result, name .. " failed: " .. json.encode(answer.error or {}))
    local found = stray_refs(answer.result, nil, name, {})
    eq(#found, 0, "refs under other keys: " .. table.concat(found, "; "))
    local text = json.encode(answer.result)
    for _ in text:gmatch(E .. ":%a+:[%d%.]+") do refs_seen = refs_seen + 1 end
  end
  ok(refs_seen > 50, "the walk saw refs: " .. refs_seen)
  H.no_dialogs()
end)

test("reads: a ref reads the one object it names", function()
  local env, peer = setup()
  fill_demo()
  local E = epoch_of(peer)
  local function one(ref)
    local response = call(peer, "read", { ref = ref })
    ok(response.result, ref .. ": " .. json.encode(response.error or {}))
    eq(#response.result.items, 1)
    eq(response.result.epoch, E)
    return response.result.items[1]
  end
  eq(one(E .. ":track:3").name, "Drums")
  eq(one(E .. ":track:3").groupRef, E .. ":track:5")
  eq(one(E .. ":seq:3").patternRef, E .. ":pattern:1")
  eq(one(E .. ":pattern:2").lines, 32)
  eq(one(E .. ":instrument:2").name, "Pad")
  eq(one(E .. ":sample:1.2").name, "Sample 2")
  eq(one(E .. ":phrase:1.1").lpb, 4)
  eq(one(E .. ":device:1.1").name, "EQ 5")
  eq(one(E .. ":param:1.1.2").name, "Freq")
  failure(call(peer, "read", { ref = E .. ":track:99" }), "bad-params")
  failure(call(peer, "read", { ref = E .. ":param:1.1.9" }), "bad-params")
  failure(call(peer, "read", { ref = "0:track:1" }), "stale")
  failure(call(peer, "read", { ref = "r:track:{GUID}" }), "bad-params")
  failure(call(peer, "read", { ref = E .. ":line:1" }), "bad-params")
  H.no_dialogs()
end)

test("reads: a read whose deadline passes is ended, not finished for nobody", function()
  local env, peer = setup(nil, heavy({ tracks = 60, lines = 32 }))
  mock.clock_step_ms = 1
  H.send(peer, { type = "request", id = 1, method = "read", params = { kind = "track", limit = 60 }, deadlineMs = os.time() * 1000 + 25 })
  local response
  for _ = 1, 100 do
    mock.advance(10)
    for _, f in ipairs(H.frames(peer)) do if f.type == "response" and f.id == 1 then response = f end end
    if response then break end
  end
  mock.clock_step_ms = 0
  ok(response, "answered")
  failure(response, "deadline")
  H.no_dialogs()
end)

-- What the tool answers to a fixed set of requests over the demo song, kept in a file the host's simulator replays
-- (daws/own/daw-mcp-server/tests/vectors/renoise-reads.json), so the simulator can't drift from the tool. KUMI_BLESS=1
-- writes the file; without it a difference fails the test.
local function pretty(value, indent)
  indent = indent or ""
  if value == json.null then return "null" end
  if type(value) ~= "table" then return json.encode(value) end
  local inner = indent .. "  "
  local parts = {}
  if json.is_array(value) then
    if #value == 0 then return "[]" end
    for _, item in ipairs(value) do parts[#parts + 1] = inner .. pretty(item, inner) end
    return "[\n" .. table.concat(parts, ",\n") .. "\n" .. indent .. "]"
  end
  local keys = {}
  for key in pairs(value) do keys[#keys + 1] = key end
  if #keys == 0 then return "{}" end
  table.sort(keys)
  for _, key in ipairs(keys) do parts[#parts + 1] = inner .. json.encode(key) .. ": " .. pretty(value[key], inner) end
  return "{\n" .. table.concat(parts, ",\n") .. "\n" .. indent .. "}"
end

test("reads: the answers are the vectors the host's simulator replays", function()
  local env, peer = setup()
  fill_demo()
  mock.ops.select_sample(2)
  mock.ops.select_phrase(1)
  mock.ops.select_device(2)
  local E = epoch_of(peer)
  local cases = {}
  local function ask(method, params)
    local response = call(peer, method, params)
    ok(response, method .. " was not answered")
    cases[#cases + 1] = { request = { method = method, params = params }, response = response.ok
      and { ok = true, result = response.result } or { ok = false, error = response.error } }
    return response
  end
  ask("read", { kind = "song" })
  local first = ask("read", { kind = "track", limit = 2 })
  ask("read", { kind = "track", limit = 2, cursor = first.result.nextCursor })
  ask("read", { kind = "track", limit = 200 })
  ask("read", { kind = "sequence" })
  ask("read", { kind = "pattern" })
  ask("read", { kind = "line", parent = E .. ":pattern:1", track = 1 })
  ask("read", { kind = "line", parent = E .. ":pattern:1", track = 1, limit = 1 })
  ask("read", { kind = "line", parent = E .. ":pattern:1", track = E .. ":track:1" }) -- the shape Kumi sends: a ref'd track
  ask("read", { kind = "line", parent = E .. ":pattern:1", track = E .. ":track:4", layout = 0 })
  ask("read", { kind = "instrument" })
  ask("read", { kind = "sample", parent = E .. ":instrument:1" })
  ask("read", { kind = "phrase", parent = E .. ":instrument:1" })
  -- (n-instruments: an instrument's modulation sets and sample FX chains)
  ask("read", { kind = "modset", parent = E .. ":instrument:1" })
  ask("read", { kind = "chain", parent = E .. ":instrument:1" })
  ask("read", { kind = "device", parent = E .. ":track:1" })
  ask("read", { kind = "param", parent = E .. ":device:1.1" })
  ask("read", { ref = E .. ":track:3" })
  ask("snapshot", {})
  ask("focus", {})
  ask("focus_body", { kind = "pattern" })
  ask("focus_body", { kind = "pattern", ref = E .. ":pattern:1" })
  ask("focus_body", { kind = "pattern", window = { from = 3, to = 6 } })
  ask("focus_body", { kind = "matrix" })
  ask("focus_body", { kind = "phrase", ref = E .. ":phrase:1.1" })
  ask("focus_body", { kind = "instrument", ref = E .. ":instrument:1" })
  ask("focus_body", { kind = "dsp", ref = E .. ":track:1" })
  -- M6 (m6-catchup): the song as records, for catch-up and for watch_me
  ask("digest", {})
  ask("digest", { values = true })
  ask("read", { kind = "nonsense" })
  ask("read", { kind = "line", track = 1 })
  ask("read", { ref = "0:track:1" })
  ask("read", { kind = "track", cursor = "0.0.2" })
  ask("focus_body", { kind = "pattern", window = { from = 5, to = 2 } })
  H.no_dialogs()
  local made = { epoch = E, cases = json.array(cases) }
  local path = T.tests_dir .. "../../own/daw-mcp-server/tests/vectors/renoise-reads.json"
  if os.getenv("KUMI_BLESS") == "1" then
    assert(os.execute("mkdir -p '" .. path:match("^(.*)/[^/]*$") .. "'") == 0)
    local file = assert(io.open(path, "wb"))
    file:write(pretty(made), "\n")
    file:close()
    return
  end
  local file = assert(io.open(path, "rb"), "no vectors at " .. path .. ": run with KUMI_BLESS=1")
  local kept = assert(json.decode(file:read("*a")))
  file:close()
  T.same(made, kept)
end)

-- No tick between a change and the next request: every answer after a change is in what the change left.

test("reads: a change to what refs index into moves the epoch at once, before any tick", function()
  local changes = {
    { "an instrument deleted", function() mock.ops.remove_instrument(1) end, "instrument:1" },
    { "an instrument made", function() mock.ops.insert_instrument({ name = "New" }) end, "instrument:1" },
    { "a pattern deleted", function() mock.ops.remove_pattern(1) end, "pattern:1" },
    { "a pattern made", function() mock.ops.insert_pattern() end, "pattern:1" },
    { "a sequence slot inserted", function() mock.ops.insert_sequence_slot(1) end, "seq:1" },
    { "a sample deleted", function() mock.ops.remove_sample(1, 1) end, "sample:1.1" },
    { "a sample made", function() mock.ops.insert_sample(1) end, "sample:1.1" },
    { "a device deleted", function() mock.ops.remove_device(1, 1) end, "device:1.1" },
    { "a device made", function() mock.ops.insert_device(1, { name = "Gate", params = {} }) end, "device:1.1" },
    { "two tracks swapped", function() mock.ops.swap_tracks(1, 2) end, "track:1", 1 },
  }
  for _, change in ipairs(changes) do
    local env, peer = setup()
    fill_demo()
    local E = epoch_of(peer)
    change[2]()
    -- (no tick)
    local now = call(peer, "focus").result
    ok(now.epoch ~= E, change[1] .. ": focus is in the new epoch at once")
    eq(now.layout, change[4] or 0, change[1] .. ": the layout moves only for the tracks (a swap is one shift)")
    local response = call(peer, "read", { ref = E .. ":" .. change[3] })
    failure(response, "stale")
    -- and what is read now is named in the new epoch
    local fresh = call(peer, "read", { kind = "track", limit = 1 }).result
    eq(fresh.epoch, now.epoch, change[1])
    H.no_dialogs()
  end
end)

test("reads: a track inserted is told at once, and a ref made before reads the same track when its layout is named", function()
  local env, peer = setup()
  fill_demo()
  local E = epoch_of(peer)
  local f = call(peer, "focus").result
  eq(f.layout, 0)
  mock.ops.insert_track(1, { name = "New" })
  -- (no tick)
  local told = H.events(peer, "tracks")
  eq(#told, 1, "the shift is on the wire before any tick")
  eq(told[1].data.count, 8)
  eq(told[1].data.made[1], 1)
  eq(#told[1].data.gone, 0)
  eq(told[1].data.layout, 1)
  eq(told[1].data.epoch, E, "a shift keeps the epoch")
  -- Hat was track 2 when the ref was made
  local carried = call(peer, "read", { ref = E .. ":track:2", layout = 0 })
  ok(carried.result, json.encode(carried.error or {}))
  eq(carried.result.items[1].name, "Hat", "the same track")
  eq(carried.result.items[1].ref, E .. ":track:3", "at its place now")
  eq(carried.result.layout, 1, "and the answer says which layout it is in")
  -- the layout of now reads what is at the number
  eq(call(peer, "read", { ref = E .. ":track:2", layout = 1 }).result.items[1].name, "Kick")
  eq(call(peer, "read", { ref = E .. ":track:2" }).result.items[1].name, "Kick", "no layout: made in the layout of now")
  -- a parent, a device and a number under `track` are carried too
  local devices = call(peer, "read", { kind = "device", parent = E .. ":track:1", layout = 0 }).result
  eq(#devices.items, 2, "Kick's devices")
  eq(devices.items[1].ref, E .. ":device:2.1", "on the track it moved to")
  local lines = call(peer, "read", { kind = "line", parent = E .. ":pattern:1", track = 1, layout = 0 }).result
  eq(lines.items[2].notes[1].note, "D#4", "Kick's lines")
  local param = call(peer, "read", { ref = E .. ":param:1.1.2", layout = 0 }).result
  eq(param.items[1].ref, E .. ":param:2.1.2")
  eq(call(peer, "focus_body", { kind = "dsp", ref = E .. ":track:1", layout = 0 }).result.nodes[1].ref, E .. ":device:2.1")
  -- a ref to something that isn't a track is left as it is
  eq(call(peer, "read", { ref = E .. ":instrument:2", layout = 0 }).result.items[1].name, "Pad")
  H.no_dialogs()
end)

test("reads: an edit that moves the epoch (a device added) after a track insert leaves every request without track refs readable", function()
  local env, peer = setup()
  fill_demo()
  local E = epoch_of(peer)
  mock.ops.insert_track(1, { name = "New" })
  eq(H.events(peer, "tracks")[1].data.layout, 1)
  -- A device added: the epoch moves and no event says so. The layout goes on, not back to 0.
  mock.ops.insert_device(2, { path = "Audio/Effects/Native/EQ 5" })
  local now = call(peer, "focus").result
  ok(now.epoch ~= E, "the epoch moved")
  eq(now.layout, 1, "the layout goes on counting")
  -- Requests that name no track answer, whatever layout they carry (Kumi's last one was 1).
  ok(call(peer, "read", { kind = "song", layout = 1 }).result, "read song")
  ok(call(peer, "read", { kind = "pattern", layout = 1 }).result, "read pattern")
  ok(call(peer, "focus_body", { kind = "pattern", layout = 1 }).result, "the pattern body")
  ok(call(peer, "focus_body", { kind = "matrix", layout = 0 }).result, "the matrix, even with an older layout")
  -- A track ref from before the device was added still names that track (the track numbers are where they were; layout 1
  -- is the numbering the ref was made in), and a device ref from then is stale: the devices moved.
  local old = call(peer, "read", { ref = E .. ":track:2", layout = 1 })
  ok(old.result and old.result.items[1], json.encode(old))
  local device = call(peer, "read", { ref = E .. ":device:2.1", layout = 1 })
  eq(device.error and device.error.code, "stale", json.encode(device))
  H.no_dialogs()
end)

test("reads: a track removed answers gone for its refs, and the tracks after it move down", function()
  local env, peer = setup()
  fill_demo()
  local E = epoch_of(peer)
  mock.ops.remove_track(2) -- Hat
  local told = H.events(peer, "tracks")
  eq(#told, 1)
  eq(told[1].data.gone[1], 2)
  eq(told[1].data.layout, 1)
  failure(call(peer, "read", { ref = E .. ":track:2", layout = 0 }), "gone")
  failure(call(peer, "read", { kind = "device", parent = E .. ":track:2", layout = 0 }), "gone")
  failure(call(peer, "read", { kind = "line", parent = E .. ":pattern:1", track = 2, layout = 0 }), "gone")
  local after = call(peer, "read", { ref = E .. ":track:3", layout = 0 })
  ok(after.result, json.encode(after.error or {}))
  local moved = after.result
  eq(moved.items[1].name, "Drums")
  eq(moved.items[1].ref, E .. ":track:2")
  eq(call(peer, "read", { ref = E .. ":track:1", layout = 0 }).result.items[1].name, "Kick", "before the removal: unmoved")
  H.no_dialogs()
end)

-- (the tool keeps 1024 shifts; this test keeps 64 of them, to make more than it keeps)
local function shift_history(n)
  return function(env)
    local path = env.bundle .. "payload/bridge.lua"
    local f = assert(io.open(path, "rb"))
    local text = f:read("*a")
    f:close()
    local lowered, count = text:gsub("M%.SHIFT_HISTORY = %d+", "M.SHIFT_HISTORY = " .. n, 1)
    assert(count == 1, "SHIFT_HISTORY was not found to set")
    f = assert(io.open(path, "wb"))
    f:write(lowered)
    f:close()
  end
end

test("reads: shifts are carried one after another, up to the last 64", function()
  local env, peer = setup({ patch = shift_history(64) })
  local E = epoch_of(peer)
  mock.ops.insert_track(1, { name = "A" })
  mock.ops.insert_track(1, { name = "B" })
  mock.ops.remove_track(1)
  mock.ops.insert_track(4, { name = "C" })
  H.events(peer, "tracks")
  -- Kick was 1: B, A, Kick -> A, Kick after the removal -> A, Kick, Hat, C
  local kick = call(peer, "read", { ref = E .. ":track:1", layout = 0 }).result
  eq(kick.items[1].name, "Kick")
  eq(kick.items[1].ref, E .. ":track:2")
  eq(kick.layout, 4)
  for _ = 1, 62 do mock.ops.insert_track(1, { name = "x" }) end -- 66 shifts in all
  H.events(peer, "tracks")
  failure(call(peer, "read", { ref = E .. ":track:1", layout = 0 }), "stale")
  failure(call(peer, "read", { ref = E .. ":track:1", layout = 1 }), "stale")
  -- (track 3 at layout 2 was Kick: B and A were above it, and the removal and 62 inserts came after)
  local recent = call(peer, "read", { ref = E .. ":track:3", layout = 2 })
  ok(recent.result, "64 shifts back is still carried: " .. json.encode(recent.error or {}))
  eq(recent.result.items[1].name, "Kick")
  eq(recent.result.items[1].ref, E .. ":track:64")
  failure(call(peer, "read", { ref = E .. ":track:1", layout = 500 }), "stale")
  failure(call(peer, "read", { ref = E .. ":track:1", layout = -1 }), "bad-params")
  failure(call(peer, "read", { ref = E .. ":track:1", layout = 1.5 }), "bad-params")
  H.no_dialogs()
end)

test("reads: a cursor made before a track was inserted or removed is refused, never read against the shifted list", function()
  local env, peer = setup()
  local first = call(peer, "read", { kind = "track", limit = 2 }).result
  eq(first.nextCursor, first.epoch .. ".0.3")
  mock.ops.insert_track(1, { name = "New" })
  failure(call(peer, "read", { kind = "track", limit = 2, cursor = first.nextCursor }), "stale")
  local again = call(peer, "read", { kind = "track", limit = 2 }).result
  eq(again.nextCursor, again.epoch .. ".1.3", "made in the layout of now")
  eq(#call(peer, "read", { kind = "track", limit = 2, cursor = again.nextCursor }).result.items, 2)
  H.no_dialogs()
end)

test("reads: a read waiting for its first turn is ended if the song changed before it", function()
  local env, peer = setup()
  fill_demo()
  local E = epoch_of(peer)
  H.send(peer, { type = "request", id = 900, method = "read", params = { ref = E .. ":instrument:1" } })
  mock.ops.remove_instrument(1) -- before the timer has run the job
  local response
  for _ = 1, 50 do
    mock.advance(10)
    for _, f in ipairs(H.frames(peer)) do if f.type == "response" and f.id == 900 then response = f end end
    if response then break end
  end
  ok(response, "answered")
  failure(response, "epoch-changed")
  -- and the same for a track inserted
  H.send(peer, { type = "request", id = 901, method = "read", params = { kind = "track" } })
  mock.ops.insert_track(1, { name = "New" })
  response = nil
  for _ = 1, 50 do
    mock.advance(10)
    for _, f in ipairs(H.frames(peer)) do if f.type == "response" and f.id == 901 then response = f end end
    if response then break end
  end
  failure(response, "changed")
  H.no_dialogs()
end)

test("reads: tracks that change in a way the tool can't follow move the epoch and say so", function()
  local env, peer = setup()
  local E = epoch_of(peer)
  -- an insert is announced but the count didn't move by one
  mock.fire(mock.song.tracks_observable, { type = "insert", index = 1 })
  local told = H.events(peer, "tracks")
  eq(#told, 1)
  eq(#told[1].data.made + #told[1].data.gone, 0, "no shift to carry")
  ok(told[1].data.epoch ~= E, "the epoch moved")
  -- (and the layout moved past what the shifts cover: a request in the layout before has no track it can be carried to)
  eq(told[1].data.layout, 1)
  failure(call(peer, "read", { ref = told[1].data.epoch .. ":track:1", layout = 0 }), "stale")
  mock.ops.swap_tracks(1, 2)
  told = H.events(peer, "tracks")
  eq(#told, 1)
  ok(told[1].data.epoch ~= E)
  H.no_dialogs()
end)

test("reads: new tracks, patterns and instruments are watched where they are made, with no tick", function()
  local env, peer = setup()
  local base = call(peer, "focus").result.changed
  mock.ops.insert_track(1, { name = "New" })
  mock.ops.rename_track(1, "Renamed")
  ok(call(peer, "focus").result.changed > base + 1, "the new track's rename counts at once")
  local n = mock.ops.insert_pattern()
  base = call(peer, "focus").result.changed
  mock.ops.set_note(n, 1, 1, 1, { note = 48 })
  ok(call(peer, "focus").result.changed > base, "a line of the new pattern counts at once")
  mock.ops.insert_instrument({ name = "New" })
  base = call(peer, "focus").result.changed
  mock.ops.rename_instrument(#mock.data.instruments, "Renamed")
  ok(call(peer, "focus").result.changed > base, "the new instrument's rename counts at once")
  H.no_dialogs()
end)

test("reads: what the bodies show is watched: each of these moves `changed`, and none moves the epoch", function()
  local env, peer = setup()
  fill_demo()
  H.tick(2)
  local E = epoch_of(peer)
  local function moves(what, change)
    local base = call(peer, "focus").result.changed
    change()
    ok(call(peer, "focus").result.changed > base, what .. " counts")
    eq(epoch_of(peer), E, what .. " leaves the epoch")
  end
  moves("a muted column", function() mock.ops.mute_column(1, 2, true) end)
  moves("a named column", function() mock.ops.name_column(1, 2, "Pad") end)
  mock.ops.insert_device(1, { name = "Gainer", params = {} })
  E = epoch_of(peer)
  moves("a device switched off", function() mock.ops.set_device(1, 3, "active", false) end)
  moves("a device renamed", function() mock.ops.set_device(1, 3, "display_name", "Warm EQ") end)
  moves("a macro", function() mock.ops.set_macro(1, 1, 0.3) end)
  moves("a section", function() mock.ops.section(2, "Verse") end)
  moves("an alias", function() mock.ops.alias(3, 1, 1) end)
  moves("a keyzone's note range", function() mock.ops.set_mapping(1, 1, "note_range", { 10, 20 }) end)
  moves("a keyzone's velocity range", function() mock.ops.set_mapping(1, 1, "velocity_range", { 5, 100 }) end)
  moves("a keyzone's base note", function() mock.ops.set_mapping(1, 1, "base_note", 40) end)
  moves("a keyzone's layer", function() mock.ops.set_mapping(1, 1, "layer", 2) end)
  moves("a keyzone's key mapping", function() mock.ops.set_mapping(1, 1, "map_key_to_pitch", false) end)
  moves("a keyzone's velocity mapping", function() mock.ops.set_mapping(1, 1, "map_velocity_to_volume", false) end)
  moves("a phrase's line", function() mock.ops.set_phrase_note(1, 1, 2, 1, { note = 50 }) end)
  moves("a phrase's length", function() mock.ops.set_phrase(1, 1, "number_of_lines", 24) end)
  moves("a phrase's lpb", function() mock.ops.set_phrase(1, 1, "lpb", 8) end)
  H.no_dialogs()
end)

test("reads: a device, sample or phrase made later is watched too, with no tick and no new watching of the song", function()
  local env, peer = setup()
  mock.ops.insert_device(1, { name = "EQ 5", params = {} }) -- (made the way Renoise makes one, not poked into the song)
  H.tick(2)
  local attached = env.payload().watching()
  mock.ops.insert_device(1, { name = "Gainer", params = {} })
  mock.ops.insert_sample(1, {})
  mock.ops.insert_phrase(1)
  local made = env.payload().watching()
  ok(made > attached and made - attached < 40, "only the new objects were watched: " .. (made - attached))
  local function moves(what, change)
    local base = call(peer, "focus").result.changed
    change()
    ok(call(peer, "focus").result.changed > base, what .. " counts")
  end
  local device = #mock.data.tracks[1].devices
  moves("the new device's switch", function() mock.ops.set_device(1, device, "active", false) end)
  moves("the new sample's range", function() mock.ops.set_mapping(1, 3, "note_range", { 1, 2 }) end)
  moves("the new phrase's line", function() mock.ops.set_phrase_note(1, 2, 1, 1, { note = 40 }) end)
  H.tick(3)
  eq(env.payload().watching(), made, "and a tick watched nothing anew")
  H.no_dialogs()
end)

test("reads: an aliased track says whose lines it shows, on a line page and in a pattern body", function()
  local env, peer = setup()
  fill_demo() -- track 2 of pattern 2 is an alias of pattern 1; pattern 1's track 1 has notes
  mock.ops.set_note(1, 2, 4, 1, { note = 55, instrument = 0 })
  local E = epoch_of(peer)
  -- the lines of the alias are the source's (as Renoise answers)
  local page = call(peer, "read", { kind = "line", parent = E .. ":pattern:2", track = 2 }).result
  eq(page.aliasOf, 0, "pattern 00, as Renoise numbers it")
  eq(page.aliasRef, E .. ":pattern:1")
  eq(page.items[1].line, 3)
  eq(page.items[1].notes[1].note, "G-4")
  local own = call(peer, "read", { kind = "line", parent = E .. ":pattern:1", track = 2 }).result
  eq(own.aliasOf, nil, "a track that isn't an alias says nothing")
  eq(own.aliasRef, nil)
  local body = call(peer, "focus_body", { kind = "pattern", ref = E .. ":pattern:2", window = { from = 1, to = 8 } }).result
  eq(body.tracks[2].aliasOf, 1)
  eq(body.tracks[2].aliasRef, E .. ":pattern:1")
  eq(#body.tracks[2].lines, 1, "and the lines in the body are pattern 1's")
  eq(body.tracks[1].aliasOf, nil)
  -- a note written through the alias is the source's
  mock.ops.set_note(2, 2, 6, 1, { note = 57, instrument = 0 })
  local source = call(peer, "read", { kind = "line", parent = E .. ":pattern:1", track = 2 }).result
  eq(source.items[2].line, 5)
  H.no_dialogs()
end)

test("reads: data in a column the track doesn't show is not read as an empty line", function()
  local env, peer = setup()
  local E = epoch_of(peer)
  -- track 1 shows one note column and one effect column
  mock.ops.set_note(1, 1, 4, 3, { note = 52, instrument = 0 })
  mock.ops.set_note(1, 1, 6, 1, { note = 48, instrument = 0 })
  mock.ops.set_note(1, 1, 6, 5, { note = 55, instrument = 0 })
  mock.ops.set_effect(1, 1, 8, 4, 10, 32)
  local page = call(peer, "read", { kind = "line", parent = E .. ":pattern:1", track = 1 }).result
  eq(#page.items, 3, "the lines with hidden data are listed")
  eq(page.items[1].line, 3)
  eq(page.items[1].notes, nil, "nothing is in the shown columns")
  eq(page.items[1].hidden.notes[1].col, 3)
  eq(page.items[1].hidden.notes[1].note, "E-4")
  eq(page.items[2].line, 5)
  eq(page.items[2].notes[1].note, "C-4")
  eq(page.items[2].hidden, nil, "a line with something shown isn't looked through unless asked")
  eq(page.items[3].line, 7)
  eq(page.items[3].hidden.effects[1].col, 4)
  eq(page.items[3].hidden.effects[1].number, "0A")
  -- asked to, every line is
  local all = call(peer, "read", { kind = "line", parent = E .. ":pattern:1", track = 1, hidden = true }).result
  eq(all.items[2].hidden.notes[1].col, 5)
  eq(all.items[2].hidden.notes[1].note, "G-4")
  -- and so is the body
  local body = call(peer, "focus_body", { kind = "pattern", window = { from = 1, to = 8 } }).result
  eq(body.tracks[1].lines[1].hidden.notes[1].col, 3)
  eq(body.tracks[1].lines[2].hidden, nil)
  body = call(peer, "focus_body", { kind = "pattern", window = { from = 1, to = 8 }, hidden = true }).result
  eq(body.tracks[1].lines[2].hidden.notes[1].col, 5)
  H.no_dialogs()
end)

test("reads: a track row says the pre-FX and the post-FX fader and pan apart", function()
  local env, peer = setup()
  fill_demo() -- track 2: pre-FX volume 0.5, pan 1
  mock.data.tracks[2].post_volume, mock.data.tracks[2].post_panning, mock.data.tracks[2].width = 0.25, 0.25, 50
  local hat = rows(peer, "track")[2]
  near(hat.preVolumeDb, -6.0206)
  eq(hat.prePan, 1)
  eq(hat.preWidth, 50)
  near(hat.postVolumeDb, -12.0412)
  eq(hat.postPan, -0.5)
  eq(hat.volumeDb, hat.preVolumeDb, "volumeDb and pan stay the pre-FX ones")
  eq(hat.pan, hat.prePan)
  local kick = rows(peer, "track")[1]
  eq(kick.postVolumeDb, 0)
  eq(kick.postPan, 0)
  eq(kick.preWidth, 0, "Renoise's own default: no widening")
  H.no_dialogs()
end)

test("reads: a pattern, a track or an instrument made later is the only thing watched anew", function()
  local env, peer = setup()
  fill_demo()
  H.tick(2)
  local total = env.payload().watching()
  ok(total > 150, "a song of this size is watched through many things: " .. total)
  local before = env.payload().watching()
  mock.ops.insert_pattern()
  local pattern = env.payload().watching() - before
  ok(pattern > 0 and pattern < 30, "a pattern: its lines, name, length and one alias per track: " .. pattern)
  before = env.payload().watching()
  mock.ops.insert_track(1, { name = "New" })
  local track = env.payload().watching() - before
  ok(track > 0 and track < 60, "a track: its own, and one alias per pattern: " .. track)
  before = env.payload().watching()
  mock.ops.insert_instrument({ name = "New", samples = 2 })
  local instrument = env.payload().watching() - before
  ok(instrument > 0 and instrument < 60, "an instrument: its name, macros and samples: " .. instrument)
  mock.ops.remove_device(1, 1)
  mock.ops.remove_pattern(1)
  H.tick(3)
  ok(env.payload().watching() >= total, "removals leave their watchers until enough pile up")
  H.no_dialogs()
end)

test("reads: a malformed ref, cursor or window is bad-params, never an internal error", function()
  local env, peer = setup()
  local E = epoch_of(peer)
  local function bad(method, params, what)
    local response = call(peer, method, params)
    ok(response and response.error, what .. ": an error")
    eq(response.error.code, "bad-params", what)
    ok(not tostring(response.error.message):find("%.lua"), what .. ": no file and line in the message: " .. tostring(response.error.message))
  end
  bad("read", { ref = E .. ":track:0" }, "track 0")
  bad("read", { ref = E .. ":sample:1.0" }, "sample 1.0")
  bad("read", { ref = E .. ":device:1..1" }, "stray dots")
  bad("read", { ref = E .. ":param:1.1.1." }, "a dot at the end")
  bad("read", { ref = E .. ":param:.1.1" }, "a dot at the start")
  bad("read", { ref = E .. ":track:03" }, "a zero in front")
  bad("read", { kind = "track", cursor = E .. ".0.0" }, "cursor position 0")
  bad("read", { kind = "line", parent = E .. ":pattern:1", track = E .. ":track:0" }, "a track ref of 0")
  bad("focus_body", { kind = "pattern", window = { from = 3 } }, "a window with only from")
  bad("focus_body", { kind = "pattern", window = { to = 3 } }, "a window with only to")
  bad("focus_body", { kind = "pattern", window = 7 }, "a window that isn't a table")
  -- a window of one line is a window
  local body = call(peer, "focus_body", { kind = "pattern", window = { from = 3, to = 3 } }).result
  eq(body.window.from, 3)
  eq(body.window.to, 3)
  H.no_dialogs()
end)

test("reads: an internal error tells the host what failed, not the file and line", function()
  local env, peer = setup()
  env.payload().handlers["test.boom"] = function() error("track index out of range: 0") end
  local response = call(peer, "test.boom")
  eq(response.error.code, "internal")
  eq(response.error.message, "track index out of range: 0")
  H.no_dialogs()
end)

test("reads: a matrix too big for a page keeps the slots around the one playing, and says where they start", function()
  local env, peer = setup(nil, demo({
    tracks = false, sequencer_tracks = 60, patterns = 2, sequence_length = 800, sequence = false,
  }))
  for s = 1, 800 do
    for t = 1, 60 do mock.ops.mute_slot(t, s, true) end
  end
  for t = 1, 60 do mock.ops.alias(2, t, 1) end
  mock.ops.select("selected_sequence_index", 600)
  local matrix = call(peer, "focus_body", { kind = "matrix" }).result
  ok(matrix.truncated, "cut to a page")
  eq(matrix.slotCount, 800)
  local first, last = matrix.from, matrix.from + #matrix.slots - 1
  ok(first <= 600 and 600 <= last, "slot 600 is in the window: " .. first .. " to " .. last)
  eq(matrix.current, 600)
  eq(matrix.slots[1].ref:match(":seq:(%d+)$"), tostring(first), "the first slot is the one `from` says")
  H.no_dialogs()
  -- an uncut matrix has no `from`
  local _, small_peer = setup()
  local small = call(small_peer, "focus_body", { kind = "matrix" }).result
  eq(small.from, nil)
  eq(small.truncated, nil)
  H.no_dialogs()
end)

test("reads: a snapshot, an instrument body and a chain are cut to a page, and say so", function()
  local long = string.rep("n", 2000)
  local tracks = {}
  for i = 1, 150 do tracks[i] = { name = long } end
  local env, peer = setup(nil, demo({ tracks = tracks, send_tracks = 0 }))
  local snapshot = call(peer, "snapshot")
  ok(#json.encode(snapshot) <= MAX_LINE, "the snapshot fits a page")
  eq(snapshot.result.moreTracks, true)
  ok(#snapshot.result.tracks < 150 and #snapshot.result.tracks >= 1, "and lists fewer tracks: " .. #snapshot.result.tracks)
  local E = snapshot.result.epoch
  -- a chain of 400 devices with long names
  for d = 1, 400 do mock.ops.insert_device(1, { name = "Gainer", display_name = long, params = {} }) end
  local chain = call(peer, "focus_body", { kind = "dsp", ref = E .. ":track:1" })
  E = epoch_of(peer)
  chain = call(peer, "focus_body", { kind = "dsp", ref = E .. ":track:1" })
  ok(#json.encode(chain) <= MAX_LINE, "the chain fits a page")
  eq(chain.result.truncated, true)
  ok(#chain.result.nodes >= 1 and #chain.result.nodes < 400)
  -- an instrument with 255 samples with long names
  for _ = 1, 253 do mock.ops.insert_sample(1, { name = long }) end
  E = epoch_of(peer)
  local body = call(peer, "focus_body", { kind = "instrument", ref = E .. ":instrument:1" })
  ok(#json.encode(body) <= MAX_LINE, "the instrument body fits a page")
  eq(body.result.truncated, true)
  ok(#body.result.keyzones >= 1 and #body.result.keyzones < 255)
  H.no_dialogs()
end)

test("reads: keyzones say their layer and mappings, instruments say what they play through, tracks their column mutes", function()
  local env, peer = setup()
  fill_demo()
  mock.ops.set_mapping(1, 1, "layer", 2)
  mock.ops.set_mapping(1, 2, "map_key_to_pitch", false)
  mock.ops.set_mapping(1, 2, "map_velocity_to_volume", false)
  mock.ops.insert_instrument({ name = "Hardware", tab = 3,
    midi = { device = "IAC Bus 1", type = 1, channel = 3, program = 5, bank = 1, transpose = -2, delay = 10, duration = 64 } })
  local E = epoch_of(peer)
  local samples = rows(peer, "sample", { parent = E .. ":instrument:1" })
  eq(samples[1].layer, "note-off")
  eq(samples[1].mapKeyToPitch, true)
  eq(samples[1].mapVelocityToVolume, true)
  eq(samples[2].layer, "note-on")
  eq(samples[2].mapKeyToPitch, false)
  eq(samples[2].mapVelocityToVolume, false)
  local instruments = rows(peer, "instrument")
  eq(instruments[1].kind, "sample")
  eq(instruments[1].midi, nil)
  eq(instruments[2].kind, "plugin")
  eq(instruments[3].kind, "midi")
  eq(instruments[3].midi.device, "IAC Bus 1")
  eq(instruments[3].midi.type, "external")
  eq(instruments[3].midi.channel, 3)
  eq(instruments[3].midi.program, 5)
  eq(instruments[3].midi.transpose, -2)
  eq(instruments[3].midi.duration, 64)
  -- column mutes and names
  mock.ops.mute_column(4, 2, true)
  mock.ops.name_column(4, 2, "Pad")
  local bass = rows(peer, "track")[4]
  eq(bass.mutedColumns[1], 2)
  eq(bass.columnNames["2"], "Pad")
  eq(rows(peer, "track")[1].mutedColumns, nil, "a track with none says nothing")
  eq(rows(peer, "track")[1].columnNames, nil)
  local page = call(peer, "read", { kind = "line", parent = E .. ":pattern:1", track = 4 }).result
  eq(page.mutedColumns[1], 2)
  local body = call(peer, "focus_body", { kind = "pattern", window = { from = 1, to = 4 } }).result
  eq(body.tracks[4].mutedColumns[1], 2)
  eq(body.tracks[1].mutedColumns, nil)
  H.no_dialogs()
end)

test("reads: where the pid is unknown the project's identity is made from when Renoise started, not 0", function()
  local env, peer = setup({ platform = "WINDOWS" })
  local f = assert(io.open(env.discovery_file(0), "rb"))
  local started = json.decode(f:read("*a")).startedMs
  f:close()
  ok(started > 0, "a start time")
  local identity = rows(peer, "song")[1].identity
  ok(identity:match("^t" .. started .. ":%d+$"), "made from the start time: " .. identity)
  H.no_dialogs()
end)

test("reads: while stopped the beat is Renoise's own edit position, which follows LPB changes", function()
  local env, peer = setup()
  mock.ops.select("selected_sequence_index", 2)
  mock.ops.select("selected_line_index", 5)
  local before = call(peer, "focus").result.transport.beat
  eq(before, (32 + 4) / 4, "one pattern of 32 lines, then four lines, at 4 lines a beat")
  mock.data.transport_fields.edit_beats = 7.25 -- as ZLxx would make it
  eq(call(peer, "focus").result.transport.beat, 7.25)
  mock.data.transport_fields.edit_beats = nil
  mock.ops.transport("playing", true)
  mock.ops.play_at(1, 3, 2.5)
  eq(call(peer, "focus").result.transport.beat, 2.5, "playing, the play position's")
  H.no_dialogs()
end)

test("reads: only Renoise's own Doofer and Splitter are containers, not a plug-in with the same name", function()
  local env, peer = setup()
  mock.ops.insert_device(1, { name = "Doofer", params = {} })
  mock.ops.insert_device(1, { name = "Splitter", params = {} })
  mock.ops.insert_device(1, { name = "Doofer", path = "Audio/Effects/AU/Acme/Doofer", params = {} })
  mock.ops.insert_device(1, { name = "Splitter", path = "Audio/Effects/VST3/Acme/Splitter", params = {} })
  local E = epoch_of(peer)
  local devices = rows(peer, "device", { parent = E .. ":track:1" })
  eq(devices[1].isDoofer, true)
  eq(devices[2].isSplitter, true)
  eq(devices[3].isDoofer, false)
  eq(devices[3].plugin, true)
  eq(devices[4].isSplitter, false)
  local chain = call(peer, "focus_body", { kind = "dsp", ref = E .. ":track:1" }).result
  eq(chain.nodes[1].container, true)
  eq(chain.nodes[3].container, false)
  H.no_dialogs()
end)

test("reads: swapping two tracks or two instruments adds no notifier and leaves every object watched", function()
  local env, peer = setup()
  H.tick(2)
  local watching = env.payload().watching()
  mock.notifier_adds = 0
  mock.ops.swap_tracks(1, 3)
  mock.ops.swap_instruments(1, 2)
  H.tick(3)
  eq(mock.notifier_adds, 0, "nothing was watched anew")
  eq(env.payload().watching(), watching)
  local function moves(what, change)
    local base = call(peer, "focus").result.changed
    change()
    ok(call(peer, "focus").result.changed > base, what .. " counts")
  end
  moves("a rename of the track now first", function() mock.ops.rename_track(1, "Renamed") end)
  moves("a rename of the track now third", function() mock.ops.rename_track(3, "Renamed too") end)
  moves("a rename of the instrument now first", function() mock.ops.rename_instrument(1, "Renamed") end)
  H.no_dialogs()
end)

test("reads: past 256 deleted objects their watchers are dropped without watching the song anew", function()
  local env, peer = setup()
  H.tick(2)
  local base = env.payload().watching()
  for i = 1, 300 do
    mock.ops.insert_track(1, { name = "x" })
    if i % 50 == 0 then drain(peer) end -- (a host reads its events)
  end
  local grown = env.payload().watching()
  ok(grown > base + 300 * 20, "each made track is watched: " .. (grown - base))
  for i = 1, 300 do
    mock.ops.remove_track(1)
    if i % 50 == 0 then drain(peer) end
  end
  mock.notifier_adds = 0
  H.tick(3)
  eq(mock.notifier_adds, 0, "nothing was added to drop the dead watchers")
  eq(env.payload().watching(), base, "and they are gone: the song is watched through what it was")
  -- what is left is still watched
  local probe = call(peer, "focus")
  ok(probe.result, "focus answers: " .. json.encode(probe.error or {}))
  local before = probe.result.changed
  mock.ops.rename_track(2, "Still watched")
  ok(call(peer, "focus").result.changed > before, "a track that was never deleted still counts")
  -- devices and patterns too
  mock.ops.insert_device(1, { name = "Gainer", params = {} })
  mock.ops.remove_device(1, 1)
  mock.ops.insert_device(1, { name = "Gainer", params = {} })
  before = call(peer, "focus").result.changed
  mock.ops.set_device(1, 1, "active", false)
  ok(call(peer, "focus").result.changed > before, "a device made after another was deleted counts")
  H.no_dialogs()
end)

-- Perf (N5): a lines read looks the pattern (and its track) up once a slice, not once a line; a read that waits between
-- slices looks them up again and reads the same rows.

-- (a note every third line of the demo's 32-line pattern: eleven)
local function notes_on(pattern, track)
  for l = 1, 31, 3 do mock.ops.set_note(pattern, track, l, 1, { note = 48 + l % 12, instrument = 0 }) end
end

-- (the rows of a lines read, and of the focus body's tracks, without what names the epoch)
local function rows_of(items)
  local out = {}
  for i, item in ipairs(items) do out[i] = { line = item.line, notes = item.notes, effects = item.effects, hidden = item.hidden } end
  return json.encode(out)
end

test("reads: a lines read asks Renoise for the pattern once a slice, not once a line (perf N5)", function()
  local _, peer = setup()
  notes_on(1, 1)
  local E = epoch_of(peer)
  local real_pattern, asked = mock.song.pattern, 0
  mock.song.pattern = function(self, p) asked = asked + 1 return real_pattern(self, p) end
  local response = call(peer, "read", { kind = "line", parent = E .. ":pattern:1", track = 1, limit = 200 })
  mock.song.pattern = real_pattern
  ok(response.result, json.encode(response.error or {}))
  eq(#response.result.items, 11)
  ok(asked <= 4, "pattern lookups for 32 lines read in one slice: " .. asked)
  -- (the focus body reads every track's lines the same way)
  asked = 0
  mock.song.pattern = function(self, p) asked = asked + 1 return real_pattern(self, p) end
  local body = call(peer, "focus_body", { kind = "pattern" })
  mock.song.pattern = real_pattern
  ok(body.result, json.encode(body.error or {}))
  ok(asked <= 6, "pattern lookups for a focus body read in one slice: " .. asked)
  H.no_dialogs()
end)

test("reads: a lines read that waits between every line looks the pattern up again each time and reads the same rows (perf N5)", function()
  local _, peer = setup()
  notes_on(1, 1)
  local E = epoch_of(peer)
  local whole = call(peer, "read", { kind = "line", parent = E .. ":pattern:1", track = 1, limit = 200 })
  ok(whole.result, json.encode(whole.error or {}))
  local body = call(peer, "focus_body", { kind = "pattern" })
  ok(body.result, json.encode(body.error or {}))
  -- (a job whose budget is spent before every row: every slice waits a tick, and what it held is looked up again)
  local _, slow = setup({ patch = function(env_)
    local path = env_.bundle .. "payload/jobs.lua"
    local f = assert(io.open(path, "rb"))
    local text = f:read("*a")
    f:close()
    local patched, n = text:gsub("\nM%.BUDGET_MS = 8\n", "\nM.BUDGET_MS = -1\n")
    assert(n == 1, "BUDGET_MS")
    f = assert(io.open(path, "wb"))
    f:write(patched)
    f:close()
  end })
  notes_on(1, 1)
  local E2 = epoch_of(slow)
  local real_pattern, asked = mock.song.pattern, 0
  mock.song.pattern = function(self, p) asked = asked + 1 return real_pattern(self, p) end
  local sliced = call(slow, "read", { kind = "line", parent = E2 .. ":pattern:1", track = 1, limit = 200 }, 2000)
  mock.song.pattern = real_pattern
  ok(sliced.result, json.encode(sliced.error or {}))
  eq(rows_of(sliced.result.items), rows_of(whole.result.items), "the same rows, slice by slice")
  ok(asked >= 30, "looked up again after each wait: " .. asked)
  local sliced_body = call(slow, "focus_body", { kind = "pattern" }, 2000)
  ok(sliced_body.result, json.encode(sliced_body.error or {}))
  eq(#sliced_body.result.tracks, #body.result.tracks)
  for t, track in ipairs(body.result.tracks) do
    eq(rows_of(sliced_body.result.tracks[t].lines), rows_of(track.lines), "track " .. t .. "'s lines, slice by slice")
  end
  H.no_dialogs()
end)

test("reads: a pattern track's automations come in the track's order, with none no parameter is read, and the walk ends at the last", function()
  local walk = T.load_payload_module("rows").each_automation
  local reads = 0
  local function device_of(parameters)
    return { parameters = parameters, parameter = function(self, k) reads = reads + 1; return self.parameters[k] end }
  end
  local a, b = { is_automated = true, lane = "a" }, { is_automated = true, lane = "b" }
  local devices = { device_of({ { is_automated = false }, a }), device_of({ b, { is_automated = true } }), device_of({ { is_automated = true } }) }
  local track = { devices = devices, device = function(self, d) return self.devices[d] end }
  local function pattern_track(lanes)
    return { automation = lanes, find_automation = function(_, parameter) return parameter.lane end }
  end
  local seen = {}
  walk(track, pattern_track({ "a", "b" }), function(d, k, _, _, automation) seen[#seen + 1] = table.concat({ d, k, automation }, ":") end)
  eq(table.concat(seen, " "), "1:2:a 2:1:b")
  eq(reads, 3, "the last parameter read is the last automation's")
  reads = 0
  walk(track, pattern_track({}), function() error("no automation to give") end)
  eq(reads, 0, "with none, no parameter is read")
end)
