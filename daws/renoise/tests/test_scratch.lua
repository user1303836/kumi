-- Scratch copies (M5 §2.7, §7) and scratch-keep (§9) against the mock Renoise: made after the last sequencer track, OFF, with the
-- source's devices, mixer and lines; one undo step per action (numbered past about a second); the journal in Kumi's folder; the
-- sweep of copies a crash left; and the kind that makes a copy the producer's, with M3's rules.
local T = ...
local mock = T.mock
local json = T.load_payload_module("json")
local test, eq, ok = T.test, T.eq, T.ok
local H = T.harness

local next_id = 12000
local A, OFF, MUTED = 1, 2, 3
local DOT = "\194\183"

local function drain(peer)
  repeat
    local taken = peer:take()
    if #taken > 0 then H.send(peer, { type = "credit", bytes = #taken }) end
  until #taken == 0
end

local function call(peer, method, params, ticks)
  next_id = next_id + 1
  local id = next_id
  H.send(peer, { type = "request", id = id, method = method, params = params or {} })
  peer.events = peer.events or {}
  for _ = 1, ticks or 600 do
    drain(peer)
    for _, line in ipairs(peer:frames(nil)) do
      local frame = assert(json.decode(line))
      if frame.type == "response" and frame.id == id then return frame end
      if frame.type == "event" then peer.events[#peer.events + 1] = frame end
    end
    mock.advance(10)
  end
end

local function result(response, what)
  ok(response and response.result, (what or "request") .. ": " .. json.encode(response and response.error or "no answer"))
  return response.result
end

local function failure(response, code, pattern)
  ok(response and response.error, "expected an error, got " .. json.encode(response and response.result or "nothing"))
  eq(response.error.code, code, "error code (" .. tostring(response.error.message) .. ")")
  if pattern then ok(response.error.message:find(pattern, 1, true), response.error.message) end
end

-- Kick, Hat, Bass (an EQ and a Compressor, a muted column, notes in patterns 1 and 3, automation of the EQ's first parameter in
-- pattern 1), Pad in a group "Bus" (track 5), the master, a send.
local function song_options(extra)
  local spec = {
    name = "Scratch", file_name = "/s/scratch.xrns", bpm = 120, lpb = 4, mixer_device = true,
    tracks = {
      { name = "Kick" }, { name = "Hat" },
      { name = "Bass", devices = {
        { name = "EQ 5", path = "Audio/Effects/Native/EQ 5", preset_data = "<eq gain='3'/>", params = { { name = "Gain 1 (100 Hz)", value = 3, min = -20, max = 20 } } },
        { name = "Compressor", path = "Audio/Effects/Native/Compressor", active = false, params = { { name = "Threshold", value = 11, min = 0, max = 15 } } },
      } },
      { name = "Pad" }, { name = "Bus", kind = "group", members = 1 },
    },
    send_tracks = 1, patterns = 3, pattern_lines = 16, sequence_length = 3, sequence = { 1, 2, 3 }, instruments = 1,
  }
  for k, v in pairs(extra or {}) do spec[k] = v end
  return spec
end

local function setup(options, extra)
  options = options or {}
  options.song = song_options(extra)
  local env = H.setup(options)
  -- (a device made from a path has that device's parameters)
  mock.device_catalog = {
    ["Audio/Effects/Native/EQ 5"] = { { name = "Gain 1 (100 Hz)", value = 0, min = -20, max = 20 } },
    ["Audio/Effects/Native/Compressor"] = { { name = "Threshold", value = 11.678, min = 0, max = 15 } },
  }
  local peer = H.connect(env)
  H.frames(peer)
  mock.ops.set_note(1, 3, 1, 1, { note = 36, instrument = 0 })
  mock.ops.set_note(3, 3, 5, 2, { note = 40, instrument = 0 })
  mock.ops.mute_column(3, 2, true)
  mock.ops.automate(1, 3, 2, 1, { { time = 1, value = 0 }, { time = 9, value = 1 } })
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  env.journal = env.dir .. "/kumi/bridges/renoise/listening/scratch.json"
  return env, peer
end

local function place(peer)
  local focus = result(call(peer, "focus"), "focus")
  return focus.epoch, focus.layout
end

local function track_ref(epoch, i) return epoch .. ":track:" .. i end
local function track(i) return mock.data.tracks[i] end
local function names()
  local out = {}
  for _, t in ipairs(mock.data.tracks) do out[#out + 1] = t.name end
  return table.concat(out, ",")
end
local function steps() return #mock.data.undo.stack end
local function newest() return mock.data.undo.stack[#mock.data.undo.stack] and mock.data.undo.stack[#mock.data.undo.stack].desc end

local function read_journal(env)
  local f = io.open(env.journal, "rb")
  if not f then return nil end
  local text = f:read("*a")
  f:close()
  return json.decode(text)
end

local function make(peer, id, count, more)
  local epoch, layout = place(peer)
  local params = { action = "make", scratchId = id, ref = track_ref(epoch, 3), count = count, label = "tune EQ 5", epoch = epoch, layout = layout }
  for k, v in pairs(more or {}) do params[k] = v end
  return call(peer, "scratch", params)
end

test("scratch: make copies a track after the last sequencer track, OFF and outside any group, with its devices, mixer and lines", function()
  local env, peer = setup()
  track(3).volume, track(3).post_volume = 0.5, 0.8
  local made = result(make(peer, "s1a2b", 3), "make")
  eq(made.scratchId, "s1a2b")
  eq(#made.copies, 3)
  eq(made.undo.steps, 1)
  eq(steps(), 1, "one Renoise undo step")
  ok(newest():find("Kumi: scratch copies of \226\128\156Bass\226\128\157 for tune EQ 5 (s1a2b)", 1, true), newest())
  eq(names(), "Kick,Hat,Bass,Pad,Bus,Kumi " .. DOT .. " try 1a2b 1,Kumi " .. DOT .. " try 1a2b 2,Kumi " .. DOT .. " try 1a2b 3,Master,Send 1")
  eq(made.copies[1].name, "Kumi " .. DOT .. " try 1a2b 1")
  eq(made.copies[1].ref, made.epoch .. ":track:6")
  local copy = track(6)
  eq(copy.mute_state, OFF, "a copy processes nothing until Kumi solos it")
  eq(copy.volume, 0.5, "the source's fader")
  eq(copy.post_volume, 0.8)
  eq(#copy.devices, 3)
  eq(copy.devices[2].path, "Audio/Effects/Native/EQ 5")
  eq(copy.devices[2].preset_data, "<eq gain='3'/>")
  eq(copy.devices[3].active, false, "bypass copied")
  eq(mock.data.patterns[1].tracks[6].lines[1].notes[1].note, 36, "lines copied")
  eq(mock.data.patterns[3].tracks[6].lines[5].notes[2].note, 40)
  eq((copy.muted_columns or {})[2], true, "the muted column too")
  local automation = mock.data.patterns[1].tracks[6].automation[1]
  ok(automation and automation.parameter == copy.devices[2].params[1], "the automation on the copy's own EQ")
  eq(mock.copies_from, 6, "only the 2 patterns it has lines in, for each of the 3 copies")
  for k = 6, 8 do
    local parents = 0
    for g, t in ipairs(mock.data.tracks) do
      if t.kind == "group" and g > k and g - t.members <= k then parents = parents + 1 end
    end
    eq(parents, 0, "outside any group")
  end
  local journal = read_journal(env)
  eq(#journal.entries, 1)
  eq(journal.entries[1].scratchId, "s1a2b")
  eq(journal.entries[1].copies[3].name, "Kumi " .. DOT .. " try 1a2b 3")
  eq(journal.entries[1].copies[3].index, 8)
  eq(journal.entries[1].source.name, "Bass")
  H.no_dialogs()
end)

test("scratch: a copy Renoise can't finish (a plug-in that isn't on this computer) goes again, and nothing is left behind", function()
  local env, peer = setup()
  local bass = track(3)
  bass.devices[#bass.devices + 1] = { name = "Gone Verb", path = "Audio/Effects/AU/Missing Verb", params = {}, active = true }
  local before = names()
  failure(make(peer, "s9f0e", 2), "failed")
  eq(names(), before, "no copy left, whole or half made")
  local journal = read_journal(env)
  eq(journal and #journal.entries or 0, 0, "nothing for a sweep to look for")
  H.no_dialogs()
end)

test("scratch: make copies only the patterns a range plays", function()
  local env, peer = setup()
  result(make(peer, "s2", 1, { range = { fromBeat = 8, beats = 4 } }))
  eq(mock.copies_from, 1, "slot 3 alone")
  eq(mock.data.patterns[3].tracks[6].lines[5].notes[2].note, 40)
  local untouched = mock.data.patterns[1].tracks[6]
  ok(not untouched or next(untouched.lines) == nil, "pattern 1 not copied")
  H.no_dialogs()
end)

test("scratch: make refuses before anything is made: not a sequencer track, counts, labels, refs of another epoch, no journal", function()
  local env, peer = setup()
  local epoch, layout = place(peer)
  local function refused(params, code, pattern)
    params.action, params.epoch, params.layout = "make", params.epoch or epoch, layout
    params.label = params.label or "x"
    params.scratchId = params.scratchId or "s9"
    failure(call(peer, "scratch", params), code, pattern)
    eq(#mock.data.tracks, 7)
  end
  refused({ ref = track_ref(epoch, 5), count = 1 }, "bad-params", "Kumi copies a sequencer track")
  refused({ ref = track_ref(epoch, 6), count = 1 }, "bad-params")
  refused({ ref = track_ref(epoch, 3), count = 17 }, "bad-params", "count is from 1 to 16")
  refused({ ref = track_ref(epoch, 3), count = 1, label = "two\nlines" }, "bad-params")
  refused({ ref = "1f:track:3", count = 1 }, "stale")
  refused({ ref = track_ref(epoch, 3), count = 1, scratchId = "x9" }, "bad-params", "scratchId")
  local journal = read_journal(env)
  eq(journal and #journal.entries or 0, 0, "a make refused notes nothing")
  -- the journal can't be written (the listening folder is a file): nothing is made. The folder may be there already (#516:
  -- in some runs of the tests it was by now), and nothing in it matters here.
  local folder = env.dir .. "/kumi/bridges/renoise/listening"
  assert(os.execute("rm -rf '" .. folder .. "'") == 0)
  local f = assert(io.open(folder, "wb"))
  f:write("in the way")
  f:close()
  refused({ ref = track_ref(epoch, 3), count = 1 }, "failed", "so it made none")
  os.remove(folder)
  eq(steps(), 0)
  H.no_dialogs()
end)

test("scratch: set writes parameters of copies by the device's place on the source, one step; a ref is followed to its copy", function()
  local env, peer = setup()
  local made = result(make(peer, "s3", 2))
  local before = steps()
  local done = result(call(peer, "scratch", { action = "set", scratchId = "s3", values = {
    { ref = made.copies[2].ref, device = 2, param = 1, value = -6 },
    { ref = made.copies[1].ref, device = 3, param = 1, normalized = 0.5 },
  } }), "set")
  eq(done.set, 2)
  eq(done.undo.steps, 1)
  eq(steps(), before + 1)
  eq(track(7).devices[2].params[1].value, -6)
  eq(track(6).devices[3].params[1].value, 7.5)
  eq(track(3).devices[2].params[1].value, 3, "the source untouched")
  -- the producer makes a track before the copies: the refs Kumi was given still name the same copies
  mock.ops.insert_track(1, { name = "New" })
  result(call(peer, "scratch", { action = "set", scratchId = "s3", values = { { ref = made.copies[2].ref, device = 2, param = 1, value = 9 } } }))
  eq(track(8).devices[2].params[1].value, 9, "copy 2, now at 8")
  eq(track(7).devices[2].params[1].value, 0, "copy 1's EQ untouched")
  eq(track(7).devices[3].params[1].value, 7.5, "copy 1 as it was set")
  H.no_dialogs()
end)

test("scratch: set checks every value before writing any: a track that is no copy, a device that changed, a value out of range", function()
  local env, peer = setup()
  local made = result(make(peer, "s4", 1))
  local epoch = made.epoch
  local before = steps()
  local gain = track(6).devices[2].params[1].value
  local function refused(values, code, pattern)
    failure(call(peer, "scratch", { action = "set", scratchId = "s4", values = values }), code, pattern)
    eq(steps(), before)
  end
  refused({ { ref = made.copies[1].ref, device = 2, param = 1, value = 1 }, { ref = track_ref(epoch, 3), device = 2, param = 1, value = 0 } }, "gone")
  refused({ { ref = made.copies[1].ref, device = 2, param = 1, value = 25 } }, "bad-params", "from -20 to 20")
  refused({ { ref = made.copies[1].ref, device = 9, param = 1, value = 1 } }, "changed")
  failure(call(peer, "scratch", { action = "set", scratchId = "s0", values = { { ref = made.copies[1].ref, device = 2, param = 1, value = 1 } } }), "bad-params", "there is no scratch s0")
  eq(track(6).devices[2].params[1].value, gain, "nothing written")
  H.no_dialogs()
end)

test("scratch: drop removes the copies in one step and forgets them; a copy the producer renamed is left and named", function()
  local env, peer = setup()
  result(make(peer, "s5", 3))
  mock.ops.rename_track(7, "Keeper")
  local before = steps()
  local dropped = result(call(peer, "scratch", { action = "drop", scratchId = "s5" }), "drop")
  eq(dropped.dropped, 2)
  eq(dropped.left, 1)
  ok(dropped.notes[1]:find("\226\128\156Keeper\226\128\157", 1, true), dropped.notes[1])
  eq(dropped.undo.steps, 1)
  eq(steps(), before + 1)
  eq(names(), "Kick,Hat,Bass,Pad,Bus,Keeper,Master,Send 1")
  eq(#read_journal(env).entries, 0, "forgotten")
  local again = result(call(peer, "scratch", { action = "drop", scratchId = "s5" }), "a second drop")
  eq(again.dropped, 0, "a retry drops nothing and is no error")
  eq(names(), "Kick,Hat,Bass,Pad,Bus,Keeper,Master,Send 1", "the renamed copy stays the producer's")
  H.no_dialogs()
end)

test("scratch: a drop of a scratch the bridge doesn't hold is harmless: nothing dropped; one its journal notes in this song is removed", function()
  local env, peer = setup()
  local before = steps()
  local none = result(call(peer, "scratch", { action = "drop", scratchId = "s404" }), "drop unknown")
  eq(none.dropped, 0)
  eq(none.left, 0)
  eq(none.undo.steps, 0)
  eq(steps(), before)
  failure(call(peer, "scratch", { action = "drop", scratchId = "x1" }), "bad-params")
  -- made, then the bridge lost it (a reload of Renoise's tools): the journal still notes it
  result(make(peer, "s17", 2))
  KUMI_BRIDGE.bridge.scratch.live = {}
  local dropped = result(call(peer, "scratch", { action = "drop", scratchId = "s17" }), "drop by the journal")
  eq(dropped.dropped, 2)
  eq(dropped.undo.steps, 1)
  eq(names(), "Kick,Hat,Bass,Pad,Bus,Master,Send 1")
  eq(#read_journal(env).entries, 0)
  H.no_dialogs()
end)

test("scratch: drop after the producer's own undo took the copies: nothing to remove, nothing left", function()
  local env, peer = setup()
  result(make(peer, "s6", 2))
  mock.song:undo()
  eq(names(), "Kick,Hat,Bass,Pad,Bus,Master,Send 1")
  local before = steps()
  local dropped = result(call(peer, "scratch", { action = "drop", scratchId = "s6" }))
  eq(dropped.dropped, 0)
  eq(dropped.left, 0)
  eq(dropped.undo.steps, 0)
  eq(steps(), before, "no step for nothing")
  H.no_dialogs()
end)

test("scratch: a copy's cost counts its patterns' lines, and one copy too big for a callback is refused by its own code before anything is made (#361 N-S18)", function()
  -- A long song's bass, in patterns of 512 lines.
  local function song_of(patterns)
    local sequence = {}
    for p = 1, patterns do sequence[p] = p end
    local _, peer = setup({}, { patterns = patterns, sequence_length = patterns, sequence = sequence, pattern_lines = 512 })
    for p = 1, patterns do mock.ops.set_note(p, 3, 1, 1, { note = 36, instrument = 0 }) end
    mock.data.undo.stack = {}
    return peer
  end
  -- About 3.2 s by the model: made (Renoise took 2.6 s for this many lines, every column full; lab 2026-11-01).
  result(make(song_of(100), "s8", 1), "make")
  -- A make of several is held to the host's wait in all: seven of those, about 22 s, are made, and seven at 4 s each, about 28 s,
  -- though one is allowed, are refused with nothing made.
  result(make(song_of(100), "s10", 7), "make")
  result(make(song_of(125), "s11", 1), "make")
  local peer = song_of(125)
  local before = #mock.data.tracks
  local refused_all = make(peer, "s12", 7)
  ok(refused_all.error and refused_all.error.code == "too-big", json.encode(refused_all.result or refused_all.error))
  ok(refused_all.error.message:find("in all", 1, true), refused_all.error.message)
  eq(#mock.data.tracks, before, "no copy made")
  -- About 6.4 s: refused.
  local peer = song_of(200)
  local tracks = #mock.data.tracks
  local refused = make(peer, "s9", 1)
  ok(refused.error and refused.error.code == "too-big", json.encode(refused.result or refused.error))
  ok(refused.error.message:find("give a range", 1, true), refused.error.message)
  eq(#mock.data.tracks, tracks, "no copy made")
  eq(steps(), 0)
end)

test("scratch: a make past about a second of work is numbered steps, each its own undo step, and says how many", function()
  local env, peer = setup({}, { patterns = 12, sequence_length = 12, sequence = { 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12 }, pattern_lines = 64 })
  for p = 4, 12 do mock.ops.set_note(p, 3, 1, 1, { note = 36, instrument = 0 }) end
  mock.data.undo.stack = {}
  -- (the clock moves as the bridge reads it: a step's time runs out after a dozen copies)
  mock.clock_step_ms = 60
  local made = result(make(peer, "s7", 16), "make")
  mock.clock_step_ms = 0
  ok(made.undo.steps > 1, "steps: " .. made.undo.steps)
  eq(steps(), made.undo.steps)
  ok(mock.data.undo.stack[1].desc:find("(1/", 1, true), mock.data.undo.stack[1].desc)
  eq(#made.copies, 16)
  for n = 1, 16 do eq(track(5 + n).name, "Kumi " .. DOT .. " try 7 " .. n) end
  H.no_dialogs()
end)

test("scratch: copies a crash left are swept when the bridge loads (Kumi told when it connects) and when their song opens again", function()
  local env, peer = setup()
  result(make(peer, "s8", 2))
  -- Renoise restarts: the bridge's live state is gone, its journal isn't
  KUMI_BRIDGE.bridge.scratch.live = {}
  KUMI_BRIDGE.bridge.scratch_sweep_due = true
  peer.events = {}
  H.tick(2)
  call(peer, "ping")
  eq(names(), "Kick,Hat,Bass,Pad,Bus,Master,Send 1", "swept")
  local swept
  for _, event in ipairs(peer.events) do
    if event.name == "scratch_swept" then swept = event.data end
  end
  ok(swept, "Kumi is told")
  eq(swept.count, 2)
  eq(swept.names[1], "Kumi " .. DOT .. " try 8 1")
  eq(newest(), "Kumi: removed scratch copies left from before")
  eq(#read_journal(env).entries, 0)
  -- another song is opened while copies of this one are noted: they wait for it
  result(make(peer, "s9", 1))
  KUMI_BRIDGE.bridge.scratch.live = {}
  mock.load_song(song_options({ name = "Other", file_name = "/s/other.xrns" }))
  H.tick(2)
  eq(#read_journal(env).entries, 1, "not this song's")
  mock.load_song(song_options())
  mock.ops.insert_track(4, { name = "Kumi " .. DOT .. " try 9 1" })
  H.tick(2)
  eq(#read_journal(env).entries, 0)
  for _, t in ipairs(mock.data.tracks) do ok(not t.name:find("try 9", 1, true), "swept when its song opened") end
  H.no_dialogs()
end)

test("scratch: a sweep while no Kumi is connected is told when one connects", function()
  local env, peer = setup()
  result(make(peer, "s10", 1))
  peer:close()
  H.tick(2)
  KUMI_BRIDGE.bridge.scratch.live = {}
  KUMI_BRIDGE.bridge.scratch_sweep_due = true
  H.tick(2)
  eq(KUMI_BRIDGE.bridge.scratch_swept.count, 1, "kept to tell")
  local later = H.connect(env)
  eq(KUMI_BRIDGE.bridge.scratch_swept, nil, "told on connecting (the harness's handshake reads the event with the welcome)")
  H.no_dialogs()
end)

-- scratch-keep

local next_k = 0
local function desc(what)
  next_k = next_k + 1
  return "Kumi: " .. what .. " (k" .. next_k .. ")"
end

local function preview(peer, input)
  local epoch, layout = place(peer)
  local response = call(peer, "change_preview", { kind = "scratch-keep", input = input, epoch = epoch, layout = layout })
  return result(response, "preview"), epoch, layout
end

local function keep(peer, input)
  local previewed, epoch, layout = preview(peer, input)
  local applied = call(peer, "change_apply", { kind = "scratch-keep", input = input, fence = previewed.fence, desc = desc("keep"), epoch = epoch, layout = layout })
  return result(applied, "apply"), previewed
end

local function undo(peer, done, more)
  local epoch, layout = place(peer)
  local params = { kind = "scratch-keep", target = done.target, prior = done.prior, applied = done.applied, created = done.created,
    material = done.material, desc = desc("undo keep"), epoch = done.epoch, layout = done.layout }
  for k, v in pairs(more or {}) do params[k] = v end
  return call(peer, "change_restore", params), epoch, layout
end

test("scratch-keep: a copy becomes the producer's track: renamed, on, out of the journal; drop leaves it; Kumi's undo removes it", function()
  local env, peer = setup()
  local made = result(make(peer, "s11", 2))
  local previewed = preview(peer, { ref = made.copies[2].ref, name = "Kumi " .. DOT .. " Goal best" })
  -- (Kumi names it "Kumi · Goal best": not a scratch name)
  eq(previewed.display.title, "Kept \226\128\156Kumi " .. DOT .. " Goal best\226\128\157")
  local before = steps()
  local done = keep(peer, { ref = made.copies[2].ref, name = "Kumi " .. DOT .. " Goal best" })
  eq(steps(), before + 1, "one undo step")
  eq(done.undo.entry, true)
  eq(done.created.ref, done.epoch .. ":track:7")
  eq(track(7).name, "Kumi " .. DOT .. " Goal best")
  eq(track(7).mute_state, A, "on")
  ok(done.display.note:find("not its mute", 1, true))
  local journal = read_journal(env)
  eq(#journal.entries[1].copies, 1, "out of the journal")
  local dropped = result(call(peer, "scratch", { action = "drop", scratchId = "s11" }))
  eq(dropped.dropped, 1)
  eq(names(), "Kick,Hat,Bass,Pad,Bus,Kumi " .. DOT .. " Goal best,Master,Send 1", "the kept one stays")
  local undone = result(undo(peer, done), "undo")
  eq(undone.restored.deleted, true)
  eq(names(), "Kick,Hat,Bass,Pad,Bus,Master,Send 1")
  H.no_dialogs()
end)

test("scratch-keep: a track that is no scratch copy is refused, and a scratch name for the kept track", function()
  local env, peer = setup()
  local made = result(make(peer, "s12", 1))
  local epoch, layout = place(peer)
  failure(call(peer, "change_preview", { kind = "scratch-keep", input = { ref = track_ref(epoch, 3), name = "Best" }, epoch = epoch, layout = layout }),
    "bad-params", "isn't one of Kumi's scratch copies")
  failure(call(peer, "change_preview", { kind = "scratch-keep", input = { ref = made.copies[1].ref, name = "Kumi " .. DOT .. " try x" }, epoch = epoch, layout = layout }),
    "bad-params")
  H.no_dialogs()
end)

test("scratch-keep: the apply refuses `changed` when the copy was renamed, muted or given lines since the preview (its fence)", function()
  local env, peer = setup()
  local made = result(make(peer, "s13", 1))
  local input = { ref = made.copies[1].ref, name = "Best" }
  local function refused(disturb, undisturb)
    local previewed, epoch, layout = preview(peer, input)
    disturb()
    local before = steps()
    failure(call(peer, "change_apply", { kind = "scratch-keep", input = input, fence = previewed.fence, desc = desc("keep"), epoch = epoch, layout = layout }), "changed")
    eq(steps(), before, "nothing written")
    undisturb()
  end
  local name = track(6).name
  refused(function() mock.ops.rename_track(6, "Mine") end, function() mock.ops.rename_track(6, name) end)
  refused(function() mock.ops.mute_track(6, MUTED) end, function() mock.ops.mute_track(6, OFF) end)
  refused(function() mock.ops.set_note(2, 6, 3, 1, { note = 50, instrument = 0 }) end, function() mock.data.patterns[2].tracks[6] = nil end)
  eq(track(6).name, name)
  H.no_dialogs()
end)

test("scratch-keep: Kumi's undo refuses `changed` once the producer worked on the kept track (unless discard), `gone` once it is deleted", function()
  local env, peer = setup()
  local made = result(make(peer, "s14", 2))
  local done = keep(peer, { ref = made.copies[1].ref, name = "Best" })
  mock.ops.set_note(2, 6, 1, 1, { note = 48, instrument = 0 })
  local before = steps()
  failure(undo(peer, done), "changed", "changed in Renoise since Kumi kept it")
  eq(steps(), before)
  eq(track(6).name, "Best")
  local forced = result(undo(peer, done, { discard = true }), "undo with discard")
  eq(forced.restored.deleted, true)
  -- gone
  local again = keep(peer, { ref = result(call(peer, "focus")).epoch .. ":track:6", name = "Best 2" })
  mock.ops.remove_track(6)
  failure(undo(peer, again), "gone")
  H.no_dialogs()
end)

-- Work the counts don't see (the pattern already had lines, the same devices): Kumi's undo leaves the track, nothing written.
for _, case in ipairs({
  { "a note where it had lines", function(song) song:pattern(1):track(6):line(2):note_column(1).note_value = 77 end },
  { "a knob", function(song) song:track(6):device(3):parameter(1).value = 5 end },
  { "the fader", function(song) song:track(6).prefx_volume.value = 0.5 end },
  { "an effect", function(song) song:pattern(3):track(6):line(5):effect_column(1).number_string = "0B" end },
  { "a slot muted", function(song) song.sequencer:set_track_sequence_slot_is_muted(6, 2, true) end },
}) do
  test("scratch-keep: Kumi's undo leaves the kept track once the producer worked on it: " .. case[1], function()
    local env, peer = setup()
    local made = result(make(peer, "s16", 1))
    local done = keep(peer, { ref = made.copies[1].ref, name = "Best" })
    ok(type(done.material.print) == "string" and done.material.print ~= "unread", "printed")
    case[2](mock.song)
    local before = steps()
    failure(undo(peer, done), "changed", "so Kumi leaves it")
    eq(steps(), before, "nothing written")
    eq(track(6).name, "Best", "the track stays")
    H.no_dialogs()
  end)
end

test("scratch-keep: a kept track untouched goes with Kumi's undo, also once the play head moved an automated knob", function()
  local env, peer = setup()
  local made = result(make(peer, "s17", 1))
  mock.ops.automate(1, 6, 2, 1, { { time = 1, value = 0.2 } })
  local done = keep(peer, { ref = made.copies[1].ref, name = "Best" })
  -- (an automated knob moves as the song plays: its automation is what's printed)
  ok(mock.song:track(6):device(2):parameter(1).is_automated, "automated")
  mock.song:track(6):device(2):parameter(1).value = 9
  eq(result(undo(peer, done), "undo").restored.deleted, true)
  eq(names(), "Kick,Hat,Bass,Pad,Bus,Master,Send 1")
  H.no_dialogs()
end)

test("scratch-keep: its track is followed through tracks made, deleted and swapped before it; another song or an unknown layout is stale", function()
  local function kept()
    local env, peer = setup()
    local made = result(make(peer, "s15", 1))
    return env, peer, keep(peer, { ref = made.copies[1].ref, name = "Best" })
  end
  -- made before it
  local _, peer, done = kept()
  mock.ops.insert_track(1, { name = "New" })
  eq(result(undo(peer, done), "undo after an insert").restored.deleted, true)
  eq(names(), "New,Kick,Hat,Bass,Pad,Bus,Master,Send 1")
  -- deleted before it
  _, peer, done = kept()
  mock.ops.remove_track(1)
  eq(result(undo(peer, done)).restored.deleted, true)
  eq(names(), "Hat,Bass,Pad,Bus,Master,Send 1")
  -- swapped (Kick and Hat): the epoch moves for tracks, the layout carries it
  _, peer, done = kept()
  mock.ops.swap_tracks(1, 2)
  -- (a swap moves the epoch of every track ref: what the record holds is refused, never guessed)
  failure(undo(peer, done), "stale")
  eq(track(6).name, "Best", "nothing deleted")
  -- another song
  _, peer, done = kept()
  mock.load_song(song_options({ name = "Other" }))
  failure(undo(peer, done), "stale")
  eq(#mock.data.tracks, 7, "the other song untouched")
  -- the shifts lost (a layout older than they reach)
  _, peer, done = kept()
  local old = done.layout
  done.layout = -1
  failure(undo(peer, done), "bad-params")
  done.layout = old + 5000
  failure(undo(peer, done), "stale")
  H.no_dialogs()
end)

test("scratch-keep: the producer's undo in Renoise takes back the name only, and Kumi is told", function()
  local env, peer = setup()
  local made = result(make(peer, "s16", 1))
  keep(peer, { ref = made.copies[1].ref, name = "Best" })
  peer.events = {}
  mock.song:undo()
  H.tick(2)
  call(peer, "ping")
  eq(track(6).name, "Kumi " .. DOT .. " try 16 1", "the name back")
  eq(track(6).mute_state, A, "the mute isn't Renoise's to take back")
  local told
  for _, event in ipairs(peer.events) do
    if event.name == "undone_in_daw" then told = event.data end
  end
  ok(told and told.how == "undo", json.encode(peer.events))
  ok(told.descs[1]:find("Kumi: keep", 1, true))
  H.no_dialogs()
end)

-- M7 (variations): players of phrases, for Kumi to hear phrase takes

test("scratch: players of phrases by their places play each where it is now, after a phrase was made before it", function()
  local env, peer = setup()
  local ins = mock.song:instrument(1)
  for k = 1, 3 do ins:insert_phrase_at(k).name = "V" .. k end
  local epoch = place(peer)
  local where = result(call(peer, "read", { kind = "where", items = { { phrase = epoch .. ":phrase:1.3" } } }), "where")
  local kept = where.items[1].place
  ok(type(kept) == "table", "a place: " .. json.encode(where))
  ins:insert_phrase_at(1).name = "New"
  local layout
  epoch, layout = place(peer)
  result(call(peer, "scratch", { action = "make", scratchId = "s9p2a", count = 1, label = "variations", epoch = epoch, layout = layout,
    range = { fromBeat = 4, beats = 4 }, phrases = { instrument = epoch .. ":instrument:1", phrases = { kept }, note = 50 } }), "make")
  eq(mock.song:pattern(2):track(6):line(1):effect_column(1).amount_string, "04", "V3, phrase 04 now")
  result(call(peer, "scratch", { action = "drop", scratchId = "s9p2a" }), "drop")
end)

test("scratch: players of phrases are empty tracks, each playing the instrument's note with a 0Zxx at the range's first line; drop takes them", function()
  local env, peer = setup()
  local ins = mock.song:instrument(1)
  for k = 1, 3 do ins:insert_phrase_at(k).name = "V" .. k end
  mock.data.undo.stack = {}
  local epoch, layout = place(peer)
  local made = result(call(peer, "scratch", { action = "make", scratchId = "s9p1a", count = 2, label = "variations", epoch = epoch, layout = layout,
    range = { fromBeat = 4, beats = 4 }, phrases = { instrument = epoch .. ":instrument:1", phrases = { 3, 1 }, note = 50 } }), "make")
  eq(#made.copies, 2)
  eq(steps(), 1, "one Renoise undo step")
  ok(newest():find("players of", 1, true), newest())
  local t = 6
  eq(track(t).name, "Kumi " .. DOT .. " try 9p1a 1")
  eq(#track(t).devices, 1, "no device but the mixer")
  eq(track(t).mute_state, OFF)
  -- (beat 4 is slot 2's start, pattern 2 at LPB 4 with 16 lines: line 1)
  local line = mock.song:pattern(2):track(t):line(1)
  eq(line:note_column(1).note_value, 50)
  eq(line:note_column(1).instrument_value, 0)
  eq(line:effect_column(1).number_string .. line:effect_column(1).amount_string, "0Z03")
  eq(mock.song:pattern(2):track(t + 1):line(1):effect_column(1).amount_string, "01")
  ok(read_journal(env).entries[1].copies[2].name:find("try 9p1a 2", 1, true), "journaled, for a sweep to find")
  result(call(peer, "scratch", { action = "drop", scratchId = "s9p1a" }), "drop")
  eq(names(), "Kick,Hat,Bass,Pad,Bus,Master,Send 1")
  failure(call(peer, "scratch", { action = "make", scratchId = "s9p1b", count = 1, label = "v", epoch = epoch, layout = layout,
    range = { fromBeat = 2, beats = 4 }, phrases = { instrument = epoch .. ":instrument:1", phrases = { 1 } } }), "bad-params")
end)
