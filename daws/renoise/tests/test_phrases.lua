-- Kumi's changes to phrases, phrase scripts, plug-in and MIDI instruments (M4, n-instruments): each applied, put back, refused
-- when changed, gone, back already; the places followed; scripts' compile errors reported.
local T = ...
local mock = T.mock
local json = T.load_payload_module("json")
local test, eq, ok = T.test, T.eq, T.ok
local H = T.harness
local I = T.instruments
local setup, place, preview, apply, change, restore, failure, call = I.setup, I.place, I.preview, I.apply, I.change, I.restore, I.failure, I.call
local steps, newest, instrument, iref = I.steps, I.newest, I.instrument, I.iref

local function pref(epoch, i, p) return epoch .. ":phrase:" .. i .. "." .. p end
local function phrase(i, p) return mock.data.instruments[i].phrases[p] end
local function cell(i, p, l, c) return mock.song:instrument(i):phrase(p):line(l):note_column(c or 1) end

-- phrase-add, phrase-delete, phrase-set

test("phrases: a phrase made with its settings; Kumi's undo removes it, refused once the producer wrote in it", function()
  local env, peer = setup()
  local record, done = change(peer, "phrase-add", { ref = iref(place(peer), 2), index = 1, name = "Riff", lines = 32, lpb = 8, keyTracking = "transpose", looping = true })
  eq(#instrument(2).phrases, 2)
  eq(phrase(2, 1).name, "Riff")
  eq(phrase(2, 1).lines, 32)
  eq(phrase(2, 1).lpb, 8)
  eq(phrase(2, 1).key_tracking, 2)
  eq(done.created.kind, "phrase")
  local undone = restore(peer, "phrase-add", record)
  ok(undone.result, json.encode(undone.error or {}))
  eq(#instrument(2).phrases, 1)
  record = change(peer, "phrase-add", { ref = iref(place(peer), 2) })
  cell(2, 2, 1).note_string = "C-4"
  failure(restore(peer, "phrase-add", record), "changed")
  eq(#instrument(2).phrases, 2)
end)

test("phrases: a deleted phrase comes back with its lines, settings, column name, keys and script, after the phrase before it", function()
  local env, peer = setup()
  local p = mock.song:instrument(2):phrase(1)
  p.name = "Arp"
  p.number_of_lines = 8
  p.lpb = 12
  p:line(1):note_column(1).note_string = "C-4"
  p:line(3):note_column(2).note_string = "E-4"
  p:line(5):effect_column(1).number_string = "0A"
  p:set_column_name(1, "Root")
  local mapping = mock.song:instrument(2):insert_phrase_mapping_at(1, p)
  mapping.note_range = { 60, 71 }
  -- (how its keys play it, the mapping's own: #361 N13)
  mapping.key_tracking, mapping.base_note, mapping.looping, mapping.loop_end, mapping.loop_start = 3, 60, false, 8, 3
  mock.data.undo.stack = {}
  local record = change(peer, "phrase-delete", { ref = pref(place(peer), 2, 1) })
  eq(#instrument(2).phrases, 0)
  eq(#instrument(2).mappings, 0, "its mapping went with it")
  mock.song:instrument(2):insert_phrase_at(1).name = "New"
  local undone = restore(peer, "phrase-delete", record)
  ok(undone.result, json.encode(undone.error or {}))
  eq(#instrument(2).phrases, 2)
  local back = mock.song:instrument(2):phrase(1)
  eq(back.name, "Arp", "first, as it was: no phrase was before it")
  eq(back.number_of_lines, 8)
  eq(back.lpb, 12)
  eq(back:line(3):note_column(2).note_string, "E-4")
  eq(back:line(5):effect_column(1).number_string, "0A")
  eq(back:column_name(1), "Root")
  eq(instrument(2).mappings[1].note_range[1], 60, "its keys mapped again")
  local kept = instrument(2).mappings[1]
  eq(string.format("%s %s %s %s %s", kept.key_tracking, kept.base_note, tostring(kept.looping), kept.loop_start, kept.loop_end), "3 60 false 3 8",
    "playing as they did (#361 N13)")
  local again = restore(peer, "phrase-delete", record)
  ok(again.result, json.encode(again.error or {}))
  ok(again.result.display.title:find("back already", 1, true), again.result.display.title)
end)

test("phrases: Kumi's undo of a phrase delete writes back the 0Z commands Renoise cleared, and plays it as the program again (#361 N-S23)", function()
  local env, peer = setup()
  local bass = mock.song:instrument(2)
  bass:insert_phrase_at(2).name = "P2"
  bass:insert_phrase_at(3).name = "P3"
  for k = 1, 3 do
    mock.ops.set_note(1, 1, k, 1, { note = 48, instrument = 1 }) -- Bass synth's notes, each with its phrase's 0Z
    mock.ops.set_effect(1, 1, k, 1, 256 + 35, k)
  end
  bass.phrase_playback_mode = renoise.Instrument.PHRASES_PLAY_SELECTIVE
  bass.phrase_program = 2
  mock.data.undo.stack = {}
  local record, done = change(peer, "phrase-delete", { ref = pref(place(peer), 2, 2) })
  local function zero_z(k) return mock.song:pattern(1):track(1):line(k):effect_column(1).number_string .. mock.song:pattern(1):track(1):line(k):effect_column(1).amount_string end
  eq(zero_z(2), "0000", "Renoise cleared the one that named it")
  eq(zero_z(3), "0Z02", "and moved the later one down")
  ok((done.display.note or ""):find("cleared the 1 0Z command", 1, true), tostring(done.display.note))
  local undone = restore(peer, "phrase-delete", record)
  ok(undone.result, json.encode(undone.error or {}))
  eq(zero_z(2), "0Z02", "written again, naming it")
  eq(zero_z(3), "0Z03")
  eq(bass.phrase_program, 2, "the program plays it again")
end)

test("phrases: Kumi's undo of a phrase delete leaves out the 0Z commands of a pattern whose lines changed since, and says so", function()
  local env, peer = setup()
  local bass = mock.song:instrument(2)
  bass:insert_phrase_at(2).name = "P2"
  for k = 1, 2 do
    mock.ops.set_note(1, 1, k, 1, { note = 48, instrument = 1 })
    mock.ops.set_effect(1, 1, k, 1, 256 + 35, k)
  end
  mock.data.undo.stack = {}
  local record = change(peer, "phrase-delete", { ref = pref(place(peer), 2, 2) })
  local function zero_z(k) return mock.song:pattern(1):track(1):line(k):effect_column(1).number_string .. mock.song:pattern(1):track(1):line(k):effect_column(1).amount_string end
  eq(zero_z(2), "0000", "Renoise cleared the one that named it")
  -- (the producer inserts rows in the pattern: the lines move, which the tool sees only as lines that changed)
  mock.ops.set_note(1, 2, 9, 1, { note = 60, instrument = 0 })
  H.tick(2)
  local undone = restore(peer, "phrase-delete", record)
  ok(undone.result, json.encode(undone.error or {}))
  eq(#bass.phrases, 2, "the phrase is back")
  eq(zero_z(2), "0000", "nothing written where the lines may have moved")
  ok((undone.result.display.note or ""):find("weren't written again", 1, true), tostring(undone.result.display.note))
end)

test("phrases: a phrase made at or before the program, and Kumi's undo of a delete below it, leave the program on the phrase it played (#361 N-S23, final review)", function()
  local env, peer = setup()
  local bass = mock.song:instrument(2)
  bass:insert_phrase_at(2).name = "P2"
  bass:insert_phrase_at(3).name = "P3"
  bass.phrase_playback_mode = renoise.Instrument.PHRASES_PLAY_SELECTIVE
  bass.phrase_program = 2
  mock.data.undo.stack = {}
  local function playing() return bass:phrase(bass.phrase_program).name end
  eq(playing(), "P2")
  -- (made at the program's place: Renoise leaves the number, which would name the new phrase)
  local added = change(peer, "phrase-add", { ref = iref(place(peer), 2), index = 2, name = "New" })
  eq(bass:phrase(2).name, "New")
  eq(playing(), "P2", "the program plays the phrase it played")
  ok(restore(peer, "phrase-add", added).result)
  eq(#bass.phrases, 3)
  eq(playing(), "P2")
  -- (a delete below it: Renoise moves the number with it; the phrase put back below leaves the number, naming the one before)
  bass.phrase_program = 3
  local deleted = change(peer, "phrase-delete", { ref = pref(place(peer), 2, 1) })
  eq(playing(), "P3")
  local undone = restore(peer, "phrase-delete", deleted)
  ok(undone.result, json.encode(undone.error or {}))
  eq(#bass.phrases, 3)
  eq(playing(), "P3", "the program plays the phrase it played")
  -- (one the producer chose since stays theirs)
  local again = change(peer, "phrase-delete", { ref = pref(place(peer), 2, 1) })
  bass.phrase_program = 1
  ok(restore(peer, "phrase-delete", again).result)
  eq(bass.phrase_program, 1)
end)

test("phrases: Kumi's undo of a phrase delete writes its 0Z back where its own insert renumbers 0Z commands and tells the lines (#361 N-N17)", function()
  local env, peer = setup()
  local bass = mock.song:instrument(2)
  bass:insert_phrase_at(2).name = "P2"
  bass:insert_phrase_at(3).name = "P3"
  for k = 1, 3 do
    mock.ops.set_note(1, 1, k, 1, { note = 48, instrument = 1 })
    mock.ops.set_effect(1, 1, k, 1, 256 + 35, k)
  end
  mock.data.undo.stack = {}
  mock.renumber_touches = true
  local record = change(peer, "phrase-delete", { ref = pref(place(peer), 2, 2) })
  local undone = restore(peer, "phrase-delete", record)
  mock.renumber_touches = nil
  ok(undone.result, json.encode(undone.error or {}))
  local function zero_z(k) return mock.song:pattern(1):track(1):line(k):effect_column(1).number_string .. mock.song:pattern(1):track(1):line(k):effect_column(1).amount_string end
  eq(zero_z(2), "0Z02", "written again, naming it")
  eq(zero_z(3), "0Z03")
end)

test("phrases: Kumi's undo of a phrase it made puts the program back on its phrase when Renoise's delete moved it off", function()
  local env, peer = setup()
  local bass = mock.song:instrument(2)
  bass:insert_phrase_at(2).name = "P2"
  bass:insert_phrase_at(3).name = "P3"
  bass.phrase_playback_mode = renoise.Instrument.PHRASES_PLAY_SELECTIVE
  bass.phrase_program = 2
  mock.data.undo.stack = {}
  local function playing() return bass:phrase(bass.phrase_program).name end
  local added = change(peer, "phrase-add", { ref = iref(place(peer), 2), name = "New" })
  eq(#bass.phrases, 4)
  eq(playing(), "P2")
  -- (Renoise moves the program as it deletes the phrase above it: 2, delete 4 -> 3, measured)
  mock.program_after_delete = function(program, ph) if program == 2 and ph == 4 then return 3 end end
  local undone = restore(peer, "phrase-add", added)
  mock.program_after_delete = nil
  ok(undone.result, json.encode(undone.error or {}))
  eq(#bass.phrases, 3)
  eq(playing(), "P2", "the program plays the phrase it played")
end)

test("phrases: a new phrase's loop is checked against the 32 lines Renoise makes it with (#361 N-N4)", function()
  local env, peer = setup()
  local record, done = change(peer, "phrase-add", { ref = iref(place(peer), 2), loopEnd = 24 })
  ok(done, "a loop to line 24 fits a new phrase")
  eq(mock.song:instrument(2):phrase(#mock.song:instrument(2).phrases).loop_end, 25)
  failure(call(peer, "change_preview", { kind = "phrase-add", input = { ref = iref(place(peer), 2), loopEnd = 40 }, epoch = place(peer) }), "bad-params")
end)

test("phrases: settings change and go back (lines, LPB, loop, shuffle, key tracking, base note, columns); a loop past its lines is bad-params", function()
  local env, peer = setup()
  local record = change(peer, "phrase-set", { ref = pref(place(peer), 2, 1), lines = 32, lpb = 16, loopStart = 16, loopEnd = 31, shuffle = 0.5, keyTracking = "offset", baseNote = 36, noteColumns = 4, delayColumn = true })
  local data = phrase(2, 1)
  eq(data.lines, 32)
  eq(data.loop_start, 17)
  eq(data.loop_end, 32)
  eq(data.shuffle, 0.5)
  eq(data.key_tracking, 3)
  eq(data.note_columns, 4)
  eq(data.delay_column_visible, true)
  local undone = restore(peer, "phrase-set", record)
  ok(undone.result, json.encode(undone.error or {}))
  eq(data.lines, 16)
  eq(data.loop_start, 1)
  eq(data.loop_end, 16)
  eq(data.note_columns, 1)
  failure(call(peer, "change_preview", { kind = "phrase-set", input = { ref = pref(place(peer), 2, 1), loopEnd = 20 }, epoch = place(peer) }), "bad-params")
end)

test("places, phrase × a phrase made before it by the producer: the undo of its settings follows it; deleted: gone", function()
  local env, peer = setup()
  local record = change(peer, "phrase-set", { ref = pref(place(peer), 2, 1), lpb = 6 })
  mock.song:instrument(2):insert_phrase_at(1).name = "Intro"
  local undone = restore(peer, "phrase-set", record)
  ok(undone.result, json.encode(undone.error or {}))
  eq(phrase(2, 2).lpb, 4)
  record = change(peer, "phrase-set", { ref = pref(place(peer), 2, 2), lpb = 6 })
  mock.song:instrument(2):delete_phrase_at(2)
  failure(restore(peer, "phrase-set", record), "gone")
end)

-- phrase-map, phrase-playback

test("phrases: a phrase mapped to keys, its range changed, unmapped; each goes back; overlapping keys are bad-params", function()
  local env, peer = setup()
  local p2 = mock.song:instrument(2):insert_phrase_at(2)
  mock.song:instrument(2):insert_phrase_mapping_at(1, p2).note_range = { 72, 83 }
  mock.data.undo.stack = {}
  local record = change(peer, "phrase-map", { ref = pref(place(peer), 2, 1), noteRange = { 48, 59 } })
  eq(#instrument(2).mappings, 2)
  eq(instrument(2).mappings[1].note_range[1], 48, "sorted by key: first")
  local undone = restore(peer, "phrase-map", record)
  ok(undone.result, json.encode(undone.error or {}))
  eq(#instrument(2).mappings, 1)
  failure(call(peer, "change_preview", { kind = "phrase-map", input = { ref = pref(place(peer), 2, 1), noteRange = { 70, 75 } }, epoch = place(peer) }), "bad-params")
  record = change(peer, "phrase-map", { ref = pref(place(peer), 2, 2), noteRange = { 60, 64 } })
  eq(instrument(2).mappings[1].note_range[2], 64)
  undone = restore(peer, "phrase-map", record)
  ok(undone.result, json.encode(undone.error or {}))
  eq(instrument(2).mappings[1].note_range[1], 72)
  record = change(peer, "phrase-map", { ref = pref(place(peer), 2, 2), noteRange = json.null })
  eq(#instrument(2).mappings, 0)
  undone = restore(peer, "phrase-map", record)
  ok(undone.result, json.encode(undone.error or {}))
  eq(instrument(2).mappings[1].note_range[2], 83)
end)

test("phrases: the instrument's phrase playback: program mode playing a phrase (kept as its place), keymap, off; each goes back", function()
  local env, peer = setup()
  mock.song:instrument(2):insert_phrase_at(2).name = "B"
  mock.data.undo.stack = {}
  local epoch = place(peer)
  local record, done = change(peer, "phrase-playback", { ref = iref(epoch, 2), mode = "program", programRef = pref(epoch, 2, 2) })
  eq(instrument(2).phrase_playback_mode, 2)
  eq(instrument(2).phrase_program, 2)
  ok(done.display.title:find("plays phrase 02 on every key", 1, true), done.display.title)
  local undone = restore(peer, "phrase-playback", record)
  ok(undone.result, json.encode(undone.error or {}))
  eq(instrument(2).phrase_playback_mode, 2, "program mode, as a new instrument has it")
  eq(instrument(2).phrase_program, 0)
  record = change(peer, "phrase-playback", { ref = iref(place(peer), 2), mode = "keymap" })
  eq(instrument(2).phrase_program, 127)
  ok(restore(peer, "phrase-playback", record).result)
  eq(instrument(2).phrase_playback_mode, 2)
  record = change(peer, "phrase-playback", { ref = iref(place(peer), 2), mode = "off" })
  eq(instrument(2).phrase_playback_mode, 1)
  ok(restore(peer, "phrase-playback", record).result)
  eq(instrument(2).phrase_playback_mode, 2)
  failure(call(peer, "change_preview", { kind = "phrase-playback", input = { ref = iref(place(peer), 1), mode = "program" }, epoch = place(peer) }), "bad-params")
end)

-- phrase-write, phrase-clear

test("phrases: lines written in Renoise's own text, columns shown first; Kumi's undo puts the cells back and hides the columns again", function()
  local env, peer = setup()
  local input = { ref = pref(place(peer), 2, 1), lines = {
    { line = 0, notes = { { col = 1, note = "C-4", instrument = "00", volume = "40" }, { col = 2, note = "G-4", delay = "10" } } },
    { line = 4, notes = { { col = 1, note = "OFF" } }, effects = { { col = 1, number = "0A", amount = "20" } } },
  } }
  local record, done = change(peer, "phrase-write", input, "Kumi: arp (k600)")
  eq(cell(2, 1, 1, 1).note_string, "C-4")
  eq(cell(2, 1, 1, 2).delay_string, "10")
  eq(phrase(2, 1).note_columns, 2, "the second note column shown first")
  eq(phrase(2, 1).delay_column_visible, true)
  eq(steps(), 1)
  eq(newest(), "Kumi: arp (k600)")
  ok(done.display.title:find("Wrote 2 lines", 1, true), done.display.title)
  local undone = restore(peer, "phrase-write", record)
  ok(undone.result, json.encode(undone.error or {}))
  eq(cell(2, 1, 1, 1).note_string, "---")
  eq(phrase(2, 1).note_columns, 1, "hidden again")
  eq(phrase(2, 1).delay_column_visible, false)
end)

test("phrases: lines back already, the columns Kumi showed still shown: Kumi's undo hides them in a step named as its own (#399 N-N3)", function()
  local env, peer = setup()
  local input = { ref = pref(place(peer), 2, 1), lines = {
    { line = 0, notes = { { col = 1, note = "C-4", instrument = "00", volume = "40" }, { col = 2, note = "G-4", delay = "10" } } },
  } }
  local record = change(peer, "phrase-write", input, "Kumi: arp (k601)")
  eq(phrase(2, 1).note_columns, 2)
  -- (the line back as it was with no line notifier told: the tool's count of the phrase's changed lines doesn't move, a
  -- state no edit of the mock's reaches)
  phrase(2, 1).data[1] = nil
  local undone = restore(peer, "phrase-write", record, "Kumi: undo arp (k602)")
  ok(undone.result, json.encode(undone.error or {}))
  ok(undone.result.display.title:find("are back already: the columns Kumi showed are hidden again", 1, true), undone.result.display.title)
  eq(undone.result.undo.entry, true)
  eq(phrase(2, 1).note_columns, 1, "hidden again")
  eq(phrase(2, 1).delay_column_visible, false)
  eq(steps(), 2, "the hiding is one step")
  eq(newest(), "Kumi: undo arp (k602)")
end)

test("phrases: an older sample-offset spelling is written as Renoise's 0Sxx, and the change says so", function()
  local env, peer = setup()
  local input = { ref = pref(place(peer), 2, 1), lines = { { line = 2, effects = { { col = 1, number = "09", amount = "40" } } } } }
  local record, done = change(peer, "phrase-write", input)
  local fx = mock.song:instrument(2):phrase(1):line(3):effect_column(1)
  eq(fx.number_string .. fx.amount_string, "0S40")
  eq(done.display.note, "Read 0940 as 0S40: Renoise's sample offset is 0Sxx.")
  ok(record, "a record")
end)

test("phrases: a line the producer edited anywhere in the phrase since refuses the undo of Kumi's lines (the plain rule), nothing written", function()
  local env, peer = setup()
  local record = change(peer, "phrase-write", { ref = pref(place(peer), 2, 1), lines = { { line = 1, notes = { { col = 1, note = "D-4" } } } } })
  cell(2, 1, 16).note_string = "A-3" -- far below Kumi's line
  local refused = restore(peer, "phrase-write", record)
  failure(refused, "changed")
  eq(cell(2, 1, 2).note_string, "D-4", "left as it is")
  eq(cell(2, 1, 16).note_string, "A-3")
end)

test("phrases: a clear of lines keeps the cells and puts them back; a write too big for one step is bad-params", function()
  local env, peer = setup()
  local p = mock.song:instrument(2):phrase(1)
  p:line(1):note_column(1).note_string = "C-4"
  p:line(4):note_column(1).note_string = "D-4"
  mock.data.undo.stack = {}
  local record = change(peer, "phrase-clear", { ref = pref(place(peer), 2, 1) })
  eq(cell(2, 1, 1).note_string, "---")
  eq(cell(2, 1, 4).note_string, "---")
  local undone = restore(peer, "phrase-clear", record)
  ok(undone.result, json.encode(undone.error or {}))
  eq(cell(2, 1, 4).note_string, "D-4")
  local lines = {}
  for l = 1, 16 do
    local notes = {}
    for c = 1, 12 do notes[c] = { col = c, note = "C-4", instrument = "00", volume = "40", panning = "40", delay = "01", fx = "0A10" } end
    lines[l] = { line = l - 1, notes = notes }
  end
  mock.song:instrument(2):phrase(1).number_of_lines = 16
  local epoch = place(peer)
  local big = call(peer, "change_preview", { kind = "phrase-write", input = { ref = pref(epoch, 2, 1), lines = lines }, epoch = epoch })
  ok(big.result, "16 lines of 12 columns fit: " .. json.encode(big.error or {}))
end)

-- phrase-script, phrase-render

test("phrases: a script written and committed reports Renoise's compile error; Kumi's undo puts the old text and mode back", function()
  local env, peer = setup()
  local previewed, epoch, layout = preview(peer, "phrase-script", { ref = pref(place(peer), 2, 1), code = "return pattern {\n  event = { \"c4\"\n" })
  local id = I.send(peer, "change_apply", { kind = "phrase-script", input = { ref = pref(epoch, 2, 1), code = "return pattern {\n  event = { \"c4\"\n" }, fence = previewed.fence, desc = "Kumi: script (k610)", epoch = epoch, layout = layout })
  mock.advance(60)
  mock.compile_scripts()
  local done = I.wait(peer, id)
  ok(done.result, json.encode(done.error or {}))
  eq(phrase(2, 1).playback_mode, 2)
  eq(phrase(2, 1).script.paragraphs[1], "return pattern {")
  eq(phrase(2, 1).script.committed[2], "  event = { \"c4\"")
  eq(done.result.applied.compileError, json.null, "this one compiles (it has a return)")
  eq(steps(), 1)
  eq(newest(), "Kumi: script (k610)")
  local record = { target = done.result.target, prior = previewed.prior, applied = done.result.applied, epoch = done.result.epoch, layout = done.result.layout }
  local undone = restore(peer, "phrase-script", record)
  ok(undone.result, json.encode(undone.error or {}))
  eq(phrase(2, 1).playback_mode, 1)
  eq(phrase(2, 1).script.paragraphs[1], "-- a phrase script")
  -- a script that doesn't compile
  previewed, epoch, layout = preview(peer, "phrase-script", { ref = pref(place(peer), 2, 1), code = "pattern { }" })
  id = I.send(peer, "change_apply", { kind = "phrase-script", input = { ref = pref(epoch, 2, 1), code = "pattern { }" }, fence = previewed.fence, desc = "Kumi: s (k611)", epoch = epoch, layout = layout })
  mock.advance(60)
  mock.compile_scripts()
  done = I.wait(peer, id)
  ok(done.result, json.encode(done.error or {}))
  ok(type(done.result.applied.compileError) == "string" and done.result.applied.compileError:find("return", 1, true), json.encode(done.result.applied))
  ok(done.result.display.note:find("couldn't compile", 1, true), done.result.display.note)
end)

test("phrases: a script written while the producer changes the song during the compile wait answers applied, and Kumi's undo puts the old one back (#361 N7)", function()
  local env, peer = setup()
  local code = "return cycle(\"c4\")"
  local previewed, epoch, layout = preview(peer, "phrase-script", { ref = pref(place(peer), 2, 1), code = code })
  local id = I.send(peer, "change_apply", { kind = "phrase-script", input = { ref = pref(epoch, 2, 1), code = code }, fence = previewed.fence, desc = "Kumi: script (k620)", epoch = epoch, layout = layout })
  mock.advance(20)
  mock.song:pattern(1).number_of_lines = 48 -- the producer, while Renoise compiles: the song's shape moves
  mock.song:instrument(1):insert_sample_at(1)
  mock.advance(60)
  mock.compile_scripts()
  local done = I.wait(peer, id)
  ok(done.result, "the script is written: " .. json.encode(done.error or {}))
  eq(phrase(2, 1).script.paragraphs[1], code)
  local named = false
  for _, step in ipairs(mock.data.undo.stack) do named = named or step.desc == "Kumi: script (k620)" end
  ok(named, "Kumi's step is in Renoise's undo")
  local record = { target = done.result.target, prior = previewed.prior, applied = done.result.applied, epoch = done.result.epoch, layout = done.result.layout }
  local undone = restore(peer, "phrase-script", record)
  ok(undone.result, json.encode(undone.error or {}))
  eq(phrase(2, 1).script.paragraphs[1], "-- a phrase script", "the producer's script is back")
end)

test("phrases: a script written just before the producer makes an instrument answers applied, though its wait hadn't begun (#361 N7, final review)", function()
  local env, peer = setup()
  local code = "return cycle(\"c4 g4\")"
  local previewed, epoch, layout = preview(peer, "phrase-script", { ref = pref(place(peer), 2, 1), code = code })
  local id = I.send(peer, "change_apply", { kind = "phrase-script", input = { ref = pref(epoch, 2, 1), code = code }, fence = previewed.fence, desc = "Kumi: script (k640)", epoch = epoch, layout = layout })
  eq(phrase(2, 1).script.paragraphs[1], code, "written in the request's own callback")
  -- (before the job's first turn: an instrument made moves the epoch)
  mock.song:insert_instrument_at(4)
  mock.advance(60)
  mock.compile_scripts()
  local done = I.wait(peer, id)
  ok(done.result, "the script is written, and the answer says so: " .. json.encode(done.error or {}))
  eq(done.result.applied.mode, "script")
end)

test("phrases: after Renoise's undo of a script Kumi wrote (its text back, uncommitted), Kumi's undo commits it so the old script plays (#361 N-S8)", function()
  local env, peer = setup()
  local code = "return cycle(\"c4 e4\")"
  local previewed, epoch, layout = preview(peer, "phrase-script", { ref = pref(place(peer), 2, 1), code = code })
  local id = I.send(peer, "change_apply", { kind = "phrase-script", input = { ref = pref(epoch, 2, 1), code = code }, fence = previewed.fence, desc = "Kumi: script (k630)", epoch = epoch, layout = layout })
  mock.advance(60)
  mock.compile_scripts()
  local done = I.wait(peer, id)
  ok(done.result, json.encode(done.error or {}))
  mock.song:undo()
  H.tick(2)
  local script = phrase(2, 1).script
  eq(script.paragraphs[1], "-- a phrase script", "Renoise's undo brought the text back")
  local record = { target = done.result.target, prior = previewed.prior, applied = done.result.applied, epoch = done.result.epoch, layout = done.result.layout }
  local undone = restore(peer, "phrase-script", record)
  ok(undone.result, json.encode(undone.error or {}))
  eq(undone.result.undo.entry, true, "a step of its own")
  mock.compile_scripts()
  eq(script.committed[1], "-- a phrase script", "what plays is the old script")
  ok(undone.result.display.note:find("committed it", 1, true), undone.result.display.note)
end)

test("phrases: a script that fixes the old one's error is told fine even when Renoise compiles it after 200 ms (#361 N-S24)", function()
  local env, peer = setup()
  local function write(code, desc, compile_after)
    local previewed, epoch, layout = preview(peer, "phrase-script", { ref = pref(place(peer), 2, 1), code = code })
    local id = I.send(peer, "change_apply", { kind = "phrase-script", input = { ref = pref(epoch, 2, 1), code = code }, fence = previewed.fence, desc = desc, epoch = epoch, layout = layout })
    mock.advance(compile_after)
    mock.compile_scripts()
    return I.wait(peer, id)
  end
  local broken = write("pattern { }", "Kumi: s (k650)", 60)
  ok(type(broken.result.applied.compileError) == "string", json.encode(broken.result or broken.error))
  local fixed = write("return cycle(\"c4\")", "Kumi: s (k651)", 350) -- Renoise compiles it late (the lab saw up to 0.5 s)
  ok(fixed.result, json.encode(fixed.error or {}))
  eq(fixed.result.applied.compileError, json.null, "the new script's (no) error, not the old one's")
end)

test("phrases: a script rendered to lines; Kumi's undo puts the lines it had back", function()
  local env, peer = setup()
  local p = mock.song:instrument(2):phrase(1)
  p:line(2):note_column(1).note_string = "F-4"
  p.playback_mode = 2
  p.script.paragraphs = { "return cycle(\"c4 e4\")" }
  p.script:commit()
  mock.compile_scripts()
  mock.data.undo.stack = {}
  local previewed, epoch, layout = preview(peer, "phrase-render", { ref = pref(place(peer), 2, 1), lpb = 4 })
  local id = I.send(peer, "change_apply", { kind = "phrase-render", input = { ref = pref(epoch, 2, 1), lpb = 4 }, fence = previewed.fence, desc = "Kumi: render (k620)", epoch = epoch, layout = layout })
  mock.advance(30)
  mock.finish_renders()
  local done = I.wait(peer, id)
  ok(done.result, json.encode(done.error or {}))
  eq(cell(2, 1, 1).note_string, "C-4", "rendered")
  eq(cell(2, 1, 2).note_string, "F-4", "(the mock's render writes every 4th line)")
  local record = { target = done.result.target, prior = previewed.prior, applied = done.result.applied, material = done.result.material, epoch = done.result.epoch, layout = done.result.layout }
  local undone = restore(peer, "phrase-render", record)
  ok(undone.result, json.encode(undone.error or {}))
  eq(cell(2, 1, 1).note_string, "---")
  eq(cell(2, 1, 2).note_string, "F-4")
  eq(phrase(2, 1).playback_mode, 2, "it plays its script again")
  failure(call(peer, "change_preview", { kind = "phrase-render", input = { ref = pref(place(peer), 1, 1) }, epoch = place(peer) }), "gone")
end)

test("phrases: a render Renoise refuses at once fails with its words; after Renoise's own undo of a render Kumi's undo finds it back already (#361 N-N5)", function()
  local env, peer = setup()
  local p = mock.song:instrument(2):phrase(1)
  p.playback_mode = 2
  p.script.paragraphs = { "return cycle(\"c4 e4\")" }
  p.script:commit()
  mock.compile_scripts()
  mock.data.undo.stack = {}
  local function render(desc)
    local previewed, epoch, layout = preview(peer, "phrase-render", { ref = pref(place(peer), 2, 1), lpb = 4 })
    local id = I.send(peer, "change_apply", { kind = "phrase-render", input = { ref = pref(epoch, 2, 1), lpb = 4 }, fence = previewed.fence, desc = desc, epoch = epoch, layout = layout })
    mock.advance(30)
    mock.finish_renders()
    return I.wait(peer, id, 100), previewed
  end
  mock.render_refuses = "the script has no pattern to render"
  local refused = render("Kumi: render (k630)")
  mock.render_refuses = nil
  failure(refused, "failed")
  ok(refused.error.message:find("no pattern to render", 1, true), refused.error.message)
  local done, previewed = render("Kumi: render (k631)")
  ok(done.result, json.encode(done.error or {}))
  mock.song:undo() -- the producer's Cmd-Z of the render
  H.tick(2)
  local undone = restore(peer, "phrase-render", { target = done.result.target, prior = previewed.prior, applied = done.result.applied, material = done.result.material,
    epoch = done.result.epoch, layout = done.result.layout })
  ok(undone.result, json.encode(undone.error or {}))
  ok(undone.result.display.title:find("back already", 1, true), undone.result.display.title)
end)

test("phrases: Renoise's own undo and redo of a render are told (real Renoise: the row said applied after its undo)", function()
  local env, peer = setup()
  local p = mock.song:instrument(2):phrase(1)
  p.playback_mode = 2
  p.script.paragraphs = { "return cycle(\"c4 e4\")" }
  p.script:commit()
  mock.compile_scripts()
  mock.data.undo.stack = {}
  local previewed, epoch, layout = preview(peer, "phrase-render", { ref = pref(place(peer), 2, 1), lpb = 4 })
  local id = I.send(peer, "change_apply", { kind = "phrase-render", input = { ref = pref(epoch, 2, 1), lpb = 4 }, fence = previewed.fence, desc = "Kumi: render (k621)", epoch = epoch, layout = layout })
  mock.advance(30)
  mock.finish_renders()
  ok(I.wait(peer, id).result)
  I.told(peer)
  mock.song:undo()
  eq(phrase(2, 1).playback_mode, 2, "Renoise's undo: it plays its script again")
  local events = I.told(peer)
  eq(#events, 1)
  eq(events[1].data.how, "undo")
  eq(events[1].data.descs[1], "Kumi: render (k621)")
  mock.song:redo()
  events = I.told(peer)
  eq(#events, 1)
  eq(events[1].data.how, "redo")
end)

-- Plug-in and MIDI instruments

local function plugin_catalog()
  mock.plugin_catalog = {
    ["Audio/Generators/AU/aumu:dls :appl"] = { name = "DLSMusicDevice", params = { { name = "Volume", value = 0.5 }, { name = "Tune", value = 0.5 } }, presets = { "Piano", "Strings", "Bass" } },
    ["Audio/Generators/VST3/Synth"] = { name = "Synth", params = { { name = "Cutoff", value = 0.2 } }, presets = {} },
  }
end

test("plugins: a plug-in loaded by name into an instrument; Kumi's undo takes it out again, and puts back one it replaced with its state", function()
  plugin_catalog()
  local env, peer = setup()
  local record, done = change(peer, "plugin-load", { ref = iref(place(peer), 3), path = "DLSMusicDevice" })
  eq(instrument(3).plugin.path, "Audio/Generators/AU/aumu:dls :appl")
  ok(done.display.title:find("DLS", 1, true), done.display.title)
  local undone = restore(peer, "plugin-load", record)
  ok(undone.result, json.encode(undone.error or {}))
  eq(instrument(3).plugin, nil)
  change(peer, "plugin-load", { ref = iref(place(peer), 3), path = "Audio/Generators/VST3/Synth" })
  instrument(3).plugin.preset_data = "<synth state='mine'/>"
  record = change(peer, "plugin-load", { ref = iref(place(peer), 3), path = "DLSMusicDevice" })
  undone = restore(peer, "plugin-load", record)
  ok(undone.result, json.encode(undone.error or {}))
  eq(instrument(3).plugin.path, "Audio/Generators/VST3/Synth")
  eq(instrument(3).plugin.preset_data, "<synth state='mine'/>", "its state too")
  failure(call(peer, "change_preview", { kind = "plugin-load", input = { ref = iref(place(peer), 3), path = "Moog" }, epoch = place(peer) }), "bad-params")
end)

test("plugins: a plug-in load that Renoise refuses after letting the old one go loads the old one back with its state (#361 N-N11)", function()
  plugin_catalog()
  local env, peer = setup()
  change(peer, "plugin-load", { ref = iref(place(peer), 3), path = "DLSMusicDevice" })
  instrument(3).plugin.preset_data = "<dls mine/>"
  mock.plugin_unloads_and_fails = "Audio/Generators/VST3/Synth"
  local epoch = place(peer)
  local input = { ref = iref(epoch, 3), path = "Audio/Generators/VST3/Synth" }
  local previewed = preview(peer, "plugin-load", input)
  local refused = apply(peer, "plugin-load", input, "Kumi: load (k660)", previewed.fence, epoch)
  mock.plugin_unloads_and_fails = nil
  failure(refused, "failed")
  ok(instrument(3).plugin and instrument(3).plugin.path == "Audio/Generators/AU/aumu:dls :appl", "the old plug-in is back")
  eq(instrument(3).plugin.preset_data, "<dls mine/>", "with its state")
end)

test("plugins: a plug-in's parameter by name or by its ref, its program, its settings; each goes back", function()
  plugin_catalog()
  local env, peer = setup()
  change(peer, "plugin-load", { ref = iref(place(peer), 3), path = "DLSMusicDevice" })
  mock.data.undo.stack = {}
  local epoch = place(peer)
  local param = change(peer, "plugin-param", { ref = iref(epoch, 3), param = "Tune", value = 0.75 })
  eq(instrument(3).plugin.params[2].value, 0.75)
  local by_ref = change(peer, "plugin-param", { ref = place(peer) .. ":pluginparam:3.1", value = 0.1 })
  eq(instrument(3).plugin.params[1].value, 0.1)
  ok(restore(peer, "plugin-param", by_ref).result)
  ok(restore(peer, "plugin-param", param).result)
  eq(instrument(3).plugin.params[2].value, 0.5)
  instrument(3).plugin.preset_data = "<dls before/>"
  local preset = change(peer, "plugin-preset", { ref = iref(place(peer), 3), preset = "Strings" })
  eq(instrument(3).plugin.preset, 2)
  local undone = restore(peer, "plugin-preset", preset)
  ok(undone.result, json.encode(undone.error or {}))
  eq(instrument(3).plugin.preset, 0)
  eq(instrument(3).plugin.preset_data, "<dls before/>")
  -- A plug-in that keeps its program: nothing changed, no step and no record.
  instrument(3).plugin.keeps_program = true
  local steps_before = #mock.data.undo.stack
  local kept = apply(peer, "plugin-preset", { ref = iref(place(peer), 3), preset = "Bass" }, "Kumi: program (k640)",
    preview(peer, "plugin-preset", { ref = iref(place(peer), 3), preset = "Bass" }).fence, place(peer))
  ok(kept.result and kept.result.display.unchanged and kept.result.undo.entry == false, json.encode(kept.result or kept.error))
  eq(#mock.data.undo.stack, steps_before)
  instrument(3).plugin.keeps_program = nil
  local settings = change(peer, "plugin-set", { ref = iref(place(peer), 3), channel = 2, transpose = -12, volumeDb = -6 })
  eq(instrument(3).plugin_props.channel, 2)
  ok(restore(peer, "plugin-set", settings).result)
  eq(instrument(3).plugin_props.channel, 1)
  failure(call(peer, "change_preview", { kind = "plugin-param", input = { ref = iref(place(peer), 1), param = 1, value = 0 }, epoch = place(peer) }), "bad-params")
end)

test("plugins: Kumi's undo of a parameter or a program never writes into another plug-in loaded since, discard or not (#361 N5)", function()
  plugin_catalog()
  local env, peer = setup()
  change(peer, "plugin-load", { ref = iref(place(peer), 3), path = "DLSMusicDevice" })
  mock.data.undo.stack = {}
  local param = change(peer, "plugin-param", { ref = iref(place(peer), 3), param = 1, value = 1 })
  instrument(3).plugin.preset_data = "<dls before/>"
  local preset = change(peer, "plugin-preset", { ref = iref(place(peer), 3), preset = "Strings" })
  -- the producer loads another plug-in into the instrument; its first parameter happens to read what Kumi wrote
  mock.song:instrument(3).plugin_properties:load_plugin("Audio/Generators/VST3/Synth")
  instrument(3).plugin.params[1].value = 1
  instrument(3).plugin.preset_data = "<synth patch/>"
  local before = steps()
  for _, more in ipairs({ {}, { discard = true } }) do
    failure(restore(peer, "plugin-param", param, nil, more), "changed")
    failure(restore(peer, "plugin-preset", preset, nil, more), "changed")
  end
  eq(instrument(3).plugin.params[1].value, 1, "the new plug-in's parameter untouched")
  eq(instrument(3).plugin.preset_data, "<synth patch/>", "and its state")
  eq(steps(), before)
end)

test("plugins: auto-suspend is no step of Renoise's undo (lab): Kumi says so, makes no step, and puts it back itself; refused once changed", function()
  plugin_catalog()
  local env, peer = setup()
  change(peer, "plugin-load", { ref = iref(place(peer), 3), path = "DLSMusicDevice" })
  mock.data.undo.stack = {}
  local record, done = change(peer, "plugin-suspend", { ref = iref(place(peer), 3), autoSuspend = true })
  eq(instrument(3).plugin_props.auto_suspend, true)
  eq(done.undo.entry, false)
  ok(done.display.note and done.display.note:find("Renoise's own undo won't take this back", 1, true), json.encode(done.display))
  eq(steps(), 0, "no step in Renoise")
  local undone = restore(peer, "plugin-suspend", record)
  ok(undone.result, json.encode(undone.error or {}))
  eq(undone.result.undo.entry, false)
  eq(instrument(3).plugin_props.auto_suspend, false)
  eq(steps(), 0)
  record = change(peer, "plugin-suspend", { ref = iref(place(peer), 3), autoSuspend = true })
  -- (the plug-in replaced since: its auto-suspend is another's)
  change(peer, "plugin-load", { ref = iref(place(peer), 3), path = "Audio/Generators/VST3/Synth" })
  instrument(3).plugin_props.auto_suspend = true
  ok(restore(peer, "plugin-suspend", record).result, "the same setting, whoever's plug-in: put back")
  eq(instrument(3).plugin_props.auto_suspend, false)
  record = change(peer, "plugin-suspend", { ref = iref(place(peer), 3), autoSuspend = true })
  instrument(3).plugin_props.auto_suspend = false
  local back = restore(peer, "plugin-suspend", record)
  ok(back.result, json.encode(back.error or {}))
  ok(back.result.display.note and back.result.display.note:find("back already", 1, true), json.encode(back.result.display))
  failure(call(peer, "change_preview", { kind = "plugin-suspend", input = { ref = iref(place(peer), 3), autoSuspend = "yes" }, epoch = place(peer) }), "bad-params")
end)

test("plugins: Renoise names an instrument after its plug-in and unnames it as it goes (lab): a name the producer gave stays, Kumi's undo puts each back", function()
  plugin_catalog()
  local env, peer = setup()
  instrument(3).name = "Lead"
  local record, done = change(peer, "plugin-load", { ref = iref(place(peer), 3), path = "DLSMusicDevice" })
  eq(instrument(3).name, "Lead", "the producer's name for an instrument with no plug-in stays")
  ok(restore(peer, "plugin-load", record).result)
  eq(instrument(3).name, "Lead")
  instrument(3).name = ""
  record = change(peer, "plugin-load", { ref = iref(place(peer), 3), path = "DLSMusicDevice" })
  eq(instrument(3).name, "AU: DLSMusicDevice", "an unnamed one takes the plug-in's name, as Renoise gives it")
  local swapped, swap_done = change(peer, "plugin-load", { ref = iref(place(peer), 3), path = "Audio/Generators/VST3/Synth" })
  eq(instrument(3).name, "VST3: Synth", "a plug-in's name follows the plug-in")
  ok(swap_done.display.note:find("after its plug-in", 1, true), swap_done.display.note)
  local out, out_done = change(peer, "plugin-load", { ref = iref(place(peer), 3), path = "" })
  eq(instrument(3).name, "", "taken out: unnamed, as Renoise leaves it")
  ok(out_done.display.note:find("left the instrument unnamed", 1, true), out_done.display.note)
  ok(restore(peer, "plugin-load", out).result)
  eq(instrument(3).plugin.path, "Audio/Generators/VST3/Synth")
  eq(instrument(3).name, "VST3: Synth", "the name it had with its plug-in")
  ok(restore(peer, "plugin-load", swapped).result)
  eq(instrument(3).name, "AU: DLSMusicDevice")
  ok(restore(peer, "plugin-load", record).result)
  eq(instrument(3).name, "")
  eq(instrument(3).plugin, nil)
end)

test("phrases: switched to its script, a phrase keeps its name (Renoise would name it Phrase 01: lab), through Kumi's script, settings and undo", function()
  local env, peer = setup()
  phrase(2, 1).name = "Riff"
  local record = change(peer, "phrase-script", { ref = pref(place(peer), 2, 1), code = "return cycle(\"c4\")" })
  eq(phrase(2, 1).playback_mode, 2)
  eq(phrase(2, 1).name, "Riff")
  ok(restore(peer, "phrase-script", record).result)
  eq(phrase(2, 1).name, "Riff")
  local set = change(peer, "phrase-set", { ref = pref(place(peer), 2, 1), playback = "script" })
  eq(phrase(2, 1).name, "Riff")
  ok(restore(peer, "phrase-set", set).result)
  -- (Renoise's own switch renames it: the mock does as Renoise)
  mock.song:instrument(2):phrase(1).playback_mode = 2
  eq(phrase(2, 1).name, "Phrase 01")
end)

test("midi: a MIDI instrument's device, channel, transpose, program, bank, delay and duration change and go back; an unknown device is bad-params", function()
  mock.midi_outputs = { "IAC Driver Bus 1" }
  local env, peer = setup()
  local record = change(peer, "midi-set", { ref = iref(place(peer), 3), device = "IAC Driver Bus 1", channel = 10, transpose = 12, program = 5, bank = 2, delay = 20, duration = 500 })
  local midi = instrument(3).midi
  eq(midi.device_name, "IAC Driver Bus 1")
  eq(midi.channel, 10)
  eq(midi.program, 5)
  eq(midi.duration, 500)
  local undone = restore(peer, "midi-set", record)
  ok(undone.result, json.encode(undone.error or {}))
  eq(midi.device_name, "")
  eq(midi.channel, 1)
  eq(midi.duration, 8000)
  failure(call(peer, "change_preview", { kind = "midi-set", input = { ref = iref(place(peer), 3), device = "Nope" }, epoch = place(peer) }), "bad-params")
  mock.midi_outputs = nil
end)
