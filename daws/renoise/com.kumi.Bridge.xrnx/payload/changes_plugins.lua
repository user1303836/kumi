-- Kumi's changes to a plug-in or MIDI instrument (M4): a plug-in loaded (or taken out), its parameters, its program, its
-- channel, transpose, volume and suspend; a MIDI instrument's output (device, channel, transpose, program, bank, delay,
-- duration). Records keep the instrument by its place (instruments.lua). Where Renoise's own undo doesn't take a field back,
-- the change says so (measured in the lab; see the notes).

local import = ...
local json = import("json")
local base = import("changes_base")
local places_ = import("instruments")
local instruments_ = import("changes_instruments")
local modfx_ = import("changes_modfx")
local material_ = import("changes_material")
local vst3_ = import("vst3_presets")

local bad, fail = base.bad, base.fail
local q = base.q
local whole, is_boolean = instruments_.whole, instruments_.is_boolean
local M = {}

-- The most plug-in state (its preset data) Kumi keeps to put a plug-in back.
M.MAX_STATE_BYTES = 8 * 1024 * 1024

-- Which of these Renoise's own undo takes back (lab, Renoise 3.5.4, each alone in a callback and then undone): a plug-in load,
-- its parameters, its program, its channel, transpose and volume, a MIDI output's fields are steps; auto-suspend is not
-- (plugin-suspend says so).
M.UNDO_COVERS = { plugin = true, preset = true, midi = true }
local NOT_IN_UNDO = "Renoise's own undo won't take this back; Kumi can."

local function state_hash(text)
  return material_.hash_of(text)
end

local function plugin_now(instrument)
  local properties = instrument.plugin_properties
  if not properties.plugin_loaded then return { path = "" } end
  local device = properties.plugin_device
  return { path = device.device_path, name = device.name, hash = state_hash(device.active_preset_data or "") }
end

-- The plug-in a request names: its path as Renoise lists it, or a name only one plug-in has (and the name Renoise shows).
local function plugin_name(properties, path)
  for _, info in ipairs(properties.available_plugin_infos) do
    if info.path == path then return info.name end
  end
  return path:match("([^/]+)$") or path
end

local function resolve_plugin(properties, wanted)
  if wanted == "" or wanted == json.null or wanted == false then return "" end
  if type(wanted) ~= "string" then bad("path: a plug-in's path (as Renoise lists it), its name, or \"\" to take the plug-in out") end
  local lower, matches, near = wanted:lower(), {}, {}
  for _, info in ipairs(properties.available_plugin_infos) do
    if info.path == wanted then return info.path end
    if info.path:lower() == lower or (info.name or ""):lower() == lower or (info.short_name or ""):lower() == lower then matches[#matches + 1] = info.path
    elseif (info.name or ""):lower():find(lower, 1, true) and #near < 5 then near[#near + 1] = info.name .. " (" .. info.path .. ")" end
  end
  if #matches == 1 then return matches[1] end
  if #matches > 1 then bad(string.format("%q names %d plug-ins: say which by its path (%s)", wanted, #matches, table.concat(matches, ", ", 1, math.min(5, #matches)))) end
  bad(string.format("Renoise has no plug-in instrument %q.%s", wanted, #near > 0 and (" Close: " .. table.concat(near, ", ") .. ".") or ""))
end

-- plugin-load {ref: instrument, path}: Kumi keeps the plug-in it replaces (its path and state) to put back.

local function load_plan(song, env, input)
  local n, objects, label = places_.locate(song, env, "instrument", input, "plugin-load")
  local properties = objects.instrument.plugin_properties
  local path = resolve_plugin(properties, input.path)
  local now = plugin_now(objects.instrument)
  local kept = { path = now.path }
  if now.path ~= "" then
    kept.state = properties.plugin_device.active_preset_data or ""
    if #kept.state > M.MAX_STATE_BYTES then fail("failed", string.format("%s's plug-in holds %.1f MB of state, more than Kumi keeps to put it back", q(label), #kept.state / 1048576)) end
  end
  return { n = n, label = label, path = path, now = now, kept = kept, instrument = objects.instrument, name = path ~= "" and plugin_name(properties, path) or "" }
end

local function load_title(plan)
  if plan.path == "" then return "Took the plug-in out of " .. q(plan.label) end
  return string.format("Loaded %s into %s", q(plan.name), q(plan.label))
end

base.register({
  kind = "plugin-load", custom = true, by_place = true,
  preview = function(song, env, input)
    local plan = load_plan(song, env, input)
    local display = { title = load_title(plan) }
    if plan.now.path == plan.path then display.unchanged = true end
    if plan.now.path ~= "" and plan.path ~= plan.now.path then display.note = "Kumi keeps the plug-in it replaces, with its state, to put back." end
    if not M.UNDO_COVERS.plugin then display.note = (display.note and (display.note .. " ") or "") .. NOT_IN_UNDO end
    return { target = places_.target_of(env, "instrument", plan.n), prior = { path = plan.now.path }, fence = plan.now, display = display }
  end,
  apply = function(song, env, input, fence, desc)
    local ok, plan = pcall(load_plan, song, env, input)
    if not ok then base.gone_is_changed(plan) end
    if not base.same_table(plan.now, fence) then fail("changed", q(plan.label) .. "'s plug-in changed since the preview") end
    local display = { title = load_title(plan) }
    local n = plan.n
    if plan.now.path == plan.path then
      display.unchanged = true
      return { applied = plan.now, display = display, undo = { desc = desc, entry = false }, prior = { path = plan.now.path }, target = places_.target_of(env, "instrument", n) }
    end
    if M.UNDO_COVERS.plugin then song:describe_undo(desc) end
    -- (Renoise names an instrument after the plug-in it loads, and leaves it unnamed when its plug-in goes: lab 2026-10-19. A
    -- name the producer gave an instrument with no plug-in stays; Kumi's undo puts back the name it had either way.)
    local name = plan.instrument.name
    local loaded = plan.instrument.plugin_properties:load_plugin(plan.path)
    if not loaded or (plan.path ~= "" and not plan.instrument.plugin_properties.plugin_loaded) then
      -- (the plug-in it had, with its state, loaded again if Renoise let it go before refusing)
      local properties, back = plan.instrument.plugin_properties, true
      if plan.kept.path ~= "" and (not properties.plugin_loaded or properties.plugin_device.device_path ~= plan.kept.path) then
        back = pcall(function()
          if not properties:load_plugin(plan.kept.path) then error("refused") end
          properties.plugin_device.active_preset_data = plan.kept.state
        end)
      end
      pcall(function() if plan.instrument.name ~= name then plan.instrument.name = name end end)
      fail("failed", "Renoise wouldn't load the plug-in " .. plan.path .. (back and "" or "; the plug-in it had couldn't be loaded back either: Renoise's own undo may bring it"))
    end
    if plan.now.path == "" and name ~= "" and plan.instrument.name ~= name then plan.instrument.name = name end
    if plan.instrument.name ~= name and plan.instrument.name == "" then
      display.note = string.format("Renoise left the instrument unnamed as its plug-in went; Kumi's undo names it %s again.", q(name))
    elseif plan.instrument.name ~= name then
      display.note = string.format("Renoise named the instrument %s after its plug-in; Kumi's undo names it %s again.", q(plan.instrument.name), q(name))
    end
    if not M.UNDO_COVERS.plugin then display.note = (display.note and (display.note .. " ") or "") .. NOT_IN_UNDO end
    return {
      applied = plugin_now(plan.instrument), material = { kept = plan.kept, name = name }, target = places_.target_of(env, "instrument", n),
      display = display, undo = { desc = desc, entry = M.UNDO_COVERS.plugin }, prior = { path = plan.now.path },
      place = places_.journal_place("instrument", n),
    }
  end,
  restore = function(song, env, restore, desc)
    local material, applied = restore.material, restore.applied
    if type(material) ~= "table" or type(material.kept) ~= "table" or type(applied) ~= "table" then bad("material: the plug-in the change replaced") end
    local n, objects, label = places_.locate(song, env, "instrument", { place = restore.target and restore.target.place }, "plugin-load")
    local instrument = objects.instrument
    local now = plugin_now(instrument)
    if now.path == material.kept.path and (material.kept.path == "" or now.hash == state_hash(material.kept.state or "")) then
      return { restored = { path = now.path }, display = { title = q(label) .. "'s plug-in is back already" }, undo = { desc = desc, entry = false }, unwritten = true }
    end
    if not restore.discard and (now.path ~= applied.path or (applied.hash and now.hash ~= applied.hash)) then
      fail("changed", q(label) .. "'s plug-in changed in Renoise since Kumi loaded it")
    end
    if M.UNDO_COVERS.plugin then song:describe_undo(desc) end
    local properties = instrument.plugin_properties
    if not properties:load_plugin(material.kept.path) then fail("failed", "Renoise wouldn't load the plug-in " .. material.kept.path .. " back") end
    local note
    if material.kept.path ~= "" and material.kept.state then
      if not pcall(function() properties.plugin_device.active_preset_data = material.kept.state end) then
        note = "Its plug-in is back, but didn't take its old state: look at its settings."
      end
    end
    -- (and the name it had: Renoise renamed it as the plug-in came or went)
    if type(material.name) == "string" and instrument.name ~= material.name then instrument.name = material.name end
    base.note_restored(env.journal, "plugin-load", places_.journal_place("instrument", n), applied, base.KINDS["plugin-load"])
    return { restored = { path = material.kept.path }, display = { title = q(label) .. "'s plug-in put back", note = note }, undo = { desc = desc, entry = M.UNDO_COVERS.plugin } }
  end,
  reread = places_.reread("instrument"), read = function(instrument) return plugin_now(instrument) end,
  same = function(a, b) return a.path == b.path and (a.hash == nil or b.hash == nil or a.hash == b.hash) end,
})

-- plugin-param {ref: pluginparam | instrument + param, value | valueString}

local function plugin_device_of(objects)
  local properties = objects.instrument.plugin_properties
  if not properties.plugin_loaded then bad(q(objects.instrument.name ~= "" and objects.instrument.name or "the instrument") .. " has no plug-in loaded") end
  return properties.plugin_device
end

modfx_.param_kind({ kind = "plugin-param", of = "instrument", param_of = "pluginparam", device_of = plugin_device_of })

-- plugin-preset {ref: instrument, preset: a number from 1, or a preset's name}: the plug-in's program; Kumi keeps the state it
-- had (its preset data) to put back exactly.

local function preset_plan(song, env, input)
  local n, objects, label = places_.locate(song, env, "instrument", input, "plugin-preset")
  local device = plugin_device_of(objects)
  local presets = device.presets
  local wanted = input.preset
  local index
  if type(wanted) == "number" then
    if not whole(1, #presets)(wanted) then bad(string.format("preset is a number from 1 to %d (the plug-in's programs)", #presets)) end
    index = wanted
  elseif type(wanted) == "string" then
    local lower = wanted:lower()
    for k, name in ipairs(presets) do if name:lower() == lower then index = k end end
    if not index then bad(string.format("the plug-in has no program %q (%d programs)", wanted, #presets)) end
  else
    bad("preset: a program's number (from 1) or name")
  end
  local state = device.active_preset_data or ""
  if #state > M.MAX_STATE_BYTES then fail("failed", "the plug-in's state is more than Kumi keeps to put it back") end
  return { n = n, label = label, device = device, index = index, name = presets[index], now = device.active_preset, state = state, path = device.device_path }
end

base.register({
  kind = "plugin-preset", custom = true, by_place = true,
  preview = function(song, env, input)
    local plan = preset_plan(song, env, input)
    local display = { title = string.format("%s plays program %s", q(plan.label), q(plan.name)) }
    if plan.now == plan.index then display.unchanged = true end
    return { target = places_.target_of(env, "instrument", plan.n), prior = { preset = plan.now }, fence = { preset = plan.now, hash = state_hash(plan.state) }, display = display }
  end,
  apply = function(song, env, input, fence, desc)
    local ok, plan = pcall(preset_plan, song, env, input)
    if not ok then base.gone_is_changed(plan) end
    if plan.now ~= fence.preset or state_hash(plan.state) ~= fence.hash then fail("changed", q(plan.label) .. "'s plug-in changed since the preview") end
    local display = { title = string.format("%s plays program %s", q(plan.label), q(plan.name)) }
    if plan.now == plan.index then
      display.unchanged = true
      return { applied = { preset = plan.now, hash = fence.hash }, display = display, undo = { desc = desc, entry = false }, prior = { preset = plan.now }, target = places_.target_of(env, "instrument", plan.n) }
    end
    if M.UNDO_COVERS.preset then song:describe_undo(desc) end
    plan.device.active_preset = plan.index
    -- (a plug-in may keep its program: Apple's DLSMusicDevice, one program, reads none chosen before and after, its state as
    -- it was, lab 2026-10-20. Nothing changed: no step, no record)
    local kept_hash = state_hash(plan.device.active_preset_data or "")
    if plan.device.active_preset ~= plan.index and kept_hash == fence.hash then
      display.unchanged, display.note = true, "the plug-in kept its program as it was (Renoise reads none chosen), so nothing changed"
      return { applied = { preset = plan.device.active_preset, hash = kept_hash }, display = display, undo = { desc = desc, entry = false }, prior = { preset = plan.now }, target = places_.target_of(env, "instrument", plan.n) }
    end
    if not M.UNDO_COVERS.preset then display.note = NOT_IN_UNDO end
    return {
      applied = { preset = plan.device.active_preset, hash = kept_hash }, material = { state = plan.state, path = plan.path },
      target = places_.target_of(env, "instrument", plan.n), display = display, undo = { desc = desc, entry = M.UNDO_COVERS.preset },
      prior = { preset = plan.now }, place = places_.journal_place("instrument", plan.n),
    }
  end,
  restore = function(song, env, restore, desc)
    local material, prior, applied = restore.material, restore.prior, restore.applied
    if type(material) ~= "table" or type(prior) ~= "table" or type(applied) ~= "table" then bad("material: the plug-in's state before") end
    local n, objects, label = places_.locate(song, env, "instrument", { place = restore.target and restore.target.place }, "plugin-preset")
    local device = plugin_device_of(objects)
    -- (the state kept is that plug-in's: another one loaded since never gets it, discard or not)
    if material.path ~= nil and device.device_path ~= material.path then
      fail("changed", string.format("%s plays another plug-in now: Kumi won't give it the old one's state", q(label)))
    end
    local hash = state_hash(device.active_preset_data or "")
    if device.active_preset == prior.preset and hash == state_hash(material.state or "") then
      return { restored = { preset = prior.preset }, display = { title = q(label) .. "'s program is back already" }, undo = { desc = desc, entry = false }, unwritten = true }
    end
    if not restore.discard and (device.active_preset ~= applied.preset or hash ~= applied.hash) then
      fail("changed", q(label) .. "'s plug-in changed in Renoise since Kumi chose its program")
    end
    if M.UNDO_COVERS.preset then song:describe_undo(desc) end
    pcall(function() if prior.preset >= 0 and prior.preset <= #device.presets then device.active_preset = prior.preset end end)
    if not pcall(function() device.active_preset_data = material.state end) then fail("failed", "the plug-in didn't take its old state back") end
    base.note_restored(env.journal, "plugin-preset", places_.journal_place("instrument", n), applied, base.KINDS["plugin-preset"])
    return { restored = { preset = prior.preset }, display = { title = q(label) .. "'s program put back" }, undo = { desc = desc, entry = M.UNDO_COVERS.preset } }
  end,
  reread = function(song, place)
    local instrument = places_.reread("instrument")(song, place)
    return instrument and instrument.plugin_properties.plugin_loaded and instrument.plugin_properties.plugin_device or nil
  end,
  read = function(device) return { preset = device.active_preset, hash = state_hash(device.active_preset_data or "") } end,
  same = function(a, b) return a.preset == b.preset and a.hash == b.hash end,
})

-- plugin-preset-data: the host snapshots a validated .vstpreset file. Keep the complete prior device XML, and apply only
-- to the same VST3 identity. A program number is not sufficient to undo a synth's arbitrary oscillator/sample/FX state.
local function data_plan(song, env, input)
  local n, objects, label = places_.locate(song, env, "instrument", input, "plugin-preset-data")
  local device = plugin_device_of(objects)
  local state = device.active_preset_data or ""
  if #state > M.MAX_STATE_BYTES then fail("failed", "the plug-in's state exceeds Kumi's undo state limit") end
  return { n = n, label = label, device = device, state = state, path = device.device_path,
    wanted = vst3_.compose(device, state, input) }
end

local function data_title(plan, input)
  return string.format("%s plays preset %s", q(plan.label), q(input.presetName or "VST3 state"))
end

base.register({
  kind = "plugin-preset-data", custom = true, by_place = true,
  preview = function(song, env, input)
    local plan = data_plan(song, env, input)
    return { target = places_.target_of(env, "instrument", plan.n), prior = { hash = state_hash(plan.state) },
      fence = { path = plan.path, hash = state_hash(plan.state), wanted = state_hash(plan.wanted) },
      display = { title = data_title(plan, input), unchanged = plan.state == plan.wanted or nil } }
  end,
  apply = function(song, env, input, fence, desc)
    local ok, plan = pcall(data_plan, song, env, input)
    if not ok then base.gone_is_changed(plan) end
    if plan.path ~= fence.path or state_hash(plan.state) ~= fence.hash or state_hash(plan.wanted) ~= fence.wanted then
      fail("changed", q(plan.label) .. "'s plug-in state or requested preset changed since the preview")
    end
    local unchanged = plan.state == plan.wanted
    if not unchanged then
      if M.UNDO_COVERS.preset then song:describe_undo(desc) end
      if not pcall(function() plan.device.active_preset_data = plan.wanted end) then
        local restored = pcall(function() plan.device.active_preset_data = plan.state end)
        fail("failed", "Renoise refused the VST3 preset state" .. (restored and "" or "; the previous state could not be restored"))
      end
    end
    local back = plan.device.active_preset_data or ""
    return { applied = { path = plan.path, hash = state_hash(back) }, material = { path = plan.path, state = plan.state },
      target = places_.target_of(env, "instrument", plan.n), prior = { hash = fence.hash },
      display = { title = data_title(plan, input), unchanged = unchanged or nil },
      undo = { desc = desc, entry = not unchanged and M.UNDO_COVERS.preset },
      place = places_.journal_place("instrument", plan.n), unwritten = unchanged }
  end,
  restore = function(song, env, restore, desc)
    local material, applied = restore.material, restore.applied
    if type(material) ~= "table" or type(material.state) ~= "string" or type(applied) ~= "table" then bad("material: the plug-in's prior state") end
    local n, objects, label = places_.locate(song, env, "instrument", { place = restore.target and restore.target.place }, "plugin-preset-data")
    local device = plugin_device_of(objects)
    if device.device_path ~= material.path then fail("changed", q(label) .. " plays another plug-in now") end
    local hash = state_hash(device.active_preset_data or "")
    if hash == state_hash(material.state) then
      return { restored = { hash = hash }, display = { title = q(label) .. "'s state is back already" }, undo = { desc = desc, entry = false }, unwritten = true }
    end
    if not restore.discard and hash ~= applied.hash then fail("changed", q(label) .. "'s plug-in changed since Kumi loaded its preset") end
    if M.UNDO_COVERS.preset then song:describe_undo(desc) end
    if not pcall(function() device.active_preset_data = material.state end) then fail("failed", "the plug-in refused its prior state") end
    local after = state_hash(device.active_preset_data or "")
    if after ~= state_hash(material.state) then fail("failed", "the plug-in did not restore its exact prior state") end
    base.note_restored(env.journal, "plugin-preset-data", places_.journal_place("instrument", n), applied, base.KINDS["plugin-preset-data"])
    return { restored = { path = material.path, hash = after }, display = { title = q(label) .. "'s state put back" }, undo = { desc = desc, entry = M.UNDO_COVERS.preset } }
  end,
  reread = function(song, place)
    local instrument = places_.reread("instrument")(song, place)
    return instrument and instrument.plugin_properties.plugin_loaded and instrument.plugin_properties.plugin_device or nil
  end,
  read = function(device) return { path = device.device_path, hash = state_hash(device.active_preset_data or "") } end,
  same = function(a, b) return a.path == b.path and a.hash == b.hash end,
})

-- plugin-set {ref: instrument, channel?, transpose?, volumeDb?, autoSuspend?}: the plug-in instrument's own settings.

local db_to_linear, linear_to_db = instruments_.db_to_linear, instruments_.linear_to_db

instruments_.instrument_spec(instruments_.settings_kind({
  kind = "plugin-set", noun = "plug-in settings", undoable = true, order = { "channel", "transpose", "volumeDb" },
  object = function(objects) return objects.instrument.plugin_properties end,
  label = function(label) return label .. "'s plug-in" end,
  fields = {
    channel = { get = function(p) return p.channel end, set = function(p, v) p.channel = v end, field = "channel", check = whole(1, 16),
      says = "channel is the MIDI channel the plug-in gets, 1 to 16", text = function(v) return "channel " .. v end },
    transpose = { get = function(p) return p.transpose end, set = function(p, v) p.transpose = v end, field = "transpose", check = whole(-120, 120),
      says = "transpose is whole semitones from -120 to 120", text = function(v) return "transpose " .. v end },
    volumeDb = { get = function(p) return linear_to_db(p.volume) end, set = function(p, v) p.volume = db_to_linear(v) end, field = "volume",
      check = function(v) return type(v) == "number" and v == v and v <= 12.0412 end, says = "volumeDb is a number up to +12 (-200 or less is silence)",
      text = function(v) return "volume " .. base.db_text(v > -200 and v or nil) .. " dB" end },
  },
}))

-- plugin-suspend {ref: instrument, autoSuspend}: whether the plug-in sleeps when silent. Renoise's own undo skips it (lab).

instruments_.instrument_spec(instruments_.settings_kind({
  kind = "plugin-suspend", noun = "auto-suspend", undoable = false, order = { "autoSuspend" },
  object = function(objects) return objects.instrument.plugin_properties end,
  label = function(label) return label .. "'s plug-in" end,
  fields = {
    autoSuspend = { get = function(p) return p.auto_suspend end, set = function(p, v) p.auto_suspend = v end, field = "auto_suspend", check = is_boolean,
      says = "autoSuspend is true or false", text = function(v) return v and "suspends when silent" or "always running" end },
  },
}))

-- midi-set {ref: instrument, device?, type?, channel?, transpose?, program?, bank?, delay?, duration?}: a MIDI instrument's output.

local TYPES = instruments_.named({ external = 1, ["line-in"] = 2, internal = 3 })

local midi = instruments_.settings_kind({
  kind = "midi-set", noun = "MIDI output", undoable = true,
  order = { "device", "type", "channel", "transpose", "program", "bank", "delay", "duration" },
  object = function(objects) return objects.instrument.midi_output_properties end,
  label = function(label) return label .. "'s MIDI output" end,
  fields = {
    device = { get = function(m) return m.device_name end, set = function(m, v) m.device_name = v end, field = "device_name",
      check = function(v)
        if v == "" then return true end
        for _, name in ipairs(renoise.Midi.available_output_devices()) do if name == v then return true end end
        return false
      end,
      says = "device is one of Renoise's MIDI outputs (as it lists them), or \"\" for none", text = function(v) return v == "" and "no device" or ("to " .. v) end },
    type = { get = function(m) return TYPES.get(m.instrument_type) end, set = function(m, v) m.instrument_type = TYPES.parse(v) end, field = "instrument_type",
      check = function(v) return TYPES.parse(v) ~= nil end, says = "type is external, line-in or internal", text = function(v) return "type " .. v end },
    channel = { get = function(m) return m.channel end, set = function(m, v) m.channel = v end, field = "channel", check = whole(1, 16), says = "channel is 1 to 16", text = function(v) return "channel " .. v end },
    transpose = { get = function(m) return m.transpose end, set = function(m, v) m.transpose = v end, field = "transpose", check = whole(-120, 120), says = "transpose is -120 to 120", text = function(v) return "transpose " .. v end },
    program = { get = function(m) return m.program end, set = function(m, v) m.program = v end, field = "program", check = whole(0, 128), says = "program is 1 to 128 (0 for none)", text = function(v) return v == 0 and "no program change" or ("program " .. v) end },
    bank = { get = function(m) return m.bank end, set = function(m, v) m.bank = v end, field = "bank", check = whole(0, 65536), says = "bank is 1 to 65536 (0 for none)", text = function(v) return v == 0 and "no bank change" or ("bank " .. v) end },
    delay = { get = function(m) return m.delay end, set = function(m, v) m.delay = v end, field = "delay", check = whole(0, 100), says = "delay is 0 to 100 ms", text = function(v) return "delay " .. v .. " ms" end },
    duration = { get = function(m) return m.duration end, set = function(m, v) m.duration = v end, field = "duration", check = whole(1, 8000), says = "duration is 1 to 8000 ms (8000 holds until the note-off)", text = function(v) return v == 8000 and "held to the note-off" or ("notes of " .. v .. " ms") end },
  },
})
-- (where Renoise's undo skips the MIDI output, the change isn't one of its steps, and says so)
midi.undoable = M.UNDO_COVERS.midi
instruments_.instrument_spec(midi)

return M
