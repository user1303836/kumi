-- Listening (M5): render jobs (render_start, listen_status, listen_cancel) against the mock Renoise, which renders as Renoise
-- 3.5.4 was measured to (see mock_listening.lua): a solo rewrites every other track's mute state, an un-solo sets every track
-- ACTIVE, the done callback comes even for a render cut short, and cancel_rendering is never to be called.
local T = ...
local mock = T.mock
local json = T.load_payload_module("json")
local test, eq, ok = T.test, T.eq, T.ok
local H = T.harness

local next_id = 9000
local A, OFF, MUTED = 1, 2, 3

local function drain(peer)
  repeat
    local taken = peer:take()
    if #taken > 0 then H.send(peer, { type = "credit", bytes = #taken }) end
  until #taken == 0
end

-- Sends a request and runs the clock until its response comes; events meanwhile are kept in `peer.events`.
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

local function result(response, what)
  ok(response and response.result, (what or "request") .. ": " .. json.encode(response and response.error or "no answer"))
  return response.result
end

local function failure(response, code, pattern)
  ok(response and response.error, "expected an error, got " .. json.encode(response and response.result or "nothing"))
  eq(response.error.code, code, "error code (" .. tostring(response.error.message) .. ")")
  if pattern then ok(response.error.message:find(pattern, 1, true), response.error.message) end
end

-- Kick, Hat, Bass and Pad (the mixer device on each), a master, a send; three patterns of 16 lines at 120 BPM, LPB 4 (a pattern
-- is 4 beats, 2 s).
local function song_options(extra)
  local spec = {
    name = "Listen", file_name = "/s/listen.xrns", bpm = 120, lpb = 4, mixer_device = true,
    tracks = { { name = "Kick" }, { name = "Hat" }, { name = "Bass" }, { name = "Pad" } },
    send_tracks = 1, patterns = 3, pattern_lines = 16, sequence_length = 3, sequence = { 1, 2, 3 }, instruments = 1,
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
  env.listening = env.dir .. "/kumi/bridges/renoise/listening"
  return env, peer
end

local function place(peer)
  local focus = result(call(peer, "focus"), "focus")
  return focus.epoch, focus.layout
end

local function track_ref(epoch, i) return epoch .. ":track:" .. i end

local function track(i) return mock.data.tracks[i] end

local function states()
  local out = {}
  for i, t in ipairs(mock.data.tracks) do
    if t.kind ~= "master" then out[#out + 1] = i .. "=" .. t.mute_state .. (t.solo and "*" or "") end
  end
  return table.concat(out, " ")
end

local function start(peer, job, sources, range, more)
  local epoch, layout = place(peer)
  local params = { jobId = job, sources = sources, range = range or { fromBeat = 0, beats = 4 }, preRollBeats = 0, epoch = epoch, layout = layout }
  for k, v in pairs(more or {}) do params[k] = v end
  return call(peer, "render_start", params), epoch, layout
end

local function status(peer, job) return result(call(peer, "listen_status", { jobId = job }), "listen_status") end

local function exists(path)
  local f = io.open(path, "rb")
  if f then f:close() return true end
  return false
end

local function bridge() return KUMI_BRIDGE.bridge end

-- The render chain

test("listening: a render job renders the mix as the producer has it, then each track soloed, and puts every mute and solo back", function()
  local env, peer = setup()
  track(2).mute_state = MUTED
  track(3).mute_state = OFF
  local before = states()
  local epoch = place(peer)
  local answer = result((start(peer, "j1", {
    { id = "bass", what = "track", ref = track_ref(epoch, 3) }, { id = "mix", what = "master" }, { id = "kick", what = "track", ref = track_ref(epoch, 1) },
  })), "render_start")
  eq(answer.jobId, "j1")
  eq(answer.state, "running")
  eq(answer.kind, "render")
  ok(math.abs(answer.seconds - 2) < 1e-9, "16 lines at 120 BPM, LPB 4: " .. answer.seconds)
  eq(#answer.files, 3)
  eq(answer.files[1].file, env.listening .. "/j1-bass.wav")
  eq(answer.files[2].file, env.listening .. "/j1-mix.wav")
  ok(exists(env.listening), "the listening folder is made")
  -- 1: the mix first, as the producer has it (no solo, their mutes)
  eq(#mock.renders, 1)
  local r = mock.renders[1]
  eq(r.path, env.listening .. "/j1-mix.wav")
  eq(r.options.start_pos.sequence, 1)
  eq(r.options.start_pos.line, 1)
  eq(r.options.end_pos.line, 16, "end_pos is inclusive: the last line before beat 4")
  eq(r.options.sample_rate, 48000)
  eq(r.options.bit_depth, 32)
  eq(r.options.priority, "high")
  eq(r.options.interpolation, "precise")
  eq(r.tracks[2].mute, MUTED)
  eq(r.tracks[3].mute, OFF)
  eq(r.tracks[1].solo, false)
  mock.render_progress(0.5)
  local half = status(peer, "j1")
  eq(half.state, "running")
  ok(math.abs(half.progress - 0.5 / 3) < 1e-9, "progress: " .. half.progress)
  -- (review K18) While Kumi renders, the focus says Renoise is busy, so Kumi keeps the body it showed (bodies are refused)
  eq(result(call(peer, "focus", {}), "focus").busy, "rendering")
  mock.finish_render()
  -- 2: Bass soloed (it was OFF: a solo plays it anyway), every other track OFF
  eq(#mock.renders, 2)
  r = mock.renders[2]
  eq(r.path, env.listening .. "/j1-bass.wav")
  eq(r.tracks[3].solo, true)
  eq(r.tracks[3].mute, A)
  eq(r.tracks[1].mute, OFF)
  eq(r.tracks[2].mute, OFF)
  mock.finish_render()
  -- 3: Kick soloed
  r = mock.renders[3]
  eq(r.tracks[1].solo, true)
  eq(r.tracks[3].solo, false)
  eq(r.tracks[3].mute, OFF)
  mock.finish_render()
  local done = status(peer, "j1")
  eq(done.state, "done")
  eq(done.progress, 1)
  eq(result(call(peer, "focus", {}), "focus").busy, nil, "free again once the job is done")
  eq(#done.files, 3)
  local by = {}
  for _, f in ipairs(done.files) do by[f.id] = f end
  eq(by.mix.stage, "master")
  eq(by.bass.stage, "post-fader", "a Renoise track is heard after its fader")
  eq(by.bass.throughMaster, true)
  eq(by.kick.sampleRate, 48000)
  eq(by.kick.channels, 2)
  ok(math.abs(by.kick.seconds - 2) < 1e-6, "the file's own length: " .. by.kick.seconds)
  eq(by.kick.start, 0)
  ok(exists(by.kick.file) and exists(by.mix.file), "the files are there")
  eq(states(), before, "every mute and solo as the producer had it")
  eq(#mock.data.undo.stack, 0, "no undo step")
  eq(done.notes, nil)
  H.no_dialogs()
end)

test("listening: the producer's soloed track is soloed again after the job, and Kumi's solo and mute writes aren't edits", function()
  local env, peer = setup()
  mock.solo_write(2, true) -- (the producer's solo: every other track OFF)
  track(4).mute_state = A -- (and they turned Pad back on while soloing)
  local before = states()
  local changed = bridge().changed
  local edits = bridge().edits or 0
  local epoch = place(peer)
  result((start(peer, "j2", { { id = "kick", what = "track", ref = track_ref(epoch, 1) } })), "render_start")
  eq(mock.renders[1].tracks[1].solo, true)
  eq(mock.renders[1].tracks[2].solo, false, "solo is exclusive")
  mock.finish_render()
  eq(status(peer, "j2").state, "done")
  eq(states(), before, "Hat soloed again and Pad on, as the producer had them")
  -- (the transport's playing on and off, twice, is all the song's notifiers counted)
  ok(bridge().changed - changed <= 4, "Kumi's writes are quiet: " .. (bridge().changed - changed))
  ok((bridge().edits or 0) - edits <= 4, "and no edits: " .. ((bridge().edits or 0) - edits))
  H.no_dialogs()
end)

test("listening: a track the producer soloed or muted during the render is left as they set it, and said", function()
  local env, peer = setup()
  track(4).mute_state = MUTED
  local epoch = place(peer)
  result((start(peer, "j3", { { id = "kick", what = "track", ref = track_ref(epoch, 1) }, { id = "hat", what = "track", ref = track_ref(epoch, 2) } })))
  mock.finish_render()
  -- (a MIDI-mapped mute button moved while Renoise rendered the Hat)
  mock.ops.mute_track(3, MUTED)
  mock.finish_render()
  local done = status(peer, "j3")
  eq(done.state, "done")
  eq(track(3).mute_state, MUTED, "the producer's mute stays")
  eq(track(4).mute_state, MUTED, "Pad as it was")
  eq(track(1).mute_state, A)
  eq(track(2).solo, false)
  ok(done.notes and done.notes[1]:find("changed during the render and left as it was set", 1, true), json.encode(done.notes))
  H.no_dialogs()
end)

test("listening: a mute changed while the mix renders, before Kumi's first solo, is left as it was set", function()
  local env, peer = setup()
  local epoch = place(peer)
  result((start(peer, "j3b", { { id = "mix", what = "master" }, { id = "kick", what = "track", ref = track_ref(epoch, 1) } })))
  mock.ops.mute_track(4, MUTED)
  mock.finish_render()
  eq(mock.renders[2].tracks[4].mute, OFF, "Kumi's solo of Kick turned it OFF")
  mock.finish_render()
  local done = status(peer, "j3b")
  eq(track(4).mute_state, MUTED, "the producer's mute, not the snapshot's")
  ok(done.notes and done.notes[1]:find("left as it was set", 1, true), json.encode(done.notes))
  H.no_dialogs()
end)

test("listening: two producer solos can't both come back through Renoise's exclusive solo: the mutes do, and it is said", function()
  local env, peer = setup()
  -- (a state the GUI can reach: Hat and Pad soloed, the rest OFF)
  for i, t in ipairs(mock.data.tracks) do
    if t.kind ~= "master" then t.mute_state, t.solo = OFF, false end
    if i == 2 or i == 4 then t.mute_state, t.solo = A, true end
  end
  local before = states()
  local epoch = place(peer)
  result((start(peer, "j4", { { id = "kick", what = "track", ref = track_ref(epoch, 1) } })))
  mock.finish_render()
  local done = status(peer, "j4")
  eq(done.state, "done")
  eq(track(2).solo, true, "the first comes back")
  eq(track(4).mute_state, A, "the second plays as it did")
  eq(track(1).mute_state, OFF)
  ok(done.notes and done.notes[1]:find("one track at a time", 1, true), json.encode(done.notes))
  ok(states() ~= before)
  H.no_dialogs()
end)

-- A group's solo solos its members too, and a member soloed reads its group soloed (measured): neither is Kumi's solo of one track.
local drums = { tracks = { { name = "Kick" }, { name = "Hat" }, { name = "Drums", kind = "group", members = 2 }, { name = "Bass" } } }

test("listening: a track rendered while the producer has its group soloed is heard on its own, and the group's solo comes back", function()
  local env, peer = setup(nil, drums)
  mock.ops.solo(3, true)
  eq(track(1).solo and track(2).solo and track(3).solo, true, "the group's solo solos its members")
  local before = states()
  local epoch = place(peer)
  result((start(peer, "jg1", { { id = "kick", what = "track", ref = track_ref(epoch, 1) } })), "render_start")
  eq(mock.renders[1].tracks[1].solo, true)
  eq(mock.renders[1].tracks[2].mute, OFF, "the Hat isn't in the Kick's file")
  mock.finish_render()
  eq(status(peer, "jg1").state, "done")
  eq(states(), before, "the Drums soloed again, with its members")
  H.no_dialogs()
end)

test("listening: a group rendered while the producer has one member soloed is heard with all its members, and the solo comes back", function()
  local env, peer = setup(nil, drums)
  mock.ops.solo(1, true)
  eq(track(3).solo, true, "a member's solo reads its group soloed")
  local before = states()
  local epoch = place(peer)
  result((start(peer, "jg2", { { id = "drums", what = "track", ref = track_ref(epoch, 3) } })), "render_start")
  eq(mock.renders[1].tracks[2].mute, A, "the Hat is in the Drums' file")
  eq(mock.renders[1].tracks[4].mute, OFF)
  mock.finish_render()
  eq(status(peer, "jg2").state, "done")
  eq(states(), before, "the Kick soloed again on its own")
  H.no_dialogs()
end)

test("listening: a track rendered while the producer has two tracks soloed in the GUI is heard on its own", function()
  local env, peer = setup()
  mock.ops.solo(1, true)
  mock.ops.add_solo(3)
  local epoch = place(peer)
  result((start(peer, "jg3", { { id = "kick", what = "track", ref = track_ref(epoch, 1) } })), "render_start")
  eq(mock.renders[1].tracks[3].mute, OFF, "the Bass isn't in the Kick's file")
  mock.finish_render()
  local done = status(peer, "jg3")
  eq(done.state, "done")
  ok(done.notes and table.concat(done.notes, " "):find("one track at a time", 1, true), json.encode(done.notes))
  eq(track(3).mute_state, A, "the Bass plays as it did")
  H.no_dialogs()
end)

test("listening: the range is planned in lines: pre-roll clamped at the start, end_pos inclusive, where the part begins said", function()
  local env, peer = setup()
  local answer = result((start(peer, "j5", { { id = "mix", what = "master" } }, { fromBeat = 1, beats = 2 }, { preRollBeats = 4 })))
  local r = mock.renders[1]
  eq(r.options.start_pos.line, 1, "the pre-roll stops at the song's start")
  eq(r.options.end_pos.line, 12, "beat 3 starts line 13: line 12 is the last")
  ok(math.abs(answer.seconds - 1.5) < 1e-9, "12 lines: " .. answer.seconds)
  mock.finish_render()
  local done = status(peer, "j5")
  ok(math.abs(done.files[1].start - 0.5) < 1e-9, "the part begins a beat in: " .. done.files[1].start)
  -- a part across patterns
  result((start(peer, "j6", { { id = "mix", what = "master" } }, { fromBeat = 3.5, beats = 1 }, { preRollBeats = 1 })))
  r = mock.renders[2]
  eq(r.options.start_pos.sequence, 1)
  eq(r.options.start_pos.line, 11, "beat 2.5 is line 11")
  eq(r.options.end_pos.sequence, 2)
  eq(r.options.end_pos.line, 2, "beat 4.5 starts slot 2's line 3")
  mock.finish_render()
  eq(status(peer, "j6").state, "done")
  H.no_dialogs()
end)

test("listening: ZL and ZT commands in any track move the lines' beats and seconds", function()
  local env, peer = setup()
  -- (ZL08 on line 1 of pattern 1, in Hat's effect column: the pattern's 16 lines are 2 beats; ZT3C (60 BPM) at slot 2's line 1)
  local function command(text)
    local digits = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ"
    return 256 + (digits:find(text:sub(1, 1), 1, true) - 1) * 36 + (digits:find(text:sub(2, 2), 1, true) - 1)
  end
  mock.ops.set_effect(1, 2, 1, 1, command("ZL"), 8)
  mock.ops.set_effect(2, 4, 1, 1, command("ZT"), 0x3C)
  local answer = result((start(peer, "j7", { { id = "mix", what = "master" } }, { fromBeat = 2, beats = 1 })))
  local r = mock.renders[1]
  eq(r.options.start_pos.sequence, 2, "beat 2 is slot 2 (pattern 1 is 2 beats at LPB 8)")
  eq(r.options.start_pos.line, 1)
  eq(r.options.end_pos.line, 8, "a beat at LPB 8 is 8 lines")
  ok(math.abs(answer.seconds - 1) < 1e-9, "a beat at 60 BPM is a second: " .. answer.seconds)
  mock.finish_render()
  eq(status(peer, "j7").state, "done", "the file has the length the commands give")
  H.no_dialogs()
end)

test("listening: a ZL in a pattern that another's track shows as an alias moves that pattern's lines too, read before and after", function()
  -- (slot 1 plays pattern 2, whose Hat track shows pattern 1's Hat lines)
  local env, peer = setup(nil, { sequence = { 2, 1, 3 } })
  local function command(text)
    local digits = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ"
    return 256 + (digits:find(text:sub(1, 1), 1, true) - 1) * 36 + (digits:find(text:sub(2, 2), 1, true) - 1)
  end
  mock.ops.alias(2, 2, 1)
  H.tick(2)
  -- (a first render reads the whole song into the cache)
  result((start(peer, "j70", { { id = "mix", what = "master" } }, { fromBeat = 0, beats = 12 })))
  mock.finish_render()
  eq(status(peer, "j70").state, "done")
  -- (then a ZL08 on line 9 of pattern 1's Hat: Renoise tells pattern 1's line notifiers only, though slot 1 shows it too)
  mock.ops.set_effect(1, 2, 9, 1, command("ZL"), 8)
  H.tick(2)
  result((start(peer, "j71", { { id = "mix", what = "master" } }, { fromBeat = 3, beats = 1 })))
  local r = mock.renders[2]
  eq(r.options.start_pos.sequence, 2, "slot 1 is 8 lines at LPB 4 and 8 at LPB 8: 3 beats")
  eq(r.options.start_pos.line, 1)
  mock.finish_render()
  H.no_dialogs()
end)

test("listening: a read of the song's tempo commands works out each pattern's cache mark once, however many slots play it", function()
  local sequence = {}
  for s = 1, 30 do sequence[s] = (s - 1) % 3 + 1 end
  local env, peer = setup(nil, { sequence = sequence, sequence_length = 30 })
  result((start(peer, "j80", { { id = "mix", what = "master" } }, { fromBeat = 0, beats = 4 })))
  mock.finish_render()
  -- (all read now: a second read looks only at the marks)
  mock.alias_reads = 0
  result((start(peer, "j81", { { id = "mix", what = "master" } }, { fromBeat = 4, beats = 4 })))
  mock.finish_render()
  H.tick(10)
  -- (the render's read of the timeline, and the warm-up's look on a later tick: while every pattern's own mark holds, its
  -- alias tracks are the ones found before, and no track is asked; perf N3)
  eq(mock.alias_reads, 0, "alias reads in a read, and a warm pass, whose marks hold")
  H.no_dialogs()
end)

test("listening: a track made an alias after a read is seen by the next read: its pattern's mark moves and its aliases are found again (perf N3)", function()
  -- (slot 1 plays pattern 2, whose Hat track comes to show pattern 1's Hat lines only after the first read)
  local env, peer = setup(nil, { sequence = { 2, 1, 3 } })
  local function command(text)
    local digits = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ"
    return 256 + (digits:find(text:sub(1, 1), 1, true) - 1) * 36 + (digits:find(text:sub(2, 2), 1, true) - 1)
  end
  result((start(peer, "j90", { { id = "mix", what = "master" } }, { fromBeat = 0, beats = 12 })))
  mock.finish_render()
  H.tick(10)
  mock.alias_reads = 0
  result((start(peer, "j91", { { id = "mix", what = "master" } }, { fromBeat = 0, beats = 12 })))
  mock.finish_render()
  H.tick(10)
  eq(mock.alias_reads, 0, "nothing asked while the marks hold")
  mock.ops.alias(2, 2, 1)
  H.tick(2)
  -- (a ZL08 on line 9 of pattern 1's Hat: pattern 1's line notifiers tell it, and slot 1 shows it through the alias)
  mock.ops.set_effect(1, 2, 9, 1, command("ZL"), 8)
  H.tick(2)
  result((start(peer, "j92", { { id = "mix", what = "master" } }, { fromBeat = 3, beats = 1 })))
  local r = mock.renders[#mock.renders]
  eq(r.options.start_pos.sequence, 2, "slot 1 is 8 lines at LPB 4 and 8 at LPB 8: 3 beats")
  eq(r.options.start_pos.line, 1)
  mock.finish_render()
  ok(mock.alias_reads >= 1, "the aliases of the pattern whose mark moved were found again: " .. mock.alias_reads)
  H.no_dialogs()
end)

test("listening: a render times its lines as Renoise renders them: before the song's first ZT it runs at that ZT's tempo, and says so", function()
  local env, peer = setup()
  local function command(text)
    local digits = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ"
    return 256 + (digits:find(text:sub(1, 1), 1, true) - 1) * 36 + (digits:find(text:sub(2, 2), 1, true) - 1)
  end
  -- (ZT3C (60 BPM) on slot 2's line 9 only: playback plays slot 1 at the transport's 120; Renoise renders it at 60)
  mock.ops.set_effect(2, 4, 9, 1, command("ZT"), 0x3C)
  local answer = result((start(peer, "j7b", { { id = "mix", what = "master" } }, { fromBeat = 1, beats = 2 }, { preRollBeats = 1 })))
  ok(math.abs(answer.seconds - 3) < 1e-9, "beats 0-3, 12 lines at 60 BPM: " .. answer.seconds)
  mock.finish_render()
  local done = status(peer, "j7b")
  eq(done.state, "done", "the file has the length Renoise renders")
  ok(math.abs(done.files[1].start - 1) < 1e-9, "the part begins a beat (at 60 BPM) in: " .. done.files[1].start)
  ok(done.notes and done.notes[1]:find("at that command's 60 BPM, LPB 4, not at the 120 BPM, LPB 4 playback has there", 1, true), json.encode(done.notes))
  -- what the render left in the transport is said, and given
  eq(done.tempo.bpm, 60)
  eq(done.tempo.before.bpm, 120)
  ok(done.notes[2]:find("it is at 60 BPM, LPB 4 now (it was at 120 BPM, LPB 4)", 1, true), json.encode(done.notes))
  eq(#mock.data.undo.stack, 0, "the render's tempo is no undo step")
  -- a part after the command renders as played
  mock.data.transport_fields.bpm = 120
  result((start(peer, "j7c", { { id = "mix", what = "master" } }, { fromBeat = 6, beats = 1 })))
  local r = mock.renders[#mock.renders]
  eq(r.options.start_pos.sequence, 2)
  eq(r.options.start_pos.line, 9)
  mock.finish_render()
  local after = status(peer, "j7c")
  ok(math.abs(after.files[1].seconds - 1) < 1e-9, "a beat at 60 BPM: " .. after.files[1].seconds)
  eq(#after.notes, 1, "only what the transport was left at: the render starts on the command, as playback has it")
  ok(after.notes[1]:find("it is at 60 BPM", 1, true), after.notes[1])
  H.no_dialogs()
end)

test("listening: the render-timing cases measured in Renoise plan the lengths Renoise renders", function()
  local function command(text)
    local digits = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ"
    return 256 + (digits:find(text:sub(1, 1), 1, true) - 1) * 36 + (digits:find(text:sub(2, 2), 1, true) - 1)
  end
  -- {commands {slot, line, text, amount}, fromBeat, beats, preRoll, seconds, start} (16-line patterns: 4 beats at LPB 4)
  local cases = {
    { {}, 1, 2, 1, 1.5, 0.5 },
    { { { 2, 1, "ZT", 0x3C } }, 1, 2, 1, 3, 1 },                       -- before the song's first ZT: at its tempo
    { { { 2, 1, "ZL", 8 } }, 0, 4, 0, 1, 0 },                          -- before the first ZL: at its LPB (16 lines at LPB 8)
    { { { 1, 1, "ZT", 0x78 }, { 3, 1, "ZT", 0x3C } }, 4, 4, 0, 2, 0 }, -- the ZT before the part wins (120)
    { { { 1, 1, "ZT", 0x78 }, { 3, 1, "ZT", 0x3C } }, 8, 2, 0, 2, 0 }, -- one met on the part's first line (60)
    { { { 1, 1, "ZL", 8 }, { 2, 1, "ZT", 0x3C } }, 1.5, 0.5, 0.5, 1, 0.5 }, -- LPB 8 chased, BPM the first ZT's: 60/8
  }
  for k, c in ipairs(cases) do
    local env, peer = setup()
    for _, cmd in ipairs(c[1]) do mock.ops.set_effect(cmd[1], 4, cmd[2], 1, command(cmd[3]), cmd[4]) end
    local job = "jc" .. k
    local answer = result((start(peer, job, { { id = "mix", what = "master" } }, { fromBeat = c[2], beats = c[3] }, { preRollBeats = c[4] })), "case " .. k)
    ok(math.abs(answer.seconds - c[5]) < 1e-9, "case " .. k .. " planned " .. answer.seconds .. ", Renoise renders " .. c[5])
    mock.finish_render()
    local done = status(peer, job)
    eq(done.state, "done", "case " .. k .. ": " .. tostring(done.error))
    ok(math.abs(done.files[1].seconds - c[5]) < 1e-6, "case " .. k .. " file " .. done.files[1].seconds)
    ok(math.abs(done.files[1].start - c[6]) < 1e-9, "case " .. k .. " start " .. done.files[1].start .. ", expected " .. c[6])
  end
  H.no_dialogs()
end)

-- Failures put everything back

test("listening: a render that ends with no callback fails, puts the mutes back and deletes its files", function()
  local env, peer = setup()
  track(2).mute_state = MUTED
  local before = states()
  local epoch = place(peer)
  result((start(peer, "j8", { { id = "mix", what = "master" }, { id = "kick", what = "track", ref = track_ref(epoch, 1) } })))
  mock.finish_render()
  mock.finish_render({ no_callback = true })
  ok(exists(env.listening .. "/j8-mix.wav"))
  H.tick(30)
  local failed = status(peer, "j8")
  eq(failed.state, "failed")
  eq(failed.code, "failed")
  eq(failed.error, "Renoise stopped the render")
  eq(states(), before)
  ok(not exists(env.listening .. "/j8-mix.wav") and not exists(env.listening .. "/j8-kick.wav"), "partial files deleted")
  H.no_dialogs()
end)

test("listening: a render cut short calls back but fails: its file is shorter than the lines asked for", function()
  local env, peer = setup()
  local epoch = place(peer)
  result((start(peer, "j9", { { id = "kick", what = "track", ref = track_ref(epoch, 1) } })))
  mock.finish_render({ fraction = 0.5 })
  local failed = status(peer, "j9")
  eq(failed.state, "failed")
  ok(failed.error:find("Renoise stopped the render after 1.0 of 2.0 s", 1, true), failed.error)
  eq(track(1).solo, false)
  eq(track(2).mute_state, A)
  ok(not exists(env.listening .. "/j9-kick.wav"))
  H.no_dialogs()
end)

test("listening: Renoise refusing a render fails the job at once with its words, nothing left changed", function()
  local env, peer = setup()
  mock.render_refuses = "could not open the file"
  local epoch = place(peer)
  local response = start(peer, "j10", { { id = "kick", what = "track", ref = track_ref(epoch, 1) } })
  failure(response, "failed", "Renoise wouldn't render: could not open the file")
  eq(track(1).solo, false)
  eq(track(2).mute_state, A)
  eq(status(peer, "j10").state, "failed")
  H.no_dialogs()
end)

test("listening: listen_cancel lets the render in flight end (never cancel_rendering), then puts everything back and deletes the files", function()
  local env, peer = setup()
  track(3).mute_state = OFF
  local before = states()
  local epoch = place(peer)
  result((start(peer, "j11", { { id = "mix", what = "master" }, { id = "kick", what = "track", ref = track_ref(epoch, 1) } })))
  mock.finish_render()
  local cancel = result(call(peer, "listen_cancel", { jobId = "j11" }), "listen_cancel")
  eq(cancel.state, "running")
  eq(cancel.stopping, true)
  eq(status(peer, "j11").stopping, true)
  mock.finish_render()
  eq(#mock.renders, 2, "nothing more was rendered")
  local cancelled = status(peer, "j11")
  eq(cancelled.state, "cancelled")
  ok(cancelled.notes and cancelled.notes[1]:find("Renoise finished the render before it could stop", 1, true), json.encode(cancelled.notes))
  eq(states(), before)
  ok(not exists(env.listening .. "/j11-mix.wav") and not exists(env.listening .. "/j11-kick.wav"))
  -- (the mock records cancel_rendering as a crash, which fails any test that calls it)
  H.no_dialogs()
end)

test("listening: listen_cancel of a job that ended answers its end; of an unknown job is bad-params", function()
  local env, peer = setup()
  result((start(peer, "j12", { { id = "mix", what = "master" } })))
  mock.finish_render()
  eq(result(call(peer, "listen_cancel", { jobId = "j12" })).state, "done")
  failure(call(peer, "listen_cancel", { jobId = "j99" }), "bad-params")
  failure(call(peer, "listen_status", { jobId = "j99" }), "bad-params")
  H.no_dialogs()
end)

-- Busy, stale, the demo

test("listening: while Kumi renders, only ping, status, focus and the job's own requests are answered; the rest are busy", function()
  local env, peer = setup()
  local epoch, layout = place(peer)
  result((start(peer, "j13", { { id = "mix", what = "master" } })))
  local preview = call(peer, "change_preview", { kind = "track-volume", input = { ref = track_ref(epoch, 1), volumeDb = -6 }, epoch = epoch, layout = layout })
  failure(preview, "busy", "Renoise is rendering for Kumi")
  failure(call(peer, "read", { kind = "track" }), "busy")
  failure(call(peer, "render_start", { jobId = "j14", sources = { { id = "mix", what = "master" } }, range = { fromBeat = 0, beats = 1 }, epoch = epoch }), "busy")
  eq(result(call(peer, "ping")).daw, nil)
  local s = result(call(peer, "status"))
  eq(s.busy, "rendering")
  eq(s.busyReason, "Renoise is rendering for Kumi")
  eq(result(call(peer, "focus")).transport.playing, false, "Renoise reads as playing while it renders; Kumi's render isn't playback")
  mock.finish_render()
  eq(status(peer, "j13").state, "done")
  ok(result(call(peer, "change_preview", { kind = "track-volume", input = { ref = track_ref(epoch, 1), volumeDb = -6 }, epoch = epoch, layout = layout })))
  H.no_dialogs()
end)

test("listening: the transport event of Renoise's own render is held back; a render while playing stops it and says so", function()
  local env, peer = setup({}, { playing = true })
  peer.events = {}
  result((start(peer, "j15", { { id = "mix", what = "master" } })))
  H.tick(3)
  for _ = 1, 3 do call(peer, "ping") end
  mock.finish_render()
  H.tick(3)
  call(peer, "ping")
  local transport = {}
  for _, event in ipairs(peer.events) do
    if event.name == "transport" then transport[#transport + 1] = event.data.playing end
  end
  eq(#transport, 1, "one event, after the job: " .. json.encode(transport))
  eq(transport[1], false, "playback stopped")
  local done = status(peer, "j15")
  ok(done.notes and done.notes[1] == "Renoise stopped playback to render.", json.encode(done.notes))
  H.no_dialogs()
end)

test("listening: render_start checks everything before writing: refs of another epoch, bad sources, rates, ranges, the hour", function()
  local env, peer = setup()
  local epoch = place(peer)
  local function bad(sources, range, more, code, pattern)
    local response = start(peer, "j16", sources, range, more)
    failure(response, code or "bad-params", pattern)
    eq(#mock.renders, 0)
    eq(track(1).solo, false)
  end
  bad({ { id = "kick", what = "track", ref = "1a:track:1" } }, nil, nil, "stale")
  bad({ { id = "mix", what = "master", beforeChain = true } }, nil, nil, "bad-params", "Renoise renders the mix after its Master track's devices")
  bad({ { id = "m", what = "track", ref = track_ref(epoch, 5) } }, nil, nil, "bad-params", "the Master track is the mix")
  bad({ { id = "a", what = "master" }, { id = "a", what = "master" } }, nil, nil, "bad-params", "two sources are named a")
  bad({ { id = "bad id!", what = "master" } })
  local many = {}
  for i = 1, 19 do many[i] = { id = "s" .. i, what = "master" } end
  bad(many)
  bad({ { id = "mix", what = "master" } }, nil, { sampleRate = 22050 }, "bad-params", "sampleRate")
  bad({ { id = "mix", what = "master" } }, { fromBeat = 0, beats = 0 })
  bad({ { id = "mix", what = "master" } }, { fromBeat = -1, beats = 1 })
  bad({ { id = "mix", what = "master" } }, { fromBeat = 12, beats = 1 }, nil, "bad-params", "the song ends at beat 12")
  failure(call(peer, "render_start", { jobId = "x1", sources = { { id = "mix", what = "master" } }, range = { fromBeat = 0, beats = 1 }, epoch = epoch }), "bad-params", "jobId")
  -- the hour: a song of 300 patterns of 512 lines at 32 BPM is far over it
  H.no_dialogs()
end)

test("listening: a render or a sample's export past the room the host says Kumi's folder has is refused before anything starts", function()
  local env, peer = setup({}, { instrument_specs = { { samples = 1 } } })
  local epoch = place(peer)
  -- (4 beats at 120 BPM: 2 s; two sources at 48 kHz, 8 bytes a frame, half again for room: 2.3 MB)
  local response = start(peer, "j17b", { { id = "mix", what = "master" }, { id = "kick", what = "track", ref = track_ref(epoch, 1) } }, nil,
    { freeBytes = 2000000 })
  failure(response, "failed", "Kumi's folder has 2 MB free; this render needs about 3 MB")
  eq(#mock.renders, 0)
  eq(track(1).solo, false)
  ok(result((start(peer, "j17c", { { id = "mix", what = "master" } }, nil, { freeBytes = 2000000 }))), "one source fits")
  mock.finish_render()
  failure(call(peer, "audio_file", { ref = epoch .. ":pattern:1", jobId = "j17d", epoch = epoch, freeBytes = 100 }), "failed", "this render needs about")
  mock.data.instruments[1].samples[1].buffer = { rate = 44100, frames = 441000, channels = 2 }
  failure(call(peer, "audio_file", { ref = epoch .. ":sample:1.1", jobId = "j17e", epoch = epoch, freeBytes = 1000 }), "failed",
    "Kumi's folder has 0 MB free; this sample needs about 6 MB")
  eq(#mock.saved_samples, 0)
  H.no_dialogs()
end)

test("listening: more than an hour of music is refused", function()
  -- (4 patterns of 512 lines at 32 BPM and LPB 1: 2,048 beats, 64 minutes)
  local env, peer = setup({}, { bpm = 32, lpb = 1, patterns = 4, pattern_lines = 512, sequence_length = 4, sequence = { 1, 2, 3, 4 } })
  local response = start(peer, "j17", { { id = "mix", what = "master" } }, { fromBeat = 0, beats = 2048 })
  failure(response, "bad-params", "Kumi renders at most 60 at a time")
  eq(#mock.renders, 0)
  H.no_dialogs()
end)

test("listening: the demo can't render and says why; status says render is off", function()
  local env, peer = setup({ version = "3.5.4 Demo" })
  local s = result(call(peer, "status"))
  eq(s.listening.render, false)
  eq(s.listening.audition, false)
  eq(s.listening.version, 1)
  local response = start(peer, "j18", { { id = "mix", what = "master" } })
  failure(response, "failed", "The Renoise demo can't render to a file, so Kumi can't hear the song: a licensed Renoise can")
  eq(#mock.renders, 0)
  H.no_dialogs()
end)

test("listening: status gives the listening flags", function()
  local env, peer = setup()
  local s = result(call(peer, "status"))
  eq(s.listening.render, true)
  eq(s.listening.capture, false)
  eq(s.listening.audition, true)
  eq(s.listening.grid, false)
  eq(s.listening.scratch, true)
  eq(s.listening.version, 1)
  eq(s.listening.render_preflight, true)
  H.no_dialogs()
end)

test("listening: a song closed during the render fails the job stale, deletes its files and touches nothing of the new song", function()
  local env, peer = setup()
  local epoch = place(peer)
  result((start(peer, "j19", { { id = "mix", what = "master" }, { id = "kick", what = "track", ref = track_ref(epoch, 1) } })))
  mock.finish_render()
  local old = mock.pending_render
  mock.load_song(song_options({ name = "Other" }))
  mock.pending_render = old
  mock.impl(mock.song).rendering = true
  H.tick()
  mock.finish_render()
  local failed = status(peer, "j19")
  eq(failed.state, "failed")
  eq(failed.code, "stale")
  ok(not exists(env.listening .. "/j19-mix.wav") and not exists(env.listening .. "/j19-kick.wav"))
  eq(states(), "1=1 2=1 3=1 4=1 6=1", "the new song untouched")
  H.no_dialogs()
end)

test("listening: the tool stopped during a job puts the mutes back now and answers the job's end after a reload", function()
  local env, peer = setup()
  track(2).mute_state = MUTED
  local before = states()
  local epoch = place(peer)
  result((start(peer, "j20", { { id = "kick", what = "track", ref = track_ref(epoch, 1) } })))
  eq(track(1).solo, true)
  -- (Renoise reloads its tools: the payload stops, and starts again)
  KUMI_BRIDGE.payload.stop({ bundle_path = env.bundle, state = KUMI_BRIDGE, tick_ms = 50, close_server = function() end }, true)
  eq(states(), before, "put back at once")
  mock.finish_render()
  ok(not exists(env.listening .. "/j20-kick.wav"), "the late file is deleted")
  H.no_dialogs()
end)

-- audio_file

test("listening: audio_file saves a sample's audio into Kumi's folder, and a slice as its sample with where the slice is", function()
  local env, peer = setup({}, { instrument_specs = { { name = "Drums", samples = 3 } } })
  local samples = mock.data.instruments[1].samples
  samples[1].name = "Break"
  samples[1].buffer = { rate = 44100, frames = 44100, channels = 2, content = "audio:Break" }
  samples[1].markers = { 1, 22051 }
  samples[2].slice_alias, samples[3].slice_alias = true, true
  local epoch = place(peer)
  local answer = result(call(peer, "audio_file", { ref = epoch .. ":sample:1.1", jobId = "j21", epoch = epoch }), "audio_file")
  eq(answer.file, env.listening .. "/j21-sample.wav")
  eq(answer.kind, "exported")
  eq(answer.name, "Break")
  eq(answer.channels, 2)
  eq(answer.sampleRate, 44100)
  ok(math.abs(answer.seconds - 1) < 1e-9)
  eq(answer.offset, nil)
  ok(exists(answer.file))
  local slice = result(call(peer, "audio_file", { ref = epoch .. ":sample:1.3", jobId = "j22", epoch = epoch }), "audio_file of a slice")
  eq(slice.file, env.listening .. "/j22-sample.wav")
  -- (#361 N-N15: the slice's frames alone, not its whole sample)
  ok(math.abs(slice.seconds - 0.5) < 1e-6, "the file holds slice 2 alone: " .. tostring(slice.seconds))
  eq(slice.offset, nil)
  eq(slice.channels, 2)
  eq(#mock.data.undo.stack, 0, "no undo step")
  H.no_dialogs()
end)

test("listening: audio_file refuses a sample with no audio, a gone sample and a ref of another epoch", function()
  local env, peer = setup({}, { instrument_specs = { { samples = 1 } } })
  local epoch = place(peer)
  failure(call(peer, "audio_file", { ref = epoch .. ":sample:1.1", jobId = "j23", epoch = epoch }), "failed", "That sample has no audio")
  failure(call(peer, "audio_file", { ref = epoch .. ":sample:1.4", jobId = "j23", epoch = epoch }), "gone")
  failure(call(peer, "audio_file", { ref = "77:sample:1.1", jobId = "j23", epoch = epoch }), "stale")
  failure(call(peer, "audio_file", { ref = epoch .. ":device:1.1", jobId = "j23", epoch = epoch }), "bad-params")
  eq(#mock.saved_samples, 0)
  H.no_dialogs()
end)

test("listening: audio_file of a pattern, a slot or a pattern track renders its lines as a job (the track soloed), files `rendered`", function()
  local env, peer = setup()
  mock.data.patterns[2].name = "Verse"
  local epoch = place(peer)
  local job = result(call(peer, "audio_file", { ref = epoch .. ":pattern:2", jobId = "j24", epoch = epoch }), "audio_file of a pattern")
  eq(job.jobId, "j24")
  eq(job.kind, "render")
  eq(job.files[1].id, "pattern")
  local r = mock.renders[1]
  eq(r.options.start_pos.sequence, 2)
  eq(r.options.start_pos.line, 1)
  eq(r.options.end_pos.sequence, 2)
  eq(r.options.end_pos.line, 16)
  mock.finish_render()
  local done = status(peer, "j24")
  eq(done.state, "done")
  eq(done.files[1].kind, "rendered")
  eq(done.files[1].name, "Verse")
  eq(done.files[1].stage, "master")
  -- a slot
  result(call(peer, "audio_file", { ref = epoch .. ":seq:3", jobId = "j25", epoch = epoch }))
  eq(mock.renders[2].options.start_pos.sequence, 3)
  mock.finish_render()
  eq(status(peer, "j25").files[1].id, "slot")
  -- a pattern track: Bass soloed over pattern 1's slot
  track(2).mute_state = MUTED
  result(call(peer, "audio_file", { ref = epoch .. ":ptrack:1.3", jobId = "j26", epoch = epoch }))
  eq(mock.renders[3].tracks[3].solo, true)
  mock.finish_render()
  local ptrack = status(peer, "j26")
  eq(ptrack.files[1].stage, "post-fader")
  eq(ptrack.files[1].name, "Bass · Pattern 00")
  eq(track(2).mute_state, MUTED, "put back")
  eq(track(3).solo, false)
  H.no_dialogs()
end)

test("listening: audio_file of a pattern no slot plays is refused in words", function()
  local env, peer = setup({}, { patterns = 3, sequence_length = 2, sequence = { 1, 2 } })
  local epoch = place(peer)
  failure(call(peer, "audio_file", { ref = epoch .. ":pattern:3", jobId = "j27", epoch = epoch }), "failed",
    "That pattern isn't in the song's sequence, so Renoise can't render it")
  eq(#mock.renders, 0)
  H.no_dialogs()
end)

-- audition: the part of the song a render came from, played by Renoise's own transport

local function rendered_mix(peer, job, range)
  result((start(peer, job, { { id = "mix", what = "master" } }, range)))
  mock.finish_render()
  return status(peer, job).files[1]
end

test("listening: audition plays the part a render came from with Renoise's transport, and puts back the loops and the edit position", function()
  local env, peer = setup()
  local file = rendered_mix(peer, "j28", { fromBeat = 4, beats = 4 })
  local tf = mock.data.transport_fields
  tf.loop_pattern, tf.follow_player = true, true
  tf.edit_pos = { sequence = 1, line = 5 }
  local played = result(call(peer, "audition", { previewId = "p1", file = file.file, fromSeconds = 0.5, gainDb = 0 }), "audition")
  eq(played.previewId, "p1")
  eq(played.playing, true)
  eq(played.via, "song")
  ok(math.abs(played.seconds - 1.5) < 1e-9, "the rest of the file: " .. played.seconds)
  ok(played.note:find("loop is off while Kumi plays", 1, true), played.note)
  eq(tf.playing, true)
  eq(mock.data.started_at.sequence, 2)
  eq(mock.data.started_at.line, 5, "half a second (a beat) into slot 2")
  eq(tf.loop_pattern, false)
  H.tick(10)
  eq(tf.playing, true, "still playing half way")
  H.tick(25)
  eq(tf.playing, false, "stopped at its length")
  eq(tf.loop_pattern, true, "the loop back")
  eq(tf.edit_pos.sequence, 1)
  eq(tf.edit_pos.line, 5, "the edit position back")
  eq(#mock.data.undo.stack, 0)
  H.no_dialogs()
end)

test("listening: after an audition, the edit position follow mode drags to where playback stopped is put back; a producer's click isn't", function()
  local env, peer = setup()
  local file = rendered_mix(peer, "j34")
  local tf = mock.data.transport_fields
  tf.edit_pos = { sequence = 1, line = 5 }
  result(call(peer, "audition", { previewId = "p9", file = file.file }))
  mock.ops.play_at(1, 12, 2.75) -- (where playback is as it stops)
  H.tick(50)
  eq(tf.playing, false)
  eq(tf.edit_pos.line, 5, "put back at the stop")
  -- Renoise's follow mode moves it a moment later, to where playback stopped (measured)
  tf.edit_pos = { sequence = 1, line = 11 }
  H.tick(2)
  eq(tf.edit_pos.line, 5, "put back again")
  -- the producer's click elsewhere is theirs
  tf.edit_pos = { sequence = 3, line = 2 }
  H.tick(2)
  eq(tf.edit_pos.sequence, 3)
  eq(tf.edit_pos.line, 2)
  H.no_dialogs()
end)

test("listening: a track's render is heard in context, gainDb can't be applied, and both are said", function()
  local env, peer = setup()
  local epoch = place(peer)
  result((start(peer, "j29", { { id = "kick", what = "track", ref = track_ref(epoch, 1) } })))
  mock.finish_render()
  local file = status(peer, "j29").files[1]
  local played = result(call(peer, "audition", { previewId = "p2", file = file.file, gainDb = -6 }))
  ok(played.note:find("heard with the rest of the song", 1, true), played.note)
  ok(played.note:find("song's own level", 1, true), played.note)
  eq(track(1).solo, false, "nothing soloed to play it")
  eq(result(call(peer, "audition_stop", { previewId = "p2" })).stopped, true)
  eq(mock.data.transport_fields.playing, false)
  eq(result(call(peer, "audition_stop", {})).stopped, false, "none playing")
  H.no_dialogs()
end)

test("listening: audition refuses a file Kumi didn't render here, and plays nothing while the producer plays", function()
  local env, peer = setup()
  failure(call(peer, "audition", { previewId = "p3", file = "/tmp/reference.wav" }), "failed",
    "Renoise can only play out loud the parts of the song Kumi rendered")
  local file = rendered_mix(peer, "j30")
  mock.data.transport_fields.playing = true
  failure(call(peer, "audition", { previewId = "p3", file = file.file }), "failed", "Renoise is playing: Kumi won't move your playhead")
  mock.data.transport_fields.playing = false
  failure(call(peer, "audition", { previewId = "x3", file = file.file }), "bad-params")
  failure(call(peer, "audition", { previewId = "p3", file = file.file, fromSeconds = 5 }), "bad-params")
  H.no_dialogs()
end)

test("listening: the producer stopping the transport or moving the playhead ends the audition (moved: the playback stays theirs)", function()
  local env, peer = setup()
  local file = rendered_mix(peer, "j31", { fromBeat = 0, beats = 8 })
  local tf = mock.data.transport_fields
  tf.loop_pattern = true
  result(call(peer, "audition", { previewId = "p4", file = file.file }))
  H.tick(10)
  -- the producer starts playing from slot 3
  mock.song.transport:start_at(renoise.SongPos(3, 1))
  H.tick(3)
  eq(tf.playing, true, "the producer's playback goes on")
  eq(tf.loop_pattern, true, "the loop back")
  eq(result(call(peer, "audition_stop", { previewId = "p4" })).stopped, false, "the audition was over")
  tf.playing = false
  -- again, and the producer stops it
  result(call(peer, "audition", { previewId = "p5", file = file.file }))
  H.tick(10)
  mock.song.transport:stop()
  H.tick(3)
  eq(result(call(peer, "audition_stop", {})).stopped, false)
  eq(tf.loop_pattern, true)
  H.no_dialogs()
end)

test("listening: a render stops an audition playing; another song ends it too", function()
  local env, peer = setup()
  local file = rendered_mix(peer, "j32")
  result(call(peer, "audition", { previewId = "p6", file = file.file }))
  eq(mock.data.transport_fields.playing, true)
  result((start(peer, "j33", { { id = "mix", what = "master" } })))
  eq(mock.renders[#mock.renders].was_playing, false, "the audition stopped before the render")
  mock.finish_render()
  file = status(peer, "j33").files[1]
  result(call(peer, "audition", { previewId = "p7", file = file.file }))
  mock.load_song(song_options({ name = "Other" }))
  H.tick(2)
  eq(result(call(peer, "audition_stop", {})).stopped, false)
  failure(call(peer, "audition", { previewId = "p8", file = file.file }), "failed", "another song")
  H.no_dialogs()
end)

-- Reads: spans, grid

local function timing_command(text)
  local digits = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ"
  return 256 + (digits:find(text:sub(1, 1), 1, true) - 1) * 36 + (digits:find(text:sub(2, 2), 1, true) - 1)
end

local function preflight(peer, range)
  local epoch = place(peer)
  return call(peer, "read", { kind = "render_preflight", range = range or { fromBeat = 0, beats = 4 }, epoch = epoch, limit = 1 })
end

test("listening: render_preflight reads exact planning and full track state without writes, folders or render jobs", function()
  local env, peer = setup()
  mock.solo_write(2, true)
  track(3).mute_state = MUTED
  local epoch = place(peer)
  local before, writes, changed = states(), mock.api_writes, bridge().changed
  local answer = result(preflight(peer))
  eq(answer.schema, 1)
  eq(answer.epoch, epoch)
  eq(answer.seconds, 2)
  eq(answer.offset, 0)
  eq(answer.aligned, true)
  eq(answer.irregular, false)
  eq(answer.clipped, false)
  eq(answer.items, nil, "the schema object is not wrapped in a row")
  eq(answer.nextCursor, nil)
  eq(answer.render_state.bpm, 120)
  eq(answer.render_state.lpb, 4)
  eq(#answer.render_state.tracks, 5, "all non-master tracks, including the send, despite limit=1")
  local indices = { 1, 2, 3, 4, 6 }
  for k, index in ipairs(indices) do
    T.same(answer.render_state.tracks[k], {
      ref = track_ref(epoch, index), mute = track(index).mute_state, solo = track(index).solo == true,
    })
  end
  eq(mock.api_writes, writes, "no API property writes, including writes later put back")
  eq(bridge().changed, changed)
  eq(states(), before)
  eq(#mock.data.undo.stack, 0)
  eq(#mock.renders, 0)
  eq(bridge().listen.job, nil)
  eq(next(bridge().listen.ended), nil)
  ok(not exists(env.listening), "preflight did not create the listening folder")
  failure(call(peer, "read", { kind = "render_preflight", range = { fromBeat = 0, beats = 4 }, epoch = "not-current" }), "stale")
  H.no_dialogs()
end)

test("listening: render_preflight follows ZT and ZL timing, including the renderer's first-command chase", function()
  local env, peer = setup()
  mock.ops.set_effect(1, 2, 1, 1, timing_command("ZT"), 120)
  mock.ops.set_effect(2, 4, 1, 1, timing_command("ZT"), 60)
  mock.ops.set_effect(1, 2, 1, 2, timing_command("ZL"), 8)
  -- The second effect column must be visible for Renoise to play it.
  mock.data.song:track(2).visible_effect_columns = 2
  local answer = result(preflight(peer, { fromBeat = 0, beats = 4 }))
  eq(answer.seconds, 3, "2 beats at 120 then 2 at 60, LPB 8")
  eq(answer.offset, 0)
  eq(answer.aligned, true)
  eq(answer.render_state.bpm, 120, "snapshot is transport state, not chased state")
  eq(answer.render_state.lpb, 4)
  local started = result((start(peer, "jpf1", { { id = "mix", what = "master" } }, { fromBeat = 0, beats = 4 }, { maxSeconds = 60 })))
  eq(started.seconds, answer.seconds)
  eq(mock.renders[1].seconds, answer.seconds)
  mock.finish_render()
  eq(status(peer, "jpf1").state, "done")
  eq(mock.data.transport_fields.bpm, 120, "guarded completion restores the expected render-left tempo natively")
  eq(mock.data.transport_fields.lpb, 4)
  env, peer = setup()
  mock.ops.set_effect(2, 4, 9, 1, timing_command("ZT"), 60)
  answer = result(preflight(peer, { fromBeat = 0, beats = 4 }))
  eq(answer.seconds, 4, "Renoise chases its first ZT even before that command")
  eq(answer.render_state.bpm, 120)
  H.no_dialogs()
end)

test("listening: render_preflight reports both fractional endpoints and clipped song ends", function()
  local env, peer = setup()
  local cases = {
    { 0.125, 0.875, false, false, 0.0625, 0.5 },
    { 0, 1.125, false, false, 0, 0.625 },
    { 11, 1, true, false, 0, 0.5 },
    { 11, 2, false, true, 0, 0.5 },
  }
  for _, c in ipairs(cases) do
    local answer = result(preflight(peer, { fromBeat = c[1], beats = c[2] }))
    eq(answer.aligned, c[3])
    eq(answer.clipped, c[4])
    eq(answer.offset, c[5])
    eq(answer.seconds, c[6], "actual full-line span, not requested fractional span")
  end
  eq(#mock.renders, 0)
  ok(not exists(env.listening))
  H.no_dialogs()
end)

test("listening: guarded render rejects fractional, clipped, irregular and preroll ranges before mutation", function()
  local cases = {
    { range = { fromBeat = 0.125, beats = 0.875 } },
    { range = { fromBeat = 0, beats = 1.125 } },
    { range = { fromBeat = 11, beats = 2 } },
    { command = "ZB" },
    { command = "ZD" },
    { speed = true },
    { preroll = 1 },
  }
  for _, c in ipairs(cases) do
    local env, peer = setup()
    if c.command then mock.ops.set_effect(1, 1, 1, 1, timing_command(c.command), 1) end
    if c.speed then mock.data.transport_fields.timing_model = renoise.Transport.TIMING_MODEL_SPEED end
    local range = c.range or { fromBeat = 0, beats = 4 }
    local read = result(preflight(peer, range))
    if c.command or c.speed then eq(read.irregular, true) end
    local before, writes = states(), mock.api_writes
    failure((start(peer, "jpf2", { { id = "kick", what = "track", ref = track_ref(place(peer), 1) } },
      range, { maxSeconds = 60, preRollBeats = c.preroll or 0 })), "bad-params")
    eq(states(), before)
    eq(mock.api_writes, writes)
    eq(#mock.renders, 0)
    eq(bridge().listen.job, nil)
    ok(not exists(env.listening))
    H.no_dialogs()
  end
end)

test("listening: guarded render validates cap and actual seconds while standalone retains longer renders", function()
  local env, peer = setup(nil, { pattern_lines = 256, patterns = 1, sequence_length = 1, sequence = { 1 }, bpm = 60 })
  local range = { fromBeat = 0, beats = 64 }
  eq(result(preflight(peer, range)).seconds, 64)
  failure((start(peer, "jpf3", { { id = "mix", what = "master" } }, range, { maxSeconds = 60 })), "stale")
  for _, cap in ipairs({ 0, -1, 60.01, "60", true, json.null }) do
    failure((start(peer, "jpf3", { { id = "mix", what = "master" } }, { fromBeat = 0, beats = 1 }, { maxSeconds = cap })),
      "bad-params")
  end
  failure((start(peer, "jpf3", { { id = "mix", what = "master" } }, { fromBeat = 0, beats = 2 }, { maxSeconds = 1 })),
    "stale")
  eq(#mock.renders, 0)
  ok(not exists(env.listening))
  local boundary = result((start(peer, "jpf60", { { id = "mix", what = "master" } },
    { fromBeat = 0, beats = 60 }, { maxSeconds = 60 })))
  eq(boundary.seconds, 60, "the exact limit is allowed")
  mock.finish_render({ no_file = true })
  local answer = result((start(peer, "jpf3", { { id = "mix", what = "master" } }, range)))
  eq(answer.seconds, 64, "standalone keeps its existing 3600-second limit")
  mock.finish_render({ no_file = true })
  H.no_dialogs()
end)

test("listening: preflight and guarded render refuse live playback, recording and rendering without stopping them", function()
  for _, field in ipairs({ "playing", "edit_mode", "rendering" }) do
    local env, peer = setup()
    local target = field == "rendering" and mock.impl(mock.data.song) or mock.data.transport_fields
    target[field] = true
    local before, writes = states(), mock.api_writes
    failure(preflight(peer), "busy")
    failure((start(peer, "jpf4", { { id = "mix", what = "master" } }, nil, { maxSeconds = 60 })), "busy")
    eq(target[field], true)
    eq(states(), before)
    eq(mock.api_writes, writes)
    eq(#mock.renders, 0)
    ok(not exists(env.listening))
    target[field] = false
    H.no_dialogs()
  end
end)

-- Exercise the real plan/launch boundary without the bridge dispatch running launch immediately after plan.
local function queued_plan(peer, range, cap, deadline)
  local listening = T.load_payload_module("listening")
  local state = {}
  listening.init(state)
  local epoch, layout = place(peer)
  local env = {
    cfg = function() return bridge().cfg end,
    epoch = function() return epoch end, layout = function() return layout end,
    document = function() return bridge().document end,
    busy = function() return nil end, quiet = function(fn) return fn() end,
    shifted = function(t) return t end,
    -- Deliberately stale: actual launch must not trust a cached pattern mark.
    pattern_mark = function() return 1 end,
    before_render = function() error("guarded refusal reached before_render") end,
  }
  local params = { jobId = "jqueued", epoch = epoch, sources = { { id = "kick", what = "track", ref = track_ref(epoch, 1) } },
    range = range, maxSeconds = cap, preRollBeats = 0, deadlineUnixMs = deadline }
  return listening, listening.plan(params, env, "render"), env, state
end

test("listening: native guarded planning rejects nonfinite caps before folder or API writes", function()
  for _, cap in ipairs({ math.huge, -math.huge, 0 / 0 }) do
    local env, peer = setup()
    local writes = mock.api_writes
    local fine, err = pcall(queued_plan, peer, { fromBeat = 0, beats = 4 }, cap)
    ok(not fine)
    eq(type(err), "table")
    eq(err.code, "bad-params")
    eq(mock.api_writes, writes)
    eq(#mock.renders, 0)
    ok(not exists(env.listening))
    H.no_dialogs()
  end
end)

test("listening: guarded rendering never repairs an existing listening root's permissions", function()
  for _, mode in ipairs({ 33152, 33188, "unknown" }) do
    local env, peer = setup()
    ok(os.mkdir(env.listening))
    mock.stat_modes[env.listening] = mode
    local chmod, calls = io.chmod, {}
    io.chmod = function(path, bits) calls[#calls + 1] = { path, bits } return true end
    local fine, response = pcall(start, peer, "jprivate", { { id = "mix", what = "master" } }, nil, { maxSeconds = 60 })
    io.chmod = chmod
    ok(fine)
    eq(#calls, 0, "guarded planning must not chmod a pre-existing root")
    eq(mock.stat_modes[env.listening], mode)
    if mode == 33152 then
      result(response)
      mock.finish_render({ no_file = true })
    else
      failure(response, "failed", "not verifiably private")
      eq(#mock.renders, 0)
      eq(bridge().listen.job, nil)
    end
    H.no_dialogs()
  end
end)

test("listening: guarded queued launch rechecks tempo, commands, fractional bounds and live transport before writes", function()
  local cases = {
    { range = { fromBeat = 0, beats = 4 }, cap = 2, change = function() mock.data.transport_fields.bpm = 60 end, code = "stale" },
    { range = { fromBeat = 0, beats = 4 }, cap = 2,
      change = function() mock.ops.set_effect(1, 1, 1, 1, timing_command("ZT"), 60) end, code = "stale" },
    { change = function() mock.ops.set_effect(1, 1, 1, 1, timing_command("ZB"), 1) end, code = "bad-params" },
    { change = function() mock.ops.set_effect(1, 1, 1, 1, timing_command("ZD"), 1) end, code = "bad-params" },
    { change = function() mock.data.transport_fields.timing_model = renoise.Transport.TIMING_MODEL_SPEED end, code = "bad-params" },
    { change = function() mock.data.patterns[1].lines = 64 end, code = "bad-params" },
    { change = function() mock.ops.set_effect(1, 1, 1, 1, timing_command("ZL"), 8) end, code = "bad-params" },
    { range = { fromBeat = 0, beats = 1.25 },
      change = function() mock.ops.set_effect(1, 1, 1, 1, timing_command("ZL"), 3) end, code = "bad-params" },
    { range = { fromBeat = 0.25, beats = 1 },
      change = function() mock.ops.set_effect(1, 1, 1, 1, timing_command("ZL"), 3) end, code = "bad-params" },
    { change = function() mock.data.transport_fields.playing = true end, code = "busy" },
    { change = function() mock.data.transport_fields.edit_mode = true end, code = "busy" },
    { change = function() mock.impl(mock.data.song).rendering = true end, code = "busy" },
  }
  for _, c in ipairs(cases) do
    local setup_env, peer = setup(nil, { pattern_lines = 256, patterns = 1, sequence_length = 1, sequence = { 1 } })
    local listening, job, env, state = queued_plan(peer, c.range or { fromBeat = 0, beats = 64 }, c.cap or 60)
    c.change()
    local before, writes, undos = states(), mock.api_writes, #mock.data.undo.stack
    local fine, err = pcall(listening.launch, job, env)
    ok(not fine)
    eq(type(err), "table")
    eq(err.code, c.code)
    eq(mock.api_writes, writes)
    eq(states(), before)
    eq(#mock.data.undo.stack, undos)
    eq(#mock.renders, 0)
    eq(state.listen.job, nil)
    eq(job.snapshot, nil)
    eq(job.tempo_before, nil)
    ok(not exists(setup_env.listening .. "/jqueued-kick.wav"))
    mock.impl(mock.data.song).rendering = false
    mock.data.transport_fields.playing, mock.data.transport_fields.edit_mode = false, false
    H.no_dialogs()
  end
end)

test("listening: guarded queued launch uses fresh permissible timing rather than stale planned seconds", function()
  local setup_env, peer = setup()
  local listening, job, env = queued_plan(peer, { fromBeat = 0, beats = 4 }, 60)
  eq(job.range.seconds, 2)
  mock.ops.set_effect(1, 1, 1, 1, timing_command("ZT"), 60)
  env.before_render = nil
  listening.launch(job, env)
  eq(job.range.seconds, 4)
  eq(job.sources[1].seconds, 4)
  eq(mock.renders[1].seconds, 4)
  mock.finish_render()
  eq(job.state, "done")
  eq(mock.data.transport_fields.bpm, 120, "guarded completion restores the launch snapshot, not the planned tempo")
  H.no_dialogs()
end)

test("listening: guarded completion preserves a newer tempo pair and publishes no files", function()
  for _, outcome in ipairs({ "done", "failed", "cancelled" }) do
    for _, field in ipairs({ "bpm", "lpb" }) do
      local setup_env, peer = setup()
      mock.ops.set_effect(1, 1, 1, 1, timing_command("ZT"), 60)
      mock.ops.set_effect(1, 2, 1, 1, timing_command("ZL"), 8)
      local listening, job, env = queued_plan(peer, { fromBeat = 0, beats = 4 }, 60)
      env.before_render = nil
      listening.launch(job, env)
      job.stopping = outcome == "cancelled"
      local completed = mock.finish_render({ no_callback = true, fraction = outcome == "failed" and 0.5 or nil })
      local changed = field == "bpm" and 137 or 7
      mock.data.transport_fields[field] = changed
      completed.done()
      eq(job.state, "failed", outcome)
      eq(job.code, "tempo_restore_ownership_uncertain", outcome)
      eq(mock.data.transport_fields.bpm, field == "bpm" and changed or 60, "the entire producer pair survives")
      eq(mock.data.transport_fields.lpb, field == "lpb" and changed or 8, "the entire producer pair survives")
      ok(not exists(job.sources[1].file), "no non-success audio remains")
      eq(listening.status({ jobId = job.id }, env).files, nil, "uncertain audio is never published")
      H.no_dialogs()
    end
  end
end)

test("listening: guarded tempo cleanup requires the same stopped document and verifies restoration", function()
  for _, fault in ipairs({ "document", "epoch", "rendering", "playing", "edit_mode", "write", "readback" }) do
    local setup_env, peer = setup()
    mock.ops.set_effect(1, 1, 1, 1, timing_command("ZT"), 60)
    local listening, job, env = queued_plan(peer, { fromBeat = 0, beats = 4 }, 60)
    env.before_render = nil
    listening.launch(job, env)
    local completed = mock.finish_render({ no_callback = true })
    if fault == "document" then
      env.document = function() return "another-document" end
    elseif fault == "epoch" then
      env.epoch = function() return "another-epoch" end
    elseif fault == "rendering" then
      mock.impl(mock.data.song).rendering = true
    elseif fault == "playing" or fault == "edit_mode" then
      mock.data.transport_fields[fault] = true
    else
      -- Keep the mute/solo restoration independent of the injected tempo setter fault.
      local quiet, calls = env.quiet, 0
      env.quiet = function(fn)
        calls = calls + 1
        if calls > 1 then
          if fault == "write" then error("tempo setter refused") end
          fn()
          mock.data.transport_fields.bpm = 60
        else
          quiet(fn)
        end
      end
    end
    completed.done()
    eq(job.state, "failed", fault)
    eq(job.code, (fault == "write" or fault == "readback") and "tempo_restore_failed" or "tempo_restore_state_changed", fault)
    eq(mock.data.transport_fields.bpm, 60, fault)
    ok(not exists(job.sources[1].file), "non-success never retains audio")
    mock.impl(mock.data.song).rendering = false
    mock.data.transport_fields.playing, mock.data.transport_fields.edit_mode = false, false
    H.no_dialogs()
  end
end)

test("listening: a cancelled guarded render restores both values at the owned pair and deletes audio", function()
  local env, peer = setup()
  mock.ops.set_effect(1, 1, 1, 1, timing_command("ZT"), 60)
  mock.ops.set_effect(1, 2, 1, 1, timing_command("ZL"), 8)
  result((start(peer, "jguardcancel", { { id = "mix", what = "master" } }, nil, { maxSeconds = 60 })))
  result(call(peer, "listen_cancel", { jobId = "jguardcancel" }))
  local completed = mock.finish_render({ no_callback = true })
  eq(mock.data.transport_fields.bpm, 60)
  eq(mock.data.transport_fields.lpb, 8)
  completed.done()
  eq(status(peer, "jguardcancel").state, "cancelled")
  eq(status(peer, "jguardcancel").files, nil, "cancelled audio is never published")
  eq(mock.data.transport_fields.bpm, 120)
  eq(mock.data.transport_fields.lpb, 4)
  ok(not exists(env.listening .. "/jguardcancel-mix.wav"))
  H.no_dialogs()
end)

test("listening: a failed guarded render restores both values at the owned pair without publishing audio", function()
  for _, fault in ipairs({ "missing-file", "partial-file" }) do
    local env, peer = setup()
    mock.ops.set_effect(1, 1, 1, 1, timing_command("ZT"), 60)
    mock.ops.set_effect(1, 2, 1, 1, timing_command("ZL"), 8)
    result((start(peer, "jguardfail", { { id = "mix", what = "master" } }, nil, { maxSeconds = 60 })))
    local completed = mock.finish_render({ no_callback = true, no_file = fault == "missing-file",
      fraction = fault == "partial-file" and 0.5 or nil })
    eq(mock.data.transport_fields.bpm, 60)
    eq(mock.data.transport_fields.lpb, 8)
    completed.done()
    local failed = status(peer, "jguardfail")
    eq(failed.state, "failed", fault)
    eq(failed.code, "failed", "restoration must preserve the original render failure")
    eq(failed.files, nil, "failed audio is never published")
    eq(mock.data.transport_fields.bpm, 120)
    eq(mock.data.transport_fields.lpb, 4)
    ok(not exists(env.listening .. "/jguardfail-mix.wav"))
    H.no_dialogs()
  end
end)

test("listening: a partial guarded render never guesses ownership of an intermediate tempo pair", function()
  local setup_env, peer = setup()
  mock.ops.set_effect(1, 1, 1, 1, timing_command("ZT"), 60)
  mock.ops.set_effect(1, 2, 1, 1, timing_command("ZL"), 8)
  mock.ops.set_effect(2, 1, 1, 1, timing_command("ZT"), 90)
  local listening, job, env = queued_plan(peer, { fromBeat = 0, beats = 4 }, 60)
  env.before_render = nil
  listening.launch(job, env)
  eq(job.tempo_expected.bpm, 90)
  eq(job.tempo_expected.lpb, 8)
  local completed = mock.finish_render({ no_callback = true, fraction = 0.25 })
  -- The mock leaves the end pair even for a short file. Model an earlier render stop explicitly:
  -- 60/8 is in the score, but the callback/file length cannot prove who left that pair.
  mock.data.transport_fields.bpm = 60
  completed.done()
  eq(job.state, "failed")
  eq(job.code, "tempo_restore_ownership_uncertain")
  eq(mock.data.transport_fields.bpm, 60)
  eq(mock.data.transport_fields.lpb, 8)
  ok(not exists(job.sources[1].file))
  H.no_dialogs()
end)

test("listening: guarded completion never rewrites a pair already restored to the launch snapshot", function()
  local setup_env, peer = setup()
  mock.ops.set_effect(1, 1, 1, 1, timing_command("ZT"), 60)
  mock.ops.set_effect(1, 2, 1, 1, timing_command("ZL"), 8)
  local listening, job, env = queued_plan(peer, { fromBeat = 0, beats = 4 }, 60)
  env.before_render = nil
  listening.launch(job, env)
  local completed = mock.finish_render({ no_callback = true })
  mock.data.transport_fields.bpm, mock.data.transport_fields.lpb = 120, 4
  local quiet, calls = env.quiet, 0
  env.quiet = function(fn)
    calls = calls + 1
    return quiet(fn)
  end
  completed.done()
  eq(job.state, "done")
  eq(calls, 1, "only mute/solo restoration needs a write action")
  eq(mock.data.transport_fields.bpm, 120)
  eq(mock.data.transport_fields.lpb, 4)
  H.no_dialogs()
end)

test("listening: guarded callback waits for stopped idle before restoring on every render outcome", function()
  for _, outcome in ipairs({ "done", "missing-file", "partial-file", "cancelled" }) do
    local setup_env, peer = setup()
    mock.ops.set_effect(1, 1, 1, 1, timing_command("ZT"), 60)
    mock.ops.set_effect(1, 2, 1, 1, timing_command("ZL"), 8)
    mock.data.song.transport.edit_pos = renoise.SongPos(3, 7)
    mock.data.transport_fields.follow_player = true
    mock.data.song_fields.selected_track_index = 2
    mock.data.song_fields.selected_note_column_index = 0
    mock.data.song_fields.selected_effect_column_index = 1
    mock.data.song_fields.selection_in_pattern = {
      start_line = 2, end_line = 5, start_track = 2, end_track = 3, start_column = 1, end_column = 2,
    }
    local focus_before = result(call(peer, "focus"))
    local listening, job, env = queued_plan(peer, { fromBeat = 0, beats = 4 }, 60, (os.time() + 60) * 1000)
    env.before_render = nil
    local before = states()
    listening.launch(job, env)
    local completed = mock.finish_render({ no_callback = true, no_file = outcome == "missing-file",
      fraction = outcome == "partial-file" and 0.5 or nil })
    -- Native ordering is deliberately unproven: callback first, rendering/playing clear on a later pulse.
    mock.impl(mock.data.song).rendering = true
    mock.data.transport_fields.playing = true
    local writes, during = mock.api_writes, states()
    completed.done()
    eq(job.state, "running", outcome)
    local pending = listening.status({ jobId = job.id }, env)
    eq(pending.state, "running", outcome)
    eq(pending.files, nil, "pending audio is never published")
    eq(pending.restored, nil, "pending is not a restoration claim")
    eq(listening.origin(job.sources[1].file), nil)
    eq(mock.api_writes, writes, "no DAW writes from callback or pending check")
    eq(states(), during)
    eq(mock.data.transport_fields.bpm, 60)
    eq(mock.data.transport_fields.lpb, 8)
    eq(mock.data.transport_fields.edit_pos.sequence, job.range.start[1], "pending keeps the renderer's cursor")
    eq(mock.data.transport_fields.edit_pos.line, job.range.start[2])
    if outcome == "cancelled" then
      local cancelled = listening.cancel({ jobId = job.id }, env)
      eq(cancelled.state, "running")
      eq(cancelled.stopping, true)
    end
    mock.impl(mock.data.song).rendering = false
    mock.data.transport_fields.playing = false
    listening.check(env)
    local done = listening.status({ jobId = job.id }, env)
    eq(done.state, outcome == "done" and "done" or outcome == "cancelled" and "cancelled" or "failed", outcome)
    eq(done.restored, true, outcome)
    eq(mock.data.transport_fields.bpm, 120)
    eq(mock.data.transport_fields.lpb, 4)
    eq(states(), before, "mute/solo restoration waits for the same stopped pulse")
    local focus_after = result(call(peer, "focus"))
    T.same(focus_after.selection, focus_before.selection, "edit cursor and selection")
    T.same(focus_after.transport, focus_before.transport, "transport and follow-player")
    if outcome == "done" then
      ok(done.files and #done.files == 1)
    else
      eq(done.files, nil)
      ok(not exists(job.sources[1].file))
    end
    H.no_dialogs()
  end
end)

test("listening: guarded deferred cleanup leaves producer cursor, selection and follow-player changes untouched", function()
  for _, changed in ipairs({ "position", "track", "range", "follow", "layout" }) do
    for _, cancelled in ipairs({ false, true }) do
      local setup_env, peer = setup()
      local song, tf, fields = mock.data.song, mock.data.transport_fields, mock.data.song_fields
      song.transport.edit_pos = renoise.SongPos(3, 7)
      tf.follow_player = true
      mock.ops.set_effect(1, 1, 1, 1, timing_command("ZT"), 60)
      mock.ops.set_effect(1, 2, 1, 1, timing_command("ZL"), 8)
      local listening, job, env = queued_plan(peer, { fromBeat = 0, beats = 4 }, 60, (os.time() + 60) * 1000)
      env.before_render = nil
      listening.launch(job, env)
      job.stopping = cancelled
      local completed = mock.finish_render({ no_callback = true })
      completed.done()
      if changed == "position" then
        song.transport.edit_pos = renoise.SongPos(2, 3)
      elseif changed == "track" then
        fields.selected_track_index = 4
      elseif changed == "range" then
        fields.selection_in_pattern = { start_line = 2, end_line = 5, start_track = 2, end_track = 3 }
      elseif changed == "follow" then
        tf.follow_player = false
      else
        env.layout = function() return job.layout + 1 end
      end
      local position = { tf.edit_pos.sequence, tf.edit_pos.line }
      local selected, block, follow = fields.selected_track_index, fields.selection_in_pattern, tf.follow_player
      local writes, during = mock.api_writes, states()
      local failed = listening.status({ jobId = job.id }, env)
      eq(failed.state, "failed", changed)
      eq(failed.restored, false, changed)
      eq(failed.code, changed == "position" and "cursor_restore_ownership_uncertain" or "cursor_restore_state_changed")
      eq(mock.api_writes, writes, "the fence runs before any restoration write")
      eq(tf.edit_pos.sequence, position[1])
      eq(tf.edit_pos.line, position[2])
      eq(fields.selected_track_index, selected)
      eq(fields.selection_in_pattern, block)
      eq(tf.follow_player, follow)
      eq(tf.bpm, 60)
      eq(tf.lpb, 8)
      eq(states(), during)
      eq(failed.files, nil)
      eq(listening.origin(job.sources[1].file), nil)
      ok(not exists(job.sources[1].file))
      H.no_dialogs()
    end
  end
end)

test("listening: guarded cursor write and readback failures publish no audio", function()
  for _, fault in ipairs({ "write", "readback", "selection" }) do
    local setup_env, peer = setup()
    local song, tf = mock.data.song, mock.data.transport_fields
    song.transport.edit_pos = fault == "selection" and renoise.SongPos(1, 1) or renoise.SongPos(3, 7)
    tf.follow_player = true
    mock.ops.set_effect(1, 1, 1, 1, timing_command("ZT"), 60)
    mock.ops.set_effect(1, 2, 1, 1, timing_command("ZL"), 8)
    local listening, job, env = queued_plan(peer, { fromBeat = 0, beats = 4 }, 60, (os.time() + 60) * 1000)
    env.before_render = nil
    listening.launch(job, env)
    local completed = mock.finish_render({ no_callback = true })
    local quiet = env.quiet
    env.quiet = function(fn)
      if tf.bpm == 120 and tf.lpb == 4 then
        if fault == "write" then error("native edit position setter refused") end
        quiet(fn)
        song.transport.edit_pos = renoise.SongPos(job.range.start[1], job.range.start[2])
      else
        quiet(fn)
        if fault == "selection" and tf.bpm == 120 and tf.lpb == 4 then mock.data.song_fields.selected_track_index = 4 end
      end
    end
    completed.done()
    local failed = listening.status({ jobId = job.id }, env)
    eq(failed.state, "failed", fault)
    eq(failed.restored, false, fault)
    eq(failed.code, "cursor_restore_failed", fault)
    eq(tf.bpm, 120)
    eq(tf.lpb, 4)
    eq(tf.follow_player, true)
    eq(failed.files, nil)
    ok(not exists(job.sources[1].file))
    H.no_dialogs()
  end
end)

test("listening: guarded deferred completion expires within the original deadline without any cleanup writes", function()
  for _, callback in ipairs({ false, true }) do
    local setup_env, peer = setup()
    mock.ops.set_effect(1, 1, 1, 1, timing_command("ZT"), 60)
    mock.ops.set_effect(1, 2, 1, 1, timing_command("ZL"), 8)
    local listening, job, env = queued_plan(peer, { fromBeat = 0, beats = 4 }, 60, (os.time() + 60) * 1000)
    env.before_render = nil
    listening.launch(job, env)
    local completed = mock.finish_render({ no_callback = true })
    mock.impl(mock.data.song).rendering = true
    mock.data.transport_fields.playing = true
    local deadline, writes, during = job.deadline_ms, mock.api_writes, states()
    -- Spending native time before the callback must not give deferral a fresh allowance.
    mock.time = deadline - 2000
    if callback then completed.done() end
    listening.check(env)
    eq(job.state, "running")
    eq(job.deadline_ms, deadline)
    mock.time = deadline
    local failed = listening.status({ jobId = job.id }, env)
    eq(failed.state, "failed")
    eq(failed.code, "tempo_restore_state_changed")
    eq(failed.restored, false)
    eq(job.restored, false, "no mute/solo restoration is claimed either")
    eq(failed.files, nil)
    eq(listening.origin(job.sources[1].file), nil)
    eq(mock.api_writes, writes, "deadline expiry must make no DAW writes")
    eq(states(), during)
    eq(mock.data.transport_fields.bpm, 60)
    eq(mock.data.transport_fields.lpb, 8)
    ok(not exists(job.sources[1].file))
    mock.impl(mock.data.song).rendering = false
    mock.data.transport_fields.playing = false
    completed.done()
    listening.check(env)
    eq(job.state, "failed", "a late callback cannot revive or restore the expired job")
    eq(mock.api_writes, writes)
    eq(mock.data.transport_fields.bpm, 60)
    eq(mock.data.transport_fields.lpb, 8)
    H.no_dialogs()
  end
end)

test("listening: a producer pair changed during guarded deferral is never overwritten", function()
  for _, field in ipairs({ "bpm", "lpb" }) do
    local setup_env, peer = setup()
    mock.ops.set_effect(1, 1, 1, 1, timing_command("ZT"), 60)
    mock.ops.set_effect(1, 2, 1, 1, timing_command("ZL"), 8)
    local listening, job, env = queued_plan(peer, { fromBeat = 0, beats = 4 }, 60, (os.time() + 60) * 1000)
    env.before_render = nil
    listening.launch(job, env)
    local completed = mock.finish_render({ no_callback = true })
    mock.impl(mock.data.song).rendering = true
    completed.done()
    listening.check(env)
    eq(job.state, "running")
    local changed = field == "bpm" and 137 or 7
    mock.data.transport_fields[field] = changed
    mock.impl(mock.data.song).rendering = false
    local failed = listening.status({ jobId = job.id }, env)
    eq(failed.state, "failed")
    eq(failed.code, "tempo_restore_ownership_uncertain")
    eq(failed.restored, false)
    eq(mock.data.transport_fields.bpm, field == "bpm" and changed or 60)
    eq(mock.data.transport_fields.lpb, field == "lpb" and changed or 8)
    eq(failed.files, nil)
    ok(not exists(job.sources[1].file))
    H.no_dialogs()
  end
end)

test("listening: guarded deferred cleanup never writes into a changed document or an active render", function()
  for _, change in ipairs({ "document", "epoch", "abort", "song-closed" }) do
    local setup_env, peer = setup()
    mock.ops.set_effect(1, 1, 1, 1, timing_command("ZT"), 60)
    local listening, job, env = queued_plan(peer, { fromBeat = 0, beats = 4 }, 60, (os.time() + 60) * 1000)
    env.before_render = nil
    listening.launch(job, env)
    local completed = mock.finish_render({ no_callback = true })
    mock.impl(mock.data.song).rendering = true
    completed.done()
    listening.check(env)
    eq(job.state, "running")
    local writes, during = mock.api_writes, states()
    if change == "document" then
      env.document = function() return "another-document" end
      mock.impl(mock.data.song).rendering = false
      listening.check(env)
    elseif change == "epoch" then
      env.epoch = function() return "another-epoch" end
      mock.impl(mock.data.song).rendering = false
      listening.check(env)
    elseif change == "abort" then
      listening.abort(env, "the tool stopped during deferral")
    else
      listening.song_closed(env)
    end
    local failed = listening.status({ jobId = job.id }, env)
    eq(failed.state, "failed", change)
    eq(failed.code, "tempo_restore_state_changed", change)
    eq(failed.restored, false, change)
    eq(failed.files, nil)
    eq(mock.api_writes, writes, change)
    eq(states(), during)
    eq(mock.data.transport_fields.bpm, 60)
    ok(not exists(job.sources[1].file))
    mock.impl(mock.data.song).rendering = false
    completed.done()
    eq(mock.api_writes, writes, "late callback cannot undo uncertainty")
    H.no_dialogs()
  end
end)

test("listening: guarded planning validates an optional host deadline before folder preparation or DAW writes", function()
  for _, deadline in ipairs({ json.null, "later", true, 0 }) do
    local env, peer = setup()
    local epoch = place(peer)
    local params = { jobId = "jdeadline", epoch = epoch, sources = { { id = "mix", what = "master" } },
      range = { fromBeat = 0, beats = 4 }, maxSeconds = 60 }
    params.deadlineUnixMs = deadline
    local writes = mock.api_writes
    failure(call(peer, "render_start", params), deadline == 0 and "stale" or "bad-params")
    eq(mock.api_writes, writes)
    eq(#mock.renders, 0)
    ok(not exists(env.listening))
    H.no_dialogs()
  end
end)

test("listening: queued guarded launch never restarts an expired native deadline", function()
  local setup_env, peer = setup()
  local listening, job, env, state = queued_plan(peer, { fromBeat = 0, beats = 4 }, 60, (os.time() + 60) * 1000)
  local writes, before, deadline = mock.api_writes, states(), job.deadline_ms
  mock.time = deadline
  local fine, problem = pcall(listening.launch, job, env)
  eq(fine, false)
  eq(type(problem), "table")
  eq(problem.code, "stale")
  eq(job.deadline_ms, deadline)
  eq(mock.api_writes, writes)
  eq(states(), before)
  eq(#mock.renders, 0)
  eq(state.listen.job, nil)
  eq(job.tempo_before, nil)
  ok(not exists(job.sources[1].file))
  H.no_dialogs()
end)

test("listening: guarded requests without a private deadline retain immediate callback completion", function()
  for _, rendering in ipairs({ false, true }) do
    local setup_env, peer = setup()
    mock.ops.set_effect(1, 1, 1, 1, timing_command("ZT"), 60)
    local listening, job, env = queued_plan(peer, { fromBeat = 0, beats = 4 }, 60)
    eq(job.deadline_ms, nil, "legacy clients do not opt in to deferral")
    env.before_render = nil
    listening.launch(job, env)
    local completed = mock.finish_render({ no_callback = true })
    mock.impl(mock.data.song).rendering = rendering
    completed.done()
    eq(job.state, rendering and "failed" or "done", "no idle/request pulse is needed for legacy completion")
    eq(mock.data.transport_fields.bpm, rendering and 60 or 120)
    if rendering then
      eq(job.code, "tempo_restore_state_changed")
      ok(not exists(job.sources[1].file))
    end
    mock.impl(mock.data.song).rendering = false
    H.no_dialogs()
  end
end)

test("listening: wall-clock corrections after anchoring neither shorten nor extend guarded deferral", function()
  for _, adjustment in ipairs({ 3600, -3600 }) do
    local setup_env, peer = setup()
    mock.ops.set_effect(1, 1, 1, 1, timing_command("ZT"), 60)
    local listening, job, env = queued_plan(peer, { fromBeat = 0, beats = 4 }, 60, (os.time() + 60) * 1000)
    env.before_render = nil
    listening.launch(job, env)
    local completed = mock.finish_render({ no_callback = true })
    mock.impl(mock.data.song).rendering = true
    completed.done()
    local deadline, writes = job.deadline_ms, mock.api_writes
    local wall_time = os.time
    os.time = function(t)
      if t then return wall_time(t) end
      return wall_time() + adjustment
    end
    local fine, problem = pcall(function()
      listening.check(env)
      eq(job.state, "running", "a forward wall-clock correction cannot expire the anchored budget")
      mock.time = deadline - 1
      listening.check(env)
      eq(job.state, "running")
      eq(job.deadline_ms, deadline)
      mock.time = deadline
      local failed = listening.status({ jobId = job.id }, env)
      eq(failed.state, "failed", "a backward wall-clock correction cannot extend the anchored budget")
      eq(failed.code, "tempo_restore_state_changed")
      eq(failed.restored, false)
      eq(failed.files, nil)
      eq(mock.api_writes, writes)
      eq(mock.data.transport_fields.bpm, 60)
      ok(not exists(job.sources[1].file))
    end)
    os.time = wall_time
    mock.impl(mock.data.song).rendering = false
    ok(fine, tostring(problem))
    H.no_dialogs()
  end
end)

test("listening: standalone still renders fractional clipped ranges and stops existing playback", function()
  local env, peer = setup()
  mock.data.transport_fields.playing = true
  local answer = result((start(peer, "jpf5", { { id = "mix", what = "master" } },
    { fromBeat = 10.125, beats = 3 }, { preRollBeats = 1 })))
  eq(answer.seconds, 1.5)
  eq(mock.renders[1].was_playing, true)
  mock.finish_render()
  eq(status(peer, "jpf5").state, "done")
  eq(mock.data.transport_fields.playing, false)
  H.no_dialogs()
end)

test("listening: read spans says where each track plays, in beats: muted columns and slots left out, aliases followed, merged", function()
  local env, peer = setup({}, { patterns = 3, sequence_length = 4, sequence = { 1, 2, 3, 1 } })
  -- Kick: a note in patterns 1 and 3 (slots 1, 3, 4); slot 4 muted for it
  mock.ops.set_note(1, 1, 1, 1, { note = 48, instrument = 0 })
  mock.ops.set_note(3, 1, 5, 1, { note = 48, instrument = 0 })
  mock.ops.mute_slot(1, 4, true)
  -- Hat: in pattern 2 only a note in its muted column 2, and an effect in pattern 3
  mock.ops.mute_column(2, 2, true)
  mock.ops.set_note(2, 2, 1, 2, { note = 50, instrument = 0 })
  mock.ops.set_effect(3, 2, 9, 1, 0x0A, 0x20)
  -- Bass: pattern 2 is an alias of pattern 1's lines, which hold a note
  mock.ops.set_note(1, 3, 3, 1, { note = 36, instrument = 0 })
  mock.ops.alias(2, 3, 1)
  local epoch = place(peer)
  local page = result(call(peer, "read", { kind = "spans" }), "spans")
  eq(page.epoch, epoch)
  local by = {}
  for _, item in ipairs(page.items) do by[item.name] = item end
  eq(json.encode(by.Kick.spans), "[[0,4],[8,12]]")
  eq(by.Kick.ref, epoch .. ":track:1")
  eq(json.encode(by.Hat.spans), "[[8,12]]")
  eq(json.encode(by.Bass.spans), "[[0,8],[12,16]]", "slots 1, 2 (the alias) and 4, merged where they touch")
  eq(json.encode(by.Pad.spans), "[]")
  eq(#page.items, 4, "the tracks that play parts: not the master or the send")
  eq(by.Kick.mute, nil)
  eq(page.nextCursor, nil)
  local first = result(call(peer, "read", { kind = "spans", limit = 2 }))
  eq(#first.items, 2)
  ok(first.nextCursor, "a next page")
  local second = result(call(peer, "read", { kind = "spans", limit = 2, cursor = first.nextCursor }))
  eq(second.items[1].name, "Bass")
  -- a track not heard now says so (muted, off, or left out by a solo); Kumi's own scratch copies aren't parts
  track(2).mute_state = MUTED
  mock.ops.insert_track(5, { name = "Kumi \194\183 try ab12 1" })
  local again = result(call(peer, "read", { kind = "spans" }))
  local names = {}
  for _, item in ipairs(again.items) do names[#names + 1] = item.name .. (item.mute and "(muted)" or "") end
  eq(table.concat(names, ","), "Kick,Hat(muted),Bass,Pad")
  H.no_dialogs()
end)

-- The song's tempo commands are read a slice at a time (every look at the clock costs 100 ms here, so a slice is a few tracks).
local function cold(sequence)
  local env, peer = setup(nil, { patterns = #sequence, sequence_length = #sequence, sequence = sequence })
  local function command(text)
    local digits = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ"
    return 256 + (digits:find(text:sub(1, 1), 1, true) - 1) * 36 + (digits:find(text:sub(2, 2), 1, true) - 1)
  end
  -- (ZL08 on pattern 9's line 1: its 16 lines are 2 beats, and slot 10 starts at beat 34, at LPB 8)
  mock.ops.set_effect(9, 2, 1, 1, command("ZL"), 8)
  -- (as after a song opens: nothing read yet, and the tool's ticks not reading ahead until told)
  bridge().listen.zcache = {}
  bridge().listen.warmed_ms = math.huge
  mock.clock_step_ms = 100
  return env, peer
end
local twelve = { 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12 }

test("listening: a render waits for the song's tempo commands: answered busy past a slice, read on between, then it renders", function()
  local env, peer = cold(twelve)
  local before = states()
  failure((start(peer, "jt1", { { id = "mix", what = "master" } }, { fromBeat = 34, beats = 1 })), "busy", "tempo commands")
  eq(#mock.renders, 0, "nothing rendered")
  eq(states(), before, "nothing written")
  -- (the tool's ticks read the rest ahead, a little at a time)
  bridge().listen.warmed_ms = nil
  mock.clock_step_ms = 5
  for _ = 1, 100 do H.tick(1) end
  eq(bridge().listen.warm_wanted, false, "all read")
  local answer = result((start(peer, "jt1", { { id = "mix", what = "master" } }, { fromBeat = 34, beats = 1 })), "render_start")
  mock.clock_step_ms = 0
  local r = mock.renders[1]
  eq(r.options.start_pos.sequence, 10, "beat 34 is slot 10 (pattern 9 is 2 beats at LPB 8)")
  eq(r.options.end_pos.line, 8, "a beat at LPB 8 is 8 lines")
  ok(math.abs(answer.seconds - 0.5) < 1e-9, answer.seconds)
  mock.finish_render()
  H.no_dialogs()
end)

test("listening: a sequence read reads the song's tempo commands across its turns, and says where each slot plays", function()
  local env, peer = cold(twelve)
  local page = result(call(peer, "read", { kind = "sequence" }, 4000), "sequence")
  mock.clock_step_ms = 0
  eq(page.items[9].positionBeats, 32)
  eq(page.items[9].lengthBeats, 2)
  eq(page.items[10].positionBeats, 34)
  H.no_dialogs()
end)

test("listening: sequence rows say where each slot plays in beats, its ZL changes included (contract §9)", function()
  local env, peer = setup()
  local function command(text)
    local digits = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ"
    return 256 + (digits:find(text:sub(1, 1), 1, true) - 1) * 36 + (digits:find(text:sub(2, 2), 1, true) - 1)
  end
  -- (ZL08 on slot 2's line 9: its first 8 lines are 2 beats, the other 8 one beat; slot 3 stays at LPB 8, 2 beats)
  mock.ops.set_effect(2, 4, 9, 1, command("ZL"), 8)
  local epoch = place(peer)
  local page = result(call(peer, "read", { kind = "sequence" }), "sequence")
  eq(page.items[1].positionBeats, 0)
  eq(page.items[1].lengthBeats, 4)
  eq(page.items[2].positionBeats, 4)
  eq(page.items[2].lengthBeats, 3)
  eq(page.items[3].positionBeats, 7)
  eq(page.items[3].lengthBeats, 2)
  local one = result(call(peer, "read", { ref = epoch .. ":seq:3" }))
  eq(one.items[1].positionBeats, 7)
  eq(one.items[1].lengthBeats, 2)
  H.no_dialogs()
end)

test("listening: read param with grid answers only the current point, gridShort (a set and put back is an undo step in Renoise)", function()
  local env, peer = setup()
  local epoch = place(peer)
  local page = result(call(peer, "read", { kind = "param", parent = epoch .. ":device:1.1", grid = 49 }), "param grid")
  ok(#page.items > 0)
  for _, row in ipairs(page.items) do
    eq(row.gridShort, true)
    eq(#row.grid, 1)
    eq(row.grid[1][1], row.value)
    eq(row.grid[1][2], row.valueString)
  end
  local one = result(call(peer, "read", { ref = epoch .. ":param:1.1.2", grid = 49 }))
  eq(one.items[1].gridShort, true)
  eq(one.items[1].steps, nil, "a continuous parameter has no steps")
  -- a stepped parameter says how many values it has (a switch: 2), read without writing
  mock.ops.insert_device(1, { name = "Filter", path = "Audio/Effects/Native/Filter", params = {
    { name = "Type", value = 3, min = 0, max = 19, quantum = 1 }, { name = "Cutoff", value = 127, min = 0, max = 127 },
    { name = "Line Sync", value = 0, min = 0, max = 1, quantum = 1 } } })
  epoch = place(peer)
  local filter = result(call(peer, "read", { kind = "param", parent = epoch .. ":device:1.2", grid = 2 }), "stepped grid")
  eq(filter.items[1].steps, 20)
  eq(filter.items[2].steps, nil)
  eq(filter.items[3].steps, 2)
  failure(call(peer, "read", { kind = "param", parent = epoch .. ":device:1.1", grid = 1 }), "bad-params")
  failure(call(peer, "read", { kind = "track", grid = 49 }), "bad-params", "grid is an option of kind param")
  eq(#mock.data.undo.stack, 0, "nothing written")
  H.no_dialogs()
end)
