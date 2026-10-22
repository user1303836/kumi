-- Kumi's changes to an instrument's samples (M4, n-instruments): settings, keyzones, overlap, a drum-kit layout, samples loaded
-- and deleted, slice markers; each applied, put back, refused when changed, gone, back already; and the places followed.
local T = ...
local mock = T.mock
local json = T.load_payload_module("json")
local test, eq, ok = T.test, T.eq, T.ok
local H = T.harness
local I = T.instruments
local setup, place, preview, apply, change, restore, failure, call = I.setup, I.place, I.preview, I.apply, I.change, I.restore, I.failure, I.call
local steps, newest, instrument, iref, sref = I.steps, I.newest, I.instrument, I.iref, I.sref

local function sample(i, s) return mock.data.instruments[i].samples[s] end
local function sample_names(i)
  local out = {}
  for s, data in ipairs(mock.data.instruments[i].samples) do out[s] = data.name end
  return table.concat(out, ",")
end

local function told(peer)
  H.tick(3)
  repeat
    local taken = peer:take()
    if #taken > 0 then H.send(peer, { type = "credit", bytes = #taken }) end
  until #taken == 0
  return H.events(peer, "undone_in_daw")
end

-- sample-set

test("samples: settings change and go back as one step each: volume, pan, loop and its points, beat sync, NNA, interpolation, mute group, name", function()
  local env, peer = setup()
  local input = {
    ref = sref(place(peer), 1, 2), name = "Snare 2", volumeDb = -6, panning = -0.5, transpose = -12, fineTune = 10, loop = "pingpong",
    loopStart = 100, loopEnd = 700, loopRelease = true, beatSync = true, beatSyncLines = 32, beatSyncMode = "texture", nna = "sustain",
    muteGroup = 2, oneshot = true, autoseek = true, autofade = true, interpolation = "sinc", oversample = true,
  }
  local record, done = change(peer, "sample-set", input, "Kumi: snare (k500)")
  local data = sample(1, 2)
  eq(data.name, "Snare 2")
  ok(math.abs(data.volume - 10 ^ (-6 / 20)) < 1e-6)
  eq(data.panning, 0.25)
  eq(data.loop_mode, 4)
  eq(data.loop_start, 100)
  eq(data.loop_end, 700)
  eq(data.beat_sync_mode, 3)
  eq(data.new_note_action, 3)
  eq(data.interpolation_mode, 4)
  eq(data.mute_group, 2)
  eq(done.applied.nna, "continue", "Renoise's word")
  eq(steps(), 1)
  eq(newest(), "Kumi: snare (k500)")
  local undone = restore(peer, "sample-set", record)
  ok(undone.result, json.encode(undone.error or {}))
  eq(data.name, "Snare")
  eq(data.volume, 1)
  eq(data.loop_mode, 1)
  eq(data.loop_start, 1)
  eq(data.loop_end, 800)
  eq(data.new_note_action, 1)
  eq(data.mute_group, 0)
  eq(steps(), 2)
  H.no_dialogs()
end)

test("samples: bad settings are bad-params: a loop past the sample's frames, its start after its end, a mute group of 16", function()
  local env, peer = setup()
  local epoch = place(peer)
  local function bad(input)
    input.ref = sref(epoch, 1, 2)
    failure(call(peer, "change_preview", { kind = "sample-set", input = input, epoch = epoch }), "bad-params")
  end
  bad({ loopEnd = 801 })
  bad({ loopStart = 500, loopEnd = 400 })
  bad({ muteGroup = 16 })
  bad({ nna = "kill" })
  bad({ volumeDb = 13 })
  bad({})
end)

test("samples: a setting the producer changed since refuses the undo; the fence refuses an apply after a change in between", function()
  local env, peer = setup()
  local record = change(peer, "sample-set", { ref = sref(place(peer), 1, 1), muteGroup = 1 })
  sample(1, 1).mute_group = 3
  failure(restore(peer, "sample-set", record), "changed")
  eq(sample(1, 1).mute_group, 3)
  local previewed, epoch, layout = preview(peer, "sample-set", { ref = sref(place(peer), 1, 3), volumeDb = -3 })
  sample(1, 3).volume = 0.5
  failure(apply(peer, "sample-set", { ref = sref(epoch, 1, 3), volumeDb = -3 }, "Kumi: v (k1)", previewed.fence, epoch, layout), "changed")
  eq(sample(1, 3).volume, 0.5)
end)

test("places, sample × insert before, swap, delete before, Kumi's own load: the undo follows the sample; deleted: gone", function()
  for _, how in ipairs({ "insert", "swap", "delete before", "Kumi's load" }) do
    local env, peer = setup()
    local record = change(peer, "sample-set", { ref = sref(place(peer), 1, 2), muteGroup = 5 })
    local at
    if how == "insert" then
      mock.song:instrument(1):insert_sample_at(1)
      at = 3
    elseif how == "swap" then
      mock.song:instrument(1):swap_samples_at(2, 3)
      at = 3
    elseif how == "delete before" then
      mock.song:instrument(1):delete_sample_at(1)
      at = 1
    else
      local wav = mock.temp_dir .. "/clap.wav"
      local f = assert(io.open(wav, "wb"))
      f:write("RIFF\0\0\0\0WAVE" .. string.rep("c", 288))
      f:close()
      change(peer, "sample-load", { ref = iref(place(peer), 1), path = wav, index = 0 })
      at = 3
    end
    eq(sample(1, at).name, "Snare", how)
    local undone = restore(peer, "sample-set", record)
    ok(undone.result, how .. ": " .. json.encode(undone.error or {}))
    eq(sample(1, at).mute_group, 0, how)
  end
  local env, peer = setup()
  local record = change(peer, "sample-set", { ref = sref(place(peer), 1, 2), muteGroup = 5 })
  mock.song:instrument(1):delete_sample_at(2)
  failure(restore(peer, "sample-set", record), "gone")
end)

test("samples: the producer's undo of a sample setting nothing else watches (its NNA) is told as undone in Renoise", function()
  local env, peer = setup()
  change(peer, "sample-set", { ref = sref(place(peer), 1, 1), nna = "off" }, "Kumi: nna (k510)")
  told(peer)
  mock.song:undo()
  local events = told(peer)
  eq(#events, 1)
  eq(events[1].data.descs[1], "Kumi: nna (k510)")
end)

test("samples: a sample's modulation set and FX chain are kept as places: the undo puts back what it had though the producer made a set before", function()
  local env, peer = setup(nil, { instrument_specs = {
    { name = "Drums", sets = 2, chains = 1, sample_specs = { { name = "Kick", frames = 1000 }, { name = "Snare", frames = 800 } } },
    { name = "Bass synth" }, { name = "Pad" } } })
  local epoch = place(peer)
  local record, done = change(peer, "sample-set", { ref = sref(epoch, 1, 1), modulationSetRef = epoch .. ":modset:1.2", fxChainRef = epoch .. ":chain:1.1" })
  eq(sample(1, 1).modulation_set_index, 2)
  eq(sample(1, 1).device_chain_index, 1)
  ok(type(done.applied.modulationSet) == "table" and done.applied.modulationSet.place, "on the wire, a place")
  mock.song:instrument(1):insert_sample_modulation_set_at(1) -- Renoise renumbers the sample's set: 3
  eq(sample(1, 1).modulation_set_index, 3)
  local undone = restore(peer, "sample-set", record)
  ok(undone.result, json.encode(undone.error or {}))
  eq(sample(1, 1).modulation_set_index, 0)
  eq(sample(1, 1).device_chain_index, 0)
  epoch = place(peer)
  failure(call(peer, "change_preview", { kind = "sample-set", input = { ref = sref(epoch, 1, 1), modulationSetRef = epoch .. ":modset:2.1" }, epoch = epoch }), "bad-params")
end)

-- keyzones

test("samples: a keyzone's notes, velocities, base note, layer and key tracking change and go back; a slice's is refused", function()
  local env, peer = setup()
  local input = { ref = sref(place(peer), 1, 2), noteRange = { 36, 47 }, velocityRange = { 64, 127 }, baseNote = 40, layer = "note-off", keyTracking = false, velocityToVolume = false }
  local record, done = change(peer, "keyzone-set", input)
  local data = sample(1, 2)
  eq(data.note_range[1], 36)
  eq(data.velocity_range[1], 64)
  eq(data.base_note, 40)
  eq(data.layer, 2)
  eq(data.map_key_to_pitch, false)
  ok(done.display.title:find("keyzone", 1, true), done.display.title)
  ok(done.display.title:find("notes C-3\226\128\147B-3", 1, true) and done.display.title:find("base note E-3", 1, true),
    "notes as Renoise names them: " .. done.display.title)
  local undone = restore(peer, "keyzone-set", record)
  ok(undone.result, json.encode(undone.error or {}))
  eq(data.note_range[1], 0)
  eq(data.note_range[2], 119)
  eq(data.layer, 1)
  eq(data.map_key_to_pitch, true)
  -- a slice's keyzone
  mock.song:instrument(1):sample(1):insert_slice_marker(500)
  local epoch = place(peer)
  failure(call(peer, "change_preview", { kind = "keyzone-set", input = { ref = sref(epoch, 1, 2), baseNote = 50 }, epoch = epoch }), "bad-params")
end)

test("samples: the overlap mode changes and goes back", function()
  local env, peer = setup()
  local record, done = change(peer, "keyzone-overlap", { ref = iref(place(peer), 1), mode = "cycle" })
  eq(instrument(1).overlap, 1)
  ok(done.display.title:find("in turn", 1, true), done.display.title)
  local undone = restore(peer, "keyzone-overlap", record)
  ok(undone.result, json.encode(undone.error or {}))
  eq(instrument(1).overlap, 0)
end)

test("samples: a drum-kit layout puts one sample per key from C-4 (white keys when asked) in one step; the undo puts each zone back", function()
  local env, peer = setup()
  local record, done = change(peer, "keyzone-drumkit", { ref = iref(place(peer), 1) })
  eq(sample(1, 1).note_range[1], 48)
  eq(sample(1, 1).note_range[2], 48)
  eq(sample(1, 2).note_range[1], 49)
  eq(sample(1, 3).base_note, 50)
  eq(steps(), 1)
  ok(done.display.title:find("drum kit: 3 samples from C-4", 1, true), done.display.title)
  local undone = restore(peer, "keyzone-drumkit", record)
  ok(undone.result, json.encode(undone.error or {}))
  eq(sample(1, 2).note_range[1], 0)
  eq(sample(1, 2).note_range[2], 119)
  local epoch = place(peer)
  record = change(peer, "keyzone-drumkit", { ref = iref(epoch, 1), first = 36, whiteKeys = true, sampleRefs = { sref(epoch, 1, 3), sref(epoch, 1, 1) } })
  eq(sample(1, 3).base_note, 36)
  eq(sample(1, 1).base_note, 38, "D-3: the next white key")
  eq(sample(1, 2).note_range[2], 119, "a sample not named stays")
  sample(1, 3).note_range = { 36, 37 } -- the producer widens one
  failure(restore(peer, "keyzone-drumkit", record), "changed")
  eq(sample(1, 1).base_note, 38, "nothing written")
  epoch = place(peer)
  failure(call(peer, "change_preview", { kind = "keyzone-drumkit", input = { ref = iref(epoch, 1), first = 118 }, epoch = epoch }), "bad-params")
end)

test("samples: a key map lays out each sample as given and the overlap in one step; the undo puts back zones and overlap", function()
  local env, peer = setup()
  local epoch = place(peer)
  local input = { ref = iref(epoch, 1), overlap = "cycle", samples = {
    { sampleRef = sref(epoch, 1, 1), noteRange = { 0, 49 }, velocityRange = { 0, 63 }, baseNote = 48 },
    { sampleRef = sref(epoch, 1, 2), noteRange = { 0, 49 }, velocityRange = { 64, 127 }, baseNote = 48 },
    { sampleRef = sref(epoch, 1, 3), noteRange = { 50, 119 }, baseNote = 52 },
  } }
  local record, done = change(peer, "keyzone-map", input)
  eq(sample(1, 1).note_range[2], 49)
  eq(sample(1, 2).velocity_range[1], 64)
  eq(sample(1, 3).base_note, 52)
  eq(sample(1, 3).velocity_range[2], 127, "a velocity range left out is the whole")
  eq(instrument(1).overlap, 1)
  eq(steps(), 1)
  ok(done.display.title:find("3 samples, round robins in turn", 1, true), done.display.title)
  local undone = restore(peer, "keyzone-map", record)
  ok(undone.result, json.encode(undone.error or {}))
  eq(sample(1, 1).note_range[2], 119)
  eq(sample(1, 2).velocity_range[1], 0)
  eq(instrument(1).overlap, 0)
  -- Back already: nothing written.
  local before = steps()
  local again = restore(peer, "keyzone-map", record)
  ok(again.result, json.encode(again.error or {}))
  eq(steps(), before)
end)

test("samples: a key map's undo refuses a zone or the overlap the producer changed since, and a sample gone", function()
  local env, peer = setup()
  local epoch = place(peer)
  local input = { ref = iref(epoch, 1), overlap = "cycle", samples = {
    { sampleRef = sref(epoch, 1, 1), noteRange = { 0, 59 }, baseNote = 48 }, { sampleRef = sref(epoch, 1, 2), noteRange = { 60, 119 }, baseNote = 60 },
  } }
  local record = change(peer, "keyzone-map", input)
  sample(1, 2).base_note = 62 -- the producer retunes one
  failure(restore(peer, "keyzone-map", record), "changed")
  eq(sample(1, 1).note_range[2], 59, "nothing written")
  sample(1, 2).base_note = 60
  instrument(1).overlap = 2 -- then picks random
  failure(restore(peer, "keyzone-map", record), "changed")
  instrument(1).overlap = 1
  mock.song:instrument(1):delete_sample_at(2)
  local gone = restore(peer, "keyzone-map", record)
  ok(gone.error and (gone.error.code == "gone" or gone.error.code == "changed"), json.encode(gone))
end)

test("samples: a key map follows its samples when the producer inserts one before them", function()
  local env, peer = setup()
  local epoch = place(peer)
  local record = change(peer, "keyzone-map", { ref = iref(epoch, 1), samples = {
    { sampleRef = sref(epoch, 1, 2), noteRange = { 10, 20 }, baseNote = 15 },
  } })
  mock.song:instrument(1):insert_sample_at(1).name = "New"
  local undone = restore(peer, "keyzone-map", record)
  ok(undone.result, json.encode(undone.error or {}))
  eq(sample(1, 3).note_range[1], 0, "the sample it laid out, now third, is back")
  eq(sample(1, 3).base_note, 48)
end)

for _, case in ipairs({
  { "base notes", function(E) return { { sampleRef = sref(E, 1, 1), noteRange = { 0, 119 }, baseNote = 60 } } end, nil },
  { "velocities", function(E) return { { sampleRef = sref(E, 1, 1), noteRange = { 0, 119 }, velocityRange = { 0, 63 }, baseNote = 48 } } end, nil },
  { "the overlap", function(E) return { { sampleRef = sref(E, 1, 1), noteRange = { 0, 119 }, baseNote = 48 } } end, "cycle" },
}) do
  test("samples: the producer's undo of a key map that changes only " .. case[1] .. " is told as undone in Renoise", function()
    local env, peer = setup()
    local epoch = place(peer)
    change(peer, "keyzone-map", { ref = iref(epoch, 1), samples = case[2](epoch), overlap = case[3] }, "Kumi: map (k520)")
    told(peer)
    mock.song:undo()
    local events = told(peer)
    eq(#events, 1, case[1])
    eq(events[1].data.descs[1], "Kumi: map (k520)")
  end)
end

test("samples: the producer's undo of a drum kit that moves only base notes is told as undone in Renoise", function()
  local env, peer = setup()
  local epoch = place(peer)
  -- (the zones already as a kit lays them, only the base notes off)
  for s = 1, 3 do
    sample(1, s).note_range = { 47 + s, 47 + s }
    sample(1, s).base_note = 60
  end
  change(peer, "keyzone-drumkit", { ref = iref(epoch, 1) }, "Kumi: kit (k521)")
  told(peer)
  mock.song:undo()
  local events = told(peer)
  eq(#events, 1)
end)

test("samples: a key map refuses a sample of another instrument, a slice, a bad range", function()
  local env, peer = setup()
  local epoch = place(peer)
  failure(call(peer, "change_preview", { kind = "keyzone-map", epoch = epoch,
    input = { ref = iref(epoch, 1), samples = { { sampleRef = sref(epoch, 2, 1), noteRange = { 0, 10 }, baseNote = 5 } } } }), "bad-params")
  failure(call(peer, "change_preview", { kind = "keyzone-map", epoch = epoch,
    input = { ref = iref(epoch, 1), samples = { { sampleRef = sref(epoch, 1, 1), noteRange = { 20, 10 }, baseNote = 5 } } } }), "bad-params")
  failure(call(peer, "change_preview", { kind = "keyzone-map", epoch = epoch,
    input = { ref = iref(epoch, 1), samples = { { sampleRef = sref(epoch, 1, 1), noteRange = { 0, 10 } } } } }), "bad-params")
end)

-- The tidy's read (#401)

test("reads: the tidy prints each instrument's samples and settings, and counts the notes that play each", function()
  local env, peer = setup()
  mock.ops.set_note(1, 1, 1, 1, { note = 48, instrument = 0 })
  mock.ops.set_note(1, 1, 5, 1, { note = 50, instrument = 0 })
  local answer = call(peer, "read", { kind = "tidy" })
  ok(answer.result, json.encode(answer.error or {}))
  local items = answer.result.items
  eq(#items, #mock.data.instruments)
  eq(answer.result.nextCursor, nil, "one page")
  eq(#items[1].samples, #mock.data.instruments[1].samples)
  ok(type(items[1].samples[1].print) == "string", json.encode(items[1].samples[1]))
  ok(type(items[1].settings) == "string", json.encode(items[1]))
  local notes = call(peer, "read", { kind = "tidy-notes" })
  ok(notes.result, json.encode(notes.error or {}))
  eq(notes.result.counts["0"], 2)
  eq(notes.result.counts["1"], nil)
  eq(notes.result.to, #mock.data.patterns)
  eq(notes.result.nextCursor, nil)
end)

test("reads: the tidy's pages go on from their cursor, and the count adds up over them", function()
  local env, peer = setup({ patch = function(env_)
    for _, swap in ipairs({ { "M%.TIDY_READS = 300000", "M.TIDY_READS = 1" }, { "M%.TIDY_CALLS = 120000", "M.TIDY_CALLS = 1" } }) do
      local path = env_.bundle .. "payload/reads.lua"
      local f = assert(io.open(path, "rb"))
      local text = f:read("*a")
      f:close()
      local changed, count = text:gsub(swap[1], swap[2])
      assert(count == 1, swap[1])
      f = assert(io.open(path, "wb"))
      f:write(changed)
      f:close()
    end
  end })
  ok(#mock.data.patterns >= 2, "a song of two patterns or more")
  mock.ops.set_note(1, 1, 1, 1, { note = 48, instrument = 0 })
  mock.ops.set_note(2, 1, 1, 1, { note = 48, instrument = 0 })
  local first = call(peer, "read", { kind = "tidy-notes" })
  eq(first.result.to, 1)
  eq(first.result.nextCursor, "2")
  local second = call(peer, "read", { kind = "tidy-notes", cursor = "2" })
  eq(second.result.counts["0"], 1)
  local instruments = call(peer, "read", { kind = "tidy" })
  eq(#instruments.result.items, 1, "a page an instrument")
  eq(instruments.result.nextCursor, "2")
  local next_page = call(peer, "read", { kind = "tidy", cursor = "2" })
  eq(next_page.result.items[1].index, 1)
  failure(call(peer, "read", { kind = "tidy", cursor = "x" }), "bad-params")
end)

test("reads: two sounds padded to one length print apart, and one sound twice prints alike", function()
  local env, peer = setup()
  local samples = mock.data.instruments[1].samples
  -- (two different sounds in their first frames, the rest of each file silence)
  for s = 1, 2 do
    samples[s].buffer.frames = 2000
    samples[s].buffer.edits = {}
    for f = 60, 2000 do samples[s].buffer.edits[f] = 0 end
  end
  local items = call(peer, "read", { kind = "tidy" }).result.items
  ok(items[1].samples[1].print ~= items[1].samples[2].print, "different sounds: " .. json.encode(items[1].samples))
  samples[2].buffer.content = samples[1].buffer.content
  items = call(peer, "read", { kind = "tidy" }).result.items
  eq(items[1].samples[1].print, items[1].samples[2].print, "one sound twice")
end)

-- sample-load

local function wav_file(name, size)
  local path = mock.temp_dir .. "/" .. name .. ".wav"
  local f = assert(io.open(path, "wb"))
  f:write("RIFF\0\0\0\0WAVE" .. string.rep("w", (size or 256) - 12))
  f:close()
  return path
end

test("samples: lossy audio counts as what it decodes to against the load bound: a 9 MB MP3 is refused, a 9 MB WAV isn't (#361 N-S25)", function()
  local env, peer = setup()
  local mp3 = mock.temp_dir .. "/Mix.mp3"
  local f = assert(io.open(mp3, "wb"))
  f:write("ID3" .. string.rep("m", 9 * 1024 * 1024))
  f:close()
  local epoch = place(peer)
  local refused = call(peer, "change_preview", { kind = "sample-load", input = { ref = iref(epoch, 1), path = mp3 }, epoch = epoch })
  failure(refused, "failed")
  ok(refused.error.message:find("decodes", 1, true), refused.error.message)
  local fine = call(peer, "change_preview", { kind = "sample-load", input = { ref = iref(epoch, 1), path = wav_file("Big", 9 * 1024 * 1024) }, epoch = epoch })
  ok(fine.result, json.encode(fine.error or {}))
  os.remove(mp3)
end)

test("samples: Kumi's undo of a sample it loaded refuses once the producer retuned it; of an instrument it made, once they worked in it (#361 N-S1)", function()
  local env, peer = setup()
  local record = change(peer, "sample-load", { ref = iref(place(peer), 1), path = wav_file("Clap", 4096), index = 1 })
  mock.song:instrument(1):sample(2).transpose = 5 -- the producer retunes it
  failure(restore(peer, "sample-load", record), "changed", "slices or settings")
  eq(sample_names(1):sub(1, 10), "Kick,Clap,", "it stays")
  local made = change(peer, "instrument-add", { name = "Lead" })
  local lead = #mock.data.instruments
  mock.song:instrument(lead):macro(1).name = "Cutoff" -- the producer names a macro of it
  failure(restore(peer, "instrument-add", made), "changed", "macros")
  eq(#mock.data.instruments, lead, "Lead stays")
end)

test("samples: a sample file loads as a new sample at its place; Kumi's undo deletes it, refused once the producer changed it", function()
  local env, peer = setup()
  local record, done = change(peer, "sample-load", { ref = iref(place(peer), 1), path = wav_file("Clap"), index = 1 })
  eq(sample_names(1), "Kick,Clap,Snare,Hat")
  eq(sample(1, 2).buffer.frames, 256)
  eq(done.created.kind, "sample")
  ok(done.created.ref:match(":sample:1%.2$"), done.created.ref)
  eq(steps(), 1)
  local undone = restore(peer, "sample-load", record)
  ok(undone.result, json.encode(undone.error or {}))
  eq(sample_names(1), "Kick,Snare,Hat")
  record = change(peer, "sample-load", { ref = iref(place(peer), 1), path = wav_file("Clap") })
  sample(1, 4).name = "Mine"
  failure(restore(peer, "sample-load", record), "changed")
  eq(#instrument(1).samples, 4)
end)

test("samples: after Renoise's own undo and redo of a sample Kumi loaded, Kumi's undo deletes it; changed since: refused", function()
  for _, changed in ipairs({ false, true }) do
    local env, peer = setup()
    local record = change(peer, "sample-load", { ref = iref(place(peer), 1), path = wav_file("Clap"), index = 1 })
    mock.song:undo()
    eq(sample_names(1), "Kick,Snare,Hat")
    mock.song:redo()
    eq(sample_names(1), "Kick,Clap,Snare,Hat")
    if changed then sample(1, 2).name = "Mine" end
    local undone = restore(peer, "sample-load", record)
    if changed then
      failure(undone, "changed")
    else
      ok(undone.result, "the sample Renoise's redo made again is Kumi's: " .. json.encode(undone.error or {}))
      eq(sample_names(1), "Kick,Snare,Hat")
    end
  end
end)

test("samples: Kumi's undo of a replaced sample refuses once the producer edited its end in the sample editor (#361 N-N10)", function()
  local env, peer = setup()
  local record = change(peer, "sample-load", { ref = sref(place(peer), 1, 1), path = wav_file("Kick2", 4000) })
  local buffer = sample(1, 1).buffer
  buffer.edits = { [buffer.frames] = 0 } -- a fade on its last frames: between the probe points that were
  failure(restore(peer, "sample-load", record), "changed")
  eq(sample(1, 1).buffer.frames, 4000, "their edit stays")
end)

test("samples: a sample's audio replaced by a file is kept and put back by the undo, its name and loop too", function()
  local env, peer = setup()
  local before = json.encode(sample(1, 1).buffer, { sort_keys = true })
  sample(1, 1).loop_mode, sample(1, 1).loop_start, sample(1, 1).loop_end = 2, 10, 900
  local record, done = change(peer, "sample-load", { ref = sref(place(peer), 1, 1), path = wav_file("Kick2", 4000) })
  eq(sample(1, 1).buffer.frames, 4000)
  ok(done.material.old.file and io.open(done.material.old.file, "rb"), "the old audio is kept in a file")
  local undone = restore(peer, "sample-load", record)
  ok(undone.result, json.encode(undone.error or {}))
  eq(json.encode(sample(1, 1).buffer, { sort_keys = true }), before, "the same audio")
  eq(sample(1, 1).name, "Kick")
  eq(sample(1, 1).loop_start, 10)
  eq(sample(1, 1).loop_end, 900)
  eq(sample(1, 1).loop_mode, 2)
  local again = restore(peer, "sample-load", record)
  ok(again.result, json.encode(again.error or {}))
  ok(again.result.display.title:find("back already", 1, true), again.result.display.title)
end)

-- sample-delete

test("samples: a deleted sample comes back after the one that was before it, with its audio, settings, keyzone and set", function()
  local env, peer = setup(nil, { instrument_specs = {
    { name = "Drums", sets = 1, sample_specs = { { name = "Kick", frames = 1000 }, { name = "Snare", frames = 800, volume = 0.5, mute_group = 4, modulation_set_index = 1, note_range = { 38, 38 }, base_note = 38 }, { name = "Hat", frames = 300 } } },
    { name = "Bass synth" }, { name = "Pad" } } })
  local snare = json.encode(sample(1, 2).buffer, { sort_keys = true })
  local record, done = change(peer, "sample-delete", { ref = sref(place(peer), 1, 2) }, "Kumi: del (k520)")
  eq(sample_names(1), "Kick,Hat")
  ok(done.material.file, "its audio kept")
  mock.song:instrument(1):insert_sample_at(1).name = "New"
  eq(sample_names(1), "New,Kick,Hat")
  local undone = restore(peer, "sample-delete", record)
  ok(undone.result, json.encode(undone.error or {}))
  eq(sample_names(1), "New,Kick,Snare,Hat")
  local back = sample(1, 3)
  eq(json.encode(back.buffer, { sort_keys = true }), snare)
  ok(math.abs(back.volume - 0.5) < 1e-9)
  eq(back.mute_group, 4)
  eq(back.modulation_set_index, 1)
  eq(back.note_range[1], 38)
  eq(back.base_note, 38)
  local again = restore(peer, "sample-delete", record)
  ok(again.result, json.encode(again.error or {}))
  ok(again.result.display.title:find("back already", 1, true))
end)

test("samples: an instrument keeps its name when Kumi deletes its last sample (Renoise would leave it unnamed: lab), or undoes the load of it", function()
  local env, peer = setup()
  local record = change(peer, "sample-delete", { ref = sref(place(peer), 2, 1) })
  eq(#instrument(2).samples, 0)
  eq(instrument(2).name, "Bass synth")
  ok(restore(peer, "sample-delete", record).result)
  eq(instrument(2).name, "Bass synth")
  instrument(3).name = "Empty"
  local loaded = change(peer, "sample-load", { ref = iref(place(peer), 3), path = wav_file("Clap") })
  ok(restore(peer, "sample-load", loaded).result)
  eq(#instrument(3).samples, 0)
  eq(instrument(3).name, "Empty")
  -- (Renoise's own delete unnames it: the mock does as Renoise)
  mock.song:instrument(2):delete_sample_at(1)
  eq(instrument(2).name, "")
end)

test("samples: a slice can't be deleted (its marker can); the sliced sample's delete keeps its markers and each slice's settings", function()
  local env, peer = setup()
  local master = mock.song:instrument(1):sample(1)
  master:insert_slice_marker(200)
  master:insert_slice_marker(600)
  sample(1, 3).volume = 0.25 -- the second slice
  mock.data.undo.stack = {}
  local epoch = place(peer)
  failure(call(peer, "change_preview", { kind = "sample-delete", input = { ref = sref(epoch, 1, 2) }, epoch = epoch }), "bad-params")
  local record = change(peer, "sample-delete", { ref = sref(epoch, 1, 1) })
  eq(sample_names(1), "", "the slices went with it")
  local undone = restore(peer, "sample-delete", record)
  ok(undone.result, json.encode(undone.error or {}))
  eq(#instrument(1).samples, 3)
  eq(sample(1, 1).markers[2], 600)
  ok(math.abs(sample(1, 3).volume - 0.25) < 1e-9, "the slice's own settings")
end)

test("samples: the undo of a sliced sample's delete refuses once the producer put a sample into the instrument (#361 N-S21)", function()
  local env, peer = setup()
  local master = mock.song:instrument(1):sample(1)
  master:insert_slice_marker(200)
  master:insert_slice_marker(600)
  mock.data.undo.stack = {}
  local record = change(peer, "sample-delete", { ref = sref(place(peer), 1, 1) })
  eq(sample_names(1), "")
  mock.song:instrument(1):insert_sample_at(1).name = "Vocal" -- the producer drops a vocal in
  H.tick(2)
  local refused = restore(peer, "sample-delete", record)
  failure(refused, "changed")
  ok(refused.error.message:find("holds other samples", 1, true), refused.error.message)
  eq(sample_names(1), "Vocal", "the producer's sample stays, alone")
end)

-- sample-slices

test("samples: slice markers inserted, moved and deleted through Renoise's calls; the undo puts the markers and a deleted slice's settings back", function()
  local env, peer = setup()
  local record, done = change(peer, "sample-slices", { ref = sref(place(peer), 1, 1), insert = { 250, 500, 750 } })
  eq(#instrument(1).samples, 4, "three slices")
  eq(mock.slices_remapped > 0, true, "through the calls that keep the notes on their slices")
  ok(done.display.title:find("3 added", 1, true), done.display.title)
  local undone = restore(peer, "sample-slices", record)
  ok(undone.result, json.encode(undone.error or {}))
  eq(#instrument(1).samples, 1)
  change(peer, "sample-slices", { ref = sref(place(peer), 1, 1), insert = { 250, 500 } })
  sample(1, 3).volume = 0.3 -- the slice at 500
  mock.data.undo.stack = {}
  record = change(peer, "sample-slices", { ref = sref(place(peer), 1, 1), delete = { 500 }, move = { { from = 250, to = 300 } } })
  eq(#instrument(1).samples, 2)
  eq(sample(1, 1).markers[1], 300)
  undone = restore(peer, "sample-slices", record)
  ok(undone.result, json.encode(undone.error or {}))
  eq(sample(1, 1).markers[1], 250)
  eq(sample(1, 1).markers[2], 500)
  ok(math.abs(sample(1, 3).volume - 0.3) < 1e-9, "the deleted slice's volume is back")
  record = change(peer, "sample-slices", { ref = sref(place(peer), 1, 1), insert = { 100 } })
  mock.song:instrument(1):sample(1):insert_slice_marker(900) -- the producer's
  failure(restore(peer, "sample-slices", record), "changed")
  local epoch = place(peer)
  failure(call(peer, "change_preview", { kind = "sample-slices", input = { ref = sref(epoch, 1, 2), insert = { 10 } }, epoch = epoch }), "bad-params")
  failure(call(peer, "change_preview", { kind = "sample-slices", input = { ref = sref(epoch, 1, 1), delete = { 123 } }, epoch = epoch }), "bad-params")
end)
