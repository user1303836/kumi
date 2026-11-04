-- M4 (n-song): Kumi's changes to the order of the pattern sequence (cloning slots, moving a slot, making slots' patterns unique):
-- preview, apply, restore, the slots followed through what the producer made or deleted since, Renoise's undo told.
local T = ...
local mock = T.mock
local test, eq, ok = T.test, T.eq, T.ok
local H = T.harness
local K = assert(loadfile(T.tests_dir .. "change_kit.lua"))(T)

local function seq() return table.concat(mock.data.sequence, ",") end
local function names()
  local out = {}
  for p, pattern in ipairs(mock.data.patterns) do out[p] = pattern.name end
  return table.concat(out, ",")
end
local function sections()
  local out = {}
  for s = 1, #mock.data.sequence do
    if mock.data.sections[s] then out[#out + 1] = s .. ":" .. mock.data.sections[s] end
  end
  return table.concat(out, " ")
end
local function mutes()
  local out = {}
  for key in pairs(mock.data.muted) do out[#out + 1] = key end
  table.sort(out)
  return table.concat(out, " ")
end

-- Patterns A, B, C with a line each; the sequence A B A C; slot 2 starts the section "Break" and mutes track 1.
local function setup()
  local env, peer = K.setup()
  for p, name in ipairs({ "A", "B", "C" }) do mock.data.patterns[p].name = name end
  for p = 1, 3 do mock.ops.set_note(p, 1, 1, 1, { note = 47 + p }) end
  mock.ops.section(2, "Break")
  mock.ops.mute_slot(1, 2, true)
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  K.told(peer)
  return env, peer
end

test("sequence: after Renoise's own undo and redo of a clone or of patterns made unique, Kumi's undo takes the copies away; a copy edited since stays", function()
  for _, case in ipairs({
    { "slot-clone", function(E) return { ref = K.ref(E, "seq", 2) } end, "1,2,4,1,3", 4 },
    { "slot-unique", function(E) return { ref = K.ref(E, "seq", 1), last = K.ref(E, "seq", 3) } end, "4,2,5,3", 4 },
  }) do
    for _, edited in ipairs({ false, true }) do
      local _, peer = setup()
      local desc = "Kumi: " .. case[1] .. " (kq01)"
      local record = K.change(peer, case[1], case[2](K.place(peer)), desc)
      local after = seq()
      ok(K.told(peer) == "", "nothing told yet")
      mock.song:undo()
      eq(seq(), "1,2,1,3", case[1] .. ": Renoise's undo")
      ok(K.told(peer):find("undo:" .. desc, 1, true), case[1] .. ": the undo told")
      mock.song:redo()
      eq(seq(), after, case[1] .. ": Renoise's redo")
      ok(K.told(peer):find("redo:" .. desc, 1, true), case[1] .. ": the redo told")
      if edited then mock.ops.set_note(case[4], 2, 5, 1, { note = 60 }) end
      local undone = K.restore(peer, record, nil, { of = desc })
      if edited then
        K.failure(undone, "changed")
      else
        ok(undone.result, case[1] .. ": what Renoise's redo made again: " .. T.load_payload_module("json").encode(undone.error or {}))
        eq(seq(), "1,2,1,3", case[1])
      end
    end
  end
end)

test("sequence: a cloned slot gets a copy of its pattern, its mutes and section; Kumi's undo takes both away", function()
  local _, peer = setup()
  local E = K.place(peer)
  local record = K.change(peer, "slot-clone", { ref = K.ref(E, "seq", 2) }, "Kumi: clone (ks01)")
  eq(record.result.display.title, "Cloned slot 2 (pattern \226\128\156B\226\128\157)")
  eq(seq(), "1,2,4,1,3")
  eq(names(), "A,B,C,#B")
  eq(sections(), "2:Break 3:#Break")
  eq(mutes(), "1,2 1,3")
  eq(K.steps(), 1)
  eq(record.target.ref, K.ref(record.epoch, "song", 1))
  eq(record.created.ref, K.ref(record.epoch, "seq", 3))
  local undone = K.restore(peer, record)
  ok(undone.result, "restored")
  eq(seq(), "1,2,1,3")
  eq(names(), "A,B,C", "the copy is gone from the pool")
  eq(sections(), "2:Break")
  eq(mutes(), "1,2")
  eq(K.steps(), 2)
  -- back already
  eq(K.restore(peer, record).result.display.note, "It was back already: nothing was written.")
  H.no_dialogs()
end)

test("sequence: a range of slots clones after its last, and is followed through a slot the producer made before it", function()
  local _, peer = setup()
  local E = K.place(peer)
  local record = K.change(peer, "slot-clone", { ref = K.ref(E, "seq", 1), last = K.ref(E, "seq", 2) })
  eq(seq(), "1,2,4,5,1,3")
  eq(names(), "A,B,C,#A,#B")
  -- the producer puts pattern C at the front
  mock.run_callback(function() mock.song.sequencer:insert_sequence_at(1, 3) end)
  eq(seq(), "3,1,2,4,5,1,3")
  local undone = K.restore(peer, record)
  ok(undone.result, "followed: " .. tostring(undone.error and undone.error.message))
  eq(seq(), "3,1,2,1,3")
  eq(names(), "A,B,C")
  H.no_dialogs()
end)

test("sequence: the clone's undo leaves what the producer changed since: a copy edited, a cloned slot's mutes", function()
  local _, peer = setup()
  local E = K.place(peer)
  local record = K.change(peer, "slot-clone", { ref = K.ref(E, "seq", 3) })
  eq(seq(), "1,2,1,4,3")
  mock.ops.set_note(4, 2, 5, 1, { note = 60 }) -- the producer writes in the copy
  local message = K.failure(K.restore(peer, record), "changed")
  ok(message:find("its lines changed", 1, true), message)
  ok(K.failure(K.restore(peer, record, nil, { discard = true }), "changed"), "discard doesn't pass over the producer's lines")
  eq(seq(), "1,2,1,4,3")
  local other = K.change(peer, "slot-clone", { ref = K.ref(K.place(peer), "seq", 1) })
  eq(seq(), "1,5,2,1,4,3")
  mock.ops.mute_slot(3, 2, true)
  K.failure(K.restore(peer, other), "changed")
  ok(K.restore(peer, other, nil, { discard = true }).result, "discard passes over the slot's mutes")
  eq(seq(), "1,2,1,4,3")
  H.no_dialogs()
end)

test("sequence: a copy another pattern's track plays as an alias stays when Kumi's undo takes its slot away, and the answer says so (#361 N-S13)", function()
  local _, peer = setup()
  local E = K.place(peer)
  local record = K.change(peer, "slot-clone", { ref = K.ref(E, "seq", 4) })
  eq(seq(), "1,2,1,3,4")
  mock.ops.alias(3, 1, 4) -- the producer: pattern 3's track 1 plays the copy
  H.tick(2)
  local undone = K.restore(peer, record)
  ok(undone.result, T.load_payload_module("json").encode(undone.error or {}))
  eq(seq(), "1,2,1,3")
  eq(#mock.data.patterns, 4, "the copy stays in the pool")
  eq(mock.song:pattern(3):track(1).alias_pattern_index, 4, "and pattern 3's track still plays it")
  ok((undone.result.display.note or ""):find("as an alias", 1, true), tostring(undone.result.display.note))
  H.no_dialogs()
end)

test("sequence: a copy that patterns made later follow stays in the pool when its slot goes, and the answer says so", function()
  local _, peer = setup()
  local E = K.place(peer)
  local record = K.change(peer, "slot-clone", { ref = K.ref(E, "seq", 4) })
  eq(seq(), "1,2,1,3,4")
  mock.ops.insert_pattern() -- the producer's, after the copy in the pool
  local undone = K.restore(peer, record)
  ok(undone.result)
  eq(seq(), "1,2,1,3")
  eq(#mock.data.patterns, 5)
  eq(undone.result.display.note, "The pattern Renoise made for it stays in the pool, unused: patterns were made after it.")
  H.no_dialogs()
end)

test("sequence: Renoise's undo and redo of a clone are told", function()
  local _, peer = setup()
  local E = K.place(peer)
  K.change(peer, "slot-clone", { ref = K.ref(E, "seq", 1) }, "Kumi: clone (ks21)")
  K.told(peer)
  mock.song:undo()
  eq(seq(), "1,2,1,3")
  eq(K.told(peer), "undo:Kumi: clone (ks21)")
  mock.song:redo()
  eq(seq(), "1,4,2,1,3")
  eq(K.told(peer), "redo:Kumi: clone (ks21)")
  H.no_dialogs()
end)

test("sequence: a slot moves with its section and mutes, and goes back to the gap it left", function()
  local _, peer = setup()
  local E = K.place(peer)
  local record = K.change(peer, "slot-move", { ref = K.ref(E, "seq", 2), index = 4 }, "Kumi: move (ks31)")
  eq(record.result.display.title, "Moved slot 2 (pattern \226\128\156B\226\128\157) to place 4")
  eq(seq(), "1,1,3,2")
  eq(sections(), "4:Break", "the section went with the slot, and didn't pass to the next one")
  eq(mutes(), "1,4")
  eq(K.steps(), 1)
  local undone = K.restore(peer, record)
  ok(undone.result)
  eq(undone.result.display.title, "Moved slot 4 back to place 2")
  eq(seq(), "1,2,1,3")
  eq(sections(), "2:Break")
  eq(mutes(), "1,2")
  H.no_dialogs()
end)

test("sequence: a slot moved between slots that play the same pattern goes back with its section and mutes, not \"back already\" (#361 N-S6)", function()
  local _, peer = K.setup(nil, { sequence = { 1, 1, 2, 3 }, sequence_length = 4 })
  local sequencer = mock.song.sequencer
  mock.run_callback(function()
    sequencer:set_sequence_is_start_of_section(1, true)
    sequencer:set_sequence_section_name(1, "Intro")
    sequencer:set_track_sequence_slot_is_muted(2, 1, true)
  end)
  mock.data.undo.stack = {}
  local E = K.place(peer)
  local record = K.change(peer, "slot-move", { ref = K.ref(E, "seq", 1), index = 2 }, "Kumi: move (ks61)")
  eq(seq(), "1,1,2,3", "the list of patterns is the same")
  ok(sequencer:sequence_is_start_of_section(2) and sequencer:track_sequence_slot_is_muted(2, 2), "the section and mute went with it")
  local undone = K.restore(peer, record)
  ok(undone.result, T.load_payload_module("json").encode(undone.error or {}))
  eq(undone.result.undo.entry, true, "a move back, not back already")
  ok(sequencer:sequence_is_start_of_section(1) and sequencer:sequence_section_name(1) == "Intro", "the section is on slot 1 again")
  ok(sequencer:track_sequence_slot_is_muted(2, 1) and not sequencer:track_sequence_slot_is_muted(2, 2), "and the mute")
  H.no_dialogs()
end)

test("sequence: a moved slot is followed through what the producer made or deleted since, and a sequence that can't tell is stale", function()
  local _, peer = setup()
  local E = K.place(peer)
  local record = K.change(peer, "slot-move", { ref = K.ref(E, "seq", 4), index = 1 })
  eq(seq(), "3,1,2,1")
  mock.run_callback(function() mock.song.sequencer:insert_sequence_at(1, 2) end) -- the producer, at the front
  eq(seq(), "2,3,1,2,1")
  ok(K.restore(peer, record).result, "followed")
  eq(seq(), "2,1,2,1,3")
  -- A, B, A, B: a move, then a slot of A made where it could be either side of the moved one: Kumi can't tell
  local _, peer2 = K.setup(nil, { sequence = { 1, 2, 1, 2 }, sequence_length = 4 })
  local E2 = K.place(peer2)
  local again = K.change(peer2, "slot-move", { ref = K.ref(E2, "seq", 1), index = 3 })
  eq(seq(), "2,1,1,2")
  mock.run_callback(function() mock.song.sequencer:insert_sequence_at(2, 1) end)
  eq(seq(), "2,1,1,1,2")
  K.failure(K.restore(peer2, again), "stale")
  eq(seq(), "2,1,1,1,2", "nothing touched")
  H.no_dialogs()
end)

test("sequence: unique patterns for repeated slots, and Kumi's undo puts the shared ones back and takes the copies away", function()
  local _, peer = setup()
  local E = K.place(peer)
  local record = K.change(peer, "slot-unique", { ref = K.ref(E, "seq", 1), last = K.ref(E, "seq", 4) }, "Kumi: unique (ks41)")
  eq(seq(), "4,2,5,3")
  eq(names(), "A,B,C,#A,#A")
  eq(K.steps(), 1)
  local undone = K.restore(peer, record)
  ok(undone.result)
  eq(seq(), "1,2,1,3")
  eq(names(), "A,B,C")
  -- nothing to make unique
  local none = K.preview(peer, "slot-unique", { ref = K.ref(K.place(peer), "seq", 2) })
  eq(none.display.unchanged, true)
  -- a copy edited since: refused
  local again = K.change(peer, "slot-unique", { ref = K.ref(K.place(peer), "seq", 3) })
  eq(seq(), "1,2,4,3")
  mock.data.patterns[4].name = "Mine"
  K.failure(K.restore(peer, again), "changed")
  H.no_dialogs()
end)

test("sequence: an apply after the producer changed the sequence is refused, nothing written (the fence)", function()
  local _, peer = setup()
  local E = K.place(peer)
  for _, case in ipairs({ { "slot-clone", { ref = K.ref(E, "seq", 2) } }, { "slot-move", { ref = K.ref(E, "seq", 2), index = 1 } },
    { "slot-unique", { ref = K.ref(E, "seq", 1) } } }) do
    local previewed, epoch, layout = K.preview(peer, case[1], case[2])
    mock.data.sequence[2] = 3 -- the producer, without a notifier: the epoch stays
    local refused = K.apply(peer, case[1], case[2], "Kumi: x (ks51)", previewed.fence, epoch, layout)
    K.failure(refused, "changed")
    eq(seq(), "1,3,1,3", case[1] .. ": nothing written")
    mock.data.sequence[2] = 2
  end
  eq(K.steps(), 0)
  H.no_dialogs()
end)

test("sequence: bad slots are bad-params", function()
  local _, peer = setup()
  local E = K.place(peer)
  local function bad(kind, input) K.failure(K.ask(peer, "change_preview", kind, input), "bad-params") end
  bad("slot-clone", { ref = K.ref(E, "seq", 3), last = K.ref(E, "seq", 2) })
  bad("slot-clone", { ref = K.ref(E, "pattern", 1) })
  bad("slot-move", { ref = K.ref(E, "seq", 1), index = 5 })
  bad("slot-move", { ref = K.ref(E, "seq", 1) })
  bad("slot-unique", { ref = K.ref(E, "seq", 2), last = K.ref(E, "seq", 1) })
  H.no_dialogs()
end)

test("sequence: a clone's record follows the pool's patterns through one deleted before them (renumbered)", function()
  local _, peer = K.setup(nil, { patterns = 4, sequence = { 1, 3, 1, 4 }, sequence_length = 4 })
  local E = K.place(peer)
  local record = K.change(peer, "slot-clone", { ref = K.ref(E, "seq", 2) })
  eq(seq(), "1,3,5,1,4")
  -- the producer deletes the unused pattern 2: the patterns after it are one less, in the sequence too
  table.remove(mock.data.patterns, 2)
  for s, p in ipairs(mock.data.sequence) do
    if p > 2 then mock.data.sequence[s] = p - 1 end
  end
  mock.fire(mock.data.song_fields.patterns_observable, { type = "remove", index = 2 })
  eq(seq(), "1,2,4,1,3")
  local undone = K.restore(peer, record)
  ok(undone.result, "followed: " .. tostring(undone.error and undone.error.message))
  eq(seq(), "1,2,1,3")
  eq(#mock.data.patterns, 3, "the copy is gone")
  H.no_dialogs()
end)

test("sequence: Renoise's undo and redo of a clone of two slots (two patterns made) are both told", function()
  local _, peer = setup()
  local E = K.place(peer)
  K.change(peer, "slot-clone", { ref = K.ref(E, "seq", 1), last = K.ref(E, "seq", 2) }, "Kumi: clone two (ks61)")
  eq(seq(), "1,2,4,5,1,3")
  K.told(peer)
  mock.song:undo()
  eq(K.told(peer), "undo:Kumi: clone two (ks61)")
  mock.song:redo()
  eq(seq(), "1,2,4,5,1,3")
  eq(K.told(peer), "redo:Kumi: clone two (ks61)")
  mock.song:undo()
  eq(K.told(peer), "undo:Kumi: clone two (ks61)")
  H.no_dialogs()
end)

test("sequence: the pool keeps a pattern another pattern's track aliases, even the last and empty, as Renoise does (lab 2026-11-01)", function()
  setup()
  local song = mock.song
  local sequencer = song.sequencer
  sequencer:insert_new_pattern_at(#sequencer.pattern_sequence + 1)
  local a = sequencer.pattern_sequence[#sequencer.pattern_sequence]
  sequencer:insert_new_pattern_at(#sequencer.pattern_sequence + 1)
  local b = sequencer.pattern_sequence[#sequencer.pattern_sequence]
  song:pattern(a):track(1).alias_pattern_index = b
  local patterns = #song.patterns
  sequencer:delete_sequence_at(#sequencer.pattern_sequence)
  eq(#song.patterns, patterns, "kept")
  eq(song:pattern(a):track(1).alias_pattern_index, b)
end)
