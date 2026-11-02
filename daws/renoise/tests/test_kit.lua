-- Kumi's instrument made from sample files (M6, m6-library): a drum kit and a keys instrument as one change, its hi-hats in one
-- mute group, into a new instrument or an empty one; put back by Kumi's undo; refused when changed, played or gone; the files
-- checked before Renoise sees them; the fence; and the instrument's place followed through inserts, deletes and swaps.
local T = ...
local mock = T.mock
local json = T.load_payload_module("json")
local test, eq, ok, same = T.test, T.eq, T.ok, T.same
local H = T.harness
local I = T.instruments
local setup, place, preview, apply, change, restore, failure, call = I.setup, I.place, I.preview, I.apply, I.change, I.restore, I.failure, I.call
local steps, newest, instrument, names, iref, note_at = I.steps, I.newest, I.instrument, I.names, I.iref, I.note_at

local function told(peer)
  H.tick(3)
  repeat
    local taken = peer:take()
    if #taken > 0 then H.send(peer, { type = "credit", bytes = #taken }) end
  until #taken == 0
  return H.events(peer, "undone_in_daw")
end

-- A sample file the mock reads as audio: a WAVE header and `size` bytes.
local function wav(name, size)
  local path = mock.temp_dir .. "/" .. name .. ".wav"
  local f = assert(io.open(path, "wb"))
  f:write("RIFF\0\0\0\0WAVE" .. string.rep("x", size or 200))
  f:close()
  return path
end

local function kit_input(extra)
  local input = {
    samples = {
      { path = wav("Kick 01"), name = "Kick" }, { path = wav("Snare 02"), name = "Snare" },
      { path = wav("HH Closed") }, { path = wav("Open Hat 1") },
    },
    name = "Tight Kit",
  }
  for k, v in pairs(extra or {}) do input[k] = v end
  return input
end

local function zone(s) return { s.note_range[1], s.note_range[2], s.base_note, s.velocity_range[1], s.velocity_range[2], s.mute_group } end

test("kits: a drum kit is one step: a sample a key from C-4, named, its hi-hats in mute group 1; Kumi's undo takes it away", function()
  local env, peer = setup()
  local previewed = preview(peer, "instrument-kit", kit_input())
  ok(previewed.display.title:find("Made drum kit \226\128\156Tight Kit\226\128\157 from 4 samples; its hi-hats choke each other", 1, true), previewed.display.title)
  eq(previewed.display.note, "Keys: C-4 Kick, C#4 Snare, D-4 HH Closed, D#4 Open Hat 1. The hi-hats share mute group 1: keep them in one track for the choke.")
  eq(steps(), 0, "a preview writes nothing")
  eq(#mock.data.instruments, 3)
  local record, done = change(peer, "instrument-kit", kit_input(), "Kumi: kit (k700)")
  eq(names(), "Drums,Bass synth,Pad,Tight Kit")
  eq(steps(), 1)
  eq(newest(), "Kumi: kit (k700)")
  eq(done.created.kind, "instrument")
  ok(done.created.ref:match(":instrument:4$"), done.created.ref)
  local kit = instrument(4)
  eq(#kit.samples, 4)
  eq(kit.samples[1].name, "Kick")
  eq(kit.samples[3].name, "HH Closed", "a sample without a name takes its file's")
  same(zone(kit.samples[1]), { 48, 48, 48, 0, 127, 0 })
  same(zone(kit.samples[2]), { 49, 49, 49, 0, 127, 0 })
  same(zone(kit.samples[3]), { 50, 50, 50, 0, 127, 1 })
  same(zone(kit.samples[4]), { 51, 51, 51, 0, 127, 1 })
  eq(kit.samples[1].buffer.frames, 212)
  eq(mock.dialogs, 0)
  local undone = restore(peer, "instrument-kit", record)
  ok(undone.result, json.encode(undone.error or {}))
  eq(names(), "Drums,Bass synth,Pad")
  eq(steps(), 2, "Kumi's undo is a step of its own")
  failure(restore(peer, "instrument-kit", record), "gone")
end)

test("kits: notes, first, white keys, a group named and the choke turned off are kept; the kit goes at index", function()
  local env, peer = setup()
  local input = kit_input({ first = 36, whiteKeys = true, index = 0 })
  input.samples[2].note = 37
  local record = change(peer, "instrument-kit", input)
  eq(names(), "Tight Kit,Drums,Bass synth,Pad")
  local kit = instrument(1)
  -- (Kick at 36; the Snare's own 37; the hats on the next white keys: 38, 40)
  same({ kit.samples[1].note_range[1], kit.samples[2].note_range[1], kit.samples[3].note_range[1], kit.samples[4].note_range[1] }, { 36, 37, 38, 40 })
  ok(restore(peer, "instrument-kit", record).result)
  input = kit_input({ chokeHats = false })
  change(peer, "instrument-kit", input)
  same({ instrument(4).samples[3].mute_group, instrument(4).samples[4].mute_group }, { 0, 0 }, "the choke turned off")
  input = kit_input()
  input.samples[1].muteGroup = 4
  change(peer, "instrument-kit", input)
  same({ instrument(5).samples[1].mute_group, instrument(5).samples[3].mute_group }, { 4, 0 }, "a group named: no hats choked for it")
end)

test("kits: a keys instrument splits the keyboard between roots, layers velocities on one root, and sets the overlap", function()
  local env, peer = setup()
  local input = {
    layout = "keys", overlap = "all",
    samples = {
      { path = wav("Piano C3"), root = 36 }, { path = wav("Piano C4 soft"), root = 48, velocity = { 0, 63 } },
      { path = wav("Piano C4 hard"), root = 48, velocity = { 64, 127 }, volumeDb = -3 }, { path = wav("Piano C5"), root = 60 },
    },
  }
  local record, done = change(peer, "instrument-kit", input)
  ok(done.display.title:find("Made instrument \226\128\156Piano C3\226\128\157 from 4 samples", 1, true), done.display.title)
  local kit = instrument(4)
  eq(kit.name, "Piano C3", "named after its first sample")
  same(zone(kit.samples[1]), { 0, 42, 36, 0, 127, 0 })
  same(zone(kit.samples[2]), { 43, 54, 48, 0, 63, 0 })
  same(zone(kit.samples[3]), { 43, 54, 48, 64, 127, 0 })
  same(zone(kit.samples[4]), { 55, 119, 60, 0, 127, 0 })
  ok(math.abs(kit.samples[3].volume - 10 ^ (-3 / 20)) < 1e-6)
  eq(kit.overlap, 0)
  ok(restore(peer, "instrument-kit", record).result)
  local epoch = place(peer)
  failure(call(peer, "change_preview", { kind = "instrument-kit", input = { layout = "keys", samples = { { path = wav("No Root") } } }, epoch = epoch }), "bad-params")
end)

test("kits: into an empty instrument (its name kept), refused into one that isn't; Kumi's undo empties it, then it's back already", function()
  local env, peer = setup()
  local epoch = place(peer)
  local input = kit_input({ ref = iref(epoch, 3) })
  input.name = nil
  local record = change(peer, "instrument-kit", input)
  eq(#mock.data.instruments, 3, "no instrument made")
  eq(instrument(3).name, "Pad")
  eq(#instrument(3).samples, 4)
  local undone = restore(peer, "instrument-kit", record)
  ok(undone.result, json.encode(undone.error or {}))
  eq(#instrument(3).samples, 0)
  local again = restore(peer, "instrument-kit", record)
  ok(again.result, json.encode(again.error or {}))
  eq(again.result.display.title, "\226\128\156Pad\226\128\157 is empty already")
  epoch = place(peer)
  failure(call(peer, "change_preview", { kind = "instrument-kit", input = kit_input({ ref = iref(epoch, 1) }), epoch = epoch }), "bad-params")
  failure(call(peer, "change_preview", { kind = "instrument-kit", input = kit_input({ ref = iref(epoch, 3), index = 0 }), epoch = epoch }), "bad-params")
end)

test("kits: a file that isn't audio, a missing one or too many samples are refused before Renoise sees them; one Renoise refuses leaves nothing", function()
  local env, peer = setup()
  local epoch = place(peer)
  local function refused(input) failure(call(peer, "change_preview", { kind = "instrument-kit", input = input, epoch = epoch }), "bad-params") end
  local fake = mock.temp_dir .. "/fake.wav"
  local f = assert(io.open(fake, "wb"))
  f:write("this is not a wave file at all")
  f:close()
  refused({ samples = { { path = fake } } })
  refused({ samples = { { path = "/nowhere/kick.wav" } } })
  refused({ samples = { { path = wav("Kick"), note = 120 } } })
  refused({ samples = {} })
  local many = {}
  for k = 1, 121 do many[k] = { path = wav("Kick") } end
  refused({ samples = many })
  local crowded = {}
  for k = 1, 20 do crowded[k] = { path = wav("Kick") } end
  refused({ samples = crowded, first = 110 })
  eq(mock.dialogs, 0, "nothing asked of Renoise that would show a dialog")
  local broken = mock.temp_dir .. "/Broken.wav"
  f = assert(io.open(broken, "wb"))
  f:write("RIFF\0\0\0\0WAVE broken inside, though it starts right")
  f:close()
  local input = { samples = { { path = wav("Kick") }, { path = broken } } }
  local previewed, epoch2, layout = preview(peer, "instrument-kit", input)
  failure(apply(peer, "instrument-kit", input, "Kumi: kit (k1)", previewed.fence, epoch2, layout), "failed")
  eq(#mock.data.instruments, 3, "the new instrument went again")
end)

test("kits: the apply refuses changed when the instruments changed after the preview (the fence)", function()
  local env, peer = setup()
  local input = kit_input()
  local previewed, epoch, layout = preview(peer, "instrument-kit", input)
  mock.song:insert_instrument_at(1)
  H.tick(2)
  failure(apply(peer, "instrument-kit", input, "Kumi: kit (k2)", previewed.fence, epoch, layout), "changed")
  eq(#mock.data.instruments, 4, "nothing written")
  -- An empty instrument filled by the producer after the preview: refused too.
  env, peer = setup()
  epoch = place(peer)
  input = kit_input({ ref = iref(epoch, 3) })
  previewed, epoch, layout = preview(peer, "instrument-kit", input)
  mock.ops.insert_sample(3, { name = "Mine", frames = 10 })
  failure(apply(peer, "instrument-kit", input, "Kumi: kit (k3)", previewed.fence, epoch, layout), "changed")
  eq(#instrument(3).samples, 1)
end)

test("kits: Kumi's undo refuses once the producer changed a keyzone, a mute group or a sample, or plays the kit; discard takes it", function()
  local env, peer = setup()
  local record = change(peer, "instrument-kit", kit_input())
  instrument(4).samples[3].mute_group = 0
  failure(restore(peer, "instrument-kit", record), "changed")
  instrument(4).samples[3].mute_group = 1
  instrument(4).samples[1].note_range = { 47, 48 }
  failure(restore(peer, "instrument-kit", record), "changed")
  instrument(4).samples[1].note_range = { 48, 48 }
  mock.ops.insert_sample(4, { name = "Mine", frames = 10 })
  failure(restore(peer, "instrument-kit", record), "changed")
  eq(#mock.data.instruments, 4)
  env, peer = setup()
  record = change(peer, "instrument-kit", kit_input())
  note_at(5, 3) -- the producer plays the kit (value 3: the fourth instrument)
  local refused = restore(peer, "instrument-kit", record)
  failure(refused, "changed")
  ok(refused.error.message:find("play", 1, true), refused.error.message)
  local forced = restore(peer, "instrument-kit", record, nil, { discard = true })
  ok(forced.result, json.encode(forced.error or {}))
  eq(#mock.data.instruments, 3)
end)

test("places, kit × insert before, delete before, swap: Kumi's undo follows the kit; deleted: gone; the producer's undo of it is told", function()
  -- An instrument made before it.
  local env, peer = setup()
  local record = change(peer, "instrument-kit", kit_input())
  mock.song:insert_instrument_at(1)
  H.tick(2)
  ok(restore(peer, "instrument-kit", record).result)
  eq(names(), ",Drums,Bass synth,Pad")
  -- One deleted before it.
  env, peer = setup()
  record = change(peer, "instrument-kit", kit_input())
  mock.song:delete_instrument_at(1)
  H.tick(2)
  ok(restore(peer, "instrument-kit", record).result)
  eq(names(), "Bass synth,Pad")
  -- Swapped with another.
  env, peer = setup()
  record = change(peer, "instrument-kit", kit_input())
  mock.song:swap_instruments_at(2, 4)
  H.tick(2)
  ok(restore(peer, "instrument-kit", record).result)
  eq(names(), "Drums,Pad,Bass synth")
  -- The kit deleted: gone, nothing touched.
  env, peer = setup()
  record = change(peer, "instrument-kit", kit_input())
  mock.song:delete_instrument_at(4)
  H.tick(2)
  failure(restore(peer, "instrument-kit", record), "gone")
  eq(names(), "Drums,Bass synth,Pad")
  -- The producer's undo of the kit: told, and Kumi's undo after it is gone.
  env, peer = setup()
  record = change(peer, "instrument-kit", kit_input(), "Kumi: kit (k400)")
  told(peer)
  mock.song:undo()
  local events = told(peer)
  eq(#events, 1)
  eq(events[1].data.descs[1], "Kumi: kit (k400)")
  failure(restore(peer, "instrument-kit", record), "gone")
end)

test("kits: Kumi's undo of a kit made into an empty instrument empties it again whatever notes played it before (#361 N-S11)", function()
  local env, peer = setup()
  note_at(1, 2) -- the producer's beat for empty instrument 02 (Pad), written before the kit
  note_at(5, 2)
  local input = kit_input({ ref = iref(place(peer), 3) })
  input.name = nil
  local record = change(peer, "instrument-kit", input)
  eq(#instrument(3).samples, 4)
  local undone = restore(peer, "instrument-kit", record)
  ok(undone.result, json.encode(undone.error or {}))
  eq(#instrument(3).samples, 0, "emptied again")
  eq(I.note_value(1), 2, "the producer's notes stay")
end)

test("kits: Kumi's undo gives an empty instrument back its own settings, which clear() would reset, or says it can't (#361 N-S12)", function()
  local env, peer = setup()
  local function slot() return mock.song:instrument(3) end
  slot().volume, slot().transpose = 0.5, 3
  slot().midi_output_properties.channel = 5
  local input = kit_input({ ref = iref(place(peer), 3) })
  input.name = nil
  local record = change(peer, "instrument-kit", input)
  eq(#instrument(3).samples, 4)
  local undone = restore(peer, "instrument-kit", record)
  ok(undone.result, json.encode(undone.error or {}))
  eq(undone.result.display.note, nil)
  eq(#instrument(3).samples, 0)
  eq(slot().volume, 0.5, "its volume")
  eq(slot().transpose, 3, "its transpose")
  eq(slot().midi_output_properties.channel, 5, "its MIDI channel")
  eq(slot().name, "Pad", "its name, not the file's")
  eq(#mock.data.instruments, 3, "and no instrument more")
  -- Without the file (Renoise emptied its temporary folder as it quit), it's emptied and that's said.
  record = change(peer, "instrument-kit", kit_input({ ref = iref(place(peer), 3), name = "Kit" }))
  os.remove(record.material.slot)
  undone = restore(peer, "instrument-kit", record)
  ok(undone.result, json.encode(undone.error or {}))
  ok((undone.result.display.note or ""):find("settings", 1, true), tostring(undone.result.display.note))
  eq(slot().volume, 1, "reset")
  eq(slot().name, "Pad")
end)

test("kits: an instrument with a MIDI output device isn't empty, so no kit goes into it (#361 N-S12)", function()
  local env, peer = setup()
  mock.midi_outputs = { "IAC Driver Bus 1" }
  mock.song:instrument(3).midi_output_properties.device_name = "IAC Driver Bus 1"
  mock.midi_outputs = nil
  local epoch = place(peer)
  failure(call(peer, "change_preview", { kind = "instrument-kit", input = kit_input({ ref = iref(epoch, 3) }), epoch = epoch }), "bad-params")
  eq(#instrument(3).samples, 0)
end)
