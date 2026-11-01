-- A song for the mock Renoise: tracks (sequencer, group, master, send), patterns with lines, the sequence, instruments with
-- samples, phrases and macros, devices with parameters, the selection and the transport. Everything is plain data behind
-- strict objects (member names are checked against the API like the rest of the mock), and `M.ops` changes the data the
-- way the producer does in Renoise's GUI: each fires the observables Renoise would.
--
-- Values follow Renoise: notes 0..119 (48 = C-4, 60 = C-5), 120 = OFF, 121 = empty; instrument, volume and panning 255 =
-- empty; text as the tracker shows it ("C-4", "01", "..").

return function(M, object)

local NOTE_NAMES = { "C-", "C#", "D-", "D#", "E-", "F-", "F#", "G-", "G#", "A-", "A#", "B-" }

local function note_text(value)
  if value == 121 then return "---" end
  if value == 120 then return "OFF" end
  return NOTE_NAMES[value % 12 + 1] .. math.floor(value / 12)
end

local function hex2(value)
  return string.format("%02X", value)
end

local function out_of_range(what, index)
  error("'" .. what .. "' index out of range: " .. tostring(index), 3)
end

local function list(items)
  local copy = {}
  for i, item in ipairs(items) do copy[i] = item end
  return copy
end

-- Undo: what a song's writes through the API are, as pairs of functions, and the steps they make. One callback (a timer, a
-- socket message) is one step; a write outside any callback is a step of its own. `describe_undo` names the step the
-- next write makes. Mute, solo, loop and selection are not recorded: they are no undo steps in Renoise.

local function new_undo()
  return { stack = {}, redo = {}, depth = 0, current = nil, desc = nil, redoing = false }
end

local function commit(undo)
  if undo.current and #undo.current > 0 then
    undo.stack[#undo.stack + 1] = { desc = undo.desc or "Edit", writes = undo.current }
    undo.redo = {}
  end
  undo.current, undo.desc = nil, nil
end

function M.callback_begin()
  local undo = M.data and M.data.undo
  if undo then undo.depth = undo.depth + 1 end
end

function M.callback_end()
  local undo = M.data and M.data.undo
  if not undo then return end
  undo.depth = undo.depth - 1
  if undo.depth <= 0 then
    undo.depth = 0
    commit(undo)
  end
end

-- A write that Renoise records: how to take it back, and how to do it again.
function M.record(undo_fn, redo_fn)
  local undo = M.data and M.data.undo
  if not undo or undo.redoing then return end
  undo.current = undo.current or {}
  undo.current[#undo.current + 1] = { undo = undo_fn, redo = redo_fn }
  if undo.depth == 0 then commit(undo) end
end

-- A track's first device, the mixer: its parameters are the track's volume, panning and width, which have automation like any.
function M.mixer_device_data()
  return {
    name = "Track Vol/Pan", path = "Audio/Effects/Native/TrackVolPan",
    params = {
      { name = "Panning", value = 0.5, min = 0, max = 1 }, { name = "Volume", value = 1, min = 0, max = 1.4125 },
      { name = "Width", value = 0, min = 0, max = 1 }, { name = "Post Panning", value = 0.5, min = 0, max = 1 },
      { name = "Post Volume", value = 1, min = 0, max = 1.4125 },
    },
  }
end

-- Columns

local CELL_FIELDS = {
  note_value = "note", instrument_value = "instrument", volume_value = "volume", panning_value = "panning",
  delay_value = "delay", effect_number_value = "fxn", effect_amount_value = "fxa",
}
local EFFECT_FIELDS = { number_value = "number", amount_value = "amount" }

-- Letter commands: the volume and panning columns and the effect number take a letter and a digit ("G3", "0Z", "ZB") as well as two hex
-- digits. The mock stores a command as 256 + 36 * first + second and shows the text again.
local DIGITS = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ"
local function command_value(text)
  local first, second = DIGITS:find(text:sub(1, 1), 1, true), DIGITS:find(text:sub(2, 2), 1, true)
  return 256 + (first - 1) * 36 + (second - 1)
end
local function command_text(value)
  local code = value - 256
  local first, second = math.floor(code / 36) + 1, code % 36 + 1
  return DIGITS:sub(first, first) .. DIGITS:sub(second, second)
end
local function column_text(value, none)
  if value == none then return ".." end
  if value >= 256 then return command_text(value) end
  return string.format("%02X", value)
end

-- The text Renoise reads into a cell's value, and the error it raises for text that isn't one.
local SEMITONES = { C = 0, ["C#"] = 1, D = 2, ["D#"] = 3, E = 4, F = 5, ["F#"] = 6, G = 7, ["G#"] = 8, A = 9, ["A#"] = 10, B = 11 }
local function cell_value(member, text)
  if type(text) ~= "string" then error("bad argument: string expected", 4) end
  if member == "note_string" then
    if text == "---" then return 121 elseif text == "OFF" then return 120 end
    local letter, sep, octave = text:match("^([A-G])([-#])(%d)$")
    local semitone = letter and SEMITONES[letter .. (sep == "#" and "#" or "")]
    if semitone then return tonumber(octave) * 12 + semitone end
  elseif member == "instrument_string" then
    if text == ".." then return 255 end
    if text:match("^%x%x$") then return tonumber(text, 16) end
  elseif member == "volume_string" or member == "panning_string" then
    if text == ".." then return 255 end
    if text:match("^%x%x$") then return tonumber(text, 16) end
    if text:match("^[G-Z]%x$") then return command_value(text) end
  elseif member == "delay_string" then
    if text == ".." then return 0 end
    if text:match("^%x%x$") then return tonumber(text, 16) end
  elseif member == "effect_number_string" or member == "number_string" then
    if text:match("^%x%x$") then return tonumber(text, 16) end
    -- (the effect commands Renoise has: 0 and a letter, or Z and one of its six)
    if text:match("^0[AUDGIOCQMSBRYVTNELPWXJHKZ]$") or text:match("^Z[TLKGBD]$") then return command_value(text) end
  elseif member == "effect_amount_string" or member == "amount_string" then
    if text:match("^%x%x$") then return tonumber(text, 16) end
  end
  error("invalid value '" .. text .. "' for " .. member, 4)
end

local STRING_MEMBERS = {
  note_string = "note_value", instrument_string = "instrument_value", volume_string = "volume_value",
  panning_string = "panning_value", delay_string = "delay_value", effect_number_string = "effect_number_value",
  effect_amount_string = "effect_amount_value", number_string = "number_value", amount_string = "amount_value",
}

-- Setters for the members of a column: `w.cell()` is the data to write (made if there is none), `w.changed()` tells the
-- pattern's line notifiers.
local function cell_setters(w, fields)
  local out = {}
  for member, field in pairs(fields) do
    out[member] = function(value)
      local cell = w.cell()
      local old = cell[field]
      if old == value then return end
      cell[field] = value
      w.changed()
      M.record(function() cell[field] = old; w.changed() end, function() cell[field] = value; w.changed() end)
    end
  end
  for member, value_member in pairs(STRING_MEMBERS) do
    if out[value_member] then
      out[member] = function(text) out[value_member](cell_value(member, text)) end
    end
  end
  return out
end

local function note_column(data, w)
  local function cur() return w and w.peek() or data end
  local setters = w and cell_setters(w, CELL_FIELDS) or nil
  return object("renoise.NoteColumn", { is_selected = false,
    clear = function()
      if not setters then return end
      for member, value in pairs({ note_value = 121, instrument_value = 255, volume_value = 255, panning_value = 255,
        delay_value = 0, effect_number_value = 0, effect_amount_value = 0 }) do setters[member](value) end
    end,
  }, {
    note_value = function() return cur().note end,
    note_string = function() return note_text(cur().note) end,
    instrument_value = function() return cur().instrument end,
    instrument_string = function() return cur().instrument == 255 and ".." or hex2(cur().instrument) end,
    volume_value = function() return cur().volume end,
    volume_string = function() return column_text(cur().volume, 255) end,
    panning_value = function() return cur().panning end,
    panning_string = function() return column_text(cur().panning, 255) end,
    delay_value = function() return cur().delay end,
    delay_string = function() return cur().delay == 0 and ".." or hex2(cur().delay) end,
    effect_number_value = function() return cur().fxn end,
    effect_number_string = function() return column_text(cur().fxn) end,
    effect_amount_value = function() return cur().fxa end,
    effect_amount_string = function() return hex2(cur().fxa) end,
    is_empty = function()
      local c = cur()
      return c.note == 121 and c.instrument == 255 and c.volume == 255 and c.panning == 255
        and c.delay == 0 and c.fxn == 0 and c.fxa == 0
    end,
  }, setters)
end

local function effect_column(data, w)
  local function cur() return w and w.peek() or data end
  local setters = w and cell_setters(w, EFFECT_FIELDS) or nil
  return object("renoise.EffectColumn", { is_selected = false,
    clear = function()
      if not setters then return end
      setters.number_value(0)
      setters.amount_value(0)
    end,
  }, {
    number_value = function() return cur().number end,
    number_string = function() return column_text(cur().number) end,
    amount_value = function() return cur().amount end,
    amount_string = function() return hex2(cur().amount) end,
    is_empty = function() return cur().number == 0 and cur().amount == 0 end,
  }, setters)
end

local function empty_note() return { note = 121, instrument = 255, volume = 255, panning = 255, delay = 0, fxn = 0, fxa = 0 } end

-- Notes are given as {note, instrument, volume, panning, delay, fxn, fxa} (any of them).
local function note_data(given)
  local data = empty_note()
  for k, v in pairs(given or {}) do data[k] = v end
  return data
end

-- A line of sparse data: { notes = {[column] = data}, effects = {[column] = data} }. `ctx` (a pattern's line, which can be
-- written): { current() -> the line's data or nil, ensure() -> its data, made if there is none, changed() }.
local function line(data, ctx)
  data = data or {}
  local function now() return (ctx and ctx.current()) or data end
  local function empty()
    local d = now()
    for _, n in pairs(d.notes or {}) do
      if not note_column(n).is_empty then return false end
    end
    for _, e in pairs(d.effects or {}) do
      if e.number ~= 0 or e.amount ~= 0 then return false end
    end
    return true
  end
  return object("renoise.PatternLine", {
    note_column = function(_, c)
      if type(c) ~= "number" or c < 1 or c > 12 then out_of_range("note column", c) end
      local w = ctx and {
        peek = function() return ((ctx.current() or {}).notes or {})[c] or empty_note() end,
        cell = function()
          local real = ctx.ensure()
          real.notes = real.notes or {}
          real.notes[c] = real.notes[c] or empty_note()
          return real.notes[c]
        end,
        changed = ctx.changed,
      }
      return note_column((now().notes or {})[c] or empty_note(), w)
    end,
    effect_column = function(_, c)
      if type(c) ~= "number" or c < 1 or c > 8 then out_of_range("effect column", c) end
      local w = ctx and {
        peek = function() return ((ctx.current() or {}).effects or {})[c] or { number = 0, amount = 0 } end,
        cell = function()
          local real = ctx.ensure()
          real.effects = real.effects or {}
          real.effects[c] = real.effects[c] or { number = 0, amount = 0 }
          return real.effects[c]
        end,
        changed = ctx.changed,
      }
      return effect_column((now().effects or {})[c] or { number = 0, amount = 0 }, w)
    end,
  }, { is_empty = empty })
end

-- Devices

local function parameter(data, set)
  data.value_observable = data.value_observable or M.observable()
  local proxy = object("renoise.DeviceParameter", {
    name_observable = data.name_observable or M.observable(),
    value_min = data.min or 0, value_max = data.max or 1, value_default = data.default or 0,
    value_quantum = data.quantum or 0,
    value_observable = data.value_observable or M.observable(),
    value_string_observable = M.observable(),
  }, {
    -- (M4 n-song: whether a pattern has automation of it, as the song's data says now; every parameter can be automated
    -- unless a test says otherwise)
    is_automated = function() return data.automated == true end,
    -- (counted: how many names a request reads, as a test asks)
    name = function()
      M.parameter_name_reads = (M.parameter_name_reads or 0) + 1
      return data.name
    end,
    is_automatable = function() return data.automatable ~= false end,
    value = function() return data.value end,
    value_string = function() return data.string and data.string(data.value) or string.format("%.2f", data.value) end,
  }, set)
  M.impl(proxy).__data = data
  return proxy
end

-- What writing a device through the API does: its data changes, its observable fires, an undo step is recorded.
local function device_setters(data)
  local function setter(member, field, observable, transform)
    return function(value)
      if member == "active_preset_data" then
        if data.ignore_preset_data == value then return end
        if data.reject_preset_data == value then
          data.preset_data = "partial state"
          error("plug-in rejected state")
        end
        if data.normalize_preset_data then value = data.normalize_preset_data(value) end
      end
      local old = data[field]
      data[field] = transform and transform(value) or value
      local new = data[field]
      if observable and data[observable] then M.fire(data[observable]) end
      M.record(function()
        data[field] = old
        if observable and data[observable] then M.fire(data[observable]) end
      end, function()
        data[field] = new
        if observable and data[observable] then M.fire(data[observable]) end
      end)
    end
  end
  return {
    is_active = setter("is_active", "active", "is_active_observable"),
    display_name = setter("display_name", "display_name", "display_name_observable"),
    is_maximized = setter("is_maximized", "maximized", "is_maximized_observable"),
    active_preset_data = setter("active_preset_data", "preset_data"),
  }
end

-- (M4 n-devices) A device whose data names its XML slot type (`slot`) has preset data as Renoise's: XML made from its
-- parameters (each saved in the element its spec names, `element`), and a write sets the parameters from the XML. What
-- Renoise does with other text (measured, 3.5.4): text that isn't XML raises, as does an empty one; another device type's
-- data crashes Renoise; after a write `active_preset` reads 0. `xml_body` is the rest of the slot (a Doofer's chain...).
local function number_text(v) return string.format("%.9g", v) end

function M.preset_xml(data)
  local lines = { '<?xml version="1.0" encoding="UTF-8"?>', '<FilterDevicePreset doc_version="14">', '  <DeviceSlot type="' .. data.slot .. '">',
    "    <IsMaximized>true</IsMaximized>" }
  for _, p in ipairs(data.params or {}) do
    -- (an element the rest of the slot holds already, a Doofer's macro, is that one)
    if p.element and p.element ~= "" and not (data.xml_body and data.xml_body:find("<" .. p.element .. ">", 1, true)) then
      lines[#lines + 1] = "    <" .. p.element .. ">"
      lines[#lines + 1] = "      <Value>" .. number_text(p.value) .. "</Value>"
      lines[#lines + 1] = "    </" .. p.element .. ">"
    end
  end
  if data.xml_body then lines[#lines + 1] = data.xml_body end
  lines[#lines + 1] = "  </DeviceSlot>"
  lines[#lines + 1] = "</FilterDevicePreset>"
  return table.concat(lines, "\n") .. "\n"
end

-- The XML as it reads now: what was written (or made), with the parameters' values as they are.
local function preset_text(data)
  local text = data.preset_data or M.preset_xml(data)
  for _, p in ipairs(data.params or {}) do
    if p.element and p.element ~= "" then
      text = text:gsub("(<" .. p.element .. ">%s*<Value>)[^<]*(</Value>)", "%1" .. number_text(p.value) .. "%2", 1)
    end
  end
  return text
end

local function write_preset_xml(data, text)
  if type(text) ~= "string" or text == "" then error("exception: 'Failed to parse a memory XML file (The document is empty)!'", 3) end
  local slot = text:match('<DeviceSlot%s+type="([%w_]+)"')
  if not text:find("<FilterDevicePreset", 1, true) or not slot or not text:find("</FilterDevicePreset>", 1, true) then
    error("exception: 'Failed to parse a memory XML file ( ^)!'", 3)
  end
  local version = tonumber(text:match('doc_version="(%d+)"'))
  if version and version > 14 then error("exception: ''[BinStream]' was saved with an incompatible, more recent version of Renoise.'", 3) end
  if slot ~= data.slot then
    M.crashes[#M.crashes + 1] = "wrote " .. slot .. " preset data to a " .. data.slot .. " (Renoise crashes)"
    return
  end
  if M.on_preset_write then M.on_preset_write(data, text) end
  local old_params, old_text, old_preset = {}, data.preset_data, data.preset
  for k, p in ipairs(data.params or {}) do old_params[k] = p.value end
  local function apply(params, preset_text_, preset)
    for k, p in ipairs(data.params or {}) do
      if params[k] ~= nil and p.value ~= params[k] then
        p.value = params[k]
        M.fire(p.value_observable)
      end
    end
    data.preset_data, data.preset = preset_text_, preset
  end
  local new_params = {}
  for k, p in ipairs(data.params or {}) do
    if p.element and p.element ~= "" then
      local v = tonumber(text:match("<" .. p.element .. ">%s*<Value>([^<]*)</Value>"))
      if v then new_params[k] = v end
    end
  end
  apply(new_params, text, 0)
  M.record(function() apply(old_params, old_text, old_preset) end, function() apply(new_params, text, 0) end)
end

-- Choosing a preset: its parameter values (`preset_values[n]`, by parameter) and the XML they make; an undo step.
local function choose_preset(data, n)
  if type(n) ~= "number" or n < 0 or n > #(data.presets or {}) then error("std::logic_error: 'invalid preset index'", 3) end
  local old_params, old_text, old_preset = {}, data.preset_data, data.preset
  for k, p in ipairs(data.params or {}) do old_params[k] = p.value end
  local values = (data.preset_values or {})[n] or {}
  local function apply(params, text, preset)
    for k, p in ipairs(data.params or {}) do
      if params[k] ~= nil and p.value ~= params[k] then
        p.value = params[k]
        M.fire(p.value_observable)
      end
    end
    data.preset_data, data.preset = text, preset
    M.fire(data.active_preset_observable)
  end
  apply(values, nil, n)
  M.record(function() apply(old_params, old_text, old_preset) end, function() apply(values, nil, n) end)
end

local function device(data)
  local params = {}
  for i, p in ipairs(data.params or {}) do
    local function write(value)
      local old = p.value
      if old == value then return end
      p.value = value
      M.fire(p.value_observable)
      M.record(function() p.value = old; M.fire(p.value_observable) end, function() p.value = value; M.fire(p.value_observable) end)
    end
    params[i] = parameter(p, {
      value = write,
      value_string = function(text)
        local number = tonumber(tostring(text):match("-?%d+%.?%d*"))
        if not number then error("could not parse " .. tostring(text), 2) end
        write(number)
      end,
    })
  end
  -- (the proxy is made anew on every read: what is watched lives in the data)
  data.is_active_observable = data.is_active_observable or M.observable()
  data.display_name_observable = data.display_name_observable or M.observable()
  data.is_maximized_observable = data.is_maximized_observable or M.observable()
  data.active_preset_observable = data.active_preset_observable or M.observable()
  local proxy = object("renoise.AudioDevice", {
    name = data.name, display_name = data.display_name or data.name, short_name = data.short_name or data.name,
    -- (M4 n-devices: a device with an editor window of its own, `editor` in the data)
    external_editor_available = data.editor == true,
    device_path = data.path or ("Audio/Effects/Native/" .. data.name),
    is_active = data.active ~= false, is_active_observable = data.is_active_observable,
    display_name_observable = data.display_name_observable, is_maximized_observable = data.is_maximized_observable,
    active_preset = data.preset or 0, active_preset_observable = data.active_preset_observable,
    is_maximized = data.maximized == true,
    active_preset_data = data.preset_data or "",
    presets = data.presets or {},
    parameters = params,
    preset = function(_, i)
      local name = (data.presets or {})[i]
      if not name then out_of_range("preset", i) end
      return name
    end,
    parameter = function(_, i)
      if not params[i] then out_of_range("parameter", i) end
      return params[i]
    end,
  }, {
    -- (read from the data: the proxy a caller holds sees a write made through it)
    is_active = function() return data.active ~= false end,
    display_name = function() return data.display_name or data.name end,
    is_maximized = function() return data.maximized == true end,
    active_preset_data = function()
      M.preset_reads = (M.preset_reads or 0) + 1 -- (counted: how often the tool reads a device's data, as a test asks)
      if data.slot then return preset_text(data) end
      return data.preset_data or ""
    end,
    active_preset = function() return data.preset or 0 end,
    external_editor_visible = function() return data.editor_visible == true end,
  }, (function()
    local setters = device_setters(data)
    setters.external_editor_visible = function(visible)
      if not data.editor then error("the device has no external editor", 2) end
      data.editor_visible = visible
    end
    -- (M4 n-devices: a device with an XML slot type takes preset data as Renoise does, and a preset choice sets its values)
    if data.slot then setters.active_preset_data = function(text) write_preset_xml(data, text) end end
    setters.active_preset = function(n)
      -- (a test's Renoise that doesn't take a preset, as a plug-in may not)
      if M.presets_refused then return end
      choose_preset(data, n)
    end
    return setters
  end)())
  return proxy
end

-- (n-instruments: the instruments' mock, which builds on the helpers above)
assert(loadfile(M.tests_dir .. "mock_instruments.lua"))()(M, object, {
  line = line, device = device, parameter = parameter, out_of_range = out_of_range, list = list, note_data = note_data,
})

-- The observable of a note column's mute or name: one per column, kept in the track's data.
local function column_observable(t, kind, c)
  if type(c) ~= "number" or c < 1 or c > 12 or t.kind == "master" or t.kind == "send" then out_of_range("column", c) end
  t.column_observables = t.column_observables or {}
  local key = kind .. c
  t.column_observables[key] = t.column_observables[key] or M.observable()
  return t.column_observables[key]
end

-- A song

function M.make_song(options)
  options = options or {}
  local d = { options = options, undo = new_undo() }
  M.data = d

  local function simple_observable() return M.observable() end

  -- Tracks: sequencer tracks (a group is one, after its members), then the master, then the sends.
  d.tracks = {}
  local sequencer_count = options.sequencer_tracks or 1
  local specs = options.tracks
  if specs then sequencer_count = #specs end
  for i = 1, sequencer_count do
    local spec = specs and specs[i] or {}
    d.tracks[i] = {
      kind = spec.kind or "sequencer", name = spec.name or string.format("Track %02d", i), color = spec.color,
      members = spec.members or 0, mute_state = spec.mute_state or 1, solo = spec.solo or false,
      volume = spec.volume or 1, panning = spec.panning or 0.5,
      note_columns = spec.note_columns or 1, effect_columns = spec.effect_columns or 1, devices = spec.devices or {},
    }
  end
  d.tracks[#d.tracks + 1] = { kind = "master", name = "Master", members = 0, mute_state = 1, volume = 1, panning = 0.5,
    note_columns = 1, effect_columns = 1, devices = {} }
  for i = 1, options.send_tracks or 0 do
    d.tracks[#d.tracks + 1] = { kind = "send", name = "Send " .. i, members = 0, mute_state = 1, volume = 1, panning = 0.5,
      note_columns = 1, effect_columns = 1, devices = {} }
  end
  -- (M4 n-song: a track given as soloed is a solo's root; Renoise's other tracks follow it, see M.solo_set)
  d.solo_roots = {}
  for _, t in ipairs(d.tracks) do
    if t.solo then d.solo_roots[#d.solo_roots + 1] = t end
  end
  for _, t in ipairs(d.tracks) do
    -- (Renoise's first device on every track is its mixer device, which can't be deleted or moved)
    if options.mixer_device then table.insert(t.devices, 1, M.mixer_device_data()) end
    t.name_observable, t.color_observable, t.mute_state_observable = simple_observable(), simple_observable(), simple_observable()
    t.solo_state_observable, t.devices_observable = simple_observable(), simple_observable()
    t.visible_note_columns_observable, t.visible_effect_columns_observable = simple_observable(), simple_observable()
    t.volume_observable, t.panning_observable = simple_observable(), simple_observable()
  end

  -- Patterns
  d.patterns = {}
  local function new_pattern(lines, name)
    local pattern = {
      name = name or "", lines = lines or options.pattern_lines or 64, tracks = {},
      name_observable = simple_observable(), number_of_lines_observable = simple_observable(), notifiers = {},
    }
    d.patterns[#d.patterns + 1] = pattern
    return pattern
  end
  for _ = 1, options.patterns or 1 do new_pattern() end

  -- The sequence: pattern indices; sections and muted slots hang off the slot
  d.sequence = {}
  for s = 1, options.sequence_length or #d.patterns do d.sequence[s] = options.sequence and options.sequence[s] or ((s - 1) % #d.patterns + 1) end
  d.sections, d.muted, d.section_names = {}, {}, {}
  d.new_pattern = new_pattern

  -- Instruments (n-instruments: mock_instruments.lua builds them)
  d.instruments = {}
  local function new_instrument(spec)
    local instrument = M.instrument_data(spec)
    d.instruments[#d.instruments + 1] = instrument
    return instrument
  end
  d.new_instrument = new_instrument
  for i = 1, options.instruments or 1 do new_instrument(options.instrument_specs and options.instrument_specs[i]) end

  -- Proxies. The song's own, built on every call from the data, so nothing is cached across a restructure.
  local song
  local track, pattern_object, instrument_object

  local function track_count() return #d.tracks end

  -- The pre-FX volume and panning are plain parameters: their value reads the data.
  -- A field of a track's data written through the API: the data changes, the observable fires and, unless it is one Renoise
  -- makes no undo step for, a step is recorded.
  local function recorded(t, field, observable, not_a_step, copy)
    return function(value)
      if copy then value = copy(value) end
      local old = t[field]
      if old == value then return end
      t[field] = value
      if observable and t[observable] then M.fire(t[observable]) end
      if not not_a_step then
        M.record(function()
          t[field] = old
          if observable and t[observable] then M.fire(t[observable]) end
        end, function()
          t[field] = value
          if observable and t[observable] then M.fire(t[observable]) end
        end)
      end
    end
  end

  local function prefx(t, field)
    local observable = field == "volume" and "volume_observable" or "panning_observable"
    return object("renoise.DeviceParameter", {
      name = field == "volume" and "Volume" or "Panning", name_observable = M.observable(),
      value_min = 0, value_max = field == "volume" and 1.4125375747681 or 1, value_default = field == "volume" and 1 or 0.5,
      value_observable = t[observable], value_string_observable = M.observable(),
      is_automated = false,
    }, {
      value = function() return t[field == "volume" and "volume" or "panning"] end,
      value_string = function() return string.format("%.2f", t[field == "volume" and "volume" or "panning"]) end,
    }, { value = recorded(t, field == "volume" and "volume" or "panning", observable) })
  end

  -- The post-FX fader and pan and the pre-FX width: plain parameters too.
  local function level(t, key, default, max)
    return object("renoise.DeviceParameter", {
      name = key, name_observable = M.observable(), value_min = 0, value_max = max, value_default = default,
      -- (M4 n-song: the value's observable is the track's, so a watcher sees writes and Renoise's undo of them)
      value_observable = (function() t[key .. "_observable"] = t[key .. "_observable"] or M.observable() return t[key .. "_observable"] end)(),
      value_string_observable = M.observable(), is_automated = false,
    }, {
      value = function() return t[key] or default end,
      value_string = function() return string.format("%.2f", t[key] or default) end,
    }, { value = recorded(t, key, key .. "_observable") })
  end

  local KIND = { sequencer = 1, master = 2, send = 3, group = 4 }

  track = function(i)
    local t = type(i) == "number" and d.tracks[i]
    if not t then out_of_range("track", i) end
    local fields = {
      name_observable = t.name_observable, color_observable = t.color_observable,
      mute_state_observable = t.mute_state_observable, solo_state_observable = t.solo_state_observable,
      devices_observable = t.devices_observable,
      visible_note_columns_observable = t.visible_note_columns_observable,
      visible_effect_columns_observable = t.visible_effect_columns_observable,
      prefx_volume = prefx(t, "volume"), prefx_panning = prefx(t, "panning"),
      prefx_width = level(t, "width", 0, 127), postfx_volume = level(t, "post_volume", 1, 1.4125375747681),
      postfx_panning = level(t, "post_panning", 0.5, 1),
      column_is_muted = function(_, c)
        column_observable(t, "muted", c)
        return (t.muted_columns or {})[c] == true
      end,
      column_name = function(_, c)
        column_observable(t, "name", c)
        return (t.column_names or {})[c] or ""
      end,
      column_is_muted_observable = function(_, c) return column_observable(t, "muted", c) end,
      column_name_observable = function(_, c) return column_observable(t, "name", c) end,
      device = function(_, k)
        if not t.devices[k] then out_of_range("device", k) end
        return device(t.devices[k])
      end,
      color_blend_observable = (function() t.color_blend_observable = t.color_blend_observable or M.observable() return t.color_blend_observable end)(),
      -- M4 (n-song): the observables of what the track kinds change
      collapsed_observable = (function() t.collapsed_observable = t.collapsed_observable or M.observable() return t.collapsed_observable end)(),
      output_routing_observable = (function() t.output_routing_observable = t.output_routing_observable or M.observable() return t.output_routing_observable end)(),
      output_delay_observable = (function() t.output_delay_observable = t.output_delay_observable or M.observable() return t.output_delay_observable end)(),
      volume_column_visible_observable = (function() t.volume_column_observable = t.volume_column_observable or M.observable() return t.volume_column_observable end)(),
      panning_column_visible_observable = (function() t.panning_column_observable = t.panning_column_observable or M.observable() return t.panning_column_observable end)(),
      delay_column_visible_observable = (function() t.delay_column_observable = t.delay_column_observable or M.observable() return t.delay_column_observable end)(),
      sample_effects_column_visible_observable = (function() t.sample_effects_column_observable = t.sample_effects_column_observable or M.observable() return t.sample_effects_column_observable end)(),
      set_column_name = function(_, c, name)
        column_observable(t, "name", c)
        t.column_names = t.column_names or {}
        local old = t.column_names[c]
        t.column_names[c] = name ~= "" and name or nil
        M.fire(column_observable(t, "name", c))
        M.record(function() t.column_names[c] = old; M.fire(column_observable(t, "name", c)) end,
          function() t.column_names[c] = name ~= "" and name or nil; M.fire(column_observable(t, "name", c)) end)
      end,
      -- (mute and solo are no undo steps: the mute uses the prefs' default mute state, Off here)
      mute = function()
        if t.kind == "master" then error("std::logic_error: 'can't mute the master track'", 2) end
        t.mute_state = 2
        M.fire(t.mute_state_observable)
      end,
      unmute = function()
        t.mute_state = 1
        M.fire(t.mute_state_observable)
      end,
      solo = function() M.solo_set(t, true) end,
      set_column_is_muted = function(_, c, muted)
        column_observable(t, "muted", c)
        t.muted_columns = t.muted_columns or {}
        local old = t.muted_columns[c]
        t.muted_columns[c] = muted or nil
        M.fire(column_observable(t, "muted", c))
        M.record(function() t.muted_columns[c] = old; M.fire(column_observable(t, "muted", c)) end,
          function() t.muted_columns[c] = muted or nil; M.fire(column_observable(t, "muted", c)) end)
      end,
      insert_device_at = function(_, path, index)
        if type(index) ~= "number" or index < 1 or index > #t.devices + 1 then out_of_range("device", index) end
        if d.options.mixer_device and index == 1 then error("the mixer device must stay first", 2) end
        if path:find("Missing", 1, true) then error("std::logic_error: 'unknown device " .. path .. "'", 2) end
        local data = { name = path:match("([^/]+)$") or path, path = path, params = {}, active = true }
        -- (a device made from a path has the parameters of that device: tests say which in M.device_catalog)
        for k, template in ipairs((M.device_catalog or {})[path] or {}) do
          data.params[k] = { name = template.name, value = template.value, min = template.min, max = template.max, element = template.element }
        end
        -- (M4 n-devices: a device whose XML slot type a test names takes preset data as Renoise does)
        local slot = (M.device_slots or {})[path]
        if slot then data.slot, data.xml_body = slot.slot, slot.body end
        table.insert(t.devices, index, data)
        M.follow_device_routes(t, index, false)
        M.fire(t.devices_observable, { type = "insert", index = index })
        M.record(function()
          for i, existing in ipairs(t.devices) do
            if existing == data then table.remove(t.devices, i) M.fire(t.devices_observable, { type = "remove", index = i }) break end
          end
        end, function()
          table.insert(t.devices, math.min(index, #t.devices + 1), data)
          M.fire(t.devices_observable, { type = "insert", index = math.min(index, #t.devices) })
        end)
        return device(data)
      end,
      swap_devices_at = function(_, a, b)
        if type(a) ~= "number" or type(b) ~= "number" or not t.devices[a] or not t.devices[b] then out_of_range("device", a) end
        if d.options.mixer_device and (a == 1 or b == 1) then error("the mixer device must stay first", 2) end
        local function swap()
          t.devices[a], t.devices[b] = t.devices[b], t.devices[a]
          M.fire(t.devices_observable, { type = "swap", index1 = a, index2 = b })
        end
        swap()
        M.record(swap, swap)
      end,
      delete_device_at = function(_, index)
        if type(index) ~= "number" or not t.devices[index] then out_of_range("device", index) end
        if d.options.mixer_device and index == 1 then error("the mixer device can't be deleted", 2) end
        M.follow_device_routes(t, index, true)
        local data = table.remove(t.devices, index)
        -- (Renoise takes a deleted device's automation with it, in every pattern; its undo brings it back)
        local taken = {}
        for _, pattern in ipairs(M.data.patterns) do
          for _, pattern_track in pairs(pattern.tracks) do
            for i = #(pattern_track.automation or {}), 1, -1 do
              local entry = pattern_track.automation[i]
              for _, parameter in ipairs(data.params) do
                if entry.parameter == parameter then
                  table.remove(pattern_track.automation, i)
                  taken[#taken + 1] = { pattern_track, entry }
                  break
                end
              end
            end
          end
        end
        M.fire(t.devices_observable, { type = "remove", index = index })
        M.record(function()
          table.insert(t.devices, math.min(index, #t.devices + 1), data)
          for _, pair in ipairs(taken) do table.insert(pair[1].automation, pair[2]) end
          M.fire(t.devices_observable, { type = "insert", index = math.min(index, #t.devices) })
        end, function()
          for _, pair in ipairs(taken) do
            for i, entry in ipairs(pair[1].automation) do
              if entry == pair[2] then table.remove(pair[1].automation, i) break end
            end
          end
          for i, existing in ipairs(t.devices) do
            if existing == data then table.remove(t.devices, i) M.fire(t.devices_observable, { type = "remove", index = i }) break end
          end
        end)
      end,
    }
    local getters = {
      name = function() return t.name end,
      -- (the devices a track can be given: tests say which in M.device_catalog; each is {path, name} to Renoise)
      available_devices = function()
        local paths = {}
        for path in pairs(M.device_catalog or {}) do paths[#paths + 1] = path end
        table.sort(paths)
        return paths
      end,
      available_device_infos = function()
        local infos = {}
        for path in pairs(M.device_catalog or {}) do
          infos[#infos + 1] = { path = path, name = "Effects: " .. (path:match("([^/]+)$") or path), short_name = path:match("([^/]+)$") or path }
        end
        table.sort(infos, function(x, y) return x.path < y.path end)
        return infos
      end,
      color = function() return t.color or M.NIL end,
      type = function() return KIND[t.kind] end,
      mute_state = function() return t.mute_state end,
      solo_state = function() return t.solo or false end,
      visible_note_columns = function() return t.note_columns end,
      visible_effect_columns = function() return t.effect_columns end,
      max_note_columns = function() return (t.kind == "master" or t.kind == "send" or t.kind == "group") and 0 or 12 end,
      max_effect_columns = function() return t.kind == "master" and 8 or 8 end,
      collapsed = function() return t.collapsed == true end,
      output_routing = function()
        if t.output_routing then return t.output_routing end
        if t.kind == "master" then return "Built-in Output" end
        -- (a group's track goes to its group)
        for g = i + 1, #d.tracks do
          if d.tracks[g].kind == "group" and g - d.tracks[g].members <= i then return d.tracks[g].name end
        end
        return "Master"
      end,
      output_delay = function() return t.output_delay or 0 end,
      volume_column_visible = function() return t.volume_column == true end,
      panning_column_visible = function() return t.panning_column == true end,
      delay_column_visible = function() return t.delay_column == true end,
      sample_effects_column_visible = function() return t.sample_effects_column == true end,
      available_output_routings = function()
        if t.kind == "master" then return { "Built-in Output" } end
        local routings = {}
        for g = i + 1, #d.tracks do
          if d.tracks[g].kind == "group" and g - d.tracks[g].members <= i then routings[#routings + 1] = d.tracks[g].name break end
        end
        routings[#routings + 1] = "Master"
        routings[#routings + 1] = "Built-in Output"
        return routings
      end,
      min_effect_columns = function() return t.kind == "sequencer" and 0 or 1 end,
      group_parent = function()
        for g = i + 1, #d.tracks do
          if d.tracks[g].kind == "group" and g - d.tracks[g].members <= i then return track(g) end
        end
        return nil
      end,
      devices = function()
        local devices = {}
        for k, spec in ipairs(t.devices) do devices[k] = device(spec) end
        return devices
      end,
    }
    local class = "renoise.Track"
    if t.kind == "group" then
      class = "renoise.GroupTrack"
      getters.members = function()
        local members = {}
        for k = 1, t.members do members[k] = track(i - k) end
        return members
      end
      getters.group_collapsed = function() return t.group_collapsed == true end
    end
    -- Every track has a colour in Renoise (it assigns one), and a blend of how much of it shows.
    getters.color = function() return t.color or (t.kind == "master" and { 220, 220, 220 } or { 41, 166, 153 }) end
    getters.color_blend = function() return t.color_blend or 0 end
    local setters = {
      name = recorded(t, "name", "name_observable"),
      color = recorded(t, "color", "color_observable", false, function(v) return { v[1], v[2], v[3] } end),
      color_blend = recorded(t, "color_blend", "color_blend_observable"),
      mute_state = recorded(t, "mute_state", "mute_state_observable", true),
      -- (M4 n-song: Renoise solos by rewriting every track's mute state, see M.solo_set)
      solo_state = function(value) M.solo_set(t, value) end,
      visible_note_columns = recorded(t, "note_columns", "visible_note_columns_observable"),
      visible_effect_columns = recorded(t, "effect_columns", "visible_effect_columns_observable"),
      volume_column_visible = recorded(t, "volume_column", "volume_column_observable"),
      panning_column_visible = recorded(t, "panning_column", "panning_column_observable"),
      delay_column_visible = recorded(t, "delay_column", "delay_column_observable"),
      sample_effects_column_visible = recorded(t, "sample_effects_column", "sample_effects_column_observable"),
      collapsed = recorded(t, "collapsed", "collapsed_observable"),
      output_routing = function(value)
        local known = false
        for _, name in ipairs(getters.available_output_routings()) do known = known or name == value end
        if not known then error("std::logic_error: 'unknown output routing string. pass one of 'available_output_routings''", 2) end
        recorded(t, "output_routing", "output_routing_observable")(value)
      end,
      output_delay = function(value)
        if t.kind ~= "sequencer" then error("std::logic_error: 'can not change the output_delay of group, master or send tracks.'", 2) end
        if value < -100 or value > 100 then error("std::logic_error: 'invalid output_delay value'", 2) end
        recorded(t, "output_delay", "output_delay_observable")(value)
      end,
      group_collapsed = recorded(t, "group_collapsed"),
    }
    return object(class, fields, getters, setters)
  end

  local function pattern_track(p, t)
    local pattern = d.patterns[p]
    pattern.tracks[t] = pattern.tracks[t] or { alias = 0, lines = {} }
    local data = pattern.tracks[t]
    data.alias_observable = data.alias_observable or M.observable()
    -- M4 (n-song): the slot colour and the list of automation have observables of their own
    data.color_observable = data.color_observable or M.observable()
    data.automation_observable = data.automation_observable or M.observable()
    data.t = t
    local made = object("renoise.PatternTrack", {
      alias_pattern_index_observable = data.alias_observable,
      color_observable = data.color_observable, automation_observable = data.automation_observable,
      -- an alias shows (and writes to) the lines of the pattern it is an alias of; lines past the pattern's end are kept
      -- (measured, 3.5.4) and reached up to the most a pattern has
      line = function(_, k)
        if type(k) ~= "number" or k < 1 or k > 512 then out_of_range("line", k) end
        M.lines_read = (M.lines_read or 0) + 1
        local source_pattern = data.alias > 0 and d.patterns[data.alias] or pattern
        local source = data.alias > 0 and d.patterns[data.alias] and d.patterns[data.alias].tracks[t] or data
        return line(source.lines[k], {
          current = function() return source.lines[k] end,
          ensure = function()
            source.lines[k] = source.lines[k] or { notes = {}, effects = {} }
            return source.lines[k]
          end,
          changed = function()
            local number
            for index, candidate in ipairs(d.patterns) do
              if candidate == source_pattern then number = index end
            end
            for _, fn in ipairs(list(source_pattern.notifiers)) do
              local ok, err = pcall(fn, { pattern = number, track = t, line = k })
              if not ok then M.errors[#M.errors + 1] = "notifier: " .. tostring(err) end
            end
          end,
        })
      end,
      find_automation = function(_, parameter)
        local identity = M.impl(parameter).__data
        for _, automation in ipairs(data.automation or {}) do
          if automation.parameter == identity then return M.api.automation_object(automation) end
        end
        return nil
      end,
      create_automation = function(_, parameter)
        local identity = M.impl(parameter).__data
        data.automation = data.automation or {}
        for _, automation in ipairs(data.automation) do
          if automation.parameter == identity then error("std::logic_error: 'an automation for this parameter already exists'", 2) end
        end
        local made = { parameter = identity, playmode = 2, points = {} }
        data.automation[#data.automation + 1] = made
        M.refresh_automated(identity)
        M.fire(data.automation_observable, { type = "insert", index = #data.automation })
        M.record(function()
          for k, automation in ipairs(data.automation) do
            if automation == made then table.remove(data.automation, k) M.fire(data.automation_observable, { type = "remove", index = k }) break end
          end
          M.refresh_automated(identity)
        end, function()
          data.automation[#data.automation + 1] = made
          M.refresh_automated(identity)
          M.fire(data.automation_observable, { type = "insert", index = #data.automation })
        end)
        return M.api.automation_object(made)
      end,
      -- (M4 n-song) a copy of another pattern's track, its automation too; this track's own alias goes. (M5) Another track's
      -- automation lands on the parameter at the same device and place of this one (measured: a scratch copy's automation lands
      -- on its own devices), and the pattern's line notifiers hear the copy. (M7, measured in 3.5.4) A copy of an alias is an
      -- alias of the same pattern, with no lines of its own: a write to it lands in the pattern it shows.
      copy_from = function(_, other)
        local source = M.impl(other).__ptrack
        local from_t = source.t
        if source.alias > 0 then
          local old = { alias = data.alias, lines = data.lines, automation = data.automation }
          data.alias, data.lines, data.automation = source.alias, {}, {}
          M.fire(data.alias_observable)
          M.record(function() data.alias, data.lines, data.automation = old.alias, old.lines, old.automation end,
            function() data.alias, data.lines, data.automation = source.alias, {}, {} end)
          return
        end
        local old = { alias = data.alias, lines = data.lines, automation = data.automation }
        local function copy(value)
          if type(value) ~= "table" then return value end
          local out = {}
          for k, v in pairs(value) do out[k] = (k == "parameter") and v or copy(v) end
          return out
        end
        local function here(parameter)
          if from_t == t then return parameter end
          for di, device in ipairs(d.tracks[from_t].devices or {}) do
            for ki, candidate in ipairs(device.params or {}) do
              local mine = d.tracks[t].devices[di] and d.tracks[t].devices[di].params and d.tracks[t].devices[di].params[ki]
              if candidate == parameter then return mine end
            end
          end
        end
        local automation = {}
        for _, entry in ipairs(source.automation or {}) do
          local mine = here(entry.parameter)
          if mine then
            local made = copy(entry)
            made.parameter = mine
            automation[#automation + 1] = made
          end
        end
        local new = { alias = 0, lines = copy(source.lines or {}), automation = automation }
        data.alias, data.lines, data.automation = new.alias, new.lines, new.automation
        M.copies_from = (M.copies_from or 0) + 1
        M.fire(data.alias_observable)
        for _, fn in ipairs(list(pattern.notifiers)) do pcall(fn, { pattern = p, track = t, line = 1 }) end
        M.record(function() data.alias, data.lines, data.automation = old.alias, old.lines, old.automation; M.fire(data.alias_observable) end,
          function() data.alias, data.lines, data.automation = new.alias, new.lines, new.automation; M.fire(data.alias_observable) end)
      end,
      delete_automation = function(_, parameter)
        local identity = M.impl(parameter).__data
        for k, automation in ipairs(data.automation or {}) do
          if automation.parameter == identity then
            table.remove(data.automation, k)
            M.refresh_automated(identity)
            M.fire(data.automation_observable, { type = "remove", index = k })
            M.record(function()
              table.insert(data.automation, math.min(k, #data.automation + 1), automation)
              M.refresh_automated(identity)
              M.fire(data.automation_observable, { type = "insert", index = k })
            end, function()
              for j, other in ipairs(data.automation) do
                if other == automation then table.remove(data.automation, j) M.fire(data.automation_observable, { type = "remove", index = j }) break end
              end
              M.refresh_automated(identity)
            end)
            return
          end
        end
        error("std::logic_error: 'no automation for this parameter'", 2)
      end,
    }, {
      color = function() return data.color and { data.color[1], data.color[2], data.color[3] } or nil end,
      automation = function()
        local list = {}
        for k, automation in ipairs(data.automation or {}) do list[k] = M.api.automation_object(automation) end
        return list
      end,
      alias_pattern_index = function() return data.alias end,
      is_alias = function()
        M.alias_reads = (M.alias_reads or 0) + 1 -- (counted: how often a read of the song asks, as a test does)
        return data.alias > 0
      end,
      is_empty = function()
        for _, l in pairs(data.lines) do
          if not line(l).is_empty then return false end
        end
        return true
      end,
    }, {
      color = function(value)
        if value ~= nil and (type(value) ~= "table" or #value ~= 3) then
          error("std::logic_error: 'expected a table with 3 numbers (rgb) as track slot color argument.'", 2)
        end
        local old, new = data.color, value and { value[1], value[2], value[3] } or nil
        data.color = new
        M.fire(data.color_observable)
        M.record(function() data.color = old; M.fire(data.color_observable) end, function() data.color = new; M.fire(data.color_observable) end)
      end,
      -- (measured, 3.5.4: a track made an alias, or no alias with 0, has no lines of its own after: only the undo brings them back)
      alias_pattern_index = function(value)
        local old, old_lines = data.alias, data.lines
        if old == value then return end
        if value > 0 and (value > #d.patterns or value == p) then error("std::logic_error: 'invalid pattern_index value'", 2) end
        if value > 0 and d.patterns[value].tracks[t] and d.patterns[value].tracks[t].alias > 0 then
          error("std::logic_error: 'can not assign an alias to an alias.'", 2)
        end
        data.alias, data.lines = value, {}
        M.fire(data.alias_observable)
        M.record(function() data.alias, data.lines = old, old_lines; M.fire(data.alias_observable) end,
          function() data.alias, data.lines = value, {}; M.fire(data.alias_observable) end)
      end,
    })
    M.impl(made).__ptrack = data
    return made
  end

  pattern_object = function(p)
    local pattern = type(p) == "number" and d.patterns[p]
    if not pattern then out_of_range("pattern", p) end
    return object("renoise.Pattern", {
      name_observable = pattern.name_observable, number_of_lines_observable = pattern.number_of_lines_observable,
      track = function(_, t)
        if type(t) ~= "number" or t < 1 or t > track_count() then out_of_range("pattern track", t) end
        return pattern_track(p, t)
      end,
      add_line_notifier = function(_, fn)
        for _, existing in ipairs(pattern.notifiers) do
          if existing == fn then error("the notifier was already added", 2) end
        end
        M.notifier_adds = (M.notifier_adds or 0) + 1
        pattern.notifiers[#pattern.notifiers + 1] = fn
      end,
      -- (M4 n-song) every track's lines, automation and alias go; its line notifiers fire for the lines that had something
      clear = function()
        local old = pattern.tracks
        local cleared = {}
        for t, data in pairs(old) do
          cleared[t] = { alias = 0, lines = {}, color = data.color, alias_observable = data.alias_observable,
            color_observable = data.color_observable, automation_observable = data.automation_observable }
        end
        pattern.tracks = cleared
        for t, data in pairs(old) do
          for k in pairs(data.lines or {}) do
            for _, fn in ipairs(list(pattern.notifiers)) do pcall(fn, { pattern = p, track = t, line = k }) end
          end
        end
        M.record(function() pattern.tracks = old end, function() pattern.tracks = cleared end)
      end,
      remove_line_notifier = function(_, fn)
        for i, existing in ipairs(pattern.notifiers) do
          if existing == fn then table.remove(pattern.notifiers, i) return end
        end
        error("the notifier was not added", 2)
      end,
      has_line_notifier = function(_, fn)
        for _, existing in ipairs(pattern.notifiers) do
          if existing == fn then return true end
        end
        return false
      end,
    }, {
      name = function() return pattern.name end,
      number_of_lines = function() return pattern.lines end,
      is_empty = function()
        for _, pattern_track_data in pairs(pattern.tracks) do
          if pattern_track_data.alias == 0 or pattern_track_data.alias == nil then
            for _, line_data in pairs(pattern_track_data.lines or {}) do
              if not line(line_data).is_empty then return false end
            end
            if #(pattern_track_data.automation or {}) > 0 then return false end
          end
        end
        return true
      end,
    }, {
      name = function(value)
        local old = pattern.name
        if old == value then return end
        pattern.name = value
        M.fire(pattern.name_observable)
        M.record(function() pattern.name = old; M.fire(pattern.name_observable) end, function() pattern.name = value; M.fire(pattern.name_observable) end)
      end,
      number_of_lines = function(value)
        local old = pattern.lines
        if old == value then return end
        pattern.lines = value
        M.fire(pattern.number_of_lines_observable)
        M.record(function() pattern.lines = old; M.fire(pattern.number_of_lines_observable) end,
          function() pattern.lines = value; M.fire(pattern.number_of_lines_observable) end)
      end,
    })
  end

  -- (n-instruments: an instrument's proxy, with its samples, phrases, sets, chains, plug-in and MIDI output, is mock_instruments.lua's)
  instrument_object = function(i) return M.instrument_proxy(i) end

  d.sequence_observable, d.assignments_observable, d.mutes_observable =
    simple_observable(), simple_observable(), simple_observable()
  d.sections_observable = simple_observable()
  local sequencer = object("renoise.PatternSequencer", {
    pattern_sequence_observable = d.sequence_observable, pattern_assignments_observable = d.assignments_observable,
    pattern_slot_mutes_observable = d.mutes_observable,
    sequence_sections_changed_observable = function() return d.sections_observable end,
    pattern = function(_, s)
      if not d.sequence[s] then out_of_range("sequence", s) end
      return d.sequence[s]
    end,
    sequence_is_start_of_section = function(_, s)
      if not d.sequence[s] then out_of_range("sequence", s) end
      return d.sections[s] ~= nil
    end,
    sequence_section_name = function(_, s)
      if not d.sequence[s] then out_of_range("sequence", s) end
      return d.sections[s] or d.section_names[s] or ""
    end,
    -- (Renoise keeps a slot's section name when it is no longer the start of a section: measured)
    set_sequence_is_start_of_section = function(_, s, flag)
      if not d.sequence[s] then out_of_range("sequence", s) end
      local old_flag, old_name = d.sections[s], d.section_names[s]
      local name = d.section_names[s] or d.sections[s] or ""
      if flag then d.sections[s], d.section_names[s] = name, nil else d.sections[s], d.section_names[s] = nil, d.sections[s] or d.section_names[s] end
      local new_flag, new_name = d.sections[s], d.section_names[s]
      M.fire(d.sections_observable)
      M.record(function() d.sections[s], d.section_names[s] = old_flag, old_name; M.fire(d.sections_observable) end,
        function() d.sections[s], d.section_names[s] = new_flag, new_name; M.fire(d.sections_observable) end)
    end,
    set_sequence_section_name = function(_, s, name)
      if not d.sequence[s] then out_of_range("sequence", s) end
      local old_flag, old_name = d.sections[s], d.section_names[s]
      if d.sections[s] ~= nil then d.sections[s] = name else d.section_names[s] = name end
      local new_flag, new_name = d.sections[s], d.section_names[s]
      M.fire(d.sections_observable)
      M.record(function() d.sections[s], d.section_names[s] = old_flag, old_name; M.fire(d.sections_observable) end,
        function() d.sections[s], d.section_names[s] = new_flag, new_name; M.fire(d.sections_observable) end)
    end,
    insert_sequence_at = function(_, i, p)
      if type(i) ~= "number" or i < 1 or i > #d.sequence + 1 then out_of_range("sequence", i) end
      if type(p) ~= "number" or not d.patterns[p] then out_of_range("pattern", p) end
      d.insert_slot(i, p)
      M.record(function() d.remove_slot(i) end, function() d.insert_slot(i, p) end)
    end,
    insert_new_pattern_at = function(_, i)
      if type(i) ~= "number" or i < 1 or i > #d.sequence + 1 then out_of_range("sequence", i) end
      local made = d.new_pattern()
      M.fire(M.data.song_fields.patterns_observable, { type = "insert", index = #d.patterns })
      local p = #d.patterns
      d.insert_slot(i, p)
      M.record(function()
        d.remove_slot(i)
        table.remove(d.patterns, p)
        M.fire(M.data.song_fields.patterns_observable, { type = "remove", index = p })
      end, function()
        d.patterns[#d.patterns + 1] = made
        M.fire(M.data.song_fields.patterns_observable, { type = "insert", index = #d.patterns })
        d.insert_slot(i, #d.patterns)
      end)
    end,
    delete_sequence_at = function(_, i)
      if type(i) ~= "number" or not d.sequence[i] then out_of_range("sequence", i) end
      if #d.sequence == 1 then error("can not delete the last sequence slot", 2) end
      local p = d.sequence[i]
      local section, name, mutes = d.sections[i], d.section_names[i], {}
      for key in pairs(d.muted) do
        local t, slot = key:match("^(%d+),(%d+)$")
        if tonumber(slot) == i then mutes[#mutes + 1] = tonumber(t) end
      end
      d.remove_slot(i)
      local dropped = d.drop_unused(p)
      M.record(function()
        if dropped then d.restore_pool(p, dropped) end
        d.insert_slot(i, p)
        d.sections[i], d.section_names[i] = section, name
        for _, t in ipairs(mutes) do d.muted[t .. "," .. i] = true end
      end, function()
        d.remove_slot(i)
        if dropped then d.drop_unused(p) end
      end)
    end,
    set_pattern = function(_, i, p)
      if type(i) ~= "number" or not d.sequence[i] then out_of_range("sequence", i) end
      if type(p) ~= "number" or not d.patterns[p] then out_of_range("pattern", p) end
      local old = d.sequence[i]
      if old == p then return end
      d.sequence[i] = p
      M.fire(d.assignments_observable)
      local dropped = d.drop_unused(old)
      M.record(function()
        if dropped then d.restore_pool(old, dropped) end
        d.sequence[i] = old
        M.fire(d.assignments_observable)
      end, function()
        d.sequence[i] = p >= old and dropped and p - 1 or p
        M.fire(d.assignments_observable)
        if dropped then d.drop_unused(old) end
      end)
    end,
    track_sequence_slot_is_muted = function(_, t, s)
      if not d.sequence[s] or not d.tracks[t] then out_of_range("slot", t .. "," .. s) end
      return d.muted[t .. "," .. s] == true
    end,
    -- M4 (n-song): measured in 3.5.4: clone_range puts copies of slots a..b after b, each with a new pattern (a copy named "#"
    -- and the name, at the pool's end), the slots' mutes and section starts copied (a section's name with "#" in front);
    -- make_range_unique gives each slot in the range whose pattern another slot plays a copy of it.
    clone_range = function(_, a, b)
      if type(a) ~= "number" or not d.sequence[a] then out_of_range("from_sequence_pos", a) end
      if type(b) ~= "number" or not d.sequence[b] or b < a then out_of_range("to_sequence_pos", b) end
      local made, slots = {}, {}
      for i = a, b do
        local copy = d.copy_pattern(d.sequence[i])
        d.patterns[#d.patterns + 1] = copy
        M.fire(M.data.song_fields.patterns_observable, { type = "insert", index = #d.patterns })
        made[#made + 1] = copy
        local section = d.sections[i]
        local mutes = {}
        for key in pairs(d.muted) do
          local t, slot = key:match("^(%d+),(%d+)$")
          if tonumber(slot) == i then mutes[#mutes + 1] = tonumber(t) end
        end
        slots[#slots + 1] = { pattern = #d.patterns, section = section and ("#" .. section) or nil, mutes = mutes }
      end
      local function put()
        for k, slot in ipairs(slots) do
          local at = b + k
          d.insert_slot(at, slot.pattern)
          d.sections[at], d.section_names[at] = slot.section, nil
          for key in pairs(d.muted) do
            local t, s_ = key:match("^(%d+),(%d+)$")
            if tonumber(s_) == at then d.muted[key] = nil end
          end
          for _, t in ipairs(slot.mutes) do d.muted[t .. "," .. at] = true end
        end
      end
      put()
      M.record(function()
        for k = #slots, 1, -1 do d.remove_slot(b + k) end
        for _ = #made, 1, -1 do
          table.remove(d.patterns)
          M.fire(M.data.song_fields.patterns_observable, { type = "remove", index = #d.patterns + 1 })
        end
      end, function()
        for _, copy in ipairs(made) do
          d.patterns[#d.patterns + 1] = copy
          M.fire(M.data.song_fields.patterns_observable, { type = "insert", index = #d.patterns })
        end
        put()
      end)
    end,
    make_range_unique = function(_, a, b)
      if type(a) ~= "number" or not d.sequence[a] then out_of_range("from_sequence_pos", a) end
      if type(b) ~= "number" or not d.sequence[b] or b < a then out_of_range("to_sequence_pos", b) end
      local played = {}
      for _, p in ipairs(d.sequence) do played[p] = (played[p] or 0) + 1 end
      local changes = {}
      for i = a, b do
        local old = d.sequence[i]
        if played[old] > 1 then
          local copy = d.copy_pattern(old)
          d.patterns[#d.patterns + 1] = copy
          M.fire(M.data.song_fields.patterns_observable, { type = "insert", index = #d.patterns })
          d.sequence[i] = #d.patterns
          changes[#changes + 1] = { slot = i, old = old, copy = copy, p = #d.patterns }
        end
      end
      if #changes > 0 then M.fire(d.assignments_observable) end
      M.record(function()
        for k = #changes, 1, -1 do
          d.sequence[changes[k].slot] = changes[k].old
          table.remove(d.patterns)
          M.fire(M.data.song_fields.patterns_observable, { type = "remove", index = #d.patterns + 1 })
        end
        M.fire(d.assignments_observable)
      end, function()
        for _, change in ipairs(changes) do
          d.patterns[#d.patterns + 1] = change.copy
          M.fire(M.data.song_fields.patterns_observable, { type = "insert", index = #d.patterns })
          d.sequence[change.slot] = change.p
        end
        M.fire(d.assignments_observable)
      end)
    end,
    set_track_sequence_slot_is_muted = function(_, t, s, muted)
      if not d.sequence[s] or not d.tracks[t] then out_of_range("slot", t .. "," .. s) end
      local key = t .. "," .. s
      local old = d.muted[key]
      d.muted[key] = muted or nil
      M.fire(d.mutes_observable)
      M.record(function() d.muted[key] = old; M.fire(d.mutes_observable) end,
        function() d.muted[key] = muted or nil; M.fire(d.mutes_observable) end)
    end,
  }, {
    pattern_sequence = function() return list(d.sequence) end,
  })

  -- The slots and the pattern pool (Renoise removes a pattern that no slot uses and nothing is in)
  d.insert_slot = function(i, p)
    table.insert(d.sequence, i, p)
    local sections, names, muted = {}, {}, {}
    for s, name in pairs(d.sections) do sections[s >= i and s + 1 or s] = name end
    for s, name in pairs(d.section_names) do names[s >= i and s + 1 or s] = name end
    for key in pairs(d.muted) do
      local t, slot = key:match("^(%d+),(%d+)$")
      slot = tonumber(slot)
      muted[t .. "," .. (slot >= i and slot + 1 or slot)] = true
    end
    -- (measured: the slot made in the middle gets the section start and the muted tracks of the slot it pushes up)
    if d.sequence[i + 1] then
      sections[i], names[i] = sections[i + 1], names[i + 1]
      for key in pairs(muted) do
        local t, slot = key:match("^(%d+),(%d+)$")
        if tonumber(slot) == i + 1 then muted[t .. "," .. i] = true end
      end
    end
    d.sections, d.section_names, d.muted = sections, names, muted
    M.fire(d.sequence_observable, { type = "insert", index = i })
  end
  d.remove_slot = function(i)
    table.remove(d.sequence, i)
    local sections, names, muted = {}, {}, {}
    for s, name in pairs(d.sections) do
      if s ~= i then sections[s > i and s - 1 or s] = name end
    end
    -- (measured: deleting the slot where a section starts makes the next slot start it)
    if d.sections[i] ~= nil and sections[i] == nil and d.sequence[i] then sections[i] = d.sections[i] end
    for s, name in pairs(d.section_names) do
      if s ~= i then names[s > i and s - 1 or s] = name end
    end
    for key in pairs(d.muted) do
      local t, slot = key:match("^(%d+),(%d+)$")
      slot = tonumber(slot)
      if slot ~= i then muted[t .. "," .. (slot > i and slot - 1 or slot)] = true end
    end
    d.sections, d.section_names, d.muted = sections, names, muted
    M.fire(d.sequence_observable, { type = "remove", index = i })
  end
  d.drop_unused = function(p)
    for _, v in ipairs(d.sequence) do
      if v == p then return nil end
    end
    if not pattern_object(p).is_empty then return nil end
    -- (measured: Renoise removes the last pattern of the pool, whatever its name or length, and keeps an earlier one, and one
    -- another pattern's track aliases: lab 2026-11-01)
    if p ~= #d.patterns then return nil end
    for _, other in ipairs(d.patterns) do
      for _, pattern_track in pairs(other.tracks) do
        if pattern_track.alias == p then return nil end
      end
    end
    local data = table.remove(d.patterns, p)
    for k, v in ipairs(d.sequence) do
      if v > p then d.sequence[k] = v - 1 end
    end
    M.fire(M.data.song_fields.patterns_observable, { type = "remove", index = p })
    return data
  end
  -- (M4 n-song) a copy of a pattern as Renoise's clone and make-unique make it: its lines, automation, aliases and slot colours
  local function deep(value)
    if type(value) ~= "table" then return value end
    local out = {}
    for k, v in pairs(value) do out[k] = (k == "parameter" or (type(k) == "string" and k:find("_observable$"))) and v or deep(v) end
    return out
  end
  d.copy_pattern = function(p)
    local source = d.patterns[p]
    local copy = {
      name = "#" .. source.name, lines = source.lines, tracks = {}, name_observable = M.observable(),
      number_of_lines_observable = M.observable(), notifiers = {},
    }
    for t, data in pairs(source.tracks) do
      copy.tracks[t] = { alias = data.alias, lines = deep(data.lines), automation = deep(data.automation), color = deep(data.color) }
    end
    return copy
  end
  d.restore_pool = function(p, data)
    table.insert(d.patterns, p, data)
    for k, v in ipairs(d.sequence) do
      if v >= p then d.sequence[k] = v + 1 end
    end
    M.fire(M.data.song_fields.patterns_observable, { type = "insert", index = p })
  end

  local transport_fields = {
    playing = options.playing or false, bpm = options.bpm or 120, lpb = options.lpb or 4, tpl = options.tpl or 6,
    edit_mode = false, follow_player = false, loop_pattern = false, loop_block_enabled = false,
    playback_pos_beats = 0, metronome_beats_per_bar = 0, loop_start_beats = 0, loop_end_beats = 16,
    loop_sequence_start = 0, loop_sequence_end = 0,
    playing_observable = simple_observable(), bpm_observable = simple_observable(), lpb_observable = simple_observable(),
    tpl_observable = simple_observable(), edit_mode_observable = simple_observable(),
    follow_player_observable = simple_observable(), loop_pattern_observable = simple_observable(),
    loop_block_enabled_observable = simple_observable(),
    playback_pos = object("renoise.SongPos", { sequence = 1, line = 1 }),
  }
  transport_fields.stop = function()
    transport_fields.playing = false
    M.fire(transport_fields.playing_observable)
  end
  -- (every note off, every voice ended: counted, as a test asks it)
  transport_fields.panic = function()
    M.panics = (M.panics or 0) + 1
    M.triggers = M.triggers or {}
    M.triggers[#M.triggers + 1] = { "panic" }
  end
  transport_fields.start = function(_, mode)
    transport_fields.playing, d.started_mode = true, mode
    M.fire(transport_fields.playing_observable)
  end
  transport_fields.start_at = function(_, position)
    transport_fields.playing, d.started_at = true, position
    M.fire(transport_fields.playing_observable)
  end
  -- (playing from a slot and the slots scheduled after it: playback, no undo step)
  transport_fields.trigger_sequence = function(_, slot)
    transport_fields.playing, d.started_at = true, slot
    M.fire(transport_fields.playing_observable)
  end
  transport_fields.add_scheduled_sequence = function(_, slot)
    d.scheduled = d.scheduled or {}
    d.scheduled[#d.scheduled + 1] = slot
  end
  transport_fields.set_scheduled_sequence = function(_, slot) d.scheduled = { slot } end
  local transport_setters = {}
  -- (the loop is not an undo step; Renoise applies it a moment after it is written: a read right after still shows the old one)
  transport_setters.loop_pattern = function(value)
    transport_fields.loop_pattern = value
    M.fire(transport_fields.loop_pattern_observable)
  end
  transport_setters.loop_sequence_range = function(value)
    transport_fields.loop_range_seq = { value[1], value[2] }
  end
  for _, key in ipairs({ "bpm", "lpb", "tpl" }) do
    transport_setters[key] = function(value)
      local old = transport_fields[key]
      if old == value then return end
      transport_fields[key] = value
      M.fire(transport_fields[key .. "_observable"])
      M.record(function() transport_fields[key] = old; M.fire(transport_fields[key .. "_observable"]) end,
        function() transport_fields[key] = value; M.fire(transport_fields[key .. "_observable"]) end)
    end
  end
  -- M4 (n-song): the groove, the metronome, the editing settings and the track headroom. Measured in 3.5.4: the groove, the
  -- metronome's beats per bar, lines per beat and volume and the headroom are undo steps (recorded here); the metronome's
  -- switch and precount, edit step, octave, record quantize, wrapped and single-track editing, keyboard velocity and the
  -- parameter record mode are not (plain fields, their observables fire on a write).
  for key, value in pairs({
    metronome_enabled = false, metronome_precount_enabled = false, metronome_precount_bars = 1, edit_step = 1, octave = 3,
    record_quantize_enabled = false, record_quantize_lines = 1, wrapped_pattern_edit = false, single_track_edit_mode = false,
    keyboard_velocity_enabled = false, keyboard_velocity = 127, record_parameter_mode = 1,
  }) do
    transport_fields[key], transport_fields[key .. "_observable"] = value, simple_observable()
  end
  local groove = { enabled = false, amounts = { 0, 0, 0, 0 } }
  transport_fields.groove_enabled_observable, transport_fields.groove_assignment_observable = simple_observable(), simple_observable()
  transport_fields.metronome_beats_per_bar_observable = simple_observable()
  for key, value in pairs({ metronome_lines_per_beat = 0, metronome_volume = 0.70794576406479, track_headroom = 0.50118720531464 }) do
    transport_fields[key], transport_fields[key .. "_observable"] = value, simple_observable()
  end
  for _, key in ipairs({ "metronome_beats_per_bar", "metronome_lines_per_beat", "metronome_volume", "track_headroom" }) do
    transport_setters[key] = function(value)
      local old = transport_fields[key]
      if old == value then return end
      transport_fields[key] = value
      M.fire(transport_fields[key .. "_observable"])
      M.record(function() transport_fields[key] = old; M.fire(transport_fields[key .. "_observable"]) end,
        function() transport_fields[key] = value; M.fire(transport_fields[key .. "_observable"]) end)
    end
  end
  transport_setters.groove_enabled = function(value)
    local old = groove.enabled
    if old == value then return end
    groove.enabled = value
    M.fire(transport_fields.groove_enabled_observable)
    M.record(function() groove.enabled = old; M.fire(transport_fields.groove_enabled_observable) end,
      function() groove.enabled = value; M.fire(transport_fields.groove_enabled_observable) end)
  end
  transport_setters.groove_amounts = function(value)
    local old, new = groove.amounts, { value[1], value[2], value[3], value[4] }
    groove.amounts = new
    M.fire(transport_fields.groove_assignment_observable)
    M.record(function() groove.amounts = old; M.fire(transport_fields.groove_assignment_observable) end,
      function() groove.amounts = new; M.fire(transport_fields.groove_assignment_observable) end)
  end
  d.groove = groove
  local transport_getters = {
    groove_enabled = function() return groove.enabled end,
    groove_amounts = function() return { groove.amounts[1], groove.amounts[2], groove.amounts[3], groove.amounts[4] } end,
    -- The edit position in quarter notes: the lines before it over the LPB (a test can set `edit_beats`, as a ZLxx would).
    loop_sequence_range = function()
      local r = transport_fields.loop_range_seq
      if r and r[1] and r[1] > 0 then return { r[1], r[2] } end
      return {}
    end,
    edit_pos_beats = function()
      if transport_fields.edit_beats ~= nil then return transport_fields.edit_beats end
      local lines = 0
      for s = 1, d.song_fields.selected_sequence_index - 1 do lines = lines + d.patterns[d.sequence[s]].lines end
      return (lines + d.song_fields.selected_line_index - 1) / transport_fields.lpb
    end,
  }
  -- (M5 m5-renoise: the play position as it plays, the song's length, the edit position; see mock_listening.lua)
  if M.transport_extras then M.transport_extras(transport_fields, transport_getters, transport_setters, d) end
  local transport = object("renoise.Transport", transport_fields, transport_getters, transport_setters)

  local song_fields = {
    name = options.name or "", name_observable = simple_observable(), file_name = options.file_name or "",
    tool_data = options.tool_data or M.NIL, rendering = false,
    transport = transport, sequencer = sequencer,
    sequencer_track_count = 0, send_track_count = 0,
    tracks_observable = simple_observable(), instruments_observable = simple_observable(),
    patterns_observable = simple_observable(),
    selected_track_index = 1, selected_instrument_index = 1, selected_pattern_index = 1, selected_sequence_index = 1,
    selected_line_index = 1, selected_note_column_index = 1, selected_effect_column_index = 0,
    selected_sample_index = 0, selected_phrase_index = 0, selected_track_device_index = 0, selected_sample_modulation_set_index = 0,
    selected_phrase_line_index = 0, selection_in_pattern = M.NIL,
    selected_track_index_observable = simple_observable(), selected_instrument_index_observable = simple_observable(),
    selected_pattern_index_observable = simple_observable(), selected_sequence_index_observable = simple_observable(),
    selected_sample_observable = simple_observable(), selected_phrase_observable = simple_observable(),
    selected_track_device_observable = simple_observable(),
    track = function(_, i) return track(i) end,
    pattern = function(_, p) return pattern_object(p) end,
    instrument = function(_, i) return instrument_object(i) end,
    insert_track_at = function(_, index) return M.api.insert_track_at(index) end,
    delete_track_at = function(_, index) M.api.delete_track_at(index) end,
    swap_tracks_at = function(_, a, b) M.api.swap_tracks_at(a, b) end,
    add_track_to_group = function(_, track_index, group_index) M.api.add_track_to_group(track_index, group_index) end,
    remove_track_from_group = function(_, track_index) M.api.remove_track_from_group(track_index) end,
    insert_group_at = function(_, index) return M.api.insert_group_at(index) end,
    -- M4 (n-song): notes played for preview, as Renoise's tests see them (measured: they play with the transport stopped, on
    -- any track; out-of-range arguments raise)
    trigger_instrument_note_on = function(_, i, t, notes, volume)
      if not d.instruments[i] then error("std::logic_error: 'invalid instrument index'", 2) end
      if not d.tracks[t] then error("std::logic_error: 'invalid track index'", 2) end
      for _, note in ipairs(type(notes) == "table" and notes or { notes or 48 }) do
        if note < 0 or note > 119 then error("std::logic_error: 'invalid note value'", 2) end
      end
      M.triggers[#M.triggers + 1] = { "on", i, t, type(notes) == "table" and table.concat(notes, ",") or tostring(notes or 48), volume }
    end,
    trigger_instrument_note_off = function(_, i, t, notes)
      M.triggers[#M.triggers + 1] = { "off", i, t, type(notes) == "table" and table.concat(notes, ",") or tostring(notes or 48) }
    end,
    trigger_sample_note_on = function(_, i, sample, t, note, volume)
      if not d.instruments[i] or not d.instruments[i].samples[sample] then error("std::logic_error: 'invalid sample index'", 2) end
      M.triggers[#M.triggers + 1] = { "sample on", i, t, tostring(note), volume, sample }
    end,
    trigger_sample_note_off = function(_, i, sample, t, note)
      M.triggers[#M.triggers + 1] = { "sample off", i, t, tostring(note), nil, sample }
    end,
    -- (n-instruments: Renoise renumbers the notes that play the instruments after the place)
    insert_instrument_at = function(_, index)
      M.instrument_list_op("insert", index, nil, nil, true)
      return instrument_object(index)
    end,
    delete_instrument_at = function(_, index) M.instrument_list_op("remove", index, nil, nil, true) end,
    swap_instruments_at = function(_, a, b) M.instrument_list_op("swap", a, b, nil, true) end,
    describe_undo = function(_, text) d.undo.desc = text end,
    can_undo = function() return #d.undo.stack > 0 end,
    can_redo = function() return #d.undo.redo > 0 end,
    undo = function() M.api.undo() end,
    redo = function() M.api.redo() end,
    is_undo_redoing = function() return d.undo.redoing end,
  }
  d.track_proxy = track
  -- M4 (n-song): the song's name, artist and comments are undo steps (measured, 3.5.4); its comments observable is a list's,
  -- fired once per paragraph.
  -- (the name stays a field of the song's, which a test may take away)
  local info = { artist = options.artist or "", comments = options.comments or {} }
  song_fields.artist_observable, song_fields.comments_observable = simple_observable(), simple_observable()
  song_fields.comments_assignment_observable = simple_observable()
  local function info_setter(key, observable, copy)
    local store = key == "name" and song_fields or info
    return function(value)
      local old, new = store[key], copy and copy(value) or value
      store[key] = new
      for _ = 1, key == "comments" and math.max(#old, #new, 1) or 1 do M.fire(song_fields[observable]) end
      M.record(function() store[key] = old; M.fire(song_fields[observable]) end, function() store[key] = new; M.fire(song_fields[observable]) end)
    end
  end
  local function lines_copy(value)
    local out = {}
    for i, line in ipairs(value) do out[i] = line end
    return out
  end
  local song_setters = {
    name = info_setter("name", "name_observable"), artist = info_setter("artist", "artist_observable"),
    comments = info_setter("comments", "comments_observable", lines_copy),
  }
  d.info = info
  local song_getters = {
    artist = function() return info.artist end,
    comments = function() return lines_copy(info.comments) end,
    patterns = function()
      local patterns = {}
      for p in ipairs(d.patterns) do patterns[p] = pattern_object(p) end
      return patterns
    end,
    instruments = function()
      local instruments = {}
      for i in ipairs(d.instruments) do instruments[i] = instrument_object(i) end
      return instruments
    end,
  }
  d.song_fields = song_fields
  d.transport_fields = transport_fields
  function d.recount()
    local sequencers, sends = 0, 0
    for _, t in ipairs(d.tracks) do
      if t.kind == "sequencer" or t.kind == "group" then sequencers = sequencers + 1 end
      if t.kind == "send" then sends = sends + 1 end
    end
    song_fields.sequencer_track_count, song_fields.send_track_count = sequencers, sends
  end
  d.recount()
  song = object("renoise.Song", song_fields, song_getters, song_setters)
  d.song = song
  return song
end

-- M4 n-devices: renoise.app()'s library loaders, as Renoise 3.5.4 does them (lab 2026-10-20). A file that isn't of the kind
-- shows a dialog (M.dialogs) and loads nothing. A device chain replaces every device after the selected track's mixer (from
-- the last), puts in the chain's devices in order (their XML types at the top of the file), resets the mixer's values and
-- selects the last device. A phrase replaces the selected phrase (one is made in an instrument with none; with none
-- selected, the first). A modulation set replaces the selected set's devices and keeps its name.
local CHAIN_PATHS = {
  CompressorDevice = "Audio/Effects/Native/Compressor", Eq10Device = "Audio/Effects/Native/EQ 10", Eq5Device = "Audio/Effects/Native/EQ 5",
  GainerDevice = "Audio/Effects/Native/Gainer", DelayDevice = "Audio/Effects/Native/Delay", LfoDevice = "Audio/Effects/Native/*LFO",
}
local MIXER_DEFAULTS = { Panning = 0.5, Volume = 1, Width = 0, ["Post Panning"] = 0.5, ["Post Volume"] = 1 }

local function library_file(path, root)
  local handle = io.open(path, "rb")
  local text = handle and handle:read("*a")
  if handle then handle:close() end
  if not text or not text:find("<" .. root, 1, true) then
    M.dialogs = (M.dialogs or 0) + 1
    return nil
  end
  return text
end

function M.app_load_chain(path)
  local text = library_file(path, "RenoiseDeviceChain")
  if not text then return false end
  local fields = M.data.song_fields
  local track = M.song:track(fields.selected_track_index)
  for d = #track.devices, 2, -1 do track:delete_device_at(d) end
  local at = 2
  for kind in text:gmatch('\n    <(%w+) type="%1">') do
    track:insert_device_at(CHAIN_PATHS[kind] or ("Audio/Effects/Native/" .. kind:gsub("Device$", "")), at)
    at = at + 1
  end
  local mixer = track:device(1)
  for k = 1, #mixer.parameters do
    local parameter = mixer:parameter(k)
    if MIXER_DEFAULTS[parameter.name] then parameter.value = MIXER_DEFAULTS[parameter.name] end
  end
  fields.selected_track_device_index = #track.devices
  return true
end

function M.app_load_phrase(path)
  local text = library_file(path, "InstrumentPhrase")
  if not text then return false end
  local fields = M.data.song_fields
  local instrument = M.song:instrument(fields.selected_instrument_index)
  local k = fields.selected_phrase_index
  if #instrument.phrases == 0 then
    instrument:insert_phrase_at(1)
    k = 1
  elseif k < 1 then
    k = 1
  end
  local phrase = instrument.phrases[k]
  phrase.name = text:match("<Name>(.-)</Name>") or ""
  phrase.number_of_lines = tonumber(text:match("<NumberOfLines>(%d+)</NumberOfLines>")) or 16
  return true
end

function M.app_load_modset(path)
  local text = library_file(path, "SampleModulationSet")
  if not text then return false end
  local fields = M.data.song_fields
  local instrument = M.song:instrument(fields.selected_instrument_index)
  local set = instrument.sample_modulation_sets[math.max(1, fields.selected_sample_modulation_set_index)]
  for d = #set.devices, 1, -1 do set:delete_device_at(d) end
  local n = 0
  for _ in text:gmatch('type="Sample%w+ModulationDevice"') do n = n + 1 end
  for d = 1, n do set:insert_device_at("Modulation/AHDSR", 1, d) end
  return true
end

-- What the producer does. Each changes the data and fires what Renoise would.

local ops = {}
M.ops = ops

local function fire(observable, ...) M.fire(observable, ...) end

local function song_fields() return M.data.song_fields end

function ops.set_note(p, t, k, column, given)
  local pattern = M.data.patterns[p]
  pattern.tracks[t] = pattern.tracks[t] or { alias = 0, lines = {} }
  -- (written through an alias, it lands in the pattern the alias shows)
  local alias = pattern.tracks[t].alias
  if alias > 0 then
    p = alias
    pattern = M.data.patterns[p]
    pattern.tracks[t] = pattern.tracks[t] or { alias = 0, lines = {} }
  end
  local data = pattern.tracks[t]
  data.lines[k] = data.lines[k] or { notes = {}, effects = {} }
  data.lines[k].notes = data.lines[k].notes or {}
  data.lines[k].notes[column] = note_data(given)
  for _, fn in ipairs(list(pattern.notifiers)) do
    local ok, err = pcall(fn, { pattern = p, track = t, line = k })
    if not ok then M.errors[#M.errors + 1] = "notifier: " .. tostring(err) end
  end
end

function ops.set_effect(p, t, k, column, number, amount)
  local pattern = M.data.patterns[p]
  pattern.tracks[t] = pattern.tracks[t] or { alias = 0, lines = {} }
  local data = pattern.tracks[t]
  data.lines[k] = data.lines[k] or { notes = {}, effects = {} }
  data.lines[k].effects = data.lines[k].effects or {}
  data.lines[k].effects[column] = { number = number, amount = amount }
  for _, fn in ipairs(list(pattern.notifiers)) do pcall(fn, { pattern = p, track = t, line = k }) end
end

function ops.alias(p, t, of)
  local pattern = M.data.patterns[p]
  pattern.tracks[t] = pattern.tracks[t] or { alias = 0, lines = {} }
  pattern.tracks[t].alias = of
  pattern.tracks[t].alias_observable = pattern.tracks[t].alias_observable or M.observable()
  fire(pattern.tracks[t].alias_observable)
end

function ops.mute_slot(t, s, muted)
  M.data.muted[t .. "," .. s] = muted or nil
  fire(M.data.mutes_observable)
end

function ops.section(s, name)
  M.data.sections[s] = name
  fire(M.data.sections_observable)
end

function ops.mute_column(t, c, muted)
  local track = M.data.tracks[t]
  track.muted_columns = track.muted_columns or {}
  track.muted_columns[c] = muted or nil
  fire(column_observable(track, "muted", c))
end

function ops.name_column(t, c, name)
  local track = M.data.tracks[t]
  track.column_names = track.column_names or {}
  track.column_names[c] = name
  fire(column_observable(track, "name", c))
end

function ops.set_device(t, d, field, value)
  local data = M.data.tracks[t].devices[d]
  data[field] = value
  local key = (field == "active" and "is_active" or field) .. "_observable"
  data[key] = data[key] or M.observable()
  fire(data[key])
end

function ops.set_macro(i, k, value)
  local macro = M.data.instruments[i].macros[k]
  macro.value = value
  fire(M.obs(macro, "value"))
end

-- A sample's mapping: field is note_range, velocity_range, base_note, layer, map_key_to_pitch or map_velocity_to_volume.
function ops.set_mapping(i, s, field, value)
  local sample = M.data.instruments[i].samples[s]
  sample[field] = value
  fire(M.obs(sample, field))
end

function ops.set_phrase_note(i, ph, k, column, given)
  local phrase = M.data.instruments[i].phrases[ph]
  phrase.data[k] = phrase.data[k] or { notes = {}, effects = {} }
  phrase.data[k].notes = phrase.data[k].notes or {}
  phrase.data[k].notes[column] = note_data(given)
  for _, fn in ipairs(list(phrase.notifiers or {})) do pcall(fn, { line = k }) end
end

-- A phrase's lines or lpb.
function ops.set_phrase(i, ph, field, value)
  local phrase = M.data.instruments[i].phrases[ph]
  phrase[field == "number_of_lines" and "lines" or field] = value
  fire(M.obs(phrase, field))
end

local function track_spec(spec)
  spec = spec or {}
  local devices = spec.devices or {}
  if M.data.options.mixer_device and not spec.devices then
    devices = { M.mixer_device_data() }
  end
  local t = {
    kind = spec.kind or "sequencer", name = spec.name or "Track", color = spec.color, members = spec.members or 0,
    mute_state = 1, solo = false, volume = 1, panning = 0.5, note_columns = 1, effect_columns = 1, devices = devices,
    name_observable = M.observable(), color_observable = M.observable(), mute_state_observable = M.observable(),
    solo_state_observable = M.observable(), devices_observable = M.observable(),
    visible_note_columns_observable = M.observable(), visible_effect_columns_observable = M.observable(),
    volume_observable = M.observable(), panning_observable = M.observable(),
  }
  return t
end

-- The data of every pattern follows its track: a track made at `index` has none, those after it move up by one.
local function move_pattern_tracks(shift)
  for _, pattern in ipairs(M.data.patterns) do
    local moved = {}
    for t, data in pairs(pattern.tracks) do
      local to = shift(t)
      if to then moved[to] = data end
    end
    pattern.tracks = moved
  end
end

-- A track's data put into the song at `index` (with the pattern data that went with it, when it comes back).
-- What Renoise does with routing aimed at tracks (measured in 3.5.4): a #Send's Receiver (the send's number among the sends, -1 for
-- none) and a meta device's Dest. Track (the track's number less one) follow tracks made and deleted, and are cleared (-1, with the
-- destination's effect and parameter) when their target is deleted.
-- (M4 n-devices) A device's destinations: each "…Track" parameter with its "…Effect" (and "…Parameter"): a meta device's
-- and #Sidechain's "Dest. …", *Hydra's and *XY Pad's "OutN …". Renoise follows and clears them all alike.
local function destinations_of(device)
  local named = {}
  for _, parameter in ipairs(device.params or {}) do named[parameter.name] = parameter end
  local out = {}
  for _, parameter in ipairs(device.params or {}) do
    local prefix = parameter.name:match("^(.*)Track$")
    if prefix and named[prefix .. "Effect"] then
      out[#out + 1] = { track = parameter, effect = named[prefix .. "Effect"], parameter = named[prefix .. "Parameter"] }
    end
  end
  return out
end

M.destinations_of = destinations_of

local function follow_routes(removed, index)
  local tracks = M.data.tracks
  local target = tracks[index]
  local sends_before = 0
  for i = 1, index - 1 do
    if tracks[i].kind == "send" then sends_before = sends_before + 1 end
  end
  for _, track in ipairs(tracks) do
    for _, device in ipairs(track.devices or {}) do
      for _, parameter in ipairs(device.params or {}) do
        if parameter.name:match("^Receiver") and target.kind == "send" then
          if removed then
            if parameter.value == sends_before then parameter.value = -1 elseif parameter.value > sends_before then parameter.value = parameter.value - 1 end
          elseif parameter.value >= sends_before then
            parameter.value = parameter.value + 1
          end
        end
      end
      -- (a destination counts every track: sequencer, group, send and master)
      for _, destination in ipairs(destinations_of(device)) do
        local dest = destination.track
        if removed then
          if dest.value == index - 1 then
            dest.value = -1
            destination.effect.value = -1
            if destination.parameter then destination.parameter.value = -1 end
          elseif dest.value > index - 1 then
            dest.value = dest.value - 1
          end
        elseif dest.value >= index - 1 then
          dest.value = dest.value + 1
        end
      end
    end
  end
end

-- The same for a destination's Effect (the device's number less one), aimed at a device of `track`.
function M.follow_device_routes(track, device_index, removed)
  local at
  for i, t in ipairs(M.data.tracks) do
    if t == track then at = i end
  end
  for _, other in ipairs(M.data.tracks) do
    for _, device in ipairs(other.devices or {}) do
      for _, destination in ipairs(destinations_of(device)) do
        local dtrack, deffect, dparameter = destination.track, destination.effect, destination.parameter
        if dtrack.value == at - 1 then
          if removed then
            if deffect.value == device_index - 1 then
              deffect.value = -1
              if dparameter then dparameter.value = -1 end
            elseif deffect.value > device_index - 1 then
              deffect.value = deffect.value - 1
            end
          elseif deffect.value >= device_index - 1 then
            deffect.value = deffect.value + 1
          end
        end
      end
    end
  end
end

local function put_track(index, data, pattern_data)
  -- (M4 n-song: a track made during a solo is Off, measured)
  if M.data.solo_roots and #M.data.solo_roots > 0 and data.kind ~= "master" then data.mute_state, data.solo = 2, false end
  table.insert(M.data.tracks, index, data)
  follow_routes(false, index)
  move_pattern_tracks(function(t) return t >= index and t + 1 or t end)
  for p, kept in pairs(pattern_data or {}) do
    if M.data.patterns[p] then M.data.patterns[p].tracks[index] = kept end
  end
  M.data.recount()
  fire(song_fields().tracks_observable, { type = "insert", index = index })
end

-- A track taken out of the song: its data, and the pattern data that went with it.
local function take_track(index)
  -- a group that held the track has one member fewer
  for g, t in ipairs(M.data.tracks) do
    if t.kind == "group" and g > index and index >= g - t.members then t.members = t.members - 1 end
  end
  local pattern_data = {}
  for p, pattern in ipairs(M.data.patterns) do pattern_data[p] = pattern.tracks[index] end
  follow_routes(true, index)
  local data = table.remove(M.data.tracks, index)
  move_pattern_tracks(function(t) if t == index then return nil end return t > index and t - 1 or t end)
  M.data.recount()
  fire(song_fields().tracks_observable, { type = "remove", index = index })
  -- (M4 n-song: deleting the soloed track ends the solo: every track Active, measured)
  for _, root in ipairs(M.data.solo_roots or {}) do
    if root == data then
      M.data.solo_roots = {}
      M.solo_refresh()
      break
    end
  end
  return data, pattern_data
end

function ops.insert_track(index, spec)
  put_track(index, track_spec(spec))
end

function ops.remove_track(index)
  take_track(index)
end

-- What a script does to the song through the API: the same, with an undo step recorded for it.
M.api = {}

-- Measured in Renoise 3.5.4: a track made at or before the master's place is a sequencer track (the master moves up), one made
-- after it is a send track; a sequencer track made just before a group (but not before its first member) joins the group.
function M.api.insert_track_at(index)
  local tracks = M.data.tracks
  local master
  for i, t in ipairs(tracks) do
    if t.kind == "master" then master = i end
  end
  if type(index) ~= "number" or index < 1 or index > #tracks + 1 then error("track index out of range: " .. tostring(index), 2) end
  local spec
  if index > master then
    spec = track_spec({ kind = "send", name = "S" .. string.format("%02d", index - master) })
  else
    spec = track_spec({ name = string.format("Track %02d", index) })
  end
  local made = index
  put_track(index, spec)
  local joined
  for g, t in ipairs(tracks) do
    if t.kind == "group" and spec.kind == "sequencer" and g > made and g - 1 - t.members < made and made <= g - 1 then
      t.members = t.members + 1
      joined = t
    end
  end
  M.record(function()
    take_track(made)
  end, function()
    put_track(made, spec)
    if joined then joined.members = joined.members + 1 end
  end)
  return M.data.track_proxy(index)
end

function M.api.insert_group_at(index)
  local tracks = M.data.tracks
  local master
  for i, t in ipairs(tracks) do
    if t.kind == "master" then master = i end
  end
  if type(index) ~= "number" or index < 1 or index > master then error("a group is made before the master", 2) end
  local spec = track_spec({ kind = "group", name = "Group" })
  put_track(index, spec)
  M.record(function() take_track(index) end, function() put_track(index, spec) end)
  return M.data.track_proxy(index)
end

-- The track becomes the last member of the group: it is moved to just before it, by swaps (as Renoise does it).
function M.api.add_track_to_group(index, group)
  local tracks = M.data.tracks
  if not tracks[index] or not tracks[group] or tracks[group].kind ~= "group" then error("not a track and a group", 2) end
  if index == group then error("indices are the same, can't add track to itself", 2) end
  local at = index
  if index < group then
    while at < group - 1 do
      M.api.swap_tracks_at(at, at + 1, true)
      at = at + 1
    end
  else
    -- (a track after the group takes its place and the group moves up)
    while at > group do
      M.api.swap_tracks_at(at - 1, at, true)
      at = at - 1
    end
    group = group + 1
  end
  local grouped, member = tracks[group], tracks[group - 1]
  -- (its output routing becomes the group's, and goes back on Renoise's undo: told each time, lab 2026-10-20)
  local function routed() if member.output_routing_observable then M.fire(member.output_routing_observable) end end
  grouped.members = grouped.members + 1
  routed()
  M.record(function() grouped.members = grouped.members - 1 routed() end, function() grouped.members = grouped.members + 1 routed() end)
end

-- The track leaves its group: it is moved to just before the group's tracks, by swaps; the tracks it passed keep their order.
function M.api.remove_track_from_group(index)
  local tracks = M.data.tracks
  local group
  for g = index + 1, #tracks do
    if tracks[g].kind == "group" and g - tracks[g].members <= index then group = g break end
  end
  if not group then error("track is not a member of a group", 2) end
  local start = group - tracks[group].members
  local at = index
  while at > start do
    M.api.swap_tracks_at(at - 1, at, true)
    at = at - 1
  end
  local grouped, member = tracks[group], tracks[start]
  local function routed() if member.output_routing_observable then M.fire(member.output_routing_observable) end end
  grouped.members = grouped.members - 1
  routed()
  M.record(function() grouped.members = grouped.members + 1 routed() end, function() grouped.members = grouped.members - 1 routed() end)
end

function M.api.delete_track_at(index)
  local tracks = M.data.tracks
  if type(index) ~= "number" or not tracks[index] then error("track index out of range: " .. tostring(index), 2) end
  if tracks[index].kind == "master" then error("the master track can not be deleted", 2) end
  local data, pattern_data = take_track(index)
  M.record(function() put_track(index, data, pattern_data) end, function() take_track(index) end)
end

function M.api.swap_tracks_at(a, b, internal)
  local tracks = M.data.tracks
  if not tracks[a] or not tracks[b] then error("track index out of range", 2) end
  local function class(t) return t.kind == "master" and "master" or t.kind == "send" and "send" or "sequencer" end
  if class(tracks[a]) ~= class(tracks[b]) then
    error("can not swap a sequencer track with a send track or a send or sequencer track with the master track.", 2)
  end
  if a == b then return end
  if a > b then a, b = b, a end
  -- (Renoise swaps tracks that are not neighbours by neighbours' swaps: the last one up to the first's place, then the first up)
  local pairs_ = {}
  if b == a + 1 then
    pairs_[1] = { a, b }
  else
    for i = b - 1, a, -1 do pairs_[#pairs_ + 1] = { i, i + 1 } end
    for i = a + 1, b - 1 do pairs_[#pairs_ + 1] = { i, i + 1 } end
  end
  for _, pair in ipairs(pairs_) do ops.swap_tracks(pair[1], pair[2]) end
  -- (measured: a track a swap leaves just before an EMPTY group becomes its track, unless the swap passes the group)
  local joined
  for g, t in ipairs(tracks) do
    if not internal and t.kind == "group" and t.members == 0 and g - 1 >= 1 and (a == g - 1 or b == g - 1) and b < g then
      t.members, joined = 1, t
    end
  end
  M.record(function()
    if joined then joined.members = joined.members - 1 end
    for i = #pairs_, 1, -1 do ops.swap_tracks(pairs_[i][1], pairs_[i][2]) end
  end, function()
    for _, pair in ipairs(pairs_) do ops.swap_tracks(pair[1], pair[2]) end
    if joined then joined.members = joined.members + 1 end
  end)
end

function M.api.undo()
  local u = M.data.undo
  local step = table.remove(u.stack)
  if not step then return end
  u.redoing = true
  for i = #step.writes, 1, -1 do step.writes[i].undo() end
  u.redoing = false
  u.redo[#u.redo + 1] = step
end

function M.api.redo()
  local u = M.data.undo
  local step = table.remove(u.redo)
  if not step then return end
  u.redoing = true
  for _, write in ipairs(step.writes) do write.redo() end
  u.redoing = false
  u.stack[#u.stack + 1] = step
end

-- (M4 n-song) whether any pattern has automation of a parameter (its data): Renoise's is_automated.
function M.refresh_automated(identity)
  local any = false
  for _, pattern in ipairs(M.data.patterns) do
    for _, data in pairs(pattern.tracks) do
      for _, automation in ipairs(data.automation or {}) do any = any or automation.parameter == identity end
    end
  end
  identity.automated = any
end

-- A pattern track's automation of a parameter, as the API shows it.
function M.api.automation_object(automation)
  automation.points_observable = automation.points_observable or M.observable()
  automation.playmode_observable = automation.playmode_observable or M.observable()
  local function setter(field)
    return function(value)
      local old = automation[field]
      automation[field] = value
      local observable = automation[field .. "_observable"]
      if observable then M.fire(observable) end
      M.record(function() automation[field] = old; if observable then M.fire(observable) end end,
        function() automation[field] = value; if observable then M.fire(observable) end end)
    end
  end
  -- (the device and parameter it is of: found by the parameter's data)
  local function dest()
    for _, t in ipairs(M.data.tracks) do
      for _, data in ipairs(t.devices) do
        for _, param in ipairs(data.params or {}) do
          if param == automation.parameter then return data, param end
        end
      end
    end
  end
  return object("renoise.PatternTrackAutomation", {
    length = 64, points_observable = automation.points_observable, playmode_observable = automation.playmode_observable,
  }, {
    dest_device = function() local data = dest() return data and device(data) or nil end,
    dest_parameter = function() local _, param = dest() return param and parameter(param) or nil end,
    playmode = function() return automation.playmode end,
    points = function()
      local copy = {}
      for i, point in ipairs(automation.points) do copy[i] = { time = point.time, value = point.value, scaling = point.scaling or 0 } end
      return copy
    end,
  }, {
    playmode = setter("playmode"),
    points = function(value)
      local copy = {}
      for i, point in ipairs(value) do copy[i] = { time = point.time, value = point.value, scaling = point.scaling } end
      setter("points")(copy)
    end,
  })
end

-- The producer's automation of a device parameter in a pattern: marks the parameter as automated.
function ops.automate(p, t, d, k, points, playmode)
  local parameter = M.data.tracks[t].devices[d].params[k]
  parameter.automated = true
  local pattern_track = M.data.patterns[p].tracks[t] or { alias = 0, lines = {} }
  M.data.patterns[p].tracks[t] = pattern_track
  pattern_track.automation = pattern_track.automation or {}
  pattern_track.automation[#pattern_track.automation + 1] = { parameter = parameter, playmode = playmode or 2, points = points or {} }
end

function ops.swap_tracks(a, b)
  M.data.tracks[a], M.data.tracks[b] = M.data.tracks[b], M.data.tracks[a]
  move_pattern_tracks(function(t) if t == a then return b elseif t == b then return a end return t end)
  fire(song_fields().tracks_observable, { type = "swap", index1 = a, index2 = b })
end

function ops.rename_track(index, name)
  local t = M.data.tracks[index]
  t.name = name
  fire(t.name_observable)
end

function ops.mute_track(index, state)
  local t = M.data.tracks[index]
  t.mute_state = state
  fire(t.mute_state_observable)
end

-- M4 (n-song): Renoise's solo, measured in 3.5.4 (n-song lab 6 and 7). The soloed track (a root) and what it plays through
-- read soloed and are Active: its groups, a group's own tracks, the send tracks it feeds and, for a send, the tracks feeding
-- it; every other non-master track is Off. A script's solo is exclusive, and its setter toggles the track's own solo when the
-- value differs from what `solo_state` reads (false on a track soloed along with another solos that one on its own); an
-- un-solo of a root (or deleting it) turns every track Active, solos cleared. A producer's solo of several tracks in the GUI
-- (`ops.add_solo`) keeps several roots; un-soloing one of them ends them all here (not measured).
local function send_numbers(track)
  local out = {}
  for _, device in ipairs(track.devices or {}) do
    for _, parameter in ipairs(device.params or {}) do
      if parameter.name:match("^Receiver") and parameter.value >= 0 then out[#out + 1] = parameter.value end
    end
  end
  return out
end

local function solo_needs(i)
  local tracks = M.data.tracks
  local t = tracks[i]
  local set = { [i] = true }
  for g, u in ipairs(tracks) do
    if u.kind == "group" and g > i and i >= g - u.members then set[g] = true end
  end
  if t.kind == "group" then
    for k = i - t.members, i - 1 do set[k] = true end
  end
  local sends, n, mine = {}, 0, nil
  for k, u in ipairs(tracks) do
    if u.kind == "send" then
      sends[n] = k
      if k == i then mine = n end
      n = n + 1
    end
  end
  for _, number in ipairs(send_numbers(t)) do
    if sends[number] then set[sends[number]] = true end
  end
  if mine then
    for k, u in ipairs(tracks) do
      for _, number in ipairs(send_numbers(u)) do
        if number == mine then set[k] = true end
      end
    end
  end
  return set
end

function M.solo_refresh()
  local tracks = M.data.tracks
  local roots = M.data.solo_roots or {}
  local soloed = {}
  for _, root in ipairs(roots) do
    for k, u in ipairs(tracks) do
      if u == root then
        for j in pairs(solo_needs(k)) do soloed[j] = true end
      end
    end
  end
  for k, u in ipairs(tracks) do
    if u.kind ~= "master" then
      local was_solo, was_mute = u.solo or false, u.mute_state
      u.solo = soloed[k] or false
      u.mute_state = (#roots == 0 or soloed[k]) and 1 or 2
      if u.mute_state ~= was_mute then fire(u.mute_state_observable) end
      if u.solo ~= was_solo then fire(u.solo_state_observable) end
    end
  end
end

function M.solo_set(t, value)
  if t.kind == "master" then
    -- (measured: no error, it reads false, and every track turns Active)
    M.data.solo_roots = {}
    M.solo_refresh()
    return
  end
  if (t.solo or false) == (value == true) then return end
  local root = false
  for _, r in ipairs(M.data.solo_roots or {}) do root = root or r == t end
  M.data.solo_roots = root and {} or { t }
  M.solo_refresh()
end

-- The producer's solo button (exclusive, as a script's), and a solo added to another one (Renoise's GUI can; a script can't).
function ops.solo(index, value) M.solo_set(M.data.tracks[index], value) end

function ops.add_solo(index)
  local roots = M.data.solo_roots or {}
  roots[#roots + 1] = M.data.tracks[index]
  M.data.solo_roots = roots
  M.solo_refresh()
end

function ops.insert_device(index, spec, at)
  local t = M.data.tracks[index]
  table.insert(t.devices, at or (#t.devices + 1), spec)
  fire(t.devices_observable, { type = "insert", index = at or #t.devices })
end

function ops.insert_pattern(lines)
  local data = M.data
  data.patterns[#data.patterns + 1] = {
    name = "", lines = lines or 64, tracks = {}, name_observable = M.observable(),
    number_of_lines_observable = M.observable(), notifiers = {},
  }
  fire(song_fields().patterns_observable, { type = "insert", index = #data.patterns })
  return #data.patterns
end

function ops.insert_sequence_slot(pattern_index, at)
  local data = M.data
  table.insert(data.sequence, at or (#data.sequence + 1), pattern_index)
  fire(data.sequence_observable)
end

function ops.rename_pattern(p, name)
  local pattern = M.data.patterns[p]
  pattern.name = name
  fire(pattern.name_observable)
end

function ops.insert_sample(i, spec)
  local instrument = M.data.instruments[i]
  local given = { name = "Sample " .. (#instrument.samples + 1) }
  for k, v in pairs(spec or {}) do given[k] = v end
  instrument.samples[#instrument.samples + 1] = M.sample_data(given)
  instrument.samples_observable = instrument.samples_observable or M.observable()
  fire(instrument.samples_observable, { type = "insert", index = #instrument.samples })
end

function ops.insert_phrase(i)
  local instrument = M.data.instruments[i]
  instrument.phrases[#instrument.phrases + 1] = M.phrase_data({ name = "Phrase " .. (#instrument.phrases + 1) })
  instrument.phrases_observable = instrument.phrases_observable or M.observable()
  fire(instrument.phrases_observable, { type = "insert", index = #instrument.phrases })
end

-- The producer's instrument list edits: Renoise renumbers the notes as for the API's (an undo step of their own).
function ops.swap_instruments(a, b)
  M.instrument_list_op("swap", a, b, nil, true)
end

function ops.insert_instrument(spec)
  M.instrument_list_op("insert", #M.data.instruments + 1, nil, spec, true)
end

-- An instrument made at `i` (Renoise's insert_instrument_at): those after it move up one.
function ops.insert_instrument_at(i, spec)
  M.instrument_list_op("insert", i, nil, spec, true)
end

function ops.remove_instrument(i)
  M.instrument_list_op("remove", i, nil, nil, true)
end

function ops.remove_pattern(p)
  table.remove(M.data.patterns, p)
  fire(song_fields().patterns_observable, { type = "remove", index = p })
end

function ops.remove_sample(i, s)
  local instrument = M.data.instruments[i]
  table.remove(instrument.samples, s)
  fire(instrument.samples_observable, { type = "remove", index = s })
end

function ops.remove_device(t, d)
  local track = M.data.tracks[t]
  table.remove(track.devices, d)
  fire(track.devices_observable, { type = "remove", index = d })
end

function ops.rename_instrument(i, name)
  local instrument = M.data.instruments[i]
  instrument.name = name
  fire(M.obs(instrument, "name"))
end

function ops.select(key, value)
  local fields = song_fields()
  fields[key] = value
  if fields[key .. "_observable"] then fire(fields[key .. "_observable"]) end
end

-- Selected sample, phrase and device have no index observable of their own in the API.
function ops.select_sample(value)
  song_fields().selected_sample_index = value
  fire(song_fields().selected_sample_observable)
end

function ops.select_phrase(value)
  song_fields().selected_phrase_index = value
  fire(song_fields().selected_phrase_observable)
end

function ops.select_device(value)
  song_fields().selected_track_device_index = value
  fire(song_fields().selected_track_device_observable)
end

function ops.transport(key, value)
  local fields = M.data.transport_fields
  fields[key] = value
  if fields[key .. "_observable"] then fire(fields[key .. "_observable"]) end
end

function ops.play_at(sequence, k, beats)
  local pos = M.impl(M.data.transport_fields.playback_pos)
  pos.sequence, pos.line = sequence, k
  M.data.transport_fields.playback_pos_beats = beats or 0
end

end
