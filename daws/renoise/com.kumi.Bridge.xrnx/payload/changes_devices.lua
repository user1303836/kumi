-- Kumi's changes to the devices on a track: bypass and parameter values (and, below, putting a device in, taking it out and
-- moving it).

local import = ...
local refs = import("refs")
local rows = import("rows")
local base = import("changes_base")
local material_ = import("changes_material")
local json = import("json")

local bad, fail = base.bad, base.fail
local q = base.q
local M = {}

-- A device is found by its ref (`E:device:track.device`), a parameter by `E:param:track.device.parameter`; both are looked at
-- again by their place. The track's first device is its mixer device, which is the track's fader and pan: not changed here.
-- A device's name as the producer sees it: the name they gave it, or the device's own.
function M.label(device)
  return device.display_name ~= "" and device.display_name or device.name
end

local function device_of(song, env, reference, what, count)
  if type(reference) ~= "string" then bad(what .. ": ref must be a " .. (count == 3 and "parameter" or "device") .. " ref") end
  local numbers = refs.expect(reference, count == 3 and "param" or "device", count, env.epoch())
  local index, at = numbers[1], numbers[2]
  local track = base.track_at(song, index)
  if at < 1 or at > #track.devices then fail("gone", "that device isn't on the track any more") end
  if at == 1 then
    if what == "device-delete" or what == "device-move" then
      bad("a track's first device is its mixer device (its fader and pan): it stays where it is")
    end
    bad("a track's first device is its mixer device: change its volume and pan with track-volume and track-pan")
  end
  local device = track:device(at)
  return index, at, track, device, M.label(device) .. " on " .. base.named(index, track), numbers[3]
end

local function device_target(env, index, at)
  return { ref = refs.make(env.epoch(), "device", index, at), kind = "device" }
end

-- (kumi-51: a device change's undo finds the device by its place, kept in `material`, so a device made, deleted or moved
-- since on any track doesn't make it stale; a record without one is found by its ref)
local function device_kept(env, index, at, device)
  return { track = index, dev = material_.device_place(env, index, at, device), label = M.label(device) }
end

M.device_kept = device_kept

local function kept_input(restore)
  local material = restore.material
  if type(material) == "table" and type(material.dev) == "table" then return { material = material } end
  return nil
end

local function device_prop(spec)
  spec.locate = function(song, env, input)
    local index, at, track, device, name
    if input.material then
      index, at, track, device = material_.device_by_place(song, env, input.material, "Kumi changed it")
      name = M.label(device) .. " on " .. base.named(index, track)
    else
      index, at, track, device, name = device_of(song, env, input.ref, spec.kind, 2)
    end
    return {
      object = device, device = device, name = name, place = { track = index, device = at }, target = device_target(env, index, at),
      id = base.track_id(track) .. "|" .. device.device_path,
    }
  end
  spec.material = function(env, loc) return device_kept(env, loc.place.track, loc.place.device, loc.device) end
  spec.restore_input, spec.by_place = kept_input, true
  spec.reread = function(song, place)
    if place.track > rows.track_count(song) then return nil end
    local track = song:track(place.track)
    if place.device > #track.devices then return nil end
    return track:device(place.device)
  end
  base.register(spec)
end

device_prop {
  kind = "device-bypass", noun = "bypass", undoable = true,
  read = function(device) return { bypass = not device.is_active } end,
  wanted = function(_, input)
    if type(input.bypass) ~= "boolean" then bad("bypass must be true or false") end
    return { bypass = input.bypass }
  end,
  write = function(device, state) device.is_active = not state.bypass end,
  same = function(a, b) return a.bypass == b.bypass end,
  title = function(name, _, to) return name .. (to.bypass and " bypassed" or " switched on") end,
}

-- A parameter's value is in its own range; text is parsed by Renoise. A Receiver of the #Send device is a send track's number.
local function param_state(parameter)
  return { value = parameter.value, valueString = parameter.value_string }
end

-- A parameter whose value is a place: a #Send's or #Multiband Send's Receiver (a send track, counted among the sends from
-- 0) and a meta device's or #Sidechain's track (Dest. Track, *Hydra's OutN Track and the like: a track counted from 0).
-- Renoise moves these values with the tracks made and deleted, so a record keeps the track the value names as a track ref,
-- which the layout carries; the value is worked out from it again.
local function points_at(device, parameter)
  local path, name = device.device_path, parameter.name
  if (path:find("#Send", 1, true) or path:find("#Multiband Send", 1, true)) and name:match("^Receiver") then return "send" end
  if (path:find("/Native/*", 1, true) or path:find("/Native/#Sidechain", 1, true)) and name:match("Track$") then return "track" end
  return nil
end

-- A Receiver (any of #Send's and #Multiband Send's) is a send track among the sends, from 0: Renoise takes any number
-- silently, so a number past the sends, or -1, is refused here.
local function receiver_check(device, parameter, value, song)
  if points_at(device, parameter) == "send" then
    local sends = song.send_track_count
    if value ~= math.floor(value) or value < 0 or value >= sends then
      bad(string.format("%s is a send track's number from 0 to %d (there are %d send tracks)", parameter.name, sends - 1, sends))
    end
  end
end

local function master_of(song) return song.sequencer_track_count + 1 end

local function track_of_value(song, points, value)
  if type(value) ~= "number" or value < 0 or value ~= math.floor(value) then return nil end
  local n = points == "send" and master_of(song) + 1 + value or value + 1
  if n < 1 or n > rows.track_count(song) then return nil end
  return n
end

local function value_of_track(song, points, n)
  return points == "send" and n - master_of(song) - 1 or n - 1
end

local param_spec = {
  kind = "device-param", noun = "parameter", undoable = true,
  wire = function(state, env, loc)
    if not (loc and loc.points) or type(state) ~= "table" then return state end
    local n = track_of_value(renoise.song(), loc.points, state.value)
    if not n then return state end
    local out = {}
    for k, v in pairs(state) do out[k] = v end
    out.at = refs.make(env.epoch(), "track", n)
    return out
  end,
  unwire = function(state, env, loc)
    if type(state) ~= "table" or type(state.at) ~= "string" or not (loc and loc.points) then return state end
    -- (carried to where that track is now; a ref of another epoch is `stale`)
    local n = refs.expect(state.at, "track", 1, env.epoch())[1]
    local out = {}
    for k, v in pairs(state) do out[k] = v end
    out.at, out.value, out.valueString = nil, value_of_track(renoise.song(), loc.points, n), nil
    return out
  end,
  read = function(parameter) return param_state(parameter) end,
  wanted = function(parameter, input)
    if type(input.value) == "number" then
      if input.value ~= input.value or input.value < parameter.value_min or input.value > parameter.value_max then
        bad(string.format("value must be from %s to %s", tostring(parameter.value_min), tostring(parameter.value_max)))
      end
      return { value = input.value }
    elseif type(input.valueString) == "string" then
      return { valueString = input.valueString }
    end
    bad("value (a number in the parameter's own range) or valueString (text as Renoise shows it)")
  end,
  write = function(parameter, state)
    if state.value ~= nil then
      parameter.value = state.value
    else
      parameter.value_string = state.valueString
    end
  end,
  -- (text is read by Renoise, which answers the value in its own words: "0.5" is read back as "0.500 dB", "-3 dB" as "-3.00 dB".
  -- Text matches what it was read as when it begins that, or has the same number to the digits shown and the same rest)
  same = function(a, b)
    if a.value ~= nil and b.value ~= nil then return base.close(a.value, b.value) end
    local read, wanted = a, b
    if a.value == nil then read, wanted = b, a end
    if read.value == nil or wanted.valueString == nil then return a.valueString == b.valueString end
    local function plain(text) return (tostring(text):lower():gsub("\226\136\146", "-"):gsub("^%s+", ""):gsub("%s+$", ""):gsub("%s+", " ")) end
    local text, asked = plain(read.valueString), plain(wanted.valueString)
    if text == asked or text:sub(1, #asked) == asked then return true end
    local n1, rest1 = text:match("^([%-+]?%d*%.?%d+)%s*(.*)$")
    local n2, rest2 = asked:match("^([%-+]?%d*%.?%d+)%s*(.*)$")
    if not n1 or not n2 or rest1 ~= rest2 then return false end
    local digits = #(n1:match("%.(%d+)$") or "")
    return math.abs(tonumber(n1) - tonumber(n2)) <= 0.5 * 10 ^ -digits + 1e-9
  end,
  title = function(name, _, to) return name .. " " .. base.minus_text(to.valueString or tostring(to.value)) end,
}

-- A value that is a place may be given as text, read as Renoise 3.5.4 reads it (lab, 2026-10-19): the number it begins with
-- ("3: Hats" is 3), a track counted from 1 (Dest. Track's "2" is the value 1) and a send from 0 (a Receiver's text is its
-- value). It is fenced as the number is. Text without a number Renoise ignores, and "-1" or "None" would set the first entry,
-- not none: Kumi refuses those.
param_spec.wanted_at = function(loc, input)
  -- (None, -1, is a track parameter's place value too, though its minimum is 0: Renoise takes it as a value, not as text;
  -- a Receiver has no None)
  if loc.points == "track" and input.value == -1 then return { value = -1 } end
  if loc.points and type(input.valueString) == "string" then
    local n = tonumber(input.valueString:match("^%s*(%d+)"))
    local first = loc.points == "send" and 0 or 1
    if not n or n < first then
      bad(string.format("%s names a %s: give its number (%s) as text, or value", loc.name, loc.points,
        loc.points == "send" and "a send among the sends, from 0" or "a track, from 1"))
    end
    return param_spec.wanted(loc.object, { value = n - first })
  end
  return param_spec.wanted(loc.object, input)
end

-- A device's parameter names, read from Renoise once: each by number, lowercased, and the numbers of each exact name. A request
-- that names many parameters reads them once (a plug-in has thousands), not once a name and pass.
local function parameter_names(device)
  local list, lowered, exact = {}, {}, {}
  for k = 1, #device.parameters do
    local name = device:parameter(k).name
    list[k], lowered[k] = name, name:lower()
    exact[lowered[k]] = exact[lowered[k]] or {}
    table.insert(exact[lowered[k]], k)
  end
  return { list = list, lowered = lowered, exact = exact }
end

-- Which parameter of a device a request means: its number from 1, or its name (any case). `names`: the device's, read once
-- (parameter_names) when a request names several.
local function parameter_number(device, wanted, names)
  local count = #device.parameters
  if type(wanted) == "number" then
    if wanted ~= math.floor(wanted) or wanted < 1 or wanted > count then bad(string.format("param is a number from 1 to %d", count)) end
    return wanted
  end
  if type(wanted) ~= "string" or wanted == "" then bad("param: a parameter's number (from 1) or its name, with a device ref") end
  names = names or parameter_names(device)
  -- (the name as written, or else the start of one name: EQ 5 names its gains "Gain 1 (100 Hz)", which follow the frequency)
  local lower = wanted:lower()
  local hits = names.exact[lower]
  if hits then
    if #hits > 1 then bad(string.format("%q names more than one parameter of this device: give its number", wanted)) end
    return hits[1]
  end
  local found
  for k, name in ipairs(names.lowered) do
    if name:sub(1, #lower) == lower then
      if found then bad(string.format("%q names more than one parameter of this device: give its number", wanted)) end
      found = k
    end
  end
  if found then return found end
  local shown = {}
  for k = 1, math.min(count, 24) do shown[k] = names.list[k] end
  bad(string.format("this device has no parameter %q (it has %s%s)", wanted, table.concat(shown, ", "), count > 24 and ", ..." or ""))
end

-- A parameter is named by its ref, or by its device's ref and `param`.
param_spec.locate = function(song, env, input)
  local parsed = type(input.ref) == "string" and refs.parse(input.ref) or nil
  local index, at, track, device, name, k
  if input.material then
    index, at, track, device = material_.device_by_place(song, env, input.material, "Kumi changed it")
    name = M.label(device) .. " on " .. base.named(index, track)
    k = input.material.param
    if type(k) ~= "number" or k > #device.parameters then
      fail("changed", string.format("%s has fewer parameters than when Kumi changed it", base.q(name)))
    end
  elseif parsed and parsed.kind == "device" then
    index, at, track, device, name = device_of(song, env, input.ref, param_spec.kind, 2)
    k = parameter_number(device, input.param)
  else
    index, at, track, device, name, k = device_of(song, env, input.ref, param_spec.kind, 3)
  end
  if k < 1 or k > #device.parameters then fail("gone", "that parameter isn't on the device any more") end
  local parameter = device:parameter(k)
  if input.value ~= nil then receiver_check(device, parameter, input.value, song) end
  return {
    object = parameter, device = device, name = name .. " " .. parameter.name, place = { track = index, device = at, param = k },
    target = { ref = refs.make(env.epoch(), "param", index, at, k), kind = "param" },
    id = base.track_id(track) .. "|" .. device.device_path .. "|" .. k, points = points_at(device, parameter),
  }
end
param_spec.material = function(env, loc)
  local kept = device_kept(env, loc.place.track, loc.place.device, loc.device)
  kept.param = loc.place.param
  return kept
end
param_spec.restore_input, param_spec.by_place = kept_input, true
param_spec.reread = function(song, place)
  if place.track > rows.track_count(song) then return nil end
  local track = song:track(place.track)
  if place.device > #track.devices then return nil end
  local device = track:device(place.device)
  if place.param > #device.parameters then return nil end
  return device:parameter(place.param)
end
-- The producer's undo moves a parameter: asked of its observable, once.
param_spec.watch = function(env, loc) env.watch_param(loc.place.track, loc.place.device, loc.place.param) end
-- (HISTORY's picture: the parameter's name and range, which the title only says)
local function parameter_shown(loc)
  local parameter = loc.object
  return { parameter = parameter.name, min = parameter.value_min, max = parameter.value_max }
end
param_spec.shown = parameter_shown
base.register(param_spec)

-- (M4 n-devices: device-params, several parameters of one device, asks and checks them as device-param does)
M.param_spec, M.parameter_number, M.points_at, M.receiver_check = param_spec, parameter_number, points_at, receiver_check
M.parameter_names = parameter_names
M.parameter_shown = parameter_shown
M.track_of_value, M.value_of_track = track_of_value, value_of_track


-- Putting a device in, taking it out, moving it. These change the track's list of devices, so the epoch moves (the answer is in
-- the new one, and says where the device is: `created` for a device made, `material.track` for the track to find it on).

-- The device Renoise has for what was asked: its path, or its name when one device has it.
local function resolve_device(track, wanted)
  if type(wanted) ~= "string" or wanted == "" then bad("path: a device's path (as Renoise lists it) or its name") end
  local lower, named_, near = wanted:lower(), {}, {}
  for _, info in ipairs(track.available_device_infos) do
    if info.path == wanted then return info end
    local last = info.path:match("([^/]+)$") or info.path
    if info.path:lower() == lower or last:lower() == lower or (info.short_name or ""):lower() == lower then
      named_[#named_ + 1] = info
    elseif info.path:lower():find(lower, 1, true) and #near < 5 then
      near[#near + 1] = info.path
    end
  end
  if #named_ == 1 then return named_[1] end
  if #named_ > 1 then
    local paths = {}
    for i = 1, math.min(#named_, 5) do paths[i] = named_[i].path end
    bad(string.format("%q names %d devices: say which by its path (%s)", wanted, #named_, table.concat(paths, ", ")))
  end
  local hint = #near > 0 and (" Close: " .. table.concat(near, ", ") .. ".") or ""
  bad(string.format("Renoise has no device %q for this track.%s", wanted, hint))
end

local function short_name(info)
  local name = info.short_name
  if name == nil or name == "" then name = info.path:match("([^/]+)$") or info.path end
  return name
end

local function sends_check(song, receiver)
  local sends = song.send_track_count
  if type(receiver) ~= "number" or receiver ~= math.floor(receiver) or receiver < 0 or receiver >= sends then
    bad(string.format("receiver is a send track's number from 0 to %d (there are %d send tracks)", sends - 1, sends))
  end
end

-- The send a #Send aims at, by its number among the send tracks (from 0: the first send is 0): receiverRef (the send
-- track) or receiver (that number).
local function receiver_of(song, env, input)
  if input.receiverRef == nil then return input.receiver end
  if input.receiver ~= nil then bad("give receiverRef (the send track) or receiver, not both") end
  local index, track = base.find(song, env, input.receiverRef, "receiverRef")
  if rows.track_type(track) ~= "send" then bad(string.format("receiverRef must be a send track; %s isn't one", q(base.named(index, track)))) end
  return index - song.sequencer_track_count - 2
end

local function add_input(song, env, track, input)
  local info = resolve_device(track, input.path)
  local is_send = info.path:find("#Send", 1, true) ~= nil
  if (input.receiver ~= nil or input.receiverRef ~= nil or input.amount ~= nil) and not is_send then
    bad("receiverRef, receiver and amount are for the #Send device")
  end
  local receiver = receiver_of(song, env, input)
  if receiver ~= nil then sends_check(song, receiver) end
  -- (the #Send's Amount is 0 to 1: measured, 3.5.4. Checked before anything is written; apply checks the parameter's own range too)
  if input.amount ~= nil and (type(input.amount) ~= "number" or input.amount ~= input.amount or input.amount < 0 or input.amount > 1) then
    bad("amount must be a number from 0 to 1")
  end
  local last = #track.devices + 1
  local at = input.index
  if at == nil then
    at = last
  elseif type(at) ~= "number" or at ~= math.floor(at) or at < 2 or at > last then
    bad(string.format("index must be a whole number from 2 (after the mixer device) to %d", last))
  end
  return info, at, receiver
end

local function add_signature(index, track)
  return { name = base.track_id(track), index = index, devices = #track.devices }
end

local function add_preview(song, env, input)
  local index, track = base.find(song, env, input.ref, "device-add")
  local info, at, receiver = add_input(song, env, track, input)
  local now = add_signature(index, track)
  -- (the send the receiver names, as a track the layout carries: one made before it by the apply refuses `changed`)
  if receiver ~= nil then now.receiver = refs.make(env.epoch(), "track", song.sequencer_track_count + 2 + receiver) end
  local display = {
    title = string.format("Added %s to %s", q(short_name(info)), q(base.named(index, track))),
    before = { devices = now.devices }, after = { devices = now.devices + 1, path = info.path, index = at },
  }
  return { target = base.target(env, index), prior = { devices = now.devices }, fence = now, display = display }
end

local function parameter_named(device, name)
  for k = 1, #device.parameters do
    if device:parameter(k).name == name then return device:parameter(k) end
  end
  return nil
end

local function add_apply(song, env, input, fence, desc)
  local ok, index, track = pcall(base.find, song, env, input.ref, "device-add")
  if not ok then base.gone_is_changed(index) end
  local now = add_signature(index, track)
  if type(fence.receiver) == "string" then
    local send = refs.expect(fence.receiver, "track", 1, env.epoch())[1]
    if receiver_of(song, env, input) ~= send - song.sequencer_track_count - 2 then fail("changed", "the send the receiver names moved since the preview: read again") end
    now.receiver = fence.receiver
  end
  if not base.same_table(now, fence) then fail("changed", base.named(index, track) .. "'s devices changed since the preview") end
  local info, at, receiver = add_input(song, env, track, input)
  song:describe_undo(desc)
  local made, device = pcall(function() return track:insert_device_at(info.path, at) end)
  if not made or not device then
    local why = made and "" or ": " .. tostring(device):gsub("^[^%s:]*%.lua:%d+: ", "")
    fail("failed", "Renoise wouldn't make the device " .. info.path .. why)
  end
  -- The device is set up in this callback; if that can't be done it goes again, so a refused change leaves no device behind.
  local set, err = pcall(function()
    if receiver ~= nil then
      local parameter = parameter_named(device, "Receiver")
      if not parameter then error({ code = "failed", message = "the #Send device has no Receiver parameter in this Renoise" }, 0) end
      parameter.value = receiver
    end
    if input.amount ~= nil then
      local parameter = parameter_named(device, "Amount")
      if not parameter then error({ code = "failed", message = "the #Send device has no Amount parameter in this Renoise" }, 0) end
      if input.amount < parameter.value_min or input.amount > parameter.value_max then
        error({ code = "bad-params", message = string.format("amount must be from %s to %s", tostring(parameter.value_min), tostring(parameter.value_max)) }, 0)
      end
      parameter.value = input.amount
    end
  end)
  if not set then
    pcall(function() track:delete_device_at(at) end)
    if type(err) ~= "table" then err = { code = "failed", message = "Renoise wouldn't set up the device: " .. tostring(err) } end
    error(err, 0)
  end
  local state = material_.state_at(song, index, at)
  return {
    -- (a Doofer's or a Splitter's devices inside, a Notepad's text, a plug-in's state: what its parameters don't show)
    applied = { index = at, path = info.path, state = state, content = material_.content_at(song, index, at) },
    created = { ref = refs.make(env.epoch(), "device", index, at), kind = "device" },
    target = { ref = refs.make(env.epoch(), "device", index, at), kind = "device" },
    -- (where its undo finds it: its place, followed through the devices made, deleted and moved since)
    material = device_kept(env, index, at, device),
    display = {
      title = string.format("Added %s to %s", q(short_name(info)), q(base.named(index, track))),
      before = { devices = now.devices }, after = { devices = #track.devices, path = info.path, index = at },
    },
    undo = { desc = desc, entry = true },
    prior = { devices = now.devices }, place = { track = index, device = at }, shape = "made",
  }
end

local function add_restore(song, env, restore, desc)
  local index, at, track, device
  if kept_input(restore) then
    index, at, track, device = material_.device_by_place(song, env, restore.material, "Kumi made it")
  else
    local created = restore.created
    if type(created) ~= "table" or type(created.ref) ~= "string" then bad("created: the device the change made") end
    local numbers = refs.expect(created.ref, "device", 2, env.epoch())
    index, at = numbers[1], numbers[2]
    track = base.track_at(song, index)
    if at < 2 or at > #track.devices then fail("gone", "the device isn't on the track any more") end
    device = track:device(at)
  end
  local applied = restore.applied
  if type(applied) ~= "table" then bad("applied: what the change wrote") end
  local now = material_.state_at(song, index, at)
  local title = (device.display_name ~= "" and device.display_name or device.name) .. " on " .. base.named(index, track)
  if not restore.discard and (now.path ~= applied.path or now.state ~= (applied.state or {}).state or now.active ~= (applied.state or {}).active) then
    fail("changed", title .. " changed in Renoise since Kumi made it")
  end
  if not restore.discard and applied.content ~= nil and material_.content_at(song, index, at) ~= applied.content then
    fail("changed", title .. " changed in Renoise since Kumi made it (what's inside it, its text or its state)")
  end
  local device_name, track_name = M.label(device), base.named(index, track)
  song:describe_undo(desc)
  track:delete_device_at(at)
  base.note_restored(env.journal, "device-add", { track = index, device = at })
  return {
    restored = { deleted = true },
    display = { title = string.format("Removed %s from %s (put back)", q(device_name), q(track_name)) },
    undo = { desc = desc, entry = true },
  }
end

base.register({
  kind = "device-add", custom = true, shape = "made", by_place = true,
  preview = add_preview, apply = add_apply, restore = add_restore,
})

-- Deleting a device keeps its path, name, bypass, preset data and automation to make it again.
local function delete_signature(song, index, track, at)
  local state = material_.state_at(song, index, at)
  state.index = at
  state.track = base.track_id(track) -- (the device on another track at that place is not this one)
  return state
end

local function delete_preview(song, env, input)
  local index, at, track, device, name = device_of(song, env, input.ref, "device-delete", 2)
  local state = delete_signature(song, index, track, at)
  local display = {
    title = string.format("Deleted %s from %s", q(M.label(device)), q(base.named(index, track))),
    before = { devices = #track.devices }, after = { devices = #track.devices - 1 },
  }
  display.note = "Kumi keeps its settings (and any automation) to put back."
  return { target = device_target(env, index, at), prior = { deleted = false }, fence = state, display = display }
end

local function delete_apply(song, env, input, fence, desc)
  local ok, index, at, track, device, name = pcall(device_of, song, env, input.ref, "device-delete", 2)
  if not ok then base.gone_is_changed(index) end
  local state = delete_signature(song, index, track, at)
  if not base.same_table(state, fence) then fail("changed", name .. " changed since the preview") end
  local automation = material_.read_automation(song, index, track, at)
  local patterns = {}
  for _, saved in ipairs(automation) do patterns[saved.pattern] = true end
  local material = {
    -- (its place: where it goes back is after the device before it, followed through the track's devices since; see places.lua)
    track = index, index = at, after = env.device_mark and env.device_mark(index) or nil,
    device = material_.read_device(device, song, index, false, env), state = state,
    automation = automation, pool = material_.keep_pool(song, env, patterns),
    routes = material_.read_routes(song, index, at, env),
  }
  local size = #json.encode(material)
  if size > base.MAX_MATERIAL_BYTES then
    fail("failed", string.format("%s holds %.1f MB, more than Kumi can keep to put it back, so it won't delete it", name, size / 1048576))
  end
  local title = string.format("Deleted %s from %s", q(M.label(device)), q(base.named(index, track)))
  song:describe_undo(desc)
  track:delete_device_at(at)
  -- (the mark just after it went: when Kumi puts it back, the places kept of it before follow it there)
  material.removed = env.device_mark and env.device_mark(index) or nil
  return {
    applied = { deleted = true }, material = material,
    display = { title = title, note = material_.routes_note(#material.routes) }, target = device_target(env, index, at),
    undo = { desc = desc, entry = true },
    prior = { deleted = false }, place = { track = index, device = at }, shape = "deleted",
  }
end

-- Where a track numbered in layout `layout` is now (as numbered then when the layout isn't known).
function M.carried_from(env, layout)
  return function(n)
    if type(layout) ~= "number" then return n end
    return (env.shifted(n, layout))
  end
end

-- Puts a kept device (`kept`: {device (read_device), automation, routes}) back at `at` on track `index`: the device, where
-- it sent its values, its automation in the patterns that still are (`pattern_now`), and what aimed at it. Counts into
-- `tally`; returns the device and the patterns its automation can't go back to. When Renoise refuses, the device goes
-- again and the error is raised.
function M.put_back(song, env, index, track, at, kept, pattern_now, moved, tally)
  local saved = kept.device
  local device = material_.restore_device(track, at, saved)
  local lost = 0
  local ok, err = pcall(function()
    for _, status in ipairs(material_.restore_destinations(song, device, saved, index, moved, env)) do material_.count(tally, status) end
    local list = {}
    for _, entry in ipairs(kept.automation or {}) do
      local p = pattern_now(entry.pattern)
      if p then
        list[#list + 1] = { pattern = p, device = at, parameter = entry.parameter, playmode = entry.playmode, points = entry.points }
      else
        lost = lost + 1
      end
    end
    material_.write_automation(song, track, index, list, nil)
  end)
  if ok then material_.write_routes(song, kept.routes, { on = index, device = at }, moved, tally, env) end
  if not ok then
    pcall(function() track:delete_device_at(at) end)
    if type(err) ~= "table" then err = { code = "failed", message = "Renoise wouldn't put the device back: " .. tostring(err) } end
    error(err, 0)
  end
  return device, lost
end

local function delete_restore(song, env, restore, desc)
  local material = restore.material
  if type(material) ~= "table" or type(material.track) ~= "number" or type(material.device) ~= "table" then
    bad("material: what the delete removed")
  end
  local index = material.track
  local track = base.track_at(song, index)
  local saved = material.device
  local label = (saved.display ~= nil and saved.display ~= "" and saved.display or saved.name or saved.path)
  local track_name = base.named(index, track)
  -- Where it goes back: after the device that was before it, followed through the devices made, deleted and moved on the track
  -- since. When that can't be told, nothing is written.
  local at = type(material.index) == "number" and env.device_after and env.device_after(index, material.index, material.after) or nil
  if not at then
    fail("stale", string.format("the devices on %s changed in a way Kumi couldn't follow since it deleted %s, so it can't tell where it goes: nothing was put back", q(track_name), q(label)))
  end
  at = math.max(2, math.min(at or 2, #track.devices + 1))
  -- It is back already (the producer's undo, or an earlier restore): nothing to write.
  local present = at <= #track.devices and track:device(at) or nil
  if present and present.device_path == saved.path then
    local state = material_.state_at(song, index, at)
    if state.state == (material.state or {}).state and state.display == (material.state or {}).display then
      return { restored = { deleted = false }, display = { title = string.format("%s is back on %s already", q(label), q(track_name)) }, undo = { desc = desc, entry = false }, unwritten = true }
    end
  end
  local pattern_now = material_.pool_now(song, env, material.pool)
  if not pattern_now then
    fail("stale", string.format("the song's patterns changed in a way Kumi couldn't follow since it deleted %s, so it can't tell where its automation goes: nothing was put back", q(label)))
  end
  song:describe_undo(desc)
  local tally = material_.new_tally()
  local device, lost = M.put_back(song, env, index, track, at, material, pattern_now, M.carried_from(env, restore.layout), tally)
  if env.device_back and type(material.removed) == "string" then env.device_back(index, material.removed, at) end
  base.note_restored(env.journal, "device-delete", { track = index, device = at })
  return {
    restored = { ref = refs.make(env.epoch(), "device", index, at), name = device.display_name },
    display = {
      title = string.format("%s put back on %s", q(label), q(track_name)),
      note = (function()
        local note = material_.tally_note(tally)
        if lost == 0 then return note end
        local more = string.format("Its automation in %s isn't put back: %s deleted or changed in Renoise since.",
          lost == 1 and "1 pattern" or (lost .. " patterns"), lost == 1 and "that pattern was" or "those patterns were")
        return note and (note .. " " .. more) or more
      end)(),
    },
    undo = { desc = desc, entry = true },
  }
end

base.register({
  kind = "device-delete", custom = true, shape = "deleted", by_place = true,
  preview = delete_preview, apply = delete_apply, restore = delete_restore,
})

-- Moving a device: swaps with its neighbours until it is where it was asked to go (`index` is the place it ends at).
local function move_wanted(track, input, at)
  local to = input.index
  if type(to) ~= "number" or to ~= math.floor(to) or to < 2 or to > #track.devices then
    bad(string.format("index must be a whole number from 2 (after the mixer device) to %d", #track.devices))
  end
  return to
end

local function move_preview(song, env, input)
  local index, at, track, device, name = device_of(song, env, input.ref, "device-move", 2)
  local to = move_wanted(track, input, at)
  local state = delete_signature(song, index, track, at)
  local display = {
    title = string.format("Moved %s on %s to place %d", q(M.label(device)), q(base.named(index, track)), to),
    before = { index = at }, after = { index = to },
  }
  if to == at then display.unchanged = true end
  return { target = device_target(env, index, at), prior = { index = at }, fence = state, display = display }
end

local function move_apply(song, env, input, fence, desc)
  local ok, index, at, track, device, name = pcall(device_of, song, env, input.ref, "device-move", 2)
  if not ok then base.gone_is_changed(index) end
  local state = delete_signature(song, index, track, at)
  if not base.same_table(state, fence) then fail("changed", name .. " changed since the preview") end
  local to = move_wanted(track, input, at)
  local display = {
    title = string.format("Moved %s on %s to place %d", q(M.label(device)), q(base.named(index, track)), to),
    before = { index = at }, after = { index = to },
  }
  -- (where its undo finds it and where it goes back, as places followed through the devices made, deleted and moved
  -- since: the device, the one it followed before (`was`), the one it follows now (`now`); the mixer device is `mixer`)
  local function neighbour(n)
    if n <= 1 then return { mixer = true } end
    return device_kept(env, index, n, track:device(n))
  end
  if to == at then
    display.unchanged = true
    local material = device_kept(env, index, at, device)
    material.was, material.now = neighbour(at - 1), neighbour(at - 1)
    return {
      applied = { index = to, state = state }, material = material, display = display, undo = { desc = desc, entry = false },
      prior = { index = at },
    }
  end
  song:describe_undo(desc)
  local step = to > at and 1 or -1
  for i = at, to - step, step do track:swap_devices_at(i, i + step) end
  local after = material_.state_at(song, index, to)
  if after.state ~= state.state or after.path ~= state.path then fail("failed", "Renoise didn't move " .. name) end
  local material = device_kept(env, index, to, track:device(to))
  material.was, material.now = neighbour(to > at and at - 1 or at), neighbour(to - 1)
  return {
    applied = { index = to, state = delete_signature(song, index, track, to) }, material = material, display = display,
    target = device_target(env, index, to),
    undo = { desc = desc, entry = true },
    prior = { index = at }, place = { track = index, device = to }, follows = "device",
  }
end

-- Where a kept neighbour is now (the mixer device is always 1), or nil and why.
local function neighbour_now(song, env, t, kept)
  if type(kept) ~= "table" then return nil, "unknown" end
  if kept.mixer then return 1 end
  return material_.device_now(song, env, t, kept.dev)
end

local function move_restore(song, env, restore, desc)
  local material = restore.material
  if type(material) ~= "table" or type(material.dev) ~= "table" then bad("material: where the device and its neighbours were") end
  local index, d, track, device = material_.device_by_place(song, env, material, "Kumi moved it")
  local label = q(M.label(device)) .. " on " .. q(base.named(index, track))
  local was, why = neighbour_now(song, env, index, material.was)
  if not was then
    local whose = q((material.was or {}).label or "the device")
    fail(why == "unknown" and "stale" or "changed", string.format("%s, which %s followed before Kumi moved it, %s, so Kumi can't tell where it goes back: nothing was moved",
      whose, label, why == "gone" and "was deleted" or why == "changed" and "isn't at its place any more" or "moved in a way Kumi couldn't follow"))
  end
  if was == d - 1 then
    return { restored = { index = d }, display = { title = label .. " is back after the device it followed already" }, undo = { desc = desc, entry = false }, unwritten = true }
  end
  local now = neighbour_now(song, env, index, material.now)
  if now ~= d - 1 and not restore.discard then
    fail("changed", string.format("%s was moved in Renoise since Kumi moved it", label))
  end
  -- (after the device it followed: one place on when that device is before it, its place when it is after)
  local to = was < d and was + 1 or was
  song:describe_undo(desc)
  local step = to > d and 1 or -1
  for i = d, to - step, step do track:swap_devices_at(i, i + step) end
  base.note_restored(env.journal, "device-move", { track = index, device = to }, restore.applied, base.KINDS["device-move"])
  return {
    restored = { index = to }, display = { title = string.format("Moved %s back to place %d", label, to) },
    undo = { desc = desc, entry = true },
  }
end

base.register({
  kind = "device-move", custom = true, by_place = true,
  preview = move_preview, apply = move_apply, restore = move_restore,
  same = function(a, b) return a.index == b.index end,
})

return M
