-- Kumi's changes to an instrument's modulation and sample FX (M4): modulation sets (made, deleted, set) and their devices (made
-- by path for a target, deleted, moved, set, their parameters), sample FX chains (made, deleted, named and routed) and their
-- devices (made by path, deleted, moved, bypassed, their parameters). A sample's set and chain are sample-set's. Records keep
-- what they touch by place (instruments.lua).

local import = ...
local json = import("json")
local refs = import("refs")
local base = import("changes_base")
local places_ = import("instruments")
local instruments_ = import("changes_instruments")
local material_ = import("changes_material")

local bad, fail = base.bad, base.fail
local q, close = base.q, base.close
local whole, is_boolean, named = instruments_.whole, instruments_.is_boolean, instruments_.named
local M = {}

local TARGETS = named({ volume = 1, panning = 2, pitch = 3, cutoff = 4, resonance = 5, drive = 6 })
local OPERATORS = named({ add = 1, subtract = 2, multiply = 3, divide = 4 })
local INPUTS = { volume = "volume_input", panning = "panning_input", pitch = "pitch_input", cutoff = "cutoff_input", resonance = "resonance_input", drive = "drive_input" }
local INPUT_ORDER = { "volume", "panning", "pitch", "cutoff", "resonance", "drive" }

local function index_in(value, count, what, first)
  first = first or 1
  if value == nil then return count + 1 end
  if type(value) ~= "number" or value ~= math.floor(value) or value < first or value > count + 1 then
    bad(string.format("%s is a place from %d to %d", what, first, count + 1))
  end
  return value
end

local function one_line(value, most, what)
  if type(value) ~= "string" or #value > most or value:find("[%c]") then bad(what .. " must be one line of text") end
  return value
end

-- The samples of an instrument that use a set (`field` modulation_set_index) or chain (device_chain_index) number `m`, as places.
local function users_of(env, instrument, i, field, m)
  local out = {}
  for s = 1, #instrument.samples do
    if instrument:sample(s)[field] == m then out[#out + 1] = places_.place_of(env, "sample", { i, s }) end
  end
  return out
end

-- Assigns the kept samples (where they are now) to set or chain `m`, where nothing assigned them since (Renoise put 0 when it
-- went). Answers how many could not be.
local function reassign(env, song, field, users, m)
  local lost = 0
  for _, kept in ipairs(users or {}) do
    local n = places_.follow(env, kept)
    local sample = n and song:instrument(n[1]).samples[n[2]] and song:instrument(n[1]):sample(n[2]) or nil
    if sample and sample[field] == 0 and pcall(function() sample[field] = m end) then
      -- (assigned again)
    else
      lost = lost + 1
    end
  end
  return lost
end

-- Modulation devices: their state, as a delete keeps it and a restore writes it.

-- (and the plain fields of a type that aren't parameters: an LFO's waveform and a Velocity Tracking's mode (`mode`), a
-- Fader's `scaling`; each read where the device has it)
local POINT_FIELDS = { "length", "play_mode", "loop_mode", "loop_start", "loop_end", "sustain_enabled", "sustain_position", "fade_amount", "play_step",
  "mode", "scaling" }

local function points_of(device)
  local ok, points = pcall(function() return device.points end)
  if not ok or type(points) ~= "table" then return nil end
  local out = {}
  for k, point in ipairs(points) do out[k] = { time = point.time, value = point.value, scaling = point.scaling } end
  return out
end

local function device_fields(device)
  local out = {}
  for _, field in ipairs(POINT_FIELDS) do
    local ok, value = pcall(function() return device[field] end)
    if ok and value ~= nil then out[field] = value end
  end
  return out
end

local function moddevice_state(device)
  local params = {}
  for k = 1, #device.parameters do params[k] = device:parameter(k).value end
  return {
    path = M.moddevice_path(device), name = device.name, target = device.target, display = device.display_name, active = device.is_active,
    maximized = device.is_maximized, operator = device.operator, bipolar = device.bipolar, synced = device.tempo_synced, params = params,
    points = points_of(device), fields = device_fields(device),
  }
end
M.moddevice_state = moddevice_state

-- A modulation device's path in available_devices ("Modulation/AHDSR"): its name is "<Target> <Type>" ("Volume AHDSR": lab).
local TARGET_WORDS = { "Volume ", "Panning ", "Pitch ", "Cutoff ", "Resonance ", "Drive " }
function M.moddevice_type(name)
  for _, word in ipairs(TARGET_WORDS) do
    if name:sub(1, #word) == word then return name:sub(#word + 1) end
  end
  return name
end

function M.moddevice_path(device, set)
  local kind = M.moddevice_type(device.name)
  if set then
    for _, path in ipairs(set.available_devices) do
      if (path:match("([^/]+)$") or path) == kind then return path end
    end
  end
  return "Modulation/" .. kind
end

local function write_moddevice(device, state)
  pcall(function() if state.display and state.display ~= "" then device.display_name = state.display end end)
  pcall(function() device.is_active = state.active ~= false end)
  pcall(function() if state.maximized ~= nil then device.is_maximized = state.maximized end end)
  pcall(function() device.operator = state.operator end)
  pcall(function() device.bipolar = state.bipolar end)
  pcall(function() if device.tempo_sync_switching_allowed then device.tempo_synced = state.synced end end)
  for k, value in ipairs(state.params or {}) do
    pcall(function() device:parameter(k).value = value end)
  end
  -- (an envelope's length before its points and loop, which must fit it)
  for _, field in ipairs(POINT_FIELDS) do
    if (state.fields or {})[field] ~= nil then pcall(function() device[field] = state.fields[field] end) end
  end
  if state.points then pcall(function() device.points = state.points end) end
end
M.write_moddevice = write_moddevice

local function state_hash(state)
  local text = json.encode(state, { sort_keys = true }) or ""
  return base.hash(text)
end

-- modset-add {ref: instrument, index?, name?}

local function set_state(set)
  local inputs = {}
  for _, key in ipairs(INPUT_ORDER) do inputs[key] = set[INPUTS[key]].value end
  local devices = {}
  for d = 1, #set.devices do devices[d] = moddevice_state(set:device(d)) end
  return { name = set.name, pitchRange = set.pitch_range, filterVersion = set.filter_version, filterType = set.filter_type, inputs = inputs, devices = devices }
end
M.set_state = set_state

local function set_print(set) return { name = set.name, devices = #set.devices, hash = state_hash(set_state(set)) } end

-- (what tells a set put back: its name and its devices' kinds and targets; Renoise may not take every value back alike)
local function set_back_print(set)
  local devices = {}
  for d = 1, #set.devices do devices[d] = set:device(d).name .. ":" .. set:device(d).target end
  return { name = set.name, devices = devices }
end

local function remove_made_in(song, env, restore, desc, kind, of, field)
  local material = restore.material
  if type(material) ~= "table" or type(material.made) ~= "table" then bad("material: what the change made") end
  local n, why, which = places_.follow(env, material.made)
  if not n then
    if why == "gone" then fail("gone", "what Kumi made isn't there any more") end
    places_.follow_failed(why, which, nil)
  end
  local objects = places_.resolve(song, of, n)
  local label = places_.label_of(of, n, objects)
  local instrument = objects.instrument
  local object = objects[of]
  if not restore.discard then
    local now = (of == "modset" and set_print(object)) or (of == "chain" and M.chain_print(object)) or { hash = state_hash(M.any_device_state(of, object)) }
    if not base.same_table(now, (restore.applied or {}).print) then fail("changed", label .. " changed in Renoise since Kumi made it") end
    if field and #users_of(env, instrument, n[1], field, n[2]) > 0 then
      fail("changed", "samples use " .. label .. " since Kumi made it: Kumi left it")
    end
  end
  song:describe_undo(desc)
  if of == "modset" then instrument:delete_sample_modulation_set_at(n[2])
  elseif of == "chain" then instrument:delete_sample_device_chain_at(n[2])
  elseif of == "moddevice" then objects.modset:delete_device_at(n[3])
  else objects.chain:delete_device_at(n[3]) end
  base.note_restored_at(env.journal, kind, places_.journal_place(of, n))
  return { restored = { deleted = true }, display = { title = "Removed " .. label .. " (put back)" }, undo = { desc = desc, entry = true } }
end

-- (modset-load in changes_loaders.lua makes a set too)
M.set_print, M.remove_made_in = set_print, remove_made_in

local function add_in_instrument(spec)
  return function(song, env, input, fence, desc, preview_only)
    local n, objects, label = places_.locate(song, env, "instrument", input, spec.kind)
    local instrument = objects.instrument
    local count = spec.count(instrument)
    local index = index_in(input.index, count, "index")
    if input.name ~= nil then one_line(input.name, 64, "name") end
    if spec.check then spec.check(song, instrument, input) end
    local what = spec.what
    local title = string.format("Added %s %s to %s", what, input.name and q(input.name) or tostring(index), q(label))
    if preview_only then
      return {
        target = places_.target_of(env, "instrument", n), prior = { count = count },
        fence = { count = count, index = index }, display = { title = title, before = { count = count }, after = { count = count + 1 } },
      }
    end
    if count ~= fence.count or index ~= fence.index then fail("changed", q(label) .. " changed since the preview") end
    song:describe_undo(desc)
    local made = spec.insert(instrument, index)
    if input.name and input.name ~= "" then made.name = input.name end
    if spec.after then
      local set_up, err = pcall(spec.after, made, input, song)
      if not set_up then
        pcall(spec.delete, instrument, index)
        error(err, 0)
      end
    end
    local m = { n[1], index }
    return {
      applied = { index = index, print = spec.print(made) }, created = { ref = places_.ref_of(env, spec.of, m), kind = spec.of },
      material = { made = places_.place_of(env, spec.of, m) }, target = places_.target_of(env, spec.of, m),
      display = { title = title, before = { count = count }, after = { count = count + 1 } }, undo = { desc = desc, entry = true },
      prior = { count = count }, place = places_.journal_place(spec.of, m), shape = "made",
    }
  end
end

local add_set = add_in_instrument({
  kind = "modset-add", of = "modset", what = "modulation set",
  count = function(i) return #i.sample_modulation_sets end,
  insert = function(i, index) return i:insert_sample_modulation_set_at(index) end,
  delete = function(i, index) i:delete_sample_modulation_set_at(index) end,
  print = set_print,
})

base.register({
  kind = "modset-add", custom = true, shape = "made", by_place = true,
  preview = function(song, env, input) return add_set(song, env, input, nil, nil, true) end,
  apply = function(song, env, input, fence, desc) return add_set(song, env, input, fence, desc) end,
  restore = function(song, env, restore, desc) return remove_made_in(song, env, restore, desc, "modset-add", "modset", "modulation_set_index") end,
})

-- modset-delete {ref: modset}: Kumi keeps its settings, every device and the samples that used it (Renoise gives them none).

local function delete_in_instrument(spec)
  local function preview(song, env, input)
    local n, objects, label = places_.locate(song, env, spec.of, input, spec.kind)
    local display = { title = "Deleted " .. label, note = spec.note }
    return { target = places_.target_of(env, spec.of, n), prior = { deleted = false }, fence = spec.print(objects[spec.of]), display = display }
  end
  local function apply(song, env, input, fence, desc)
    local ok, n, objects, label = pcall(places_.locate, song, env, spec.of, input, spec.kind)
    if not ok then base.gone_is_changed(n) end
    local object, instrument = objects[spec.of], objects.instrument
    if not base.same_table(spec.print(object), fence) then fail("changed", label .. " changed since the preview") end
    local material = { state = spec.state(object), index = n[2], users = users_of(env, instrument, n[1], spec.field, n[2]) }
    if #json.encode(material) > base.MAX_MATERIAL_BYTES then fail("failed", label .. " holds more than Kumi can keep to put it back, so it won't delete it") end
    -- (its devices' log, for the one Kumi puts back to take on: the places of its devices kept before then follow)
    material.logs = env.logs_of and env.logs_of(spec.of, n) or nil
    song:describe_undo(desc)
    spec.delete(instrument, n[2])
    material.instrument = places_.place_of(env, "instrument", { n[1] })
    material.after = env.list_mark and env.list_mark(spec.list, n[1]) or nil
    return {
      applied = { deleted = true }, material = material, target = places_.target_of(env, "instrument", { n[1] }),
      display = { title = "Deleted " .. label }, undo = { desc = desc, entry = true }, prior = { deleted = false },
      place = places_.journal_place(spec.of, n), shape = "deleted",
    }
  end
  local function restore(song, env, restore_, desc)
    local material = restore_.material
    if type(material) ~= "table" or type(material.state) ~= "table" or type(material.instrument) ~= "table" then bad("material: what the delete kept") end
    local label = q(material.state.name ~= "" and material.state.name or spec.what)
    local n, why, which = places_.follow(env, material.instrument)
    if not n then places_.follow_failed(why, which, material.state.name) end
    local instrument = song:instrument(n[1])
    local at = env.list_after and env.list_after(spec.list, n[1], nil, material.index, material.after) or nil
    if not at then
      fail("stale", string.format("the %ss of %s changed in a way Kumi couldn't follow since it deleted %s: nothing was put back", spec.what, q(places_.instrument_label(n[1], instrument)), label))
    end
    at = math.max(1, math.min(at or 1, spec.count(instrument) + 1))
    if at <= spec.count(instrument) and base.same_table(spec.back_print(spec.get(instrument, at)), spec.print_of_state(material.state)) then
      base.note_restored_at(env.journal, spec.kind, places_.journal_place(spec.of, { n[1], at }))
      return { restored = { ref = places_.ref_of(env, spec.of, { n[1], at }) }, display = { title = label .. " is back already" }, undo = { desc = desc, entry = false }, unwritten = true }
    end
    song:describe_undo(desc)
    local made = spec.insert(instrument, at)
    local notes = spec.write(made, material.state, song) or {}
    -- (the one Kumi deleted, at its place again: what was kept by its place before the delete follows it, and its devices'
    -- places too when every device was made again, in its order)
    if env.back then env.back(spec.list, n[1], nil, material.after, at) end
    if env.adopt and material.logs and #made.devices == spec.devices_kept(material.state) then env.adopt(spec.of, { n[1], at }, material.logs) end
    local lost = reassign(env, song, spec.field, material.users, at)
    if lost > 0 then notes[#notes + 1] = string.format("%d %s that used it %s it again: changed in Renoise since.", lost, lost == 1 and "sample" or "samples", lost == 1 and "doesn't use" or "don't use") end
    base.note_restored_at(env.journal, spec.kind, places_.journal_place(spec.of, { n[1], at }))
    return {
      restored = { ref = places_.ref_of(env, spec.of, { n[1], at }) },
      display = { title = label .. " put back", note = #notes > 0 and table.concat(notes, " ") or nil }, undo = { desc = desc, entry = true },
    }
  end
  base.register({ kind = spec.kind, custom = true, shape = "deleted", by_place = true, preview = preview, apply = apply, restore = restore })
end

local function write_set(set, state)
  local notes = {}
  pcall(function() set.name = state.name end)
  pcall(function() set.pitch_range = state.pitchRange end)
  local filter_ok = pcall(function() set.filter_type = state.filterType end)
  if not filter_ok or set.filter_type ~= state.filterType then
    notes[#notes + 1] = string.format("Its filter (%s, filters of version %d) isn't one Renoise makes new sets with: look at its filter.", tostring(state.filterType), state.filterVersion or 0)
  end
  for _, key in ipairs(INPUT_ORDER) do
    if state.inputs and state.inputs[key] ~= nil then pcall(function() set[INPUTS[key]].value = state.inputs[key] end) end
  end
  for d, device in ipairs(state.devices or {}) do
    local ok, made = pcall(function() return set:insert_device_at(device.path, device.target, d) end)
    if ok and made then write_moddevice(made, device) else notes[#notes + 1] = "A device of it (" .. tostring(device.name) .. ") couldn't be made again." end
  end
  return notes
end

local function print_of_set_state(state)
  local devices = {}
  for d, device in ipairs(state.devices or {}) do devices[d] = tostring(device.name) .. ":" .. tostring(device.target) end
  return { name = state.name, devices = devices }
end

delete_in_instrument({
  kind = "modset-delete", of = "modset", list = "modsets", what = "modulation set", field = "modulation_set_index",
  note = "Kumi keeps its settings and devices, and which samples use it, to put back.",
  print = set_print, back_print = set_back_print, state = set_state, print_of_state = print_of_set_state, write = write_set,
  count = function(i) return #i.sample_modulation_sets end, get = function(i, at) return i:sample_modulation_set(at) end,
  insert = function(i, at) return i:insert_sample_modulation_set_at(at) end, delete = function(i, at) i:delete_sample_modulation_set_at(at) end,
  devices_kept = function(state) return #(state.devices or {}) end,
})

-- modset-set {ref: modset, name?, pitchRange?, filterType?, volume?, panning?, pitch?, cutoff?, resonance?, drive?}

local SET_FIELDS = {
  name = { get = function(s) return s.name end, set = function(s, v) s.name = v end, field = "name",
    check = function(v) return type(v) == "string" and #v <= 64 and not v:find("[%c]") end, says = "name must be one line of text", text = function(v) return "named " .. q(v) end },
  pitchRange = { get = function(s) return s.pitch_range end, set = function(s, v) s.pitch_range = v end, field = "pitch_range",
    check = whole(1, 96), says = "pitchRange is from 1 to 96 semitones", text = function(v) return "pitch range " .. v end },
  filterType = { get = function(s) return s.filter_type end, set = function(s, v) s.filter_type = v end, field = "filter_type",
    check = function(v, s)
      for _, name in ipairs(s.available_filter_types) do if name == v then return true end end
      return false
    end, says = "filterType is one of the set's filter types (as Renoise names them)", text = function(v) return "filter " .. v end },
}
local SET_ORDER = { "name", "pitchRange", "filterType" }
-- (each input in its own range, as Renoise has it: volume 0..1, panning and pitch -1..1, cutoff, resonance and drive 0..127)
for _, key in ipairs(INPUT_ORDER) do
  SET_FIELDS[key] = {
    get = function(s) return s[INPUTS[key]].value end, set = function(s, v) s[INPUTS[key]].value = v end,
    obs = function(s) return s[INPUTS[key]].value_observable end,
    check = function(v, s)
      local p = s[INPUTS[key]]
      return type(v) == "number" and v == v and v >= p.value_min and v <= p.value_max
    end,
    says = key .. " is the set's " .. key .. " input, in its own range (volume 0 to 1, panning and pitch -1 to 1, cutoff, resonance and drive 0 to 127)",
    text = function(v) return key .. " " .. string.format("%g", math.floor(v * 1000 + 0.5) / 1000) end,
  }
  SET_ORDER[#SET_ORDER + 1] = key
end

instruments_.instrument_spec(instruments_.settings_kind({
  kind = "modset-set", of = "modset", noun = "settings", undoable = true, order = SET_ORDER, fields = SET_FIELDS,
}))

-- moddevice-add {ref: modset, path, target, index?}

local function resolve_modpath(set, wanted)
  if type(wanted) ~= "string" or wanted == "" then bad("path: a modulation device's path (as the set lists it) or its name (AHDSR, LFO, ...)") end
  local lower = wanted:lower()
  for _, path in ipairs(set.available_devices) do
    if path == wanted or path:lower() == lower or (path:match("([^/]+)$") or path):lower() == lower then return path end
  end
  local names = {}
  for k, path in ipairs(set.available_devices) do names[k] = path:match("([^/]+)$") or path end
  bad(string.format("a modulation set has no device %q (it has %s)", wanted, table.concat(names, ", ")))
end

local function moddevice_print(device) return { name = device.name, target = device.target, hash = state_hash(moddevice_state(device)) } end

function M.any_device_state(of, device)
  if of == "moddevice" then return moddevice_state(device) end
  return material_.read_device(device)
end

local function moddevice_add(song, env, input, fence, desc, preview_only)
  local n, objects, label = places_.locate(song, env, "modset", input, "moddevice-add")
  local set = objects.modset
  local path = resolve_modpath(set, input.path)
  local target = TARGETS.parse(input.target)
  if not target then bad("target is volume, panning, pitch, cutoff, resonance or drive") end
  local count = #set.devices
  local index = index_in(input.index, count, "index")
  local name = path:match("([^/]+)$") or path
  local title = string.format("Added %s on %s to %s", q(name), input.target, label)
  if preview_only then
    return {
      target = places_.target_of(env, "modset", n), prior = { devices = count }, fence = { devices = count, index = index },
      display = { title = title, before = { devices = count }, after = { devices = count + 1 } },
    }
  end
  if count ~= fence.devices or index ~= fence.index then fail("changed", label .. "'s devices changed since the preview") end
  song:describe_undo(desc)
  local made, device = pcall(function() return set:insert_device_at(path, target, index) end)
  if not made or not device then fail("failed", "Renoise wouldn't make " .. path .. (made and "" or (": " .. tostring(device)))) end
  local m = { n[1], n[2], index }
  return {
    applied = { index = index, path = path, print = { hash = state_hash(moddevice_state(device)) } },
    created = { ref = places_.ref_of(env, "moddevice", m), kind = "moddevice" }, material = { made = places_.place_of(env, "moddevice", m) },
    target = places_.target_of(env, "moddevice", m), display = { title = title, before = { devices = count }, after = { devices = count + 1 } },
    undo = { desc = desc, entry = true }, prior = { devices = count }, place = places_.journal_place("moddevice", m), shape = "made",
  }
end

base.register({
  kind = "moddevice-add", custom = true, shape = "made", by_place = true,
  preview = function(song, env, input) return moddevice_add(song, env, input, nil, nil, true) end,
  apply = function(song, env, input, fence, desc) return moddevice_add(song, env, input, fence, desc) end,
  restore = function(song, env, restore, desc) return remove_made_in(song, env, restore, desc, "moddevice-add", "moddevice", nil) end,
})

-- moddevice-delete {ref: moddevice}: Kumi keeps its state (parameters, envelope points) to make it again.

local function device_in_list(spec)
  local function preview(song, env, input)
    local n, objects, label = places_.locate(song, env, spec.of, input, spec.kind)
    local device = objects[spec.of]
    if spec.check then spec.check(n, device, label) end
    return {
      target = places_.target_of(env, spec.of, n), prior = { deleted = false }, fence = spec.print(device, objects[spec.owner], n[3]),
      display = { title = "Deleted " .. label, note = spec.note(objects.instrument) },
    }
  end
  local function apply(song, env, input, fence, desc)
    local ok, n, objects, label = pcall(places_.locate, song, env, spec.of, input, spec.kind)
    if not ok then base.gone_is_changed(n) end
    local device, owner = objects[spec.of], objects[spec.owner]
    if spec.check then spec.check(n, device, label) end
    if not base.same_table(spec.print(device, owner, n[3]), fence) then fail("changed", label .. " changed since the preview") end
    local material = { state = spec.state(device, owner, n[3]), index = n[3] }
    if #json.encode(material) > base.MAX_MATERIAL_BYTES then fail("failed", label .. " holds more than Kumi can keep to put it back, so it won't delete it") end
    song:describe_undo(desc)
    owner:delete_device_at(n[3])
    material.owner = places_.place_of(env, spec.owner, { n[1], n[2] })
    material.after = env.list_mark and env.list_mark(spec.list, n[1], n[2]) or nil
    return {
      applied = { deleted = true }, material = material, target = places_.target_of(env, spec.owner, { n[1], n[2] }),
      display = { title = "Deleted " .. label }, undo = { desc = desc, entry = true }, prior = { deleted = false },
      place = places_.journal_place(spec.of, n), shape = "deleted",
    }
  end
  local function restore(song, env, restore_, desc)
    local material = restore_.material
    if type(material) ~= "table" or type(material.state) ~= "table" or type(material.owner) ~= "table" then bad("material: what the delete kept") end
    local state = material.state
    local label = q(state.display ~= nil and state.display ~= "" and state.display or state.name or "the device")
    local n, why, which = places_.follow(env, material.owner)
    if not n then places_.follow_failed(why, which, nil) end
    local objects = places_.resolve(song, spec.owner, n)
    local owner = objects[spec.owner]
    -- (`first` here is the list's first object, which never moves: a chain's mixer at 1, before the first place a device goes)
    local at = env.list_after and env.list_after(spec.list, n[1], n[2], material.index, material.after, spec.first and spec.first - 1) or nil
    if not at then fail("stale", string.format("the devices changed in a way Kumi couldn't follow since it deleted %s: nothing was put back", label)) end
    at = math.max(spec.first or 1, math.min(at or 1, #owner.devices + 1))
    if at <= #owner.devices and base.same_table(spec.back_print(owner:device(at)), spec.print_of_state(state)) then
      base.note_restored_at(env.journal, spec.kind, places_.journal_place(spec.of, { n[1], n[2], at }))
      return { restored = { ref = places_.ref_of(env, spec.of, { n[1], n[2], at }) }, display = { title = label .. " is back already" }, undo = { desc = desc, entry = false }, unwritten = true }
    end
    song:describe_undo(desc)
    spec.make(owner, at, state)
    -- (the device Kumi deleted, at its place again: what was kept by its place before the delete follows it)
    if env.back then env.back(spec.list, n[1], n[2], material.after, at) end
    base.note_restored_at(env.journal, spec.kind, places_.journal_place(spec.of, { n[1], n[2], at }))
    local note = spec.restore_note
    if type(note) == "function" then note = note(state) end
    if spec.after_make then note = spec.after_make(owner, at, state) end
    return {
      restored = { ref = places_.ref_of(env, spec.of, { n[1], n[2], at }) },
      display = { title = label .. " put back", note = note },
      undo = { desc = desc, entry = true },
    }
  end
  base.register({ kind = spec.kind, custom = true, shape = "deleted", by_place = true, preview = preview, apply = apply, restore = restore })
end

-- Whether any of an instrument's macros is mapped: Renoise drops a mapping to a device it deletes, and Kumi can't make one.
local function has_mappings(instrument)
  for k = 1, 8 do
    local ok, count = pcall(function() return #instrument:macro(k).mappings end)
    if ok and count > 0 then return true end
  end
  return false
end
M.has_mappings = has_mappings

local MAPPING_NOTE = "A macro mapping to it would go with it, and Kumi can't make mappings: only Renoise's undo brings one back."

device_in_list({
  kind = "moddevice-delete", of = "moddevice", owner = "modset", list = "moddevices",
  note = function(instrument) return "Kumi keeps its settings and points to put back." .. (has_mappings(instrument) and (" " .. MAPPING_NOTE) or "") end,
  print = moddevice_print, back_print = function(device) return { name = device.name, target = device.target, display = device.display_name } end,
  state = moddevice_state, print_of_state = function(state) return { name = state.name, target = state.target, display = state.display } end,
  make = function(set, at, state)
    local made = set:insert_device_at(state.path, state.target, at)
    write_moddevice(made, state)
    return made
  end,
})

-- moddevice-move {ref: moddevice, index}: Renoise has no swap for these: a copy is made at the new place and the old one goes,
-- in one step (refused when the instrument's macros have mappings, which would go with the old device).

local function moddevice_move_plan(song, env, input)
  local n, objects, label = places_.locate(song, env, "moddevice", input, "moddevice-move")
  local set = objects.modset
  local to = input.index
  if not whole(1, #set.devices)(to) then bad(string.format("index is a place from 1 to %d", #set.devices)) end
  if has_mappings(objects.instrument) and to ~= n[3] then bad(label .. ": the instrument's macros have mappings, which moving it this way would lose: move it in Renoise") end
  return { n = n, label = label, to = to, print = moddevice_print(objects.moddevice) }
end

-- A set's devices in order, each as what it is and holds: what the journal compares to tell Renoise's undo and redo of a move.
local function set_order(set)
  local out = {}
  for k = 1, #set.devices do out[k] = moddevice_print(set:device(k)) end
  return out
end

local function move_moddevice(set, from, to)
  local old = set:device(from)
  local made = set:insert_device_at(M.moddevice_path(old, set), old.target, to > from and to + 1 or to)
  made:copy_from(set:device(to > from and from or from + 1))
  set:delete_device_at(to > from and from or from + 1)
end

base.register({
  kind = "moddevice-move", custom = true, by_place = true,
  preview = function(song, env, input)
    local plan = moddevice_move_plan(song, env, input)
    local display = { title = string.format("Moved %s to place %d", plan.label, plan.to), before = { index = plan.n[3] }, after = { index = plan.to } }
    if plan.to == plan.n[3] then display.unchanged = true end
    return { target = places_.target_of(env, "moddevice", plan.n), prior = { index = plan.n[3] }, fence = { index = plan.n[3], print = plan.print }, display = display }
  end,
  apply = function(song, env, input, fence, desc)
    local ok, plan = pcall(moddevice_move_plan, song, env, input)
    if not ok then base.gone_is_changed(plan) end
    if plan.n[3] ~= fence.index or not base.same_table(plan.print, fence.print) then fail("changed", plan.label .. " changed since the preview") end
    local display = { title = string.format("Moved %s to place %d", plan.label, plan.to), before = { index = plan.n[3] }, after = { index = plan.to } }
    local m = { plan.n[1], plan.n[2], plan.to }
    if plan.to == plan.n[3] then
      display.unchanged = true
      return { applied = { index = plan.to, print = plan.print }, display = display, undo = { desc = desc, entry = false }, prior = { index = plan.n[3] }, target = places_.target_of(env, "moddevice", m) }
    end
    local set = song:instrument(plan.n[1]):sample_modulation_set(plan.n[2])
    local order = set_order(set)
    song:describe_undo(desc)
    move_moddevice(set, plan.n[3], plan.to)
    return {
      applied = { index = plan.to, print = moddevice_print(set:device(plan.to)) }, material = { at = places_.place_of(env, "moddevice", m) },
      target = places_.target_of(env, "moddevice", m), display = display, undo = { desc = desc, entry = true },
      -- (the journal follows the set's order of devices: Renoise's undo and redo of the move put back one or the other)
      prior = { index = plan.n[3], order = order }, journal_applied = { order = set_order(set) },
      place = places_.journal_place("modset", { plan.n[1], plan.n[2] }),
    }
  end,
  restore = function(song, env, restore, desc)
    local material, prior, applied = restore.material, restore.prior, restore.applied
    if type(material) ~= "table" or type(material.at) ~= "table" then bad("material: where the device moved") end
    local n, why, which = places_.follow(env, material.at)
    -- (Renoise's undo and redo of the move made its copy again: the device the move left is at its place in the same set)
    if not n and restore.redone and type(material.at.at) == "table" then
      local set_n = places_.follow(env, { of = "modset", at = { material.at.at[1], material.at.at[2] } })
      if set_n then n, why, which = { set_n[1], set_n[2], applied.index }, nil, nil end
    end
    if not n then places_.follow_failed(why, which, nil) end
    local objects = places_.resolve(song, "moddevice", n)
    local label = places_.label_of("moddevice", n, objects)
    if n[3] == prior.index then return { restored = { index = n[3] }, display = { title = label .. " is at its place already" }, undo = { desc = desc, entry = false }, unwritten = true } end
    if not base.same_table(moddevice_print(objects.moddevice), applied.print) or n[3] ~= applied.index then
      fail("changed", label .. " changed or moved in Renoise since Kumi moved it")
    end
    if prior.index > #objects.modset.devices then fail("changed", "place " .. prior.index .. " isn't in the set any more") end
    if has_mappings(objects.instrument) then fail("changed", "the instrument's macros have mappings now, which moving it back this way would lose") end
    song:describe_undo(desc)
    move_moddevice(objects.modset, n[3], prior.index)
    base.note_restored_at(env.journal, "moddevice-move", places_.journal_place("modset", { n[1], n[2] }))
    return { restored = { index = prior.index }, display = { title = string.format("Moved %s back to place %d", label, prior.index) }, undo = { desc = desc, entry = true } }
  end,
  reread = places_.reread("modset"),
  read = function(set) return { order = set_order(set) } end,
  same = function(a, b) return base.same_table(a.order, b.order) end,
})

-- moddevice-set {ref: moddevice, operator?, bipolar?, tempoSynced?, active?, name?, points?, length?, playMode?, loopMode?,
-- loopStart?, loopEnd?, sustain?, sustainPosition?, fade?, playStep?}

local PLAY_MODES = named({ points = 1, lines = 2, curves = 3 })
local ENV_LOOPS = named({ off = 1, forward = 2, reverse = 3, pingpong = 4 })
-- (Renoise's own constants where it has them; 3.5's numbers otherwise)
local function constant(class, name, fallback)
  local ok, value = pcall(function() return renoise[class][name] end)
  return ok and type(value) == "number" and value or fallback
end
local LFO, FADER, VELOCITY = "SampleLfoModulationDevice", "SampleFaderModulationDevice", "SampleVelocityTrackingModulationDevice"
local LFO_WAVES = named({ sine = constant(LFO, "MODE_SIN", 1), saw = constant(LFO, "MODE_SAW", 2), pulse = constant(LFO, "MODE_PULSE", 3),
  random = constant(LFO, "MODE_RANDOM", 4) })
local VELOCITY_MODES = named({ clamp = constant(VELOCITY, "MODE_CLAMP", 1), scale = constant(VELOCITY, "MODE_SCALE", 2) })
local FADER_SCALINGS = named({ ["log fast"] = constant(FADER, "SCALING_LOG_FAST", 1), ["log slow"] = constant(FADER, "SCALING_LOG_SLOW", 2),
  linear = constant(FADER, "SCALING_LINEAR", 3), ["exp slow"] = constant(FADER, "SCALING_EXP_SLOW", 4), ["exp fast"] = constant(FADER, "SCALING_EXP_FAST", 5) })

-- A field only one type of device has, by its names (`map`) both ways.
local function typed_field(kind, field, map, says, text)
  return {
    get = function(d)
      local ok, value = pcall(function() return d[field] end)
      return ok and map.get(value) or nil
    end,
    set = function(d, v) d[field] = map.parse(v) end, field = field,
    check = function(v, d) return M.moddevice_type(d.name) == kind and map.parse(v) ~= nil end, says = says, text = text,
  }
end

local function has_field(device, field) return pcall(function() return device[field] end) end

local function device_field(field, check, says, text)
  return {
    get = function(d) return d[field] end, set = function(d, v) d[field] = v end, field = field,
    check = function(v, d) return has_field(d, field) and check(v, d) end, says = says, text = text,
  }
end

local function same_points(a, b)
  if type(a) ~= "table" or type(b) ~= "table" or #a ~= #b then return false end
  for k = 1, #a do
    if a[k].time ~= b[k].time or not close(a[k].value, b[k].value) or not close(a[k].scaling or 0, b[k].scaling or 0) then return false end
  end
  return true
end

local MODDEVICE_FIELDS = {
  operator = { get = function(d) return OPERATORS.get(d.operator) end, set = function(d, v) d.operator = OPERATORS.parse(v) end, field = "operator",
    check = function(v) return OPERATORS.parse(v) ~= nil end, says = "operator is add, subtract, multiply or divide", text = function(v) return "operator " .. v end },
  bipolar = device_field("bipolar", is_boolean, "bipolar is true or false", function(v) return v and "bipolar" or "unipolar" end),
  tempoSynced = device_field("tempo_synced", function(v, d) return is_boolean(v) and d.tempo_sync_switching_allowed end, "tempoSynced is true or false, for a device with times", function(v) return v and "synced to the song" or "in milliseconds" end),
  active = { get = function(d) return d.is_active end, set = function(d, v) d.is_active = v end, field = "is_active", check = is_boolean, says = "active is true or false", text = function(v) return v and "on" or "bypassed" end },
  name = { get = function(d) return d.display_name end, set = function(d, v) d.display_name = v end, field = "display_name",
    check = function(v) return type(v) == "string" and #v <= 64 and not v:find("[%c]") end, says = "name must be one line of text", text = function(v) return "named " .. q(v) end },
  length = device_field("length", function(v, d)
    local stepper = M.moddevice_type(d.name) == "Stepper"
    return type(v) == "number" and v == math.floor(v) and v >= (stepper and 1 or 6) and v <= (stepper and 256 or 6144)
  end, "length is the envelope's length in points (Envelope 6-6144, Stepper 1-256)", function(v) return "length " .. v end),
  playMode = { get = function(d) return PLAY_MODES.get(d.play_mode) end, set = function(d, v) d.play_mode = PLAY_MODES.parse(v) end, field = "play_mode",
    check = function(v, d) return has_field(d, "play_mode") and PLAY_MODES.parse(v) ~= nil end, says = "playMode is points, lines or curves (Envelope and Stepper)", text = function(v) return "plays " .. v end },
  loopMode = { get = function(d) return ENV_LOOPS.get(d.loop_mode) end, set = function(d, v) d.loop_mode = ENV_LOOPS.parse(v) end, field = "loop_mode",
    check = function(v, d) return has_field(d, "loop_mode") and ENV_LOOPS.parse(v) ~= nil end, says = "loopMode is off, forward, reverse or pingpong (Envelope)", text = function(v) return "loop " .. v end },
  loopStart = device_field("loop_start", whole(1, 6144), "loopStart is a point of the envelope", function(v) return "loop from " .. v end),
  loopEnd = device_field("loop_end", whole(1, 6144), "loopEnd is a point of the envelope", function(v) return "loop to " .. v end),
  sustain = device_field("sustain_enabled", is_boolean, "sustain is true or false (Envelope)", function(v) return v and "sustain on" or "sustain off" end),
  sustainPosition = device_field("sustain_position", whole(1, 6144), "sustainPosition is a point of the envelope", function(v) return "sustain at " .. v end),
  fade = device_field("fade_amount", whole(0, 4095), "fade is from 0 to 4095 (a volume Envelope)", function(v) return "fade " .. v end),
  playStep = device_field("play_step", whole(-1, 16), "playStep is from -1 (random) to 16 (Stepper)", function(v) return v == -1 and "random steps" or ("step " .. v) end),
  waveform = typed_field("LFO", "mode", LFO_WAVES, "waveform is sine, saw, pulse or random (LFO)", function(v) return v .. " wave" end),
  mode = typed_field("Velocity Tracking", "mode", VELOCITY_MODES, "mode is clamp or scale (Velocity Tracking)", function(v) return v .. " mode" end),
  scaling = typed_field("Fader", "scaling", FADER_SCALINGS, "scaling is log fast, log slow, linear, exp slow or exp fast (Fader)", function(v) return v .. " scaling" end),
  points = {
    get = function(d) return points_of(d) end, set = function(d, v) d.points = v end, field = "points",
    check = function(v, d)
      if not has_field(d, "points") or type(v) ~= "table" or #v == 0 then return false end
      local seen = {}
      for _, point in ipairs(v) do
        if type(point) ~= "table" or not whole(1, 6144)(point.time) or type(point.value) ~= "number" or point.value < 0 or point.value > 1 or seen[point.time] then return false end
        if point.scaling ~= nil and (type(point.scaling) ~= "number" or point.scaling < -1 or point.scaling > 1) then return false end
        seen[point.time] = true
      end
      return true
    end,
    says = "points is a list of {time (1 to the length), value 0-1, scaling? -1..1}, no two at one time (Envelope and Stepper)",
    text = function(v) return #v .. " points" end,
  },
}
local MODDEVICE_ORDER = { "name", "active", "operator", "bipolar", "tempoSynced", "waveform", "mode", "scaling", "length", "playMode", "loopMode", "loopStart", "loopEnd", "sustain", "sustainPosition", "fade", "playStep", "points" }

local moddevice_set = instruments_.settings_kind({
  kind = "moddevice-set", of = "moddevice", noun = "settings", undoable = true, order = MODDEVICE_ORDER, fields = MODDEVICE_FIELDS,
})
-- (points are compared point by point, sorted by time as Renoise keeps them)
do
  local inner_same = moddevice_set.same
  moddevice_set.same = function(a, b)
    if (a.points ~= nil or b.points ~= nil) and not same_points(a.points, b.points) then return false end
    local a2, b2 = {}, {}
    for k, v in pairs(a) do if k ~= "points" then a2[k] = v end end
    for k, v in pairs(b) do if k ~= "points" then b2[k] = v end end
    return inner_same(a2, b2)
  end
  local inner_wanted = moddevice_set.wanted
  moddevice_set.wanted = function(object, input)
    local state = inner_wanted(object, input)
    if state.points then
      local sorted = {}
      for k, point in ipairs(state.points) do sorted[k] = { time = point.time, value = point.value, scaling = point.scaling or 0 } end
      table.sort(sorted, function(x, y) return x.time < y.time end)
      local length = state.length or (has_field(object, "length") and object.length) or 6144
      for _, point in ipairs(sorted) do if point.time > length then bad("a point's time is past the envelope's length (" .. length .. ")") end end
      state.points = sorted
    end
    return state
  end
end
instruments_.instrument_spec(moddevice_set)

-- Parameters of instrument-side devices (a modulation device's, a sample FX device's, a plug-in's): by number or name, value in
-- the parameter's range or text Renoise reads.

local function parameter_number(device, wanted)
  local count = #device.parameters
  if type(wanted) == "number" then
    if wanted ~= math.floor(wanted) or wanted < 1 or wanted > count then bad(string.format("param is a number from 1 to %d", count)) end
    return wanted
  end
  if type(wanted) ~= "string" or wanted == "" then bad("param: a parameter's number (from 1) or its name") end
  local lower = wanted:lower()
  for _, exact in ipairs({ true, false }) do
    local found
    for k = 1, count do
      local name = device:parameter(k).name:lower()
      if (exact and name == lower) or (not exact and name:sub(1, #lower) == lower) then
        if found then bad(string.format("%q names more than one parameter: give its number", wanted)) end
        found = k
      end
    end
    if found then return found end
  end
  local names = {}
  for k = 1, math.min(count, 24) do names[k] = device:parameter(k).name end
  bad(string.format("it has no parameter %q (it has %s%s)", wanted, table.concat(names, ", "), count > 24 and ", ..." or ""))
end
M.parameter_number = parameter_number

-- A parameter kind: `of` the device's kind (moddevice, chaindevice), `param_of` the param ref kind (or nil), `device_of(objects)`.
function M.param_kind(spec)
  local device_param = base.KINDS["device-param"]
  local kind = {
    kind = spec.kind, of = spec.of, noun = "parameter", undoable = true, by_place = true, restore_by_id = true,
    read = function(parameter) return { value = parameter.value, valueString = parameter.value_string } end,
    wanted = device_param.wanted, write = device_param.write, same = device_param.same, title = device_param.title,
    observables = function(parameter) return { parameter.value_observable } end, shown = device_param.shown,
  }
  kind.locate = function(song, env, input)
    local n, objects, label, k
    local parsed = type(input.ref) == "string" and input.ref:match("^[^:]+:(%a+):") or nil
    if type(input.place) == "table" then
      n, objects, label = places_.locate(song, env, spec.of, input, spec.kind)
      k = input.place.param
    elseif spec.param_of and parsed == spec.param_of then
      local numbers = refs.expect(input.ref, spec.param_of, #places_.SHAPES[spec.of].lists + 1, env.epoch())
      local count = #numbers
      k = numbers[count]
      local device_numbers = {}
      for j = 1, count - 1 do device_numbers[j] = numbers[j] end
      objects = places_.resolve(song, spec.of, device_numbers)
      n, label = device_numbers, places_.label_of(spec.of, device_numbers, objects)
    else
      n, objects, label = places_.locate(song, env, spec.of, input, spec.kind)
      k = parameter_number(spec.device_of(objects), input.param)
    end
    local device = spec.device_of(objects)
    if type(k) ~= "number" or k < 1 or k > #device.parameters then fail("gone", "that parameter isn't on the device any more") end
    local parameter = device:parameter(k)
    local target = places_.target_of(env, spec.of, n)
    target.place.param = k
    if spec.param_of then target.ref = places_.ref_of(env, spec.param_of, (function() local all = {} for j, v in ipairs(n) do all[j] = v end all[#all + 1] = k return all end)()) end
    local place = places_.journal_place(spec.of, n)
    place.param = k
    -- (what it is, not only where: the device's type (a plug-in's path) and the parameter's name)
    local ok, path = pcall(function() return device.device_path end)
    local id = tostring(ok and path or device.name) .. "/" .. tostring(parameter.name)
    return { object = parameter, name = label .. " " .. parameter.name, place = place, target = target, id = id }
  end
  kind.reread = function(song, place)
    local numbers = {}
    for k, key in ipairs(places_.SHAPES[spec.of].keys) do
      if type(place[key]) ~= "number" then return nil end
      numbers[k] = place[key]
    end
    local ok, device = pcall(function() return spec.device_of(places_.resolve(song, spec.of, numbers)) end)
    if not ok or not device or type(place.param) ~= "number" or place.param > #device.parameters then return nil end
    return device:parameter(place.param)
  end
  kind.watch = function(env, loc) if env.watch_undo then env.watch_undo(loc.object.value_observable) end end
  base.register(kind)
end

M.param_kind({ kind = "moddevice-param", of = "moddevice", device_of = function(objects) return objects.moddevice end })

-- Sample FX chains

-- What a meta device in a chain aims at, a list or nil. Inside its chain Dest. Track stays -1 and Dest. Effect counts the chain's
-- devices from 0, the mixer. Renoise follows a target made, moved or swapped, clears the aim as its target goes, and gives a device
-- made from its settings none; writing the values aims it again (lab 2026-11-01).
local function aims_of(device)
  local aims
  for _, destination in ipairs(material_.destinations(device)) do
    if destination.effect.value >= 0 then
      aims = aims or {}
      aims[#aims + 1] = { prefix = destination.prefix, effect = destination.effect.value, parameter = destination.parameter and destination.parameter.value }
    end
  end
  return aims
end

-- Writes `aims` on the device again: whether it took them all.
local function aim(device, aims)
  local all = true
  for _, wanted in ipairs(aims) do
    local found
    for _, destination in ipairs(material_.destinations(device)) do
      if destination.prefix == wanted.prefix then found = destination end
    end
    local took = found and pcall(function()
      found.effect.value = wanted.effect
      if found.parameter and wanted.parameter then found.parameter.value = wanted.parameter end
    end)
    if not took or found.effect.value ~= wanted.effect then all = false end
  end
  return all
end

-- The parameters a chain's meta devices move, by device and parameter: their values change on their own, so they say nothing of
-- whether anyone touched the device (Dest. Parameter is the parameter's own number, from 1: lab 2026-11-01).
local function moving_in(chain)
  local moving = {}
  for d = 2, #chain.devices do
    for _, wanted in ipairs(aims_of(chain:device(d)) or {}) do
      if wanted.parameter and wanted.parameter >= 1 then
        moving[wanted.effect + 1] = moving[wanted.effect + 1] or {}
        moving[wanted.effect + 1][wanted.parameter] = true
      end
    end
  end
  return moving
end

local AIMS_NOTE = "Its meta devices come back aimed at nothing (Renoise gives a device made from its settings no destination): aim them again."

local function chain_state(chain)
  local devices = {}
  for d = 2, #chain.devices do
    local state = material_.read_device(chain:device(d))
    state.aims = aims_of(chain:device(d))
    devices[#devices + 1] = state
  end
  local mixer = {}
  local first = chain:device(1)
  for k = 1, #first.parameters do mixer[k] = first:parameter(k).value end
  return { name = chain.name, routing = chain.output_routing, devices = devices, mixer = mixer }
end

function M.chain_print(chain)
  local states, moving = {}, moving_in(chain)
  for d = 1, #chain.devices do states[d] = material_.device_state(chain:device(d), moving[d]).state end
  return { name = chain.name, routing = chain.output_routing, devices = #chain.devices, hash = state_hash(states) }
end

local function routing_check(chain, routing)
  for _, name in ipairs(chain.available_output_routings) do
    if name == routing then return true end
  end
  return false
end

-- Where a chain may send its sound, as any chain of the song lists it (the same for all of them); nil when the song has none yet.
local function known_routings(song)
  for i = 1, #song.instruments do
    local instrument = song:instrument(i)
    if #instrument.sample_device_chains > 0 then return instrument:sample_device_chain(1).available_output_routings end
  end
  return nil
end

local function routing_says(routings)
  return "outputRouting is one of the chain's routings: " .. table.concat(routings, ", ")
end

local add_chain = add_in_instrument({
  kind = "chain-add", of = "chain", what = "sample FX chain",
  count = function(i) return #i.sample_device_chains end,
  insert = function(i, index) return i:insert_sample_device_chain_at(index) end,
  delete = function(i, index) i:delete_sample_device_chain_at(index) end,
  print = function(chain) return M.chain_print(chain) end,
  check = function(song, _, input)
    if input.outputRouting == nil then return end
    if type(input.outputRouting) ~= "string" then bad("outputRouting is a routing's name") end
    local routings = known_routings(song)
    if not routings then return end
    for _, name in ipairs(routings) do if name == input.outputRouting then return end end
    bad(routing_says(routings))
  end,
  after = function(chain, input)
    if input.outputRouting ~= nil then
      -- (checked before when the song had a chain to ask; else here, where a refusal takes the new chain back with it)
      if not routing_check(chain, input.outputRouting) then error({ code = "bad-params", message = routing_says(chain.available_output_routings) }, 0) end
      chain.output_routing = input.outputRouting
    end
  end,
})

base.register({
  kind = "chain-add", custom = true, shape = "made", by_place = true,
  preview = function(song, env, input) return add_chain(song, env, input, nil, nil, true) end,
  apply = function(song, env, input, fence, desc) return add_chain(song, env, input, fence, desc) end,
  restore = function(song, env, restore, desc) return remove_made_in(song, env, restore, desc, "chain-add", "chain", "device_chain_index") end,
})

local function write_chain(chain, state)
  local notes = {}
  pcall(function() chain.name = state.name end)
  if state.routing and not pcall(function() chain.output_routing = state.routing end) then
    notes[#notes + 1] = string.format("Its output (%s) isn't there any more: it goes to the current track.", tostring(state.routing))
  end
  local mixer = chain:device(1)
  for k, value in ipairs(state.mixer or {}) do pcall(function() mixer:parameter(k).value = value end) end
  for d, device in ipairs(state.devices or {}) do
    if not pcall(material_.restore_device, chain, d + 1, device) then notes[#notes + 1] = "A device of it (" .. tostring(device.path) .. ") couldn't be made again." end
  end
  -- (all made, so that each aim finds its target where it was; a record of the Kumi before this kept only that there were aims)
  local unaimed = state.aimed
  for d, device in ipairs(state.devices or {}) do
    if device.aims and not (d + 1 <= #chain.devices and aim(chain:device(d + 1), device.aims)) then unaimed = true end
  end
  if unaimed then notes[#notes + 1] = AIMS_NOTE end
  return notes
end

local function print_of_chain_state(state)
  -- (compared by name, routing and how many devices: a device made again from its preset data hashes alike in Renoise)
  return { name = state.name, routing = state.routing, devices = #(state.devices or {}) + 1 }
end

-- (a chain made again is that chain by its name, output and devices: their preset data makes them alike, not their hashes)
local function chain_back_print(chain) return { name = chain.name, routing = chain.output_routing, devices = #chain.devices } end

delete_in_instrument({
  kind = "chain-delete", of = "chain", list = "chains", what = "sample FX chain", field = "device_chain_index",
  note = "Kumi keeps its devices (with their settings), its output and which samples use it, to put back.",
  print = function(chain) return M.chain_print(chain) end, back_print = chain_back_print, state = chain_state, write = write_chain,
  print_of_state = print_of_chain_state,
  count = function(i) return #i.sample_device_chains end, get = function(i, at) return i:sample_device_chain(at) end,
  insert = function(i, at) return i:insert_sample_device_chain_at(at) end, delete = function(i, at) i:delete_sample_device_chain_at(at) end,
  -- (its mixer, and the devices after it)
  devices_kept = function(state) return 1 + #(state.devices or {}) end,
})

-- chain-set {ref: chain, name?, outputRouting?}

instruments_.instrument_spec(instruments_.settings_kind({
  kind = "chain-set", of = "chain", noun = "settings", undoable = true, order = { "name", "outputRouting" },
  fields = {
    name = { get = function(c) return c.name end, set = function(c, v) c.name = v end, field = "name",
      check = function(v) return type(v) == "string" and #v <= 64 and not v:find("[%c]") end, says = "name must be one line of text", text = function(v) return "named " .. q(v) end },
    outputRouting = { get = function(c) return c.output_routing end, set = function(c, v) c.output_routing = v end, field = "output_routing",
      check = function(v, c) return type(v) == "string" and routing_check(c, v) end, says = "outputRouting is one of the chain's routings (\"Current Track\", or a track's name)",
      text = function(v) return "out to " .. v end },
  },
}))

-- chaindevice-add {ref: chain, path, index? (2 or later: the chain's mixer is first)}

-- (a sample FX chain lists its devices' paths only, with no infos (lab): the names are the tracks' infos of the same paths)
local function device_names(song)
  local names = {}
  local ok, infos = pcall(function() return song:track(1).available_device_infos end)
  for _, info in ipairs(ok and infos or {}) do names[info.path] = info end
  return names
end

local function resolve_chain_device(chain, wanted, song)
  if type(wanted) ~= "string" or wanted == "" then bad("path: a device's path (as Renoise lists it) or its name") end
  local lower, matches, near = wanted:lower(), {}, {}
  local names = device_names(song)
  for _, path in ipairs(chain.available_devices) do
    if path == wanted then return path end
    local last = path:match("([^/]+)$") or path
    local info = names[path] or {}
    if path:lower() == lower or last:lower() == lower or (info.short_name or ""):lower() == lower or (info.name or ""):lower() == lower then
      matches[#matches + 1] = path
    elseif path:lower():find(lower, 1, true) and #near < 5 then
      near[#near + 1] = path
    end
  end
  if #matches == 1 then return matches[1] end
  if #matches > 1 then bad(string.format("%q names %d devices: say which by its path (%s)", wanted, #matches, table.concat(matches, ", ", 1, math.min(#matches, 5)))) end
  bad(string.format("a sample FX chain has no device %q.%s", wanted, #near > 0 and (" Close: " .. table.concat(near, ", ") .. ".") or ""))
end

local function chaindevice_print(device, chain, at)
  local state = material_.device_state(device, chain and moving_in(chain)[at])
  return { path = state.path, state = state.state, active = state.active, display = state.display }
end

local function chaindevice_add(song, env, input, fence, desc, preview_only)
  local n, objects, label = places_.locate(song, env, "chain", input, "chaindevice-add")
  local chain = objects.chain
  local path = resolve_chain_device(chain, input.path, song)
  local count = #chain.devices
  local index = index_in(input.index, count, "index", 2)
  local name = path:match("([^/]+)$") or path
  local title = string.format("Added %s to %s", q(name), label)
  if preview_only then
    return { target = places_.target_of(env, "chain", n), prior = { devices = count }, fence = { devices = count, index = index }, display = { title = title, before = { devices = count }, after = { devices = count + 1 } } }
  end
  if count ~= fence.devices or index ~= fence.index then fail("changed", label .. "'s devices changed since the preview") end
  song:describe_undo(desc)
  local made, device = pcall(function() return chain:insert_device_at(path, index) end)
  if not made or not device then fail("failed", "Renoise wouldn't make " .. path .. (made and "" or (": " .. tostring(device):gsub("^[^%s:]*%.lua:%d+: ", "")))) end
  local m = { n[1], n[2], index }
  return {
    applied = { index = index, path = path, print = chaindevice_print(device), content = material_.content_fence(device) },
    created = { ref = places_.ref_of(env, "chaindevice", m), kind = "chaindevice" },
    material = { made = places_.place_of(env, "chaindevice", m) }, target = places_.target_of(env, "chaindevice", m),
    display = { title = title, before = { devices = count }, after = { devices = count + 1 } }, undo = { desc = desc, entry = true },
    prior = { devices = count }, place = places_.journal_place("chaindevice", m), shape = "made",
  }
end

base.register({
  kind = "chaindevice-add", custom = true, shape = "made", by_place = true,
  preview = function(song, env, input) return chaindevice_add(song, env, input, nil, nil, true) end,
  apply = function(song, env, input, fence, desc) return chaindevice_add(song, env, input, fence, desc) end,
  restore = function(song, env, restore, desc)
    local material = restore.material
    if type(material) ~= "table" or type(material.made) ~= "table" then bad("material: the device the change made") end
    local n, why, which = places_.follow(env, material.made)
    if not n then
      if why == "gone" then fail("gone", "the device Kumi made isn't in the chain any more") end
      places_.follow_failed(why, which, nil)
    end
    local objects = places_.resolve(song, "chaindevice", n)
    local label = places_.label_of("chaindevice", n, objects)
    if not restore.discard and not base.same_table(chaindevice_print(objects.chaindevice), (restore.applied or {}).print) then
      fail("changed", label .. " changed in Renoise since Kumi made it")
    end
    -- (a Doofer's devices inside, a plug-in's state: what its parameters don't show)
    local content = (restore.applied or {}).content
    if not restore.discard and content ~= nil and material_.content_fence(objects.chaindevice) ~= content then
      fail("changed", label .. " changed in Renoise since Kumi made it (what's inside it, its text or its state)")
    end
    song:describe_undo(desc)
    objects.chain:delete_device_at(n[3])
    base.note_restored_at(env.journal, "chaindevice-add", places_.journal_place("chaindevice", n))
    return { restored = { deleted = true }, display = { title = "Removed " .. label .. " (put back)" }, undo = { desc = desc, entry = true } }
  end,
})

-- (a device made again from its preset data is that device by its path, bypass and name, whatever its hash)
local function chaindevice_back_print(device)
  local state = material_.device_state(device)
  return { path = state.path, active = state.active, display = state.display }
end

device_in_list({
  kind = "chaindevice-delete", of = "chaindevice", owner = "chain", list = "chaindevices", first = 2,
  check = function(n) if n[3] == 1 then bad("a chain's first device is its mixer: it stays") end end,
  note = function(instrument) return "Kumi keeps its settings (its preset data) to put back." .. (has_mappings(instrument) and (" " .. MAPPING_NOTE) or "") end,
  print = chaindevice_print, back_print = chaindevice_back_print,
  -- (what it aims at, what else in the chain aims at it, which Renoise clears as it goes, and the chain it leaves: those numbers
  -- mean the same only in that chain)
  state = function(device, chain, at)
    local state = material_.read_device(device)
    state.aims, state.at = aims_of(device), at
    local rest, aiming = {}, {}
    for d = 2, #chain.devices do
      if d ~= at then
        local other = chain:device(d)
        rest[#rest + 1] = other.device_path
        local aims = {}
        for _, wanted in ipairs(aims_of(other) or {}) do
          if wanted.effect == at - 1 then aims[#aims + 1] = wanted end
        end
        if #aims > 0 then aiming[#aiming + 1] = { at = d, aims = aims } end
      end
    end
    state.rest, state.aiming = rest, #aiming > 0 and aiming or nil
    return state
  end,
  print_of_state = function(state) return { path = state.path, active = state.active ~= false, display = state.display } end,
  make = function(chain, at, state) return material_.restore_device(chain, at, state) end,
  -- What it aimed at and what aimed at it, again, where the chain is as it left it: else the numbers kept mean other devices.
  after_make = function(chain, at, state)
    local unaimed = "It comes back aimed at nothing, and what aimed at it too (Renoise gives a device made from its settings no destination): aim them again."
    if state.aimed then return unaimed end
    if not state.aims and not state.aiming then return nil end
    local same = at == state.at and #chain.devices == #state.rest + 2
    for k, path in ipairs(same and state.rest or {}) do
      local d = k + 1 < at and k + 1 or k + 2
      if chain:device(d).device_path ~= path then same = false end
    end
    if not same then return unaimed end
    local all = not state.aims or aim(chain:device(at), state.aims)
    for _, other in ipairs(state.aiming or {}) do
      all = aim(chain:device(other.at), other.aims) and all
    end
    return not all and unaimed or nil
  end,
})

-- chaindevice-move {ref: chaindevice, index}: by swaps; the device ends at index.

local function move_plan(song, env, input)
  local n, objects, label = places_.locate(song, env, "chaindevice", input, "chaindevice-move")
  if n[3] == 1 then bad("a chain's first device is its mixer: it stays") end
  local count = #objects.chain.devices
  if not whole(2, count)(input.index) then bad(string.format("index is a place from 2 to %d (the mixer is first)", count)) end
  return { n = n, label = label, to = input.index, print = chaindevice_print(objects.chaindevice) }
end

local function swap_along(chain, from, to)
  local step = to > from and 1 or -1
  for k = from, to - step, step do chain:swap_devices_at(k, k + step) end
end

base.register({
  kind = "chaindevice-move", custom = true, by_place = true,
  preview = function(song, env, input)
    local plan = move_plan(song, env, input)
    local display = { title = string.format("Moved %s to place %d", plan.label, plan.to), before = { index = plan.n[3] }, after = { index = plan.to } }
    if plan.to == plan.n[3] then display.unchanged = true end
    return { target = places_.target_of(env, "chaindevice", plan.n), prior = { index = plan.n[3] }, fence = { index = plan.n[3], print = plan.print }, display = display }
  end,
  apply = function(song, env, input, fence, desc)
    local ok, plan = pcall(move_plan, song, env, input)
    if not ok then base.gone_is_changed(plan) end
    if plan.n[3] ~= fence.index or not base.same_table(plan.print, fence.print) then fail("changed", plan.label .. " changed since the preview") end
    local display = { title = string.format("Moved %s to place %d", plan.label, plan.to), before = { index = plan.n[3] }, after = { index = plan.to } }
    local m = { plan.n[1], plan.n[2], plan.to }
    if plan.to == plan.n[3] then
      display.unchanged = true
      return { applied = { index = plan.to }, display = display, undo = { desc = desc, entry = false }, prior = { index = plan.n[3] }, target = places_.target_of(env, "chaindevice", m) }
    end
    song:describe_undo(desc)
    swap_along(song:instrument(plan.n[1]):sample_device_chain(plan.n[2]), plan.n[3], plan.to)
    return {
      applied = { index = plan.to }, material = { at = places_.place_of(env, "chaindevice", m) }, target = places_.target_of(env, "chaindevice", m),
      display = display, undo = { desc = desc, entry = true }, prior = { index = plan.n[3] }, place = places_.journal_place("chaindevice", m),
      follows = "chaindevice",
    }
  end,
  restore = function(song, env, restore, desc)
    local material, prior = restore.material, restore.prior
    if type(material) ~= "table" or type(material.at) ~= "table" or type(prior) ~= "table" then bad("material and prior: where the device moved") end
    local n, why, which = places_.follow(env, material.at)
    if not n then places_.follow_failed(why, which, nil) end
    local objects = places_.resolve(song, "chaindevice", n)
    local label = places_.label_of("chaindevice", n, objects)
    if n[3] == prior.index then return { restored = { index = n[3] }, display = { title = label .. " is at its place already" }, undo = { desc = desc, entry = false }, unwritten = true } end
    -- (moved again by the producer since: their order stays, unless discard)
    if not restore.discard and n[3] ~= (restore.applied or {}).index then fail("changed", label .. " was moved in Renoise since Kumi moved it") end
    if prior.index > #objects.chain.devices or prior.index < 2 then fail("changed", "place " .. prior.index .. " isn't in the chain any more") end
    song:describe_undo(desc)
    swap_along(objects.chain, n[3], prior.index)
    base.note_restored(env.journal, "chaindevice-move", places_.journal_place("chaindevice", { n[1], n[2], prior.index }), restore.applied, base.KINDS["chaindevice-move"])
    return { restored = { index = prior.index }, display = { title = string.format("Moved %s back to place %d", label, prior.index) }, undo = { desc = desc, entry = true } }
  end,
  same = function(a, b) return a.index == b.index end,
})

-- chaindevice-bypass {ref, bypass}, chaindevice-param {ref: chainparam | chaindevice + param, value | valueString}

instruments_.instrument_spec({
  kind = "chaindevice-bypass", of = "chaindevice", noun = "bypass", undoable = true,
  read = function(device) return { bypass = not device.is_active } end,
  observables = function(device) return { device.is_active_observable } end,
  decorate = function(loc) if loc.numbers[3] == 1 then bad("a chain's first device is its mixer: it is always on") end end,
  wanted = function(_, input)
    if type(input.bypass) ~= "boolean" then bad("bypass must be true or false") end
    return { bypass = input.bypass }
  end,
  write = function(device, state) device.is_active = not state.bypass end,
  same = function(a, b) return a.bypass == b.bypass end,
  title = function(name, _, to) return name .. (to.bypass and " bypassed" or " switched on") end,
})

M.param_kind({ kind = "chaindevice-param", of = "chaindevice", param_of = "chainparam", device_of = function(objects) return objects.chaindevice end })

return M
