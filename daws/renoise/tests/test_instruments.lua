-- Kumi's changes to Renoise's instruments (M4, stream n-instruments): each kind applied, put back by Kumi's undo, refused when
-- the producer changed it in between, when it is gone, written nothing when it is back already; the fences; and the places a
-- record keeps followed through the instruments (and what is in them) made, deleted and swapped since.
local T = ...
local mock = T.mock
local json = T.load_payload_module("json")
local test, eq, ok = T.test, T.eq, T.ok
local H = T.harness

local next_id = 90000

local function drain(peer)
  repeat
    local taken = peer:take()
    if #taken > 0 then H.send(peer, { type = "credit", bytes = #taken }) end
  until #taken == 0
end

local function wait(peer, id, ticks)
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

local function send(peer, method, params)
  next_id = next_id + 1
  H.send(peer, { type = "request", id = next_id, method = method, params = params or {} })
  return next_id
end

local function call(peer, method, params, ticks) return wait(peer, send(peer, method, params), ticks) end

-- A song of four tracks with the mixer device, and three instruments: Drums (3 samples), Bass synth (1 sample, 1 phrase),
-- Pad (empty).
local function song_options(extra)
  local spec = {
    name = "Lab", file_name = "/s/lab.xrns", bpm = 120, lpb = 4, mixer_device = true,
    tracks = { { name = "Kick" }, { name = "Hat" }, { name = "Bass", note_columns = 2 }, { name = "Pad" } },
    send_tracks = 1, patterns = 3, pattern_lines = 32, sequence_length = 3, instruments = 3,
    instrument_specs = {
      { name = "Drums", sample_specs = { { name = "Kick", frames = 1000 }, { name = "Snare", frames = 800 }, { name = "Hat", frames = 300 } } },
      { name = "Bass synth", sample_specs = { { name = "Saw", frames = 5000 } }, phrases = 1 },
      { name = "Pad" },
    },
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
  mock.dialogs = 0
  return env, peer
end

local function place(peer)
  local focus = call(peer, "focus").result
  return focus.epoch, focus.layout
end

local function steps() return #mock.data.undo.stack end
local function newest() local top = mock.data.undo.stack[#mock.data.undo.stack] return top and top.desc end

local function preview(peer, kind, input)
  local epoch, layout = place(peer)
  local response = call(peer, "change_preview", { kind = kind, input = input, epoch = epoch, layout = layout })
  ok(response and response.result, kind .. " preview: " .. json.encode(response and response.error or {}))
  return response.result, epoch, layout
end

local function apply(peer, kind, input, desc, fence, epoch, layout)
  return call(peer, "change_apply", { kind = kind, input = input, fence = fence, desc = desc, epoch = epoch, layout = layout })
end

-- Preview then apply, as Kumi does; answers the record (target, prior, applied, created, material) and the apply's answer.
local function change(peer, kind, input, desc)
  local previewed, epoch, layout = preview(peer, kind, input)
  local done = apply(peer, kind, input, desc or ("Kumi: " .. kind .. " (k" .. kind:len() .. ")"), previewed.fence, epoch, layout)
  ok(done and done.result, kind .. " apply: " .. json.encode(done and done.error or {}))
  -- (the tool's ticks run before the next request, as they do in Renoise: what a notifier asked of them is done)
  H.tick(2)
  local record = {
    target = done.result.target or previewed.target, prior = previewed.prior, applied = done.result.applied, created = done.result.created,
    material = done.result.material, epoch = done.result.epoch, layout = done.result.layout,
  }
  return record, done.result, previewed
end

local function restore(peer, kind, record, desc, more)
  local params = {
    kind = kind, target = record.target, prior = record.prior, applied = record.applied, created = record.created,
    material = record.material, desc = desc or ("Kumi: undo " .. kind .. " (ku1)"), epoch = record.epoch, layout = record.layout,
  }
  for k, v in pairs(more or {}) do params[k] = v end
  return call(peer, "change_restore", params, 800)
end

-- The undone_in_daw events told since the last look.
local function told(peer)
  H.tick(3)
  drain(peer)
  return H.events(peer, "undone_in_daw")
end

local function failure(response, code)
  ok(response and response.error, "expected an error, got " .. json.encode(response and response.result or "nothing"))
  eq(response.error.code, code, "error code (" .. tostring(response.error.message) .. ")")
end

local function instrument(i) return mock.data.instruments[i] end
local function names()
  local out = {}
  for i, data in ipairs(mock.data.instruments) do out[i] = data.name end
  return table.concat(out, ",")
end

local function iref(epoch, i) return epoch .. ":instrument:" .. i end
local function sref(epoch, i, s) return epoch .. ":sample:" .. i .. "." .. s end

-- A note in pattern 1 on the Bass track (3) that plays instrument value `v`, at line `l`.
local function note_at(l, v, col)
  local column = mock.song:pattern(1):track(3):line(l):note_column(col or 1)
  column.note_value, column.instrument_value, column.volume_value = 48, v, 64
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
end
local function note_value(l, col) return mock.song:pattern(1):track(3):line(l):note_column(col or 1).instrument_value end

T.instruments = {
  setup = setup, place = place, preview = preview, apply = apply, change = change, restore = restore, failure = failure, call = call,
  steps = steps, newest = newest, instrument = instrument, names = names, iref = iref, sref = sref, note_at = note_at, note_value = note_value,
  send = send, wait = wait, told = told,
}

-- instrument-rename

test("instruments: a rename is one named step; Kumi's undo puts the name back as a new step", function()
  local env, peer = setup()
  local epoch = place(peer)
  local record, done = change(peer, "instrument-rename", { ref = iref(epoch, 2), name = "Acid" }, "Kumi: rename (k100)")
  eq(instrument(2).name, "Acid")
  eq(steps(), 1)
  eq(newest(), "Kumi: rename (k100)")
  eq(done.display.title, "Renamed instrument \226\128\156Bass synth\226\128\157 \226\134\146 \226\128\156Acid\226\128\157")
  eq(done.target.kind, "instrument")
  ok(done.target.place and done.target.place.of == "instrument", "the record keeps the instrument's place")
  local undone = restore(peer, "instrument-rename", record, "Kumi: undo rename (k101)")
  ok(undone.result, json.encode(undone.error or {}))
  eq(instrument(2).name, "Bass synth")
  eq(steps(), 2)
  eq(newest(), "Kumi: undo rename (k101)")
  local again = restore(peer, "instrument-rename", record, "Kumi: undo rename (k102)")
  ok(again.result, json.encode(again.error or {}))
  eq(steps(), 2, "back already: nothing written")
  ok(again.result.display.note:find("back already", 1, true), again.result.display.note)
  H.no_dialogs()
end)

test("instruments: a rename refuses changed when the producer renamed it between the preview and the apply (the fence)", function()
  local env, peer = setup()
  local previewed, epoch, layout = preview(peer, "instrument-rename", { ref = iref(place(peer), 1), name = "Kit" })
  mock.ops.rename_instrument(1, "Drums 2")
  failure(apply(peer, "instrument-rename", { ref = iref(epoch, 1), name = "Kit" }, "Kumi: r (k1)", previewed.fence, epoch, layout), "changed")
  eq(instrument(1).name, "Drums 2")
  eq(steps(), 0)
end)

test("instruments: Kumi's undo of a rename refuses changed after the producer renamed it, and discard writes it back", function()
  local env, peer = setup()
  local record = change(peer, "instrument-rename", { ref = iref(place(peer), 1), name = "Kit" })
  mock.ops.rename_instrument(1, "Mine")
  failure(restore(peer, "instrument-rename", record), "changed")
  eq(instrument(1).name, "Mine")
  local forced = restore(peer, "instrument-rename", record, nil, { discard = true })
  ok(forced.result, json.encode(forced.error or {}))
  eq(instrument(1).name, "Drums")
end)

test("places, instrument × insert, delete before, swap: Kumi's undo follows the instrument; deleted: gone; untold: stale", function()
  for _, how in ipairs({ "insert", "delete before", "swap", "Kumi's own insert" }) do
    local env, peer = setup()
    local record = change(peer, "instrument-rename", { ref = iref(place(peer), 2), name = "Acid" })
    local at
    if how == "insert" then
      mock.ops.insert_instrument_at(1, { name = "New" })
      at = 3
    elseif how == "delete before" then
      mock.ops.remove_instrument(1)
      at = 1
    elseif how == "swap" then
      mock.ops.swap_instruments(2, 3)
      at = 3
    else
      change(peer, "instrument-add", { index = 0, name = "Made" })
      at = 3
    end
    eq(instrument(at).name, "Acid", how)
    local undone = restore(peer, "instrument-rename", record)
    ok(undone.result, how .. ": " .. json.encode(undone.error or {}))
    eq(instrument(at).name, "Bass synth", how)
  end
  local env, peer = setup()
  local record = change(peer, "instrument-rename", { ref = iref(place(peer), 2), name = "Acid" })
  mock.ops.remove_instrument(2)
  local before = steps()
  failure(restore(peer, "instrument-rename", record), "gone")
  eq(steps(), before)
  env, peer = setup({ patch = function(env_)
    local path = env_.bundle .. "payload/places.lua"
    local f = assert(io.open(path, "rb"))
    local text = f:read("*a")
    f:close()
    text = text:gsub("M%.KEPT = 256", "M.KEPT = 2")
    f = assert(io.open(path, "wb"))
    f:write(text)
    f:close()
  end })
  record = change(peer, "instrument-rename", { ref = iref(place(peer), 2), name = "Acid" })
  for _ = 1, 3 do mock.ops.swap_instruments(1, 3) end
  before = steps()
  failure(restore(peer, "instrument-rename", record), "stale")
  eq(steps(), before)
  eq(instrument(2).name, "Acid", "nothing written")
end)

test("instruments: the producer's undo of Kumi's rename is told as undone in Renoise, also after an instrument was made before it", function()
  local env, peer = setup()
  change(peer, "instrument-rename", { ref = iref(place(peer), 2), name = "Acid" }, "Kumi: rename (k200)")
  mock.ops.insert_instrument_at(1, { name = "New" }) -- (a step of its own)
  told(peer)
  mock.song:undo() -- the insert
  mock.song:undo() -- Kumi's rename
  local events = told(peer)
  eq(#events, 1)
  eq(events[1].data.descs[1], "Kumi: rename (k200)")
  eq(events[1].data.how, "undo")
  mock.song:redo()
  events = told(peer)
  eq(#events, 1)
  eq(events[1].data.how, "redo")
end)

-- instrument-set, instrument-macro

test("instruments: an instrument's volume, transpose, trigger quantize, mono and glide, scale and key change and go back", function()
  local env, peer = setup()
  local input = { ref = iref(place(peer), 2), volumeDb = -6, transpose = 12, quantize = "beat", monophonic = true, glide = 40, scale = "Dorian", scaleKey = "D" }
  local record, done = change(peer, "instrument-set", input)
  local data = instrument(2)
  ok(math.abs(data.volume - 10 ^ (-6 / 20)) < 1e-6)
  eq(data.transpose, 12)
  eq(data.trigger.quantize, 3)
  eq(data.trigger.monophonic, true)
  eq(data.trigger.monophonic_glide, 40)
  eq(data.trigger.scale_mode, "Dorian")
  eq(data.trigger.scale_key, 3)
  ok(done.display.title:find("volume \226\136\1466 dB", 1, true), done.display.title)
  eq(steps(), 1)
  local undone = restore(peer, "instrument-set", record)
  ok(undone.result, json.encode(undone.error or {}))
  eq(data.volume, 1)
  eq(data.transpose, 0)
  eq(data.trigger.quantize, 1)
  eq(data.trigger.monophonic, false)
  eq(data.trigger.scale_mode, "None")
  eq(data.trigger.scale_key, 1)
  failure(call(peer, "change_preview", { kind = "instrument-set", input = { ref = iref(place(peer), 2), scale = "Klingon" }, epoch = place(peer) }), "bad-params")
  failure(call(peer, "change_preview", { kind = "instrument-set", input = { ref = iref(place(peer), 2), volumeDb = 12 }, epoch = place(peer) }), "bad-params")
end)

test("instruments: a macro's value and name, and the pitch-bend macro, change and go back; the producer's turn of it refuses", function()
  local env, peer = setup()
  local epoch = place(peer)
  local record, done = change(peer, "instrument-macro", { ref = iref(epoch, 1), macro = 3, value = 0.25, name = "Decay" })
  eq(instrument(1).macros[3].value, 0.25)
  eq(instrument(1).macros[3].name, "Decay")
  eq(done.display.title, "Drums macro 3 25%")
  local undone = restore(peer, "instrument-macro", record)
  ok(undone.result, json.encode(undone.error or {}))
  eq(instrument(1).macros[3].value, 0.5, "Renoise's default")
  eq(instrument(1).macros[3].name, "Macro 3")
  local bend = change(peer, "instrument-macro", { ref = iref(place(peer), 1), macro = "pitchbend", value = 0.75 })
  eq(instrument(1).pitchbend.value, 0.75)
  mock.ops.set_macro(1, 3, 0.9) -- (another macro: no bearing)
  instrument(1).pitchbend.value = 0.6 -- the producer turns it
  failure(restore(peer, "instrument-macro", bend), "changed")
  eq(instrument(1).pitchbend.value, 0.6)
  failure(call(peer, "change_preview", { kind = "instrument-macro", input = { ref = iref(place(peer), 1), macro = 9, value = 0.1 }, epoch = place(peer) }), "bad-params")
end)

-- instrument-add

test("instruments: an instrument added at 1 renumbers the notes after it (Renoise's own doing); Kumi's undo takes it away", function()
  local env, peer = setup()
  note_at(1, 1) -- plays Bass synth (value 1)
  local record, done = change(peer, "instrument-add", { index = 0, name = "Lead" })
  eq(names(), "Lead,Drums,Bass synth,Pad")
  eq(note_value(1), 2, "the note still plays Bass synth")
  eq(done.created.kind, "instrument")
  ok(done.created.ref:match(":instrument:1$"), done.created.ref)
  eq(steps(), 1)
  local undone = restore(peer, "instrument-add", record)
  ok(undone.result, json.encode(undone.error or {}))
  eq(names(), "Drums,Bass synth,Pad")
  eq(note_value(1), 1)
  failure(restore(peer, "instrument-add", record), "gone")
end)

test("instruments: Kumi won't take away an instrument it made once the producer filled it or plays it (discard does)", function()
  local env, peer = setup()
  local record = change(peer, "instrument-add", { name = "Lead" })
  mock.ops.insert_sample(4, { name = "Mine", frames = 10 })
  failure(restore(peer, "instrument-add", record), "changed")
  eq(#mock.data.instruments, 4)
  env, peer = setup()
  record = change(peer, "instrument-add", { name = "Lead" })
  note_at(5, 3) -- the producer plays the new instrument
  local refused = restore(peer, "instrument-add", record)
  failure(refused, "changed")
  ok(refused.error.message:find("play", 1, true), refused.error.message)
  eq(#mock.data.instruments, 4)
  local forced = restore(peer, "instrument-add", record, nil, { discard = true })
  ok(forced.result, json.encode(forced.error or {}))
  eq(#mock.data.instruments, 3)
end)

test("instruments: the producer's undo of Kumi's add is told, and Kumi's undo after it is gone", function()
  local env, peer = setup()
  local record = change(peer, "instrument-add", { index = 1 }, "Kumi: add (k300)")
  told(peer)
  mock.song:undo()
  local events = told(peer)
  eq(#events, 1)
  eq(events[1].data.descs[1], "Kumi: add (k300)")
  failure(restore(peer, "instrument-add", record), "gone")
end)

-- instrument-delete

test("instruments: a delete keeps the instrument and the notes Renoise clears; Kumi's undo brings both back where they were", function()
  local env, peer = setup()
  note_at(1, 1)
  note_at(2, 2)
  local record, done = change(peer, "instrument-delete", { ref = iref(place(peer), 2) }, "Kumi: delete (k400)")
  eq(names(), "Drums,Pad")
  eq(note_value(1), 255, "Renoise cleared the note that played it")
  eq(note_value(2), 1, "the Pad note follows Pad")
  ok(done.material.file and io.open(done.material.file, "rb"), "the instrument is kept in a file")
  eq(#done.material.notes, 1)
  ok(done.display.note:find("cleared the 1 note", 1, true), done.display.note)
  eq(steps(), 1)
  local undone = restore(peer, "instrument-delete", record, "Kumi: undo delete (k401)")
  ok(undone.result, json.encode(undone.error or {}))
  eq(names(), "Drums,Bass synth,Pad")
  eq(instrument(2).samples[1].name, "Saw")
  eq(instrument(2).samples[1].buffer.frames, 5000)
  eq(#instrument(2).phrases, 1)
  eq(note_value(1), 1, "its note plays it again")
  eq(note_value(2), 2)
  eq(steps(), 2)
  local again = restore(peer, "instrument-delete", record, "Kumi: undo delete (k402)")
  ok(again.result, json.encode(again.error or {}))
  eq(steps(), 2, "back already")
  H.no_dialogs()
end)

test("places, instrument delete × the producer's insert before, and the producer's undo of the delete", function()
  local env, peer = setup()
  local record = change(peer, "instrument-delete", { ref = iref(place(peer), 2) })
  mock.ops.insert_instrument_at(1, { name = "New" })
  local undone = restore(peer, "instrument-delete", record)
  ok(undone.result, json.encode(undone.error or {}))
  eq(names(), "New,Drums,Bass synth,Pad", "after the instrument that was before it")
  env, peer = setup()
  record = change(peer, "instrument-delete", { ref = iref(place(peer), 2) }, "Kumi: delete (k410)")
  told(peer)
  mock.song:undo()
  eq(names(), "Drums,Bass synth,Pad")
  local events = told(peer)
  eq(#events, 1)
  eq(events[1].data.descs[1], "Kumi: delete (k410)", "the producer's undo is told")
  local before = steps()
  local again = restore(peer, "instrument-delete", record)
  ok(again.result, json.encode(again.error or {}))
  eq(steps(), before, "back already: nothing written")
end)

test("instruments: a note the producer wrote where Kumi's kept note was isn't overwritten by the undo of a delete", function()
  local env, peer = setup()
  note_at(1, 1)
  local record = change(peer, "instrument-delete", { ref = iref(place(peer), 2) })
  local column = mock.song:pattern(1):track(3):line(1):note_column(1)
  column.note_value, column.instrument_value = 50, 0 -- the producer's note, playing Drums
  local undone = restore(peer, "instrument-delete", record)
  ok(undone.result, json.encode(undone.error or {}))
  eq(note_value(1), 0, "the producer's note stays")
  ok(undone.result.display.note:find("weren't put back", 1, true), undone.result.display.note)
end)

test("instruments: Kumi's undo of a delete leaves out the notes of a pattern whose lines changed since (rows may have moved), and says so", function()
  local env, peer = setup()
  note_at(1, 1)
  note_at(3, 1)
  local record = change(peer, "instrument-delete", { ref = iref(place(peer), 2) })
  -- (the producer inserts rows in the pattern: Renoise moves every line, Kumi's cleared cells too, which the tool sees only as
  -- lines that changed)
  mock.ops.set_note(1, 3, 9, 1, { note = 60, instrument = 0 })
  H.tick(2)
  local undone = restore(peer, "instrument-delete", record)
  ok(undone.result, json.encode(undone.error or {}))
  eq(names(), "Drums,Bass synth,Pad", "the instrument is back")
  eq(note_value(1), 255, "nothing written where the lines may have moved")
  eq(note_value(3), 255)
  ok(undone.result.display.note:find("2 weren't put back", 1, true), undone.result.display.note)
end)

test("instruments: Kumi's undo of a delete puts its notes back where its own insert renumbers notes and tells the lines (#361 N-N17)", function()
  local env, peer = setup()
  note_at(1, 1)
  note_at(3, 2) -- (Pad's note: Kumi's insert renumbers it)
  local record = change(peer, "instrument-delete", { ref = iref(place(peer), 2) })
  local undone = restore(peer, "instrument-delete", record)
  ok(undone.result, json.encode(undone.error or {}))
  eq(note_value(1), 1, "its note plays it again")
  eq(note_value(3), 2, "Pad's note follows Pad")
  ok(not (undone.result.display.note or ""):find("weren't put back", 1, true), tostring(undone.result.display.note))
end)

test("instruments: an instrument too big to keep isn't deleted, and the only instrument can't be", function()
  local env, peer = setup()
  instrument(2).samples[1].buffer.frames = 200 * 1024 * 1024
  local previewed, epoch, layout = preview(peer, "instrument-delete", { ref = iref(place(peer), 2) })
  ok(previewed.display.note:find("more than Kumi can keep", 1, true), previewed.display.note)
  failure(apply(peer, "instrument-delete", { ref = iref(epoch, 2) }, "Kumi: d (k1)", previewed.fence, epoch, layout), "failed")
  eq(#mock.data.instruments, 3)
  eq(steps(), 0)
  env, peer = setup(nil, { instruments = 1, instrument_specs = { { name = "Only" } } })
  failure(call(peer, "change_preview", { kind = "instrument-delete", input = { ref = iref(place(peer), 1) }, epoch = place(peer) }), "bad-params")
end)

-- A setup whose payload has `from` replaced with `to` in `file` (a cap lowered for a test).
local function patched(file, from, to)
  return setup({ patch = function(env_)
    local path = env_.bundle .. "payload/" .. file
    local f = assert(io.open(path, "rb"))
    local text = f:read("*a")
    f:close()
    local changed, count = text:gsub(from, to)
    assert(count == 1, from .. " isn't in " .. file)
    f = assert(io.open(path, "wb"))
    f:write(changed)
    f:close()
  end })
end

test("instruments: a delete whose notes are more than its undo can write back, or too many to keep, refuses before anything is written (#361 N1, N16)", function()
  for _, cap in ipairs({ { "changes_instruments.lua", "M%.MAX_KEPT_NOTES = 3500", "M.MAX_KEPT_NOTES = 1", "put back in one go" },
    { "changes_base.lua", "M%.MAX_MATERIAL_BYTES = 8 %* 1024 %* 1024", "M.MAX_MATERIAL_BYTES = 200", "too many notes" } }) do
    local env, peer = patched(cap[1], cap[2], cap[3])
    note_at(1, 1)
    note_at(5, 1)
    local previewed, epoch, layout = preview(peer, "instrument-delete", { ref = iref(place(peer), 2) })
    local refused = apply(peer, "instrument-delete", { ref = iref(epoch, 2) }, "Kumi: d (k1)", previewed.fence, epoch, layout)
    failure(refused, "failed")
    ok(refused.error.message:find(cap[4], 1, true), refused.error.message)
    ok(refused.error.message:find("delete it in Renoise", 1, true), "it says what the producer can do: " .. refused.error.message)
    eq(#mock.data.instruments, 3, "the instrument is still there")
    eq(note_value(1), 1, "its notes too")
    eq(steps(), 0, "nothing written")
  end
end)

test("instruments: in a big song Kumi's undo of an instrument it made looks for its notes in slices; notes refuse it, discard says it cleared them (#361 N10)", function()
  local env, peer = patched("changes_instruments.lua", "M%.SCAN_BUDGET = 300000", "M.SCAN_BUDGET = 10")
  local record = change(peer, "instrument-add", { index = 3, name = "Lead" })
  note_at(1, 3) -- the producer plays it (its instrument value is 3: Renoise's 03)
  note_at(5, 3)
  local refused = restore(peer, "instrument-add", record)
  failure(refused, "changed")
  ok(refused.error.message:find("2 notes", 1, true), refused.error.message)
  ok(not refused.error.message:find("discard", 1, true), "it doesn't point to discard: " .. refused.error.message)
  eq(#mock.data.instruments, 4, "Lead stays")
  local before = steps()
  local forced = restore(peer, "instrument-add", record, nil, { discard = true })
  ok(forced.result, json.encode(forced.error or {}))
  ok((forced.result.display.note or ""):find("cleared the 2 notes", 1, true), "discard says what it cost: " .. tostring(forced.result.display.note))
  eq(#mock.data.instruments, 3)
  eq(steps(), before + 1, "one step")
  -- nothing plays it: the sliced look ends in the undo
  local plain = change(peer, "instrument-add", { name = "Empty" })
  local undone = restore(peer, "instrument-add", plain)
  ok(undone.result, json.encode(undone.error or {}))
  eq(#mock.data.instruments, 3)
end)

test("instruments: a delete's look counts reads, not lines: dense patterns under the line budget are looked through in a job (#361 N-S17)", function()
  local env, peer = patched("changes_instruments.lua", "M%.SCAN_BUDGET = 300000", "M.SCAN_BUDGET = 700")
  for p = 1, 2 do
    for l = 1, 32 do
      local column = mock.song:pattern(p):track(3):line(l):note_column(1)
      column.note_value, column.instrument_value = 48, 0
    end
  end
  note_at(1, 1)
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  local previewed, epoch, layout = preview(peer, "instrument-delete", { ref = iref(place(peer), 2) })
  local id = send(peer, "change_apply", { kind = "instrument-delete", input = { ref = iref(epoch, 2) }, fence = previewed.fence, desc = "Kumi: d (k171)", epoch = epoch, layout = layout })
  local answered = false
  for _, line in ipairs(peer:frames(nil)) do
    local frame = json.decode(line)
    if frame.type == "response" and frame.id == id then answered = true end
  end
  ok(not answered, "not answered inside the request's own callback: a job looks")
  local done = wait(peer, id, 800)
  ok(done.result, json.encode(done.error or {}))
  eq(#done.result.material.notes, 1)
end)

test("instruments: the copies kept for undo are bounded: past the bound the oldest goes, and its undo says the copy is gone (#361 N-S19)", function()
  local env, peer = patched("instruments.lua", "M%.MAX_KEPT_FILES, M%.MAX_KEPT_BYTES = 256", "M.MAX_KEPT_FILES, M.MAX_KEPT_BYTES = 1")
  local first = change(peer, "instrument-delete", { ref = iref(place(peer), 3) })
  ok(io.exists(first.material.file), "kept")
  local second = change(peer, "instrument-delete", { ref = iref(place(peer), 2) })
  ok(io.exists(second.material.file), "the newest is kept")
  eq(io.exists(first.material.file), false, "the oldest went")
  local undone = restore(peer, "instrument-delete", first)
  failure(undone, "failed")
  ok(undone.error.message:find("is gone", 1, true), undone.error.message)
end)

test("instruments: a big song is looked through in slices before the delete, which is one step; a pattern changed meanwhile is looked at again", function()
  local env, peer = setup({ patch = function(env_)
    local path = env_.bundle .. "payload/changes_instruments.lua"
    local f = assert(io.open(path, "rb"))
    local text = f:read("*a")
    f:close()
    text = text:gsub("M%.SCAN_BUDGET = 300000", "M.SCAN_BUDGET = 10")
    f = assert(io.open(path, "wb"))
    f:write(text)
    f:close()
  end })
  note_at(1, 1)
  mock.clock_step_ms = 1 -- (each read of the clock costs a millisecond: the look takes many slices)
  local previewed, epoch, layout = preview(peer, "instrument-delete", { ref = iref(place(peer), 2) })
  local id = send(peer, "change_apply", { kind = "instrument-delete", input = { ref = iref(epoch, 2) }, fence = previewed.fence, desc = "Kumi: d (k420)", epoch = epoch, layout = layout })
  mock.advance(10)
  local column = mock.song:pattern(1):track(3):line(4):note_column(1)
  column.note_value, column.instrument_value = 52, 1 -- the producer writes another note playing it, mid-look
  local before = steps()
  local done = wait(peer, id, 800)
  mock.clock_step_ms = 0
  ok(done.result, json.encode(done.error or {}))
  eq(#done.result.material.notes, 2, "both notes kept, the later one too")
  eq(steps(), before + 1, "Kumi's delete is one step")
  eq(newest(), "Kumi: d (k420)")
end)

-- instrument-load

local function saved_instrument(peer, i)
  local path = mock.temp_dir .. "/kept-" .. i .. ".xrni"
  mock.data.song_fields.selected_instrument_index = i
  ok(mock.app:save_instrument(path))
  return path
end

test("instruments: a zip named .xrni that isn't an instrument (a song renamed) is refused before Renoise's dialog (#361 N-N12)", function()
  local env, peer = setup()
  local path = mock.temp_dir .. "/renamed-song.xrni"
  local f = assert(io.open(path, "wb"))
  f:write("PK\3\4" .. string.rep("z", 200) .. "PK\1\2 Song.xml")
  f:close()
  local epoch = place(peer)
  local refused = call(peer, "change_preview", { kind = "instrument-load", input = { path = path }, epoch = epoch })
  failure(refused, "bad-params")
  ok(refused.error.message:find("contents say otherwise", 1, true), refused.error.message)
  os.remove(path)
end)

test("instruments: an .xrni, an .sfz and a sample file load into a new instrument; Kumi's undo takes it away", function()
  local env, peer = setup()
  local xrni = saved_instrument(peer, 2)
  local record, done = change(peer, "instrument-load", { path = xrni, index = 0 })
  eq(names(), "kept-2,Drums,Bass synth,Pad", "named after its file, as Renoise names what it loads")
  eq(done.created.kind, "instrument")
  eq(steps(), 1)
  local undone = restore(peer, "instrument-load", record)
  ok(undone.result, json.encode(undone.error or {}))
  eq(names(), "Drums,Bass synth,Pad")
  local sfz = mock.temp_dir .. "/kit.sfz"
  local f = assert(io.open(sfz, "wb"))
  f:write("<region> sample=kick.wav lokey=36 hikey=36 pitch_keycenter=36\n<region> sample=snare.wav lokey=38 hikey=38\n")
  f:close()
  change(peer, "instrument-load", { path = sfz })
  eq(#instrument(4).samples, 2)
  eq(instrument(4).samples[2].note_range[1], 38)
  local wav = mock.temp_dir .. "/Clap.wav"
  f = assert(io.open(wav, "wb"))
  f:write("RIFF\0\0\0\0WAVE" .. string.rep("x", 500))
  f:close()
  change(peer, "instrument-load", { path = wav })
  eq(instrument(5).name, "Clap")
  eq(instrument(5).samples[1].buffer.frames, 512)
  eq(mock.dialogs, 0)
end)

test("instruments: Renoise's empty instrument after a load into the last one is taken out again: a load at the end, into the last, a delete's undo", function()
  local env, peer = setup()
  local xrni = saved_instrument(peer, 2)
  local record = change(peer, "instrument-load", { path = xrni, index = 3 })
  eq(names(), "Drums,Bass synth,Pad,kept-2", "no empty instrument after it")
  ok(restore(peer, "instrument-load", record).result)
  eq(names(), "Drums,Bass synth,Pad")
  change(peer, "instrument-load", { path = xrni, ref = iref(place(peer), 3) })
  eq(names(), "Drums,Bass synth,kept-2", "loaded into Pad, the last, empty")
  env, peer = setup()
  record = change(peer, "instrument-delete", { ref = iref(place(peer), 3) })
  eq(names(), "Drums,Bass synth")
  ok(restore(peer, "instrument-delete", record).result)
  eq(names(), "Drums,Bass synth,Pad")
  H.no_dialogs()
end)

test("instruments: Renoise's undo and redo of an instrument loaded as the last one are told", function()
  local env, peer = setup()
  local xrni = saved_instrument(peer, 2)
  change(peer, "instrument-load", { path = xrni, index = 3 }, "Kumi: load (k430)")
  told(peer)
  mock.song:undo()
  eq(names(), "Drums,Bass synth,Pad")
  local events = told(peer)
  eq(#events, 1, "undone")
  eq(events[1].data.how, "undo")
  mock.song:redo()
  eq(names(), "Drums,Bass synth,Pad,kept-2")
  events = told(peer)
  eq(#events, 1, "redone")
  eq(events[1].data.how, "redo")
end)

test("instruments: after Renoise's own undo and redo of an instrument Kumi added, duplicated or loaded, or a phrase it added, Kumi's undo takes it away; changed since: refused", function()
  local cases = {
    { "instrument-add", function(E) return { index = 3, name = "New" } end, "Drums,Bass synth,Pad" },
    { "instrument-duplicate", function(E) return { ref = iref(E, 2) } end, "Drums,Bass synth,Pad" },
    { "instrument-load", function(E, peer) return { path = saved_instrument(peer, 2), index = 0 } end, "Drums,Bass synth,Pad" },
  }
  for _, case in ipairs(cases) do
    for _, changed in ipairs({ false, true }) do
      local env, peer = setup()
      local input = case[2](place(peer), peer)
      local record, done = change(peer, case[1], input)
      local made = done.created and done.created.ref
      mock.song:undo()
      eq(names(), "Drums,Bass synth,Pad", case[1] .. ": Renoise's undo")
      mock.song:redo()
      H.tick(2)
      if changed then
        -- (the producer adds a sample to it after the redo)
        local i = tonumber(made:match(":(%d+)$"))
        mock.song:instrument(i):insert_sample_at(1)
      end
      local undone = restore(peer, case[1], record)
      if changed then
        failure(undone, "changed")
      else
        ok(undone.result, case[1] .. ": what Renoise's redo made again is Kumi's: " .. json.encode(undone.error or {}))
        eq(names(), case[3], case[1])
      end
    end
  end
  -- a phrase
  local env, peer = setup()
  local record = change(peer, "phrase-add", { ref = iref(place(peer), 3), name = "Hook" })
  mock.song:undo()
  mock.song:redo()
  H.tick(2)
  ok(restore(peer, "phrase-add", record).result, "the phrase Renoise's redo made again is Kumi's")
  eq(#instrument(3).phrases, 0)
  H.no_dialogs()
end)

test("instruments: a load into an empty instrument, refused into one that isn't; a missing file or a kind Renoise won't load is bad-params", function()
  local env, peer = setup()
  local xrni = saved_instrument(peer, 1)
  local epoch = place(peer)
  local record = change(peer, "instrument-load", { path = xrni, ref = iref(epoch, 3) })
  eq(#instrument(3).samples, 3)
  eq(#mock.data.instruments, 3, "no instrument made")
  local undone = restore(peer, "instrument-load", record)
  ok(undone.result, json.encode(undone.error or {}))
  eq(#instrument(3).samples, 0, "emptied again")
  epoch = place(peer)
  failure(call(peer, "change_preview", { kind = "instrument-load", input = { path = xrni, ref = iref(epoch, 1) }, epoch = epoch }), "bad-params")
  failure(call(peer, "change_preview", { kind = "instrument-load", input = { path = "/nowhere/none.xrni" }, epoch = epoch }), "bad-params")
  failure(call(peer, "change_preview", { kind = "instrument-load", input = { path = "relative.xrni" }, epoch = epoch }), "bad-params")
  local txt = mock.temp_dir .. "/notes.txt"
  local f = assert(io.open(txt, "wb"))
  f:write("hello")
  f:close()
  failure(call(peer, "change_preview", { kind = "instrument-load", input = { path = txt }, epoch = epoch }), "bad-params")
  eq(mock.dialogs, 0, "nothing asked of Renoise that would show a dialog")
end)

test("instruments: a load Renoise refuses leaves nothing behind", function()
  local env, peer = setup()
  local not_wav = mock.temp_dir .. "/notes.wav"
  local f = assert(io.open(not_wav, "wb"))
  f:write("this is not a wave file")
  f:close()
  -- (a file that isn't what its name says is refused before Renoise is asked: it would show a dialog)
  failure(call(peer, "change_preview", { kind = "instrument-load", input = { path = not_wav }, epoch = place(peer) }), "bad-params")
  local bad_wav = mock.temp_dir .. "/broken.wav"
  f = assert(io.open(bad_wav, "wb"))
  f:write("RIFF\0\0\0\0WAVE broken inside")
  f:close()
  local previewed, epoch, layout = preview(peer, "instrument-load", { path = bad_wav })
  local done = apply(peer, "instrument-load", { path = bad_wav }, "Kumi: load (k1)", previewed.fence, epoch, layout)
  failure(done, "failed")
  eq(#mock.data.instruments, 3, "the new instrument went again")
end)

-- instrument-duplicate, instrument-move

test("instruments: a duplicate copies samples and phrases right after it; Kumi's undo takes the copy away", function()
  local env, peer = setup()
  local record, done = change(peer, "instrument-duplicate", { ref = iref(place(peer), 2) })
  eq(names(), "Drums,Bass synth,Bass synth,Pad")
  eq(instrument(3).samples[1].buffer.frames, 5000)
  eq(#instrument(3).phrases, 1)
  ok(done.created.ref:match(":instrument:3$"))
  local undone = restore(peer, "instrument-duplicate", record)
  ok(undone.result, json.encode(undone.error or {}))
  eq(names(), "Drums,Bass synth,Pad")
end)

test("instruments: a duplicate's copy tells its lists in Renoise's own order (lab): the places Kumi keeps elsewhere still hold", function()
  local env, peer = setup()
  mock.device_catalog = { ["Audio/Effects/Native/Gainer"] = { { name = "Gain", value = 1, min = 0, max = 4 } } }
  local E = place(peer)
  local set = change(peer, "modset-add", { ref = iref(E, 2), name = "Env" })
  local device = change(peer, "moddevice-add", { ref = place(peer) .. ":modset:2.2", path = "AHDSR", target = "volume" })
  local chain = change(peer, "chain-add", { ref = iref(place(peer), 2), name = "Verb" })
  local gainer = change(peer, "chaindevice-add", { ref = place(peer) .. ":chain:2.1", path = "Gainer" })
  local phrase = change(peer, "phrase-set", { ref = place(peer) .. ":phrase:2.1", lpb = 8 })
  local copy = change(peer, "instrument-duplicate", { ref = iref(place(peer), 2) })
  eq(#instrument(3).chains[1].devices, 2, "the copy has the chain's mixer and its Gainer")
  for _, step in ipairs({ { "instrument-duplicate", copy }, { "phrase-set", phrase }, { "chaindevice-add", gainer }, { "chain-add", chain },
    { "moddevice-add", device }, { "modset-add", set } }) do
    local undone = restore(peer, step[1], step[2])
    ok(undone.result, step[1] .. ": " .. json.encode(undone.error or {}))
  end
  eq(#mock.data.instruments, 3)
  eq(#instrument(2).sets, 1)
  eq(#instrument(2).chains, 0)
  eq(instrument(2).phrases[1].lpb, 4)
end)

test("instruments: a move to 3 keeps the others in order and renumbers notes; Kumi's undo moves it back, refused after the producer moved others", function()
  local env, peer = setup()
  note_at(1, 0) -- Drums
  local record = change(peer, "instrument-move", { ref = iref(place(peer), 1), index = 2 })
  eq(names(), "Bass synth,Pad,Drums")
  eq(note_value(1), 2, "the note plays Drums still")
  local undone = restore(peer, "instrument-move", record)
  ok(undone.result, json.encode(undone.error or {}))
  eq(names(), "Drums,Bass synth,Pad")
  eq(note_value(1), 0)
  record = change(peer, "instrument-move", { ref = iref(place(peer), 1), index = 2 })
  mock.ops.insert_instrument_at(1, { name = "New" })
  failure(restore(peer, "instrument-move", record), "changed")
  eq(names(), "New,Bass synth,Pad,Drums")
end)

-- instrument_save (an action)

test("instruments: an instrument saves to a file, never over one already there unless asked", function()
  local env, peer = setup()
  local path = mock.temp_dir .. "/Bass.xrni"
  local epoch = place(peer)
  local saved = call(peer, "instrument_save", { ref = iref(epoch, 2), path = path, epoch = epoch })
  ok(saved.result, json.encode(saved.error or {}))
  eq(saved.result.saved, path)
  local value = mock.read_file(path)
  eq(value.instrument.name, "Bass synth")
  eq(steps(), 0, "no undo step")
  failure(call(peer, "instrument_save", { ref = iref(epoch, 1), path = path, epoch = epoch }), "bad-params")
  eq(mock.read_file(path).instrument.name, "Bass synth", "the file there is kept")
  local again = call(peer, "instrument_save", { ref = iref(epoch, 1), path = path, epoch = epoch, overwrite = true })
  ok(again.result, json.encode(again.error or {}))
  eq(mock.read_file(path).instrument.name, "Drums")
  eq(mock.data.song_fields.selected_instrument_index, 1, "the selection is the producer's again")
  -- (Kumi names the instrument instrumentRef; the answer says what it saved, in Renoise's words)
  local named = call(peer, "instrument_save", { instrumentRef = iref(epoch, 3), path = mock.temp_dir .. "/Pad", epoch = epoch })
  ok(named.result, json.encode(named.error or {}))
  eq(named.result.saved, mock.temp_dir .. "/Pad.xrni", "an instrument file's extension")
  ok(named.result.display.title:find("Saved instrument", 1, true), named.result.display.title)
  failure(call(peer, "instrument_save", { instrumentRef = iref(epoch, 3), path = "Pad.xrni", epoch = epoch }), "bad-params")
  -- (a file there is replaced only by its exact path: the extension Kumi adds isn't the producer's word)
  failure(call(peer, "instrument_save", { instrumentRef = iref(epoch, 1), path = mock.temp_dir .. "/Pad", overwrite = true, epoch = epoch }), "bad-params")
  eq(mock.read_file(mock.temp_dir .. "/Pad.xrni").instrument.name, "Pad", "kept")
end)

test("instruments: what Kumi keeps to put back (a deleted instrument, a sample's audio) is in a folder of its own in Renoise's temporary folder", function()
  local env, peer = setup()
  local record = change(peer, "instrument-delete", { ref = iref(place(peer), 3) })
  ok(record.material.file:find("/kumi/", 1, true), record.material.file)
  local sample = change(peer, "sample-delete", { ref = sref(place(peer), 1, 1) })
  ok(sample.material.file:find("/kumi/", 1, true), sample.material.file)
  ok(restore(peer, "sample-delete", sample).result)
  ok(restore(peer, "instrument-delete", record).result)
  eq(instrument(3).name, "Pad")
end)
