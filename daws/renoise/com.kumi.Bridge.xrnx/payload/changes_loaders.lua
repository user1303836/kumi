-- Kumi's changes that load Renoise's library files (M4 n-devices, for the library's find_presets): a device chain (.xrnt)
-- onto a track, a phrase (.xrnz) and a modulation set (.xrno) into an instrument. Kumi's host checks the file (in Renoise's
-- library or the User Library once every link is resolved, of its kind, not of a newer Renoise) and sends its path as
-- `file`; this checks it again (a file Renoise can't load shows a dialog that holds Renoise) and loads it in one step.
--
-- Renoise 3.5.4 (lab, 2026-10-20): load_track_device_chain replaces every device after the mixer of the selected track and
-- resets the mixer's values; load_instrument_phrase replaces the selected phrase (or makes one in an instrument with none);
-- load_instrument_modulation_set replaces the selected set's devices and keeps its name. So Kumi makes a new phrase or set
-- for the file to go into, keeps the track's volume and pan, and keeps the devices a chain replaces to put back.

local import = ...
local json = import("json")
local rows = import("rows")
local base = import("changes_base")
local material_ = import("changes_material")
local devices_ = import("changes_devices")
local places_ = import("instruments")
local phrases_ = import("changes_phrases")
local modfx_ = import("changes_modfx")

local bad, fail, q = base.bad, base.fail, base.q
local M = {}

local FILES = {
  ["device-chain-load"] = { extension = "xrnt", root = "RenoiseDeviceChain", noun = "device chain" },
  ["phrase-load"] = { extension = "xrnz", root = "InstrumentPhrase", noun = "phrase" },
  ["modset-load"] = { extension = "xrno", root = "SampleModulationSet", noun = "modulation set" },
}

-- The file Kumi's host checked, checked again: a path of this kind that is there and holds a document of it. Returns the
-- path and the name the file gives what it holds.
local function file_of(kind, input)
  local spec = FILES[kind]
  if input.path ~= nil then bad(kind .. " with a path is checked by Kumi's host: ask it through the host's change_preview") end
  local file = input.file
  if type(file) ~= "string" or file == "" or not file:lower():match("%." .. spec.extension .. "$") then
    bad(string.format("file: a .%s file in Renoise's library, as Kumi's host checked it", spec.extension))
  end
  local handle, head = io.open(file, "rb"), ""
  if handle then
    head = handle:read(512) or ""
    handle:close()
  else
    fail("failed", string.format("the %s file %s isn't there any more", spec.noun, file))
  end
  if not head:find("<" .. spec.root, 1, true) then bad(string.format("%s isn't a %s file (no <%s>)", file, spec.noun, spec.root)) end
  local name = type(input.fileName) == "string" and input.fileName ~= "" and input.fileName or (file:match("([^/\\]+)%.%w+$") or file)
  return file, name
end

local function one_line(text, most, what)
  if type(text) ~= "string" or #text > most or text:find("%c") then bad(string.format("%s must be one line of at most %d characters", what, most)) end
end

-- device-chain-load {trackRef, file, fileName, replace?}

-- The devices at the top of a chain file (their XML types), for the words.
local function chain_types(file)
  local handle = io.open(file, "rb")
  local text = handle and handle:read("*a") or ""
  if handle then handle:close() end
  local out = {}
  for kind in text:gmatch('\n    <(%w+) type="%1">') do out[#out + 1] = kind end
  return out
end

local function chain_plan(song, env, input)
  local index, track = base.find(song, env, input.trackRef, "device-chain-load")
  local file, name = file_of("device-chain-load", input)
  if input.replace ~= nil and type(input.replace) ~= "boolean" then bad("replace must be true or false") end
  local count = #track.devices - 1
  if count > 0 and input.replace ~= true then
    bad(string.format("%s has %d device%s: Renoise's chain load replaces them all. Give replace: true (Kumi keeps them to put back), or load the chain onto another track",
      q(base.named(index, track)), count, count == 1 and "" or "s"))
  end
  local devices = {}
  for d = 2, #track.devices do devices[#devices + 1] = material_.device_state(track:device(d)) end
  return { index = index, track = track, file = file, name = name, devices = devices, count = count }
end

-- The devices after a track's mixer, by path (what the journal compares after Renoise's own undo or redo).
local function chain_read(track)
  local paths = {}
  for d = 2, #track.devices do paths[#paths + 1] = track:device(d).device_path end
  return { paths = paths }
end

local function paths_of_replaced(replaced)
  local paths = {}
  for k, kept in ipairs(replaced) do paths[k] = (kept.device or {}).path end
  return { paths = paths }
end

local function chain_title(plan, loaded)
  local title = string.format("Loaded the device chain %s onto %s", q(plan.name), q(base.named(plan.index, plan.track)))
  if loaded then title = title .. string.format(" (%d device%s)", loaded, loaded == 1 and "" or "s") end
  if plan.count > 0 then title = title .. string.format(", in place of its %d device%s", plan.count, plan.count == 1 and "" or "s") end
  return title
end

local function chain_preview(song, env, input)
  local plan = chain_plan(song, env, input)
  local names = {}
  for d = 2, #plan.track.devices do names[#names + 1] = devices_.label(plan.track:device(d)) end
  local note = "Kumi keeps the track's volume and pan (Renoise's chain load resets them)"
  if plan.count > 0 then note = note .. ", and the devices it replaces, with their automation, to put back" end
  return {
    target = base.target(env, plan.index), prior = { devices = names },
    fence = { id = base.track_id(plan.track), devices = plan.devices },
    display = { title = chain_title(plan, #chain_types(plan.file)), before = { devices = plan.count }, note = note .. "." },
  }
end

local function chain_apply(song, env, input, fence, desc)
  local ok, plan = pcall(chain_plan, song, env, input)
  if not ok then base.gone_is_changed(plan) end
  local index, track = plan.index, plan.track
  if fence.id ~= base.track_id(track) then fail("changed", base.moved_text(base.named(index, track))) end
  if not base.same_table(plan.devices, fence.devices) then fail("changed", q(base.named(index, track)) .. "'s devices changed since the preview") end
  -- What the undo needs, read before anything is written: the devices the chain replaces, as a delete keeps one.
  local replaced, patterns = {}, {}
  for d = 2, #track.devices do
    local automation = material_.read_automation(song, index, track, d)
    for _, saved in ipairs(automation) do patterns[saved.pattern] = true end
    replaced[#replaced + 1] = {
      device = material_.read_device(track:device(d), song, index, false, env), automation = automation,
      routes = material_.read_routes(song, index, d, env),
    }
  end
  local material = { track = index, name = plan.name, label = base.named(index, track), replaced = replaced, pool = material_.keep_pool(song, env, patterns) }
  local size = #json.encode(material)
  if size > base.MAX_MATERIAL_BYTES then
    fail("failed", string.format("the devices on %s hold %.1f MB, more than Kumi can keep to put them back, so it won't replace them", q(material.label), size / 1048576))
  end
  local mixer = track:device(1)
  local levels = {}
  for k = 1, #mixer.parameters do levels[k] = mixer:parameter(k).value end
  local selected, selected_device = song.selected_track_index, song.selected_track_device_index
  song:describe_undo(desc)
  -- (the producer's undo or redo of it changes the track's devices: watched, for the journal to compare)
  if env.watch_undo then env.watch_undo(track.devices_observable) end
  -- Kumi takes the devices off itself, the last first, each with the mark just after it went: when its undo puts them back,
  -- the places kept of them before follow them there (places.back), as a delete's undo does. The chain then loads onto the
  -- empty track (Renoise would remove them the same way, in this same step).
  for d = #track.devices, 2, -1 do
    track:delete_device_at(d)
    replaced[d - 1].removed = env.device_mark and env.device_mark(index) or nil
  end
  song.selected_track_index = index
  local loaded, result = pcall(function() return renoise.app():load_track_device_chain(plan.file) end)
  -- (another track was selected: it is again, with its device; on this one Renoise selects the chain's last device)
  if selected ~= index then
    pcall(function()
      song.selected_track_index = selected
      song.selected_track_device_index = selected_device
    end)
  end
  if not loaded or result == false then
    -- (the devices it took off go back, so nothing half-done stays)
    local pattern_now = material_.pool_now(song, env, material.pool) or function(p) return p end
    for k, kept in ipairs(replaced) do
      pcall(devices_.put_back, song, env, index, track, math.min(k + 1, #track.devices + 1), kept, pattern_now, function(n) return n end, material_.new_tally())
    end
    fail("failed", string.format("Renoise couldn't load the device chain %s: %s", q(plan.name), loaded and "it said no" or tostring(result)))
  end
  -- (the track's volume, pan and width as they were: a chain is its devices)
  mixer = track:device(1)
  for k, value in ipairs(levels) do
    if k <= #mixer.parameters then pcall(function() mixer:parameter(k).value = value end) end
  end
  local kept, states, names, contents = {}, {}, {}, {}
  for d = 2, #track.devices do
    local device = track:device(d)
    kept[#kept + 1] = devices_.device_kept(env, index, d, device)
    states[#states + 1] = material_.device_state(device)
    names[#names + 1] = devices_.label(device)
    -- (a Doofer's or a Splitter's devices inside, a Notepad's text, a plug-in's state: false for a device with none)
    contents[#contents + 1] = material_.content_fence(device) or false
  end
  material.loaded = kept
  return {
    applied = { count = #kept, devices = names, states = states, contents = contents }, material = material, target = base.target(env, index),
    display = { title = chain_title(plan, #kept) }, undo = { desc = desc, entry = true },
    -- (the journal tells Renoise's own undo and redo of it by the track's devices: those it replaced, or those it loaded)
    prior = paths_of_replaced(replaced), journal_applied = chain_read(track), place = { track = index },
  }
end

-- Whether the devices a chain replaced are on the track as they were (Renoise's own undo of the load).
local function replaced_back(track, replaced)
  if #track.devices - 1 ~= #replaced then return false end
  for k, kept in ipairs(replaced) do
    if track:device(k + 1).device_path ~= (kept.device or {}).path then return false end
  end
  return true
end

-- Whether the devices after the track's mixer are exactly those a chain load left (by kind, settings, name and bypass).
local function loaded_again(track, states)
  if type(states) ~= "table" or #states == 0 or #track.devices - 1 ~= #states then return false end
  for j, was in ipairs(states) do
    local now = material_.device_state(track:device(j + 1))
    if now.path ~= was.path or now.state ~= was.state or now.display ~= was.display or now.active ~= was.active then return false end
  end
  return true
end

local function chain_restore(song, env, restore, desc)
  local material, applied = restore.material, restore.applied
  if type(material) ~= "table" or type(material.track) ~= "number" or type(material.loaded) ~= "table" or type(applied) ~= "table" then
    bad("material and applied: the devices the chain load made and replaced")
  end
  local index = material.track
  local name = q(material.name or "the device chain")
  if index < 1 or index > rows.track_count(song) then fail("gone", string.format("the track Kumi loaded %s onto isn't in the song any more", name)) end
  local track = song:track(index)
  local track_name = q(base.named(index, track))
  local found, missing, why_missing = {}, 0, nil
  for j, kept in ipairs(material.loaded) do
    local d, why = material_.device_now(song, env, index, kept.dev)
    if d then found[#found + 1] = { d = d, j = j } else missing, why_missing = missing + 1, why_missing or why end
  end
  local replaced = material.replaced or {}
  -- (Renoise's own undo and redo of the load take its devices off and make them again: the log sees them go, so none is
  -- found by its place; the track holding exactly what Kumi loaded, device for device, is the chain again)
  if missing > 0 and missing == #material.loaded and loaded_again(track, applied.states) then
    found, missing = {}, 0
    for j = 1, #material.loaded do found[j] = { d = j + 1, j = j } end
  end
  if missing > 0 then
    if missing == #material.loaded and replaced_back(track, replaced) then
      return { restored = { count = 0 }, display = { title = string.format("%s is off %s already", name, track_name) }, undo = { desc = desc, entry = false }, unwritten = true }
    end
    fail(why_missing == "unknown" and "stale" or why_missing == "changed" and "changed" or "gone",
      string.format("%d of the devices Kumi loaded with %s %s on %s, so Kumi won't take the chain off: nothing was written", missing,
        name, why_missing == "unknown" and "moved in a way Kumi couldn't follow" or "aren't there any more", track_name))
  end
  if not restore.discard then
    local moved = {}
    for _, f in ipairs(found) do
      local now, was = material_.device_state(track:device(f.d)), (applied.states or {})[f.j] or {}
      local content = (applied.contents or {})[f.j]
      if now.path ~= was.path or now.state ~= was.state or now.display ~= was.display or now.active ~= was.active
        or (content and material_.content_fence(track:device(f.d)) ~= content) then
        moved[#moved + 1] = devices_.label(track:device(f.d))
      end
    end
    if #moved > 0 then
      fail("changed", string.format("%s on %s changed in Renoise since Kumi loaded the chain", table.concat(moved, ", "), track_name))
    end
  end
  local pattern_now = material_.pool_now(song, env, material.pool)
  if not pattern_now then
    fail("stale", "the song's patterns changed in a way Kumi couldn't follow since it loaded the chain, so it can't tell where the replaced devices' automation goes: nothing was written")
  end
  song:describe_undo(desc)
  if env.watch_undo then env.watch_undo(track.devices_observable) end
  table.sort(found, function(a, b) return a.d > b.d end)
  for _, f in ipairs(found) do track:delete_device_at(f.d) end
  local tally, lost = material_.new_tally(), 0
  local moved = devices_.carried_from(env, restore.layout)
  for k, kept in ipairs(replaced) do
    local at = math.min(k + 1, #track.devices + 1)
    local _, not_back = devices_.put_back(song, env, index, track, at, kept, pattern_now, moved, tally)
    lost = lost + not_back
    -- (the device put back is the one the load took off: the places kept of it before follow it)
    if env.device_back and type(kept.removed) == "string" then env.device_back(index, kept.removed, at) end
  end
  local note = material_.tally_note(tally)
  if lost > 0 then
    local more = string.format("Automation in %d pattern%s isn't put back: deleted or changed in Renoise since.", lost, lost == 1 and "" or "s")
    note = note and (note .. " " .. more) or more
  end
  local title = string.format("Took the device chain %s off %s", name, track_name)
  if #replaced > 0 then title = title .. string.format(" and put back its %d device%s", #replaced, #replaced == 1 and "" or "s") end
  base.note_restored_at(env.journal, "device-chain-load", { track = index })
  return { restored = { count = #found, back = #replaced }, display = { title = title, note = note }, undo = { desc = desc, entry = true } }
end

base.register({
  kind = "device-chain-load", custom = true, by_place = true,
  preview = chain_preview, apply = chain_apply, restore = chain_restore,
  reread = function(song, place)
    if place.track > rows.track_count(song) then return nil end
    return song:track(place.track)
  end,
  read = chain_read,
  same = function(a, b) return base.same_table(a.paths, b.paths) end,
})

-- phrase-load {ref: instrument, file, fileName, index?}: a new phrase at index (the end by default) with the file's lines and
-- settings. Its undo is phrase-add's: the phrase goes while it is as loaded.

local function phrase_plan(song, env, input)
  local n, objects, label = places_.locate(song, env, "instrument", input, "phrase-load")
  local count = #objects.instrument.phrases
  if count >= 126 then bad(q(label) .. " has 126 phrases: Renoise takes no more") end
  local index = input.index == nil and count + 1 or input.index
  if type(index) ~= "number" or index ~= math.floor(index) or index < 1 or index > count + 1 then
    bad(string.format("index is a place from 1 to %d among the phrases", count + 1))
  end
  local file, name = file_of("phrase-load", input)
  return { n = n, label = label, count = count, index = index, file = file, name = name }
end

local function phrase_note(plan)
  return plan.index <= plan.count and "The phrases after it move up one, and Renoise renumbers the 0Z commands in patterns that name them." or nil
end

base.register({
  kind = "phrase-load", custom = true, shape = "made", by_place = true,
  preview = function(song, env, input)
    local plan = phrase_plan(song, env, input)
    return {
      target = places_.target_of(env, "instrument", plan.n), prior = { phrases = plan.count }, fence = { phrases = plan.count, index = plan.index },
      display = {
        title = string.format("Loaded the phrase %s into %s", q(plan.name), q(plan.label)), before = { phrases = plan.count },
        after = { phrases = plan.count + 1 }, note = phrase_note(plan),
      },
    }
  end,
  apply = function(song, env, input, fence, desc)
    local ok, plan = pcall(phrase_plan, song, env, input)
    if not ok then base.gone_is_changed(plan) end
    if plan.count ~= fence.phrases or plan.index ~= fence.index then fail("changed", q(plan.label) .. "'s phrases changed since the preview") end
    local i = plan.n[1]
    local instrument = song:instrument(i)
    local selected_i, selected_p = song.selected_instrument_index, song.selected_phrase_index
    local program_before = phrases_.program_of(instrument)
    song:describe_undo(desc)
    instrument:insert_phrase_at(plan.index)
    -- (Renoise loads a phrase file into the selected phrase: the new one)
    song.selected_instrument_index = i
    song.selected_phrase_index = plan.index
    local loaded, result = pcall(function() return renoise.app():load_instrument_phrase(plan.file) end)
    pcall(function() song.selected_instrument_index = selected_i end)
    pcall(function()
      song.selected_phrase_index = (selected_i == i and selected_p >= plan.index) and selected_p + 1 or selected_p
    end)
    if not loaded or result == false then
      pcall(function() instrument:delete_phrase_at(plan.index) end)
      fail("failed", string.format("Renoise couldn't load the phrase %s: %s", q(plan.name), loaded and "it said no" or tostring(result)))
    end
    local phrase = instrument.phrases[plan.index]
    local m = { i, plan.index }
    -- (the program plays the phrase it played before the new one came)
    local program_place, program_after = phrases_.keep_program(env, instrument, i, program_before, plan.index)
    -- (whether it plays as the program now: Renoise's doing, so Kumi's undo may take it; see changes_phrases.lua's in_use)
    local program = instrument.phrase_playback_mode == renoise.Instrument.PHRASES_PLAY_SELECTIVE and instrument.phrase_program == plan.index
    return {
      applied = { index = plan.index, print = phrases_.phrase_print(phrase), program = program or nil }, created = { ref = places_.ref_of(env, "phrase", m), kind = "phrase" },
      material = { made = places_.place_of(env, "phrase", m), program = program_place, programAfter = program_after, zeroZScan = phrases_.zero_z_scan(song, env) },
      target = places_.target_of(env, "phrase", m),
      display = {
        title = string.format("Loaded the phrase %s into %s", q(places_.phrase_label(plan.index, phrase)), q(plan.label)),
        before = { phrases = plan.count }, after = { phrases = plan.count + 1 },
      },
      undo = { desc = desc, entry = true }, prior = { phrases = plan.count }, place = places_.journal_place("phrase", m), shape = "made",
    }
  end,
  restore = function(song, env, restore, desc) return phrases_.remove_made(song, env, restore, desc, "phrase-load") end,
})

-- modset-load {ref: instrument, file, fileName, index?, name?}: a new modulation set at index (the end by default) with the
-- file's devices, named name or after the file. Its undo is modset-add's: the set goes while it is as loaded and no sample
-- uses it.

local function modset_plan(song, env, input)
  local n, objects, label = places_.locate(song, env, "instrument", input, "modset-load")
  local count = #objects.instrument.sample_modulation_sets
  local index = input.index == nil and count + 1 or input.index
  if type(index) ~= "number" or index ~= math.floor(index) or index < 1 or index > count + 1 then
    bad(string.format("index is a place from 1 to %d among the modulation sets", count + 1))
  end
  if input.name ~= nil then one_line(input.name, 64, "name") end
  local file, name = file_of("modset-load", input)
  return { n = n, label = label, count = count, index = index, file = file, name = input.name ~= nil and input.name ~= "" and input.name or name }
end

base.register({
  kind = "modset-load", custom = true, shape = "made", by_place = true,
  preview = function(song, env, input)
    local plan = modset_plan(song, env, input)
    return {
      target = places_.target_of(env, "instrument", plan.n), prior = { count = plan.count }, fence = { count = plan.count, index = plan.index },
      display = { title = string.format("Loaded the modulation set %s into %s", q(plan.name), q(plan.label)), before = { count = plan.count }, after = { count = plan.count + 1 } },
    }
  end,
  apply = function(song, env, input, fence, desc)
    local ok, plan = pcall(modset_plan, song, env, input)
    if not ok then base.gone_is_changed(plan) end
    if plan.count ~= fence.count or plan.index ~= fence.index then fail("changed", q(plan.label) .. "'s modulation sets changed since the preview") end
    local i = plan.n[1]
    local instrument = song:instrument(i)
    local selected_i, selected_s = song.selected_instrument_index, song.selected_sample_modulation_set_index
    song:describe_undo(desc)
    instrument:insert_sample_modulation_set_at(plan.index)
    -- (Renoise loads a modulation set file into the selected set: the new one; it keeps the set's name)
    song.selected_instrument_index = i
    song.selected_sample_modulation_set_index = plan.index
    local loaded, result = pcall(function() return renoise.app():load_instrument_modulation_set(plan.file) end)
    local set = instrument.sample_modulation_sets[plan.index]
    if loaded and result ~= false then pcall(function() set.name = plan.name end) end
    pcall(function() song.selected_instrument_index = selected_i end)
    pcall(function()
      song.selected_sample_modulation_set_index = (selected_i == i and selected_s >= plan.index) and selected_s + 1 or selected_s
    end)
    if not loaded or result == false then
      pcall(function() instrument:delete_sample_modulation_set_at(plan.index) end)
      fail("failed", string.format("Renoise couldn't load the modulation set %s: %s", q(plan.name), loaded and "it said no" or tostring(result)))
    end
    local m = { i, plan.index }
    return {
      applied = { index = plan.index, print = modfx_.set_print(set) }, created = { ref = places_.ref_of(env, "modset", m), kind = "modset" },
      material = { made = places_.place_of(env, "modset", m) }, target = places_.target_of(env, "modset", m),
      display = {
        title = string.format("Loaded the modulation set %s into %s (%d device%s)", q(plan.name), q(plan.label), #set.devices, #set.devices == 1 and "" or "s"),
        before = { count = plan.count }, after = { count = plan.count + 1 },
      },
      undo = { desc = desc, entry = true }, prior = { count = plan.count }, place = places_.journal_place("modset", m), shape = "made",
    }
  end,
  restore = function(song, env, restore, desc) return modfx_.remove_made_in(song, env, restore, desc, "modset-load", "modset", "modulation_set_index") end,
})

return M
