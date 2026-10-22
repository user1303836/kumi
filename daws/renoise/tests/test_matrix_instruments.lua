-- The disruption matrix for Kumi's changes inside an instrument (M4, n-instruments): each kind's undo after the producer made
-- an instrument before it, deleted one before it, swapped it with another, or deleted it; and after Kumi made one before it.
-- The undo follows the instrument (and what is in it) to where it is, or refuses `gone` and writes nothing.
local T = ...
local mock = T.mock
local json = T.load_payload_module("json")
local test, eq, ok = T.test, T.eq, T.ok
local I = T.instruments
local setup, place, change, restore, failure, steps, iref, sref = I.setup, I.place, I.change, I.restore, I.failure, I.steps, I.iref, I.sref

local function pref(epoch, i, p) return epoch .. ":phrase:" .. i .. "." .. p end
local function data(i) return mock.data.instruments[i] end

-- Each kind: what it changes on Bass synth (instrument 2: one sample, Saw; one phrase; its one modulation set), and whether
-- the instrument at `i` is as it was before the change.
local KINDS = {
  { "sample-set", function(E) return { ref = sref(E, 2, 1), volumeDb = -6 } end, function(i) return data(i).samples[1].volume == 1 end },
  { "keyzone-set", function(E) return { ref = sref(E, 2, 1), baseNote = 60 } end, function(i) return data(i).samples[1].base_note == 48 end },
  { "instrument-set", function(E) return { ref = iref(E, 2), transpose = 5 } end, function(i) return data(i).transpose == 0 end },
  { "instrument-macro", function(E) return { ref = iref(E, 2), macro = 3, value = 0.9 } end, function(i) return data(i).macros[3].value == 0.5 end },
  { "keyzone-overlap", function(E) return { ref = iref(E, 2), mode = "cycle" } end, function(i) return data(i).overlap == 0 end },
  { "phrase-set", function(E) return { ref = pref(E, 2, 1), lpb = 8 } end, function(i) return data(i).phrases[1].lpb == 4 end },
  { "phrase-playback", function(E) return { ref = iref(E, 2), mode = "keymap" } end, function(i) return data(i).phrase_playback_mode == 2 end },
  { "phrase-write", function(E) return { ref = pref(E, 2, 1), lines = { { line = 2, notes = { { col = 1, note = "C-4" } } } } } end,
    function(i) local line = data(i).phrases[1].data[3] return line == nil or line.notes == nil or line.notes[1] == nil or line.notes[1].note == 121 end },
  { "modset-set", function(E) return { ref = E .. ":modset:2.1", pitchRange = 24 } end, function(i) return data(i).sets[1].pitch_range == 12 end },
  { "chain-add", function(E) return { ref = iref(E, 2), name = "Verb" } end, function(i) return #data(i).chains == 0 end },
  { "sample-delete", function(E) return { ref = sref(E, 2, 1) } end, function(i) return #data(i).samples == 1 and data(i).samples[1].name == "Saw" end },
  { "sample-invert", function(E) return { ref = sref(E, 2, 1) } end, function(i)
    -- (as it was: the value at a frame is the one its audio had before any write)
    local buffer = data(i).samples[1].buffer
    local now = mock.song:instrument(i):sample(1).sample_buffer:sample_data(1, 10)
    local written = buffer.values
    buffer.values = nil
    local plain = mock.song:instrument(i):sample(1).sample_buffer:sample_data(1, 10)
    buffer.values = written
    return math.abs(now - plain) < 1e-12
  end },
  { "phrase-delete", function(E) return { ref = pref(E, 2, 1) } end, function(i) return #data(i).phrases == 1 end },
}

-- Each disruption, and where Bass synth is after it (nil: gone).
local DISRUPTIONS = {
  { "an instrument made before it", function() mock.ops.insert_instrument_at(1, { name = "New" }) end, 3 },
  { "an instrument deleted before it", function() mock.ops.remove_instrument(1) end, 1 },
  { "swapped with the one after it", function() mock.ops.swap_instruments(2, 3) end, 3 },
  { "Kumi's own instrument made before it", function(peer) change(peer, "instrument-add", { index = 0, name = "Made" }) end, 3 },
  { "deleted", function() mock.ops.remove_instrument(2) end, nil },
}

for _, kind in ipairs(KINDS) do
  test("matrix, " .. kind[1] .. " × the instrument list: the undo follows Bass synth, or refuses gone and writes nothing", function()
    for _, disruption in ipairs(DISRUPTIONS) do
      local env, peer = setup()
      local record = change(peer, kind[1], kind[2](place(peer)))
      ok(not kind[3](2), kind[1] .. ": the change took")
      disruption[2](peer)
      local at = disruption[3]
      local before = steps()
      local undone = restore(peer, kind[1], record)
      if at then
        ok(undone.result, kind[1] .. " × " .. disruption[1] .. ": " .. json.encode(undone.error or {}))
        eq(data(at).name, "Bass synth", disruption[1])
        ok(kind[3](at), kind[1] .. " × " .. disruption[1] .. ": put back where Bass synth is now")
      else
        failure(undone, "gone")
        eq(steps(), before, kind[1] .. " × deleted: nothing written")
      end
    end
  end)
end
