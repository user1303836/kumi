-- Kumi's changes to an instrument's samples (M4): a sample's settings and keyzone, the instrument's overlap mode, a drum-kit
-- layout of its keyzones, a sample loaded from a file (new, or in place of one), a sample deleted, and slice markers. Each
-- record keeps its samples by their places (instruments.lua); a sample's modulation set and FX chain are places too (the
-- number Renoise keeps is the set's or chain's place in the instrument).

local import = ...
local json = import("json")
local base = import("changes_base")
local places_ = import("instruments")
local instruments_ = import("changes_instruments")

local bad, fail = base.bad, base.fail
local q = base.q
local whole, is_boolean, named = instruments_.whole, instruments_.is_boolean, instruments_.named
local M = {}

-- The most a sample Kumi deletes or replaces may hold in memory (saving it to keep and loading it back stay inside one callback).
M.MAX_SAMPLE_BYTES = 64 * 1024 * 1024
-- The most slice markers one change inserts, deletes or moves.
M.MAX_SLICE_OPS = 64

-- A sample's buffer in brief: its shape and some of its values (256 points through it and its last frame, every channel), enough
-- to tell it is the one Kumi left: an edit in the sample editor (a fade at its end, a short cut) moves one of them.
M.BUFFER_PROBES = 256
function M.buffer_print(buffer)
  if not buffer.has_sample_data then return { frames = 0 } end
  local frames, channels = buffer.number_of_frames, buffer.number_of_channels
  local a, b = 5381, 0
  for k = 0, M.BUFFER_PROBES do
    local frame = k == M.BUFFER_PROBES and frames or math.max(1, math.floor(frames * k / M.BUFFER_PROBES) + 1)
    -- (the definitions say a list, the guide a number: either is read)
    for channel = 1, channels do
      local value = buffer:sample_data(channel, math.min(frame, frames))
      local x = type(value) == "table" and value[1] or value
      local text = string.format("%.6f|", tonumber(x) or 0)
      for i = 1, #text do
        local byte = text:byte(i)
        a = (a * 33 + byte) % 4294967296
        b = (b * 65599 + byte) % 4294967296
      end
    end
  end
  return { frames = frames, channels = channels, rate = buffer.sample_rate, bits = buffer.bit_depth, values = string.format("%08x%08x", a, b) }
end

-- Renoise leaves an instrument unnamed when its last sample goes (lab 2026-10-19): Kumi deletes the sample and keeps the
-- instrument's name, in the same step.
local function delete_sample(instrument, s)
  local name = instrument.name
  instrument:delete_sample_at(s)
  if instrument.name ~= name then instrument.name = name end
end

local function buffer_bytes(buffer)
  if not buffer.has_sample_data then return 0 end
  return buffer.number_of_frames * buffer.number_of_channels * math.max(1, math.floor(buffer.bit_depth / 8))
end
M.buffer_bytes = buffer_bytes

-- sample-set: a sample's settings, only those given.

local LOOPS = named({ off = 1, forward = 2, reverse = 3, pingpong = 4 })
local NNAS = named({ cut = 1, off = 2, continue = 3 })
local SYNC_MODES = named({ repitch = 1, percussion = 2, texture = 3 })
local INTERPOLATIONS = named({ none = 1, linear = 2, cubic = 3, sinc = 4 })
local db_to_linear, linear_to_db = instruments_.db_to_linear, instruments_.linear_to_db

local function plain(field, check, says, text)
  return { get = function(s) return s[field] end, set = function(s, v) s[field] = v end, field = field, check = check, says = says, text = text }
end

local function enum(field, map, says, text)
  return {
    get = function(s) return map.get(s[field]) end, set = function(s, v) s[field] = map.parse(v) end, field = field,
    check = function(v) return map.parse(v) ~= nil end, says = says, text = text,
  }
end

local function frames_of(sample)
  local buffer = sample.sample_buffer
  return buffer.has_sample_data and buffer.number_of_frames or 1
end

-- A sample's set or chain: the number of one in its instrument, or 0. The state keeps the number; on the wire it is a place.
local function assignment(field, list, of, says, label)
  return {
    get = function(s) return s[field] end, set = function(s, v) s[field] = v end, field = field, says = says, of = of, list = list,
    text = function(v) return v == 0 and ("no " .. label) or (label .. " " .. v) end,
  }
end

local SAMPLE_FIELDS = {
  name = plain("name", function(v) return type(v) == "string" and #v <= 256 and not v:find("[%c]") end, "name must be one line of text",
    function(v) return "named " .. q(v) end),
  volumeDb = {
    get = function(s) return linear_to_db(s.volume) end, set = function(s, v) s.volume = db_to_linear(v) end, field = "volume",
    check = function(v) return type(v) == "number" and v == v and v <= 12.0412 end, says = "volumeDb is a number up to +12 (-200 or less is silence)",
    text = function(v) return "volume " .. base.db_text(v > -200 and v or nil) .. " dB" end,
  },
  panning = {
    get = function(s) return s.panning * 2 - 1 end, set = function(s, v) s.panning = (v + 1) / 2 end, field = "panning",
    check = function(v) return type(v) == "number" and v >= -1 and v <= 1 end, says = "panning is from -1 (left) to 1 (right)",
    text = function(v) return "pan " .. base.pan_text(v) end,
  },
  transpose = plain("transpose", whole(-120, 120), "transpose is whole semitones from -120 to 120", function(v) return "transpose " .. v end),
  fineTune = plain("fine_tune", whole(-127, 127), "fineTune is from -127 to 127", function(v) return "fine tune " .. v end),
  loop = enum("loop_mode", LOOPS, "loop is off, forward, reverse or pingpong", function(v) return "loop " .. v end),
  loopStart = plain("loop_start", function(v, s) return type(v) == "number" and v == math.floor(v) and v >= 1 and v <= frames_of(s) end,
    "loopStart is a frame of the sample, from 1", function(v) return "loop from frame " .. v end),
  loopEnd = plain("loop_end", function(v, s) return type(v) == "number" and v == math.floor(v) and v >= 1 and v <= frames_of(s) end,
    "loopEnd is a frame of the sample, up to its length", function(v) return "loop to frame " .. v end),
  loopRelease = plain("loop_release", is_boolean, "loopRelease is true or false", function(v) return v and "loop finishes after release" or "loop stops at release" end),
  beatSync = plain("beat_sync_enabled", is_boolean, "beatSync is true or false", function(v) return v and "beat sync on" or "beat sync off" end),
  beatSyncLines = plain("beat_sync_lines", whole(1, 512), "beatSyncLines is from 1 to 512", function(v) return "beat sync " .. v .. " lines" end),
  beatSyncMode = enum("beat_sync_mode", SYNC_MODES, "beatSyncMode is repitch, percussion or texture", function(v) return "beat sync " .. v end),
  nna = {
    get = function(s) return NNAS.get(s.new_note_action) end, set = function(s, v) s.new_note_action = NNAS.parse(v == "sustain" and "continue" or v) end,
    field = "new_note_action", parse = function(v) return v == "sustain" and "continue" or v end,
    check = function(v) return NNAS.parse(v) ~= nil end, says = "nna is cut, off or continue", text = function(v) return "new note " .. v end,
  },
  muteGroup = plain("mute_group", whole(0, 15), "muteGroup is from 0 (none) to 15", function(v) return v == 0 and "no mute group" or ("mute group " .. v) end),
  oneshot = plain("oneshot", is_boolean, "oneshot is true or false", function(v) return v and "one-shot" or "not one-shot" end),
  autoseek = plain("autoseek", is_boolean, "autoseek is true or false", function(v) return v and "autoseek on" or "autoseek off" end),
  autofade = plain("autofade", is_boolean, "autofade is true or false", function(v) return v and "autofade on" or "autofade off" end),
  interpolation = enum("interpolation_mode", INTERPOLATIONS, "interpolation is none, linear, cubic or sinc", function(v) return "interpolation " .. v end),
  oversample = plain("oversample_enabled", is_boolean, "oversample is true or false", function(v) return v and "oversampled" or "not oversampled" end),
  modulationSet = assignment("modulation_set_index", "modsets", "modset", "modulationSetRef is one of the instrument's modulation sets (a modset ref), or null for none", "modulation set"),
  fxChain = assignment("device_chain_index", "chains", "chain", "fxChainRef is one of the instrument's sample FX chains (a chain ref), or null for none", "FX chain"),
}
local SAMPLE_ORDER = { "name", "volumeDb", "panning", "transpose", "fineTune", "loop", "loopStart", "loopEnd", "loopRelease", "beatSync",
  "beatSyncLines", "beatSyncMode", "nna", "muteGroup", "oneshot", "autoseek", "autofade", "interpolation", "oversample", "modulationSet", "fxChain" }
M.SAMPLE_FIELDS, M.SAMPLE_ORDER = SAMPLE_FIELDS, SAMPLE_ORDER

-- The set or chain a request names (by its ref, in the sample's instrument), as its number; null is none.
local function assignment_wanted(field, value, env, n)
  if value == json.null or value == false then return 0 end
  local found = places_.numbers_of(env, field.of, value, field.of)
  if found[1] ~= n[1] then bad(field.says .. ": it must be in the sample's own instrument") end
  return found[2]
end

local sample_set = instruments_.settings_kind({
  kind = "sample-set", of = "sample", noun = "settings", undoable = true, order = SAMPLE_ORDER, fields = SAMPLE_FIELDS,
})
-- (what the request gives, by the sample it names: a set or chain is named by its ref)
local inner_wanted = sample_set.wanted
sample_set.wanted = nil
-- (a request names a set or chain by its ref; the state by the setting's name)
local inner_read = sample_set.read
sample_set.read = function(object, source)
  if type(source) == "table" and (source.modulationSetRef ~= nil or source.fxChainRef ~= nil) then
    local keys = {}
    for key, value in pairs(source) do keys[key] = value end
    if source.modulationSetRef ~= nil then keys.modulationSet = true end
    if source.fxChainRef ~= nil then keys.fxChain = true end
    source = keys
  end
  return inner_read(object, source)
end
sample_set.wanted_at = function(loc, input)
  local given = {}
  for key, value in pairs(input) do given[key] = value end
  for key, ref_key in pairs({ modulationSet = "modulationSetRef", fxChain = "fxChainRef" }) do
    given[key] = nil
    if input[ref_key] ~= nil then given[key] = assignment_wanted(SAMPLE_FIELDS[key], input[ref_key], loc.env, loc.numbers) end
  end
  -- (both ends of a loop given: checked together, the end after the start)
  local state = inner_wanted(loc.object, given)
  local start, finish = state.loopStart or loc.object.loop_start, state.loopEnd or loc.object.loop_end
  if (state.loopStart or state.loopEnd) and start > finish then bad("loopStart must be before loopEnd") end
  return state
end
-- (a set or chain on the wire is its place in the instrument, followed when the record is used)
sample_set.wire = function(state, env, loc)
  if type(state) ~= "table" or not loc or not loc.numbers then return state end
  local out
  for _, key in ipairs({ "modulationSet", "fxChain" }) do
    if type(state[key]) == "number" and state[key] > 0 then
      out = out or {}
      out[key] = { place = places_.place_of(env, SAMPLE_FIELDS[key].of, { loc.numbers[1], state[key] }) }
    end
  end
  if not out then return state end
  for k, v in pairs(state) do if out[k] == nil then out[k] = v end end
  return out
end
sample_set.unwire = function(state, env, loc)
  if type(state) ~= "table" then return state end
  local out
  for _, key in ipairs({ "modulationSet", "fxChain" }) do
    local kept = state[key]
    if type(kept) == "table" and kept.place then
      local n, why, list = places_.follow(env, kept.place)
      if not n then places_.follow_failed(why, list, SAMPLE_FIELDS[key].text(1)) end
      if loc and loc.numbers and n[1] ~= loc.numbers[1] then fail("changed", "that " .. SAMPLE_FIELDS[key].text(1) .. " is in another instrument now") end
      out = out or {}
      out[key] = n[2]
    end
  end
  if not out then return state end
  for k, v in pairs(state) do if out[k] == nil then out[k] = v end end
  return out
end
instruments_.instrument_spec(sample_set)
-- (the sample's numbers and the env, for what the request's refs name)
do
  local locate = sample_set.locate
  sample_set.locate = function(song, env, input)
    local loc = locate(song, env, input)
    loc.env = env
    return loc
  end
end

-- keyzone-set: where a sample plays in its instrument's keyzones (a slice's zone follows its sample: read-only).

local function range_check(low, high)
  return function(v)
    return type(v) == "table" and type(v[1]) == "number" and type(v[2]) == "number" and v[1] == math.floor(v[1])
      and v[2] == math.floor(v[2]) and v[1] >= low and v[2] <= high and v[1] <= v[2]
  end
end
local function copy_range(v) return { v[1], v[2] } end
local LAYERS = named({ ["note-on"] = 1, ["note-off"] = 2 })

local ZONE_FIELDS = {
  noteRange = { get = function(m) return copy_range(m.note_range) end, set = function(m, v) m.note_range = copy_range(v) end, field = "note_range",
    check = range_check(0, 119), says = "noteRange is [low, high], notes from 0 to 119 (C-4 is 48)", text = function(v) return "notes " .. base.notes_text(v[1], v[2]) end },
  velocityRange = { get = function(m) return copy_range(m.velocity_range) end, set = function(m, v) m.velocity_range = copy_range(v) end, field = "velocity_range",
    check = range_check(0, 127), says = "velocityRange is [low, high] from 0 to 127", text = function(v) return "velocity " .. v[1] .. "-" .. v[2] end },
  baseNote = plain("base_note", whole(0, 119), "baseNote is a note from 0 to 119 (C-4 is 48)", function(v) return "base note " .. base.note_name(v) end),
  layer = enum("layer", LAYERS, "layer is note-on or note-off", function(v) return "plays on " .. v end),
  keyTracking = plain("map_key_to_pitch", is_boolean, "keyTracking is true or false", function(v) return v and "pitch follows the key" or "fixed pitch" end),
  velocityToVolume = plain("map_velocity_to_volume", is_boolean, "velocityToVolume is true or false", function(v) return v and "velocity to volume" or "fixed volume" end),
}
local ZONE_ORDER = { "noteRange", "velocityRange", "baseNote", "layer", "keyTracking", "velocityToVolume" }

local keyzone = instruments_.settings_kind({
  kind = "keyzone-set", of = "sample", noun = "keyzone", undoable = true, order = ZONE_ORDER, fields = ZONE_FIELDS,
  object = function(objects)
    local mapping = objects.sample.sample_mapping
    return mapping
  end,
  label = function(label) return label .. " keyzone" end,
})
instruments_.instrument_spec(keyzone)
do
  local locate = keyzone.locate
  keyzone.locate = function(song, env, input)
    local loc = locate(song, env, input)
    if loc.object.read_only then bad("that sample is a slice (or the sliced sample): its keyzone follows the slice markers") end
    return loc
  end
end

-- keyzone-overlap: what plays when keyzones overlap (the instrument's).

local OVERLAPS = named({ all = 0, cycle = 1, random = 2 })
instruments_.instrument_spec(instruments_.settings_kind({
  kind = "keyzone-overlap", noun = "overlap", undoable = true, order = { "mode" },
  fields = {
    mode = { get = function(i) return OVERLAPS.get(i.sample_mapping_overlap_mode) end, set = function(i, v) i.sample_mapping_overlap_mode = OVERLAPS.parse(v) end,
      field = "sample_mapping_overlap_mode", check = function(v) return OVERLAPS.parse(v) ~= nil end, says = "mode is all (layered), cycle (round robin) or random",
      text = function(v) return v == "all" and "overlapping zones all play" or ("overlapping zones play in " .. (v == "cycle" and "turn" or "random order")) end },
  },
  title = function(name, _, to) return name .. ": " .. (to.mode == "all" and "overlapping zones all play" or ("overlapping zones play in " .. (to.mode == "cycle" and "turn" or "random order"))) end,
}))

-- keyzone-drumkit: one key per sample, from `first` up (white keys only when asked), the whole velocity range: Renoise's
-- Drumkit layout, as one change. The record keeps each sample's zone before and after, by its place.

local BLACK = { [1] = true, [3] = true, [6] = true, [8] = true, [10] = true }

local function zone_of(sample)
  local mapping = sample.sample_mapping
  return { noteRange = copy_range(mapping.note_range), velocityRange = copy_range(mapping.velocity_range), baseNote = mapping.base_note }
end

local function same_zone(a, b)
  return a.noteRange[1] == b.noteRange[1] and a.noteRange[2] == b.noteRange[2] and a.velocityRange[1] == b.velocityRange[1]
    and a.velocityRange[2] == b.velocityRange[2] and a.baseNote == b.baseNote
end

local function set_zone(sample, zone)
  local mapping = sample.sample_mapping
  -- (a range is set whole; the base note after it)
  mapping.note_range = copy_range(zone.noteRange)
  mapping.velocity_range = copy_range(zone.velocityRange)
  mapping.base_note = zone.baseNote
end

local function drumkit_plan(song, env, input)
  local n, objects, label = places_.locate(song, env, "instrument", input, "keyzone-drumkit")
  local instrument = objects.instrument
  local chosen = {}
  if input.sampleRefs ~= nil then
    if type(input.sampleRefs) ~= "table" or #input.sampleRefs == 0 then bad("sampleRefs: the samples to lay out, in key order") end
    local seen = {}
    for _, reference in ipairs(input.sampleRefs) do
      local found = places_.numbers_of(env, "sample", reference, "sampleRefs")
      if found[1] ~= n[1] then bad("sampleRefs must be samples of that instrument") end
      if seen[found[2]] then bad("a sample is given twice in sampleRefs") end
      seen[found[2]] = true
      chosen[#chosen + 1] = found[2]
    end
  else
    for s = 1, #instrument.samples do chosen[#chosen + 1] = s end
  end
  if #chosen == 0 then bad(q(label) .. " has no samples to lay out") end
  local first = input.first == nil and 48 or input.first
  if not whole(0, 119)(first) then bad("first is a note from 0 to 119 (C-4 is 48)") end
  local white = input.whiteKeys == true
  local keys, key = {}, first
  for k = 1, #chosen do
    while white and BLACK[key % 12] do key = key + 1 end
    if key > 119 then bad(string.format("%d samples from note %d run past the top of the keyboard (119)", #chosen, first)) end
    keys[k] = key
    key = key + 1
  end
  local zones, before = {}, {}
  for k, s in ipairs(chosen) do
    local sample = instrument:sample(s)
    if sample.sample_mapping.read_only then bad(q(label) .. " is sliced: its slices' keys follow the slice markers") end
    before[k] = zone_of(sample)
    zones[k] = { noteRange = { keys[k], keys[k] }, velocityRange = { 0, 127 }, baseNote = keys[k] }
  end
  return { n = n, label = label, samples = chosen, zones = zones, before = before, count = #instrument.samples }
end

local function drumkit_title(plan)
  local function note(k) return ({ "C-", "C#", "D-", "D#", "E-", "F-", "F#", "G-", "G#", "A-", "A#", "B-" })[k % 12 + 1] .. math.floor(k / 12) end
  return string.format("Laid out %s as a drum kit: %d %s from %s", q(plan.label), #plan.samples, #plan.samples == 1 and "sample" or "samples", note(plan.zones[1].baseNote))
end

-- All the instrument's zones, in sample order (what the journal compares for the producer's undo).
local function all_zones(instrument)
  local out = {}
  for s = 1, #instrument.samples do out[s] = zone_of(instrument:sample(s)) end
  return { zones = out }
end

local function same_zones(a, b)
  if type(a.zones) ~= "table" or type(b.zones) ~= "table" or #a.zones ~= #b.zones then return false end
  for k = 1, #a.zones do if not same_zone(a.zones[k], b.zones[k]) then return false end end
  return true
end

local function drumkit_preview(song, env, input)
  local plan = drumkit_plan(song, env, input)
  local unchanged = true
  for k = 1, #plan.zones do if not same_zone(plan.zones[k], plan.before[k]) then unchanged = false end end
  local display = { title = drumkit_title(plan), before = { zones = plan.before }, after = { zones = plan.zones }, unchanged = unchanged or nil }
  return {
    target = places_.target_of(env, "instrument", plan.n), prior = { zones = plan.before },
    fence = { samples = plan.samples, zones = plan.before, count = plan.count }, display = display,
  }
end

local function drumkit_apply(song, env, input, fence, desc)
  local ok, plan = pcall(drumkit_plan, song, env, input)
  if not ok then base.gone_is_changed(plan) end
  if plan.count ~= fence.count or not base.same_table(plan.samples, fence.samples) then fail("changed", q(plan.label) .. "'s samples changed since the preview") end
  for k = 1, #plan.before do
    if not same_zone(plan.before[k], fence.zones[k]) then fail("changed", q(plan.label) .. "'s keyzones changed since the preview") end
  end
  local instrument = song:instrument(plan.n[1])
  local journal_prior = all_zones(instrument)
  song:describe_undo(desc)
  for k, s in ipairs(plan.samples) do set_zone(instrument:sample(s), plan.zones[k]) end
  local places = {}
  for k, s in ipairs(plan.samples) do places[k] = places_.place_of(env, "sample", { plan.n[1], s }) end
  for k, s in ipairs(plan.samples) do
    if not same_zone(zone_of(instrument:sample(s)), plan.zones[k]) then
      for j, back in ipairs(plan.samples) do pcall(set_zone, instrument:sample(back), plan.before[j]) end
      fail("failed", "Renoise didn't keep the keyzones as laid out")
    end
  end
  if env.watch_undo then
    for _, s in ipairs(plan.samples) do env.watch_undo(instrument:sample(s).sample_mapping.note_range_observable) end
  end
  return {
    applied = { zones = plan.zones }, material = { samples = places }, target = places_.target_of(env, "instrument", plan.n),
    display = { title = drumkit_title(plan), before = { zones = plan.before }, after = { zones = plan.zones } },
    undo = { desc = desc, entry = true }, prior = journal_prior, journal_applied = all_zones(instrument),
    place = places_.journal_place("instrument", plan.n),
  }
end

local function drumkit_restore(song, env, restore, desc)
  local material, prior, applied = restore.material, restore.prior, restore.applied
  if type(material) ~= "table" or type(material.samples) ~= "table" or type(prior) ~= "table" or type(applied) ~= "table" then
    bad("material, prior and applied: the samples and their zones")
  end
  local found, todo = {}, false
  for k, kept in ipairs(material.samples) do
    local n, why, list = places_.follow(env, kept)
    if not n then places_.follow_failed(why, list, "a sample of the kit") end
    local sample = song:instrument(n[1]):sample(n[2])
    local now = zone_of(sample)
    if not same_zone(now, prior.zones[k]) then
      if not restore.discard and not same_zone(now, applied.zones[k]) then
        fail("changed", string.format("the keyzone of %s changed in Renoise since Kumi laid out the kit", q(places_.sample_label(n[2], sample))))
      end
      todo = true
    end
    found[k] = n
  end
  if not todo then return { restored = { done = true }, display = { title = "The keyzones are back already" }, undo = { desc = desc, entry = false }, unwritten = true } end
  song:describe_undo(desc)
  for k, n in ipairs(found) do set_zone(song:instrument(n[1]):sample(n[2]), prior.zones[k]) end
  if found[1] then base.note_restored_at(env.journal, "keyzone-drumkit", places_.journal_place("instrument", { found[1][1] })) end
  return { restored = { done = true }, display = { title = "Keyzones put back as they were" }, undo = { desc = desc, entry = true } }
end

base.register({
  kind = "keyzone-drumkit", custom = true, by_place = true,
  preview = drumkit_preview, apply = drumkit_apply, restore = drumkit_restore,
  reread = places_.reread("instrument"), read = function(instrument) return all_zones(instrument) end, same = same_zones,
})

-- keyzone-map (#401, sample mapping): an instrument's samples laid out as given, each its own zone, velocity range and base
-- note, and the instrument's overlap mode, as one change: what Kumi's sample map plans for an instrument the song has. The
-- record keeps each sample's zone before and after by its place, and the overlap mode; the undo puts back only what still
-- holds what Kumi set.

local function map_plan(song, env, input)
  local n, objects, label = places_.locate(song, env, "instrument", input, "keyzone-map")
  local instrument = objects.instrument
  if type(input.samples) ~= "table" or #input.samples == 0 then bad("samples: each sample's {sampleRef, noteRange, velocityRange?, baseNote}") end
  if #input.samples > #instrument.samples then bad("there are more samples than " .. q(label) .. " holds") end
  local chosen, zones, before, seen = {}, {}, {}, {}
  local in_keys, in_velocity = whole(0, 119), whole(0, 127)
  for k, given in ipairs(input.samples) do
    if type(given) ~= "table" then bad("each sample is {sampleRef, noteRange, velocityRange?, baseNote}") end
    local found = places_.numbers_of(env, "sample", given.sampleRef, "sampleRef")
    if found[1] ~= n[1] then bad("every sampleRef must be a sample of that instrument") end
    if seen[found[2]] then bad("a sample is given twice") end
    seen[found[2]] = true
    local sample = instrument:sample(found[2])
    if sample.sample_mapping.read_only then bad(q(label) .. " is sliced: its slices' keys follow the slice markers") end
    local notes, velocity = given.noteRange, given.velocityRange or { 0, 127 }
    if type(notes) ~= "table" or not in_keys(notes[1]) or not in_keys(notes[2]) or notes[1] > notes[2] then
      bad("a sample's noteRange is [low, high], notes from 0 to 119")
    end
    if type(velocity) ~= "table" or not in_velocity(velocity[1]) or not in_velocity(velocity[2]) or velocity[1] > velocity[2] then
      bad("a sample's velocityRange is [low, high], from 0 to 127")
    end
    if not in_keys(given.baseNote) then bad("a sample's baseNote is the note it sounds at, 0 to 119 (C-4 is 48)") end
    chosen[k] = found[2]
    before[k] = zone_of(sample)
    zones[k] = { noteRange = { notes[1], notes[2] }, velocityRange = { velocity[1], velocity[2] }, baseNote = given.baseNote }
  end
  if input.overlap ~= nil and OVERLAPS.parse(input.overlap) == nil then bad("overlap is all (layered), cycle (round robin) or random") end
  return {
    n = n, label = label, samples = chosen, zones = zones, before = before, count = #instrument.samples,
    overlap = input.overlap, overlap_before = OVERLAPS.get(instrument.sample_mapping_overlap_mode),
  }
end

local function map_title(plan)
  local words = string.format("Laid out %s: %d %s", q(plan.label), #plan.samples, #plan.samples == 1 and "sample" or "samples")
  if plan.overlap and plan.overlap ~= plan.overlap_before then
    words = words .. (plan.overlap == "cycle" and ", round robins in turn" or plan.overlap == "random" and ", round robins at random" or ", layers together")
  end
  return words
end

local function map_preview(song, env, input)
  local plan = map_plan(song, env, input)
  local unchanged = plan.overlap == nil or plan.overlap == plan.overlap_before
  for k = 1, #plan.zones do if not same_zone(plan.zones[k], plan.before[k]) then unchanged = false end end
  return {
    target = places_.target_of(env, "instrument", plan.n), prior = { zones = plan.before, overlap = plan.overlap_before },
    fence = { samples = plan.samples, zones = plan.before, count = plan.count, overlap = plan.overlap_before },
    display = { title = map_title(plan), before = { zones = plan.before }, after = { zones = plan.zones }, unchanged = unchanged or nil },
  }
end

local function map_apply(song, env, input, fence, desc)
  local ok, plan = pcall(map_plan, song, env, input)
  if not ok then base.gone_is_changed(plan) end
  if plan.count ~= fence.count or not base.same_table(plan.samples, fence.samples) then fail("changed", q(plan.label) .. "'s samples changed since the preview") end
  if plan.overlap_before ~= fence.overlap then fail("changed", q(plan.label) .. "'s overlap changed since the preview") end
  for k = 1, #plan.before do
    if not same_zone(plan.before[k], fence.zones[k]) then fail("changed", q(plan.label) .. "'s keyzones changed since the preview") end
  end
  local instrument = song:instrument(plan.n[1])
  local journal_prior = all_zones(instrument)
  journal_prior.overlap = plan.overlap_before
  song:describe_undo(desc)
  for k, s in ipairs(plan.samples) do set_zone(instrument:sample(s), plan.zones[k]) end
  if plan.overlap then instrument.sample_mapping_overlap_mode = OVERLAPS.parse(plan.overlap) end
  local held = not plan.overlap or OVERLAPS.get(instrument.sample_mapping_overlap_mode) == plan.overlap
  for k, s in ipairs(plan.samples) do if not same_zone(zone_of(instrument:sample(s)), plan.zones[k]) then held = false end end
  if not held then
    for k, s in ipairs(plan.samples) do pcall(set_zone, instrument:sample(s), plan.before[k]) end
    pcall(function() instrument.sample_mapping_overlap_mode = OVERLAPS.parse(plan.overlap_before) end)
    fail("failed", "Renoise didn't keep the keyzones as laid out")
  end
  local places = {}
  for k, s in ipairs(plan.samples) do places[k] = places_.place_of(env, "sample", { plan.n[1], s }) end
  if env.watch_undo then
    for _, s in ipairs(plan.samples) do env.watch_undo(instrument:sample(s).sample_mapping.note_range_observable) end
  end
  local after_overlap = OVERLAPS.get(instrument.sample_mapping_overlap_mode)
  local journal_applied = all_zones(instrument)
  journal_applied.overlap = after_overlap
  return {
    applied = { zones = plan.zones, overlap = after_overlap },
    material = { samples = places, instrument = places_.place_of(env, "instrument", plan.n) },
    target = places_.target_of(env, "instrument", plan.n),
    display = { title = map_title(plan), before = { zones = plan.before }, after = { zones = plan.zones } },
    undo = { desc = desc, entry = true }, prior = journal_prior, journal_applied = journal_applied,
    place = places_.journal_place("instrument", plan.n),
  }
end

local function map_restore(song, env, restore, desc)
  local material, prior, applied = restore.material, restore.prior, restore.applied
  if type(material) ~= "table" or type(material.samples) ~= "table" or type(prior) ~= "table" or type(applied) ~= "table"
    or type(prior.zones) ~= "table" or type(applied.zones) ~= "table" then
    bad("material, prior and applied: the samples, their zones and the overlap")
  end
  local found, todo = {}, false
  for k, kept in ipairs(material.samples) do
    local n, why, list = places_.follow(env, kept)
    if not n then places_.follow_failed(why, list, "a sample Kumi laid out") end
    local sample = song:instrument(n[1]):sample(n[2])
    local now = zone_of(sample)
    if not same_zone(now, prior.zones[k]) then
      if not restore.discard and not same_zone(now, applied.zones[k]) then
        fail("changed", string.format("the keyzone of %s changed in Renoise since Kumi laid it out", q(places_.sample_label(n[2], sample))))
      end
      todo = true
    end
    found[k] = n
  end
  local at
  if material.instrument then
    local n, why, list = places_.follow(env, material.instrument)
    if not n then places_.follow_failed(why, list, "the instrument Kumi laid out") end
    at = n[1]
  elseif found[1] then
    at = found[1][1]
  end
  local overlap_back = false
  if at and prior.overlap and applied.overlap then
    local now = OVERLAPS.get(song:instrument(at).sample_mapping_overlap_mode)
    if now ~= prior.overlap then
      if not restore.discard and now ~= applied.overlap then fail("changed", "the instrument's overlap changed in Renoise since Kumi laid it out") end
      overlap_back = true
    end
  end
  if not todo and not overlap_back then
    return { restored = { done = true }, display = { title = "The keyzones are back already" }, undo = { desc = desc, entry = false }, unwritten = true }
  end
  song:describe_undo(desc)
  for k, n in ipairs(found) do set_zone(song:instrument(n[1]):sample(n[2]), prior.zones[k]) end
  if overlap_back then song:instrument(at).sample_mapping_overlap_mode = OVERLAPS.parse(prior.overlap) end
  if at then base.note_restored_at(env.journal, "keyzone-map", places_.journal_place("instrument", { at })) end
  return { restored = { done = true }, display = { title = "Keyzones put back as they were" }, undo = { desc = desc, entry = true } }
end

base.register({
  kind = "keyzone-map", custom = true, by_place = true,
  preview = map_preview, apply = map_apply, restore = map_restore, reread = places_.reread("instrument"),
  read = function(instrument)
    local state = all_zones(instrument)
    state.overlap = OVERLAPS.get(instrument.sample_mapping_overlap_mode)
    return state
  end,
  same = function(a, b) return same_zones(a, b) and a.overlap == b.overlap end,
})

-- Samples loaded, deleted

local SETTINGS_KEPT = { "name", "volumeDb", "panning", "transpose", "fineTune", "loop", "loopStart", "loopEnd", "loopRelease", "beatSync",
  "beatSyncLines", "beatSyncMode", "nna", "muteGroup", "oneshot", "autoseek", "autofade", "interpolation", "oversample" }

-- Everything of a sample a delete keeps, but its buffer: its settings, keyzone, set and chain (as places).
local function sample_settings(env, n, sample)
  local settings = {}
  for _, key in ipairs(SETTINGS_KEPT) do settings[key] = SAMPLE_FIELDS[key].get(sample) end
  local mapping = sample.sample_mapping
  local zone = {}
  for _, key in ipairs(ZONE_ORDER) do zone[key] = ZONE_FIELDS[key].get(mapping) end
  local kept = { settings = settings, zone = zone }
  if sample.modulation_set_index > 0 then kept.modulationSet = places_.place_of(env, "modset", { n[1], sample.modulation_set_index }) end
  if sample.device_chain_index > 0 then kept.fxChain = places_.place_of(env, "chain", { n[1], sample.device_chain_index }) end
  return kept
end
M.sample_settings = sample_settings

-- Puts kept settings on a sample: the loop's points within its frames (the end first when it moves past the start); a set or
-- chain where it is now (left out when it went: counted in `lost`).
local function put_settings(env, sample, kept, keyzone_too)
  local settings = kept.settings or {}
  local lost = 0
  for _, key in ipairs(SETTINGS_KEPT) do
    if settings[key] ~= nil and key ~= "loopStart" and key ~= "loopEnd" then pcall(SAMPLE_FIELDS[key].set, sample, settings[key]) end
  end
  if settings.loopStart and settings.loopEnd then
    local frames = frames_of(sample)
    local start, finish = math.min(settings.loopStart, frames), math.min(settings.loopEnd, frames)
    pcall(function()
      if start > sample.loop_end then sample.loop_end = finish sample.loop_start = start else sample.loop_start = start sample.loop_end = finish end
    end)
  end
  if keyzone_too and kept.zone and not sample.sample_mapping.read_only then
    for _, key in ipairs(ZONE_ORDER) do
      if kept.zone[key] ~= nil then pcall(ZONE_FIELDS[key].set, sample.sample_mapping, kept.zone[key]) end
    end
  end
  for key, field in pairs({ modulationSet = "modulation_set_index", fxChain = "device_chain_index" }) do
    if kept[key] then
      local n = places_.follow(env, kept[key])
      if n then pcall(function() sample[field] = n[2] end) else lost = lost + 1 end
    end
  end
  return lost
end
M.put_settings = put_settings

local function sample_target(env, n) return places_.target_of(env, "sample", n) end

-- sample-load {ref: instrument (a new sample at index, the end by default) | ref: sample (its audio replaced), path, name?}

local function load_plan(song, env, input)
  local path = instruments_.load_path(input, instruments_.SAMPLE_FILES)
  local parsed = type(input.ref) == "string" and input.ref:match("^[^:]+:(%a+):") or nil
  if parsed == "sample" then
    local n, objects, label = places_.locate(song, env, "sample", input, "sample-load")
    local sample = objects.sample
    if sample.is_slice_alias then bad(label .. " is a slice: load into the sliced sample's instrument instead") end
    if #sample.slice_markers > 0 then bad(label .. " is sliced: delete its slices before replacing its audio") end
    if buffer_bytes(sample.sample_buffer) > M.MAX_SAMPLE_BYTES then
      fail("failed", string.format("%s holds %.0f MB, more than Kumi keeps to put it back, so it won't replace it", label, buffer_bytes(sample.sample_buffer) / 1048576))
    end
    return { path = path, replace = n, label = label, print = M.buffer_print(sample.sample_buffer), name = sample.name }
  end
  local n, objects, label = places_.locate(song, env, "instrument", input, "sample-load")
  local instrument = objects.instrument
  if instrument.samples[1] and #instrument.samples[1].slice_markers > 0 then bad(q(label) .. " is sliced: Renoise takes no other samples in it") end
  local count = #instrument.samples
  -- (`index` by Renoise's number, from 0, as it numbers samples from 00)
  local index = input.index == nil and count or input.index
  if not whole(0, count)(index) then bad(string.format("index is a sample's number from 0 to %d, as Renoise numbers them (00 the first)", count)) end
  index = index + 1
  return { path = path, instrument = n[1], index = index, count = count, label = label }
end

local function file_name(path) return path:match("([^/\\]+)%.[^.]+$") or path end

local function load_preview(song, env, input)
  local plan = load_plan(song, env, input)
  if input.name ~= nil and not SAMPLE_FIELDS.name.check(input.name) then bad(SAMPLE_FIELDS.name.says) end
  if plan.replace then
    return {
      target = sample_target(env, plan.replace), prior = { print = plan.print },
      fence = { print = plan.print, name = plan.name },
      display = { title = string.format("Loaded %s into %s", q(file_name(plan.path)), plan.label), note = "Kumi keeps the audio it replaces, to put back." },
    }
  end
  return {
    target = places_.target_of(env, "instrument", { plan.instrument }), prior = { samples = plan.count },
    fence = { samples = plan.count, index = plan.index },
    display = { title = string.format("Loaded %s into %s", q(file_name(plan.path)), q(plan.label)), before = { samples = plan.count }, after = { samples = plan.count + 1 } },
  }
end

local function load_apply(song, env, input, fence, desc)
  local ok, plan = pcall(load_plan, song, env, input)
  if not ok then base.gone_is_changed(plan) end
  if plan.replace then
    local sample = song:instrument(plan.replace[1]):sample(plan.replace[2])
    if not base.same_table(M.buffer_print(sample.sample_buffer), fence.print) or sample.name ~= fence.name then fail("changed", plan.label .. " changed since the preview") end
    -- (kept first: if Renoise can't save it, nothing is replaced)
    local kept = { name = sample.name, loopStart = sample.loop_start, loopEnd = sample.loop_end, loop = LOOPS.get(sample.loop_mode) }
    if sample.sample_buffer.has_sample_data then
      kept.file = places_.temp_file(".flac")
      if not sample.sample_buffer:save_as(kept.file, "flac") or not io.exists(kept.file) then
        fail("failed", "Renoise wouldn't save the audio of " .. plan.label .. " to keep it, so Kumi didn't replace it")
      end
    end
    song:describe_undo(desc)
    local loaded = sample.sample_buffer:load_from(plan.path)
    if not loaded then
      -- (the audio it had, loaded again if Renoise cleared it before refusing)
      local back = true
      if kept.file and not base.same_table(M.buffer_print(sample.sample_buffer), fence.print) then
        back = pcall(function() if not sample.sample_buffer:load_from(kept.file) then error("refused") end end)
      end
      fail("failed", "Renoise wouldn't load " .. plan.path .. (back and "" or "; the audio it had couldn't be loaded back either: Renoise's own undo may bring it"))
    end
    if input.name ~= nil then sample.name = input.name end
    if env.watch_undo then env.watch_undo(sample.sample_buffer_observable) end
    local print = M.buffer_print(sample.sample_buffer)
    return {
      applied = { print = print, name = sample.name }, material = { old = kept, at = places_.place_of(env, "sample", plan.replace) },
      target = sample_target(env, plan.replace),
      display = { title = string.format("Loaded %s into %s", q(file_name(plan.path)), plan.label) },
      undo = { desc = desc, entry = true }, prior = { print = fence.print, name = fence.name },
      place = places_.journal_place("sample", plan.replace),
    }
  end
  local instrument = song:instrument(plan.instrument)
  if #instrument.samples ~= fence.samples then fail("changed", q(plan.label) .. "'s samples changed since the preview") end
  song:describe_undo(desc)
  local sample = instrument:insert_sample_at(plan.index)
  local loaded = sample.sample_buffer:load_from(plan.path)
  if not loaded then
    pcall(delete_sample, instrument, plan.index)
    fail("failed", "Renoise wouldn't load " .. plan.path)
  end
  if input.name ~= nil then sample.name = input.name elseif sample.name == "" then sample.name = file_name(plan.path) end
  local n = { plan.instrument, plan.index }
  return {
    applied = { print = M.buffer_print(sample.sample_buffer), name = sample.name, work = places_.sample_work(sample) },
    created = { ref = places_.ref_of(env, "sample", n), kind = "sample" }, material = { made = places_.place_of(env, "sample", n) },
    target = sample_target(env, n),
    display = { title = string.format("Loaded %s into %s", q(file_name(plan.path)), q(plan.label)), before = { samples = plan.count }, after = { samples = #instrument.samples } },
    undo = { desc = desc, entry = true }, prior = { samples = plan.count }, place = places_.journal_place("sample", n), shape = "made",
  }
end

local function load_restore(song, env, restore, desc)
  local material, applied = restore.material, restore.applied
  if type(material) ~= "table" or type(applied) ~= "table" then bad("material and applied: what the load made or replaced") end
  local kept_place = material.made or material.at
  local n, why, list = places_.follow(env, kept_place)
  if not n then
    if why == "gone" and material.made then fail("gone", "the sample Kumi loaded isn't in the instrument any more") end
    places_.follow_failed(why, list, applied.name)
  end
  local instrument = song:instrument(n[1])
  local sample = instrument:sample(n[2])
  local label = string.format("%s in %s", q(places_.sample_label(n[2], sample)), q(places_.instrument_label(n[1], instrument)))
  local now = M.buffer_print(sample.sample_buffer)
  if material.made then
    if not restore.discard and (not base.same_table(now, applied.print) or sample.name ~= applied.name
      or (applied.work ~= nil and not base.same_table(places_.sample_work(sample), applied.work))) then
      fail("changed", label .. " changed in Renoise since Kumi loaded it (its audio, slices or settings)")
    end
    song:describe_undo(desc)
    delete_sample(instrument, n[2])
    base.note_restored_at(env.journal, "sample-load", places_.journal_place("sample", n))
    return { restored = { deleted = true }, display = { title = "Removed " .. label .. " (put back)" }, undo = { desc = desc, entry = true } }
  end
  local old = material.old or {}
  if base.same_table(now, (restore.prior or {}).print) and sample.name == old.name then
    return { restored = { done = true }, display = { title = label .. " has its audio back already" }, undo = { desc = desc, entry = false }, unwritten = true }
  end
  if not restore.discard and (not base.same_table(now, applied.print) or sample.name ~= applied.name) then
    fail("changed", label .. " changed in Renoise since Kumi loaded into it")
  end
  if old.file and not io.exists(old.file) then fail("failed", "the audio Kumi kept is gone (Renoise empties its temporary folder as it quits)") end
  song:describe_undo(desc)
  if old.file then
    if not sample.sample_buffer:load_from(old.file) then fail("failed", "Renoise wouldn't load the kept audio back") end
  else
    sample.sample_buffer:delete_sample_data()
  end
  put_settings(env, sample, { settings = { name = old.name, loop = old.loop, loopStart = old.loopStart, loopEnd = old.loopEnd } }, false)
  base.note_restored(env.journal, "sample-load", places_.journal_place("sample", n), applied, base.KINDS["sample-load"])
  return { restored = { done = true }, display = { title = label .. " has its audio back" }, undo = { desc = desc, entry = true } }
end

base.register({
  kind = "sample-load", custom = true, by_place = true,
  preview = load_preview, apply = load_apply, restore = load_restore,
  reread = places_.reread("sample"),
  read = function(sample) return { print = M.buffer_print(sample.sample_buffer), name = sample.name } end,
  same = function(a, b) return base.same_table(a.print, b.print) and a.name == b.name end,
})

-- sample-delete {ref}: Kumi keeps its audio (a FLAC in Renoise's temporary folder), settings, keyzone, set and chain, and, for
-- the sliced sample, its markers and every slice's settings; the undo puts it back after the sample that was before it.

local function delete_fence(n, sample)
  return { index = n[2], name = sample.name, print = M.buffer_print(sample.sample_buffer), markers = #sample.slice_markers }
end

local function delete_preview(song, env, input)
  local n, objects, label = places_.locate(song, env, "sample", input, "sample-delete")
  local sample = objects.sample
  if sample.is_slice_alias then bad(label .. " is a slice: delete its slice marker instead (sample-slices)") end
  local display = { title = "Deleted sample " .. label, note = "Kumi keeps its audio and settings to put back. Phrases name samples by number (Renoise doesn't renumber them): the samples after it move down one until it is back." }
  if #sample.slice_markers > 0 then display.note = "Its slices go with it; Kumi keeps its audio, markers and every slice's settings to put back." end
  if buffer_bytes(sample.sample_buffer) > M.MAX_SAMPLE_BYTES then
    display.note = string.format("It holds %.0f MB, more than Kumi keeps to put it back: the apply will refuse.", buffer_bytes(sample.sample_buffer) / 1048576)
  end
  return { target = sample_target(env, n), prior = { deleted = false }, fence = delete_fence(n, sample), display = display }
end

local function delete_apply(song, env, input, fence, desc)
  local ok, n, objects, label = pcall(places_.locate, song, env, "sample", input, "sample-delete")
  if not ok then base.gone_is_changed(n) end
  local instrument, sample = objects.instrument, objects.sample
  if not base.same_table(delete_fence(n, sample), fence) then fail("changed", label .. " changed since the preview") end
  if sample.is_slice_alias then bad(label .. " is a slice: delete its slice marker instead") end
  local bytes = buffer_bytes(sample.sample_buffer)
  if bytes > M.MAX_SAMPLE_BYTES then
    fail("failed", string.format("%s holds %.0f MB, more than Kumi keeps to put it back, so it won't delete it", label, bytes / 1048576))
  end
  local material = sample_settings(env, n, sample)
  material.index, material.print = n[2], M.buffer_print(sample.sample_buffer)
  if sample.sample_buffer.has_sample_data then
    material.file = places_.temp_file(".flac")
    if not sample.sample_buffer:save_as(material.file, "flac") or not io.exists(material.file) then
      fail("failed", "Renoise wouldn't save the audio of " .. label .. " to keep it, so Kumi didn't delete it")
    end
  end
  if #sample.slice_markers > 0 then
    material.markers = sample.slice_markers
    material.slices = {}
    for s = 2, #instrument.samples do
      local alias = instrument:sample(s)
      if alias.is_slice_alias then material.slices[#material.slices + 1] = sample_settings(env, { n[1], s }, alias) end
    end
  end
  song:describe_undo(desc)
  delete_sample(instrument, n[2])
  material.instrument = places_.place_of(env, "instrument", { n[1] })
  material.after = env.list_mark and env.list_mark("samples", n[1]) or nil
  return {
    applied = { deleted = true }, material = material, target = places_.target_of(env, "instrument", { n[1] }),
    display = { title = "Deleted sample " .. label }, undo = { desc = desc, entry = true },
    prior = { deleted = false }, place = places_.journal_place("sample", n), shape = "deleted",
  }
end

local function delete_restore(song, env, restore, desc)
  local material = restore.material
  if type(material) ~= "table" or type(material.instrument) ~= "table" or type(material.index) ~= "number" then bad("material: what the delete kept") end
  local name = (material.settings or {}).name
  local label = q(name ~= nil and name ~= "" and name or string.format("Sample %02X", material.index - 1))
  local n, why, list = places_.follow(env, material.instrument)
  if not n then places_.follow_failed(why, list, name) end
  local i = n[1]
  local instrument = song:instrument(i)
  local at = env.list_after and env.list_after("samples", i, nil, material.index, material.after) or nil
  if not at then
    fail("stale", string.format("the samples of %s changed in a way Kumi couldn't follow since it deleted %s, so it can't tell where it goes: nothing was put back", q(places_.instrument_label(i, instrument)), label))
  end
  at = math.max(1, math.min(at or 1, #instrument.samples + 1))
  if at <= #instrument.samples then
    local there = instrument:sample(at)
    if there.name == name and base.same_table(M.buffer_print(there.sample_buffer), material.print) then
      base.note_restored_at(env.journal, "sample-delete", places_.journal_place("sample", { i, at }))
      return { restored = { ref = places_.ref_of(env, "sample", { i, at }) }, display = { title = label .. " is back already" }, undo = { desc = desc, entry = false }, unwritten = true }
    end
  end
  if instrument.samples[1] and #instrument.samples[1].slice_markers > 0 then
    fail("changed", q(places_.instrument_label(i, instrument)) .. " is sliced now: Renoise takes no other samples in it")
  end
  -- (a sliced sample is its instrument's only one: its slices come back only into an instrument that holds no other)
  if material.markers and #instrument.samples > 0 then
    fail("changed", string.format("%s was sliced, and %s holds other samples now: its slices can't come back beside them, so Kumi left it",
      label, q(places_.instrument_label(i, instrument))))
  end
  if material.file and not io.exists(material.file) then fail("failed", "the audio Kumi kept is gone (Renoise empties its temporary folder as it quits)") end
  song:describe_undo(desc)
  local sample = instrument:insert_sample_at(at)
  if material.file and not sample.sample_buffer:load_from(material.file) then
    pcall(delete_sample, instrument, at)
    fail("failed", "Renoise wouldn't load the kept audio of " .. label .. " back: nothing was put back")
  end
  -- (the sample Kumi deleted, at its place again: what was kept by its place before the delete follows it)
  if env.back then env.back("samples", i, nil, material.after, at) end
  local lost = put_settings(env, sample, material, true)
  if material.markers then
    local sliced = pcall(function() sample.slice_markers = material.markers end)
    if not sliced or #sample.slice_markers ~= #material.markers then
      pcall(delete_sample, instrument, at)
      fail("failed", "Renoise wouldn't slice " .. label .. " again: nothing was put back")
    end
    for k, kept in ipairs(material.slices or {}) do
      local alias = instrument.samples[at + k] and instrument:sample(at + k)
      if alias and alias.is_slice_alias then lost = lost + put_settings(env, alias, kept, false) end
    end
  end
  base.note_restored_at(env.journal, "sample-delete", places_.journal_place("sample", { i, at }))
  return {
    restored = { ref = places_.ref_of(env, "sample", { i, at }) },
    display = { title = label .. " put back", note = lost > 0 and "Its modulation set or FX chain was deleted since: it plays without it." or nil },
    undo = { desc = desc, entry = true },
  }
end

base.register({
  kind = "sample-delete", custom = true, shape = "deleted", by_place = true,
  preview = delete_preview, apply = delete_apply, restore = delete_restore,
})

-- sample-slices {ref: the first sample, insert?: [frame…], delete?: [frame…], move?: [{from, to}…]}: slice markers, through
-- Renoise's own calls (which keep the notes and 0S commands playing the slices they played). The record keeps the markers
-- before and after, and each deleted slice's settings.

local function slices_plan(song, env, input)
  local n, objects, label = places_.locate(song, env, "sample", input, "sample-slices")
  local sample = objects.sample
  if n[2] ~= 1 then bad("only an instrument's first sample can be sliced: give the first sample's ref") end
  if sample.is_slice_alias then bad(label .. " is a slice: give the sliced sample") end
  if not sample.sample_buffer.has_sample_data then bad(label .. " has no audio to slice") end
  local frames = sample.sample_buffer.number_of_frames
  local markers = sample.slice_markers
  local present = {}
  for _, at in ipairs(markers) do present[at] = true end
  local ops, count = {}, 0
  local function frame(v, what)
    if type(v) ~= "number" or v ~= math.floor(v) or v < 1 or v > frames then bad(string.format("%s is a frame of the sample, from 1 to %d", what, frames)) end
    return v
  end
  for _, at in ipairs(input.delete or {}) do
    frame(at, "delete")
    if not present[at] then bad(string.format("there's no slice marker at frame %d", at)) end
    present[at] = nil
    ops[#ops + 1] = { op = "delete", at = at }
    count = count + 1
  end
  for _, move in ipairs(input.move or {}) do
    if type(move) ~= "table" then bad("move: a list of {from, to}") end
    frame(move.from, "move.from")
    frame(move.to, "move.to")
    if not present[move.from] then bad(string.format("there's no slice marker at frame %d", move.from)) end
    if present[move.to] then bad(string.format("there's a slice marker at frame %d already", move.to)) end
    present[move.from], present[move.to] = nil, true
    ops[#ops + 1] = { op = "move", from = move.from, to = move.to }
    count = count + 1
  end
  for _, at in ipairs(input.insert or {}) do
    frame(at, "insert")
    if present[at] then bad(string.format("there's a slice marker at frame %d already", at)) end
    present[at] = true
    ops[#ops + 1] = { op = "insert", at = at }
    count = count + 1
  end
  if count == 0 then bad("give insert, delete or move: frames of the sample") end
  if count > M.MAX_SLICE_OPS then bad(string.format("that is %d marker changes, more than one change makes (%d)", count, M.MAX_SLICE_OPS)) end
  local after = {}
  for at in pairs(present) do after[#after + 1] = at end
  table.sort(after)
  if #after > 255 then bad("a sample takes at most 255 slices") end
  return { n = n, label = label, ops = ops, before = markers, after = after, print = M.buffer_print(sample.sample_buffer) }
end

local function slices_title(plan)
  local parts = {}
  local counts = { insert = 0, delete = 0, move = 0 }
  for _, op in ipairs(plan.ops) do counts[op.op] = counts[op.op] + 1 end
  if counts.insert > 0 then parts[#parts + 1] = counts.insert .. " added" end
  if counts.move > 0 then parts[#parts + 1] = counts.move .. " moved" end
  if counts.delete > 0 then parts[#parts + 1] = counts.delete .. " deleted" end
  return string.format("Slices of %s: %s (%d now)", plan.label, table.concat(parts, ", "), #plan.after)
end

local function slices_preview(song, env, input)
  local plan = slices_plan(song, env, input)
  return {
    target = sample_target(env, plan.n), prior = { markers = plan.before },
    fence = { markers = plan.before, print = plan.print },
    display = { title = slices_title(plan), before = { slices = #plan.before }, after = { slices = #plan.after } },
  }
end

local function same_list(a, b)
  if type(a) ~= "table" or type(b) ~= "table" or #a ~= #b then return false end
  for k = 1, #a do if a[k] ~= b[k] then return false end end
  return true
end

-- The settings of the slice starting at frame `at` (its alias sample), for its marker deleted.
local function slice_settings(env, instrument, i, at)
  local markers = instrument:sample(1).slice_markers
  for k, marker in ipairs(markers) do
    if marker == at and instrument.samples[1 + k] then return sample_settings(env, { i, 1 + k }, instrument:sample(1 + k)) end
  end
  return nil
end

local function put_slice_settings(env, instrument, at, kept)
  local markers = instrument:sample(1).slice_markers
  for k, marker in ipairs(markers) do
    if marker == at and instrument.samples[1 + k] then return put_settings(env, instrument:sample(1 + k), kept, false) end
  end
  return 0
end

local function slices_apply(song, env, input, fence, desc)
  local ok, plan = pcall(slices_plan, song, env, input)
  if not ok then base.gone_is_changed(plan) end
  if not same_list(plan.before, fence.markers) or not base.same_table(plan.print, fence.print) then fail("changed", plan.label .. "'s slices changed since the preview") end
  local instrument = song:instrument(plan.n[1])
  local sample = instrument:sample(1)
  local deleted = {}
  for _, op in ipairs(plan.ops) do
    if op.op == "delete" then deleted[#deleted + 1] = { at = op.at, settings = slice_settings(env, instrument, plan.n[1], op.at) } end
  end
  song:describe_undo(desc)
  local wrote, err = pcall(function()
    for _, op in ipairs(plan.ops) do
      if op.op == "delete" then sample:delete_slice_marker(op.at)
      elseif op.op == "move" then sample:move_slice_marker(op.from, op.to)
      else sample:insert_slice_marker(op.at) end
    end
  end)
  local now = sample.slice_markers
  if not wrote or not same_list(now, plan.after) then
    -- (nothing half-done stays: the markers as they were)
    pcall(function() sample.slice_markers = plan.before end)
    fail("failed", "Renoise didn't take those slice markers" .. (wrote and "" or (": " .. tostring(type(err) == "table" and err.message or err))))
  end
  if env.watch_undo then env.watch_undo(sample.slice_markers_observable) end
  return {
    applied = { markers = now }, material = { ops = plan.ops, deleted = deleted, at = places_.place_of(env, "sample", plan.n) },
    target = sample_target(env, plan.n), display = { title = slices_title(plan), before = { slices = #plan.before }, after = { slices = #now } },
    undo = { desc = desc, entry = true }, prior = { markers = plan.before }, place = places_.journal_place("sample", plan.n),
  }
end

local function slices_restore(song, env, restore, desc)
  local material, prior, applied = restore.material, restore.prior, restore.applied
  if type(material) ~= "table" or type(material.at) ~= "table" or type(prior) ~= "table" or type(applied) ~= "table" then bad("material, prior and applied: the markers") end
  local n, why, list = places_.follow(env, material.at)
  if not n then places_.follow_failed(why, list, nil) end
  local instrument = song:instrument(n[1])
  local sample = instrument:sample(n[2])
  local label = string.format("%s in %s", q(places_.sample_label(n[2], sample)), q(places_.instrument_label(n[1], instrument)))
  local now = sample.slice_markers
  if same_list(now, prior.markers) then
    return { restored = { done = true }, display = { title = label .. "'s slices are back already" }, undo = { desc = desc, entry = false }, unwritten = true }
  end
  if not same_list(now, applied.markers) then
    -- (markers have no identity but their frame: a discard can't say which is which once they moved)
    fail("changed", label .. "'s slices changed in Renoise since Kumi set them")
  end
  song:describe_undo(desc)
  local ok, err = pcall(function()
    for k = #material.ops, 1, -1 do
      local op = material.ops[k]
      if op.op == "insert" then sample:delete_slice_marker(op.at)
      elseif op.op == "move" then sample:move_slice_marker(op.to, op.from)
      else sample:insert_slice_marker(op.at) end
    end
  end)
  if not ok or not same_list(sample.slice_markers, prior.markers) then
    fail("failed", "Renoise didn't take the slice markers back" .. (ok and "" or (": " .. tostring(type(err) == "table" and err.message or err))))
  end
  for _, kept in ipairs(material.deleted or {}) do
    if kept.settings then put_slice_settings(env, instrument, kept.at, kept.settings) end
  end
  base.note_restored(env.journal, "sample-slices", places_.journal_place("sample", n), applied, base.KINDS["sample-slices"])
  return { restored = { markers = sample.slice_markers }, display = { title = label .. "'s slices put back" }, undo = { desc = desc, entry = true } }
end

base.register({
  kind = "sample-slices", custom = true, by_place = true,
  preview = slices_preview, apply = slices_apply, restore = slices_restore,
  reread = places_.reread("sample"), read = function(sample) return { markers = sample.slice_markers } end,
  same = function(a, b) return same_list(a.markers, b.markers) end,
})

return M
