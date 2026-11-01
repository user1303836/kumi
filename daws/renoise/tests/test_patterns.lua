-- M4 (n-song): Kumi's changes to a pattern (its name and length) and to a pattern's track (its slot colour): preview, apply,
-- restore, what the producer did in between, the places a record keeps.
local T = ...
local mock = T.mock
local test, eq, ok = T.test, T.eq, T.ok
local H = T.harness
local K = assert(loadfile(T.tests_dir .. "change_kit.lua"))(T)

local function pattern(p) return mock.data.patterns[p] end
local function note_at(p, t, l)
  local column = mock.song:pattern(p):track(t):line(l):note_column(1)
  return column.note_string
end

test("patterns: a pattern renamed by its ref or by a slot that plays it, and put back", function()
  local _, peer = K.setup()
  local E = K.place(peer)
  local record = K.change(peer, "pattern-name", { ref = K.ref(E, "seq", 2), name = "Verse" }, "Kumi: name (kp01)")
  eq(record.result.display.title, "Named the pattern \226\128\156Verse\226\128\157")
  eq(record.target.ref, K.ref(E, "pattern", 2))
  eq(pattern(2).name, "Verse")
  eq(K.steps(), 1)
  local again = K.change(peer, "pattern-name", { ref = K.ref(E, "pattern", 2), name = "Chorus" })
  eq(again.result.display.title, "Renamed pattern \226\128\156Verse\226\128\157 \226\134\146 \226\128\156Chorus\226\128\157")
  ok(K.restore(peer, again).result)
  ok(K.restore(peer, record).result)
  eq(pattern(2).name, "")
  -- the producer renames it in between: refused, and discard puts it back anyway
  local third = K.change(peer, "pattern-name", { ref = K.ref(E, "pattern", 1), name = "Intro" })
  pattern(1).name = "Mine"
  K.failure(K.restore(peer, third), "changed")
  eq(pattern(1).name, "Mine")
  ok(K.restore(peer, third, nil, { discard = true }).result)
  eq(pattern(1).name, "")
  H.no_dialogs()
end)

test("patterns: a pattern's record follows its pattern through patterns made or deleted before it, and is gone with it", function()
  local _, peer = K.setup(nil, { patterns = 4, sequence = { 2, 3, 2, 4 }, sequence_length = 4 })
  local E = K.place(peer)
  local name = K.change(peer, "pattern-name", { ref = K.ref(E, "pattern", 4), name = "Outro" })
  local length = K.change(peer, "pattern-length", { ref = K.ref(E, "pattern", 4), lines = 16 })
  local other = K.change(peer, "pattern-name", { ref = K.ref(E, "pattern", 3), name = "Drop" })
  mock.ops.insert_pattern() -- at the pool's end
  mock.ops.remove_pattern(1) -- the producer deletes the unused first pattern: the others are one less
  ok(K.restore(peer, name).result, "followed")
  ok(K.restore(peer, length).result, "followed")
  eq(pattern(3).name, "")
  eq(pattern(3).lines, 32)
  mock.ops.remove_pattern(2) -- the pattern "Drop" is in
  K.failure(K.restore(peer, other), "gone")
  H.no_dialogs()
end)

test("patterns: a slot colour set and cleared on a pattern's track, each a step, and Renoise's undo of it told", function()
  local _, peer = K.setup()
  local E = K.place(peer)
  local record = K.change(peer, "slot-color", { ref = K.ref(E, "seq", 1), track = K.ref(E, "track", 2), color = "#FF8000" }, "Kumi: colour (kc01)")
  eq(record.result.display.title, "Slot colour of \226\128\156Pattern 00\226\128\157 on \226\128\156Hat\226\128\157 #ff8000")
  eq(record.target.ref, K.ref(E, "ptrack", 1, 2))
  eq(pattern(1).tracks[2].color[1], 255)
  eq(K.newest(), "Kumi: colour (kc01)")
  local cleared = K.change(peer, "slot-color", { ref = K.ref(E, "ptrack", 1, 2), color = T.load_payload_module("json").null }, "Kumi: clear (kc02)")
  eq(cleared.result.display.title, "Slot colour of \226\128\156Pattern 00\226\128\157 on \226\128\156Hat\226\128\157 cleared")
  eq(pattern(1).tracks[2].color, nil)
  K.told(peer)
  mock.song:undo()
  eq(K.told(peer), "undo:Kumi: clear (kc02)")
  eq(pattern(1).tracks[2].color[2], 128)
  -- Kumi's undo of the first: the colour goes
  ok(K.restore(peer, record).result)
  eq(pattern(1).tracks[2].color, nil)
  K.failure(K.ask(peer, "change_preview", "slot-color", { ref = K.ref(E, "pattern", 1), track = K.ref(E, "track", 2), color = "orange" }), "bad-params")
  H.no_dialogs()
end)

test("patterns: a slot colour's record follows its track through a track made before it, and is gone with the track", function()
  local _, peer = K.setup()
  local E = K.place(peer)
  local record = K.change(peer, "slot-color", { ref = K.ref(E, "pattern", 2), track = K.ref(E, "track", 3), color = "#112233" })
  local other = K.change(peer, "slot-color", { ref = K.ref(E, "pattern", 2), track = K.ref(E, "track", 4), color = "#445566" })
  mock.ops.insert_track(1, { name = "Intro" })
  ok(K.restore(peer, record).result, "followed")
  eq(mock.data.tracks[4].name, "Bass")
  eq(pattern(2).tracks[4].color, nil)
  mock.ops.remove_track(5) -- Pad
  K.failure(K.restore(peer, other), "gone")
  H.no_dialogs()
end)

test("patterns: a shorter pattern keeps its lines hidden, and Kumi's undo shows them again", function()
  local _, peer = K.setup()
  local E = K.place(peer)
  mock.ops.set_note(1, 1, 30, 1, { note = 48 })
  local record = K.change(peer, "pattern-length", { ref = K.ref(E, "pattern", 1), lines = 16 }, "Kumi: length (kl01)")
  eq(record.result.display.title, "Pattern 00 16 lines")
  eq(record.result.display.note, "Lines 16 to 31 are kept, hidden: making it longer again shows them.")
  eq(pattern(1).lines, 16)
  eq(K.steps(), 1)
  ok(K.restore(peer, record).result)
  eq(pattern(1).lines, 32)
  eq(note_at(1, 1, 30), "C-4", "the hidden line is back in sight")
  local again = K.restore(peer, record)
  eq(again.result.display.note, "It was back already: nothing was written.")
  H.no_dialogs()
end)

test("patterns: Kumi's undo of a longer pattern is refused once the producer wrote in the lines it showed", function()
  local _, peer = K.setup()
  local E = K.place(peer)
  local record = K.change(peer, "pattern-length", { ref = K.ref(E, "pattern", 2), lines = 48 })
  eq(pattern(2).lines, 48)
  mock.ops.set_note(2, 3, 40, 1, { note = 60 }) -- the producer, in a line the change showed
  local message = K.failure(K.restore(peer, record), "changed")
  ok(message:find("lines 32 to 47", 1, true), message)
  eq(pattern(2).lines, 48, "left as it is")
  ok(K.restore(peer, record, nil, { discard = true }).result, "discard hides them")
  eq(pattern(2).lines, 32)
  eq(note_at(2, 3, 40), "C-5", "hidden, not deleted")
  -- a lengthening that shows what was kept says so
  local shown = K.preview(peer, "pattern-length", { ref = K.ref(E, "pattern", 2), lines = 64 })
  eq(shown.display.note, "Lines 32 to 63 hold what was kept there, hidden: it shows again.")
  H.no_dialogs()
end)

test("patterns: a length changed in Renoise since refuses the undo; the apply is fenced by the lines it would show or hide", function()
  local _, peer = K.setup()
  local E = K.place(peer)
  local record = K.change(peer, "pattern-length", { ref = K.ref(E, "pattern", 3), lines = 8 })
  pattern(3).lines = 12
  K.failure(K.restore(peer, record), "changed")
  eq(pattern(3).lines, 12)
  -- the fence: a hidden line changed between the preview and the apply of a lengthening
  local previewed, epoch, layout = K.preview(peer, "pattern-length", { ref = K.ref(E, "pattern", 3), lines = 32 })
  mock.ops.set_note(3, 1, 20, 1, { note = 50 })
  local refused = K.apply(peer, "pattern-length", { ref = K.ref(E, "pattern", 3), lines = 32 }, "Kumi: l (kl09)", previewed.fence, epoch, layout)
  K.failure(refused, "changed")
  eq(pattern(3).lines, 12, "nothing written")
  H.no_dialogs()
end)

test("patterns: bad lengths and names are bad-params", function()
  local _, peer = K.setup()
  local E = K.place(peer)
  local function bad(kind, input) K.failure(K.ask(peer, "change_preview", kind, input), "bad-params") end
  bad("pattern-length", { ref = K.ref(E, "pattern", 1), lines = 0 })
  bad("pattern-length", { ref = K.ref(E, "pattern", 1), lines = 513 })
  bad("pattern-length", { ref = K.ref(E, "pattern", 1), lines = 2.5 })
  bad("pattern-name", { ref = K.ref(E, "pattern", 1), name = "a\nb" })
  bad("pattern-name", { ref = K.ref(E, "track", 1), name = "x" })
  H.no_dialogs()
end)

test("patterns: the pattern rows and the matrix show slot colours", function()
  local _, peer = K.setup()
  local E = K.place(peer)
  mock.song:pattern(1):track(2).color = { 255, 0, 0 }
  local rows = K.call(peer, "read", { kind = "pattern" }).result.items
  eq(rows[1].colors["2"], "#ff0000")
  eq(rows[2].colors, nil)
  local matrix = K.call(peer, "focus_body", { kind = "matrix" }).result
  eq(matrix.slots[1].colors["2"], "#ff0000")
  eq(matrix.slots[3].colors["2"], "#ff0000", "every slot that plays the pattern")
  eq(matrix.slots[2].colors, nil)
  H.no_dialogs()
end)

-- Aliases

local function cells(p, t, from, to)
  local out = {}
  for l = from, to do
    local text = note_at(p, t, l)
    if text ~= "---" then out[#out + 1] = l .. "=" .. text end
  end
  return table.concat(out, " ")
end

test("patterns: a track made an alias drops its own lines; Kumi keeps them and its undo writes them back", function()
  local _, peer = K.setup()
  local E = K.place(peer)
  mock.ops.set_note(1, 2, 1, 1, { note = 48 })
  mock.ops.set_note(2, 2, 3, 1, { note = 52 })
  local previewed = K.preview(peer, "pattern-alias", { ref = K.ref(E, "pattern", 2), track = K.ref(E, "track", 2), alias = K.ref(E, "pattern", 1) })
  eq(previewed.display.title, "\226\128\156Pattern 01\226\128\157 on \226\128\156Hat\226\128\157 plays \226\128\156Pattern 00\226\128\157's lines (an alias)")
  eq(previewed.display.note, "Its own lines go out of sight (Renoise drops them); Kumi keeps them to put back.")
  local record = K.change(peer, "pattern-alias", { ref = K.ref(E, "pattern", 2), track = K.ref(E, "track", 2), alias = K.ref(E, "pattern", 1) }, "Kumi: alias (ka01)")
  eq(pattern(2).tracks[2].alias, 1)
  eq(cells(2, 2, 1, 8), "1=C-4", "it shows the source's lines")
  eq(#record.material.lines, 1, "its own line is kept")
  eq(K.steps(), 1)
  ok(K.restore(peer, record).result)
  eq(pattern(2).tracks[2].alias, 0)
  eq(cells(2, 2, 1, 8), "3=E-4", "its own lines are back")
  eq(K.steps(), 2)
  eq(K.restore(peer, record).result.display.note, "It was back already: nothing was written.")
  H.no_dialogs()
end)

test("patterns: an alias's kept lines go back playing the instruments they played, where those are now; a deleted one refuses (#361 N11)", function()
  local _, peer = K.setup(nil, { instruments = 2 })
  local E = K.place(peer)
  mock.ops.set_note(2, 2, 3, 1, { note = 52, instrument = 1 }) -- (the second instrument: Renoise's 01)
  local input = { ref = K.ref(E, "pattern", 2), track = K.ref(E, "track", 2), alias = K.ref(E, "pattern", 1) }
  local record = K.change(peer, "pattern-alias", input, "Kumi: alias (ka31)")
  mock.ops.insert_instrument_at(1, { name = "Piano" }) -- the producer makes one before it: Renoise renumbers notes, not kept lines
  H.tick(2)
  local undone = K.restore(peer, record)
  ok(undone.result, T.load_payload_module("json").encode(undone.error or {}))
  eq(mock.song:pattern(2):track(2):line(3):note_column(1).instrument_value, 2, "the same instrument, now 02")
  -- the instrument it played deleted: refused, nothing written
  local again = K.change(peer, "pattern-alias", { ref = K.ref(K.place(peer), "pattern", 2), track = K.ref(K.place(peer), "track", 2),
    alias = K.ref(K.place(peer), "pattern", 1) }, "Kumi: alias (ka32)")
  mock.song:delete_instrument_at(3)
  H.tick(2)
  local before = K.steps()
  K.failure(K.restore(peer, again), "changed")
  eq(pattern(2).tracks[2].alias, 1, "still the alias")
  eq(K.steps(), before)
  H.no_dialogs()
end)

test("patterns: after Renoise's own undo and redo of an alias Kumi made, Kumi's undo takes it away with its own lines back", function()
  local _, peer = K.setup()
  local E = K.place(peer)
  mock.ops.set_note(1, 2, 1, 1, { note = 48 })
  mock.ops.set_note(2, 2, 3, 1, { note = 52 })
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  local record = K.change(peer, "pattern-alias", { ref = K.ref(E, "pattern", 2), track = K.ref(E, "track", 2), alias = K.ref(E, "pattern", 1) }, "Kumi: alias (ka21)")
  K.told(peer)
  mock.song:undo()
  eq(pattern(2).tracks[2].alias, 0)
  ok(K.told(peer):find("undo:Kumi: alias (ka21)", 1, true), "the undo told")
  mock.song:redo()
  eq(pattern(2).tracks[2].alias, 1)
  ok(K.told(peer):find(":Kumi: alias (ka21)", 1, true), "the redo told")
  local undone = K.restore(peer, record)
  ok(undone.result, "the alias Renoise's redo made again is Kumi's: " .. T.load_payload_module("json").encode(undone.error or {}))
  eq(pattern(2).tracks[2].alias, 0)
  eq(cells(2, 2, 1, 8), "3=E-4", "its own lines are back")
  H.no_dialogs()
end)

test("patterns: an alias taken away leaves a copy of its lines (keep) or nothing, and Kumi's undo makes it an alias again", function()
  local _, peer = K.setup()
  local E = K.place(peer)
  mock.ops.set_note(1, 3, 2, 1, { note = 55 })
  mock.ops.alias(2, 3, 1)
  local copy = K.change(peer, "pattern-alias", { ref = K.ref(E, "pattern", 2), track = K.ref(E, "track", 3), alias = T.load_payload_module("json").null })
  eq(copy.result.display.title, "\226\128\156Pattern 01\226\128\157 on \226\128\156Bass\226\128\157 is no alias any more (a copy of its lines)")
  eq(pattern(2).tracks[3].alias, 0)
  eq(cells(2, 3, 1, 8), "2=G-4")
  ok(K.restore(peer, copy).result)
  eq(pattern(2).tracks[3].alias, 1)
  local empty = K.change(peer, "pattern-alias", { ref = K.ref(E, "pattern", 2), track = K.ref(E, "track", 3), alias = false, keep = false })
  eq(pattern(2).tracks[3].alias, 0)
  eq(cells(2, 3, 1, 8), "")
  ok(K.restore(peer, empty).result)
  eq(pattern(2).tracks[3].alias, 1)
  H.no_dialogs()
end)

test("patterns: an alias of an alias is made of the source it shows; its own pattern is refused", function()
  local _, peer = K.setup()
  local E = K.place(peer)
  mock.ops.alias(2, 1, 1)
  local record = K.change(peer, "pattern-alias", { ref = K.ref(E, "pattern", 3), track = K.ref(E, "track", 1), alias = K.ref(E, "pattern", 2) })
  eq(pattern(3).tracks[1].alias, 1, "the source pattern 2's track shows")
  eq(record.result.display.title, "\226\128\156Pattern 02\226\128\157 on \226\128\156Kick\226\128\157 plays \226\128\156Pattern 00\226\128\157's lines (an alias)")
  K.failure(K.ask(peer, "change_preview", "pattern-alias", { ref = K.ref(E, "pattern", 1), track = K.ref(E, "track", 1), alias = K.ref(E, "pattern", 2) }), "bad-params")
  H.no_dialogs()
end)

test("patterns: an edit in the pattern since refuses the alias's undo (the plain rule), and the apply is fenced by the track's lines", function()
  local _, peer = K.setup()
  local E = K.place(peer)
  mock.ops.set_note(2, 2, 3, 1, { note = 52 })
  local record = K.change(peer, "pattern-alias", { ref = K.ref(E, "pattern", 2), track = K.ref(E, "track", 2), alias = K.ref(E, "pattern", 1) })
  mock.ops.set_note(2, 4, 9, 1, { note = 40 }) -- the producer, on another track of the same pattern
  local message = K.failure(K.restore(peer, record), "changed")
  ok(message:find("the pattern changed in Renoise since Kumi's step", 1, true), message)
  ok(K.failure(K.restore(peer, record, nil, { discard = true }), "changed"), "discard doesn't pass over lines")
  eq(pattern(2).tracks[2].alias, 1)
  -- the fence: the track's own lines changed between the preview and the apply
  local previewed, epoch, layout = K.preview(peer, "pattern-alias", { ref = K.ref(E, "pattern", 3), track = K.ref(E, "track", 2), alias = K.ref(E, "pattern", 1) })
  mock.ops.set_note(3, 2, 5, 1, { note = 41 })
  local refused = K.apply(peer, "pattern-alias", { ref = K.ref(E, "pattern", 3), track = K.ref(E, "track", 2), alias = K.ref(E, "pattern", 1) }, "Kumi: a (ka09)", previewed.fence, epoch, layout)
  K.failure(refused, "changed")
  eq(pattern(3).tracks[2].alias, 0, "nothing written")
  H.no_dialogs()
end)

test("patterns: Renoise's undo of an alias is told, and an alias over lines Kumi can't put back in one step is refused", function()
  local _, peer = K.setup(nil, { pattern_lines = 512 })
  local E = K.place(peer)
  K.change(peer, "pattern-alias", { ref = K.ref(E, "pattern", 2), track = K.ref(E, "track", 1), alias = K.ref(E, "pattern", 1) }, "Kumi: alias (ka11)")
  K.told(peer)
  mock.song:undo()
  eq(K.told(peer), "undo:Kumi: alias (ka11)")
  for l = 1, 512 do
    for c = 1, 8 do mock.song:pattern(3):track(3):line(l):note_column(c).note_value = 40 end
  end
  local refused = K.call(peer, "change_apply", (function()
    local previewed, epoch, layout = K.preview(peer, "pattern-alias", { ref = K.ref(E, "pattern", 3), track = K.ref(E, "track", 3), alias = K.ref(E, "pattern", 1) })
    return { kind = "pattern-alias", input = { ref = K.ref(E, "pattern", 3), track = K.ref(E, "track", 3), alias = K.ref(E, "pattern", 1) }, fence = previewed.fence, desc = "Kumi: a (ka12)", epoch = epoch, layout = layout }
  end)())
  local message = K.failure(refused, "failed")
  ok(message:find("more than Kumi puts back in one step", 1, true), message)
  eq(pattern(3).tracks[3].alias, 0)
  H.no_dialogs()
end)

test("patterns: an alias's record follows its patterns through a pattern made and one deleted before them", function()
  local _, peer = K.setup(nil, { patterns = 4, sequence = { 2, 3, 2, 4 }, sequence_length = 4 })
  local E = K.place(peer)
  mock.ops.set_note(4, 2, 3, 1, { note = 52 })
  local record = K.change(peer, "pattern-alias", { ref = K.ref(E, "pattern", 4), track = K.ref(E, "track", 2), alias = K.ref(E, "pattern", 3) })
  eq(pattern(4).tracks[2].alias, 3)
  mock.ops.insert_pattern()
  mock.ops.remove_pattern(1)
  -- (the mock leaves alias numbers as they were; Renoise renumbers them with the pool)
  pattern(3).tracks[2].alias = 2
  local undone = K.restore(peer, record)
  ok(undone.result, "followed: " .. tostring(undone.error and undone.error.message))
  eq(pattern(3).tracks[2].alias, 0)
  eq(cells(3, 2, 1, 8), "3=E-4", "its own lines back, in the pattern where it is now")
  H.no_dialogs()
end)

test("patterns: a longer pattern says it shows kept lines only when there are some, reading the region once (#361 N-S20)", function()
  local _, peer = K.setup()
  local E = K.place(peer)
  local plain = K.preview(peer, "pattern-length", { ref = K.ref(E, "pattern", 1), lines = 64 })
  eq(plain.display.note, nil, "nothing was kept past its end: nothing to say")
  mock.ops.set_note(1, 2, 40, 1, { note = 48 }) -- a line past its end, kept hidden
  local kept = K.preview(peer, "pattern-length", { ref = K.ref(K.place(peer), "pattern", 1), lines = 64 })
  ok((kept.display.note or ""):find("hold what was kept", 1, true), tostring(kept.display.note))
end)

test("patterns: read with prints, each track's lines summed up as the catch-up does, its automation counted, 16 a page", function()
  local _, peer = K.setup()
  local E = K.place(peer)
  local column = mock.song:pattern(1):track(1):line(1):note_column(1)
  column.note_string, column.instrument_value = "C-4", 0
  local read = K.call(peer, "read", { kind = "pattern", prints = true, limit = 200, epoch = E })
  ok(read.result, T.load_payload_module("json").encode(read.error or {}))
  local first = read.result.items[1]
  ok(first.prints["1"]:find("^1 notes, 0 effects #"), first.prints["1"])
  eq(first.prints["2"], nil, "an empty track has no print")
  eq(first.automation, 0)
  ok(#read.result.items <= 16, "at most 16 a page")
  -- The same lines, the same print; other lines, another.
  local again = K.call(peer, "read", { kind = "pattern", prints = true, epoch = E }).result.items[1]
  eq(again.prints["1"], first.prints["1"])
  column.note_string = "D-4"
  local moved = K.call(peer, "read", { kind = "pattern", prints = true, epoch = E }).result.items[1]
  ok(moved.prints["1"] ~= first.prints["1"], "another print")
end)

test("patterns: a length's region is read as its lines' text, every column of them, none of them one by one", function()
  local _, peer = K.setup()
  local E = K.place(peer)
  pattern(3).lines = 12
  local input = { ref = K.ref(E, "pattern", 3), lines = 32 }
  mock.ops.set_note(3, 1, 20, 1, { note = 50 })
  mock.column_reads = 0
  local previewed, epoch, layout = K.preview(peer, "pattern-length", input)
  eq(mock.column_reads, 0, "no column is read one by one")
  -- A note column Renoise hides, on a line that holds a note already: the region changed.
  mock.ops.set_note(3, 1, 20, 12, { note = 60 })
  K.failure(K.apply(peer, "pattern-length", input, "Kumi: l (kl10)", previewed.fence, epoch, layout), "changed")
  -- The last effect column of that line: so did it.
  previewed, epoch, layout = K.preview(peer, "pattern-length", input)
  mock.song:pattern(3):track(1):line(20):effect_column(8).amount_value = 9
  K.failure(K.apply(peer, "pattern-length", input, "Kumi: l (kl11)", previewed.fence, epoch, layout), "changed")
  -- The same note in the next column: the column counts, not only what's in the line.
  mock.song:pattern(3):track(1):line(20):effect_column(8).amount_value = 0
  mock.song:pattern(3):track(1):line(20):note_column(12).note_value = 121
  previewed, epoch, layout = K.preview(peer, "pattern-length", input)
  mock.song:pattern(3):track(1):line(20):note_column(1).note_value = 121
  mock.song:pattern(3):track(1):line(20):note_column(1).instrument_value = 255
  mock.ops.set_note(3, 1, 20, 2, { note = 50 })
  K.failure(K.apply(peer, "pattern-length", input, "Kumi: l (kl13)", previewed.fence, epoch, layout), "changed")
  -- Nothing changed since: it applies.
  previewed, epoch, layout = K.preview(peer, "pattern-length", input)
  ok(K.apply(peer, "pattern-length", input, "Kumi: l (kl12)", previewed.fence, epoch, layout).result)
  H.no_dialogs()
end)

test("patterns: a length's undo from an older tool says it can't tell whether its lines changed, and one over nothing still goes", function()
  local _, peer = K.setup()
  local E = K.place(peer)
  pattern(3).lines = 12
  mock.ops.set_note(3, 1, 20, 1, { note = 50 })
  local record = K.change(peer, "pattern-length", { ref = K.ref(E, "pattern", 3), lines = 32 })
  local steps = K.steps()
  -- Lines that changed under this tool's own record say so, not that it's older.
  mock.ops.set_note(3, 1, 20, 1, { note = 51 })
  local changed = K.failure(K.restore(peer, record), "changed")
  ok(changed:find("changed in Renoise since", 1, true) and not changed:find("older version", 1, true), changed)
  mock.ops.set_note(3, 1, 20, 1, { note = 50 })
  -- The fingerprint as an older tool kept it.
  local mine = record.applied.region
  record.applied.region = "7e61980ad040e005"
  local older = K.failure(K.restore(peer, record), "changed")
  ok(older:find("older version of Kumi's tool", 1, true) and older:find("lines 12 to 31", 1, true), older)
  eq(pattern(3).lines, 32, "nothing written")
  eq(K.steps(), steps, "no step")
  -- Taking it back without keeping what's there doesn't ask.
  ok(K.restore(peer, record, nil, { discard = true }).result)
  eq(pattern(3).lines, 12)
  ok(mine:sub(1, 1) == "t", mine)
  -- Over nothing, an older tool's record and this one's read the same.
  pattern(2).lines = 12
  local empty = K.change(peer, "pattern-length", { ref = K.ref(E, "pattern", 2), lines = 16 })
  eq(empty.applied.region, "none")
  ok(K.restore(peer, empty).result)
  eq(pattern(2).lines, 12)
  H.no_dialogs()
end)
