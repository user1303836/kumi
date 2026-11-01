-- Kumi's instrument made from sample files (M6, m6-library): instrument-kit, one change and one step of Renoise's undo. A drum
-- kit puts each sample on a key of its own (Renoise's Drumkit layout, from `first` up), with the hi-hats in one mute group so an
-- open hat is cut by the closed one; a keys instrument (a multi-sample) spreads each sample's zone between its root note and its
-- neighbours', with velocity layers where samples share a root. Every file is checked before Renoise is asked (a file that
-- isn't what its name says would show a dialog). The undo takes the instrument away (or empties the one it filled) only while
-- it is as Kumi made it and no note plays it.

local import = ...
local refs = import("refs")
local base = import("changes_base")
local places_ = import("instruments")
local instruments_ = import("changes_instruments")

local bad, fail = base.bad, base.fail
local q = base.q
local whole = instruments_.whole
local M = {}

-- The most samples one kit holds (a key each, 0 to 119), and the most audio it loads in its one callback.
M.MAX_SAMPLES = 120
M.MAX_BYTES = instruments_.MAX_LOAD_BYTES

local note_text = base.note_name
M.note_text = note_text

local BLACK = { [1] = true, [3] = true, [6] = true, [8] = true, [10] = true }
local OVERLAPS = { all = 0, cycle = 1, random = 2 }
local SOUND_FILES = instruments_.SAMPLE_FILES

local function file_title(path) return path:match("([^/\\]+)%.[^.]+$") or path end

-- A hi-hat, by its name: what chokes in a drum kit unless the request says otherwise.
local function is_hat(name)
  local lower = " " .. name:lower():gsub("[_%-%.]", " ") .. " "
  return lower:find("hat", 1, true) ~= nil or lower:find("%Whh%W") ~= nil
end
M.is_hat = is_hat

local function range_of(value, low, high, what)
  if type(value) ~= "table" or not whole(low, high)(value[1]) or not whole(low, high)(value[2]) or value[1] > value[2] then
    bad(string.format("%s is [low, high], whole numbers from %d to %d", what, low, high))
  end
  return { value[1], value[2] }
end

-- What the kit will be: its samples (file, name, zone, mute group, volume), checked; nothing read from the song.
local function kit_of(input)
  local list = input.samples
  if type(list) ~= "table" or #list == 0 then bad("samples: the sample files, as [{path, name?, ...}], at least one") end
  if #list > M.MAX_SAMPLES then bad(string.format("a kit holds at most %d samples (one key each)", M.MAX_SAMPLES)) end
  local layout = input.layout or "drumkit"
  if layout ~= "drumkit" and layout ~= "keys" then bad("layout is drumkit (a key per sample) or keys (zones around each sample's root note)") end
  local first = input.first == nil and 48 or input.first
  if not whole(0, 119)(first) then bad("first is a note from 0 to 119 (C-4 is 48)") end
  if input.overlap ~= nil and OVERLAPS[input.overlap] == nil then bad("overlap is all (layers play together), cycle (round robin) or random") end
  local samples, bytes, any_group = {}, 0, false
  for k, given in ipairs(list) do
    if type(given) ~= "table" then bad("each sample is {path, name?, note?, root?, velocity?, muteGroup?, volumeDb?}") end
    local path, extension = instruments_.load_path({ path = given.path }, SOUND_FILES)
    local stat = io.stat(path)
    bytes = bytes + instruments_.load_cost(stat and stat.size or 0, extension)
    local name = given.name
    if name ~= nil and (type(name) ~= "string" or #name > 256 or name:find("[%c]")) then bad("a sample's name is one line of text") end
    local sample = { path = path, name = (name ~= nil and name ~= "") and name or file_title(path) }
    if given.note ~= nil then
      if not whole(0, 119)(given.note) then bad("a sample's note is its key, from 0 to 119 (C-4 is 48)") end
      sample.note = given.note
    end
    if given.root ~= nil then
      if not whole(0, 119)(given.root) then bad("a sample's root is the note it sounds at, from 0 to 119 (C-4 is 48)") end
      sample.root = given.root
    end
    if given.notes ~= nil then sample.notes = range_of(given.notes, 0, 119, "a sample's notes") end
    sample.velocity = given.velocity ~= nil and range_of(given.velocity, 0, 127, "a sample's velocity") or { 0, 127 }
    if given.muteGroup ~= nil then
      if not whole(0, 15)(given.muteGroup) then bad("a sample's muteGroup is from 0 (none) to 15") end
      sample.group = given.muteGroup
      any_group = true
    end
    if given.volumeDb ~= nil then
      if type(given.volumeDb) ~= "number" or given.volumeDb ~= given.volumeDb or given.volumeDb > 12.0412 then bad("a sample's volumeDb is a number up to +12") end
      sample.volume = instruments_.db_to_linear(given.volumeDb)
    end
    if layout == "keys" and sample.root == nil and sample.notes == nil and sample.note == nil then
      bad(string.format("in a keys layout each sample needs its root note (%s has none)", q(sample.name)))
    end
    samples[k] = sample
  end
  if bytes > M.MAX_BYTES then
    fail("failed", string.format("the samples are %.0f MB, more than Kumi loads in one go (%d MB): make the kit in two", bytes / 1048576, M.MAX_BYTES / 1048576))
  end
  -- The zones: a drum kit's keys from `first` up, past the keys samples name for themselves; a keys instrument's split between
  -- neighbouring roots.
  if layout == "drumkit" then
    local taken = {}
    for _, sample in ipairs(samples) do if sample.note then taken[sample.note] = true end end
    local key = first
    for _, sample in ipairs(samples) do
      if not sample.note and not sample.notes then
        while key <= 119 and (taken[key] or (input.whiteKeys == true and BLACK[key % 12])) do key = key + 1 end
        if key > 119 then bad(string.format("%d samples from %s run past the top of the keyboard (119)", #samples, note_text(first))) end
        sample.note, key = key, key + 1
      end
      sample.zone = sample.notes or { sample.note, sample.note }
      sample.base = sample.root or sample.note or sample.zone[1]
    end
  else
    local roots, seen = {}, {}
    for _, sample in ipairs(samples) do
      local root = sample.root or sample.note or sample.notes[1]
      sample.base = root
      if not sample.notes and not seen[root] then seen[root] = true; roots[#roots + 1] = root end
    end
    table.sort(roots)
    local spans = {}
    for i, root in ipairs(roots) do
      local low = i == 1 and 0 or math.floor((roots[i - 1] + root) / 2) + 1
      local high = i == #roots and 119 or math.floor((root + roots[i + 1]) / 2)
      spans[root] = { low, high }
    end
    for _, sample in ipairs(samples) do sample.zone = sample.notes or spans[sample.base] end
  end
  -- Hi-hats choke each other in a drum kit (Renoise's mute groups cut across a track's note columns), unless a sample names its
  -- own group or the request turns it off.
  local choked = {}
  if layout == "drumkit" and not any_group and input.chokeHats ~= false then
    for _, sample in ipairs(samples) do if is_hat(sample.name) then choked[#choked + 1] = sample end end
    if #choked >= 2 then
      for _, sample in ipairs(choked) do sample.group = 1 end
    else
      choked = {}
    end
  end
  local name = input.name
  if name ~= nil and (type(name) ~= "string" or #name > 256 or name:find("[%c]")) then bad("name is one line of text") end
  if name == nil or name == "" then name = layout == "drumkit" and "Drum kit" or samples[1].name end
  return { samples = samples, layout = layout, name = name, choked = #choked, bytes = bytes, overlap = input.overlap }
end

-- What Kumi left, in brief: the instrument's print and each sample's name, zone, mute group, volume and tuning (what its undo
-- would take away with the instrument, so a later edit of any of them is the producer's and refuses it).
local function kit_print(instrument)
  local samples = {}
  for s = 1, #instrument.samples do
    local sample = instrument:sample(s)
    local mapping = sample.sample_mapping
    samples[s] = {
      name = sample.name, group = sample.mute_group, volume = string.format("%.6f", sample.volume), panning = string.format("%.6f", sample.panning),
      transpose = sample.transpose, fine = sample.fine_tune, nna = sample.new_note_action, loop = sample.loop_mode,
      notes = { mapping.note_range[1], mapping.note_range[2] }, velocity = { mapping.velocity_range[1], mapping.velocity_range[2] },
      base = mapping.base_note, layer = mapping.layer,
    }
  end
  return { instrument = places_.instrument_print(instrument), overlap = instrument.sample_mapping_overlap_mode, samples = samples }
end
M.kit_print = kit_print

-- (`index` by Renoise's number, from 0, to a place from 1)
local function index_input(input, count)
  local index = input.index
  if index == nil then return count + 1 end
  if not whole(0, count)(index) then
    bad(string.format("index is an instrument's number from 0 to %d, as Renoise numbers them (00 the first)", count))
  end
  return index + 1
end

local function plan_of(song, env, input, applying)
  local kit = kit_of(input)
  local count = #song.instruments
  if input.ref ~= nil and input.index ~= nil then bad("give index (a new instrument) or ref (an empty one), not both") end
  if input.ref ~= nil then
    local n, objects, label = places_.locate(song, env, "instrument", input, "instrument-kit")
    if not places_.is_empty(objects.instrument) then
      if applying then fail("changed", q(label) .. " isn't empty any more: it changed since the preview") end
      bad(string.format("%s isn't empty: Kumi makes a kit in a new instrument or an empty one (give index, or leave it out)", q(label)))
    end
    kit.into, kit.label = n[1], label
  else
    if count >= 255 then bad("a song has at most 255 instruments") end
    kit.index = index_input(input, count)
  end
  kit.count = count
  return kit
end

-- The keys, as the producer reads them: "C-4 Kick, C#4 Snare".
local function keys_text(kit)
  local parts = {}
  for _, sample in ipairs(kit.samples) do
    local zone = sample.zone
    local where = zone[1] == zone[2] and note_text(zone[1]) or (note_text(zone[1]) .. "\226\128\147" .. note_text(zone[2]))
    if sample.velocity[1] ~= 0 or sample.velocity[2] ~= 127 then where = where .. string.format(" (velocity %d\226\128\147%d)", sample.velocity[1], sample.velocity[2]) end
    parts[#parts + 1] = where .. " " .. sample.name
  end
  return table.concat(parts, ", ")
end

local function title_of(kit, label)
  local what = kit.layout == "drumkit" and "drum kit" or "instrument"
  local text = string.format("Made %s %s from %d %s", what, q(label or kit.name), #kit.samples, #kit.samples == 1 and "sample" or "samples")
  if kit.choked > 0 then text = text .. "; its hi-hats choke each other" end
  return text
end

local function note_of(kit)
  local note = "Keys: " .. keys_text(kit) .. "."
  if kit.choked > 0 then note = note .. " The hi-hats share mute group 1: keep them in one track for the choke." end
  if kit.index and kit.index <= kit.count then note = note .. " The instruments after it move up one; Renoise renumbers the notes that play them." end
  return note
end

local function preview(song, env, input)
  local kit = plan_of(song, env, input)
  local fence = { instruments = kit.count, index = kit.index, into = kit.into }
  if kit.into then fence.print = places_.instrument_print(song:instrument(kit.into)) end
  local target = kit.into and places_.target_of(env, "instrument", { kit.into }) or { ref = refs.make(env.epoch(), "song", 1), kind = "song" }
  return {
    target = target, prior = { instruments = kit.count }, fence = fence,
    display = { title = title_of(kit, kit.label), note = note_of(kit), before = { instruments = kit.count }, after = { instruments = kit.count + (kit.into and 0 or 1) } },
  }
end

-- The samples into the instrument at `i`, each set as the kit says. Raises on the first that Renoise won't take.
local function fill(instrument, kit)
  for k, sample in ipairs(kit.samples) do
    local made = instrument:insert_sample_at(k)
    if not made.sample_buffer:load_from(sample.path) then error({ code = "failed", message = "Renoise wouldn't load " .. sample.path }, 0) end
    made.name = sample.name
    local mapping = made.sample_mapping
    -- (a range is set whole; then the base note: Renoise keeps a base note inside its zone)
    mapping.note_range = { sample.zone[1], sample.zone[2] }
    mapping.velocity_range = { sample.velocity[1], sample.velocity[2] }
    mapping.base_note = sample.base
    if sample.group then made.mute_group = sample.group end
    if sample.volume then made.volume = sample.volume end
  end
  if kit.overlap then instrument.sample_mapping_overlap_mode = OVERLAPS[kit.overlap] end
end

local function apply(song, env, input, fence, desc)
  local ok, kit = pcall(plan_of, song, env, input, true)
  if not ok then base.gone_is_changed(kit) end
  if kit.count ~= fence.instruments or kit.into ~= fence.into or kit.index ~= fence.index then fail("changed", "the instruments changed since the preview") end
  if kit.into and not base.same_table(places_.instrument_print(song:instrument(kit.into)), fence.print) then fail("changed", q(kit.label) .. " changed since the preview") end
  local before = song.selected_instrument_index
  local i = kit.into or kit.index
  local was = kit.into and song:instrument(i).name or nil
  local slot = kit.into and instruments_.keep_slot(song, i) or nil
  song:describe_undo(desc)
  if not kit.into then song:insert_instrument_at(i) end
  local instrument = song:instrument(i)
  local filled, why = pcall(fill, instrument, kit)
  -- (an empty instrument keeps its own name unless the request gives one)
  local keeps = kit.into and (input.name == nil or input.name == "") and instrument.name ~= ""
  if filled and not keeps then filled, why = pcall(function() instrument.name = kit.name end) end
  if not kit.into then
    -- (the producer's selection moves up with the instruments after the new one)
    local wanted = before >= i and before + 1 or before
    pcall(function() if wanted >= 1 and wanted <= #song.instruments and song.selected_instrument_index ~= wanted then song.selected_instrument_index = wanted end end)
  end
  if not filled then
    -- (nothing half-made stays: the new instrument goes, the empty one is emptied again)
    pcall(function() if kit.into then instruments_.empty_slot(song, env, i, slot, was) else song:delete_instrument_at(i) end end)
    local message = type(why) == "table" and why.message or tostring(why or "it went wrong"):gsub("^[^%s:]*%.lua:%d+: ", "")
    fail("failed", message)
  end
  local n = { i }
  local label = places_.instrument_label(i, instrument)
  local done = {
    applied = { index = i, name = instrument.name, print = kit_print(instrument), samples = #kit.samples },
    target = places_.target_of(env, "instrument", n),
    display = { title = title_of(kit, label), note = note_of(kit), before = { instruments = kit.count }, after = { instruments = #song.instruments } },
    undo = { desc = desc, entry = true }, prior = { instruments = kit.count }, place = places_.journal_place("instrument", n),
  }
  if kit.into then
    done.material = { into = places_.place_of(env, "instrument", n), name = was, slot = slot }
  else
    done.material = { made = places_.place_of(env, "instrument", n) }
    done.created = { ref = places_.ref_of(env, "instrument", n), kind = "instrument" }
    done.shape = "made"
  end
  return done
end

-- The undo: the instrument found by its place, taken away (or emptied) while it is as Kumi left it and no note plays it;
-- `discard` takes it whatever the producer did in it (as an add's undo does), never past what can't be followed.
local function restore(song, env, restore, desc)
  local material, applied = restore.material, restore.applied or {}
  if type(material) ~= "table" or (type(material.made) ~= "table" and type(material.into) ~= "table") then bad("material: the instrument the kit is in") end
  local function find(live)
    local n, why, list = places_.follow(env, material.made or material.into)
    if not n then
      if why == "gone" then fail("gone", string.format("the kit Kumi made (%s) isn't in the song any more", q(applied.name or "the instrument"))) end
      places_.follow_failed(why, list or "instrument", applied.name)
    end
    return n[1], places_.instrument_label(n[1], live:instrument(n[1]))
  end
  local function locate(live)
    local i, label = find(live)
    local instrument = live:instrument(i)
    if not restore.discard and type(applied.print) == "table" and not base.same_table(kit_print(instrument), applied.print) then
      fail("changed", string.format("%s changed in Renoise since Kumi made the kit (its samples, keyzones, mute groups, name or more)", q(label)))
    end
    if material.made and #live.instruments == 1 then fail("changed", "it is the song's only instrument now: Renoise needs one") end
    return i, label
  end
  if material.into then
    local i, label = find(song)
    if places_.is_empty(song:instrument(i)) then
      return { restored = { emptied = true }, display = { title = q(label) .. " is empty already" }, undo = { desc = desc, entry = false }, unwritten = true }
    end
  end
  return instruments_.remove_checked(song, env, restore, locate, function(live, i, label)
    live:describe_undo(desc)
    if material.made then
      live:delete_instrument_at(i)
      base.note_restored_at(env.journal, "instrument-kit", places_.journal_place("instrument", { i }))
      return { restored = { deleted = true }, display = { title = string.format("Removed the kit %s (put back)", q(label)) }, undo = { desc = desc, entry = true } }
    end
    local note = instruments_.empty_slot(live, env, i, material.slot, material.name)
    base.note_restored_at(env.journal, "instrument-kit", places_.journal_place("instrument", { i }))
    label = places_.instrument_label(i, live:instrument(i))
    return { restored = { emptied = true }, display = { title = string.format("Emptied %s again (put back)", q(label)), note = note }, undo = { desc = desc, entry = true } }
  end, material.into ~= nil)
end

base.register({
  kind = "instrument-kit", custom = true, by_place = true,
  preview = preview, apply = apply, restore = restore,
})

return M
