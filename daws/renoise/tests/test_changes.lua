-- Kumi's changes to a Renoise song (M3): preview, apply, restore, the producer's undo noticed, the emergency stop. Against the
-- mock Renoise, which keeps an undo step for every callback that writes (and none for mute, solo, loop or selection).
local T = ...
local mock = T.mock
local json = T.load_payload_module("json")
local test, eq, ok = T.test, T.eq, T.ok
local H = T.harness

local next_id = 5000

local function drain(peer)
  repeat
    local taken = peer:take()
    if #taken > 0 then H.send(peer, { type = "credit", bytes = #taken }) end
  until #taken == 0
end

-- Sends a request and runs the clock until its response comes; the events that came meanwhile are kept in `peer.events`.
local function call(peer, method, params, ticks)
  next_id = next_id + 1
  local id = next_id
  H.send(peer, { type = "request", id = id, method = method, params = params or {} })
  peer.events = peer.events or {}
  for _ = 1, ticks or 400 do
    drain(peer)
    for _, line in ipairs(peer:frames(nil)) do
      local frame = assert(json.decode(line))
      if frame.type == "response" and frame.id == id then return frame end
      if frame.type == "event" then peer.events[#peer.events + 1] = frame end
    end
    mock.advance(10)
  end
end

-- A song of five tracks (Kick, Hat, Bass, Pad, a group) with the mixer device every Renoise track has, a master and a send.
local function song_options(extra)
  local spec = {
    name = "Lab", file_name = "/s/lab.xrns", bpm = 120, lpb = 4, mixer_device = true,
    tracks = {
      { name = "Kick", color = { 255, 0, 0 } }, { name = "Hat" }, { name = "Bass", note_columns = 2 }, { name = "Pad" },
    },
    send_tracks = 1, patterns = 3, pattern_lines = 32, sequence_length = 4, sequence = { 1, 2, 1, 3 }, instruments = 1,
  }
  for k, v in pairs(extra or {}) do spec[k] = v end
  return spec
end

local function setup(options, extra)
  options = options or {}
  options.song = song_options(extra)
  local env = H.setup(options)
  local peer = H.connect(env)
  H.frames(peer)
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  return env, peer
end

local function place(peer)
  local focus = call(peer, "focus").result
  return focus.epoch, focus.layout
end

local function track_ref(epoch, i) return epoch .. ":track:" .. i end

local function ask(peer, method, kind, input, more)
  local epoch, layout = place(peer)
  local params = { kind = kind, input = input, epoch = epoch, layout = layout }
  for k, v in pairs(more or {}) do params[k] = v end
  return call(peer, method, params), epoch, layout
end

local function preview(peer, kind, input)
  local response, epoch, layout = ask(peer, "change_preview", kind, input)
  ok(response and response.result, kind .. " preview: " .. json.encode(response and response.error or {}))
  return response.result, epoch, layout
end

-- The step count of the song's undo, and the newest step's name.
local function steps() return #mock.data.undo.stack end
local function newest() return mock.data.undo.stack[#mock.data.undo.stack] and mock.data.undo.stack[#mock.data.undo.stack].desc end

local function apply(peer, kind, input, desc, fence, epoch, layout)
  return call(peer, "change_apply", { kind = kind, input = input, fence = fence, desc = desc, epoch = epoch, layout = layout })
end

local function restore(peer, kind, record, desc, epoch, layout, more)
  local params = {
    kind = kind, target = record.target, prior = record.prior, applied = record.applied, created = record.created,
    material = record.material, desc = desc, epoch = epoch, layout = layout,
  }
  for k, v in pairs(more or {}) do params[k] = v end
  return call(peer, "change_restore", params)
end

local function failure(response, code)
  ok(response and response.error, "expected an error")
  eq(response.error.code, code, "error code (" .. tostring(response.error.message) .. ")")
end

local function track_data(i) return mock.data.tracks[i] end
-- (a track's post-FX volume: the fader track-volume moves unless asked for the pre-FX stage)
local function post_volume(i) return mock.data.tracks[i].post_volume or 1 end

-- Preview, apply

test("changes: a preview writes nothing and says what a change would do, with true minus signs", function()
  local env, peer = setup()
  local before = steps()
  local result, epoch, layout = preview(peer, "track-volume", { ref = track_ref(place(peer), 1), volumeDb = -6 })
  eq(result.epoch, epoch)
  eq(result.layout, layout)
  eq(result.target.ref, track_ref(epoch, 1))
  eq(result.target.kind, "track")
  eq(result.prior.volume, 1)
  eq(result.prior.stage, "post", "the post-FX fader, the one Renoise's mixer shows")
  eq(result.fence.volume, 1)
  eq(result.display.title, "Kick volume \226\136\1466 dB")
  eq(result.display.unchanged, nil)
  eq(steps(), before, "no undo step")
  eq(post_volume(1), 1, "nothing written")
  local same = preview(peer, "track-volume", { ref = track_ref(epoch, 1), volumeDb = 0 })
  eq(same.display.unchanged, true)
  eq(same.display.title, "Kick volume 0 dB")
  H.no_dialogs()
end)

test("changes: an apply makes one undo step named by the host's description and reads back what it wrote", function()
  local env, peer = setup()
  local epoch = place(peer)
  local ref = track_ref(epoch, 2)
  local previewed, _, layout = preview(peer, "track-volume", { ref = ref, volumeDb = -6 })
  local done = apply(peer, "track-volume", { ref = ref, volumeDb = -6 }, "Kumi: Hat volume \226\136\1466 dB (k7f3)", previewed.fence, epoch, layout)
  ok(done.result, json.encode(done.error or {}))
  eq(done.result.epoch, epoch)
  eq(done.result.layout, layout)
  ok(math.abs(done.result.applied.volume - 10 ^ (-6 / 20)) < 1e-6)
  eq(done.result.undo.desc, "Kumi: Hat volume \226\136\1466 dB (k7f3)")
  eq(done.result.undo.entry, true)
  eq(done.result.display.title, "Hat volume \226\136\1466 dB")
  eq(steps(), 1, "one step for the whole change")
  eq(newest(), "Kumi: Hat volume \226\136\1466 dB (k7f3)")
  ok(math.abs(post_volume(2) - 10 ^ (-6 / 20)) < 1e-6, "the track has it")
  eq(track_data(2).volume, 1, "on its post-FX fader: the pre-FX volume is as it was")
  H.no_dialogs()
end)

test("changes: an apply is refused before anything is written when the fence fails, the epoch is old or Renoise is busy", function()
  local env, peer = setup()
  local epoch = place(peer)
  local ref = track_ref(epoch, 1)
  local previewed, _, layout = preview(peer, "track-volume", { ref = ref, volumeDb = -6 })
  -- the producer moves the fader first
  mock.ops.rename_track(1, "Kick") -- (a bystander)
  track_data(1).post_volume = 0.5
  local changed = apply(peer, "track-volume", { ref = ref, volumeDb = -6 }, "Kumi: x (k1)", previewed.fence, epoch, layout)
  failure(changed, "changed")
  eq(post_volume(1), 0.5, "left as the producer had it")
  eq(steps(), 0)
  track_data(1).post_volume = 1
  -- an old epoch
  local stale = apply(peer, "track-volume", { ref = ref, volumeDb = -6 }, "Kumi: x (k1)", previewed.fence, "0", layout)
  failure(stale, "stale")
  -- busy
  mock.data.song_fields.rendering = true
  local busy = apply(peer, "track-volume", { ref = ref, volumeDb = -6 }, "Kumi: x (k1)", previewed.fence, epoch, layout)
  failure(busy, "busy")
  mock.data.song_fields.rendering = false
  eq(steps(), 0)
  eq(post_volume(1), 1)
  H.no_dialogs()
end)

test("changes: bad inputs are bad-params, the master has no name or solo, and an unknown kind is refused", function()
  local env, peer = setup()
  local epoch = place(peer)
  local function bad(kind, input)
    local response = ask(peer, "change_preview", kind, input)
    failure(response, "bad-params")
  end
  bad("track-volume", { ref = track_ref(epoch, 1), volumeDb = "loud" })
  bad("track-volume", { ref = track_ref(epoch, 1), volumeDb = 40 })
  bad("track-pan", { ref = track_ref(epoch, 1), pan = 2 })
  bad("track-mute", { ref = track_ref(epoch, 1), mute = "yes" })
  bad("track-color", { ref = track_ref(epoch, 1), color = "red" })
  bad("track-rename", { ref = track_ref(epoch, 1), name = "two\nlines" })
  bad("track-rename", { ref = track_ref(epoch, 5), name = "Master" }) -- (the master is the fifth: four tracks, then it)
  bad("track-solo", { ref = track_ref(epoch, 5), solo = true })
  bad("track-delete", { ref = track_ref(epoch, 5) })
  bad("track-fly", { ref = track_ref(epoch, 1) })
  bad("track-volume", { ref = "not a ref", volumeDb = -3 })
  failure(call(peer, "change_preview", { kind = "track-volume", input = { ref = track_ref(epoch, 1), volumeDb = -3 } }), "bad-params")
  H.no_dialogs()
end)

test("changes: pan, rename and colour change and put back, each as one step", function()
  local env, peer = setup()
  local epoch = place(peer)
  local function round_trip(kind, input, read, expected, original)
    local ref = input.ref
    local previewed, _, layout = preview(peer, kind, input)
    local before = steps()
    local done = apply(peer, kind, input, "Kumi: " .. kind .. " (k" .. kind:len() .. ")", previewed.fence, epoch, layout)
    ok(done.result, kind .. ": " .. json.encode(done.error or {}))
    eq(steps(), before + 1, kind .. ": one step")
    eq(read(), expected, kind .. " written")
    local undone = restore(peer, kind, {
      target = previewed.target, prior = previewed.prior, applied = done.result.applied,
    }, "Kumi: undo " .. kind .. " (k9)", epoch, layout)
    ok(undone.result, kind .. " restore: " .. json.encode(undone.error or {}))
    eq(read(), original, kind .. " put back")
    eq(steps(), before + 2, kind .. ": the restore is a new step")
    eq(newest(), "Kumi: undo " .. kind .. " (k9)")
    return previewed, done.result
  end
  local _, pan = round_trip("track-pan", { ref = track_ref(epoch, 3), pan = -0.5, stage = "pre" }, function() return track_data(3).panning end, 0.25, 0.5)
  eq(pan.display.title, "Bass pre-FX pan 50% left", "the title names the pre-FX stage")
  local _, rename = round_trip("track-rename", { ref = track_ref(epoch, 4), name = "Pad 2" }, function() return track_data(4).name end, "Pad 2", "Pad")
  eq(rename.display.title, "Renamed track “Pad” → “Pad 2”")
  local _, colour = round_trip("track-color", { ref = track_ref(epoch, 2), color = "#336699" }, function()
    local c = track_data(2).color
    return c and string.format("#%02x%02x%02x", c[1], c[2], c[3])
  end, "#336699", "#29a699")
  eq(colour.display.title, "Hat colour changed")
  H.no_dialogs()
end)

test("changes: mute and solo are not Renoise undo steps, say so, and are put back by Kumi", function()
  local env, peer = setup()
  local epoch = place(peer)
  local ref = track_ref(epoch, 1)
  local previewed, _, layout = preview(peer, "track-mute", { ref = ref, mute = true })
  ok(previewed.display.note:find("Renoise's own undo won't take this back", 1, true), previewed.display.note)
  local done = apply(peer, "track-mute", { ref = ref, mute = true }, "Kumi: Kick muted (k2)", previewed.fence, epoch, layout)
  eq(done.result.undo.entry, false)
  ok(done.result.display.note)
  eq(steps(), 0, "no step in Renoise")
  eq(track_data(1).mute_state, 3)
  local undone = restore(peer, "track-mute", { target = previewed.target, prior = previewed.prior, applied = done.result.applied }, "Kumi: undo (k3)", epoch, layout)
  eq(undone.result.undo.entry, false)
  eq(track_data(1).mute_state, 1)
  local solo = preview(peer, "track-solo", { ref = ref, solo = true })
  local soloed = apply(peer, "track-solo", { ref = ref, solo = true }, "Kumi: Kick soloed (k4)", solo.fence, epoch, layout)
  eq(soloed.result.undo.entry, false)
  eq(track_data(1).solo, true)
  eq(steps(), 0)
  H.no_dialogs()
end)

test("changes: a restore refuses `changed` when the producer moved the value, accepts discard, and a value already back is not written", function()
  local env, peer = setup()
  local epoch = place(peer)
  local ref = track_ref(epoch, 1)
  local previewed, _, layout = preview(peer, "track-rename", { ref = ref, name = "Boom" })
  local done = apply(peer, "track-rename", { ref = ref, name = "Boom" }, "Kumi: rename (k5)", previewed.fence, epoch, layout)
  local record = { target = previewed.target, prior = previewed.prior, applied = done.result.applied }
  mock.ops.rename_track(1, "Producer's")
  local before = steps()
  failure(restore(peer, "track-rename", record, "Kumi: undo (k6)", epoch, layout), "changed")
  eq(track_data(1).name, "Producer's")
  eq(steps(), before)
  local forced = restore(peer, "track-rename", record, "Kumi: undo (k6)", epoch, layout, { discard = true })
  ok(forced.result, json.encode(forced.error or {}))
  eq(track_data(1).name, "Kick")
  -- a retry of a restore that went through writes nothing
  local after = steps()
  local again = restore(peer, "track-rename", record, "Kumi: undo (k6)", epoch, layout)
  ok(again.result)
  eq(steps(), after, "nothing written, no step")
  -- a track that is gone
  mock.ops.remove_track(1)
  failure(restore(peer, "track-rename", record, "Kumi: undo (k7)", epoch, layout), "gone")
  H.no_dialogs()
end)

test("changes: refs made in an earlier layout are carried to where the tracks went", function()
  local env, peer = setup()
  local epoch, layout = place(peer)
  local ref = track_ref(epoch, 3) -- Bass
  mock.ops.insert_track(1, { name = "New" })
  local response = call(peer, "change_preview", { kind = "track-rename", input = { ref = ref, name = "Bass 2" }, epoch = epoch, layout = layout })
  ok(response.result, json.encode(response.error or {}))
  eq(response.result.target.ref, track_ref(epoch, 4), "a request that names the old layout is carried through the new track: Bass is the fourth")
  eq(response.result.layout, layout + 1)
  eq(response.result.prior.name, "Bass")
  H.no_dialogs()
end)

-- The producer's undo

test("changes: the producer's undo and redo of a Kumi change are told, once, by the description", function()
  local env, peer = setup()
  local epoch = place(peer)
  local ref = track_ref(epoch, 1)
  local previewed, _, layout = preview(peer, "track-volume", { ref = ref, volumeDb = -6 })
  local desc = "Kumi: Kick volume \226\136\1466 dB (k7f3)"
  apply(peer, "track-volume", { ref = ref, volumeDb = -6 }, desc, previewed.fence, epoch, layout)
  H.tick(2)
  peer.events = {}
  mock.song:undo()
  H.tick(3)
  drain(peer)
  local told = H.events(peer, "undone_in_daw")
  eq(#told, 1)
  eq(told[1].data.how, "undo")
  eq(told[1].data.descs[1], desc)
  eq(told[1].data.epoch, epoch)
  eq(track_data(1).volume, 1)
  mock.song:redo()
  H.tick(3)
  drain(peer)
  told = H.events(peer, "undone_in_daw")
  eq(#told, 1)
  eq(told[1].data.how, "redo")
  eq(told[1].data.descs[1], desc)
  -- told once: nothing moves, nothing more is told
  H.tick(5)
  eq(#H.events(peer, "undone_in_daw"), 0)
  H.no_dialogs()
end)

test("changes: the producer's own edit is not told, and neither is Kumi's own restore", function()
  local env, peer = setup()
  local epoch = place(peer)
  local ref = track_ref(epoch, 1)
  local previewed, _, layout = preview(peer, "track-volume", { ref = ref, volumeDb = -6 })
  local done = apply(peer, "track-volume", { ref = ref, volumeDb = -6 }, "Kumi: v (k8)", previewed.fence, epoch, layout)
  H.tick(2)
  H.events(peer, "undone_in_daw")
  -- the producer sets the same value by hand: no undo, no word
  mock.ops.rename_track(1, "Kick again")
  H.tick(3)
  eq(#H.events(peer, "undone_in_daw"), 0)
  -- Kumi's restore is an ordinary write
  restore(peer, "track-volume", { target = previewed.target, prior = previewed.prior, applied = done.result.applied }, "Kumi: undo v (k9)", epoch, layout)
  H.tick(3)
  eq(#H.events(peer, "undone_in_daw"), 0)
  -- the producer's undo of that restore brings the change back: told as a redo
  mock.song:undo()
  H.tick(3)
  drain(peer)
  local told = H.events(peer, "undone_in_daw")
  eq(#told, 1)
  eq(told[1].data.how, "redo")
  eq(told[1].data.descs[1], "Kumi: v (k8)")
  H.no_dialogs()
end)

-- Deleting a track

local function dense_song()
  local env, peer = setup()
  mock.device_catalog = { ["Audio/Effects/Native/EQ 5"] = { { name = "Gain", value = 0.5, min = 0, max = 1 } } }
  local song = mock.song
  -- Bass (track 3) gets two devices, lines in two patterns, a slot mute, a column name and an automation
  local bass = song:track(3)
  local eq5 = bass:insert_device_at("Audio/Effects/Native/EQ 5", 2)
  local gain = bass:insert_device_at("Audio/Effects/Native/Gainer", 3)
  mock.data.tracks[3].devices[2].params = { { name = "Gain", value = 0.5, min = 0, max = 1 } }
  mock.data.tracks[3].devices[2].preset_data = "<preset eq5/>"
  mock.data.tracks[3].devices[3].display_name = "Louder"
  mock.data.tracks[3].devices[3].active = false
  for _, spec in ipairs({ { 1, 1, 1, 48 }, { 1, 5, 2, 51 }, { 3, 9, 1, 40 } }) do
    local pattern_track = song:pattern(spec[1]):track(3)
    local column = pattern_track:line(spec[2]):note_column(spec[3])
    column.note_value, column.instrument_value, column.volume_value = spec[4], 1, 64
  end
  local effect = song:pattern(2):track(3):line(2):effect_column(1)
  effect.number_value, effect.amount_value = 10, 32
  song:track(3).visible_note_columns = 3
  song:track(3):set_column_name(2, "Octave")
  song.sequencer:set_track_sequence_slot_is_muted(3, 2, true)
  song:track(3).prefx_volume.value = 0.8
  song:track(3).postfx_panning.value = 0.3
  mock.ops.automate(1, 3, 2, 1, { { time = 1, value = 0.2, scaling = 0 }, { time = 9, value = 0.9, scaling = 0 } })
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  return env, peer
end

local function snapshot_of_track(index)
  local t = mock.data.tracks[index]
  -- (what Renoise fills in by itself is its default whether it is set or not)
  local copy = { name = t.name, volume = t.volume, panning = t.panning, post_panning = t.post_panning or 0.5, post_volume = t.post_volume or 1,
    note_columns = t.note_columns, muted_columns = t.muted_columns or {}, column_names = t.column_names or {}, mute_state = t.mute_state,
    devices = {}, patterns = {}, color = t.color or { 41, 166, 153 }, color_blend = t.color_blend or 0, solo = t.solo == true,
    effect_columns = t.effect_columns }
  for i, device in ipairs(t.devices) do
    copy.devices[i] = { path = device.path, name = device.name, active = device.active ~= false, display_name = device.display_name, preset_data = device.preset_data }
  end
  for p, pattern in ipairs(mock.data.patterns) do
    local pt = pattern.tracks[index]
    local lines = {}
    for k, l in pairs(pt and pt.lines or {}) do lines[k] = json.encode(l) end
    copy.patterns[p] = lines
  end
  return json.encode(copy)
end

test("changes: deleting a track keeps its material, and the restore makes the same track again", function()
  local env, peer = dense_song()
  local epoch = place(peer)
  local ref = track_ref(epoch, 3)
  local before = snapshot_of_track(3)
  local previewed, _, layout = preview(peer, "track-delete", { ref = ref })
  eq(previewed.display.title, "Deleted track “Bass”")
  ok(previewed.display.note:find("2 devices", 1, true), previewed.display.note)
  eq(previewed.prior.name, "Bass")
  local done = apply(peer, "track-delete", { ref = ref }, "Kumi: Deleted track Bass (kd01)", previewed.fence, epoch, layout)
  ok(done.result, json.encode(done.error or {}))
  eq(done.result.applied.deleted, true)
  eq(done.result.layout, layout + 1, "the removal moved the layout")
  eq(done.result.undo.entry, true)
  eq(steps(), 1)
  eq(#mock.data.tracks, 5, "the track is gone")
  local material = done.result.material
  eq(material.name, "Bass")
  eq(material.index, 3)
  eq(#material.devices, 2)
  eq(material.devices[1].path, "Audio/Effects/Native/EQ 5")
  eq(material.devices[1].presetData, "<preset eq5/>")
  eq(material.devices[2].active, false)
  eq(material.devices[2].display, "Louder")
  eq(#material.patterns, 3, "the lines of three patterns (two with notes, one with an effect)")
  eq(material.slotMutes[1], 2)
  eq(material.columns.names["2"], "Octave")
  eq(#material.automation, 1)
  eq(material.automation[1].points[2].value, 0.9)
  ok(math.abs(material.volume.pre - 0.8) < 1e-9)
  -- restore, with the layout the apply answered
  local undone = restore(peer, "track-delete", { target = previewed.target, prior = previewed.prior, applied = done.result.applied, material = material },
    "Kumi: undo Deleted track Bass (kd02)", epoch, done.result.layout)
  ok(undone.result, json.encode(undone.error or {}))
  eq(undone.result.restored.ref, track_ref(undone.result.epoch, 3), "(its devices moved the epoch as they were made)")
  eq(undone.result.restored.name, "Bass")
  eq(undone.result.display.title, "“Bass” put back")
  eq(undone.result.undo.entry, true)
  eq(steps(), 2, "the restore is one step")
  eq(newest(), "Kumi: undo Deleted track Bass (kd02)")
  eq(snapshot_of_track(3), before, "the track is what it was")
  H.no_dialogs()
end)

test("changes: a track delete's restore finds its place after tracks were made and deleted since", function()
  local env, peer = dense_song()
  local epoch = place(peer)
  local previewed, _, layout = preview(peer, "track-delete", { ref = track_ref(epoch, 3) })
  local done = apply(peer, "track-delete", { ref = track_ref(epoch, 3) }, "Kumi: d (kd03)", previewed.fence, epoch, layout)
  -- the producer makes a track in front of it
  mock.ops.insert_track(1, { name = "Intro" })
  local undone = restore(peer, "track-delete", { target = previewed.target, prior = previewed.prior, applied = done.result.applied, material = done.result.material },
    "Kumi: undo d (kd04)", epoch, done.result.layout)
  ok(undone.result, json.encode(undone.error or {}))
  eq(undone.result.restored.ref, track_ref(undone.result.epoch, 4), "Bass goes back after Intro, Kick and Hat")
  eq(mock.data.tracks[4].name, "Bass")
  eq(mock.data.tracks[1].name, "Intro")
  H.no_dialogs()
end)

test("changes: a track delete is refused `changed` when the track changed since the preview, and a group or the master is refused", function()
  local env, peer = dense_song()
  local epoch = place(peer)
  local previewed, _, layout = preview(peer, "track-delete", { ref = track_ref(epoch, 3) })
  mock.song:track(3).visible_note_columns = 5
  failure(apply(peer, "track-delete", { ref = track_ref(epoch, 3) }, "Kumi: d (kd05)", previewed.fence, epoch, layout), "changed")
  eq(#mock.data.tracks, 6, "nothing deleted")
  failure(ask(peer, "change_preview", "track-delete", { ref = track_ref(epoch, 5) }), "bad-params")
  H.no_dialogs()
end)

test("changes: a restore finds the track there already and says changed, unless it is told to discard", function()
  local env, peer = dense_song()
  local epoch = place(peer)
  local previewed, _, layout = preview(peer, "track-delete", { ref = track_ref(epoch, 3) })
  local done = apply(peer, "track-delete", { ref = track_ref(epoch, 3) }, "Kumi: d (kd06)", previewed.fence, epoch, layout)
  local record = { target = previewed.target, prior = previewed.prior, applied = done.result.applied, material = done.result.material }
  mock.ops.insert_track(3, { name = "Bass" })
  failure(restore(peer, "track-delete", record, "Kumi: undo d (kd07)", epoch, done.result.layout), "changed")
  H.no_dialogs()
end)

test("changes: a numbered restore whose connection goes after its first step runs to its end, unheard (#361 N-S4)", function()
  local env, peer = setup({ patch = function(env_)
    local path = env_.bundle .. "payload/changes_base.lua"
    local f = assert(io.open(path, "rb"))
    local text = f:read("*a")
    f:close()
    local lowered = text:gsub("M%.SLICE_WRITES = %d+", "M.SLICE_WRITES = 40")
    f = assert(io.open(path, "wb"))
    f:write(lowered)
    f:close()
  end })
  for p = 1, 3 do
    for k = 1, 10 do
      local column = mock.song:pattern(p):track(2):line(k):note_column(1)
      column.note_value, column.instrument_value, column.volume_value = 40 + k, 1, 64
    end
  end
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  local epoch = place(peer)
  local previewed, _, layout = preview(peer, "track-delete", { ref = track_ref(epoch, 2) })
  local before = snapshot_of_track(2)
  local done = apply(peer, "track-delete", { ref = track_ref(epoch, 2) }, "Kumi: d (ks41)", previewed.fence, epoch, layout)
  ok(done.result, json.encode(done.error or {}))
  H.send(peer, { type = "request", id = 9041, method = "change_restore", params = { kind = "track-delete", target = previewed.target,
    prior = previewed.prior, applied = done.result.applied, material = done.result.material, desc = "Kumi: undo d (ks42)", epoch = epoch,
    layout = done.result.layout } })
  for _ = 1, 50 do
    mock.advance(10)
    if steps() >= 2 then break end
  end
  eq(steps(), 2, "its first step is in")
  peer:close() -- Kumi quits (or its host restarts) between the steps
  for _ = 1, 100 do mock.advance(10) end
  eq(steps(), 4, "every step ran")
  eq(snapshot_of_track(2), before, "the whole track is back, not half of it")
end)

test("changes: a restore too big for one callback is numbered steps, each its own, and ends with the whole track", function()
  local env, peer = setup({ patch = function(env)
    local path = env.bundle .. "payload/changes_base.lua"
    local f = assert(io.open(path, "rb"))
    local text = f:read("*a")
    f:close()
    local lowered = text:gsub("M%.SLICE_WRITES = %d+", "M.SLICE_WRITES = 40")
    assert(lowered ~= text, "the slice size was not found to lower")
    f = assert(io.open(path, "wb"))
    f:write(lowered)
    f:close()
  end })
  local song = mock.song
  for p = 1, 3 do
    for k = 1, 10 do
      local column = song:pattern(p):track(2):line(k):note_column(1)
      column.note_value, column.instrument_value, column.volume_value = 40 + k, 1, 64
    end
  end
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  local epoch = place(peer)
  local previewed, _, layout = preview(peer, "track-delete", { ref = track_ref(epoch, 2) })
  local before = snapshot_of_track(2)
  local done = apply(peer, "track-delete", { ref = track_ref(epoch, 2) }, "Kumi: d (ks01)", previewed.fence, epoch, layout)
  ok(done.result, json.encode(done.error or {}))
  eq(steps(), 1)
  local undone = restore(peer, "track-delete", { target = previewed.target, prior = previewed.prior, applied = done.result.applied, material = done.result.material },
    "Kumi: undo d (ks02)", epoch, done.result.layout)
  ok(undone.result, json.encode(undone.error or {}))
  eq(undone.result.undo.steps, 3, "three numbered steps")
  eq(steps(), 4, "the delete, then three")
  eq(mock.data.undo.stack[2].desc, "Kumi: undo d (ks02) (1/3)")
  eq(mock.data.undo.stack[3].desc, "Kumi: undo d (ks02) (2/3)")
  eq(mock.data.undo.stack[4].desc, "Kumi: undo d (ks02) (3/3)")
  eq(snapshot_of_track(2), before, "all of it is back")
  -- each step undoes exactly its own part: the last one takes back one pattern's lines and nothing else
  mock.song:undo()
  ok(mock.song:pattern(3):track(2).is_empty, "the last step's lines are gone")
  eq(mock.data.tracks[2].name, "Hat", "the track itself stays")
  ok(not mock.song:pattern(1):track(2).is_empty, "the first step's lines stay")
  ok(not mock.song:pattern(2):track(2).is_empty, "so do the second's")
  H.no_dialogs()
end)

-- Transport and steps

test("changes: the emergency stop stops what plays and says whether it was recording", function()
  local env, peer = setup()
  local idle = call(peer, "transport_stop").result
  eq(idle.stopped, false)
  eq(idle.wasRecording, false)
  mock.ops.transport("playing", true)
  local playing = call(peer, "transport_stop").result
  eq(playing.stopped, true)
  eq(playing.wasRecording, false)
  H.no_dialogs()
end)

test("changes: a plan's step in Renoise is not merged: every change stays its own step", function()
  local env, peer = setup()
  local begun = call(peer, "undo_step_begin", { label = "Kumi: a plan (kp01)" }).result
  ok(begun.stepId)
  eq(begun.merged, false)
  failure(call(peer, "undo_step_end", { stepId = "nope" }), "failed")
  local ended = call(peer, "undo_step_end", { stepId = begun.stepId }).result
  eq(ended.desc, "Kumi: a plan (kp01)")
  eq(ended.entry, false)
  eq(ended.merged, false)
  failure(call(peer, "undo_step_begin", {}), "bad-params")
  H.no_dialogs()
end)

test("changes: the producer's undo of a Kumi track delete brings the track back and is told, and a redo is told too", function()
  local env, peer = dense_song()
  local epoch = place(peer)
  local previewed, _, layout = preview(peer, "track-delete", { ref = track_ref(epoch, 3) })
  local desc = "Kumi: Deleted track Bass (kd08)"
  local done = apply(peer, "track-delete", { ref = track_ref(epoch, 3) }, desc, previewed.fence, epoch, layout)
  ok(done.result)
  H.tick(2)
  H.events(peer, "undone_in_daw")
  mock.song:undo()
  H.tick(3)
  drain(peer)
  local told = H.events(peer, "undone_in_daw")
  eq(#told, 1, "told once")
  eq(told[1].data.how, "undo")
  eq(told[1].data.descs[1], desc)
  eq(mock.data.tracks[3].name, "Bass", "Renoise brought it back")
  mock.song:redo()
  H.tick(3)
  drain(peer)
  told = H.events(peer, "undone_in_daw")
  eq(#told, 1)
  eq(told[1].data.how, "redo")
  eq(#mock.data.tracks, 5)
  -- Kumi puts it back itself, then the producer takes that step back: the delete is done again, which is told as a redo
  local undone = restore(peer, "track-delete", { target = previewed.target, prior = previewed.prior, applied = done.result.applied, material = done.result.material },
    "Kumi: undo d (kd09)", epoch, done.result.layout)
  ok(undone.result, json.encode(undone.error or {}))
  H.tick(3)
  eq(#H.events(peer, "undone_in_daw"), 0, "Kumi's own restore is not the producer's undo")
  mock.song:undo()
  H.tick(3)
  drain(peer)
  told = H.events(peer, "undone_in_daw")
  eq(#told, 1)
  eq(told[1].data.how, "redo")
  eq(told[1].data.descs[1], desc)
  H.no_dialogs()
end)

test("changes: a producer's insert of a track moves the places the journal knows, so a later undo is matched to the right track", function()
  local env, peer = setup()
  local epoch = place(peer)
  local ref = track_ref(epoch, 3)
  local previewed, _, layout = preview(peer, "track-rename", { ref = ref, name = "Bass 2" })
  apply(peer, "track-rename", { ref = ref, name = "Bass 2" }, "Kumi: rename (kr01)", previewed.fence, epoch, layout)
  mock.ops.insert_track(1, { name = "Intro" }) -- Bass is the fourth now
  H.tick(2)
  H.events(peer, "undone_in_daw")
  -- the producer's own (unrecorded) edit made the insert; Kumi's rename is the newest step
  mock.song:undo()
  H.tick(3)
  drain(peer)
  local told = H.events(peer, "undone_in_daw")
  eq(#told, 1)
  eq(told[1].data.descs[1], "Kumi: rename (kr01)")
  eq(mock.data.tracks[4].name, "Bass")
  H.no_dialogs()
end)

-- Send tracks and groups (the places Renoise gives a track made again)

local function order()
  local out = {}
  for i, t in ipairs(mock.data.tracks) do out[i] = t.name .. (t.kind == "group" and "[G]" or "") .. (mock.song:track(i).group_parent and "(in)" or "") end
  return table.concat(out, ",")
end

local function delete_and_restore(peer, index)
  local epoch = place(peer)
  local previewed, _, layout = preview(peer, "track-delete", { ref = track_ref(epoch, index) })
  local done = apply(peer, "track-delete", { ref = track_ref(epoch, index) }, "Kumi: d (kg01)", previewed.fence, epoch, layout)
  ok(done.result, json.encode(done.error or {}))
  local undone = restore(peer, "track-delete", { target = previewed.target, prior = previewed.prior, applied = done.result.applied, material = done.result.material },
    "Kumi: undo d (kg02)", epoch, done.result.layout)
  ok(undone.result, json.encode(undone.error or {}))
  return done.result.material, undone.result
end

test("changes: a send track goes back after the master, where it was", function()
  local env, peer = setup(nil, { send_tracks = 2 })
  local before = order()
  eq(before, "Kick,Hat,Bass,Pad,Master,Send 1,Send 2")
  local material, restored = delete_and_restore(peer, 6)
  eq(material.kind, "send")
  eq(order(), before)
  eq(mock.data.tracks[6].kind, "send")
  H.no_dialogs()
end)

test("changes: a group's first member goes back first, its last member back last, and both are members again", function()
  local env, peer = setup()
  local song = mock.song
  song:insert_group_at(5)
  song:add_track_to_group(3, 5) -- Bass joins
  song:add_track_to_group(3, 5) -- Pad joins
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  local before = order()
  eq(before, "Kick,Hat,Bass(in),Pad(in),Group[G],Master,Send 1")
  local first = delete_and_restore(peer, 3)
  eq(first.group.name, "Group")
  eq(order(), before, "the first member is the first member again")
  local last = delete_and_restore(peer, 4)
  eq(last.group.at, 4, "the group, one place up once its last member is gone")
  eq(order(), before, "and the last is the last")
  eq(mock.data.tracks[5].members, 2)
  -- a track that is not in a group is not put in one
  local plain = delete_and_restore(peer, 2)
  eq(plain.group, nil)
  eq(order(), before)
  H.no_dialogs()
end)

test("changes: a restore Renoise can't finish takes back the track it began, and says so", function()
  local env, peer = setup()
  local song = mock.song
  local bass = song:track(3)
  bass:insert_device_at("Audio/Effects/Native/EQ 5", 2)
  bass:insert_device_at("Audio/Effects/Native/Gainer", 3)
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  local epoch = place(peer)
  local previewed, _, layout = preview(peer, "track-delete", { ref = track_ref(epoch, 3) })
  local done = apply(peer, "track-delete", { ref = track_ref(epoch, 3) }, "Kumi: d (kf01)", previewed.fence, epoch, layout)
  ok(done.result, json.encode(done.error or {}))
  done.result.material.devices[2].path = "Audio/Effects/Native/Missing Device" -- a plug-in this Renoise doesn't have
  local before = #mock.data.tracks
  local undone = restore(peer, "track-delete", { target = previewed.target, prior = previewed.prior, applied = done.result.applied, material = done.result.material },
    "Kumi: undo d (kf02)", epoch, done.result.layout)
  failure(undone, "failed")
  ok(undone.error.message:find("wouldn't put the track back", 1, true), undone.error.message)
  eq(#mock.data.tracks, before, "no track was left behind")
  H.no_dialogs()
end)

-- Song settings and devices

local function simple_change(peer, kind, input, desc)
  local previewed, epoch, layout = preview(peer, kind, input)
  local done = apply(peer, kind, input, desc, previewed.fence, epoch, layout)
  ok(done.result, kind .. ": " .. json.encode(done.error or {}))
  return previewed, done.result, epoch, layout
end

test("changes: tempo, lines per beat and ticks per line are undoable steps that put back", function()
  local env, peer = setup()
  for _, case in ipairs({
    { "song-bpm", { bpm = 140 }, "Tempo 140 BPM", function() return mock.data.transport_fields.bpm end, 140, 120 },
    { "song-lpb", { lpb = 8 }, "Lines per beat 8", function() return mock.data.transport_fields.lpb end, 8, 4 },
    { "song-tpl", { tpl = 3 }, "Ticks per line 3", function() return mock.data.transport_fields.tpl end, 3, 6 },
  }) do
    local kind, input, title, read, written, original = case[1], case[2], case[3], case[4], case[5], case[6]
    local before = steps()
    local previewed, done, epoch, layout = simple_change(peer, kind, input, "Kumi: " .. kind .. " (ka01)")
    eq(previewed.display.title, title)
    eq(previewed.target.kind, "song")
    eq(done.undo.entry, true)
    eq(read(), written, kind .. " written")
    eq(steps(), before + 1)
    local undone = restore(peer, kind, { target = previewed.target, prior = previewed.prior, applied = done.applied }, "Kumi: undo " .. kind .. " (ka02)", epoch, layout)
    ok(undone.result, json.encode(undone.error or {}))
    eq(read(), original, kind .. " put back")
  end
  local response = ask(peer, "change_preview", "song-bpm", { bpm = 5 })
  failure(response, "bad-params")
  failure(ask(peer, "change_preview", "song-lpb", { lpb = 1.5 }), "bad-params")
  failure(ask(peer, "change_preview", "song-tpl", { tpl = 17 }), "bad-params")
  H.no_dialogs()
end)

test("changes: the producer's undo of a tempo change is told", function()
  local env, peer = setup()
  local _, done = simple_change(peer, "song-bpm", { bpm = 150 }, "Kumi: tempo (ka03)")
  H.tick(2)
  H.events(peer, "undone_in_daw")
  mock.song:undo()
  H.tick(3)
  drain(peer)
  local told = H.events(peer, "undone_in_daw")
  eq(#told, 1)
  eq(told[1].data.how, "undo")
  eq(told[1].data.descs[1], "Kumi: tempo (ka03)")
  H.no_dialogs()
end)

local function device_song()
  local env, peer = setup(nil, { send_tracks = 2 })
  mock.device_catalog = {
    ["Audio/Effects/Native/EQ 5"] = { { name = "Gain", value = 0.5, min = 0, max = 1 } },
    ["Audio/Effects/Native/#Send"] = { { name = "Amount", value = 1, min = 0, max = 1 }, { name = "Panning", value = 0.5, min = 0, max = 1 }, { name = "Receiver", value = 0, min = 0, max = 255 } },
    ["Audio/Effects/Native/Gainer"] = { { name = "Gain", value = 1, min = 0, max = 4 } },
  }
  local bass = mock.song:track(3)
  bass:insert_device_at("Audio/Effects/Native/EQ 5", 2)
  bass:insert_device_at("Audio/Effects/Native/#Send", 3)
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  return env, peer
end

test("changes: a device is bypassed and put back, and the track's mixer device is not touched", function()
  local env, peer = device_song()
  local epoch = place(peer)
  local ref = epoch .. ":device:3.2"
  local previewed, done, e, layout = simple_change(peer, "device-bypass", { ref = ref, bypass = true }, "Kumi: bypass (kb01)")
  eq(previewed.display.title, "EQ 5 on Bass bypassed")
  eq(previewed.target.ref, ref)
  eq(mock.data.tracks[3].devices[2].active, false)
  eq(steps(), 1)
  local undone = restore(peer, "device-bypass", { target = previewed.target, prior = previewed.prior, applied = done.applied }, "Kumi: undo (kb02)", e, layout)
  ok(undone.result, json.encode(undone.error or {}))
  eq(mock.data.tracks[3].devices[2].active, true)
  failure(ask(peer, "change_preview", "device-bypass", { ref = epoch .. ":device:3.1", bypass = true }), "bad-params")
  failure(ask(peer, "change_preview", "device-bypass", { ref = epoch .. ":device:3.9", bypass = true }), "gone")
  failure(ask(peer, "change_preview", "device-bypass", { ref = ref, bypass = "yes" }), "bad-params")
  H.no_dialogs()
end)

test("changes: a parameter is set by value or by text, kept in its range, and a Receiver must be a send track", function()
  local env, peer = device_song()
  local epoch = place(peer)
  local ref = epoch .. ":param:3.2.1"
  local previewed, done, e, layout = simple_change(peer, "device-param", { ref = ref, value = 0.25 }, "Kumi: gain (kc01)")
  eq(previewed.display.title, "EQ 5 on Bass Gain 0.25")
  -- (m4-tui) HISTORY's picture names the parameter and its range, in the preview and the apply
  eq(previewed.display.parameter, "Gain")
  eq(done.display.parameter, "Gain")
  eq(done.display.max, mock.data.tracks[3].devices[2].params[1].max or 1)
  eq(mock.data.tracks[3].devices[2].params[1].value, 0.25)
  local undone = restore(peer, "device-param", { target = previewed.target, prior = previewed.prior, applied = done.applied }, "Kumi: undo (kc02)", e, layout)
  ok(undone.result)
  eq(mock.data.tracks[3].devices[2].params[1].value, 0.5)
  -- by text
  local text, textdone = simple_change(peer, "device-param", { ref = ref, valueString = "0.75" }, "Kumi: gain text (kc03)")
  eq(mock.data.tracks[3].devices[2].params[1].value, 0.75)
  eq(textdone.applied.value, 0.75)
  -- out of range, and neither
  failure(ask(peer, "change_preview", "device-param", { ref = ref, value = 2 }), "bad-params")
  failure(ask(peer, "change_preview", "device-param", { ref = ref }), "bad-params")
  -- a #Send's Receiver is a send track's number (there are two)
  local receiver = epoch .. ":param:3.3.3"
  simple_change(peer, "device-param", { ref = receiver, value = 1 }, "Kumi: send (kc04)")
  eq(mock.data.tracks[3].devices[3].params[3].value, 1)
  local past = ask(peer, "change_preview", "device-param", { ref = receiver, value = 2 })
  failure(past, "bad-params")
  ok(past.error.message:find("send track", 1, true), past.error.message)
  failure(ask(peer, "change_preview", "device-param", { ref = receiver, value = 0.5 }), "bad-params")
  H.no_dialogs()
end)

test("changes: a parameter is named by its device's ref and its number or name too", function()
  local env, peer = device_song()
  local epoch = place(peer)
  local device = epoch .. ":device:3.2"
  local by_name = preview(peer, "device-param", { ref = device, param = "gain", value = 0.25 })
  eq(by_name.target.ref, epoch .. ":param:3.2.1", "the answer names the parameter")
  eq(by_name.display.title, "EQ 5 on Bass Gain 0.25")
  local done = apply(peer, "device-param", { ref = device, param = "Gain", value = 0.25 }, "Kumi: gain (kd21)", by_name.fence, epoch, 0)
  ok(done.result, json.encode(done.error or {}))
  eq(mock.data.tracks[3].devices[2].params[1].value, 0.25)
  -- the start of a name is enough when it names one parameter
  eq(preview(peer, "device-param", { ref = device, param = "Ga", value = 0.5 }).target.ref, epoch .. ":param:3.2.1")
  local by_number = preview(peer, "device-param", { ref = device, param = 1, valueString = "0.5" })
  eq(by_number.target.ref, epoch .. ":param:3.2.1")
  -- the restore finds it by the parameter's ref alone
  local undone = restore(peer, "device-param", { target = by_name.target, prior = by_name.prior, applied = done.result.applied }, "Kumi: undo (kd22)", epoch, 0)
  ok(undone.result, json.encode(undone.error or {}))
  eq(mock.data.tracks[3].devices[2].params[1].value, 0.5)
  local missing = ask(peer, "change_preview", "device-param", { ref = device, param = "Nope", value = 0.5 })
  failure(missing, "bad-params")
  ok(missing.error.message:find("Gain", 1, true), missing.error.message)
  failure(ask(peer, "change_preview", "device-param", { ref = device, value = 0.5 }), "bad-params")
  failure(ask(peer, "change_preview", "device-param", { ref = device, param = 9, value = 0.5 }), "bad-params")
  failure(ask(peer, "change_preview", "device-param", { ref = epoch .. ":device:3.1", param = 1, value = 0.5 }), "bad-params")
  -- text with a unit: the read-back "0.75" has no unit, so a number alone is matched to the digits shown
  local unit = apply(peer, "device-param", { ref = device, param = 1, valueString = "0.7500" }, "Kumi: gain (kd23)",
    preview(peer, "device-param", { ref = device, param = 1, valueString = "0.7500" }).fence, epoch, 0)
  ok(unit.result, json.encode(unit.error or {}))
  H.no_dialogs()
end)

test("changes: the producer's undo of a parameter change is told, and the device's place follows the devices made before it", function()
  local env, peer = device_song()
  local epoch = place(peer)
  simple_change(peer, "device-param", { ref = epoch .. ":param:3.3.3", value = 1 }, "Kumi: send (kd11)")
  H.tick(2)
  H.events(peer, "undone_in_daw")
  -- the producer puts a device in front of it: the #Send is the fourth device now
  mock.ops.insert_device(3, { name = "Filter", params = {} }, 2)
  H.tick(2)
  mock.data.tracks[3].devices[4].params[3].value = 1
  mock.song:undo() -- the parameter change is the newest step (the device was made by a producer's op, no step)
  H.tick(3)
  drain(peer)
  local told = H.events(peer, "undone_in_daw")
  eq(#told, 1, "told")
  eq(told[1].data.descs[1], "Kumi: send (kd11)")
  H.no_dialogs()
end)

-- Putting a device in, taking it out, moving it

local function devices_of(i) return mock.data.tracks[i].devices end
local function paths_of(i)
  local out = {}
  for k, device in ipairs(devices_of(i)) do out[k] = k == 1 and "Mixer" or device.path:match("([^/]+)$") end
  return table.concat(out, ",")
end

test("changes: a device is put in by its path or its name as one undo step, and taken out again", function()
  local env, peer = device_song()
  local epoch, layout = place(peer)
  local ref = track_ref(epoch, 3)
  eq(paths_of(3), "Mixer,EQ 5,#Send")
  local input = { ref = ref, path = "Audio/Effects/Native/Gainer", index = 3 }
  local previewed = preview(peer, "device-add", input)
  eq(previewed.display.title, "Added “Gainer” to “Bass”")
  eq(previewed.display.after.index, 3)
  eq(previewed.target.ref, ref)
  eq(paths_of(3), "Mixer,EQ 5,#Send", "a preview writes nothing")
  eq(steps(), 0)
  -- by name, in any case
  eq(preview(peer, "device-add", { ref = ref, path = "gainer" }).display.after.index, 4)
  local done = apply(peer, "device-add", input, "Kumi: add (ke01)", previewed.fence, epoch, layout)
  ok(done.result, json.encode(done.error or {}))
  eq(paths_of(3), "Mixer,EQ 5,Gainer,#Send")
  eq(steps(), 1)
  eq(newest(), "Kumi: add (ke01)")
  ok(done.result.epoch ~= epoch, "the devices moved the epoch")
  eq(done.result.created.ref, done.result.epoch .. ":device:3.3")
  eq(done.result.created.kind, "device")
  eq(done.result.applied.index, 3)
  eq(done.result.undo.entry, true)
  eq(done.result.material.track, 3)
  -- put back from where the apply left the song
  local undone = restore(peer, "device-add", { target = previewed.target, prior = previewed.prior, applied = done.result.applied,
    created = done.result.created, material = done.result.material }, "Kumi: undo add (ke02)", done.result.epoch, done.result.layout)
  ok(undone.result, json.encode(undone.error or {}))
  eq(paths_of(3), "Mixer,EQ 5,#Send")
  eq(steps(), 2)
  eq(newest(), "Kumi: undo add (ke02)")
  H.no_dialogs()
end)

test("changes: after Renoise's own undo and redo of a track or a device Kumi made, Kumi's undo takes it away; one changed since stays", function()
  local env, peer = device_song()
  -- a track
  local epoch, layout = place(peer)
  local input = { type = "sequencer", name = "Lead", index = 3 }
  local previewed = preview(peer, "track-add", input)
  local done = apply(peer, "track-add", input, "Kumi: add (kr01)", previewed.fence, epoch, layout)
  ok(done.result, json.encode(done.error or {}))
  local record = { target = previewed.target, prior = previewed.prior, applied = done.result.applied, created = done.result.created, material = done.result.material }
  mock.song:undo()
  mock.song:redo()
  local undone = restore(peer, "track-add", record, "Kumi: undo (kr02)", done.result.epoch, done.result.layout)
  ok(undone.result, "the track Renoise's redo made again is Kumi's: " .. json.encode(undone.error or {}))
  local names = {}
  for k, track in ipairs(mock.data.tracks) do names[k] = track.name end
  eq(table.concat(names, ","), "Kick,Hat,Bass,Pad,Master,Send 1,Send 2")
  -- a device, and one the producer turned after the redo
  for _, turned in ipairs({ false, true }) do
    epoch, layout = place(peer)
    local add = { ref = track_ref(epoch, 3), path = "Gainer", index = 3 }
    local p = preview(peer, "device-add", add)
    local d = apply(peer, "device-add", add, "Kumi: add (kr03)", p.fence, epoch, layout)
    ok(d.result, json.encode(d.error or {}))
    mock.song:undo()
    eq(paths_of(3), "Mixer,EQ 5,#Send")
    mock.song:redo()
    eq(paths_of(3), "Mixer,EQ 5,Gainer,#Send")
    if turned then devices_of(3)[3].params[1].value = 2 end
    local back = restore(peer, "device-add", { target = p.target, prior = p.prior, applied = d.result.applied, created = d.result.created,
      material = d.result.material }, "Kumi: undo (kr04)", d.result.epoch, d.result.layout)
    if turned then
      failure(back, "changed")
      eq(paths_of(3), "Mixer,EQ 5,Gainer,#Send")
    else
      ok(back.result, "the device Renoise's redo made again is Kumi's: " .. json.encode(back.error or {}))
      eq(paths_of(3), "Mixer,EQ 5,#Send")
    end
  end
  H.no_dialogs()
end)

test("changes: a device is refused when the song has none like it, the place is out of range or a send's settings are not for a send", function()
  local env, peer = device_song()
  local epoch = place(peer)
  local ref = track_ref(epoch, 3)
  local unknown = ask(peer, "change_preview", "device-add", { ref = ref, path = "Audio/Effects/Native/Gain" })
  failure(unknown, "bad-params")
  ok(unknown.error.message:find("Gainer", 1, true), unknown.error.message)
  failure(ask(peer, "change_preview", "device-add", { ref = ref, path = "Nothing at all" }), "bad-params")
  for _, index in ipairs({ 1, 5, 2.5 }) do
    failure(ask(peer, "change_preview", "device-add", { ref = ref, path = "Gainer", index = index }), "bad-params")
  end
  failure(ask(peer, "change_preview", "device-add", { ref = ref, path = "Gainer", receiver = 0 }), "bad-params")
  failure(ask(peer, "change_preview", "device-add", { ref = ref, path = "#Send", receiver = 2 }), "bad-params")
  failure(ask(peer, "change_preview", "device-add", { ref = ref, path = "#Send", receiver = 0.5 }), "bad-params")
  failure(ask(peer, "change_preview", "device-add", { ref = epoch .. ":track:9", path = "Gainer" }), "gone")
  failure(ask(peer, "change_preview", "device-add", { path = "Gainer" }), "bad-params")
  eq(steps(), 0)
  H.no_dialogs()
end)

test("changes: a #Send is put in with its receiver and amount", function()
  local env, peer = device_song()
  local epoch, layout = place(peer)
  local input = { ref = track_ref(epoch, 1), path = "#Send", receiver = 1, amount = 0.5 }
  local previewed = preview(peer, "device-add", input)
  local done = apply(peer, "device-add", input, "Kumi: send (ke11)", previewed.fence, epoch, layout)
  ok(done.result, json.encode(done.error or {}))
  eq(paths_of(1), "Mixer,#Send")
  eq(devices_of(1)[2].params[3].value, 1, "the receiver")
  eq(devices_of(1)[2].params[1].value, 0.5, "the amount")
  eq(done.result.created.ref, done.result.epoch .. ":device:1.2")
  -- an amount outside 0 to 1 is refused where it is previewed, and at the apply before the device is made
  local e2, l2 = done.result.epoch, done.result.layout
  local fine = { ref = track_ref(e2, 1), path = "#Send", amount = 0.25 }
  local valid = call(peer, "change_preview", { kind = "device-add", input = fine, epoch = e2, layout = l2 })
  ok(valid.result, json.encode(valid.error or {}))
  failure(call(peer, "change_preview", { kind = "device-add", input = { ref = fine.ref, path = "#Send", amount = 3 }, epoch = e2, layout = l2 }), "bad-params")
  local before_devices, before_steps = #devices_of(1), steps()
  local refused = apply(peer, "device-add", { ref = fine.ref, path = "#Send", amount = 3 }, "Kumi: send (ke12)", valid.result.fence, e2, l2)
  failure(refused, "bad-params")
  eq(#devices_of(1), before_devices, "no device is left behind")
  eq(steps(), before_steps, "and no step")
  H.no_dialogs()
end)

test("changes: a #Send whose parameter refuses what it was asked is taken out again", function()
  local env, peer = device_song()
  local epoch, layout = place(peer)
  -- (this Renoise's Amount only goes to 0.5: the plan's own check passes, the parameter's own range doesn't)
  mock.device_catalog["Audio/Effects/Native/#Send"][1].max = 0.5
  local input = { ref = track_ref(epoch, 1), path = "#Send", amount = 0.75 }
  local previewed = preview(peer, "device-add", input)
  local before = #devices_of(1)
  failure(apply(peer, "device-add", input, "Kumi: send (ke13)", previewed.fence, epoch, layout), "bad-params")
  eq(#devices_of(1), before, "the device the apply made is gone again")
  H.no_dialogs()
end)

test("changes: a device the producer changed since is not taken out unless Kumi is told to", function()
  local env, peer = device_song()
  local epoch, layout = place(peer)
  local input = { ref = track_ref(epoch, 3), path = "Gainer", index = 2 }
  local previewed = preview(peer, "device-add", input)
  local done = apply(peer, "device-add", input, "Kumi: add (ke21)", previewed.fence, epoch, layout).result
  local record = { target = previewed.target, prior = previewed.prior, applied = done.applied, created = done.created, material = done.material }
  devices_of(3)[2].params[1].value = 3
  failure(restore(peer, "device-add", record, "Kumi: undo add (ke22)", done.epoch, done.layout), "changed")
  eq(paths_of(3), "Mixer,Gainer,EQ 5,#Send")
  eq(steps(), 1, "nothing was written")
  local forced = restore(peer, "device-add", record, "Kumi: undo add (ke22)", done.epoch, done.layout, { discard = true })
  ok(forced.result, json.encode(forced.error or {}))
  eq(paths_of(3), "Mixer,EQ 5,#Send")
  -- another epoch is stale
  local made = apply(peer, "device-add", input, "Kumi: add (ke23)", previewed.fence, "ffff", 0)
  failure(made, "stale")
  H.no_dialogs()
end)

local function bass_with_automation()
  local env, peer = device_song()
  local d = devices_of(3)
  d[2].preset_data, d[2].display_name, d[2].active = "<preset eq5/>", "Warm", false
  mock.ops.automate(1, 3, 2, 1, { { time = 1, value = 0.2, scaling = 0 }, { time = 9, value = 0.9, scaling = 0 } })
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  return env, peer
end

test("changes: deleting a device keeps its settings and automation, and the restore makes it again where it was", function()
  local env, peer = bass_with_automation()
  local epoch, layout = place(peer)
  local ref = epoch .. ":device:3.2"
  local previewed = preview(peer, "device-delete", { ref = ref })
  eq(previewed.display.title, "Deleted “Warm” from “Bass”")
  eq(previewed.prior.deleted, false)
  eq(steps(), 0)
  local done = apply(peer, "device-delete", { ref = ref }, "Kumi: delete device (kf01)", previewed.fence, epoch, layout)
  ok(done.result, json.encode(done.error or {}))
  eq(paths_of(3), "Mixer,#Send")
  eq(steps(), 1)
  eq(done.result.applied.deleted, true)
  eq(done.result.material.track, 3)
  eq(done.result.material.index, 2)
  eq(done.result.material.device.presetData, "<preset eq5/>")
  eq(#done.result.material.automation, 1)
  eq(done.result.material.automation[1].points[2].value, 0.9)
  ok(done.result.epoch ~= epoch)
  local record = { target = previewed.target, prior = previewed.prior, applied = done.result.applied, material = done.result.material }
  local undone = restore(peer, "device-delete", record, "Kumi: undo delete device (kf02)", done.result.epoch, done.result.layout)
  ok(undone.result, json.encode(undone.error or {}))
  eq(paths_of(3), "Mixer,EQ 5,#Send")
  local back = devices_of(3)[2]
  eq(back.preset_data, "<preset eq5/>")
  eq(back.display_name, "Warm")
  eq(back.active, false)
  eq(steps(), 2)
  eq(newest(), "Kumi: undo delete device (kf02)")
  eq(undone.result.restored.ref, undone.result.epoch .. ":device:3.2")
  local automation = mock.data.patterns[1].tracks[3].automation
  eq(#automation, 1)
  eq(automation[1].points[2].value, 0.9)
  -- asked again: it is back, nothing is written
  local again = restore(peer, "device-delete", record, "Kumi: undo delete device (kf03)", undone.result.epoch, undone.result.layout)
  ok(again.result, json.encode(again.error or {}))
  eq(again.result.undo.entry, false)
  eq(steps(), 2)
  H.no_dialogs()
end)

test("changes: a deleted device goes back to its track after tracks were made before it, and the mixer device is never touched", function()
  local env, peer = bass_with_automation()
  local epoch, layout = place(peer)
  local ref = epoch .. ":device:3.3"
  local previewed = preview(peer, "device-delete", { ref = ref })
  local done = apply(peer, "device-delete", { ref = ref }, "Kumi: delete device (kf11)", previewed.fence, epoch, layout).result
  eq(paths_of(3), "Mixer,EQ 5")
  mock.api.insert_track_at(1)
  H.tick(3)
  local now = place(peer)
  eq(now, done.epoch, "a made track moves the layout, not the epoch")
  local undone = restore(peer, "device-delete", { target = previewed.target, prior = previewed.prior, applied = done.applied, material = done.material },
    "Kumi: undo delete device (kf12)", done.epoch, done.layout)
  ok(undone.result, json.encode(undone.error or {}))
  eq(paths_of(4), "Mixer,EQ 5,#Send", "Bass is track 4 now")
  eq(undone.result.restored.ref, undone.result.epoch .. ":device:4.3")
  failure(ask(peer, "change_preview", "device-delete", { ref = undone.result.epoch .. ":device:4.1" }), "bad-params")
  failure(ask(peer, "change_preview", "device-move", { ref = undone.result.epoch .. ":device:4.1", index = 2 }), "bad-params")
  failure(ask(peer, "change_preview", "device-delete", { ref = undone.result.epoch .. ":device:4.9" }), "gone")
  H.no_dialogs()
end)

test("changes: a device changed since the preview is not deleted", function()
  local env, peer = bass_with_automation()
  local epoch, layout = place(peer)
  local ref = epoch .. ":device:3.2"
  local previewed = preview(peer, "device-delete", { ref = ref })
  devices_of(3)[2].params[1].value = 0.1
  failure(apply(peer, "device-delete", { ref = ref }, "Kumi: delete device (kf21)", previewed.fence, epoch, layout), "changed")
  eq(paths_of(3), "Mixer,EQ 5,#Send")
  eq(steps(), 0)
  H.no_dialogs()
end)

local function four_devices()
  local env, peer = device_song()
  devices_of(3)[4] = nil
  mock.song:track(3):insert_device_at("Audio/Effects/Native/Gainer", 4)
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  return env, peer
end

test("changes: a device is moved by swaps as one undo step, and moved back", function()
  local env, peer = four_devices()
  eq(paths_of(3), "Mixer,EQ 5,#Send,Gainer")
  local epoch, layout = place(peer)
  local ref = epoch .. ":device:3.2"
  local previewed = preview(peer, "device-move", { ref = ref, index = 4 })
  eq(previewed.display.title, "Moved “EQ 5” on “Bass” to place 4")
  eq(previewed.prior.index, 2)
  eq(steps(), 0)
  local done = apply(peer, "device-move", { ref = ref, index = 4 }, "Kumi: move (kg01)", previewed.fence, epoch, layout)
  ok(done.result, json.encode(done.error or {}))
  eq(paths_of(3), "Mixer,#Send,Gainer,EQ 5")
  eq(steps(), 1)
  eq(done.result.applied.index, 4)
  eq(done.result.material.track, 3)
  local record = { target = previewed.target, prior = previewed.prior, applied = done.result.applied, material = done.result.material }
  local undone = restore(peer, "device-move", record, "Kumi: undo move (kg02)", done.result.epoch, done.result.layout)
  ok(undone.result, json.encode(undone.error or {}))
  eq(paths_of(3), "Mixer,EQ 5,#Send,Gainer")
  eq(steps(), 2)
  -- already back: nothing written
  local again = restore(peer, "device-move", record, "Kumi: undo move (kg03)", undone.result.epoch, undone.result.layout)
  ok(again.result, json.encode(again.error or {}))
  eq(again.result.undo.entry, false)
  eq(steps(), 2)
  -- to where it is: nothing to do
  local same = preview(peer, "device-move", { ref = place(peer) .. ":device:3.2", index = 2 })
  eq(same.display.unchanged, true)
  H.no_dialogs()
end)

test("changes: a device the producer moved since is not moved back", function()
  local env, peer = four_devices()
  local epoch, layout = place(peer)
  local ref = epoch .. ":device:3.2"
  local previewed = preview(peer, "device-move", { ref = ref, index = 3 })
  local done = apply(peer, "device-move", { ref = ref, index = 3 }, "Kumi: move (kg11)", previewed.fence, epoch, layout).result
  eq(paths_of(3), "Mixer,#Send,EQ 5,Gainer")
  mock.song:track(3):swap_devices_at(3, 4)
  H.tick(2)
  local now, layout_now = place(peer)
  local record = { target = previewed.target, prior = previewed.prior, applied = done.applied, material = done.material }
  failure(restore(peer, "device-move", record, "Kumi: undo move (kg12)", now, layout_now), "changed")
  eq(paths_of(3), "Mixer,#Send,Gainer,EQ 5")
  failure(ask(peer, "change_preview", "device-move", { ref = now .. ":device:3.2", index = 9 }), "bad-params")
  H.no_dialogs()
end)

test("changes: the producer's undo and redo of a device made, deleted or moved is told", function()
  local env, peer = bass_with_automation()
  local epoch, layout = place(peer)
  local function told_after(action)
    H.tick(2)
    H.events(peer, "undone_in_daw")
    action()
    H.tick(3)
    drain(peer)
    return H.events(peer, "undone_in_daw")
  end
  -- made
  local added = apply(peer, "device-add", { ref = track_ref(epoch, 3), path = "Gainer", index = 2 }, "Kumi: add (kh01)",
    preview(peer, "device-add", { ref = track_ref(epoch, 3), path = "Gainer", index = 2 }).fence, epoch, layout).result
  local told = told_after(function() mock.song:undo() end)
  eq(#told, 1, "made, undone")
  eq(told[1].data.how, "undo")
  eq(told[1].data.descs[1], "Kumi: add (kh01)")
  eq(paths_of(3), "Mixer,EQ 5,#Send")
  told = told_after(function() mock.song:redo() end)
  eq(#told, 1, "made, redone")
  eq(told[1].data.how, "redo")
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  -- deleted
  local epoch2, layout2 = place(peer)
  local ref = epoch2 .. ":device:3.2"
  local fence = preview(peer, "device-delete", { ref = ref }).fence
  apply(peer, "device-delete", { ref = ref }, "Kumi: delete (kh02)", fence, epoch2, layout2)
  told = told_after(function() mock.song:undo() end)
  eq(#told, 1, "deleted, undone")
  eq(told[1].data.how, "undo")
  eq(told[1].data.descs[1], "Kumi: delete (kh02)")
  told = told_after(function() mock.song:redo() end)
  eq(#told, 1, "deleted, redone")
  eq(told[1].data.how, "redo")
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  H.no_dialogs()
end)

test("changes: the producer's undo of a device move is told", function()
  local env, peer = four_devices()
  local epoch, layout = place(peer)
  local ref = epoch .. ":device:3.2"
  local fence = preview(peer, "device-move", { ref = ref, index = 4 }).fence
  apply(peer, "device-move", { ref = ref, index = 4 }, "Kumi: move (ki01)", fence, epoch, layout)
  H.tick(2)
  H.events(peer, "undone_in_daw")
  mock.song:undo()
  H.tick(3)
  drain(peer)
  local told = H.events(peer, "undone_in_daw")
  eq(#told, 1)
  eq(told[1].data.how, "undo")
  eq(told[1].data.descs[1], "Kumi: move (ki01)")
  eq(paths_of(3), "Mixer,EQ 5,#Send,Gainer")
  mock.song:redo()
  H.tick(3)
  drain(peer)
  told = H.events(peer, "undone_in_daw")
  eq(told[#told].data.how, "redo")
  H.no_dialogs()
end)

-- Notes and effects on lines

local function pattern_ref(epoch, p) return epoch .. ":pattern:" .. p end

local function cell(p, t, l, c)
  local column = mock.song:pattern(p):track(t):line(l):note_column(c)
  return column.note_string .. " " .. column.instrument_string .. " " .. column.volume_string .. " " .. column.panning_string
    .. " " .. column.delay_string .. " " .. column.effect_number_string .. column.effect_amount_string
end

local function effect_cell(p, t, l, c)
  local column = mock.song:pattern(p):track(t):line(l):effect_column(c)
  return column.number_string .. column.amount_string
end

local function write_input(epoch)
  return {
    pattern = pattern_ref(epoch, 1), track = track_ref(epoch, 3),
    lines = {
      { line = 0, notes = { { col = 1, note = "c-2", instrument = 0, volume = "40" }, { col = 3, note = "E-2", instrument = "01", delay = "10", fx = "0a20" } },
        effects = { { col = 2, number = "0A", amount = "20" } } },
      { line = 4, notes = { { col = 1, note = "OFF" } } },
    },
  }
end

test("changes: lines are written as the tracker's own text, the columns they need are shown, and it is one undo step", function()
  local env, peer = setup()
  local epoch, layout = place(peer)
  local input = write_input(epoch)
  local previewed = preview(peer, "notes-write", input)
  eq(previewed.display.title, "Wrote 2 lines in “Pattern 00” on “Bass”")
  eq(previewed.target.ref, epoch .. ":ptrack:1.3")
  eq(previewed.target.kind, "pattern-track")
  eq(#previewed.prior.cells, 4)
  eq(previewed.prior.columns.note, 2)
  eq(steps(), 0)
  eq(cell(1, 3, 1, 1), "--- .. .. .. .. 0000", "a preview writes nothing")
  local done = apply(peer, "notes-write", input, "Kumi: notes (kn01)", previewed.fence, epoch, layout)
  ok(done.result, json.encode(done.error or {}))
  eq(steps(), 1)
  eq(newest(), "Kumi: notes (kn01)")
  eq(done.result.undo.entry, true)
  eq(cell(1, 3, 1, 1), "C-2 00 40 .. .. 0000")
  eq(cell(1, 3, 1, 3), "E-2 01 .. .. 10 0A20")
  eq(effect_cell(1, 3, 1, 2), "0A20")
  eq(cell(1, 3, 5, 1), "OFF .. .. .. .. 0000")
  eq(#done.result.applied.cells, 4)
  eq(done.result.applied.cells[1].note, "C-2")
  eq(done.result.applied.columns.note, 3)
  eq(mock.data.tracks[3].note_columns, 3, "the third note column is shown")
  eq(mock.data.tracks[3].effect_columns, 2)
  eq(mock.data.tracks[3].volume_column, true)
  eq(mock.data.tracks[3].delay_column, true)
  eq(mock.data.tracks[3].sample_effects_column, true)
  eq(done.result.epoch, epoch, "lines don't move the epoch")
  -- put back
  local undone = restore(peer, "notes-write", { target = previewed.target, prior = previewed.prior, applied = done.result.applied },
    "Kumi: undo notes (kn02)", epoch, layout)
  ok(undone.result, json.encode(undone.error or {}))
  eq(cell(1, 3, 1, 1), "--- .. .. .. .. 0000")
  eq(cell(1, 3, 1, 3), "--- .. .. .. .. 0000")
  eq(effect_cell(1, 3, 1, 2), "0000")
  eq(cell(1, 3, 5, 1), "--- .. .. .. .. 0000")
  eq(mock.data.tracks[3].note_columns, 2, "the columns Kumi showed are hidden again")
  eq(mock.data.tracks[3].effect_columns, 1)
  eq(mock.data.tracks[3].volume_column or false, false)
  eq(steps(), 2)
  eq(newest(), "Kumi: undo notes (kn02)")
  H.no_dialogs()
end)

test("changes: the columns Kumi showed stay shown when the producer put something in them", function()
  local env, peer = setup()
  local epoch, layout = place(peer)
  local input = write_input(epoch)
  local previewed = preview(peer, "notes-write", input)
  local done = apply(peer, "notes-write", input, "Kumi: notes (kn11)", previewed.fence, epoch, layout).result
  local other = mock.song:pattern(2):track(3):line(7):note_column(3)
  other.note_value, other.instrument_value = 40, 2
  local undone = restore(peer, "notes-write", { target = previewed.target, prior = previewed.prior, applied = done.applied }, "Kumi: undo (kn12)", epoch, layout)
  ok(undone.result, json.encode(undone.error or {}))
  eq(mock.data.tracks[3].note_columns, 3, "column 3 holds a note in another pattern")
  eq(mock.data.tracks[3].effect_columns, 1, "nothing is in the second effect column")
  H.no_dialogs()
end)

test("changes: a cell the producer changed since is not overwritten by a restore, discard or not (lines have no identity but their place)", function()
  local env, peer = setup()
  local epoch, layout = place(peer)
  local input = write_input(epoch)
  local previewed = preview(peer, "notes-write", input)
  local done = apply(peer, "notes-write", input, "Kumi: notes (kn21)", previewed.fence, epoch, layout).result
  local mine = mock.song:pattern(1):track(3):line(1):note_column(1)
  mine.note_string = "G-3"
  local before = steps()
  local record = { target = previewed.target, prior = previewed.prior, applied = done.applied }
  failure(restore(peer, "notes-write", record, "Kumi: undo (kn22)", epoch, layout), "changed")
  failure(restore(peer, "notes-write", record, "Kumi: undo (kn22)", epoch, layout, { discard = true }), "changed")
  eq(cell(1, 3, 1, 1), "G-3 00 40 .. .. 0000")
  eq(steps(), before)
  H.no_dialogs()
end)

test("changes: lines the producer changed between the preview and the apply are not overwritten", function()
  local env, peer = setup()
  local epoch, layout = place(peer)
  local input = write_input(epoch)
  local previewed = preview(peer, "notes-write", input)
  mock.song:pattern(1):track(3):line(5):note_column(1).note_string = "C-5"
  local before = steps()
  failure(apply(peer, "notes-write", input, "Kumi: notes (kn31)", previewed.fence, epoch, layout), "changed")
  eq(cell(1, 3, 1, 1), "--- .. .. .. .. 0000")
  eq(steps(), before)
  failure(apply(peer, "notes-write", input, "Kumi: notes (kn32)", previewed.fence, "ffff", layout), "stale")
  H.no_dialogs()
end)

test("changes: merge keeps the fields a cell isn't given, and without it a cell is written whole", function()
  local env, peer = setup()
  local epoch, layout = place(peer)
  local first = { pattern = pattern_ref(epoch, 1), track = track_ref(epoch, 3), lines = { { line = 1, notes = { { col = 1, note = "A-3", instrument = "02", volume = "30" } } } } }
  local p1 = preview(peer, "notes-write", first)
  ok(apply(peer, "notes-write", first, "Kumi: a (kn41)", p1.fence, epoch, layout).result)
  local merged = { pattern = first.pattern, track = first.track, merge = true, lines = { { line = 1, notes = { { col = 1, volume = "50" } } } } }
  local p2 = preview(peer, "notes-write", merged)
  ok(apply(peer, "notes-write", merged, "Kumi: b (kn42)", p2.fence, epoch, layout).result)
  eq(cell(1, 3, 2, 1), "A-3 02 50 .. .. 0000")
  local whole = { pattern = first.pattern, track = first.track, lines = { { line = 1, notes = { { col = 1, volume = "60" } } } } }
  local p3 = preview(peer, "notes-write", whole)
  ok(apply(peer, "notes-write", whole, "Kumi: c (kn43)", p3.fence, epoch, layout).result)
  eq(cell(1, 3, 2, 1), "--- .. 60 .. .. 0000")
  -- a write of what is there is no change
  local same = preview(peer, "notes-write", whole)
  eq(same.display.unchanged, true)
  local done = apply(peer, "notes-write", whole, "Kumi: d (kn44)", same.fence, epoch, layout)
  eq(done.result.undo.entry, false)
  eq(steps(), 3)
  H.no_dialogs()
end)

test("changes: lines are cleared with every cell on them, hidden columns too, and notes-clear clears a range", function()
  local env, peer = setup()
  local song = mock.song
  for _, spec in ipairs({ { 1, 1, 1, 48 }, { 1, 4, 5, 50 }, { 1, 10, 10, 52 } }) do
    local column = song:pattern(1):track(3):line(spec[2]):note_column(spec[3])
    column.note_value, column.instrument_value = spec[4], 1
  end
  local effect = song:pattern(1):track(3):line(4):effect_column(2)
  effect.number_value, effect.amount_value = 10, 32
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  local epoch, layout = place(peer)
  local input = { pattern = pattern_ref(epoch, 1), track = track_ref(epoch, 3), lines = { { line = 3, clear = true } } }
  local previewed = preview(peer, "notes-write", input)
  eq(previewed.display.title, "Wrote 1 line in “Pattern 00” on “Bass”")
  eq(#previewed.prior.cells, 2, "the note in column 5 and the effect in column 2")
  local done = apply(peer, "notes-write", input, "Kumi: clear (kn51)", previewed.fence, epoch, layout)
  ok(done.result, json.encode(done.error or {}))
  ok(song:pattern(1):track(3):line(4).is_empty, "the line is empty")
  local undone = restore(peer, "notes-write", { target = previewed.target, prior = previewed.prior, applied = done.result.applied }, "Kumi: undo (kn52)", epoch, layout)
  ok(undone.result, json.encode(undone.error or {}))
  eq(cell(1, 3, 4, 5), "D-4 01 .. .. .. 0000")
  eq(effect_cell(1, 3, 4, 2), "0A20")
  -- a range
  local range = { pattern = pattern_ref(epoch, 1), track = track_ref(epoch, 3), first = 0, last = 4 }
  local p2 = preview(peer, "notes-clear", range)
  eq(p2.display.title, "Cleared 2 lines in “Pattern 00” on “Bass”")
  eq(p2.display.before.notes, 2)
  eq(p2.display.before.effects, 1)
  local d2 = apply(peer, "notes-clear", range, "Kumi: clear (kn53)", p2.fence, epoch, layout)
  ok(d2.result, json.encode(d2.error or {}))
  ok(not song:pattern(1):track(3):line(10).is_empty, "line 10 is outside the range")
  ok(song:pattern(1):track(3):line(1).is_empty and song:pattern(1):track(3):line(4).is_empty)
  local u2 = restore(peer, "notes-clear", { target = p2.target, prior = p2.prior, applied = d2.result.applied }, "Kumi: undo (kn54)", epoch, layout)
  ok(u2.result, json.encode(u2.error or {}))
  eq(cell(1, 3, 1, 1), "C-4 01 .. .. .. 0000")
  eq(effect_cell(1, 3, 4, 2), "0A20")
  -- nothing to clear
  local empty = preview(peer, "notes-clear", { pattern = pattern_ref(epoch, 3), track = track_ref(epoch, 3) })
  eq(empty.display.unchanged, true)
  H.no_dialogs()
end)

test("changes: lines are refused when what they name isn't there or isn't text Renoise reads", function()
  local env, peer = setup()
  local epoch = place(peer)
  local function with(lines, more)
    local input = { pattern = pattern_ref(epoch, 1), track = track_ref(epoch, 3), lines = lines }
    for k, v in pairs(more or {}) do input[k] = v end
    return ask(peer, "change_preview", "notes-write", input)
  end
  failure(with({ { line = -1, notes = { { col = 1, note = "C-4" } } } }), "bad-params")
  failure(with({ { line = 32, notes = { { col = 1, note = "C-4" } } } }), "bad-params")
  failure(with({ { line = 0, notes = { { col = 13, note = "C-4" } } } }), "bad-params")
  failure(with({ { line = 0, effects = { { col = 9, number = "0A" } } } }), "bad-params")
  failure(with({ { line = 0, notes = { { col = 1, note = "H-4" } } } }), "bad-params")
  failure(with({ { line = 0, notes = { { col = 1, note = "E#4" } } } }), "bad-params")
  failure(with({ { line = 0, notes = { { col = 1, instrument = "ZZ" } } } }), "bad-params")
  failure(with({ { line = 0, notes = { { col = 1, volume = "400" } } } }), "bad-params")
  failure(with({ { line = 0, notes = { { col = 1, fx = "0A" } } } }), "bad-params")
  failure(with({ { line = 0, notes = { { col = 1 } } } }), "bad-params")
  failure(with({ { line = 0 } }), "bad-params")
  failure(with({ { line = 0, notes = { { col = 1, note = "C-4" }, { col = 1, note = "D-4" } } } }), "bad-params")
  failure(with({ { line = 0, notes = { { col = 1, note = "C-4" } } }, { line = 0, notes = { { col = 2, note = "C-4" } } } }), "bad-params")
  failure(with({}), "bad-params")
  failure(with(nil), "bad-params")
  failure(ask(peer, "change_preview", "notes-write", { pattern = pattern_ref(epoch, 9), track = track_ref(epoch, 3), lines = { { line = 0, clear = true } } }), "gone")
  failure(ask(peer, "change_preview", "notes-write", { pattern = pattern_ref(epoch, 1), track = track_ref(epoch, 9), lines = { { line = 0, clear = true } } }), "gone")
  -- the master has no note columns, but takes effects
  failure(ask(peer, "change_preview", "notes-write", { pattern = pattern_ref(epoch, 1), track = track_ref(epoch, 5), lines = { { line = 0, notes = { { col = 1, note = "C-4" } } } } }), "bad-params")
  local master = ask(peer, "change_preview", "notes-write", { pattern = pattern_ref(epoch, 1), track = track_ref(epoch, 5), lines = { { line = 0, effects = { { col = 1, number = "0A", amount = "10" } } } } })
  ok(master.result, json.encode(master.error or {}))
  -- an alias writes nowhere
  mock.song:pattern(2):track(3).alias_pattern_index = 1
  local before = steps()
  local alias = ask(peer, "change_preview", "notes-write", { pattern = pattern_ref(epoch, 2), track = track_ref(epoch, 3), lines = { { line = 0, notes = { { col = 1, note = "C-4" } } } } })
  failure(alias, "bad-params")
  ok(alias.error.message:find("alias of pattern 00", 1, true), alias.error.message)
  eq(steps(), before)
  H.no_dialogs()
end)

test("changes: a sequence slot names its pattern, and Renoise refusing the text takes back what was written", function()
  local env, peer = setup()
  local epoch, layout = place(peer)
  -- slot 4 is pattern 3
  local input = { pattern = epoch .. ":seq:4", track = track_ref(epoch, 1), lines = { { line = 1, notes = { { col = 1, note = "C-5" } } } } }
  local previewed = preview(peer, "notes-write", input)
  eq(previewed.target.ref, epoch .. ":ptrack:3.1")
  local done = apply(peer, "notes-write", input, "Kumi: slot (kn61)", previewed.fence, epoch, layout)
  ok(done.result, json.encode(done.error or {}))
  eq(cell(3, 1, 2, 1), "C-5 .. .. .. .. 0000")
  eq(cell(1, 1, 2, 1), "--- .. .. .. .. 0000")
  -- a text that Renoise itself refuses (QQ is no effect command) leaves the pattern as it was
  local column = mock.song:pattern(1):track(2):line(3):note_column(1)
  local refused = { pattern = pattern_ref(epoch, 1), track = track_ref(epoch, 2), lines = { { line = 2, notes = { { col = 1, note = "C-4", fx = "QQ20" } } } } }
  local p2 = preview(peer, "notes-write", refused)
  local before = steps()
  failure(apply(peer, "notes-write", refused, "Kumi: bad (kn62)", p2.fence, epoch, layout), "failed")
  eq(column.note_string, "---", "the note that was written before the refusal is taken back")
  H.no_dialogs()
end)

test("changes: a big restore is numbered steps, and the producer's undo of lines is told", function()
  local env, peer = setup({ patch = function(env)
    local path = env.bundle .. "payload/changes_base.lua"
    local f = assert(io.open(path, "rb"))
    local text = f:read("*a")
    f:close()
    f = assert(io.open(path, "wb"))
    f:write((text:gsub("M%.SLICE_WRITES = %d+", "M.SLICE_WRITES = 30")))
    f:close()
  end })
  local epoch, layout = place(peer)
  local lines = {}
  for l = 1, 12 do lines[l] = { line = l - 1, notes = { { col = 1, note = "C-3", instrument = "01" } } } end
  local input = { pattern = pattern_ref(epoch, 1), track = track_ref(epoch, 2), lines = lines }
  local previewed = preview(peer, "notes-write", input)
  local done = apply(peer, "notes-write", input, "Kumi: many (kn71)", previewed.fence, epoch, layout)
  ok(done.result, json.encode(done.error or {}))
  eq(steps(), 1)
  -- the producer's undo of it is told
  H.tick(2)
  H.events(peer, "undone_in_daw")
  mock.song:undo()
  H.tick(3)
  drain(peer)
  local told = H.events(peer, "undone_in_daw")
  eq(#told, 1)
  eq(told[1].data.how, "undo")
  eq(told[1].data.descs[1], "Kumi: many (kn71)")
  mock.song:redo()
  H.tick(3)
  drain(peer)
  told = H.events(peer, "undone_in_daw")
  -- (lines back as Kumi wrote them may be Renoise's redo, or its undo of something older: Kumi can't tell)
  eq(told[#told].data.how, "unsure")
  -- and Kumi's undo refuses: Renoise's undo and redo changed the pattern since Kumi's step
  local refused = restore(peer, "notes-write", { target = previewed.target, prior = previewed.prior, applied = done.result.applied }, "Kumi: undo many (kn72)", epoch, layout)
  eq(refused.error and refused.error.code, "changed", json.encode(refused))
  eq(cell(1, 2, 12, 1):sub(1, 3), "C-3")
  -- restored in numbered steps (84 writes in slices of 30), in a pattern nothing changed since
  input.pattern = pattern_ref(epoch, 2)
  previewed = preview(peer, "notes-write", input)
  done = apply(peer, "notes-write", input, "Kumi: many (kn73)", previewed.fence, epoch, layout)
  ok(done.result, json.encode(done.error or {}))
  local undone = restore(peer, "notes-write", { target = previewed.target, prior = previewed.prior, applied = done.result.applied }, "Kumi: undo many (kn74)", epoch, layout)
  ok(undone.result, json.encode(undone.error or {}))
  eq(undone.result.undo.steps, 3)
  eq(mock.data.undo.stack[#mock.data.undo.stack].desc, "Kumi: undo many (kn74) (3/3)")
  eq(cell(2, 2, 12, 1), "--- .. .. .. .. 0000")
  eq(cell(2, 2, 1, 1), "--- .. .. .. .. 0000")
  H.no_dialogs()
end)

test("changes: a track made before the pattern's track carries the restore to where the lines are now", function()
  local env, peer = setup()
  local epoch, layout = place(peer)
  local input = { pattern = pattern_ref(epoch, 1), track = track_ref(epoch, 2), lines = { { line = 0, notes = { { col = 1, note = "C-3" } } } } }
  local previewed = preview(peer, "notes-write", input)
  local done = apply(peer, "notes-write", input, "Kumi: shift (kn81)", previewed.fence, epoch, layout).result
  mock.api.insert_track_at(1)
  H.tick(3)
  local undone = restore(peer, "notes-write", { target = previewed.target, prior = previewed.prior, applied = done.applied }, "Kumi: undo shift (kn82)", epoch, layout)
  ok(undone.result, json.encode(undone.error or {}))
  eq(cell(1, 3, 1, 1), "--- .. .. .. .. 0000", "Hat is track 3 now")
  H.no_dialogs()
end)

-- Making, moving and grouping tracks

local function layout_text()
  local out = {}
  for i = 1, #mock.data.tracks do
    local t = mock.song:track(i)
    out[i] = t.name .. (t.group_parent and ("@" .. t.group_parent.name) or "")
  end
  return table.concat(out, ",")
end

test("changes: a track is made as sequencer, send or group, and removed again when it is still empty", function()
  local env, peer = setup()
  local epoch, layout = place(peer)
  eq(layout_text(), "Kick,Hat,Bass,Pad,Master,Send 1")
  local input = { type = "sequencer", name = "Lead", color = "#ff8000", index = 3 }
  local previewed = preview(peer, "track-add", input)
  eq(previewed.display.title, "Added track “Lead”")
  eq(previewed.display.after.index, 3)
  eq(steps(), 0)
  local done = apply(peer, "track-add", input, "Kumi: add track (kt01)", previewed.fence, epoch, layout)
  ok(done.result, json.encode(done.error or {}))
  eq(layout_text(), "Kick,Hat,Lead,Bass,Pad,Master,Send 1")
  eq(steps(), 1)
  eq(done.result.created.ref, epoch .. ":track:3")
  eq(done.result.created.kind, "track")
  eq(done.result.layout, layout + 1)
  eq(done.result.applied.kind, "sequencer")
  eq(mock.data.tracks[3].color[1], 255)
  local record = { target = previewed.target, prior = previewed.prior, applied = done.result.applied, created = done.result.created, material = done.result.material }
  local undone = restore(peer, "track-add", record, "Kumi: undo add (kt02)", done.result.epoch, done.result.layout)
  ok(undone.result, json.encode(undone.error or {}))
  eq(layout_text(), "Kick,Hat,Bass,Pad,Master,Send 1")
  eq(steps(), 2)
  -- a send track goes after the master, a group before it; the default places are the ends
  local epoch2, layout2 = place(peer)
  local send = { type = "send", name = "Verb" }
  local p2 = preview(peer, "track-add", send)
  eq(p2.display.after.index, 7)
  local d2 = apply(peer, "track-add", send, "Kumi: add send (kt03)", p2.fence, epoch2, layout2)
  ok(d2.result, json.encode(d2.error or {}))
  eq(layout_text(), "Kick,Hat,Bass,Pad,Master,Send 1,Verb")
  eq(d2.result.applied.kind, "send")
  local group = { type = "group", name = "Drums" }
  local p3 = preview(peer, "track-add", group)
  eq(p3.display.after.index, 5)
  local d3 = apply(peer, "track-add", group, "Kumi: add group (kt04)", p3.fence, epoch2, d2.result.layout)
  ok(d3.result, json.encode(d3.error or {}))
  eq(layout_text(), "Kick,Hat,Bass,Pad,Drums,Master,Send 1,Verb")
  eq(d3.result.applied.kind, "group")
  H.no_dialogs()
end)

test("changes: a track made next to a group says so, and a track made inside one is a member", function()
  local env, peer = setup()
  mock.song:insert_group_at(5)
  mock.song:add_track_to_group(4, 5)
  mock.song:track(5).name = "Bus"
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  eq(layout_text(), "Kick,Hat,Bass,Pad@Bus,Bus,Master,Send 1")
  local epoch, layout = place(peer)
  -- at the group's place: it joins the group
  local before = preview(peer, "track-add", { index = 5 })
  ok(before.display.note:find("joins the group Bus", 1, true), before.display.note)
  local input = { name = "Pad 2", group = track_ref(epoch, 5) }
  local previewed = preview(peer, "track-add", input)
  local done = apply(peer, "track-add", input, "Kumi: add in group (kt11)", previewed.fence, epoch, layout)
  ok(done.result, json.encode(done.error or {}))
  eq(layout_text(), "Kick,Hat,Bass,Pad@Bus,Pad 2@Bus,Bus,Master,Send 1")
  eq(done.result.applied.group, 6)
  failure(ask(peer, "change_preview", "track-add", { index = 2, group = track_ref(epoch, 5) }), "bad-params")
  failure(ask(peer, "change_preview", "track-add", { group = track_ref(epoch, 2) }), "bad-params")
  failure(ask(peer, "change_preview", "track-add", { type = "send", group = track_ref(epoch, 5) }), "bad-params")
  H.no_dialogs()
end)

test("changes: a made track is not removed when it holds something, or when a group has tracks in it", function()
  local env, peer = setup()
  local epoch, layout = place(peer)
  local input = { name = "Lead", index = 2 }
  local previewed = preview(peer, "track-add", input)
  local done = apply(peer, "track-add", input, "Kumi: add (kt21)", previewed.fence, epoch, layout).result
  local record = { target = previewed.target, prior = previewed.prior, applied = done.applied, created = done.created, material = done.material }
  mock.song:track(2):insert_device_at("Audio/Effects/Native/Gainer", 2)
  H.tick(2)
  local now, now_layout = place(peer)
  record.created = { ref = now .. ":track:2", kind = "track" }
  failure(restore(peer, "track-add", record, "Kumi: undo add (kt22)", now, now_layout), "changed")
  eq(layout_text(), "Kick,Lead,Hat,Bass,Pad,Master,Send 1")
  local forced = restore(peer, "track-add", record, "Kumi: undo add (kt22)", now, now_layout, { discard = true })
  ok(forced.result, json.encode(forced.error or {}))
  eq(layout_text(), "Kick,Hat,Bass,Pad,Master,Send 1")
  -- a group that got members stays
  local g_epoch, g_layout = place(peer)
  local group = { type = "group", name = "Bus", index = 5 }
  local gp = preview(peer, "track-add", group)
  local gd = apply(peer, "track-add", group, "Kumi: add group (kt23)", gp.fence, g_epoch, g_layout).result
  mock.song:add_track_to_group(4, 5)
  H.tick(2)
  local e2, l2 = place(peer)
  local grecord = { target = gp.target, prior = gp.prior, applied = gd.applied, created = { ref = e2 .. ":track:5", kind = "track" }, material = gd.material }
  failure(restore(peer, "track-add", grecord, "Kumi: undo group (kt24)", e2, l2, { discard = true }), "changed")
  eq(layout_text(), "Kick,Hat,Bass,Pad@Bus,Bus,Master,Send 1")
  H.no_dialogs()
end)

test("changes: a track is refused a place it can't have", function()
  local env, peer = setup()
  local epoch = place(peer)
  for _, input in ipairs({
    { index = 0 }, { index = 6 }, { index = 2.5 }, { type = "send", index = 5 }, { type = "send", index = 9 }, { type = "group", index = 6 },
    { type = "thing" }, { color = "red" }, { name = 5 },
  }) do
    failure(ask(peer, "change_preview", "track-add", input), "bad-params")
  end
  eq(steps(), 0)
  H.no_dialogs()
end)

test("changes: a track is moved by swaps among the tracks it can pass, and moved back", function()
  local env, peer = setup()
  local epoch, layout = place(peer)
  local ref = track_ref(epoch, 1)
  local previewed = preview(peer, "track-move", { ref = ref, index = 3 })
  eq(previewed.display.title, "Moved track “Kick” to place 3")
  eq(steps(), 0)
  local done = apply(peer, "track-move", { ref = ref, index = 3 }, "Kumi: move (kv01)", previewed.fence, epoch, layout)
  ok(done.result, json.encode(done.error or {}))
  eq(layout_text(), "Hat,Bass,Kick,Pad,Master,Send 1")
  eq(steps(), 1)
  eq(done.result.applied.index, 3)
  eq(done.result.material.track, 3)
  ok(done.result.epoch ~= epoch, "swaps move the epoch")
  local record = { target = previewed.target, prior = previewed.prior, applied = done.result.applied, material = done.result.material }
  local undone = restore(peer, "track-move", record, "Kumi: undo move (kv02)", done.result.epoch, done.result.layout)
  ok(undone.result, json.encode(undone.error or {}))
  eq(layout_text(), "Kick,Hat,Bass,Pad,Master,Send 1")
  eq(steps(), 2)
  -- already back
  local again = restore(peer, "track-move", record, "Kumi: undo move (kv03)", undone.result.epoch, undone.result.layout)
  failure(again, "changed")
  -- edges
  local now = place(peer)
  failure(ask(peer, "change_preview", "track-move", { ref = track_ref(now, 1), index = 5 }), "bad-params")
  failure(ask(peer, "change_preview", "track-move", { ref = track_ref(now, 5), index = 1 }), "bad-params")
  failure(ask(peer, "change_preview", "track-move", { ref = track_ref(now, 6), index = 1 }), "bad-params")
  eq(preview(peer, "track-move", { ref = track_ref(now, 2), index = 2 }).display.unchanged, true)
  H.no_dialogs()
end)

test("changes: a track the producer moved since is not moved back, and the journal follows the move", function()
  local env, peer = setup()
  local epoch, layout = place(peer)
  local ref = track_ref(epoch, 1)
  local previewed = preview(peer, "track-move", { ref = ref, index = 2 })
  local done = apply(peer, "track-move", { ref = ref, index = 2 }, "Kumi: move (kv11)", previewed.fence, epoch, layout).result
  eq(layout_text(), "Hat,Kick,Bass,Pad,Master,Send 1")
  -- a volume change of Kick (track 2 now) is journalled and follows the producer's swap of the two
  local now, now_layout = place(peer)
  local vp = preview(peer, "track-volume", { ref = track_ref(now, 2), volumeDb = -6 })
  ok(apply(peer, "track-volume", { ref = track_ref(now, 2), volumeDb = -6 }, "Kumi: vol (kv12)", vp.fence, now, now_layout).result)
  mock.song:swap_tracks_at(1, 2)
  H.tick(3)
  H.events(peer, "undone_in_daw")
  mock.song:undo() -- the swap
  H.tick(3)
  drain(peer)
  local told = H.events(peer, "undone_in_daw")
  eq(#told, 0, "an undo of the producer's own swap is not Kumi's change")
  -- the producer's undo of Kumi's volume change is told, though the tracks moved since
  mock.song:undo()
  H.tick(3)
  drain(peer)
  told = H.events(peer, "undone_in_daw")
  eq(#told, 1)
  eq(told[1].data.descs[1], "Kumi: vol (kv12)")
  H.no_dialogs()
end)

test("changes: the producer's undo of a move is told", function()
  local env, peer = setup()
  local epoch, layout = place(peer)
  local ref = track_ref(epoch, 1)
  local fence = preview(peer, "track-move", { ref = ref, index = 3 }).fence
  apply(peer, "track-move", { ref = ref, index = 3 }, "Kumi: move (kv21)", fence, epoch, layout)
  H.tick(2)
  H.events(peer, "undone_in_daw")
  mock.song:undo()
  H.tick(3)
  drain(peer)
  local told = H.events(peer, "undone_in_daw")
  eq(#told, 1)
  eq(told[1].data.how, "undo")
  eq(told[1].data.descs[1], "Kumi: move (kv21)")
  eq(layout_text(), "Kick,Hat,Bass,Pad,Master,Send 1")
  mock.song:redo()
  H.tick(3)
  drain(peer)
  told = H.events(peer, "undone_in_daw")
  eq(told[#told].data.how, "redo")
  H.no_dialogs()
end)

local function with_group()
  local env, peer = setup()
  mock.song:insert_group_at(4)
  mock.song:track(4).name = "Bus"
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  return env, peer
end

test("changes: a track before a group goes into it as its last, and comes back to where it was", function()
  local env, peer = with_group()
  eq(layout_text(), "Kick,Hat,Bass,Bus,Pad,Master,Send 1")
  local epoch, layout = place(peer)
  local input = { ref = track_ref(epoch, 1), group = track_ref(epoch, 4) }
  local previewed = preview(peer, "track-group", input)
  eq(previewed.display.title, "Put “Kick” into the group “Bus”")
  local done = apply(peer, "track-group", input, "Kumi: group (kg01)", previewed.fence, epoch, layout)
  ok(done.result, json.encode(done.error or {}))
  eq(layout_text(), "Hat,Bass,Kick@Bus,Bus,Pad,Master,Send 1")
  eq(steps(), 1)
  eq(done.result.applied.index, 3)
  eq(done.result.applied.into, true)
  eq(done.result.material.track, 3)
  eq(done.result.material.groupRef, done.result.epoch .. ":track:4")
  -- put back
  local undone = restore(peer, "track-group", { target = previewed.target, prior = previewed.prior, applied = done.result.applied, material = done.result.material },
    "Kumi: undo group (kg02)", done.result.epoch, done.result.layout)
  ok(undone.result, json.encode(undone.error or {}))
  eq(layout_text(), "Kick,Hat,Bass,Bus,Pad,Master,Send 1")
  eq(steps(), 2)
  -- a second one goes in after another, then both come out the other way round
  local e2, l2 = place(peer)
  local first = { ref = track_ref(e2, 2), group = track_ref(e2, 4) }
  local p1 = preview(peer, "track-group", first)
  local d1 = apply(peer, "track-group", first, "Kumi: group (kg03)", p1.fence, e2, l2)
  ok(d1.result, json.encode(d1.error or {}))
  eq(layout_text(), "Kick,Bass,Hat@Bus,Bus,Pad,Master,Send 1")
  local e3, l3 = d1.result.epoch, d1.result.layout
  local second = { ref = track_ref(e3, 1), group = track_ref(e3, 4) }
  local p2 = preview(peer, "track-group", second)
  local d2 = apply(peer, "track-group", second, "Kumi: group (kg04)", p2.fence, e3, l3)
  ok(d2.result, json.encode(d2.error or {}))
  eq(layout_text(), "Bass,Hat@Bus,Kick@Bus,Bus,Pad,Master,Send 1")
  local r2 = restore(peer, "track-group", { target = p2.target, prior = p2.prior, applied = d2.result.applied, material = d2.result.material },
    "Kumi: undo group (kg05)", d2.result.epoch, d2.result.layout)
  ok(r2.result, json.encode(r2.error or {}))
  eq(layout_text(), "Kick,Bass,Hat@Bus,Bus,Pad,Master,Send 1")
  H.no_dialogs()
end)

test("changes: the producer's undo and redo of a track put into the empty group just after it are told (it doesn't move)", function()
  local env, peer = with_group()
  local epoch, layout = place(peer)
  local input = { ref = track_ref(epoch, 3), group = track_ref(epoch, 4) }
  local done = apply(peer, "track-group", input, "Kumi: group (kg21)", preview(peer, "track-group", input).fence, epoch, layout)
  ok(done.result, json.encode(done.error or {}))
  eq(layout_text(), "Kick,Hat,Bass@Bus,Bus,Pad,Master,Send 1")
  local function told_after(action)
    H.tick(2)
    H.events(peer, "undone_in_daw")
    action()
    H.tick(3)
    drain(peer)
    return H.events(peer, "undone_in_daw")
  end
  local told = told_after(function() mock.song:undo() end)
  eq(layout_text(), "Kick,Hat,Bass,Bus,Pad,Master,Send 1")
  eq(#told, 1, "undone")
  eq(told[1].data.how, "undo")
  eq(told[1].data.descs[1], "Kumi: group (kg21)")
  told = told_after(function() mock.song:redo() end)
  eq(#told, 1, "redone")
  eq(told[1].data.how, "redo")
end)

test("changes: a track in a group comes out to just before its tracks, and goes back to its place among them", function()
  local env, peer = with_group()
  mock.song:add_track_to_group(1, 4)
  mock.song:add_track_to_group(1, 4)
  mock.song:add_track_to_group(1, 4)
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  local text = layout_text()
  eq(text, "Kick@Bus,Hat@Bus,Bass@Bus,Bus,Pad,Master,Send 1")
  local epoch, layout = place(peer)
  local input = { ref = track_ref(epoch, 2), group = nil }
  input.group = json.null
  local previewed = preview(peer, "track-group", input)
  local done = apply(peer, "track-group", input, "Kumi: out (kg11)", previewed.fence, epoch, layout)
  ok(done.result, json.encode(done.error or {}))
  eq(layout_text(), "Hat,Kick@Bus,Bass@Bus,Bus,Pad,Master,Send 1")
  eq(done.result.applied.into, false)
  eq(done.result.applied.index, 1)
  eq(done.result.undo.entry, true)
  local undone = restore(peer, "track-group", { target = previewed.target, prior = previewed.prior, applied = done.result.applied, material = done.result.material },
    "Kumi: undo out (kg12)", done.result.epoch, done.result.layout)
  ok(undone.result, json.encode(undone.error or {}))
  eq(layout_text(), text)
  H.no_dialogs()
end)

test("changes: a track is refused a group it can't join or leave", function()
  local env, peer = with_group()
  local epoch = place(peer)
  -- after the group
  failure(ask(peer, "change_preview", "track-group", { ref = track_ref(epoch, 5), group = track_ref(epoch, 4) }), "bad-params")
  -- not a group
  failure(ask(peer, "change_preview", "track-group", { ref = track_ref(epoch, 1), group = track_ref(epoch, 2) }), "bad-params")
  -- not in a group
  failure(ask(peer, "change_preview", "track-group", { ref = track_ref(epoch, 1), group = json.null }), "bad-params")
  -- no group named
  failure(ask(peer, "change_preview", "track-group", { ref = track_ref(epoch, 1) }), "bad-params")
  -- a send track
  failure(ask(peer, "change_preview", "track-group", { ref = track_ref(epoch, 7), group = track_ref(epoch, 4) }), "bad-params")
  H.no_dialogs()
end)

-- The sequence

local function seq_text() return table.concat(mock.data.sequence, ",") end
local function seq_ref(epoch, s) return epoch .. ":seq:" .. s end

test("changes: after Renoise's own undo and redo of a track move, a slot made or a slot deleted, Kumi's undo puts it back (of: the change's step)", function()
  local env, peer = setup()
  local function told() H.tick(3) drain(peer) H.events(peer, "undone_in_daw") end
  local function undo_redo() told() mock.song:undo() told() mock.song:redo() told() end
  -- a track move
  local epoch, layout = place(peer)
  local move = { ref = track_ref(epoch, 1), index = 3 }
  local p1 = preview(peer, "track-move", move)
  local d1 = apply(peer, "track-move", move, "Kumi: move (kw01)", p1.fence, epoch, layout)
  undo_redo()
  eq(layout_text(), "Hat,Bass,Kick,Pad,Master,Send 1")
  local r1 = restore(peer, "track-move", { target = p1.target, prior = p1.prior, applied = d1.result.applied, material = d1.result.material },
    "Kumi: undo (kw02)", d1.result.epoch, d1.result.layout, { of = "Kumi: move (kw01)" })
  ok(r1.result, "the move Renoise's redo made again: " .. json.encode(r1.error or {}))
  eq(layout_text(), "Kick,Hat,Bass,Pad,Master,Send 1")
  -- a slot made
  epoch, layout = place(peer)
  local p2 = preview(peer, "slot-insert", { index = 2 })
  local d2 = apply(peer, "slot-insert", { index = 2 }, "Kumi: slot (kw03)", p2.fence, epoch, layout)
  undo_redo()
  eq(seq_text(), "1,4,2,1,3")
  local r2 = restore(peer, "slot-insert", { target = p2.target, prior = p2.prior, applied = d2.result.applied, created = d2.result.created },
    "Kumi: undo (kw04)", d2.result.epoch, d2.result.layout, { of = "Kumi: slot (kw03)" })
  ok(r2.result, "the slot Renoise's redo made again: " .. json.encode(r2.error or {}))
  eq(seq_text(), "1,2,1,3")
  -- without the change's step name it is refused as before
  epoch, layout = place(peer)
  local d2b = apply(peer, "slot-insert", { index = 2 }, "Kumi: slot (kw05)", preview(peer, "slot-insert", { index = 2 }).fence, epoch, layout)
  undo_redo()
  failure(restore(peer, "slot-insert", { target = p2.target, prior = p2.prior, applied = d2b.result.applied, created = d2b.result.created },
    "Kumi: undo (kw06)", d2b.result.epoch, d2b.result.layout), "stale")
  -- a slot deleted
  epoch, layout = place(peer)
  local del = { ref = seq_ref(epoch, 3) }
  local p3 = preview(peer, "slot-delete", del)
  local d3 = apply(peer, "slot-delete", del, "Kumi: delete (kw07)", p3.fence, epoch, layout)
  local deleted = seq_text()
  undo_redo()
  eq(seq_text(), deleted)
  local r3 = restore(peer, "slot-delete", { target = p3.target, prior = p3.prior, applied = d3.result.applied, material = d3.result.material },
    "Kumi: undo (kw08)", d3.result.epoch, d3.result.layout, { of = "Kumi: delete (kw07)" })
  ok(r3.result, "the slot Renoise's redo deleted again: " .. json.encode(r3.error or {}))
  H.no_dialogs()
end)

test("changes: a slot is made with a new pattern or an existing one, and removed again with the pattern it made", function()
  local env, peer = setup()
  local epoch, layout = place(peer)
  eq(seq_text(), "1,2,1,3")
  eq(#mock.data.patterns, 3)
  local input = { index = 2 }
  local previewed = preview(peer, "slot-insert", input)
  eq(previewed.display.title, "Inserted slot 2 (a new pattern)")
  eq(steps(), 0)
  local done = apply(peer, "slot-insert", input, "Kumi: slot (ks01)", previewed.fence, epoch, layout)
  ok(done.result, json.encode(done.error or {}))
  eq(seq_text(), "1,4,2,1,3")
  eq(#mock.data.patterns, 4)
  eq(steps(), 1)
  eq(done.result.applied.pattern, done.result.epoch .. ":pattern:4")
  eq(done.result.created.ref, done.result.epoch .. ":seq:2")
  ok(done.result.epoch ~= epoch, "the sequence moved the epoch")
  local record = { target = previewed.target, prior = previewed.prior, applied = done.result.applied, created = done.result.created }
  local undone = restore(peer, "slot-insert", record, "Kumi: undo slot (ks02)", done.result.epoch, done.result.layout)
  ok(undone.result, json.encode(undone.error or {}))
  eq(seq_text(), "1,2,1,3")
  eq(#mock.data.patterns, 3, "the empty pattern the slot made went with it")
  eq(steps(), 2)
  -- an existing pattern, at the end
  local e2, l2 = place(peer)
  local existing = { pattern = pattern_ref(e2, 2) }
  local p2 = preview(peer, "slot-insert", existing)
  eq(p2.display.title, "Inserted slot 5 (pattern “Pattern 01”)")
  local d2 = apply(peer, "slot-insert", existing, "Kumi: slot (ks03)", p2.fence, e2, l2)
  ok(d2.result, json.encode(d2.error or {}))
  eq(seq_text(), "1,2,1,3,2")
  eq(#mock.data.patterns, 3)
  local e3 = place(peer)
  failure(ask(peer, "change_preview", "slot-insert", { index = 9 }), "bad-params")
  failure(ask(peer, "change_preview", "slot-insert", { pattern = pattern_ref(e3, 9) }), "gone")
  H.no_dialogs()
end)

test("changes: a made slot is not removed when it got a section or a mute since, unless Kumi is told to", function()
  local env, peer = setup()
  local epoch, layout = place(peer)
  local input = { index = 1 }
  local previewed = preview(peer, "slot-insert", input)
  local done = apply(peer, "slot-insert", input, "Kumi: slot (ks11)", previewed.fence, epoch, layout).result
  local record = { target = previewed.target, prior = previewed.prior, applied = done.applied, created = done.created }
  mock.song.sequencer:set_track_sequence_slot_is_muted(2, 1, true)
  H.tick(2)
  failure(restore(peer, "slot-insert", record, "Kumi: undo (ks12)", done.epoch, done.layout), "changed")
  eq(#mock.data.sequence, 5)
  local forced = restore(peer, "slot-insert", record, "Kumi: undo (ks12)", done.epoch, done.layout, { discard = true })
  ok(forced.result, json.encode(forced.error or {}))
  eq(#mock.data.sequence, 4)
  H.no_dialogs()
end)

local function sectioned()
  local env, peer = setup()
  local q = mock.song.sequencer
  q:set_sequence_is_start_of_section(3, true)
  q:set_sequence_section_name(3, "Drop")
  q:set_track_sequence_slot_is_muted(2, 3, true)
  q:set_track_sequence_slot_is_muted(4, 3, true)
  -- pattern 3 (slot 4) holds a note
  mock.song:pattern(3):track(1):line(1):note_column(1).note_string = "C-4"
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  return env, peer
end

test("changes: a deleted slot comes back with its pattern, section and muted tracks", function()
  local env, peer = sectioned()
  local epoch, layout = place(peer)
  local ref = seq_ref(epoch, 3)
  local previewed = preview(peer, "slot-delete", { ref = ref })
  eq(previewed.display.title, "Deleted slot 3 (pattern “Pattern 00”)")
  eq(steps(), 0)
  local done = apply(peer, "slot-delete", { ref = ref }, "Kumi: delete slot (kl01)", previewed.fence, epoch, layout)
  ok(done.result, json.encode(done.error or {}))
  eq(seq_text(), "1,2,3")
  eq(steps(), 1)
  eq(done.result.material.pattern, done.result.epoch .. ":pattern:1")
  eq(done.result.material.section.name, "Drop")
  eq(#done.result.material.mutes, 2)
  eq(done.result.material.gone, nil, "pattern 1 is used by slot 1 still")
  eq(done.result.material.next.start, false)
  eq(mock.song.sequencer:sequence_is_start_of_section(3), true, "(Renoise makes the next slot start the section)")
  local record = { target = previewed.target, prior = previewed.prior, applied = done.result.applied, material = done.result.material }
  local undone = restore(peer, "slot-delete", record, "Kumi: undo delete slot (kl02)", done.result.epoch, done.result.layout)
  ok(undone.result, json.encode(undone.error or {}))
  eq(seq_text(), "1,2,1,3")
  local q = mock.song.sequencer
  eq(q:sequence_is_start_of_section(3), true)
  eq(q:sequence_section_name(3), "Drop")
  eq(q:track_sequence_slot_is_muted(2, 3), true)
  eq(q:track_sequence_slot_is_muted(4, 3), true)
  eq(q:track_sequence_slot_is_muted(1, 3), false)
  eq(q:sequence_is_start_of_section(4), false, "the slot after it started no section")
  eq(q:track_sequence_slot_is_muted(2, 4), false)
  eq(undone.result.restored.ref, undone.result.epoch .. ":seq:3")
  eq(steps(), 2)
  -- asked again: it is back, nothing is written
  local again = restore(peer, "slot-delete", record, "Kumi: undo (kl03)", undone.result.epoch, undone.result.layout)
  ok(again.result, json.encode(again.error or {}))
  eq(again.result.undo.entry, false)
  eq(steps(), 2)
  H.no_dialogs()
end)

test("changes: a deleted slot over a pattern of its own keeps the pattern with something in it, and makes an empty one again", function()
  local env, peer = sectioned()
  local epoch, layout = place(peer)
  -- slot 4 is pattern 3, which holds a note: the pattern stays in the pool
  local ref4 = seq_ref(epoch, 4)
  local p4 = preview(peer, "slot-delete", { ref = ref4 })
  eq(p4.display.note, nil)
  local d4 = apply(peer, "slot-delete", { ref = ref4 }, "Kumi: d (kl11)", p4.fence, epoch, layout)
  ok(d4.result, json.encode(d4.error or {}))
  eq(#mock.data.patterns, 3, "the pattern stays")
  eq(d4.result.material.gone, nil)
  local r4 = restore(peer, "slot-delete", { target = p4.target, prior = p4.prior, applied = d4.result.applied, material = d4.result.material },
    "Kumi: undo (kl12)", d4.result.epoch, d4.result.layout)
  ok(r4.result, json.encode(r4.error or {}))
  eq(seq_text(), "1,2,1,3")
  -- slot 2 is pattern 2: empty and used by no other slot, but not the last of the pool: Renoise keeps it
  mock.song:pattern(2).name = "Verse"
  local e1, l1 = place(peer)
  local named = preview(peer, "slot-delete", { ref = seq_ref(e1, 2) })
  eq(named.display.note, nil)
  local dn = apply(peer, "slot-delete", { ref = seq_ref(e1, 2) }, "Kumi: d (kl13)", named.fence, e1, l1)
  ok(dn.result, json.encode(dn.error or {}))
  eq(#mock.data.patterns, 3, "an earlier pattern stays")
  eq(dn.result.material.gone, nil)
  local rn = restore(peer, "slot-delete", { target = named.target, prior = named.prior, applied = dn.result.applied, material = dn.result.material },
    "Kumi: undo (kl14)", dn.result.epoch, dn.result.layout)
  ok(rn.result, json.encode(rn.error or {}))
  eq(seq_text(), "1,2,1,3")
  -- a slot made at the end holds the pool's last pattern, empty: Renoise removes it with the slot
  local e0, l0 = place(peer)
  local added = apply(peer, "slot-insert", {}, "Kumi: slot (kl20)", preview(peer, "slot-insert", {}).fence, e0, l0)
  ok(added.result, json.encode(added.error or {}))
  eq(#mock.data.patterns, 4)
  mock.song:pattern(4).name = "Outro"
  mock.song:pattern(4).number_of_lines = 48
  local e2, l2 = place(peer)
  local ref2 = seq_ref(e2, 5)
  local p2 = preview(peer, "slot-delete", { ref = ref2 })
  ok(p2.display.note:find("removes it", 1, true), p2.display.note)
  local d2 = apply(peer, "slot-delete", { ref = ref2 }, "Kumi: d (kl15)", p2.fence, e2, l2)
  ok(d2.result, json.encode(d2.error or {}))
  eq(#mock.data.patterns, 3, "Renoise removed the last, empty pattern")
  eq(d2.result.material.gone.name, "Outro")
  eq(d2.result.material.gone.lines, 48)
  eq(seq_text(), "1,2,1,3")
  local r2 = restore(peer, "slot-delete", { target = p2.target, prior = p2.prior, applied = d2.result.applied, material = d2.result.material },
    "Kumi: undo (kl16)", d2.result.epoch, d2.result.layout)
  ok(r2.result, json.encode(r2.error or {}))
  eq(#mock.data.patterns, 4)
  eq(seq_text(), "1,2,1,3,4")
  eq(mock.song:pattern(4).name, "Outro")
  eq(mock.song:pattern(4).number_of_lines, 48)
  H.no_dialogs()
end)

test("changes: the only slot is not deleted, and a slot changed since the preview is not either", function()
  local env, peer = setup()
  local epoch, layout = place(peer)
  local ref = seq_ref(epoch, 2)
  local previewed = preview(peer, "slot-delete", { ref = ref })
  mock.song.sequencer:set_track_sequence_slot_is_muted(1, 2, true)
  failure(apply(peer, "slot-delete", { ref = ref }, "Kumi: d (kl21)", previewed.fence, epoch, layout), "changed")
  eq(#mock.data.sequence, 4)
  failure(ask(peer, "change_preview", "slot-delete", { ref = seq_ref(epoch, 9) }), "gone")
  H.no_dialogs()
end)

test("changes: a slot plays another pattern unless that would make Renoise remove the one it played", function()
  local env, peer = setup()
  local epoch, layout = place(peer)
  -- slot 4 holds pattern 3, the pool's last, empty and used by no other slot
  local refused = ask(peer, "change_preview", "slot-pattern", { ref = seq_ref(epoch, 4), pattern = pattern_ref(epoch, 2) })
  failure(refused, "bad-params")
  ok(refused.error.message:find("slot-insert", 1, true), refused.error.message)
  -- slot 3 (pattern 1) is used by slot 1 as well
  local input = { ref = seq_ref(epoch, 3), pattern = pattern_ref(epoch, 3) }
  local previewed = preview(peer, "slot-pattern", input)
  eq(previewed.display.title, "Slot 3 → pattern “Pattern 02”")
  eq(previewed.prior.pattern, epoch .. ":pattern:1")
  local done = apply(peer, "slot-pattern", input, "Kumi: pattern (km01)", previewed.fence, epoch, layout)
  ok(done.result, json.encode(done.error or {}))
  eq(seq_text(), "1,2,3,3")
  eq(steps(), 1)
  eq(done.result.applied.pattern, epoch .. ":pattern:3")
  local undone = restore(peer, "slot-pattern", { target = previewed.target, prior = previewed.prior, applied = done.result.applied }, "Kumi: undo (km02)", epoch, layout)
  ok(undone.result, json.encode(undone.error or {}))
  eq(seq_text(), "1,2,1,3")
  -- changed since
  local d2 = apply(peer, "slot-pattern", input, "Kumi: pattern (km03)", preview(peer, "slot-pattern", input).fence, epoch, layout)
  ok(d2.result, json.encode(d2.error or {}))
  mock.song.sequencer:set_pattern(3, 2)
  failure(restore(peer, "slot-pattern", { target = previewed.target, prior = previewed.prior, applied = d2.result.applied }, "Kumi: undo (km04)", epoch, layout), "changed")
  H.no_dialogs()
end)

test("changes: a slot starts a section, is named, and loses it again", function()
  local env, peer = setup()
  local epoch, layout = place(peer)
  local ref = seq_ref(epoch, 3)
  local input = { ref = ref, name = "Drop" }
  local previewed = preview(peer, "slot-section", input)
  eq(previewed.display.title, "Section “Drop” at slot 3")
  eq(previewed.prior.start, false)
  local done = apply(peer, "slot-section", input, "Kumi: section (kn91)", previewed.fence, epoch, layout)
  ok(done.result, json.encode(done.error or {}))
  eq(mock.song.sequencer:sequence_is_start_of_section(3), true)
  eq(mock.song.sequencer:sequence_section_name(3), "Drop")
  eq(steps(), 1)
  local undone = restore(peer, "slot-section", { target = previewed.target, prior = previewed.prior, applied = done.result.applied }, "Kumi: undo (kn92)", epoch, layout)
  ok(undone.result, json.encode(undone.error or {}))
  eq(mock.song.sequencer:sequence_is_start_of_section(3), false)
  eq(mock.song.sequencer:sequence_section_name(3), "", "the name it kept hidden is the one it had (real Renoise kept “Drop”)")
  -- a rename keeps the start; clearing needs no name
  local p2 = preview(peer, "slot-section", { ref = ref, start = true })
  eq(p2.display.title, "Section at slot 3")
  failure(ask(peer, "change_preview", "slot-section", { ref = ref, start = false, name = "x" }), "bad-params")
  failure(ask(peer, "change_preview", "slot-section", { ref = ref }), "bad-params")
  failure(ask(peer, "change_preview", "slot-section", { ref = ref, start = "yes" }), "bad-params")
  H.no_dialogs()
end)

test("changes: a track is muted in a slot as an undo step, found again by its slot ref, and carried when tracks are made", function()
  local env, peer = setup()
  local epoch, layout = place(peer)
  local input = { ref = seq_ref(epoch, 2), track = track_ref(epoch, 3), mute = true }
  local previewed = preview(peer, "slot-mute", input)
  eq(previewed.display.title, "Bass in slot 2 muted")
  eq(previewed.target.ref, epoch .. ":slot:3.2")
  local done = apply(peer, "slot-mute", input, "Kumi: slot mute (kp01)", previewed.fence, epoch, layout)
  ok(done.result, json.encode(done.error or {}))
  eq(mock.song.sequencer:track_sequence_slot_is_muted(3, 2), true)
  eq(done.result.undo.entry, true)
  eq(steps(), 1)
  mock.api.insert_track_at(1)
  H.tick(3)
  local undone = restore(peer, "slot-mute", { target = previewed.target, prior = previewed.prior, applied = done.result.applied }, "Kumi: undo (kp02)", epoch, layout)
  ok(undone.result, json.encode(undone.error or {}))
  eq(mock.song.sequencer:track_sequence_slot_is_muted(4, 2), false, "Bass is track 4 now")
  failure(ask(peer, "change_preview", "slot-mute", { ref = seq_ref(epoch, 2), track = track_ref(epoch, 3), mute = "yes" }), "bad-params")
  failure(ask(peer, "change_preview", "slot-mute", { ref = seq_ref(epoch, 2), mute = true }), "bad-params")
  H.no_dialogs()
end)

test("changes: the producer's undo of a slot made, deleted or muted is told", function()
  local env, peer = setup()
  local epoch, layout = place(peer)
  local function told_after(action)
    H.tick(2)
    H.events(peer, "undone_in_daw")
    action()
    H.tick(3)
    drain(peer)
    return H.events(peer, "undone_in_daw")
  end
  local made = { index = 2 }
  apply(peer, "slot-insert", made, "Kumi: slot (kq01)", preview(peer, "slot-insert", made).fence, epoch, layout)
  local told = told_after(function() mock.song:undo() end)
  eq(#told, 1, "made, undone")
  eq(told[1].data.how, "undo")
  eq(told[1].data.descs[1], "Kumi: slot (kq01)")
  told = told_after(function() mock.song:redo() end)
  eq(told[#told].data.how, "redo")
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  -- deleted
  local e2, l2 = place(peer)
  local ref = seq_ref(e2, 3)
  apply(peer, "slot-delete", { ref = ref }, "Kumi: slot (kq02)", preview(peer, "slot-delete", { ref = ref }).fence, e2, l2)
  told = told_after(function() mock.song:undo() end)
  eq(#told, 1, "deleted, undone")
  eq(told[1].data.descs[1], "Kumi: slot (kq02)")
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  -- muted
  local e3, l3 = place(peer)
  local mute = { ref = seq_ref(e3, 1), track = track_ref(e3, 1), mute = true }
  apply(peer, "slot-mute", mute, "Kumi: slot (kq03)", preview(peer, "slot-mute", mute).fence, e3, l3)
  told = told_after(function() mock.song:undo() end)
  eq(#told, 1, "muted, undone")
  eq(told[1].data.descs[1], "Kumi: slot (kq03)")
  H.no_dialogs()
end)

test("changes: the loop is set and cleared without an undo step, and Kumi says so", function()
  local env, peer = setup()
  local epoch, layout = place(peer)
  local input = { first = 2, last = 3 }
  local previewed = preview(peer, "loop-range", input)
  eq(previewed.display.title, "Loop slots 2 to 3")
  eq(previewed.display.note, "Renoise's own undo won't take this back; Kumi can.")
  eq(previewed.prior.first, 0)
  local done = apply(peer, "loop-range", input, "Kumi: loop (kr01)", previewed.fence, epoch, layout)
  ok(done.result, json.encode(done.error or {}))
  eq(done.result.undo.entry, false)
  eq(steps(), 0)
  eq(mock.song.transport.loop_sequence_range[1], 2)
  eq(mock.song.transport.loop_sequence_range[2], 3)
  local undone = restore(peer, "loop-range", { target = previewed.target, prior = previewed.prior, applied = done.result.applied }, "Kumi: undo (kr02)", epoch, layout)
  ok(undone.result, json.encode(undone.error or {}))
  eq(#mock.song.transport.loop_sequence_range, 0)
  failure(ask(peer, "change_preview", "loop-range", { first = 3, last = 2 }), "bad-params")
  failure(ask(peer, "change_preview", "loop-range", { first = 1, last = 9 }), "bad-params")
  eq(preview(peer, "loop-range", {}).display.unchanged, true)
  local pattern_loop = preview(peer, "loop-pattern", { on = true })
  eq(pattern_loop.display.title, "Pattern loop on")
  local d2 = apply(peer, "loop-pattern", { on = true }, "Kumi: loop (kr03)", pattern_loop.fence, epoch, layout)
  ok(d2.result, json.encode(d2.error or {}))
  eq(mock.song.transport.loop_pattern, true)
  eq(steps(), 0)
  failure(ask(peer, "change_preview", "loop-pattern", { on = "yes" }), "bad-params")
  H.no_dialogs()
end)

test("changes: playing starts the transport where it is or at a line, and the emergency stop stops it", function()
  local env, peer = setup()
  local epoch = place(peer)
  local played = call(peer, "transport_play", {})
  ok(played.result, json.encode(played.error or {}))
  eq(played.result.started, true)
  eq(played.result.playing, true)
  eq(mock.data.started_mode, 2, "from where the pattern is")
  eq(call(peer, "transport_play", {}).result.started, false, "already playing")
  eq(call(peer, "transport_stop", {}).result.stopped, true)
  -- a line of a slot
  local at = call(peer, "transport_play", { sequence = 3, line = 4, epoch = epoch })
  ok(at.result, json.encode(at.error or {}))
  eq(at.result.started, true)
  eq(at.result.playing, true)
  eq(at.result.sequence, 3)
  eq(at.result.line, 4, "Renoise's line 04")
  eq(mock.data.started_at.sequence, 3)
  eq(mock.data.started_at.line, 5, "its fifth")
  call(peer, "transport_stop", {})
  -- a slot alone starts at its first line; a line alone is a line of the pattern being edited
  eq(call(peer, "transport_play", { sequence = 2 }).result.line, 0)
  call(peer, "transport_stop", {})
  local line_only = call(peer, "transport_play", { line = 6 })
  ok(line_only.result, json.encode(line_only.error or {}))
  eq(mock.data.started_at, 7)
  eq(line_only.result.line, 6)
  call(peer, "transport_stop", {})
  failure(call(peer, "transport_play", { sequence = 3, line = 5, epoch = "ffff" }), "stale")
  failure(call(peer, "transport_play", { sequence = 9, line = 5 }), "bad-params")
  failure(call(peer, "transport_play", { sequence = 3, line = 99 }), "bad-params")
  failure(call(peer, "transport_play", { line = -1 }), "bad-params")
  failure(call(peer, "transport_play", { sequence = 0 }), "bad-params")
  failure(call(peer, "transport_play", { line = 1.5 }), "bad-params")
  mock.data.song_fields.rendering = true
  failure(call(peer, "transport_play", {}), "busy")
  failure(call(peer, "transport_play", { sequence = 1 }), "busy")
  mock.data.song_fields.rendering = false
  H.no_dialogs()
end)

-- Track refs across epochs that moved for something else

test("changes: a track ref from before a device change is still that track, and the answer says what was kept", function()
  local env, peer = device_song()
  local e1, l1 = place(peer)
  local volume = { ref = track_ref(e1, 2), volumeDb = -6 }
  local pv = preview(peer, "track-volume", volume)
  -- a device is put in on another track: the epoch moves
  local add = { ref = track_ref(e1, 1), path = "Gainer" }
  local added = apply(peer, "device-add", add, "Kumi: add (kk01)", preview(peer, "device-add", add).fence, e1, l1)
  ok(added.result, json.encode(added.error or {}))
  ok(added.result.epoch ~= e1, "the epoch moved")
  eq(added.result.kept.from, e1)
  eq(added.result.kept.tracks, true)
  -- the volume change made in the old epoch is applied there, and answered in the new one
  local done = apply(peer, "track-volume", volume, "Kumi: vol (kk02)", pv.fence, e1, l1)
  ok(done.result, json.encode(done.error or {}))
  eq(done.result.epoch, added.result.epoch)
  eq(done.result.kept.from, e1)
  ok(math.abs(post_volume(2) - 10 ^ (-6 / 20)) < 1e-6)
  -- and put back from the old epoch too
  local undone = restore(peer, "track-volume", { target = pv.target, prior = pv.prior, applied = done.result.applied }, "Kumi: undo (kk03)", e1, l1)
  ok(undone.result, json.encode(undone.error or {}))
  eq(post_volume(2), 1)
  -- a device ref of the old epoch is not kept: the device numbers moved
  local stale = call(peer, "change_preview", { kind = "device-bypass", input = { ref = e1 .. ":device:3.2", bypass = true }, epoch = e1, layout = l1 })
  failure(stale, "stale")
  -- nor is a request that names a track and a device
  local mixed = call(peer, "change_preview", { kind = "device-move", input = { ref = e1 .. ":device:3.2", index = 3 }, epoch = e1, layout = l1 })
  failure(mixed, "stale")
  H.no_dialogs()
end)

test("changes: a request in the epoch now may name a track by a ref from an earlier one the tracks kept through", function()
  local env, peer = device_song()
  local e1, l1 = place(peer)
  local old_ref = track_ref(e1, 2)
  local add = { ref = track_ref(e1, 1), path = "Gainer" }
  local added = apply(peer, "device-add", add, "Kumi: add (kk31)", preview(peer, "device-add", add).fence, e1, l1)
  ok(added.result, json.encode(added.error or {}))
  local e2, l2 = added.result.epoch, added.result.layout
  ok(e2 ~= e1)
  -- Kumi's request names the epoch now and the track by the ref it kept
  local volume = { ref = old_ref, volumeDb = -6 }
  local pv = call(peer, "change_preview", { kind = "track-volume", input = volume, epoch = e2, layout = l2 })
  ok(pv.result, json.encode(pv.error or {}))
  eq(pv.result.target.ref, track_ref(e2, 2), "the answer names the track in this epoch")
  local done = apply(peer, "track-volume", volume, "Kumi: vol (kk32)", pv.result.fence, e2, l2)
  ok(done.result, json.encode(done.error or {}))
  -- a device ref of the old epoch is not kept
  failure(call(peer, "change_preview", { kind = "device-bypass", input = { ref = e1 .. ":device:3.2", bypass = true }, epoch = e2, layout = l2 }), "stale")
  -- nor a track ref across a swap
  local mv = { ref = track_ref(e2, 1), index = 3 }
  local moved = apply(peer, "track-move", mv, "Kumi: move (kk33)", preview(peer, "track-move", mv).fence, e2, l2)
  ok(moved.result, json.encode(moved.error or {}))
  failure(call(peer, "change_preview", { kind = "track-volume", input = { ref = old_ref, volumeDb = -3 }, epoch = moved.result.epoch, layout = moved.result.layout }), "stale")
  H.no_dialogs()
end)

test("changes: a deleted track is put back after a device change made another epoch", function()
  local env, peer = device_song()
  local e1, l1 = place(peer)
  local previewed = preview(peer, "track-delete", { ref = track_ref(e1, 2) })
  local done = apply(peer, "track-delete", { ref = track_ref(e1, 2) }, "Kumi: del (kk11)", previewed.fence, e1, l1).result
  -- a device goes into Bass (track 2 now) in the new epoch
  local e2, l2 = place(peer)
  local add = { ref = track_ref(e2, 2), path = "Gainer" }
  local added = apply(peer, "device-add", add, "Kumi: add (kk12)", preview(peer, "device-add", add).fence, e2, l2)
  ok(added.result, json.encode(added.error or {}))
  local undone = restore(peer, "track-delete", { target = previewed.target, prior = previewed.prior, applied = done.applied, material = done.material },
    "Kumi: undo del (kk13)", done.epoch, done.layout)
  ok(undone.result, json.encode(undone.error or {}))
  eq(mock.data.tracks[2].name, "Hat")
  H.no_dialogs()
end)

test("changes: tracks that swapped places are not kept: an older track ref is stale and the answer says nothing was kept", function()
  local env, peer = setup()
  local e1, l1 = place(peer)
  local ref = track_ref(e1, 1)
  local moved = apply(peer, "track-move", { ref = ref, index = 3 }, "Kumi: move (kk21)", preview(peer, "track-move", { ref = ref, index = 3 }).fence, e1, l1)
  ok(moved.result, json.encode(moved.error or {}))
  -- what the swap left alone is said; the tracks and what hangs on them are not
  eq(moved.result.kept.tracks, false)
  local kinds = {}
  for _, kind in ipairs(moved.result.kept.kinds) do kinds[kind] = true end
  ok(kinds.pattern and kinds.seq and kinds.instrument and kinds.song, json.encode(moved.result.kept))
  ok(not kinds.track and not kinds.device and not kinds.param and not kinds.slot and not kinds.ptrack, json.encode(moved.result.kept))
  failure(call(peer, "change_preview", { kind = "track-volume", input = { ref = ref, volumeDb = -3 }, epoch = e1, layout = l1 }), "stale")
  H.no_dialogs()
end)

test("changes: a null in a request (taking a track out of its group) survives the refs being carried to the layout now", function()
  local env, peer = with_group()
  mock.song:add_track_to_group(1, 4)
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  local epoch, layout = place(peer)
  -- (Kick is the group's track, at 3: Hat and Bass are before it)
  local member = track_ref(epoch, 3)
  mock.api.insert_track_at(1) -- a track is made before: the layout moves, and the ref is carried
  H.tick(3)
  local pv = call(peer, "change_preview", { kind = "track-group", input = { ref = member, group = json.null }, epoch = epoch, layout = layout })
  ok(pv.result, json.encode(pv.error or {}))
  eq(pv.result.display.title, "Took “Kick” out of the group “Bus”")
  H.no_dialogs()
end)

test("changes: a tempo change made before a sequence or device change is put back after it; a loop range is not", function()
  local env, peer = device_song()
  local e1, l1 = place(peer)
  local tempo = { bpm = 133 }
  local tp = preview(peer, "song-bpm", tempo)
  local td = apply(peer, "song-bpm", tempo, "Kumi: tempo (kj01)", tp.fence, e1, l1)
  ok(td.result, json.encode(td.error or {}))
  local loop = { first = 2, last = 3 }
  local pv = preview(peer, "loop-range", loop)
  local done = apply(peer, "loop-range", loop, "Kumi: loop (kj02)", pv.fence, e1, l1)
  ok(done.result, json.encode(done.error or {}))
  -- the sequence changes (the epoch moves, and slot refs with it)
  local ins = apply(peer, "slot-insert", {}, "Kumi: slot (kj03)", preview(peer, "slot-insert", {}).fence, e1, l1)
  ok(ins.result, json.encode(ins.error or {}))
  ok(ins.result.epoch ~= e1)
  local undone = restore(peer, "song-bpm", { target = tp.target, prior = tp.prior, applied = td.result.applied }, "Kumi: undo tempo (kj04)", e1, l1)
  ok(undone.result, json.encode(undone.error or {}))
  eq(mock.data.transport_fields.bpm, 120)
  -- the loop range names slots: after the sequence changed Kumi's undo would loop other slots, so it is refused
  failure(restore(peer, "loop-range", { target = pv.target, prior = pv.prior, applied = done.result.applied }, "Kumi: undo loop (kj05)", e1, l1), "stale")
  eq(#mock.song.transport.loop_sequence_range, 2, "nothing was written")
  -- a slot ref from before is not kept
  failure(call(peer, "change_preview", { kind = "slot-section", input = { ref = seq_ref(e1, 1), name = "A" }, epoch = e1, layout = l1 }), "stale")
  H.no_dialogs()
end)

local function kinds_of(kept)
  local out = {}
  for _, kind in ipairs(kept.kinds) do out[kind] = true end
  return out
end

test("changes: a device change leaves tracks, patterns, slots and instruments as they were, and a slot change leaves the slots alone no more", function()
  local env, peer = device_song()
  local e1, l1 = place(peer)
  local add = { ref = track_ref(e1, 1), path = "Gainer" }
  local added = apply(peer, "device-add", add, "Kumi: add (kl01)", preview(peer, "device-add", add).fence, e1, l1)
  ok(added.result, json.encode(added.error or {}))
  local kinds = kinds_of(added.result.kept)
  ok(kinds.track and kinds.song and kinds.pattern and kinds.ptrack and kinds.seq and kinds.slot and kinds.instrument and kinds.sample and kinds.phrase,
    json.encode(added.result.kept))
  ok(not kinds.device and not kinds.param, "the device numbers moved: " .. json.encode(added.result.kept))
  eq(added.result.kept.tracks, true)
  -- notes are written to a pattern and a track the request names by refs from before the device was made
  local notes = { pattern = pattern_ref(e1, 1), track = track_ref(e1, 3), lines = { { line = 0, notes = { { col = 1, note = "C-4" } } } } }
  local e2, l2 = added.result.epoch, added.result.layout
  local pv = call(peer, "change_preview", { kind = "notes-write", input = notes, epoch = e2, layout = l2 })
  ok(pv.result, json.encode(pv.error or {}))
  eq(pv.result.target.ref, e2 .. ":ptrack:1.3")
  -- a slot made: the slots are not kept, the rest is
  local existing = { pattern = pattern_ref(e2, 1) }
  local ins = apply(peer, "slot-insert", existing, "Kumi: slot (kl02)", preview(peer, "slot-insert", existing).fence, e2, l2)
  ok(ins.result, json.encode(ins.error or {}))
  local after = kinds_of(ins.result.kept)
  ok(not after.seq and not after.slot, json.encode(ins.result.kept))
  ok(after.track and after.pattern and after.device and after.param and after.instrument, json.encode(ins.result.kept))
  -- and a device ref from before the slot was made still names the device (the sequence moved no devices)
  local e3, l3 = ins.result.epoch, ins.result.layout
  local bypass = call(peer, "change_preview", { kind = "device-bypass", input = { ref = e2 .. ":device:1.2", bypass = true }, epoch = e3, layout = l3 })
  ok(bypass.result, json.encode(bypass.error or {}))
  failure(call(peer, "change_preview", { kind = "slot-section", input = { ref = seq_ref(e2, 1), name = "A" }, epoch = e3, layout = l3 }), "stale")
  H.no_dialogs()
end)

-- Names in titles are in quotes where they are inside the sentence, as Live's titles have them

local function curly(text) return "\226\128\156" .. text .. "\226\128\157" end

test("changes: a track called Track 005 reads right in the titles of rename, delete and restore", function()
  local env, peer = setup()
  mock.song:track(2).name = "Track 005"
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  local epoch, layout = place(peer)
  local ref = track_ref(epoch, 2)
  -- a title that opens with the track's name leaves it bare
  eq(preview(peer, "track-volume", { ref = ref, volumeDb = -6 }).display.title, "Track 005 volume \226\136\1466 dB")
  eq(preview(peer, "track-mute", { ref = ref, mute = true }).display.title, "Track 005 muted")
  -- a name inside the sentence is quoted
  local rename = { ref = ref, name = "Lead Vox" }
  local rp = preview(peer, "track-rename", rename)
  eq(rp.display.title, "Renamed track " .. curly("Track 005") .. " \226\134\146 " .. curly("Lead Vox"))
  local rd = apply(peer, "track-rename", rename, "Kumi: rename (kq01)", rp.fence, epoch, layout)
  ok(rd.result, json.encode(rd.error or {}))
  eq(rd.result.display.title, "Renamed track " .. curly("Track 005") .. " \226\134\146 " .. curly("Lead Vox"))
  local back = restore(peer, "track-rename", { target = rp.target, prior = rp.prior, applied = rd.result.applied }, "Kumi: undo rename (kq02)", epoch, layout)
  ok(back.result, json.encode(back.error or {}))
  eq(back.result.display.title, "Renamed track " .. curly("Lead Vox") .. " \226\134\146 " .. curly("Track 005") .. " (put back)")
  -- delete and restore: no "Track Track 005"
  local dp = preview(peer, "track-delete", { ref = ref })
  eq(dp.display.title, "Deleted track " .. curly("Track 005"))
  local dd = apply(peer, "track-delete", { ref = ref }, "Kumi: delete (kq03)", dp.fence, epoch, layout)
  ok(dd.result, json.encode(dd.error or {}))
  eq(dd.result.display.title, "Deleted track " .. curly("Track 005"))
  local undone = restore(peer, "track-delete", { target = dp.target, prior = dp.prior, applied = dd.result.applied, material = dd.result.material },
    "Kumi: undo delete (kq04)", epoch, dd.result.layout)
  ok(undone.result, json.encode(undone.error or {}))
  eq(undone.result.display.title, curly("Track 005") .. " put back")
  H.no_dialogs()
end)

test("changes: the titles of made and put-back things quote their names", function()
  local env, peer = device_song()
  local epoch, layout = place(peer)
  -- a track made and taken away
  local add = { name = "Pad 2", index = 2 }
  local ap = preview(peer, "track-add", add)
  eq(ap.display.title, "Added track " .. curly("Pad 2"))
  local done = apply(peer, "track-add", add, "Kumi: add (kq11)", ap.fence, epoch, layout)
  ok(done.result, json.encode(done.error or {}))
  eq(done.result.display.title, "Added track " .. curly("Pad 2"))
  local gone = restore(peer, "track-add", { target = ap.target, prior = ap.prior, applied = done.result.applied, created = done.result.created, material = done.result.material },
    "Kumi: undo add (kq12)", done.result.epoch, done.result.layout)
  ok(gone.result, json.encode(gone.error or {}))
  eq(gone.result.display.title, "Removed track " .. curly("Pad 2") .. " (put back)")
  -- a device deleted and put back
  local e2, l2 = place(peer)
  local dref = e2 .. ":device:3.2"
  local dp = preview(peer, "device-delete", { ref = dref })
  eq(dp.display.title, "Deleted " .. curly("EQ 5") .. " from " .. curly("Bass"))
  local dd = apply(peer, "device-delete", { ref = dref }, "Kumi: del (kq13)", dp.fence, e2, l2)
  ok(dd.result, json.encode(dd.error or {}))
  local back = restore(peer, "device-delete", { target = dp.target, prior = dp.prior, applied = dd.result.applied, material = dd.result.material },
    "Kumi: undo del (kq14)", dd.result.epoch, dd.result.layout)
  ok(back.result, json.encode(back.error or {}))
  eq(back.result.display.title, curly("EQ 5") .. " put back on " .. curly("Bass"))
  -- a slot deleted and put back
  local e3, l3 = place(peer)
  local sp = preview(peer, "slot-delete", { ref = seq_ref(e3, 2) })
  local sd = apply(peer, "slot-delete", { ref = seq_ref(e3, 2) }, "Kumi: slot (kq15)", sp.fence, e3, l3)
  ok(sd.result, json.encode(sd.error or {}))
  local sb = restore(peer, "slot-delete", { target = sp.target, prior = sp.prior, applied = sd.result.applied, material = sd.result.material },
    "Kumi: undo slot (kq16)", sd.result.epoch, sd.result.layout)
  ok(sb.result, json.encode(sb.error or {}))
  eq(sb.result.display.title, "Slot 2 put back")
  H.no_dialogs()
end)

-- U2: what a number means in a hex column, an instrument by its ref, and a #Send aimed by the send track itself

test("changes: a note's instrument may be its ref, a number is decimal, and an effect's amount reaches FF", function()
  local env, peer = setup(nil, { instruments = 12 })
  local epoch, layout = place(peer)
  local input = { pattern = pattern_ref(epoch, 1), track = track_ref(epoch, 3), lines = {
    { line = 0, notes = { { col = 1, note = "C-4", instrumentRef = epoch .. ":instrument:11" } } },
    { line = 1, notes = { { col = 1, note = "D-4", instrument = 10 } }, effects = { { col = 1, number = "0A", amount = 255 } } },
  } }
  local previewed = preview(peer, "notes-write", input)
  local done = apply(peer, "notes-write", input, "Kumi: notes (ku21)", previewed.fence, epoch, layout)
  ok(done.result, json.encode(done.error or {}))
  eq(cell(1, 3, 1, 1), "C-4 0A .. .. .. 0000", "the 11th instrument is Renoise's 0A")
  eq(cell(1, 3, 2, 1), "D-4 0A .. .. .. 0000", "instrument 10 is decimal: 0A")
  eq(effect_cell(1, 3, 2, 1), "0AFF")
  local e2, l2 = done.result.epoch, done.result.layout
  local function asked(cells) return call(peer, "change_preview", { kind = "notes-write", epoch = e2, layout = l2,
    input = { pattern = pattern_ref(e2, 1), track = track_ref(e2, 3), lines = { { line = 2, notes = cells } } } }) end
  failure(asked({ { col = 1, note = "C-4", instrument = 1, instrumentRef = e2 .. ":instrument:2" } }), "bad-params")
  failure(asked({ { col = 1, note = "C-4", instrumentRef = track_ref(e2, 1) } }), "bad-params")
  failure(asked({ { col = 1, note = "C-4", instrument = 255 } }), "bad-params")
  H.no_dialogs()
end)

test("changes: a #Send is aimed by receiverRef, the send track; anything else is refused", function()
  local env, peer = device_song()
  local epoch, layout = place(peer)
  local second = mock.song.sequencer_track_count + 3 -- (the second send: the sends come after the master)
  local input = { ref = track_ref(epoch, 1), path = "#Send", receiverRef = track_ref(epoch, second) }
  local previewed = preview(peer, "device-add", input)
  local done = apply(peer, "device-add", input, "Kumi: send (ku22)", previewed.fence, epoch, layout)
  ok(done.result, json.encode(done.error or {}))
  eq(paths_of(1), "Mixer,#Send")
  eq(devices_of(1)[2].params[3].value, 1, "the second send is receiver 1")
  local e2, l2 = done.result.epoch, done.result.layout
  local function asked(more)
    local ask = { ref = track_ref(e2, 1), path = "#Send" }
    for k, v in pairs(more) do ask[k] = v end
    return call(peer, "change_preview", { kind = "device-add", input = ask, epoch = e2, layout = l2 })
  end
  failure(asked({ receiverRef = track_ref(e2, 2) }), "bad-params")
  failure(asked({ receiverRef = track_ref(e2, second), receiver = 1 }), "bad-params")
  failure(asked({ path = "Gainer", receiverRef = track_ref(e2, second) }), "bad-params")
  H.no_dialogs()
end)

test("changes: Kumi's undo of the second of two device adds marks that one undone, so the producer's Cmd-Z of it is told as its redo (#361 N-S3)", function()
  local env, peer = device_song()
  local function add(t, desc)
    local epoch, layout = place(peer)
    local input = { ref = track_ref(epoch, t), path = "Gainer" }
    local previewed = preview(peer, "device-add", input)
    local done = apply(peer, "device-add", input, desc, previewed.fence, epoch, layout)
    ok(done.result, json.encode(done.error or {}))
    H.tick(2)
    return { target = done.result.target, prior = previewed.prior, applied = done.result.applied, created = done.result.created, material = done.result.material }, done.result
  end
  add(1, "Kumi: add one (ks01)")
  local second, done = add(2, "Kumi: add two (ks02)")
  local undone = restore(peer, "device-add", second, "Kumi: undo two (ks03)", done.epoch, done.layout, { of = "Kumi: add two (ks02)" })
  ok(undone.result, json.encode(undone.error or {}))
  H.tick(2)
  drain(peer)
  peer.events = {}
  mock.song:undo() -- the producer takes Kumi's undo back: the second Gainer again
  H.tick(3)
  drain(peer)
  local told = H.events(peer, "undone_in_daw")
  eq(#told, 1, "told once")
  eq(told[1].data.how, "redo")
  eq(told[1].data.descs[1], "Kumi: add two (ks02)", "the change Kumi undid, not the first add")
end)

test("changes: Kumi's undo of columns it showed refuses to hide a note written in them since; discard hides it (#361 N-S7)", function()
  local env, peer = setup()
  local epoch, layout = place(peer)
  local input = { ref = track_ref(epoch, 3), noteColumns = 4 }
  local previewed = preview(peer, "track-columns", input)
  local done = apply(peer, "track-columns", input, "Kumi: columns (kc71)", previewed.fence, epoch, layout)
  ok(done.result, json.encode(done.error or {}))
  eq(mock.song:track(3).visible_note_columns, 4)
  mock.ops.set_note(1, 3, 1, 4, { note = 48 }) -- the producer writes a chord note in column 4
  H.tick(2)
  local record = { target = previewed.target, prior = previewed.prior, applied = done.result.applied }
  local refused = restore(peer, "track-columns", record, "Kumi: undo columns (kc72)", done.result.epoch, done.result.layout)
  failure(refused, "changed")
  ok(refused.error.message:find("written since", 1, true), refused.error.message)
  eq(mock.song:track(3).visible_note_columns, 4, "still shown")
  local forced = restore(peer, "track-columns", record, "Kumi: undo columns (kc73)", done.result.epoch, done.result.layout, { discard = true })
  ok(forced.result, json.encode(forced.error or {}))
  eq(mock.song:track(3).visible_note_columns, 2)
end)

test("changes: soloing a track that reads soloed only along with its group's solo says the group's solo ends (#361 N-N16)", function()
  local env, peer = setup(nil, { tracks = { { name = "Kick" }, { name = "Hat" }, { name = "Drums", kind = "group", members = 2 }, { name = "Bass" } } })
  mock.ops.solo(3, true) -- the producer soloed the group: its members read soloed
  local epoch = place(peer)
  local previewed = preview(peer, "track-solo", { ref = track_ref(epoch, 1), solo = true })
  ok((previewed.display.note or ""):find("exclusive", 1, true), "it says what ends: " .. tostring(previewed.display.note))
end)

test("changes: Kumi's undo of a track it made refuses once the producer automated it or set its levels, and discard removes it", function()
  local env, peer = setup()
  local function made()
    local epoch, layout = place(peer)
    local input = { type = "sequencer", name = "Lead", index = 3 }
    local previewed = preview(peer, "track-add", input)
    local done = apply(peer, "track-add", input, "Kumi: add track (kt11)", previewed.fence, epoch, layout)
    ok(done.result, json.encode(done.error or {}))
    return { target = previewed.target, prior = previewed.prior, applied = done.result.applied, created = done.result.created, material = done.result.material }, done
  end
  -- (only its volume automated in one pattern: no lines, so `is_empty` reads true)
  local record, done = made()
  local automation = mock.song:pattern(1):track(3):create_automation(mock.song:track(3):device(1):parameter(5))
  automation.points = { { time = 1, value = 0.2, scaling = 0 }, { time = 9, value = 0.8, scaling = 0 } }
  H.tick(2)
  eq(mock.song:pattern(1):track(3).is_empty, true)
  failure(restore(peer, "track-add", record, "Kumi: undo add (kt12)", done.result.epoch, done.result.layout), "changed")
  eq(layout_text(), "Kick,Hat,Lead,Bass,Pad,Master,Send 1", "their automated track stays")
  local forced = restore(peer, "track-add", record, "Kumi: undo add (kt13)", done.result.epoch, done.result.layout, { discard = true })
  ok(forced.result, json.encode(forced.error or {}))
  eq(layout_text(), "Kick,Hat,Bass,Pad,Master,Send 1")
  -- (its fader set, nothing else)
  record, done = made()
  mock.song:track(3):device(1):parameter(5).value = 0.5
  H.tick(2)
  failure(restore(peer, "track-add", record, "Kumi: undo add (kt14)", done.result.epoch, done.result.layout), "changed")
  eq(layout_text(), "Kick,Hat,Lead,Bass,Pad,Master,Send 1")
  H.no_dialogs()
end)

test("changes: Kumi's undo of a slot it made with a new pattern refuses once the producer wrote into that pattern, and discard removes it", function()
  local env, peer = setup()
  local function made()
    local epoch, layout = place(peer)
    local previewed = preview(peer, "slot-insert", { index = 2 })
    local done = apply(peer, "slot-insert", { index = 2 }, "Kumi: slot (kw21)", previewed.fence, epoch, layout)
    ok(done.result, json.encode(done.error or {}))
    return { target = previewed.target, prior = previewed.prior, applied = done.result.applied, created = done.result.created }, done
  end
  local record, done = made()
  local p = mock.data.sequence[2]
  mock.ops.set_note(p, 1, 1, 1, { note = 48, instrument = 0 }) -- the producer's line in the new pattern
  H.tick(2)
  failure(restore(peer, "slot-insert", record, "Kumi: undo (kw22)", done.result.epoch, done.result.layout), "changed")
  eq(mock.data.sequence[2], p, "the slot that plays their pattern stays")
  local forced = restore(peer, "slot-insert", record, "Kumi: undo (kw23)", done.result.epoch, done.result.layout, { discard = true })
  ok(forced.result, json.encode(forced.error or {}))
  eq(seq_text(), "1,2,1,3")
  -- (one left empty goes as before)
  record, done = made()
  ok(restore(peer, "slot-insert", record, "Kumi: undo (kw24)", done.result.epoch, done.result.layout).result)
  eq(seq_text(), "1,2,1,3")
  H.no_dialogs()
end)

test("changes: Kumi's undo of columns it showed refuses to hide them when the track is too big to look through, then or now; discard hides them (#361 N-S7, final review)", function()
  local env, peer = setup({ patch = function(env_)
    local path = env_.bundle .. "payload/changes_tracks.lua"
    local f = assert(io.open(path, "rb"))
    local text = f:read("*a")
    f:close()
    local lowered, n = text:gsub("M%.HIDDEN_READS = %d+", "M.HIDDEN_READS = 20")
    assert(n == 1, "HIDDEN_READS was not found to set")
    f = assert(io.open(path, "wb"))
    f:write(lowered)
    f:close()
  end })
  -- (more lines than the look reads: some in a pattern it never reaches)
  for p = 1, 3 do
    for k = 1, 16 do mock.ops.set_note(p, 3, k, 1, { note = 48 }) end
  end
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  local epoch, layout = place(peer)
  local input = { ref = track_ref(epoch, 3), noteColumns = 4 }
  local previewed = preview(peer, "track-columns", input)
  local done = apply(peer, "track-columns", input, "Kumi: columns (kc81)", previewed.fence, epoch, layout)
  ok(done.result, json.encode(done.error or {}))
  eq(done.result.applied.heldHidden, "unknown")
  mock.ops.set_note(3, 3, 16, 4, { note = 52 }) -- the producer's chord note, past where the look reads
  H.tick(2)
  local record = { target = previewed.target, prior = previewed.prior, applied = done.result.applied }
  local refused = restore(peer, "track-columns", record, "Kumi: undo columns (kc82)", done.result.epoch, done.result.layout)
  failure(refused, "changed")
  ok(refused.error.message:find("too big", 1, true), refused.error.message)
  eq(mock.song:track(3).visible_note_columns, 4, "still shown")
  local forced = restore(peer, "track-columns", record, "Kumi: undo columns (kc83)", done.result.epoch, done.result.layout, { discard = true })
  ok(forced.result, json.encode(forced.error or {}))
  eq(mock.song:track(3).visible_note_columns, 2)
end)

test("changes: the producer's redo of a Kumi change is told after they clicked a track, pressed play or muted one meanwhile", function()
  local env, peer = setup()
  local epoch = place(peer)
  local ref = track_ref(epoch, 1)
  local previewed, _, layout = preview(peer, "track-volume", { ref = ref, volumeDb = -6 })
  local desc = "Kumi: Kick volume \226\136\1466 dB (k7f9)"
  apply(peer, "track-volume", { ref = ref, volumeDb = -6 }, desc, previewed.fence, epoch, layout)
  H.tick(2)
  peer.events = {}
  mock.song:undo()
  H.tick(3)
  drain(peer)
  eq(H.events(peer, "undone_in_daw")[1].data.how, "undo")
  -- (none of these is a step of Renoise's undo: its redo is still there)
  mock.song.selected_track_index = 2
  mock.song.transport:start(renoise.Transport.PLAYMODE_RESTART_PATTERN)
  mock.ops.mute_track(2, 3)
  H.tick(3)
  drain(peer)
  mock.song:redo()
  H.tick(3)
  drain(peer)
  local told = H.events(peer, "undone_in_daw")
  eq(#told, 1, "the redo is told")
  eq(told[1].data.how, "redo")
  eq(told[1].data.descs[1], desc)
end)

test("changes: the producer's redo of a Kumi change is told after the player moved the tempo or an instrument's tab was shown", function()
  local env, peer = setup()
  local epoch = place(peer)
  local ref = track_ref(epoch, 1)
  local previewed, _, layout = preview(peer, "track-volume", { ref = ref, volumeDb = -6 })
  local desc = "Kumi: Kick volume \226\136\1466 dB (k7fa)"
  apply(peer, "track-volume", { ref = ref, volumeDb = -6 }, desc, previewed.fence, epoch, layout)
  H.tick(2)
  peer.events = {}
  mock.song:undo()
  H.tick(3)
  drain(peer)
  eq(H.events(peer, "undone_in_daw")[1].data.how, "undo")
  -- (playing through a ZT moves the tempo, and the producer shows an instrument's plug-in tab: no step of Renoise's undo)
  local transport = mock.data.transport_fields
  transport.playing = true
  transport.bpm = 140
  mock.fire(transport.bpm_observable)
  transport.playing = false
  mock.song:instrument(1).active_tab = 2
  H.tick(3)
  drain(peer)
  mock.song:redo()
  H.tick(3)
  drain(peer)
  local told = H.events(peer, "undone_in_daw")
  eq(#told, 1, "the redo is told")
  eq(told[1].data.how, "redo")
end)

test("changes: a pattern whose lines the tool couldn't count when a record kept its marks counts as touched since", function()
  local base = T.load_payload_module("changes_base")
  local env = { pattern_touched_since = function() return false end }
  local touched = base.touched_patterns(env, { ["3"] = false, ["4"] = "p4=1" }, function(p) return p end)
  eq(touched[3], true, "can't tell: as touched")
  eq(touched[4], nil, "counted, and untouched")
end)

-- Perf (N7): a notifier asks whether the move was Renoise's undo only once Kumi has a change in the journal; before that it
-- looks at the journal and asks Renoise for nothing.

test("changes: a notifier asks Renoise for the song only once Kumi has a change in the journal (perf N7)", function()
  local _, peer = setup()
  local real_song, asked = renoise.song, 0
  local function counting() asked = asked + 1 return real_song() end
  -- (the journal is empty: a track's name moves, its notifier fires, and the song isn't asked for)
  renoise.song = counting
  mock.data.tracks[1].name = "Kick 2"
  mock.fire(mock.song:track(1).name_observable)
  renoise.song = real_song
  eq(asked, 0, "renoise.song() calls from a notifier with an empty journal")
  -- (a change of Kumi's in the journal: the notifier now asks whether Renoise's undo or redo moved it)
  local epoch = place(peer)
  local ref = track_ref(epoch, 2)
  local previewed, _, layout = preview(peer, "track-volume", { ref = ref, volumeDb = -6 })
  local done = apply(peer, "track-volume", { ref = ref, volumeDb = -6 }, "Kumi: Hat volume \226\136\1466 dB (kn7a)", previewed.fence, epoch, layout)
  ok(done.result, json.encode(done.error or {}))
  asked = 0
  renoise.song = counting
  mock.data.tracks[1].name = "Kick 3"
  mock.fire(mock.song:track(1).name_observable)
  renoise.song = real_song
  ok(asked >= 1, "asked once a change is journalled")
  H.no_dialogs()
end)

-- Perf (N6): the env a change or the listening runs in is built once; the tick (twenty a second) takes the kept one.

test("changes: the tick takes the env it built once, and a change still runs in it (perf N6)", function()
  KUMI_ENVS = 0
  local _, peer = setup({ patch = function(env_)
    local path = env_.bundle .. "payload/bridge.lua"
    local f = assert(io.open(path, "rb"))
    local text = f:read("*a")
    f:close()
    local counted, n = text:gsub("\n  change_env_kept = env\n", "\n  change_env_kept = env\n  KUMI_ENVS = (KUMI_ENVS or 0) + 1\n")
    assert(n == 1, "change_env_kept")
    counted, n = counted:gsub("\n  listen_env_kept = env\n", "\n  listen_env_kept = env\n  KUMI_ENVS = (KUMI_ENVS or 0) + 1\n")
    assert(n == 1, "listen_env_kept")
    f = assert(io.open(path, "wb"))
    f:write(counted)
    f:close()
  end })
  ok(KUMI_ENVS >= 1 and KUMI_ENVS <= 2, "the envs built as the tool started: " .. KUMI_ENVS)
  local before = KUMI_ENVS
  H.tick(100) -- (five seconds of ticks)
  eq(KUMI_ENVS - before, 0, "envs built by a hundred ticks")
  local epoch = place(peer)
  local ref = track_ref(epoch, 2)
  local previewed, _, layout = preview(peer, "track-volume", { ref = ref, volumeDb = -6 })
  local done = apply(peer, "track-volume", { ref = ref, volumeDb = -6 }, "Kumi: Hat volume \226\136\1466 dB (kn6a)", previewed.fence, epoch, layout)
  ok(done.result, json.encode(done.error or {}))
  ok(math.abs(post_volume(2) - 10 ^ (-6 / 20)) < 1e-6, "written through the kept env")
  ok(KUMI_ENVS <= 2, "still the two envs: " .. KUMI_ENVS)
  KUMI_ENVS = nil
  H.no_dialogs()
end)
