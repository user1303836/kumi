-- Kumi's changes to a device's settings (M4): a preset chosen, preset data written, the device's name and whether it shows
-- maximized; and the Doofer, Splitter and Notepad edits Kumi's host composes as preset data (crates/daw-mcp-server/src/
-- presets.rs), which this tool only writes. A device is found by its ref when a change is made, and by its place when the
-- change is put back: its track carried through the layout (the number under `track`), the device followed through that
-- track's device log (places.lua). So an undo outlives the devices made, deleted and moved since, here or elsewhere.
--
-- Renoise 3.5.4 (lab): preset data of another device type crashes Renoise, as does a Doofer macro mapping to chain -1; text
-- that isn't XML, an empty text and a newer document version are refused with an error and change nothing. Writing preset
-- data is one undo step; `active_preset` reads 0 after it.

local import = ...
local refs = import("refs")
local rows = import("rows")
local base = import("changes_base")
local material_ = import("changes_material")
local devices_ = import("changes_devices")
local vst3_ = import("vst3_presets")
local json_null = import("json").null
local json_encode = import("json").encode
local json_array = import("json").array

local bad, fail, q = base.bad, base.fail, base.q
local M = {}

-- The most preset data a change writes or keeps (a plug-in's state can be megabytes).
M.MAX_PRESET_BYTES = base.MAX_MATERIAL_BYTES
-- The most parameters of one device watched for the producer's undo of a preset change.
M.MOST_WATCHED = 64

-- A text's print (changes_material.lua; Kumi's host computes the same: presets.rs `print_of`).
M.print_of = material_.print_of

-- Preset data with what moves by itself left out: parameter values (automation, meta devices and macros move them all the
-- time), plug-in state chunks (which follow the plug-in's parameters), and the inner devices' SelectedPresetIsModified and
-- IsSelected (Renoise 3.5.4 turns the first true when it writes a container's data again, and the second follows the
-- producer's clicks: lab, 2026-10-20). Two reads of a device nobody edited give the same shape. (presets.rs `shape_of` is
-- the same.) Kept by the text, with its other prints (changes_material.lua `prints_of`).
function M.shape_of(xml)
  local kept = material_.prints_of(xml)
  local printed = kept.shape
  if printed == nil then
    local shape = xml:gsub("<Value>[^<]*</Value>", "<Value/>"):gsub("<ParameterChunk>.-</ParameterChunk>", "<ParameterChunk/>")
    shape = shape:gsub("<SelectedPresetIsModified>[^<]*</SelectedPresetIsModified>", "<SelectedPresetIsModified/>")
    shape = shape:gsub("<IsSelected>[^<]*</IsSelected>", "<IsSelected/>")
    -- (and what Renoise's view shows of it: no change to what it holds)
    for _, tag in ipairs(material_.VIEW_STATE) do
      shape = shape:gsub("<" .. tag .. ">[^<]*</" .. tag .. ">", "<" .. tag .. "/>")
    end
    printed = material_.print_now(shape)
    kept.shape = printed
  end
  return printed
end

-- Preset data as it reads, values and all, but what moves by itself (changes_material.lua).
M.content_of = material_.content_of

-- What preset data says it is: its slot's device type, its document version, and a plug-in's identifier.
local function preset_kind(xml)
  if type(xml) ~= "string" then xml = "" end
  local slot = xml:match('<DeviceSlot%s+type="([%w_]+)"')
  return {
    slot = slot, version = tonumber(xml:match('<FilterDevicePreset%s+doc_version="(%d+)"')),
    -- (a plug-in's own identifier; a Doofer's inner plug-ins are no part of what the device is)
    plugin = slot == "AudioPluginDevice" and xml:match("<PluginIdentifier>([^<]*)</PluginIdentifier>") or nil,
  }
end

-- What in a Doofer's or Splitter's preset data Renoise 3.5.4 crashes on (n-devices lab): a routing device inside it, or a
-- macro mapping into a chain or a device the container hasn't (chains and devices from 0; a mapping to device -1 is none). Nil
-- when there is none, else the words. (presets.rs refuses the same in the edits it makes.)
local ROUTING = { SendDevice = true, SideChainDevice = true, CrossoverDevice = true, LineInDevice = true }
local CONTAINER_SLOTS = { DooferDevice = 1, SplitterDevice = 2 }

local function container_trouble(xml)
  local stack, containers = {}, {}
  for close, name, attributes, empty, text in xml:gmatch("<(/?)([%w_]+)([^>]-)(/?)>([^<]*)") do
    if close == "/" then
      stack[#stack] = nil
    else
      local frame = { name = name }
      local lanes = CONTAINER_SLOTS[name] or (name == "DeviceSlot" and CONTAINER_SLOTS[attributes:match('type="([%w_]+)"') or ""])
      if ROUTING[name] then return name .. " can't be inside a Doofer or Splitter (Renoise crashes on routing devices there)" end
      if lanes then
        frame.container = { lanes = lanes, counts = {}, mappings = {} }
        containers[#containers + 1] = frame.container
      end
      local parent, chain, owner = stack[#stack], stack[#stack - 1], stack[#stack - 2]
      -- (a device in one of the container's chains: DeviceChain, or a Splitter's DeviceChain0 and DeviceChain1)
      if parent and parent.name == "Devices" and chain and chain.name:match("^DeviceChain%d?$") and owner and owner.container then
        local lane = chain.name == "DeviceChain1" and 1 or 0
        owner.container.counts[lane] = (owner.container.counts[lane] or 0) + 1
      end
      -- (a mapping of one of the container's macros)
      if parent and parent.name == "Mappings" and chain and chain.name:match("^Macro%d+$") and owner and owner.container then
        frame.mapping = {}
        owner.container.mappings[#owner.container.mappings + 1] = frame.mapping
      end
      if parent and parent.mapping and (name == "DestChainIndex" or name == "DestDeviceIndex") then parent.mapping[name] = tonumber(text) end
      if empty ~= "/" then stack[#stack + 1] = frame end
    end
  end
  for _, container in ipairs(containers) do
    for _, mapping in ipairs(container.mappings) do
      local lane, device = mapping.DestChainIndex or 0, mapping.DestDeviceIndex or -1
      if lane < 0 or lane >= container.lanes then
        return string.format("a macro maps into chain %d, which the container hasn't (Renoise crashes on it)", lane)
      end
      local count = container.counts[lane] or 0
      if device < -1 or device >= count then
        return string.format("a macro maps to device %d of a chain of %d (from 0; Renoise crashes on it)", device, count)
      end
    end
  end
  return nil
end
M.container_trouble = container_trouble

-- Preset data that may go to this device: the same device type (another crashes Renoise), not a newer document, the same
-- plug-in, nothing a container crashes on, within the bound. Refused before anything is written. (`data`: the device's
-- preset data as the caller read it a moment ago, else it is read here: Renoise writes the whole device out on each read.)
local function check_preset_data(device, xml, label, data)
  if type(xml) ~= "string" or xml == "" then bad("presetData (the device's preset data, XML as Renoise gives it) or path (an .xrdp preset file)") end
  if #xml > M.MAX_PRESET_BYTES then
    bad(string.format("presetData is %.1f MB, more than Kumi writes (%d MB)", #xml / 1048576, M.MAX_PRESET_BYTES / 1048576))
  end
  local now, given = preset_kind(data or device.active_preset_data), preset_kind(xml)
  if not given.slot or not given.version then
    bad("presetData isn't a device preset (<FilterDevicePreset doc_version=…><DeviceSlot type=…>)")
  end
  if now.slot and given.slot ~= now.slot then
    bad(string.format("that preset data is for a %s, and %s is a %s: Renoise would crash on it", given.slot, q(label), now.slot))
  end
  if now.version and given.version > now.version then
    bad(string.format("that preset data is of a newer Renoise (document version %d; this one reads %d)", given.version, now.version))
  end
  if now.plugin and given.plugin ~= now.plugin then
    bad(string.format("that preset data is for the plug-in %s, and %s is %s", tostring(given.plugin), q(label), now.plugin))
  end
  if CONTAINER_SLOTS[given.slot] then
    local trouble = container_trouble(xml)
    if trouble then bad(string.format("that preset data won't go to %s: %s", q(label), trouble)) end
  end
end

-- The device a change names: its ref (a device, not the track's mixer device, which is its fader and pan).
local function device_by_ref(song, env, reference, what)
  if type(reference) ~= "string" then bad(what .. ": ref must be a device ref") end
  local numbers = refs.expect(reference, "device", 2, env.epoch())
  local t, d = numbers[1], numbers[2]
  local track = base.track_at(song, t)
  if d < 1 or d > #track.devices then fail("gone", "that device isn't on the track any more") end
  if d == 1 then bad("a track's first device is its mixer device: change its volume and pan with track-volume and track-pan") end
  return t, d, track, track:device(d)
end
M.device_by_ref = device_by_ref

-- The device a record keeps, where it is now (changes_material.lua).
local device_by_place = material_.device_by_place
M.device_by_place = device_by_place

local function label_of(t, track, device)
  return devices_.label(device) .. " on " .. base.named(t, track)
end

local function identity(track, device) return base.track_id(track) .. "|" .. device.device_path end

-- A Doofer's, a Splitter's, a Notepad's or a plug-in's whole content, beyond its parameters (changes_material.lua).
local content_fence = material_.content_fence

-- Where a record finds the device again, and what it was called then (for the words of a refusal).
local function keep(env, t, d, track, device)
  return { track = t, dev = material_.device_place(env, t, d, device), label = devices_.label(device) }
end
M.keep = keep

-- (a change's target: the device's track and place, as numbers: refs of devices go stale on any device change)
local function target_of(t, d) return { kind = "device", track = t, device = d } end

-- The producer's undo of a preset change moves the device's parameters: those are watched for it (the first ones).
local function watch_all(env, t, d, device)
  if not env.watch_param then return end
  for k = 1, math.min(#device.parameters, M.MOST_WATCHED) do env.watch_param(t, d, k) end
end

local function reread(song, place)
  if place.track > rows.track_count(song) then return nil end
  local track = song:track(place.track)
  if place.device > #track.devices then return nil end
  return track:device(place.device)
end

-- device-preset: one of the device's presets (its own or the user library's), by number or name. The parameters it replaced
-- are kept as preset data, and the undo writes them back.

local function preset_names(device)
  local names = {}
  for k = 1, #device.presets do names[k] = device:preset(k) end
  return names
end

local function preset_number(device, wanted)
  local names = preset_names(device)
  if #names == 0 then bad("this device has no presets") end
  if type(wanted) == "number" then
    if wanted ~= math.floor(wanted) or wanted < 1 or wanted > #names then bad(string.format("preset is a number from 1 to %d", #names)) end
    return wanted, names[wanted]
  end
  if type(wanted) ~= "string" or wanted == "" then bad("preset: a preset's number (from 1) or its name") end
  local lower = wanted:lower()
  for _, exact in ipairs({ true, false }) do
    local found
    for k, name in ipairs(names) do
      local n = name:lower()
      -- (a name as written, or its last part: "Dynamics/Hard Clipper" is "Hard Clipper" too, or the start of one)
      if (exact and (n == lower or n:match("([^/]+)$") == lower)) or (not exact and (n:sub(1, #lower) == lower or (n:match("([^/]+)$") or ""):sub(1, #lower) == lower)) then
        if found then bad(string.format("%q names more than one preset of this device: give its number", wanted)) end
        found = k
      end
    end
    if found then return found, names[found] end
  end
  local listed = {}
  for k = 1, math.min(#names, 24) do listed[k] = names[k] end
  bad(string.format("this device has no preset %q (it has %s%s)", wanted, table.concat(listed, ", "), #names > 24 and ", ..." or ""))
end

local function preset_name(device, n)
  if type(n) ~= "number" or n < 1 or n > #device.presets then return nil end
  return device:preset(n)
end

local function preset_preview(song, env, input)
  local t, d, track, device = device_by_ref(song, env, input.ref, "device-preset")
  local n, name = preset_number(device, input.preset)
  local xml = device.active_preset_data
  local label = label_of(t, track, device)
  return {
    target = target_of(t, d), prior = { preset = device.active_preset, print = M.print_of(xml) },
    fence = { id = identity(track, device), state = material_.device_state(device).state, preset = device.active_preset, content = content_fence(device, xml) },
    display = {
      title = string.format("%s: preset %s", label, q(name)),
      before = { preset = preset_name(device, device.active_preset) }, after = { preset = name },
      note = "Kumi keeps the settings it replaces to put back.",
    },
  }
end

local function preset_apply(song, env, input, fence, desc)
  local ok, t, d, track, device = pcall(device_by_ref, song, env, input.ref, "device-preset")
  if not ok then base.gone_is_changed(t) end
  local label = label_of(t, track, device)
  if fence.id ~= identity(track, device) then fail("changed", base.moved_text(label)) end
  local xml = device.active_preset_data
  if material_.device_state(device).state ~= fence.state or device.active_preset ~= fence.preset or content_fence(device, xml) ~= fence.content then
    fail("changed", label .. "'s settings changed since the preview")
  end
  local n, name = preset_number(device, input.preset)
  if #xml > M.MAX_PRESET_BYTES then
    fail("failed", string.format("%s holds %.1f MB of settings, more than Kumi keeps to put back, so it won't replace them", label, #xml / 1048576))
  end
  song:describe_undo(desc)
  watch_all(env, t, d, device)
  device.active_preset = n
  if device.active_preset ~= n then
    pcall(function() device.active_preset_data = xml end)
    fail("failed", "Renoise didn't take the preset " .. q(name))
  end
  local back = device.active_preset_data
  local printed = M.print_of(back)
  return {
    applied = { preset = n, state = material_.device_state(device).state, print = printed, content = content_fence(device, back) },
    material = (function()
      local kept = keep(env, t, d, track, device)
      kept.presetData = xml
      return kept
    end)(),
    target = target_of(t, d),
    display = { title = string.format("%s: preset %s", label, q(name)), before = { preset = preset_name(device, fence.preset) }, after = { preset = name } },
    undo = { desc = desc, entry = true },
    prior = { print = M.print_of(xml) }, journal_applied = { print = printed }, place = { track = t, device = d },
  }
end

-- Puts the kept settings back, if the device still has what the change left (`applied.state`).
local function settings_restore(kind, song, env, restore, desc, what)
  local material, applied, prior = restore.material, restore.applied, restore.prior
  if type(material) ~= "table" or type(material.presetData) ~= "string" then bad("material: the settings the change replaced") end
  if type(applied) ~= "table" or type(prior) ~= "table" then bad("prior and applied: the change's settings") end
  local t, d, track, device = device_by_place(song, env, material, "Kumi changed it")
  local label = label_of(t, track, device)
  local now = device.active_preset_data
  if M.print_of(now) == prior.print then
    return { restored = { preset = device.active_preset }, display = { title = label .. ": its settings are back already" }, undo = { desc = desc, entry = false }, unwritten = true }
  end
  if not restore.discard and material_.device_state(device).state ~= applied.state then
    fail("changed", string.format("%s's settings changed in Renoise since Kumi %s (automation and meta devices move them too)", label, what))
  end
  -- (a Doofer's or Splitter's devices inside, a Notepad's text, a plug-in's state: what its parameters don't show)
  if not restore.discard and applied.content ~= nil and content_fence(device, now) ~= applied.content then
    fail("changed", string.format("%s changed in Renoise since Kumi %s (what's inside it, its text or its state)", label, what))
  end
  song:describe_undo(desc)
  watch_all(env, t, d, device)
  local ok, err = pcall(function() device.active_preset_data = material.presetData end)
  if not ok then fail("failed", "Renoise wouldn't take the settings back: " .. tostring(err)) end
  -- (written settings leave no preset chosen: when the device played a preset whose parameters these are, it plays it again,
  -- in the same step; otherwise the written settings stay. A preset may differ in what isn't a parameter: EQ 5's "Init"
  -- shows 18 dB of gain where a new EQ 5 shows 20, lab 2026-10-20)
  local preset = prior.preset
  if type(preset) == "number" and preset >= 1 and preset <= #device.presets then
    local wanted = material_.device_state(device).state
    local chosen = pcall(function() device.active_preset = preset end)
    -- (a container's preset brings its own inside, a plug-in's its own state: unless that is what was kept, the kept data stays)
    local deep = applied.content ~= nil or material_.is_deep(device)
    if not chosen or material_.device_state(device).state ~= wanted or (deep and M.content_of(device.active_preset_data) ~= M.content_of(material.presetData)) then
      pcall(function() device.active_preset_data = material.presetData end)
    end
  end
  local after_content, prior_content = M.content_of(device.active_preset_data), M.content_of(material.presetData)
  if after_content == M.content_of(now) and after_content ~= prior_content then
    fail("failed", "the device kept the applied settings instead of restoring its earlier settings")
  end
  base.note_restored_at(env.journal, kind, { track = t, device = d })
  return {
    restored = { preset = device.active_preset }, display = { title = label .. ": settings put back",
      note = material.vst3 and after_content ~= prior_content
        and "The device accepted its earlier settings and normalized the saved bytes." or nil },
    undo = { desc = desc, entry = true },
  }
end

local function settings_read(device) return { print = M.print_of(device.active_preset_data) } end
local function same_print(a, b) return a.print == b.print end

base.register({
  kind = "device-preset", custom = true, places = true,
  preview = preset_preview, apply = preset_apply,
  restore = function(song, env, restore, desc) return settings_restore("device-preset", song, env, restore, desc, "chose the preset") end,
  reread = reread, read = settings_read, same = same_print,
})

-- device-preset-data: the device's preset data written whole (XML, as Renoise gives it and as a read shows it). Kumi's host
-- reads it from an .xrdp file in Renoise's library when the change names one (`presetName` then names it in the title).

local function data_title(input, label)
  if type(input.presetName) == "string" and input.presetName ~= "" then
    return string.format("Loaded the preset %s into %s", q(input.presetName), q(label))
  end
  return "Wrote the settings of " .. q(label)
end

local function data_preview(song, env, input)
  local t, d, track, device = device_by_ref(song, env, input.ref, "device-preset-data")
  local label = label_of(t, track, device)
  local xml = device.active_preset_data
  local wanted = input.vst3Data and vst3_.compose(device, xml, input) or input.presetData
  check_preset_data(device, wanted, label, xml)
  return {
    target = target_of(t, d), prior = { preset = device.active_preset, print = M.print_of(xml) },
    fence = { id = identity(track, device), state = material_.device_state(device).state, content = content_fence(device, xml) },
    display = {
      title = data_title(input, label), unchanged = xml == wanted or nil,
      note = "Kumi keeps the settings it replaces to put back.",
    },
  }
end

local function data_apply(song, env, input, fence, desc)
  local ok, t, d, track, device = pcall(device_by_ref, song, env, input.ref, "device-preset-data")
  if not ok then base.gone_is_changed(t) end
  local label = label_of(t, track, device)
  if fence.id ~= identity(track, device) then fail("changed", base.moved_text(label)) end
  local xml = device.active_preset_data
  if material_.device_state(device).state ~= fence.state or content_fence(device, xml) ~= fence.content then fail("changed", label .. "'s settings changed since the preview") end
  local wanted = input.vst3Data and vst3_.compose(device, xml, input) or input.presetData
  check_preset_data(device, wanted, label, xml)
  if #xml > M.MAX_PRESET_BYTES then
    fail("failed", string.format("%s holds %.1f MB of settings, more than Kumi keeps to put back, so it won't replace them", label, #xml / 1048576))
  end
  song:describe_undo(desc)
  watch_all(env, t, d, device)
  local written, err = pcall(function() device.active_preset_data = wanted end)
  -- XML parsing errors occur before writes. A plug-in may reject a later part of its VST3 state after accepting another,
  -- so a VST3 load failure restores the saved whole state before returning an error.
  if not written then
    local restored = not input.vst3Data or pcall(function() device.active_preset_data = xml end)
    fail("failed", "Renoise refused the preset data: " .. tostring(err):gsub("^[^%s:]*%.lua:%d+: ", "")
      .. (restored and "" or "; the previous state could not be restored"))
  end
  local back = device.active_preset_data
  local printed = M.print_of(back)
  local kept = keep(env, t, d, track, device)
  kept.presetData = xml
  kept.vst3 = input.vst3Data ~= nil
  return {
    applied = { state = material_.device_state(device).state, print = printed, content = content_fence(device, back) }, material = kept, target = target_of(t, d),
    display = { title = data_title(input, label) }, undo = { desc = desc, entry = true },
    prior = { print = M.print_of(xml) }, journal_applied = { print = printed }, place = { track = t, device = d },
  }
end

base.register({
  kind = "device-preset-data", custom = true, places = true,
  preview = data_preview, apply = data_apply,
  restore = function(song, env, restore, desc) return settings_restore("device-preset-data", song, env, restore, desc, "wrote them") end,
  reread = reread, read = settings_read, same = same_print,
})

-- device-set: the device's name in the chain (its display name; "" gives it back its own) and whether it shows maximized.

local function set_read(device, source)
  local out = {}
  if source == nil or source.name ~= nil then out.name = device.display_name end
  if source == nil or source.maximized ~= nil then out.maximized = device.is_maximized end
  return out
end

local function set_wanted(device, input)
  local out = {}
  if input.name ~= nil then
    if type(input.name) ~= "string" or #input.name > 256 or input.name:find("[%c]") then bad("name must be one line of text") end
    out.name = input.name
  end
  if input.maximized ~= nil then
    if type(input.maximized) ~= "boolean" then bad("maximized must be true or false") end
    out.maximized = input.maximized
  end
  if next(out) == nil then bad("give name or maximized") end
  return out
end

-- (a name given back as "" reads as the device's own name: either is what was asked)
local function set_same(device_name)
  return function(a, b)
    if (a.maximized == nil) ~= (b.maximized == nil) or a.maximized ~= b.maximized then return false end
    if (a.name == nil) ~= (b.name == nil) then return false end
    if a.name == b.name then return true end
    local function plain(n) return (n == "" or n == device_name) and "" or n end
    return device_name ~= nil and plain(a.name) == plain(b.name)
  end
end

local function set_title(label, track_name, from, to)
  local parts = {}
  if to.name ~= nil and to.name ~= from.name then
    local new = to.name == "" and "its own name" or q(to.name)
    parts[#parts + 1] = string.format("Renamed %s on %s → %s", q(label), q(track_name), new)
  end
  if to.maximized ~= nil and to.maximized ~= from.maximized then
    local word = to.maximized and "expanded" or "collapsed"
    if #parts == 0 then return string.format("%s on %s %s", label, track_name, word) end
    parts[#parts + 1] = word
  end
  if #parts == 0 then return string.format("%s on %s as it was", label, track_name) end
  return table.concat(parts, ", ")
end

local function set_write(device, state)
  if state.name ~= nil then device.display_name = state.name end
  if state.maximized ~= nil then device.is_maximized = state.maximized end
end

local function set_preview(song, env, input)
  local t, d, track, device = device_by_ref(song, env, input.ref, "device-set")
  local wanted = set_wanted(device, input)
  local now = set_read(device, wanted)
  local display = { title = set_title(devices_.label(device), base.named(t, track), now, wanted), before = now, after = wanted }
  if set_same(device.name)(now, wanted) then display.unchanged = true end
  local fence = set_read(device, wanted)
  fence.id = identity(track, device)
  return { target = target_of(t, d), prior = now, fence = fence, display = display }
end

local function set_apply(song, env, input, fence, desc)
  local ok, t, d, track, device = pcall(device_by_ref, song, env, input.ref, "device-set")
  if not ok then base.gone_is_changed(t) end
  local label, track_name = devices_.label(device), base.named(t, track)
  if fence.id ~= identity(track, device) then fail("changed", base.moved_text(label .. " on " .. track_name)) end
  local wanted = set_wanted(device, input)
  local now = set_read(device, wanted)
  local fenced = {}
  for k, v in pairs(fence) do if k ~= "id" then fenced[k] = v end end
  if not set_same(device.name)(now, fenced) then fail("changed", string.format("%s on %s changed since the preview", label, track_name)) end
  local same = set_same(device.name)
  if not same(now, wanted) then
    song:describe_undo(desc)
    set_write(device, wanted)
  end
  local after = set_read(device, wanted)
  if not same(after, wanted) then
    pcall(set_write, device, now)
    fail("failed", string.format("Renoise kept %s on %s as it was", label, track_name))
  end
  return {
    applied = after, material = keep(env, t, d, track, device), target = target_of(t, d),
    display = { title = set_title(label, track_name, now, after), before = now, after = after },
    undo = { desc = desc, entry = not same(now, wanted) }, prior = now, place = { track = t, device = d },
  }
end

local function set_restore(song, env, restore, desc)
  local prior, applied = restore.prior, restore.applied
  if type(prior) ~= "table" or type(applied) ~= "table" then bad("prior and applied: the name and look the change set") end
  local t, d, track, device = device_by_place(song, env, restore.material, "Kumi changed it")
  local label, track_name = devices_.label(device), base.named(t, track)
  local same = set_same(device.name)
  local now = set_read(device, prior)
  if same(now, prior) then
    return { restored = now, display = { title = set_title(label, track_name, prior, prior) .. " (back already)" }, undo = { desc = desc, entry = false }, unwritten = true }
  end
  if not restore.discard and not same(now, applied) then
    fail("changed", string.format("%s on %s was renamed or changed in Renoise since Kumi set it", label, track_name))
  end
  song:describe_undo(desc)
  set_write(device, prior)
  base.note_restored_at(env.journal, "device-set", { track = t, device = d })
  return {
    restored = set_read(device, prior), display = { title = set_title(label, track_name, now, prior) .. " (put back)" },
    undo = { desc = desc, entry = true },
  }
end

base.register({
  kind = "device-set", custom = true, places = true,
  preview = set_preview, apply = set_apply, restore = set_restore,
  reread = reread, read = function(device, source) return set_read(device, source) end,
  same = function(a, b) return set_same(nil)(a, b) end,
})

-- The Doofer, Splitter and Notepad edits Kumi's host composes (presets.rs): the host reads the device's preset data
-- (`preset_read`), edits it, and sends it whole with the shape it edited from; this writes it in one step if the device's
-- shape is still that (nothing structural changed in between), and puts the host's restore back the same way. The words
-- are the host's. Only containers (and the Notepad) take them, and only preset data of their own type.

local XML_KINDS = {
  { kind = "doofer-macro", devices = "container" },
  { kind = "doofer-device-add", devices = "container" },
  { kind = "doofer-device-delete", devices = "container" },
  { kind = "doofer-device-move", devices = "container" },
  { kind = "doofer-device-set", devices = "container" },
  { kind = "splitter-set", devices = "splitter" },
  { kind = "notepad-write", devices = "notepad" },
}

local CONTAINERS = { ["Audio/Effects/Native/Doofer"] = true, ["Audio/Effects/Native/Splitter"] = true }
local function takes(spec, device)
  local path = device.device_path
  if spec.devices == "container" then return CONTAINERS[path] == true end
  if spec.devices == "splitter" then return path == "Audio/Effects/Native/Splitter" end
  if spec.devices == "notepad" then return path == "Audio/Effects/Native/Notepad" or CONTAINERS[path] == true end
  return false
end

local function display_from(given, fallback)
  if type(given) == "table" and type(given.title) == "string" and given.title ~= "" then return given end
  return { title = fallback }
end

-- (the journal polls each container it keeps about once a second and on every undo: a text read again unchanged is the
-- same string, whose prints changes_material keeps, so a poll costs Renoise's read of the data and two lookups. One read
-- for every kind: the poll reads a device once for all the kinds it keeps of it, #361 N-S16)
local function polled_read(device)
  local xml = device.active_preset_data
  return { shape = M.shape_of(xml), content = M.content_of(xml) }
end

for _, spec in ipairs(XML_KINDS) do
  local kind = spec.kind
  -- (`was`: the device's preset data as the caller read it a moment ago, else it is read here)
  local function write_checked(song, env, t, d, track, device, xml, shape, desc, was)
    local label = label_of(t, track, device)
    if not takes(spec, device) then bad(string.format("%s isn't a device %s changes", q(label), kind)) end
    was = was or device.active_preset_data
    check_preset_data(device, xml, label, was)
    if type(shape) ~= "string" or M.shape_of(was) ~= shape then
      fail("changed", string.format("%s changed in Renoise since Kumi read it: read it again", q(label)))
    end
    song:describe_undo(desc)
    watch_all(env, t, d, device)
    local ok, err = pcall(function() device.active_preset_data = xml end)
    if not ok then fail("failed", "Renoise refused the preset data: " .. tostring(err):gsub("^[^%s:]*%.lua:%d+: ", "")) end
    return device.active_preset_data
  end
  base.register({
    kind = kind, custom = true, places = true, host_made = true,
    preview = function()
      bad(kind .. " is made by Kumi's host from the device's preset data: ask it through the host's change_preview")
    end,
    apply = function(song, env, input, fence, desc)
      local ok, t, d, track, device = pcall(device_by_ref, song, env, input.ref, kind)
      if not ok then base.gone_is_changed(t) end
      local label = label_of(t, track, device)
      if fence.id ~= nil and fence.id ~= identity(track, device) then fail("changed", base.moved_text(label)) end
      local was = device.active_preset_data
      local before = M.shape_of(was)
      local back = write_checked(song, env, t, d, track, device, input.presetData, input.shape, desc, was)
      local shape, content = M.shape_of(back), M.content_of(back)
      return {
        applied = { shape = shape, edit = input.edit }, material = keep(env, t, d, track, device), target = target_of(t, d),
        display = display_from(input.display, "Changed " .. q(label)), undo = { desc = desc, entry = true },
        prior = { shape = before, content = M.content_of(was) }, journal_applied = { shape = shape, content = content },
        place = { track = t, device = d },
      }
    end,
    restore = function(song, env, restore, desc)
      local material = restore.material
      if type(material) ~= "table" or type(material.write) ~= "table" then bad("material.write: the preset data Kumi's host put back") end
      local t, d, track, device = device_by_place(song, env, material, "Kumi changed it")
      local back = write_checked(song, env, t, d, track, device, material.write.presetData, material.write.shape, desc)
      base.note_restored_at(env.journal, kind, { track = t, device = d })
      return {
        restored = { shape = M.shape_of(back) }, display = display_from(material.write.display, q(label_of(t, track, device)) .. " put back"),
        undo = { desc = desc, entry = true },
      }
    end,
    reread = reread,
    read = polled_read,
    -- (by content where both say it: a value inside changes it, which no shape tells)
    same = function(a, b)
      if a.content and b.content then return a.content == b.content end
      return a.shape == b.shape
    end,
    polled = true,
  })
end

-- meta-destination: where a meta device (*LFO, *Key Tracker, *Velocity Tracker, *Signal Follower, *Meta Mixer, *Formula),
-- an output of *Hydra or *XY Pad, or a #Sidechain sends its values: a device (any track's, its mixer device too) and, but for
-- #Sidechain, that device's parameter (its number from 1, 0 its on/off, or its name); or nothing. Renoise's numbers are
-- 0-based (Dest. Track a track's number less one, Dest. Effect a device's with the mixer device 0) and Renoise moves them
-- with the tracks and devices made, deleted and moved; a record keeps each as a place (the track carried through the layout
-- by hand, the device followed through its track's device log) and works the numbers out again when it is put back.

local function output_of(device, out, label)
  local all = material_.destinations(device)
  if #all == 0 then bad(string.format("%s sends no values anywhere (it has no destination)", q(label))) end
  if out == nil then
    if #all > 1 then bad(string.format("%s has %d outputs: give out (1 to %d)", q(label), #all, #all)) end
    return all[1]
  end
  if type(out) ~= "number" or out ~= math.floor(out) or out < 1 or out > #all then
    bad(string.format("out is a number from 1 to %d for %s", #all, q(label)))
  end
  return all[out]
end

local function numbers_of(destination)
  return { track = destination.track.value, effect = destination.effect.value, parameter = destination.parameter and destination.parameter.value or nil }
end

local function same_numbers(a, b)
  return a.track == b.track and a.effect == b.effect and (a.parameter == nil or b.parameter == nil or a.parameter == b.parameter)
end

-- The place a destination's numbers name now: nothing, or a device on a track (and its parameter).
local function place_of_numbers(song, env, numbers)
  if numbers.track < 0 then return { none = true } end
  local t, d = numbers.track + 1, numbers.effect + 1
  if t > rows.track_count(song) then return { none = true } end
  -- (a track aimed at with no device yet: the producer's half-made aim, its track kept)
  if numbers.effect < 0 or d > #song:track(t).devices then return { none = true, track_only = numbers.track } end
  local device = song:track(t):device(d)
  return { to = t, dev = material_.device_place(env, t, d, device), param = numbers.parameter, label = label_of(t, song:track(t), device) }
end

-- The numbers a kept place names now (its track carried from `layout`, its device followed), or nil and why.
local function numbers_of_place(song, env, place, layout)
  if place.none then
    -- (a half-made aim at a track: that track, where it is now, with no device)
    if type(place.track_only) == "number" and place.track_only >= 0 then
      local t = place.track_only + 1
      if type(layout) == "number" then t = env.shifted(t, layout) end
      if t and t >= 1 and t <= rows.track_count(song) then return { track = t - 1, effect = -1, parameter = -1 } end
    end
    return { track = -1, effect = -1, parameter = -1 }
  end
  local t, why = place.to, nil
  if type(layout) == "number" then t, why = env.shifted(place.to, layout) end
  if not t then return nil, why end
  if t < 1 or t > rows.track_count(song) then return nil, "gone" end
  local d, how = material_.device_now(song, env, t, place.dev)
  if not d then return nil, how end
  return { track = t - 1, effect = d - 1, parameter = place.param }
end

local function write_numbers(destination, numbers)
  destination.track.value = numbers.track
  destination.effect.value = numbers.effect
  if destination.parameter and numbers.parameter ~= nil then destination.parameter.value = numbers.parameter end
end

local function target_words(song, numbers)
  if numbers.track < 0 then return "nothing" end
  local t, d = numbers.track + 1, numbers.effect + 1
  if t > rows.track_count(song) or d < 1 or d > #song:track(t).devices then return "nothing" end
  local track, device = song:track(t), song:track(t):device(d)
  local where = q(devices_.label(device)) .. " on " .. q(base.named(t, track))
  if numbers.parameter == nil or numbers.parameter < 0 then return where end
  local name = numbers.parameter == 0 and "on/off" or (numbers.parameter <= #device.parameters and device:parameter(numbers.parameter).name or ("parameter " .. numbers.parameter))
  return q(name) .. " of " .. where
end

local function meta_title(label, destination, all, song, numbers)
  local whose = #all > 1 and (label .. ": output " .. destination.prefix:match("(%d+)")) or label
  local verb = destination.parameter and "drives " or "feeds "
  return whose .. " " .. (numbers.track < 0 and "sends nowhere" or (verb .. target_words(song, numbers)))
end

-- What the change asks, as Renoise's numbers now: the target device (its ref) and parameter, or none.
local function wanted_numbers(song, env, destination, input, self_t, self_d)
  if input.targetRef == nil or input.targetRef == json_null then return { track = -1, effect = -1, parameter = destination.parameter and -1 or nil } end
  if type(input.targetRef) ~= "string" then bad("targetRef: the device it drives, or null for none") end
  local numbers = refs.expect(input.targetRef, "device", 2, env.epoch())
  local t, d = numbers[1], numbers[2]
  local track = base.track_at(song, t)
  if d < 1 or d > #track.devices then fail("gone", "that device isn't on the track any more") end
  if t == self_t and d == self_d then bad("a device can't drive itself") end
  local target = track:device(d)
  local parameter
  if destination.parameter then
    local wanted = input.param
    if type(wanted) == "number" then
      if wanted ~= math.floor(wanted) or wanted < 0 or wanted > #target.parameters then
        bad(string.format("param is a number from 0 (its on/off) to %d for %s", #target.parameters, q(devices_.label(target))))
      end
      parameter = wanted
    elseif type(wanted) == "string" and wanted ~= "" then
      local lower = wanted:lower()
      for _, exact in ipairs({ true, false }) do
        for k = 1, #target.parameters do
          local name = target:parameter(k).name:lower()
          if (exact and name == lower) or (not exact and name:sub(1, #lower) == lower) then
            if parameter and parameter ~= k then bad(string.format("%q names more than one parameter of %s: give its number", wanted, q(devices_.label(target)))) end
            parameter = k
          end
        end
        if parameter then break end
      end
      if not parameter then bad(string.format("%s has no parameter %q", q(devices_.label(target)), wanted)) end
    else
      bad("param: the parameter it drives, its number from 1 (0 its on/off) or its name")
    end
  elseif input.param ~= nil then
    bad("a #Sidechain feeds a device, not a parameter: leave param out")
  end
  return { track = t - 1, effect = d - 1, parameter = parameter }
end

local function meta_locate(song, env, input, what)
  local t, d, track, device = device_by_ref(song, env, input.ref, what)
  local label = label_of(t, track, device)
  local destination = output_of(device, input.out, label)
  return t, d, track, device, label, destination
end

-- (device-params sets a destination's numbers as a place too: changes_params.lua)
M.destination = {
  numbers_of = numbers_of, same_numbers = same_numbers, place_of_numbers = place_of_numbers, numbers_of_place = numbers_of_place,
  write_numbers = write_numbers, target_words = target_words,
}
M.identity, M.label_of, M.target_of, M.reread = identity, label_of, target_of, reread

base.register({
  kind = "meta-destination", custom = true, places = true,
  preview = function(song, env, input)
    local t, d, track, device, label, destination = meta_locate(song, env, input, "meta-destination")
    local now = numbers_of(destination)
    local wanted = wanted_numbers(song, env, destination, input, t, d)
    local all = material_.destinations(device)
    local display = { title = meta_title(label, destination, all, song, wanted), before = { target = target_words(song, now) }, after = { target = target_words(song, wanted) } }
    if same_numbers(now, wanted) then display.unchanged = true end
    local fence = numbers_of(destination)
    fence.id, fence.prefix = identity(track, device), destination.prefix
    return { target = target_of(t, d), prior = { target = target_words(song, now) }, fence = fence, display = display }
  end,
  apply = function(song, env, input, fence, desc)
    local ok, t, d, track, device, label, destination = pcall(meta_locate, song, env, input, "meta-destination")
    if not ok then base.gone_is_changed(t) end
    if fence.id ~= identity(track, device) or fence.prefix ~= destination.prefix then fail("changed", base.moved_text(label)) end
    local now = numbers_of(destination)
    if not same_numbers(now, fence) then fail("changed", string.format("where %s sends changed since the preview", q(label))) end
    local wanted = wanted_numbers(song, env, destination, input, t, d)
    local prior = place_of_numbers(song, env, now)
    local all = material_.destinations(device)
    local unchanged = same_numbers(now, wanted)
    if not unchanged then
      song:describe_undo(desc)
      -- (the producer's undo of it moves these: watched for it)
      for _, k in pairs(destination.numbers) do
        if env.watch_param then env.watch_param(t, d, k) end
      end
      write_numbers(destination, wanted)
      local after = numbers_of(destination)
      if not same_numbers(after, wanted) then
        pcall(write_numbers, destination, now)
        fail("failed", string.format("Renoise kept where %s sends as it was", q(label)))
      end
    end
    local applied = place_of_numbers(song, env, wanted)
    local kept = keep(env, t, d, track, device)
    kept.prefix = destination.prefix
    local journal_state = numbers_of(destination)
    journal_state.prefix = destination.prefix
    local journal_prior = { track = now.track, effect = now.effect, parameter = now.parameter, prefix = destination.prefix }
    return {
      applied = { place = applied, prefix = destination.prefix }, material = { at = kept, prior = prior },
      target = target_of(t, d),
      display = { title = meta_title(label, destination, all, song, wanted), before = { target = target_words(song, now) }, after = { target = target_words(song, wanted) }, unchanged = unchanged or nil },
      undo = { desc = desc, entry = not unchanged },
      prior = journal_prior, journal_applied = journal_state, place = { track = t, device = d },
    }
  end,
  restore = function(song, env, restore, desc)
    local material, applied = restore.material, restore.applied
    if type(material) ~= "table" or type(material.at) ~= "table" or type(material.prior) ~= "table" or type(applied) ~= "table" or type(applied.place) ~= "table" then
      bad("material and applied: where the device sent its values before and after")
    end
    local t, d, track, device = device_by_place(song, env, material.at, "Kumi aimed it")
    local label = label_of(t, track, device)
    local destination
    for _, candidate in ipairs(material_.destinations(device)) do
      if candidate.prefix == material.at.prefix then destination = candidate end
    end
    if not destination then fail("changed", string.format("%s isn't the device Kumi aimed any more", q(label))) end
    local now = numbers_of(destination)
    local was, why = numbers_of_place(song, env, material.prior, restore.layout)
    if not was then
      fail(why == "unknown" and "stale" or "gone", string.format("the device %s drove before Kumi aimed it %s, so Kumi can't aim it back: nothing was written",
        q(label), why == "gone" and "is gone" or "moved in a way Kumi couldn't follow"))
    end
    if same_numbers(now, was) then
      return { restored = { target = target_words(song, now) }, display = { title = label .. " sends where it did already" }, undo = { desc = desc, entry = false }, unwritten = true }
    end
    local set = numbers_of_place(song, env, applied.place, restore.layout)
    if not restore.discard and not (set and same_numbers(now, set)) then
      fail("changed", string.format("where %s sends changed in Renoise since Kumi aimed it", q(label)))
    end
    song:describe_undo(desc)
    write_numbers(destination, was)
    base.note_restored_at(env.journal, "meta-destination", { track = t, device = d })
    local all = material_.destinations(device)
    return { restored = { target = target_words(song, was) }, display = { title = meta_title(label, destination, all, song, was) .. " (put back)" }, undo = { desc = desc, entry = true } }
  end,
  reread = reread,
  read = function(device, state)
    for _, destination in ipairs(material_.destinations(device)) do
      if destination.prefix == (state or {}).prefix then
        local numbers = numbers_of(destination)
        numbers.prefix = destination.prefix
        return numbers
      end
    end
    return { prefix = (state or {}).prefix }
  end,
  same = function(a, b) return a.prefix == b.prefix and same_numbers(a, b) end,
})

-- doofer-make: a Doofer or Splitter made of devices on a track, in one step: the container (with the preset data Kumi's host
-- composed: the devices inside, meta devices aimed at each other, the macros asked) goes in where the first of them was,
-- and they go out. Refused before anything is written when a device has automation (Renoise drops it with the device),
-- something outside aims at one of them (it would lose its target), or one changed since the host read it. Kumi's undo
-- takes the container out and puts the devices back with their settings, names, bypass and aims, at their places when
-- nothing else moved on the track since, else together where the container was (and says so).

local CONTAINER_PATHS = { doofer = "Audio/Effects/Native/Doofer", splitter = "Audio/Effects/Native/Splitter" }

local function moving(song, env, input, fence, what)
  if type(input.deviceRefs) ~= "table" or #input.deviceRefs == 0 then bad("deviceRefs: the devices to put in it") end
  local own = type(input.trackRef) == "string" and refs.expect(input.trackRef, "track", 1, env.epoch())[1] or nil
  local list, seen = {}, {}
  for i, reference in ipairs(input.deviceRefs) do
    local ok, t, d, track, device = pcall(device_by_ref, song, env, reference, what)
    if not ok then base.gone_is_changed(t) end
    if own and t ~= own then bad("every device must be on the track trackRef names") end
    if seen[d] then bad("a device is named twice") end
    seen[d] = true
    local label = label_of(t, track, device)
    if fence and fence.ids and fence.ids[i] ~= identity(track, device) then fail("changed", base.moved_text(label)) end
    if type(input.shapes) == "table" and input.shapes[i] ~= nil and input.shapes[i] ~= M.shape_of(device.active_preset_data) then
      fail("changed", string.format("%s changed in Renoise since Kumi read it: read it again", q(label)))
    end
    list[i] = { t = t, d = d, track = track, device = device, label = label }
  end
  return list
end

local function make_checks(song, env, list)
  local moved = {}
  for _, m in ipairs(list) do moved[m.d] = true end
  for _, m in ipairs(list) do
    for k = 1, #m.device.parameters do
      if m.device:parameter(k).is_automated then
        bad(string.format("%s has automation (on %s), which Renoise drops with the device: Kumi won't move it into a Doofer", q(m.label), q(m.device:parameter(k).name)))
      end
    end
    for _, route in ipairs(material_.read_routes(song, m.t, m.d, env)) do
      if not (route.at == m.t and moved[route.device]) then
        bad(string.format("a device on %s aims at %s: inside a Doofer it would lose its target", q(base.named(route.at, song:track(route.at))), q(m.label)))
      end
    end
  end
end

local function all_back(song, t, saved)
  if t < 1 or t > rows.track_count(song) then return false end
  local track = song:track(t)
  for _, entry in ipairs(saved or {}) do
    local d = entry.index
    if d > #track.devices or track:device(d).device_path ~= entry.device.path then return false end
  end
  return true
end

base.register({
  kind = "doofer-make", custom = true, places = true, shape = "made", host_made = true,
  preview = function()
    bad("doofer-make is made by Kumi's host from the devices' preset data: ask it through the host's change_preview")
  end,
  apply = function(song, env, input, fence, desc)
    local list = moving(song, env, input, fence, "doofer-make")
    local t, track = list[1].t, list[1].track
    local path = CONTAINER_PATHS[input.type or "doofer"]
    if not path then bad("type is doofer or splitter") end
    make_checks(song, env, list)
    local first = math.huge
    for _, m in ipairs(list) do first = math.min(first, m.d) end
    local at = input.index
    if at == nil or at == json_null then at = first end
    if type(at) ~= "number" or at ~= math.floor(at) or at < 2 or at > #track.devices + 1 then
      bad(string.format("index must be a whole number from 2 (after the mixer device) to %d", #track.devices + 1))
    end
    if type(input.presetData) ~= "string" or #input.presetData > M.MAX_PRESET_BYTES then bad("presetData: the container's preset data") end
    -- (what can be told of the data before the container is made: a refusal after it would leave a step in Renoise's undo,
    -- which clears the producer's redo)
    local slot = path == CONTAINER_PATHS.splitter and "SplitterDevice" or "DooferDevice"
    if preset_kind(input.presetData).slot ~= slot then bad("presetData isn't a " .. (input.type or "doofer") .. "'s preset data (" .. slot .. ")") end
    local trouble = container_trouble(input.presetData)
    if trouble then bad("that preset data won't go into the " .. (input.type or "doofer") .. ": " .. trouble) end
    -- What goes back on the undo: each device as it is (ascending places), and which of the others each meta device aims at.
    local by_place = {}
    for i, m in ipairs(list) do by_place[i] = m end
    table.sort(by_place, function(a, b) return a.d < b.d end)
    local saved = {}
    for i, m in ipairs(by_place) do
      local aims = {}
      for _, destination in ipairs(material_.destinations(m.device)) do
        if destination.track.value == t - 1 then
          for j, other in ipairs(by_place) do
            if destination.effect.value == other.d - 1 then
              aims[#aims + 1] = { prefix = destination.prefix, to = j, parameter = destination.parameter and destination.parameter.value }
            end
          end
        end
      end
      saved[i] = { index = m.d, device = material_.read_device(m.device), aims = #aims > 0 and aims or nil }
    end
    local size = #json_encode(saved)
    if size > base.MAX_MATERIAL_BYTES then
      fail("failed", string.format("the devices hold %.1f MB, more than Kumi keeps to put them back, so it won't move them", size / 1048576))
    end
    song:describe_undo(desc)
    local made, container = pcall(function() return track:insert_device_at(path, at) end)
    if not made or not container then fail("failed", "Renoise wouldn't make the " .. (input.type or "doofer")) end
    local ok, err = pcall(function()
      check_preset_data(container, input.presetData, label_of(t, track, container))
      container.active_preset_data = input.presetData
    end)
    if not ok then
      pcall(function() track:delete_device_at(at) end)
      if type(err) ~= "table" then err = { code = "failed", message = "Renoise refused the preset data: " .. tostring(err) } end
      error(err, 0)
    end
    -- The devices out, from the last: each place past the container's moved one on by its insert.
    for i = #by_place, 1, -1 do
      local d = by_place[i].d
      track:delete_device_at(d >= at and d + 1 or d)
    end
    local before = 0
    for _, m in ipairs(by_place) do if m.d < at then before = before + 1 end end
    local final = at - before
    if type(input.name) == "string" and input.name ~= "" then container.display_name = input.name end
    local kept = keep(env, t, final, track, container)
    kept.devices, kept.mark = saved, env.device_mark and env.device_mark(t) or nil
    local made_data = container.active_preset_data
    return {
      applied = { shape = M.shape_of(made_data), content = M.content_of(made_data), count = #saved }, material = kept,
      created = { ref = refs.make(env.epoch(), "device", t, final), kind = "device" }, target = target_of(t, final),
      display = display_from(input.display, "Made a Doofer on " .. q(base.named(t, track))), undo = { desc = desc, entry = true },
      prior = { count = 0 }, place = { track = t, device = final }, shape = "made",
    }
  end,
  restore = function(song, env, restore, desc)
    local material, applied = restore.material, restore.applied
    if type(material) ~= "table" or type(material.devices) ~= "table" or type(applied) ~= "table" then bad("material: the devices the container took in") end
    local ok, t, d, track, container = pcall(device_by_place, song, env, material, "Kumi made it")
    if not ok then
      local err = t
      if type(err) == "table" and err.code == "gone" and all_back(song, material.track, material.devices) then
        return { restored = { count = 0 }, display = { title = "The devices are back already" }, undo = { desc = desc, entry = false }, unwritten = true }
      end
      error(err, 0)
    end
    local label = label_of(t, track, container)
    local data = container.active_preset_data
    if M.shape_of(data) ~= applied.shape then
      fail("changed", string.format("the devices or macros inside %s changed in Renoise since Kumi made it, so it can't take it apart", q(label)))
    end
    -- (the devices go back as they were kept: a knob turned inside since, a macro or a plug-in's state, would be lost)
    if not restore.discard and applied.content ~= nil and M.content_of(data) ~= applied.content then
      fail("changed", string.format("settings inside %s changed in Renoise since Kumi made it (a knob, a macro or a plug-in's state): taking it apart would put the old ones back, so Kumi left it", q(label)))
    end
    for k = 1, #container.parameters do
      if container:parameter(k).is_automated then fail("changed", string.format("%s's macros have automation since, which taking it apart would lose", q(label))) end
    end
    for _, route in ipairs(material_.read_routes(song, t, d, env)) do
      fail("changed", string.format("a device on %s aims at %s since: taking it apart would lose that", q(base.named(route.at, song:track(route.at))), q(label)))
    end
    -- At their own places when nothing moved on the track since; else together where the container is.
    local exact = material.mark ~= nil and env.device_mark and env.device_mark(t) == material.mark
    song:describe_undo(desc)
    track:delete_device_at(d)
    local places = {}
    for i, entry in ipairs(material.devices) do
      local at = exact and entry.index or (d + i - 1)
      at = math.max(2, math.min(at, #track.devices + 1))
      material_.restore_device(track, at, entry.device)
      places[i] = at
    end
    for i, entry in ipairs(material.devices) do
      for _, aim in ipairs(entry.aims or {}) do
        local device = track:device(places[i])
        for _, destination in ipairs(material_.destinations(device)) do
          if destination.prefix == aim.prefix and places[aim.to] then
            pcall(function()
              destination.track.value = t - 1
              destination.effect.value = places[aim.to] - 1
              if destination.parameter and type(aim.parameter) == "number" then destination.parameter.value = aim.parameter end
            end)
          end
        end
      end
    end
    base.note_restored(env.journal, "doofer-make", { track = t, device = d })
    return {
      restored = { count = #material.devices },
      display = {
        title = string.format("Took %s apart (put back)", q(label)),
        note = not exact and "The track's devices changed since, so they went back together where it was." or nil,
      },
      undo = { desc = desc, entry = true },
    }
  end,
})

-- preset_read {ref | at, epoch, layout}: a device's preset data and what Kumi's host needs to edit it (its shape, its
-- parameters, where a record finds it again). `at` is a record's place (`material`), for an undo.
function M.read(params, env)
  local song = base.context(params, env)
  local t, d, track, device
  if params.ref ~= nil then
    t, d, track, device = device_by_ref(song, env, params.ref, "preset_read")
  elseif type(params.at) == "table" then
    t, d, track, device = device_by_place(song, env, params.at, "Kumi changed it")
  else
    bad("ref (a device) or at (where a change left it)")
  end
  local xml = device.active_preset_data
  if #xml > M.MAX_PRESET_BYTES then
    fail("failed", string.format("%s holds %.1f MB of settings, more than Kumi edits", q(label_of(t, track, device)), #xml / 1048576))
  end
  local parameters = json_array()
  for k = 1, math.min(#device.parameters, M.MOST_WATCHED) do
    local parameter = device:parameter(k)
    parameters[k] = { name = parameter.name, value = parameter.value, valueString = parameter.value_string, min = parameter.value_min, max = parameter.value_max }
  end
  -- (the plug-ins in it by identifier, with the names Renoise gives them: a Doofer's inner plug-ins have none in the XML)
  local plugins
  for id in xml:gmatch("<PluginIdentifier>([^<]*)</PluginIdentifier>") do
    plugins = plugins or {}
    plugins[id] = false
  end
  -- (the device paths asked about that the device's track can take: Audio Units the host would put inside a Doofer)
  local available, wanted = nil, {}
  if type(params.paths) == "table" then
    available = {}
    for i = 1, math.min(#params.paths, 16) do
      if type(params.paths[i]) == "string" then wanted[params.paths[i]] = true; available[params.paths[i]] = false end
    end
  end
  if plugins or available then
    for _, info in ipairs(track.available_device_infos) do
      local id = info.path:match("([^/]+)$")
      if wanted[info.path] then
        available[info.path] = true
        plugins = plugins or {}
        plugins[id] = info.short_name
      elseif plugins and plugins[id] == false then
        plugins[id] = info.short_name
      end
    end
    for id, name in pairs(plugins or {}) do
      if name == false then plugins[id] = nil end
    end
  end
  return {
    available = available, plugins = plugins,
    epoch = env.epoch(), layout = env.layout(), ref = refs.make(env.epoch(), "device", t, d), target = target_of(t, d),
    at = keep(env, t, d, track, device), id = identity(track, device), path = device.device_path, name = device.name,
    label = devices_.label(device), trackName = base.named(t, track), active = device.is_active, presetData = xml,
    shape = M.shape_of(xml), print = M.print_of(xml), parameters = parameters,
  }
end

-- Actions (no step of Renoise's undo, nothing Kumi takes back): a device's own editor window, its settings saved as a preset.

-- device_editor {ref, show?}: a plug-in's own editor window (or a *LFO's) shown, or hidden with show false.
function M.editor(params, env)
  local song = base.context(params, env)
  local t, _, track, device = device_by_ref(song, env, params.ref, "device_editor")
  local label = label_of(t, track, device)
  if not device.external_editor_available then
    bad(string.format("%s has no editor window of its own: Renoise shows its settings in the track's DSP lane", q(label)))
  end
  device.external_editor_visible = params.show ~= false
  return { epoch = env.epoch(), layout = env.layout(), visible = device.external_editor_visible, label = label }
end

-- Renoise's user library, where it was made at its first start (Documents/Renoise/User Library).
function M.user_library()
  local home = os.getenv("HOME") or os.getenv("USERPROFILE")
  if not home then return nil end
  return home .. "/Documents/Renoise/User Library"
end

-- The folder a native device's presets go in, as Renoise's library names them: its name without its * or # ("LFO", "Send").
local function preset_folder(device)
  return (device.name:gsub("^[%*#]", ""))
end

-- preset_save {ref, name, replace?}: a native device's settings (its preset data) saved as a preset file in the user
-- library, Effect Presets/<device>/<name>.xrdp; one of that name is replaced only with replace. Renoise 3.5.4 lists such a
-- file in the device's presets from its next start, and never a plug-in's (lab), so plug-ins are refused.
function M.save(params, env)
  local song = base.context(params, env)
  local t, _, track, device = device_by_ref(song, env, params.ref, "preset_save")
  local label = label_of(t, track, device)
  if not device.device_path:find("/Native/", 1, true) then
    bad(string.format("Renoise lists a plug-in's presets only when it saves them itself: save %s's from its preset menu in Renoise", q(label)))
  end
  local name = params.name
  if type(name) ~= "string" or name == "" or #name > 128 or name:find('[%c/\\:%*%?"<>|]') or name:match("^%.") or name:match("[%. ]$") then
    bad('name: a preset\'s name, one line without / \\ : * ? " < > | that doesn\'t start with a dot or end with a dot or space')
  end
  local library = M.user_library()
  if not library or not io.exists(library) then
    fail("failed", string.format("Kumi can't find Renoise's user library at %s", tostring(library)))
  end
  local folder = preset_folder(device)
  if type(folder) ~= "string" or folder == "" or folder:find('[%c/\\:%*%?"<>|]') then
    bad(string.format("Kumi can't tell which folder of the user library %s's presets go in", q(label)))
  end
  local dir = library .. "/Effect Presets"
  if not io.exists(dir) then os.mkdir(dir) end
  dir = dir .. "/" .. folder
  if not io.exists(dir) then os.mkdir(dir) end
  if not io.exists(dir) then fail("failed", "Kumi couldn't make the folder " .. dir) end
  local file = dir .. "/" .. name .. ".xrdp"
  if io.exists(file) and params.replace ~= true then
    bad(string.format("a preset named %s is in %s already: give another name, or replace it", q(name), dir))
  end
  local out, why = io.open(file, "wb")
  if out then
    out:write(device.active_preset_data)
    out:close()
  else
    fail("failed", "Kumi couldn't write " .. file .. ": " .. tostring(why))
  end
  local listed = false
  for _, preset in ipairs(device.presets) do
    if preset == name then listed = true end
  end
  -- (where it went, said: Kumi takes the user library from where Renoise makes it, which its preferences can move)
  local note = string.format("Saved in %s. Renoise lists it from its next start if that is the user library its preferences name.", dir)
  return { epoch = env.epoch(), layout = env.layout(), file = file, listed = listed, label = label, note = note }
end

return M
