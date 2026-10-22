-- M7 (variations): what the variations loop needs from the tool: a phrase's lines read, a phrase made with its lines in one step
-- (phrase-add's content), a slot playing a variation of a pattern (slot-variation), and several takes dropped in one step
-- (takes-drop); each applied, put back by Kumi's undo, refused when the producer changed it, gone, back already.
local T = ...
local mock = T.mock
local json = T.load_payload_module("json")
local test, eq, ok = T.test, T.eq, T.ok
local I = T.instruments
local setup, place, preview, change, restore, failure, call = I.setup, I.place, I.preview, I.change, I.restore, I.failure, I.call
local steps, newest, instrument, iref = I.steps, I.newest, I.instrument, I.iref

local function pref(epoch, i, p) return epoch .. ":phrase:" .. i .. "." .. p end
local function phrase(i, p) return mock.song:instrument(i):phrase(p) end

-- A chop's lines, as phrase-write takes them: kick, ghost with a 0S offset, snare reversed, a roll.
local CHOP = {
  { line = 0, notes = { { col = 1, note = "C-4", instrument = 0 } } },
  { line = 3, notes = { { col = 1, note = "C-4", volume = "20", fx = "0S40" } } },
  { line = 4, notes = { { col = 1, note = "D-4" } }, effects = { { col = 1, number = "0B", amount = 0 } } },
  { line = 6, notes = { { col = 1, note = "D-4", delay = "80" } }, effects = { { col = 1, number = "0R", amount = "03" } } },
}

-- read {kind: line, parent: phrase}

test("variations: a phrase's lines read as a pattern track's are, in Renoise's text, numbered from 00", function()
  local env, peer = setup()
  local p = phrase(2, 1)
  p.number_of_lines = 8
  p:line(1):note_column(1).note_string = "C-4"
  p:line(4):note_column(1).note_string = "E-4"
  p:line(4):effect_column(1).number_string = "0S"
  p:line(4):effect_column(1).amount_string = "40"
  local epoch = place(peer)
  local read = call(peer, "read", { kind = "line", parent = pref(epoch, 2, 1) })
  ok(read.result, json.encode(read.error or {}))
  eq(#read.result.items, 2)
  eq(read.result.items[1].line, 0)
  eq(read.result.items[1].notes[1].note, "C-4")
  eq(read.result.items[2].line, 3)
  eq(read.result.items[2].effects[1].number .. read.result.items[2].effects[1].amount, "0S40")
  eq(read.result.lines, 8)
  failure(call(peer, "read", { kind = "line", parent = pref(epoch, 2, 5) }), "bad-params")
end)

-- phrase-add with content

test("variations: a phrase made with its lines is one step, the columns its lines need shown; Kumi's undo removes it", function()
  local env, peer = setup()
  local before = steps()
  local record, done = change(peer, "phrase-add", { ref = iref(place(peer), 2), name = "V1 \194\183 Chop", lines = 8, lpb = 4, content = CHOP }, "Kumi: V1 (k1)")
  eq(steps(), before + 1, "one step of Renoise's undo")
  eq(newest(), "Kumi: V1 (k1)")
  ok(done.display.title:find("with 4 lines", 1, true), done.display.title)
  local made = phrase(2, 2)
  eq(made.name, "V1 \194\183 Chop")
  eq(made.number_of_lines, 8)
  eq(made:line(4):note_column(1).effect_number_string .. made:line(4):note_column(1).effect_amount_string, "0S40")
  eq(made:line(4):note_column(1).volume_string, "20")
  eq(made:line(5):effect_column(1).number_string, "0B")
  eq(made:line(7):note_column(1).delay_string, "80")
  ok(made.volume_column_visible and made.delay_column_visible and made.sample_effects_column_visible, "the columns its lines use are shown")
  ok(made.visible_effect_columns >= 1, "an effect column shown")
  local undone = restore(peer, "phrase-add", record)
  ok(undone.result, json.encode(undone.error or {}))
  eq(#instrument(2).phrases, 1)
end)

test("variations: a take the producer edited isn't removed by Kumi's undo; one gone is gone", function()
  local env, peer = setup()
  local record = change(peer, "phrase-add", { ref = iref(place(peer), 2), lines = 8, content = CHOP })
  phrase(2, 2):line(4):note_column(1).volume_string = "40"
  failure(restore(peer, "phrase-add", record), "changed")
  eq(#instrument(2).phrases, 2)
  local again = change(peer, "phrase-add", { ref = iref(place(peer), 2), lines = 8, content = CHOP })
  mock.song:instrument(2):delete_phrase_at(3)
  failure(restore(peer, "phrase-add", again), "gone")
end)

test("variations: content past the phrase's lines, or a cell Renoise can't hold, is refused before anything is made", function()
  local env, peer = setup()
  local epoch = place(peer)
  failure(call(peer, "change_preview", { kind = "phrase-add", input = { ref = iref(epoch, 2), lines = 4, content = CHOP }, epoch = epoch }), "bad-params")
  failure(call(peer, "change_preview", { kind = "phrase-add", input = { ref = iref(epoch, 2), content = { { line = 0, notes = { { col = 1, note = "H-4" } } } } }, epoch = epoch }), "bad-params")
  eq(#instrument(2).phrases, 1)
end)

-- slot-variation and slots-drop (the sequence: patterns A, B, C in slots 1,2,1,3; slot 2 starts "Break" and mutes track 1)

local K = assert(loadfile(T.tests_dir .. "change_kit.lua"))(T)

local function seq() return table.concat(mock.data.sequence, ",") end
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
local function seq_setup()
  local env, peer = K.setup()
  for p, name in ipairs({ "A", "B", "C" }) do mock.data.patterns[p].name = name end
  -- (the break on track 1 of B: kick, snare, ghost; a bassline on track 3)
  mock.ops.set_note(2, 1, 1, 1, { note = 48 })
  mock.ops.set_note(2, 1, 5, 1, { note = 50 })
  mock.ops.set_note(2, 1, 8, 1, { note = 48, volume = 0x20 })
  mock.ops.set_note(2, 3, 1, 1, { note = 36 })
  mock.ops.set_note(1, 1, 1, 1, { note = 48 })
  mock.ops.set_note(3, 1, 1, 1, { note = 48 })
  mock.ops.section(2, "Break")
  mock.ops.mute_slot(4, 2, true)
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  K.told(peer)
  return env, peer
end
local function cell(p, t, k) return mock.song:pattern(p):track(t):line(k):note_column(1) end

-- The varied lines of a take of B's break: kick kept, the snare moved and reversed, a roll.
local TAKE = {
  { line = 0, notes = { { col = 1, note = "C-4" } } },
  { line = 6, notes = { { col = 1, note = "D-4" } }, effects = { { col = 1, number = "0B", amount = 0 } } },
  { line = 7, notes = { { col = 1, note = "D-4", fx = "0R03" } } },
}

local function variation_input(E, extra)
  local input = { pattern = K.ref(E, "seq", 2), track = K.ref(E, "track", 1), first = 0, last = 7, content = TAKE, name = "V1 \194\183 B", section = "Variations of B" }
  for k, v in pairs(extra or {}) do input[k] = v end
  if input.section == false then input.section = nil end
  return input
end

test("variations: a variation slot is one step: a new pattern at the end, its other tracks B's aliases, the range varied, B's mutes", function()
  local _, peer = seq_setup()
  local effect_columns = mock.song:track(1).visible_effect_columns
  local record = K.change(peer, "slot-variation", variation_input((K.place(peer))), "Kumi: V1 (kv01)")
  eq(K.steps(), 1)
  eq(K.newest(), "Kumi: V1 (kv01)")
  eq(seq(), "1,2,1,3,4")
  eq(mock.data.patterns[4].name, "V1 \194\183 B")
  eq(sections(), "2:Break 5:Variations of B")
  eq(mutes(), "4,2 4,5", "the slot mutes what B's slot mutes")
  eq(mock.song:pattern(4):track(3).alias_pattern_index, 2, "the bass plays B's lines")
  eq(mock.song:pattern(4):track(1).alias_pattern_index, 0, "the varied track is its own")
  eq(cell(4, 1, 1).note_string, "C-4")
  eq(cell(4, 1, 5).note_string, "---", "the snare moved")
  eq(cell(4, 1, 7).note_string, "D-4")
  eq(mock.song:pattern(4):track(1):line(7):effect_column(1).number_string, "0B")
  eq(cell(4, 1, 8).effect_number_string .. cell(4, 1, 8).effect_amount_string, "0R03")
  eq(cell(2, 1, 5).note_string, "D-4", "B itself is as it was")
  ok(record.result.display.title:find("variation of \226\128\156B\226\128\157", 1, true), record.result.display.title)
  ok(mock.song:track(1).sample_effects_column_visible, "the sample effects column it needs is shown")
  ok(record.created.place and record.created.place.n == 4, "the pattern's place, to find it by")
  local undone = K.restore(peer, record)
  ok(undone.result, json.encode(undone.error or {}))
  eq(seq(), "1,2,1,3")
  eq(#mock.data.patterns, 3, "its pattern went with it")
  eq(sections(), "2:Break")
  ok(not mock.song:track(1).sample_effects_column_visible, "the column Kumi showed is hidden again")
  eq(mock.song:track(1).visible_effect_columns, effect_columns)
end)

test("variations: where tells a variation pattern's own tracks: the varied one, and another once the producer writes there", function()
  local _, peer = seq_setup()
  local record = K.change(peer, "slot-variation", variation_input((K.place(peer))), "Kumi: V1 (kv01)")
  local function own()
    local where = K.call(peer, "read", { kind = "where", items = { { pattern = record.created.place } } })
    ok(where.result, json.encode(where.error or {}))
    return table.concat(where.result.items[1].ownTracks, ",")
  end
  eq(own(), "1", "only the varied track holds lines of its own")
  mock.song:pattern(4):track(3).alias_pattern_index = 0
  mock.ops.set_note(4, 3, 2, 1, { note = 36, instrument = 0 })
  eq(own(), "1,3", "the bass the producer wrote")
end)

test("variations: a variation Renoise refused part of goes again: its slot, and a free pattern of the pool it took gets its name and length back", function()
  seq_setup()
  local S = T.load_payload_module("changes_sequence")
  local song = mock.song
  local function sequence() return table.concat(song.sequencer.pattern_sequence, ",") end
  -- (a free pattern in the pool: made with a slot and a note, the slot deleted, then emptied)
  song.sequencer:insert_new_pattern_at(5)
  local free = song.sequencer.pattern_sequence[5]
  mock.ops.set_note(free, 1, 1, 1, { note = 48, instrument = 0 })
  song.sequencer:delete_sequence_at(5)
  song:pattern(free):clear()
  song:pattern(free).name, song:pattern(free).number_of_lines = "Old", 32
  local before = {}
  for s, p in ipairs(song.sequencer.pattern_sequence) do before[s] = p end
  -- Renoise gave the variation's slot that free pattern, and the variation renamed and lengthened it before failing
  song.sequencer:insert_sequence_at(5, free)
  song:pattern(free).name, song:pattern(free).number_of_lines = "V1 \194\183 B", 64
  mock.ops.set_note(free, 1, 1, 1, { note = 50, instrument = 0 })
  S.unmake_variation(song, { index = 5, made = { p = free, reused = { name = "Old", lines = 32 } } }, before)
  eq(sequence(), table.concat(before, ","), "the slot went")
  eq(song:pattern(free).name, "Old")
  eq(song:pattern(free).number_of_lines, 32)
  ok(song:pattern(free).is_empty, "the variation's lines went")
  -- (a slot not as the variation left it is left alone)
  song.sequencer:insert_sequence_at(5, 1)
  S.unmake_variation(song, { index = 5, made = { p = free } }, before)
  eq(#song.sequencer.pattern_sequence, #before + 1, "another slot stays")
end)

test("variations: Kumi's undo of a variation refuses once the producer renamed the section its slot starts; with discard it goes", function()
  local _, peer = seq_setup()
  local record = K.change(peer, "slot-variation", variation_input((K.place(peer))), "Kumi: V1 (kv01)")
  eq(sections(), "2:Break 5:Variations of B")
  mock.song.sequencer:set_sequence_section_name(5, "Fills")
  local refused = K.restore(peer, record)
  K.failure(refused, "changed")
  ok(refused.error.message:find("Fills", 1, true), refused.error.message)
  eq(seq(), "1,2,1,3,4", "the slot stays")
  local undone = K.restore(peer, record, nil, { discard = true })
  ok(undone.result, json.encode(undone.error or {}))
  eq(seq(), "1,2,1,3")
end)

test("variations: Kumi's undo of a variation follows it past a slot made before it, refuses one edited since, and is unwritten once Renoise's undo took it", function()
  local _, peer = seq_setup()
  local record = K.change(peer, "slot-variation", variation_input((K.place(peer))))
  mock.song.sequencer:insert_sequence_at(1, 3)
  eq(seq(), "3,1,2,1,3,4")
  local undone = K.restore(peer, record)
  ok(undone.result, json.encode(undone.error or {}))
  eq(seq(), "3,1,2,1,3")
  -- (edited since: the varied track's lines)
  local _, peer2 = seq_setup()
  local again = K.change(peer2, "slot-variation", variation_input((K.place(peer2))))
  mock.ops.set_note(4, 1, 3, 1, { note = 55 })
  K.failure(K.restore(peer2, again), "changed")
  eq(seq(), "1,2,1,3,4")
  -- (Renoise's undo took it: nothing written)
  local _, peer3 = seq_setup()
  local third = K.change(peer3, "slot-variation", variation_input((K.place(peer3))))
  mock.song:undo()
  eq(seq(), "1,2,1,3")
  local back = K.restore(peer3, third)
  ok(back.result and back.result.unwritten ~= false, json.encode(back.error or {}))
end)

test("variations: an edit of B's bass since is no edit of the variation: its alias plays it, and Kumi's undo still removes it", function()
  local _, peer = seq_setup()
  local record = K.change(peer, "slot-variation", variation_input((K.place(peer))))
  mock.ops.set_note(2, 3, 9, 1, { note = 40 })
  eq(mock.song:pattern(4):track(3):line(9):note_column(1).note_string, "E-3", "the variation plays B's new bass note")
  local undone = K.restore(peer, record)
  ok(undone.result, json.encode(undone.error or {}))
  eq(seq(), "1,2,1,3")
end)

test("variations: a variation's apply refuses `changed` when the sequence changed after its preview (the fence)", function()
  local _, peer = seq_setup()
  local E = K.place(peer)
  local input = variation_input(E, { pattern = K.ref(E, "pattern", 2) })
  local previewed, epoch, layout = K.preview(peer, "slot-variation", input)
  mock.song.sequencer:insert_sequence_at(5, 1)
  local done = K.apply(peer, "slot-variation", input, "Kumi: V (kv9)", previewed.fence, epoch, layout)
  K.failure(done, "changed")
  eq(seq(), "1,2,1,3,1")
end)

test("variations: a variation's content outside the lines it varies, or on a group track, is bad-params", function()
  local _, peer = seq_setup()
  local E = K.place(peer)
  K.failure(K.ask(peer, "change_preview", "slot-variation", variation_input(E, { first = 0, last = 3 })), "bad-params")
  K.failure(K.ask(peer, "change_preview", "slot-variation", variation_input(E, { track = K.ref(E, "track", 5) })), "bad-params")
end)

test("variations: slots dropped in one step keep their patterns; Kumi's undo puts each back with its section and mutes", function()
  local _, peer = seq_setup()
  local E = K.place(peer)
  local record = K.change(peer, "slots-drop", { refs = { K.ref(E, "seq", 4), K.ref(E, "seq", 2) } }, "Kumi: drop (kd01)")
  eq(K.steps(), 1)
  eq(seq(), "1,1")
  eq(#mock.data.patterns, 3, "the patterns stay in the pool")
  eq(record.result.display.title, "Deleted slots 2 and 4")
  local undone = K.restore(peer, record)
  ok(undone.result, json.encode(undone.error or {}))
  eq(seq(), "1,2,1,3")
  eq(sections(), "2:Break")
  eq(mutes(), "4,2")
end)

test("variations: a dropped slot that started a section passes it on; Kumi's undo gives each its own back", function()
  local _, peer = seq_setup()
  local E = K.place(peer)
  local record = K.change(peer, "slots-drop", { refs = { K.ref(E, "seq", 2) } })
  eq(seq(), "1,1,3")
  eq(sections(), "2:Break", "Renoise passed the section on to the next slot")
  ok(K.restore(peer, record).result)
  eq(sections(), "2:Break")
  eq(mock.data.sections[3], nil, "the slot after it starts none again")
end)

test("variations: a drop's undo is refused `stale` once the sequence changed since, and unwritten once Renoise's undo put the slots back", function()
  local _, peer = seq_setup()
  local E = K.place(peer)
  local record = K.change(peer, "slots-drop", { refs = { K.ref(E, "seq", 2), K.ref(E, "seq", 3) } })
  mock.song.sequencer:insert_sequence_at(1, 2)
  K.failure(K.restore(peer, record), "stale")
  local _, peer2 = seq_setup()
  local E2 = K.place(peer2)
  local again = K.change(peer2, "slots-drop", { refs = { K.ref(E2, "seq", 2) } })
  mock.song:undo()
  eq(seq(), "1,2,1,3")
  local back = K.restore(peer2, again)
  ok(back.result and back.result.display.title:find("already", 1, true), json.encode(back.result or back.error))
end)

test("variations: a drop's apply refuses `changed` when the sequence changed after its preview (the fence); all slots is bad-params", function()
  local _, peer = seq_setup()
  local E = K.place(peer)
  local input = { refs = { K.ref(E, "seq", 2) } }
  local previewed, epoch, layout = K.preview(peer, "slots-drop", input)
  mock.song.sequencer:set_pattern(4, 2)
  K.failure(K.apply(peer, "slots-drop", input, "Kumi: drop (kd9)", previewed.fence, epoch, layout), "changed")
  K.failure(K.ask(peer, "change_preview", "slots-drop", { refs = { K.ref(E, "seq", 1), K.ref(E, "seq", 2), K.ref(E, "seq", 3), K.ref(E, "seq", 4) } }), "bad-params")
end)

test("variations: an empty pattern Renoise removes with its dropped slot is made again by Kumi's undo, with its name and length", function()
  local _, peer = seq_setup()
  local E = K.place(peer)
  local made = K.change(peer, "slot-insert", {})
  mock.data.patterns[4].name = "Empty"
  local E2 = K.place(peer)
  local record = K.change(peer, "slots-drop", { refs = { K.ref(E2, "seq", 5) } })
  eq(#mock.data.patterns, 3, "Renoise removed it")
  local undone = K.restore(peer, record)
  ok(undone.result, json.encode(undone.error or {}))
  eq(seq(), "1,2,1,3,4")
  eq(mock.data.patterns[4].name, "Empty")
  ok(made)
end)

test("variations: a removed pattern made again by Kumi's undo aliases the patterns it did, where the pool has them now", function()
  local _, peer = seq_setup()
  -- (pattern 4: unused, kept in the pool by a note; pattern 5: played at slot 5, the one the variation aliases)
  K.change(peer, "slot-insert", {})
  mock.ops.set_note(4, 1, 1, 1, { note = 48, instrument = 0 })
  local E = K.place(peer)
  K.change(peer, "slots-drop", { refs = { K.ref(E, "seq", 5) } })
  K.change(peer, "slot-insert", {})
  local target = #mock.data.patterns
  mock.ops.set_note(target, 1, 1, 1, { note = 50, instrument = 0 })
  -- (the variation: a new pattern after it, aliasing it on track 2)
  K.change(peer, "slot-insert", {})
  local v = #mock.data.patterns
  mock.song:pattern(v):track(2).alias_pattern_index = target
  mock.data.patterns[v].name = "V"
  local E2 = K.place(peer)
  local record = K.change(peer, "slots-drop", { refs = { K.ref(E2, "seq", 6) } })
  eq(#mock.data.patterns, target, "Renoise removed the variation's pattern")
  -- (the producer deletes the unused pattern 4: the pool's later patterns move down, and the slots playing them)
  mock.ops.remove_pattern(4)
  for s, p in ipairs(mock.data.sequence) do if p > 4 then mock.data.sequence[s] = p - 1 end end
  local undone = K.restore(peer, record)
  ok(undone.result, json.encode(undone.error or {}))
  local again = #mock.data.patterns
  eq(mock.data.patterns[again].name, "V")
  eq(mock.song:pattern(again):track(2).alias_pattern_index, target - 1, "the pattern it aliased, one down now")
end)

-- (review W3)
test("variations: Kumi's undo of a slot drop leaves a section the producer made or renamed on the next slot since", function()
  local _, peer = seq_setup()
  local E = K.place(peer)
  local record = K.change(peer, "slots-drop", { refs = { K.ref(E, "seq", 3) } })
  eq(seq(), "1,2,3")
  mock.song.sequencer:set_sequence_is_start_of_section(3, true)
  mock.song.sequencer:set_sequence_section_name(3, "Outro")
  local undone = K.restore(peer, record)
  ok(undone.result, json.encode(undone.error or {}))
  eq(seq(), "1,2,1,3")
  eq(sections(), "2:Break 4:Outro", "the producer's section stays")
  ok((undone.result.display.note or ""):find("left it", 1, true), json.encode(undone.result.display))
  local _, peer2 = seq_setup()
  local E2 = K.place(peer2)
  local again = K.change(peer2, "slots-drop", { refs = { K.ref(E2, "seq", 2) } })
  eq(sections(), "2:Break", "passed on")
  mock.song.sequencer:set_sequence_section_name(2, "Chorus")
  ok(K.restore(peer2, again).result)
  eq(sections(), "2:Break 3:Chorus", "the slot gets its own back; the renamed one stays")
end)

-- phrases-drop

local function four_phrases()
  local env, peer = setup()
  local bass = mock.song:instrument(2)
  bass:phrase(1).name = "P1"
  for k = 2, 4 do bass:insert_phrase_at(k).name = "P" .. k end
  for k = 1, 4 do bass:phrase(k):line(1):note_column(1).note_value = 47 + k end
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  return env, peer, bass
end
local function phrase_names(instrument)
  local out = {}
  for k = 1, #instrument.phrases do out[k] = instrument:phrase(k).name end
  return table.concat(out, ",")
end

test("variations: phrases dropped in one step; Kumi's undo puts each back after the phrase before it, with its lines", function()
  local _, peer, bass = four_phrases()
  local E = place(peer)
  local record, done = change(peer, "phrases-drop", { refs = { pref(E, 2, 4), pref(E, 2, 2) } }, "Kumi: drop (kp01)")
  eq(steps(), 1)
  eq(phrase_names(bass), "P1,P3")
  ok(done.display.title:find("Deleted 2 phrases", 1, true), done.display.title)
  bass:insert_phrase_at(1).name = "New"
  local undone = restore(peer, "phrases-drop", record)
  ok(undone.result, json.encode(undone.error or {}))
  eq(phrase_names(bass), "New,P1,P2,P3,P4")
  eq(bass:phrase(3):line(1):note_column(1).note_value, 49)
  eq(bass:phrase(5):line(1):note_column(1).note_value, 51)
end)

test("variations: Renoise's undo of a phrase drop leaves Kumi's undo nothing to write; a phrase in use is refused", function()
  local _, peer, bass = four_phrases()
  local E = place(peer)
  local record = change(peer, "phrases-drop", { refs = { pref(E, 2, 3), pref(E, 2, 1) } })
  mock.song:undo()
  eq(phrase_names(bass), "P1,P2,P3,P4")
  local back = restore(peer, "phrases-drop", record)
  ok(back.result and back.result.display.title:find("already", 1, true), json.encode(back.result or back.error))
  eq(phrase_names(bass), "P1,P2,P3,P4")
  local mapping = bass:insert_phrase_mapping_at(1, bass:phrase(2))
  mapping.note_range = { 60, 71 }
  local E2 = place(peer)
  failure(call(peer, "change_preview", { kind = "phrases-drop", input = { refs = { pref(E2, 2, 2) } }, epoch = E2 }), "bad-params")
  mock.ops.set_note(1, 3, 1, 1, { note = 48, instrument = 1 })
  mock.ops.set_effect(1, 3, 1, 1, 256 + 35, 3)
  failure(call(peer, "change_preview", { kind = "phrases-drop", input = { refs = { pref(E2, 2, 3) } }, epoch = E2 }), "bad-params")
end)

test("variations: a dropped phrase that was the instrument's program plays as the program again after Kumi's undo", function()
  local _, peer, bass = four_phrases()
  bass.phrase_playback_mode = renoise.Instrument.PHRASES_PLAY_SELECTIVE
  bass.phrase_program = 2
  mock.data.undo.stack = {}
  local E = place(peer)
  local record = change(peer, "phrases-drop", { refs = { pref(E, 2, 2), pref(E, 2, 4) } })
  eq(phrase_names(bass), "P1,P3")
  ok(bass.phrase_program ~= 2 or bass:phrase(2).name ~= "P2", "Renoise moved the program off it")
  local undone = restore(peer, "phrases-drop", record)
  ok(undone.result, json.encode(undone.error or {}))
  eq(phrase_names(bass), "P1,P2,P3,P4")
  eq(bass.phrase_program, 2, "P2 is the program again")
end)

test("variations: a phrase drop's apply refuses `changed` when a phrase changed after its preview (the fence)", function()
  local _, peer, bass = four_phrases()
  local epoch, layout = place(peer)
  local input = { refs = { pref(epoch, 2, 2) } }
  local previewed = preview(peer, "phrases-drop", input)
  bass:phrase(2):line(2):note_column(1).note_string = "G-4"
  local done = I.apply(peer, "phrases-drop", input, "Kumi: drop (kp9)", previewed.fence, epoch, layout)
  failure(done, "changed")
  eq(phrase_names(bass), "P1,P2,P3,P4")
end)

local function five_phrases()
  local env, peer, bass = four_phrases()
  bass:insert_phrase_at(5).name = "P5"
  bass:phrase(5):line(1):note_column(1).note_value = 52
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  return env, peer, bass
end

-- (review W1)
test("variations: phrases dropped next to each other come back in their order by Kumi's undo", function()
  local _, peer, bass = five_phrases()
  local E = place(peer)
  local record = change(peer, "phrases-drop", { refs = { pref(E, 2, 2), pref(E, 2, 3), pref(E, 2, 4) } })
  eq(phrase_names(bass), "P1,P5")
  local undone = restore(peer, "phrases-drop", record)
  ok(undone.result, json.encode(undone.error or {}))
  eq(phrase_names(bass), "P1,P2,P3,P4,P5")
  eq(bass:phrase(3):line(1):note_column(1).note_value, 50, "P3's lines with it")
  local _, peer2, bass2 = four_phrases()
  local E2 = place(peer2)
  local two = change(peer2, "phrases-drop", { refs = { pref(E2, 2, 3), pref(E2, 2, 2) } })
  eq(phrase_names(bass2), "P1,P4")
  bass2:insert_phrase_at(3).name = "New"
  ok(restore(peer2, "phrases-drop", two).result)
  eq(phrase_names(bass2), "P1,P2,P3,P4,New", "after P1, wherever it is, and before what came after it")
end)

test("variations: after Renoise's undo of a drop of neighbouring phrases, Kumi's undo finds them all back", function()
  local _, peer, bass = five_phrases()
  local E = place(peer)
  local record = change(peer, "phrases-drop", { refs = { pref(E, 2, 2), pref(E, 2, 3), pref(E, 2, 4) } })
  mock.song:undo()
  eq(phrase_names(bass), "P1,P2,P3,P4,P5")
  local back = restore(peer, "phrases-drop", record)
  ok(back.result and back.result.display.title:find("already", 1, true), json.encode(back.result or back.error))
  eq(phrase_names(bass), "P1,P2,P3,P4,P5")
end)

-- (review W2)
test("variations: a program on a phrase that stays plays that phrase after the drop and after Kumi's undo", function()
  local _, peer, bass = five_phrases()
  bass.phrase_playback_mode = renoise.Instrument.PHRASES_PLAY_SELECTIVE
  bass.phrase_program = 5
  mock.data.undo.stack = {}
  local E = place(peer)
  local record = change(peer, "phrases-drop", { refs = { pref(E, 2, 2), pref(E, 2, 3) } })
  eq(bass:phrase(bass.phrase_program).name, "P5", "after the drop")
  local undone = restore(peer, "phrases-drop", record)
  ok(undone.result, json.encode(undone.error or {}))
  eq(phrase_names(bass), "P1,P2,P3,P4,P5")
  eq(bass:phrase(bass.phrase_program).name, "P5", "after Kumi's undo")
end)

-- (review W13)
test("variations: a phrase drop looks through the song for 0Z commands once, however many phrases it drops", function()
  local _, peer = five_phrases()
  local E = place(peer)
  local fields = mock.impl(mock.song)
  local real, looks = fields.pattern, 0
  fields.pattern = function(self, p)
    looks = looks + 1
    return real(self, p)
  end
  preview(peer, "phrases-drop", { refs = { pref(E, 2, 2) } })
  local one = looks
  looks = 0
  preview(peer, "phrases-drop", { refs = { pref(E, 2, 2), pref(E, 2, 3), pref(E, 2, 4) } })
  fields.pattern = real
  ok(one > 0, "it looks")
  eq(looks, one, "one look for three phrases")
end)

-- (review W4)
test("variations: Kumi's undo of a take leaves it once the producer plays it by a 0Z command, keys or as the program", function()
  local _, peer = setup()
  local record = change(peer, "phrase-add", { ref = iref(place(peer), 2), lines = 8, content = CHOP })
  mock.ops.set_note(1, 3, 1, 1, { note = 48, instrument = 1 })
  mock.ops.set_effect(1, 3, 1, 1, 256 + 35, 2)
  local refused = restore(peer, "phrase-add", record)
  failure(refused, "changed")
  ok(refused.error.message:find("0Z", 1, true), refused.error.message)
  eq(#instrument(2).phrases, 2, "the take stays")
  eq(mock.song:pattern(1):track(3):line(1):effect_column(1).amount_value, 2, "and the producer's 0Z with it")
  mock.ops.set_effect(1, 3, 1, 1, 0, 0)
  local bass = mock.song:instrument(2)
  bass:insert_phrase_mapping_at(1, bass:phrase(2)).note_range = { 60, 71 }
  failure(restore(peer, "phrase-add", record), "changed")
  bass:delete_phrase_mapping_at(1)
  bass.phrase_playback_mode = renoise.Instrument.PHRASES_PLAY_SELECTIVE
  bass.phrase_program = 2
  failure(restore(peer, "phrase-add", record), "changed")
  bass.phrase_program = 1
  ok(restore(peer, "phrase-add", record).result, "unused again: Kumi's undo takes it")
  eq(#instrument(2).phrases, 1)
  -- (the program Renoise put on it as it was made is no use of the producer's)
  bass.phrase_program = 2
  local again = change(peer, "phrase-add", { ref = iref(place(peer), 2), lines = 8, content = CHOP })
  eq(bass.phrase_program, 2)
  ok(restore(peer, "phrase-add", again).result)
end)

-- read {kind: where}

test("variations: where a kept phrase and pattern are now, and a track carried past a track made before it", function()
  local _, peer, bass = four_phrases()
  local E, layout = place(peer)
  local record = change(peer, "phrase-add", { ref = iref(E, 2), lines = 8, content = CHOP })
  local kept = record.created.place
  ok(kept and kept.of == "phrase", "phrase-add says where its phrase is")
  bass:insert_phrase_at(1).name = "New"
  mock.api.insert_track_at(1)
  local E2 = place(peer)
  local where = call(peer, "read", { kind = "where", items = { { phrase = kept }, { track = K.ref(E, "track", 3) } }, layout = layout })
  ok(where.result, json.encode(where.error or {}))
  eq(where.result.items[1].index, 6, "the phrase moved up one")
  eq(where.result.items[1].ref, pref(E2, 2, 6))
  eq(where.result.items[2].index, 4, "the track moved up one")
  bass:delete_phrase_at(6)
  local gone = call(peer, "read", { kind = "where", items = { { phrase = kept } } })
  ok(gone.result.items[1].gone, json.encode(gone.result))
end)

test("variations: where gives the place of what a ref names now (a slot's pattern, an instrument), to find it by later", function()
  local _, peer = seq_setup()
  local E = K.place(peer)
  local where = K.call(peer, "read", { kind = "where", items = { { pattern = K.ref(E, "seq", 4) }, { instrument = K.ref(E, "instrument", 1) } } })
  ok(where.result, json.encode(where.error or {}))
  local pattern, instrument = where.result.items[1], where.result.items[2]
  eq(pattern.number, 2, "slot 4 plays C (pattern 02)")
  eq(table.concat(pattern.slots, ","), "4")
  ok(type(pattern.place) == "table" and pattern.place.n == 3, json.encode(pattern))
  ok(type(instrument.place) == "table" and instrument.place.of == "instrument", json.encode(instrument))
  -- (a pattern made after it leaves it where it is; the place follows)
  K.change(peer, "slot-insert", {})
  local again = K.call(peer, "read", { kind = "where", items = { { pattern = pattern.place } } })
  eq(again.result.items[1].number, 2)
  eq(table.concat(again.result.items[1].slots, ","), "4")
end)

test("variations: a kept variation that took the section of one dropped before it is still Kumi's to undo; its mutes are the producer's", function()
  local _, peer = seq_setup()
  local first = K.change(peer, "slot-variation", variation_input((K.place(peer))))
  local second = K.change(peer, "slot-variation", variation_input((K.place(peer)), { section = false, content = { { line = 0, notes = { { col = 1, note = "C-4" } } } } }))
  eq(seq(), "1,2,1,3,4,5")
  local E = K.place(peer)
  K.change(peer, "slots-drop", { refs = { K.ref(E, "seq", 5) } })
  eq(sections(), "2:Break 5:Variations of B", "the section passed on to the second variation")
  local undone = K.restore(peer, second)
  ok(undone.result, json.encode(undone.error or {}))
  eq(seq(), "1,2,1,3")
  -- (a track the producer muted in a variation's slot keeps it there)
  local _, peer2 = seq_setup()
  local made = K.change(peer2, "slot-variation", variation_input((K.place(peer2))))
  mock.ops.mute_slot(1, 5, true)
  K.failure(K.restore(peer2, made), "changed")
  ok(first)
end)

test("variations: a source track that is an alias is read through, never written through: the take's track is a copy of its own", function()
  local _, peer = seq_setup()
  -- (B's track 1 plays A's lines as an alias: A holds the break now)
  mock.ops.set_note(1, 1, 1, 1, { note = 48 })
  mock.ops.set_note(1, 1, 5, 1, { note = 50 })
  mock.ops.alias(2, 1, 1)
  local before = table.concat({ cell(1, 1, 1).note_string, cell(1, 1, 5).note_string, cell(1, 1, 7).note_string }, ",")
  local record = K.change(peer, "slot-variation", variation_input((K.place(peer))))
  eq(mock.song:pattern(4):track(1).alias_pattern_index, 0, "the take's track is no alias")
  eq(cell(4, 1, 7).note_string, "D-4", "the take holds its lines")
  eq(table.concat({ cell(1, 1, 1).note_string, cell(1, 1, 5).note_string, cell(1, 1, 7).note_string }, ","), before, "A, the alias's source, is as it was")
  -- (and a write to the take later goes to the take, not to A)
  local E = K.place(peer)
  K.change(peer, "notes-write", { pattern = K.ref(E, "pattern", 4), track = K.ref(E, "track", 1), lines = { { line = 2, notes = { { col = 1, note = "G-4" } } } } })
  eq(cell(1, 1, 3).note_string, "---", "A untouched")
  eq(cell(4, 1, 3).note_string, "G-4")
  ok(record)
end)

test("variations: the take's slot mutes what the source slot named mutes", function()
  local _, peer = seq_setup()
  mock.ops.mute_slot(3, 3, true) -- (A plays in slots 1 and 3; slot 3 mutes track 3)
  local E = K.place(peer)
  K.change(peer, "slot-variation", variation_input(E, { pattern = K.ref(E, "seq", 3), first = 0, last = 7, content = { { line = 0, notes = { { col = 1, note = "D-4" } } } } }))
  eq(mutes(), "3,3 3,5 4,2", "slot 5 mutes track 3, as slot 3 does (not slot 1, which plays A too)")
end)
