-- The fence of each of Kumi's instrument kinds (M4, n-instruments): what the producer changed between the preview and the apply
-- refuses the apply (`changed`), and nothing is written. Each test here fails with its kind's fence check taken out.
local T = ...
local mock = T.mock
local json = T.load_payload_module("json")
local test, eq, ok = T.test, T.eq, T.ok
local I, F = T.instruments, T.modfx
local setup, place, preview, apply, change, failure, steps, iref, sref = I.setup, I.place, I.preview, I.apply, I.change, I.failure, I.steps, I.iref, I.sref

local function pref(E, i, p) return E .. ":phrase:" .. i .. "." .. p end
local function mref(E, i, m, d) return E .. ":" .. (d and "moddevice" or "modset") .. ":" .. i .. "." .. m .. (d and ("." .. d) or "") end
local function cref(E, i, c, d) return E .. ":" .. (d and "chaindevice" or "chain") .. ":" .. i .. "." .. c .. (d and ("." .. d) or "") end
local function data(i) return mock.data.instruments[i] end
local function song() return mock.song end

local function wav_file(name)
  local path = mock.temp_dir .. "/" .. name .. ".wav"
  local f = assert(io.open(path, "wb"))
  f:write("RIFF\0\0\0\0WAVE" .. string.rep("w", 244))
  f:close()
  return path
end

local function xrni_file()
  local path = mock.temp_dir .. "/fence.xrni"
  mock.data.song_fields.selected_instrument_index = 2
  ok(mock.app:save_instrument(path))
  return path
end

-- The songs the kinds need: plain (Drums, Bass synth, Pad), modfx (Drums with a set and a chain), plugin (Pad plays a plug-in),
-- script (Bass synth's phrase plays a committed script).
local SONGS = {
  plain = function() return setup() end,
  modfx = function() local env, peer = F.setup() return env, peer end,
  plugin = function()
    mock.plugin_catalog = {
      ["Audio/Generators/AU/aumu:dls :appl"] = { name = "DLSMusicDevice", params = { { name = "Volume", value = 0.5 }, { name = "Tune", value = 0.5 } }, presets = { "Piano", "Strings" } },
      ["Audio/Generators/VST3/Synth"] = { name = "Synth", params = { { name = "Cutoff", value = 0.2 } }, presets = {} },
    }
    local env, peer = setup()
    change(peer, "plugin-load", { ref = iref(place(peer), 3), path = "DLSMusicDevice" })
    return env, peer
  end,
  script = function()
    local env, peer = setup()
    local phrase = song():instrument(2):phrase(1)
    phrase:line(2):note_column(1).note_string = "F-4"
    phrase.playback_mode = 2
    phrase.script.paragraphs = { "return cycle(\"c4 e4\")" }
    phrase.script:commit()
    mock.compile_scripts()
    return env, peer
  end,
  lines = function()
    local env, peer = setup()
    song():instrument(2):phrase(1):line(2):note_column(1).note_string = "F-4"
    return env, peer
  end,
}

-- kind, song, input, the producer's change in between (it may answer the input and epoch Kumi applies with after reading again)
local FENCES = {
  { "instrument-rename", "plain", function(E) return { ref = iref(E, 2), name = "Acid" } end, function() data(2).name = "Other" end },
  { "instrument-set", "plain", function(E) return { ref = iref(E, 2), transpose = 5 } end, function() data(2).transpose = 3 end },
  { "instrument-macro", "plain", function(E) return { ref = iref(E, 2), macro = 1, value = 0.9 } end, function() data(2).macros[1].value = 0.2 end },
  { "instrument-add", "plain", function() return { index = 0, name = "New" } end, function() mock.ops.insert_instrument({ name = "Theirs" }) end },
  { "instrument-delete", "plain", function(E) return { ref = iref(E, 2) } end, function() data(2).name = "Other" end },
  { "instrument-load", "plain", function() return { path = xrni_file(), index = 0 } end, function() mock.ops.insert_instrument({ name = "Theirs" }) end },
  { "instrument-load", "plain", function(E) return { path = xrni_file(), ref = iref(E, 3) } end, function() data(3).name = "Theirs" end },
  { "instrument-duplicate", "plain", function(E) return { ref = iref(E, 2) } end, function() data(2).name = "Other" end },
  { "instrument-move", "plain", function(E) return { ref = iref(E, 2), index = 2 } end, function(peer)
    -- (Kumi read again after the producer made an instrument before it: the same instrument, by a ref of now)
    mock.ops.insert_instrument_at(1, { name = "New" })
    local E = place(peer)
    return { ref = iref(E, 3), index = 2 }, E
  end },
  { "sample-set", "plain", function(E) return { ref = sref(E, 2, 1), volumeDb = -6 } end, function() data(2).samples[1].volume = 0.5 end },
  { "keyzone-set", "plain", function(E) return { ref = sref(E, 1, 1), baseNote = 60 } end, function() data(1).samples[1].base_note = 50 end },
  { "keyzone-overlap", "plain", function(E) return { ref = iref(E, 1), mode = "cycle" } end, function() data(1).overlap = 2 end },
  { "keyzone-drumkit", "plain", function(E) return { ref = iref(E, 1) } end, function() data(1).samples[2].base_note = 70 end },
  { "keyzone-drumkit", "plain", function(E) return { ref = iref(E, 1) } end, function() mock.ops.insert_sample(1) end },
  { "keyzone-map", "plain", function(E) return { ref = iref(E, 1), samples = { { sampleRef = sref(E, 1, 1), noteRange = { 0, 59 }, baseNote = 48 } } } end,
    function() data(1).samples[1].base_note = 70 end },
  { "keyzone-map", "plain", function(E) return { ref = iref(E, 1), overlap = "cycle", samples = { { sampleRef = sref(E, 1, 1), noteRange = { 0, 59 }, baseNote = 48 } } } end,
    function() data(1).overlap = 2 end },
  { "sample-load", "plain", function(E) return { ref = iref(E, 1), path = wav_file("Clap") } end, function() mock.ops.insert_sample(1) end },
  { "sample-load", "plain", function(E) return { ref = sref(E, 1, 1), path = wav_file("Clap") } end, function() data(1).samples[1].name = "Other" end },
  { "sample-delete", "plain", function(E) return { ref = sref(E, 1, 2) } end, function() data(1).samples[2].name = "Other" end },
  { "sample-slices", "plain", function(E) return { ref = sref(E, 1, 1), insert = { 500 } } end, function() data(1).samples[1].buffer.frames = 900 end },
  { "sample-invert", "plain", function(E) return { ref = sref(E, 1, 1) } end, function() data(1).samples[1].buffer.edits = { [1] = 0.25 } end },
  { "phrase-add", "plain", function(E) return { ref = iref(E, 2) } end, function() mock.ops.insert_phrase(2) end },
  { "phrase-delete", "plain", function(E) return { ref = pref(E, 2, 1) } end, function() data(2).phrases[1].name = "Other" end },
  { "phrase-set", "plain", function(E) return { ref = pref(E, 2, 1), lpb = 8 } end, function() data(2).phrases[1].lpb = 2 end },
  { "phrase-map", "plain", function(E) return { ref = pref(E, 2, 1), noteRange = { 48, 59 } } end, function()
    song():instrument(2):insert_phrase_mapping_at(1, song():instrument(2):phrase(1))
  end },
  { "phrase-playback", "plain", function(E) return { ref = iref(E, 2), mode = "keymap" } end, function() data(2).phrase_playback_mode = 1 end },
  { "phrase-write", "plain", function(E) return { ref = pref(E, 2, 1), lines = { { line = 0, notes = { { col = 1, note = "C-4" } } } } } end,
    function() mock.ops.set_phrase_note(2, 1, 1, 1, { note = 50 }) end },
  { "phrase-clear", "lines", function(E) return { ref = pref(E, 2, 1) } end, function() mock.ops.set_phrase_note(2, 1, 2, 1, { note = 50 }) end },
  { "phrase-script", "plain", function(E) return { ref = pref(E, 2, 1), code = "return cycle(\"c4\")" } end, function() data(2).phrases[1].script.paragraphs = { "-- mine" } end },
  { "phrase-render", "script", function(E) return { ref = pref(E, 2, 1) } end, function() mock.ops.set_phrase_note(2, 1, 3, 1, { note = 50 }) end },
  { "modset-add", "modfx", function(E) return { ref = iref(E, 1), name = "Filter" } end, function() song():instrument(1):insert_sample_modulation_set_at(2) end },
  { "modset-delete", "modfx", function(E) return { ref = mref(E, 1, 1) } end, function() data(1).sets[1].name = "Other" end },
  { "modset-set", "modfx", function(E) return { ref = mref(E, 1, 1), pitchRange = 24 } end, function() data(1).sets[1].pitch_range = 36 end },
  { "moddevice-add", "modfx", function(E) return { ref = mref(E, 1, 1), path = "LFO", target = "pitch" } end, function()
    song():instrument(1):sample_modulation_set(1):insert_device_at("Modulation/Fader", 1, 1)
  end },
  { "moddevice-delete", "modfx", function(E) return { ref = mref(E, 1, 1, 1) } end, function() data(1).sets[1].devices[1].params[1].value = 0.7 end },
  { "moddevice-move", "modfx", function(E) return { ref = mref(E, 1, 1, 1), index = 2 } end, function() data(1).sets[1].devices[1].params[1].value = 0.7 end },
  { "moddevice-set", "modfx", function(E) return { ref = mref(E, 1, 1, 1), bipolar = true } end, function() data(1).sets[1].devices[1].bipolar = true end },
  { "moddevice-param", "modfx", function(E) return { ref = mref(E, 1, 1, 1), param = "Attack", value = 0.3 } end, function() data(1).sets[1].devices[1].params[1].value = 0.7 end },
  { "chain-add", "modfx", function(E) return { ref = iref(E, 1), name = "Verb" } end, function() song():instrument(1):insert_sample_device_chain_at(2) end },
  { "chain-delete", "modfx", function(E) return { ref = cref(E, 1, 1) } end, function() data(1).chains[1].name = "Other" end },
  { "chain-set", "modfx", function(E) return { ref = cref(E, 1, 1), name = "Verb" } end, function() data(1).chains[1].name = "Other" end },
  { "chaindevice-add", "modfx", function(E) return { ref = cref(E, 1, 1), path = "Gainer" } end, function()
    song():instrument(1):sample_device_chain(1):insert_device_at("Audio/Effects/Native/Delay", 2)
  end },
  { "chaindevice-delete", "modfx", function(E) return { ref = cref(E, 1, 1, 2) } end, function() data(1).chains[1].devices[2].params[1].value = 9 end },
  { "chaindevice-move", "modfx", function(E)
    -- (a third device to move the EQ past)
    data(1).chains[1].devices[3] = { name = "Gainer", path = "Audio/Effects/Native/Gainer", params = { { name = "Gain", value = 1, min = 0, max = 4 } }, active = true }
    return { ref = cref(E, 1, 1, 2), index = 3 }
  end, function() data(1).chains[1].devices[2].params[1].value = 9 end },
  { "chaindevice-bypass", "modfx", function(E) return { ref = cref(E, 1, 1, 2), bypass = true } end, function() data(1).chains[1].devices[2].active = false end },
  { "chaindevice-param", "modfx", function(E) return { ref = cref(E, 1, 1, 2), param = "Gain 1", value = 6 } end, function() data(1).chains[1].devices[2].params[1].value = 9 end },
  { "plugin-load", "plugin", function(E) return { ref = iref(E, 3), path = "Audio/Generators/VST3/Synth" } end, function() data(3).plugin.preset_data = "<mine/>" end },
  { "plugin-param", "plugin", function(E) return { ref = iref(E, 3), param = "Tune", value = 0.75 } end, function() data(3).plugin.params[2].value = 0.1 end },
  { "plugin-preset", "plugin", function(E) return { ref = iref(E, 3), preset = 2 } end, function() data(3).plugin.preset_data = "<mine/>" end },
  { "plugin-set", "plugin", function(E) return { ref = iref(E, 3), channel = 2 } end, function() data(3).plugin_props.channel = 5 end },
  { "plugin-suspend", "plugin", function(E) return { ref = iref(E, 3), autoSuspend = true } end, function() data(3).plugin_props.auto_suspend = true end },
  { "midi-set", "plain", function(E) return { ref = iref(E, 3), channel = 10 } end, function() data(3).midi.channel = 4 end },
}

local counted = {}
for _, fence in ipairs(FENCES) do
  local kind, song_kind, input_of, between = fence[1], fence[2], fence[3], fence[4]
  counted[kind] = (counted[kind] or 0) + 1
  local name = counted[kind] > 1 and (kind .. " (" .. counted[kind] .. ")") or kind
  test("fence, " .. name .. ": the producer's change between the preview and the apply refuses it, nothing written", function()
    local _, peer = SONGS[song_kind]()
    mock.data.undo.stack = {}
    local input = input_of(place(peer))
    local previewed, epoch, layout = preview(peer, kind, input)
    local again, now = between(peer, input)
    if again then input, epoch = again, now end
    local before, undo = steps(), json.encode(mock.data.undo.stack)
    local response = apply(peer, kind, input, "Kumi: " .. kind .. " (k1)", previewed.fence, epoch, layout)
    failure(response, "changed")
    eq(steps(), before, "nothing written")
    eq(json.encode(mock.data.undo.stack), undo, "no step")
  end)
end

-- Every kind of the stream has its fence here.
test("fence: every instrument kind is in the fence table", function()
  for _, kind in ipairs({ "instrument-rename", "instrument-set", "instrument-macro", "instrument-add", "instrument-delete", "instrument-load",
    "instrument-duplicate", "instrument-move", "sample-set", "keyzone-set", "keyzone-overlap", "keyzone-drumkit", "keyzone-map", "sample-load", "sample-delete",
    "sample-slices", "sample-invert", "modset-add", "modset-delete", "modset-set", "moddevice-add", "moddevice-delete", "moddevice-move", "moddevice-set",
    "moddevice-param", "chain-add", "chain-delete", "chain-set", "chaindevice-add", "chaindevice-delete", "chaindevice-move", "chaindevice-bypass",
    "chaindevice-param", "phrase-add", "phrase-delete", "phrase-set", "phrase-map", "phrase-playback", "phrase-write", "phrase-clear",
    "phrase-script", "phrase-render", "plugin-load", "plugin-param", "plugin-preset", "plugin-set", "plugin-suspend", "midi-set" }) do
    ok(counted[kind], kind .. " has a fence test")
  end
end)
