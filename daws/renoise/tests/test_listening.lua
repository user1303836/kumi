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
