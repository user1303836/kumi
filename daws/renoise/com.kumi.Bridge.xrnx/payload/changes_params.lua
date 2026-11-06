-- Kumi's change of several parameters of one device in one step (M4 n-devices, for M5's listening: a gain stage, an EQ
-- fit): device-params {ref: device, values: [{param: number from 1 | name, value? | valueString?}]}. Each value is asked
-- and checked as device-param asks it; all are fenced and checked before anything is written, then written in one
-- callback under one describe_undo, and read back in Renoise's words. The device is kept by its place (its track carried
-- through the layout, the device followed through the track's device log), the values by parameter number.
--
-- Values that are places are kept as the places they name, never as Renoise's numbers, which Renoise moves with the tracks
-- and devices made and deleted: a Receiver (#Send, #Multiband Send) as the send track it names, a destination (a meta
-- device's, a *Hydra or *XY Pad output's, a #Sidechain's Track, Effect and Parameter) as the device it drives, as
-- meta-destination keeps it.

local import = ...
local refs = import("refs")
local rows = import("rows")
local base = import("changes_base")
local material_ = import("changes_material")
local devices_ = import("changes_devices")
local presets = import("changes_presets")

local bad, fail, q = base.bad, base.fail, base.q
local dest = presets.destination
local param_spec = devices_.param_spec

-- The most values one change sets (as REAPER's fx-params).
local MAX_VALUES = 256

local function label_of(t, track, device) return presets.label_of(t, track, device) end

-- A parameter's number as the key the records keep it under ("3").
local function key_of(k) return string.format("%d", k) end

local function title(device, t, track, count)
  return string.format("%s %d %s on %s", devices_.label(device), count, count == 1 and "parameter" or "parameters", q(base.named(t, track)))
end

-- The destinations of the device by their parameters' numbers: each parameter that is a destination's Track, Effect or
-- Parameter names its destination.
local function destinations_by_number(device)
  local out = {}
  for _, destination in ipairs(material_.destinations(device)) do
    for _, k in pairs(destination.numbers) do out[k] = destination end
  end
  return out
end

-- What a destination's parameter (its Effect or Parameter: text and value are one number) asks: a number.
local function number_asked(parameter, ask)
  local value = ask.value
  if value == nil and type(ask.valueString) == "string" then value = tonumber(ask.valueString:match("^%s*(%-?%d+)")) end
  if type(value) ~= "number" or value ~= math.floor(value) or value < 0 or value > parameter.value_max then
    bad(string.format("%s is a number from 0 to %d (as Renoise shows it): give value, or valueString with that number", parameter.name, parameter.value_max))
  end
  return value
end

-- The track a Receiver's value names (the send tracks come after the master, counted from 0), or nil.
local function send_track(song, value) return devices_.track_of_value(song, "send", value) end

-- What each value asks, checked as device-param checks it, before anything is read for the fence: a list of
-- {k, parameter, wanted (device-param's state), how ("plain" | "send" | "destination"), destination}.
local function asks_of(song, device, values, label)
  if type(values) ~= "table" or #values == 0 then bad("values: a list of {param, value | valueString}, 1 to 256 of them") end
  if #values > MAX_VALUES then bad(string.format("values: at most %d in one change (%d given)", MAX_VALUES, #values)) end
  local dests = destinations_by_number(device)
  local asks, seen = {}, {}
  -- (the device's names read once for all the values: a plug-in has thousands of parameters)
  local names
  for i, value in ipairs(values) do
    if type(value) ~= "table" then bad(string.format("values[%d]: {param, value | valueString}", i)) end
    if type(value.param) == "string" and not names then names = devices_.parameter_names(device) end
    local k = devices_.parameter_number(device, value.param, names)
    local parameter = device:parameter(k)
    if seen[k] then bad(string.format("%s is given twice (values %d and %d)", parameter.name, seen[k], i)) end
    seen[k] = i
    local ask = { k = k, parameter = parameter, how = "plain" }
    local points = devices_.points_at(device, parameter)
    if dests[k] then
      ask.how, ask.destination = "destination", dests[k]
      if k == dests[k].numbers[1] then
        -- (its Track: text from 1, -1 none, as device-param reads it)
        ask.wanted = param_spec.wanted_at({ object = parameter, name = label .. " " .. parameter.name, points = "track" }, value)
      else
        ask.wanted = { value = number_asked(parameter, value) }
      end
    else
      ask.wanted = param_spec.wanted_at({ object = parameter, name = label .. " " .. parameter.name, points = points }, value)
      if points == "send" then
        ask.how = "send"
        -- (a Receiver given as text is checked as one given as a number: a send track there is)
        devices_.receiver_check(device, parameter, ask.wanted.value, song)
      end
    end
    asks[#asks + 1] = ask
  end
  return asks
end

-- The destinations the asks touch, by prefix: each with what it names now and what the asks make of it (a destination
-- aimed nowhere, Track -1, has its Effect and Parameter cleared too, as meta-destination does).
local function touched_of(asks)
  local out, order = {}, {}
  for _, ask in ipairs(asks) do
    local destination = ask.destination
    if destination then
      local entry = out[destination.prefix]
      if not entry then
        entry = { destination = destination, now = dest.numbers_of(destination) }
        entry.wanted = { track = entry.now.track, effect = entry.now.effect, parameter = entry.now.parameter }
        out[destination.prefix] = entry
        order[#order + 1] = destination.prefix
      end
      local field = ask.k == destination.numbers[1] and "track" or ask.k == destination.numbers[2] and "effect" or "parameter"
      entry.wanted[field] = ask.wanted.value
    end
  end
  for _, prefix in ipairs(order) do
    local wanted = out[prefix].wanted
    if wanted.track < 0 then
      wanted.track, wanted.effect = -1, -1
      if wanted.parameter ~= nil then wanted.parameter = -1 end
    end
  end
  return out, order
end

-- A destination's numbers checked against the song: a track there is, a device on it, a parameter of it, not the device
-- itself. (Renoise takes any number silently.)
local function check_destination(song, entry, self_t, self_d)
  local wanted, prefix = entry.wanted, entry.destination.prefix
  if wanted.track < 0 then return end
  local t, d = wanted.track + 1, wanted.effect + 1
  if t > rows.track_count(song) then
    bad(string.format("%sTrack is a track from 1 to %d", prefix, rows.track_count(song)))
  end
  local track = song:track(t)
  if wanted.effect < 0 or d > #track.devices then
    bad(string.format("%sEffect is a device of %s from 0 (its mixer) to %d", prefix, q(base.named(t, track)), #track.devices - 1))
  end
  if t == self_t and d == self_d then bad("a device can't drive itself") end
  if wanted.parameter ~= nil and wanted.parameter > #track:device(d).parameters then
    bad(string.format("%sParameter is a parameter of %s from 0 (its on/off) to %d", prefix, q(devices_.label(track:device(d))), #track:device(d).parameters))
  end
end

local function locate(song, env, input)
  local t, d, track, device = presets.device_by_ref(song, env, input.ref, "device-params")
  local label = label_of(t, track, device)
  return t, d, track, device, label, asks_of(song, device, input.values, label)
end

-- A parameter's value as the restore keeps it: its value and Renoise's text, and for a Receiver the send track it names
-- (`to`; a Receiver that names none, with no send tracks in the song, is kept as its number).
local function kept_value(song, ask, parameter)
  local state = { value = parameter.value, valueString = parameter.value_string }
  if ask.how == "send" then state.to = send_track(song, parameter.value) end
  return state
end

-- What a parameter's text says, with the track a place names (the producer's words).
local function line(song, parameter, how)
  local text = parameter.name .. " → " .. base.minus_text(parameter.value_string)
  if how == "send" then
    local n = send_track(song, parameter.value)
    if n then text = text .. " (" .. q(base.named(n, song:track(n))) .. ")" end
  end
  return text
end

local function wanted_line(ask)
  local wanted = ask.wanted
  return ask.parameter.name .. " → " .. base.minus_text(wanted.valueString or tostring(wanted.value))
end

-- The values the journal compares after the producer's undo or redo: every touched parameter's value, by number.
local function raw_of(device, state)
  local out = { params = {} }
  for key in pairs((state or {}).params or {}) do
    local k = tonumber(key)
    if k and k >= 1 and k <= #device.parameters then out.params[key] = device:parameter(k).value end
  end
  return out
end

local function same_raw(a, b)
  if type(a) ~= "table" or type(b) ~= "table" or type(a.params) ~= "table" or type(b.params) ~= "table" then return false end
  for key, value in pairs(a.params) do
    if not base.close(value, b.params[key]) then return false end
  end
  for key in pairs(b.params) do
    if a.params[key] == nil then return false end
  end
  return true
end

local function preview(song, env, input)
  local t, d, track, device, label, asks = locate(song, env, input)
  local touched, order = touched_of(asks)
  for _, prefix in ipairs(order) do check_destination(song, touched[prefix], t, d) end
  local prior_params, fenced, wanted, fenced_dests = {}, {}, {}, {}
  local lines, before, after, unchanged = {}, {}, {}, true
  for _, ask in ipairs(asks) do
    local key, parameter = key_of(ask.k), ask.parameter
    prior_params[key] = { value = parameter.value, valueString = parameter.value_string }
    fenced[key] = parameter.value
    before[parameter.name], after[parameter.name] = base.minus_text(parameter.value_string), ask.wanted.valueString or ask.wanted.value
    lines[#lines + 1] = wanted_line(ask)
    if ask.how == "send" then
      -- (what the wanted number names now: the apply refuses it if that moved in between, as device-param does)
      local n = send_track(song, ask.wanted.value)
      if n then wanted[key] = refs.make(env.epoch(), "track", n) end
    end
    if ask.how ~= "destination" and not param_spec.same({ value = parameter.value, valueString = parameter.value_string }, ask.wanted) then unchanged = false end
  end
  for _, prefix in ipairs(order) do
    local entry = touched[prefix]
    fenced_dests[prefix] = entry.now
    -- (the device the wanted numbers name, as a place: a device made before it in between moves the numbers)
    if entry.wanted.track >= 0 then
      local wt, wd = entry.wanted.track + 1, entry.wanted.effect + 1
      wanted[prefix] = { track = wt, dev = material_.device_place(env, wt, wd, song:track(wt):device(wd)) }
    end
    if not dest.same_numbers(entry.now, entry.wanted) then unchanged = false end
  end
  local display = { title = title(device, t, track, #asks), lines = lines, before = before, after = after, unchanged = unchanged or nil }
  local fence = { id = presets.identity(track, device), label = label, params = fenced, wanted = wanted, destinations = fenced_dests }
  return { target = presets.target_of(t, d), prior = { params = prior_params }, fence = fence, display = display }
end

local function apply(song, env, input, fence, desc)
  local ok, t, d, track, device, label, asks = pcall(locate, song, env, input)
  if not ok then base.gone_is_changed(t) end
  if fence.id ~= presets.identity(track, device) then fail("changed", base.moved_text(fence.label or label)) end
  local fenced_params = type(fence.params) == "table" and fence.params or {}
  local fenced_dests = type(fence.destinations) == "table" and fence.destinations or {}
  local fenced_wanted = type(fence.wanted) == "table" and fence.wanted or {}
  -- Everything is checked before anything is written: the values the preview read, what the wanted values name.
  local moved = {}
  for _, ask in ipairs(asks) do
    local key = key_of(ask.k)
    if fenced_params[key] == nil or not base.close(ask.parameter.value, fenced_params[key]) then moved[#moved + 1] = ask.parameter.name end
  end
  local touched, order = touched_of(asks)
  for _, prefix in ipairs(order) do
    local kept = fenced_dests[prefix]
    if type(kept) ~= "table" or not dest.same_numbers(touched[prefix].now, kept) then moved[#moved + 1] = prefix .. "Track" end
  end
  if #moved > 0 then
    fail("changed", string.format("%s of %s changed since the preview", table.concat(moved, ", "), q(label)))
  end
  for _, ask in ipairs(asks) do
    local key = key_of(ask.k)
    if ask.how == "send" and fenced_wanted[key] ~= nil then
      local was = refs.expect(fenced_wanted[key], "track", 1, env.epoch())[1]
      if send_track(song, ask.wanted.value) ~= was then
        fail("changed", string.format("the send track %s's %s names moved since the preview: read again", q(label), ask.parameter.name))
      end
    end
  end
  for _, prefix in ipairs(order) do
    local entry = touched[prefix]
    check_destination(song, entry, t, d)
    local kept = fenced_wanted[prefix]
    if type(kept) == "table" and entry.wanted.track >= 0 then
      local wd = material_.device_now(song, env, entry.wanted.track + 1, kept.dev)
      if kept.track ~= entry.wanted.track + 1 or wd ~= entry.wanted.effect + 1 then
        fail("changed", string.format("the device %s's %s names moved since the preview: read again", q(label), prefix .. "Effect"))
      end
    end
  end
  -- What the restore needs, read before the write: the values and the places they name.
  local material = presets.keep(env, t, d, track, device)
  local kept_params, kept_dests, journal_params = {}, {}, {}
  for _, ask in ipairs(asks) do
    local key = key_of(ask.k)
    journal_params[key] = ask.parameter.value
    if ask.how ~= "destination" then kept_params[key] = kept_value(song, ask, ask.parameter) end
  end
  for _, prefix in ipairs(order) do kept_dests[prefix] = dest.place_of_numbers(song, env, touched[prefix].now) end
  material.prior, material.count = { params = kept_params, destinations = kept_dests }, #asks
  local journal_prior = { params = journal_params }
  -- One step: every value written in this callback.
  local todo = {}
  for _, ask in ipairs(asks) do
    if ask.how ~= "destination" and not param_spec.same({ value = ask.parameter.value, valueString = ask.parameter.value_string }, ask.wanted) then
      todo[#todo + 1] = ask
    end
  end
  local dests_todo = {}
  for _, prefix in ipairs(order) do
    if not dest.same_numbers(touched[prefix].now, touched[prefix].wanted) then dests_todo[#dests_todo + 1] = prefix end
  end
  local written = #todo > 0 or #dests_todo > 0
  if written then
    song:describe_undo(desc)
    if env.watch_param then
      for _, ask in ipairs(asks) do env.watch_param(t, d, ask.k) end
    end
    for _, ask in ipairs(todo) do param_spec.write(ask.parameter, ask.wanted) end
    for _, prefix in ipairs(dests_todo) do dest.write_numbers(touched[prefix].destination, touched[prefix].wanted) end
    -- (what Renoise kept: every value written goes back, so nothing half-done stays)
    local kept = {}
    for _, ask in ipairs(todo) do
      if not param_spec.same({ value = ask.parameter.value, valueString = ask.parameter.value_string }, ask.wanted) then kept[#kept + 1] = ask.parameter.name end
    end
    for _, prefix in ipairs(dests_todo) do
      if not dest.same_numbers(dest.numbers_of(touched[prefix].destination), touched[prefix].wanted) then kept[#kept + 1] = prefix .. "Track" end
    end
    if #kept > 0 then
      -- A refused setter may have written nothing. Don't write its unchanged value back: that write can make a
      -- native undo step of its own. A partial write still goes back inside this callback's one undo group: its
      -- native step stays, rather than taking back or ungrouping the producer's native undo/redo history to hide it.
      for _, ask in ipairs(todo) do
        local was = journal_params[key_of(ask.k)]
        if not base.close(ask.parameter.value, was) then pcall(function() ask.parameter.value = was end) end
      end
      for _, prefix in ipairs(dests_todo) do
        local entry = touched[prefix]
        if not dest.same_numbers(dest.numbers_of(entry.destination), entry.now) then
          pcall(dest.write_numbers, entry.destination, entry.now)
        end
      end
      fail("failed", string.format("Renoise kept %s of %s as they were; nothing is changed", table.concat(kept, ", "), q(label)))
    end
  end
  -- Read back in Renoise's words.
  local applied_params, applied_dests, applied_raw, lines = {}, {}, {}, {}
  for _, ask in ipairs(asks) do
    local key = key_of(ask.k)
    applied_raw[key] = ask.parameter.value
    if ask.how ~= "destination" then applied_params[key] = kept_value(song, ask, ask.parameter) end
    lines[#lines + 1] = line(song, ask.parameter, ask.how)
  end
  for _, prefix in ipairs(order) do
    local numbers = dest.numbers_of(touched[prefix].destination)
    applied_dests[prefix] = dest.place_of_numbers(song, env, numbers)
    lines[#lines + 1] = prefix .. "→ " .. dest.target_words(song, numbers)
  end
  return {
    applied = { params = applied_params, destinations = applied_dests }, material = material, target = presets.target_of(t, d),
    display = { title = title(device, t, track, #asks), lines = lines, unchanged = not written or nil },
    undo = { desc = desc, entry = written }, prior = journal_prior, journal_applied = { params = applied_raw }, place = { track = t, device = d },
  }
end

-- A kept value as Renoise's number now: a plain value as it was; a Receiver's from the send track it named, carried from
-- `layout` (nil and why: "gone", "unknown").
local function value_now(song, env, state, layout)
  if type(state) ~= "table" then return nil, "unknown" end
  if state.to == nil then return state.value end
  local n, why = env.shifted(state.to, layout)
  if not n then return nil, why end
  if n < 1 or n > rows.track_count(song) then return nil, "gone" end
  local value = devices_.value_of_track(song, "send", n)
  if value < 0 or value >= song.send_track_count then return nil, "gone" end
  return value
end

local function restore(song, env, record, desc)
  local material, applied = record.material, record.applied
  if type(material) ~= "table" or type(material.prior) ~= "table" or type(applied) ~= "table" then
    bad("material and applied: the device and the values before and after")
  end
  local t, d, track, device = presets.device_by_place(song, env, material, "Kumi set them")
  local label = label_of(t, track, device)
  local prior_params, applied_params = material.prior.params or {}, applied.params or {}
  local prior_dests, applied_dests = material.prior.destinations or {}, applied.destinations or {}
  local count = type(material.count) == "number" and material.count or 0
  -- Each parameter, in the device's order: back already, still at what Kumi set (to be put back), or changed since.
  local keys, prefixes = {}, {}
  for key in pairs(prior_params) do keys[#keys + 1] = key end
  table.sort(keys, function(a, b) return (tonumber(a) or 0) < (tonumber(b) or 0) end)
  for prefix in pairs(prior_dests) do prefixes[#prefixes + 1] = prefix end
  table.sort(prefixes)
  if count == 0 then count = #keys + #prefixes end
  local writes, moved = {}, {}
  for _, key in ipairs(keys) do
    local state = prior_params[key]
    local k = tonumber(key)
    if not k or k < 1 or k > #device.parameters then
      fail("changed", string.format("%s has fewer parameters than when Kumi set them", q(label)))
    end
    local parameter = device:parameter(k)
    local was, why = value_now(song, env, state, record.layout)
    if was == nil then
      fail(why == "unknown" and "stale" or "gone", string.format("the send track %s's %s named before Kumi set it %s, so Kumi can't put it back: nothing was written",
        q(label), parameter.name, why == "gone" and "is gone" or "moved in a way Kumi couldn't follow"))
    end
    if not base.close(parameter.value, was) then
      local set = value_now(song, env, applied_params[key], record.layout)
      if set == nil or not base.close(parameter.value, set) then moved[#moved + 1] = parameter.name end
      writes[#writes + 1] = { parameter = parameter, value = was, k = k }
    end
  end
  local dest_writes = {}
  for _, prefix in ipairs(prefixes) do
    local place = prior_dests[prefix]
    local destination
    for _, candidate in ipairs(material_.destinations(device)) do
      if candidate.prefix == prefix then destination = candidate end
    end
    if not destination then fail("changed", string.format("%s isn't the device Kumi set any more", q(label))) end
    local was, why = dest.numbers_of_place(song, env, place, record.layout)
    if not was then
      fail(why == "unknown" and "stale" or "gone", string.format("the device %s drove before Kumi set it %s, so Kumi can't aim it back: nothing was written",
        q(label), why == "gone" and "is gone" or "moved in a way Kumi couldn't follow"))
    end
    local now = dest.numbers_of(destination)
    if not dest.same_numbers(now, was) then
      local set = type(applied_dests[prefix]) == "table" and dest.numbers_of_place(song, env, applied_dests[prefix], record.layout) or nil
      if not (set and dest.same_numbers(now, set)) then moved[#moved + 1] = prefix .. "Track" end
      dest_writes[#dest_writes + 1] = { destination = destination, numbers = was }
    end
  end
  if #writes == 0 and #dest_writes == 0 then
    return { restored = { count = count }, display = { title = title(device, t, track, count) .. " as they were" }, undo = { desc = desc, entry = false }, unwritten = true }
  end
  if #moved > 0 and not record.discard then
    fail("changed", string.format("%s of %s changed in Renoise since Kumi set them", table.concat(moved, ", "), q(label)))
  end
  song:describe_undo(desc)
  if env.watch_param then
    for _, w in ipairs(writes) do env.watch_param(t, d, w.k) end
  end
  for _, w in ipairs(writes) do w.parameter.value = w.value end
  for _, w in ipairs(dest_writes) do dest.write_numbers(w.destination, w.numbers) end
  local kept = {}
  for _, w in ipairs(writes) do
    if not base.close(w.parameter.value, w.value) then kept[#kept + 1] = w.parameter.name end
  end
  if #kept > 0 then fail("failed", string.format("Renoise kept %s of %s as they were", table.concat(kept, ", "), q(label))) end
  base.note_restored_at(env.journal, "device-params", { track = t, device = d })
  local lines = {}
  for _, w in ipairs(writes) do lines[#lines + 1] = w.parameter.name .. " → " .. base.minus_text(w.parameter.value_string) end
  for _, w in ipairs(dest_writes) do lines[#lines + 1] = w.destination.prefix .. "→ " .. dest.target_words(song, w.numbers) end
  return {
    restored = { count = #writes + #dest_writes }, display = { title = title(device, t, track, count) .. " put back", lines = lines },
    undo = { desc = desc, entry = true },
  }
end

base.register({
  kind = "device-params", custom = true, places = true,
  preview = preview, apply = apply, restore = restore,
  reread = presets.reread, read = raw_of, same = same_raw,
})

return {}
