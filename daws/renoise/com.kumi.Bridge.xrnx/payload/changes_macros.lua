-- Kumi's changes to an instrument's macro mappings (M4 n-devices). A mapping's range and scaling are written through the
-- API (`set`). Adding, removing or retargeting mappings goes through the instrument's file: the tool saves the instrument
-- into Kumi's work folder without a dialog (`instrument_export`), Kumi's host edits its Instrument.xml and packs it again
-- (crates/daw-mcp-server/src/instruments.rs), and the tool loads it back into the same slot in one step (`file`), its name
-- kept. Both are undone by Kumi as a new step: the ranges written back, or the host's restore file loaded.
--
-- Renoise 3.5.4 (lab): save_instrument writes the selected instrument and overwrites silently; load_instrument loads into
-- the selected slot, is told as an insert after it and a remove of it, keeps the notes that play it, and renames the
-- instrument after the file. 4.58 M sample frames take about 250 ms each way, so bigger instruments are refused.

local import = ...
local refs = import("refs")
local base = import("changes_base")
local presets = import("changes_presets")
local json = import("json")
local instruments_ = import("instruments")

local bad, fail, q = base.bad, base.fail, base.q
local M = {}

-- The most sample frames (times channels) an instrument may hold for Kumi to save and load it: about half a second of
-- Renoise's time each way.
M.MOST_FRAMES = 8000000
local SCALINGS = { "log-fast", "log-slow", "linear", "exp-slow", "exp-fast" }

local label_of = instruments_.instrument_label

local function instrument_by_ref(song, env, reference, what)
  local n = instruments_.numbers_of(env, "instrument", reference, what)
  return n[1], instruments_.resolve(song, "instrument", n).instrument
end

-- The instrument a record keeps: its place, followed through the instruments' log (places.lua).
local function instrument_by_place(song, env, kept)
  local n, why, list = instruments_.follow(env, kept)
  if not n then instruments_.follow_failed(why, list, type(kept) == "table" and kept.label or nil) end
  return n[1], instruments_.resolve(song, "instrument", n).instrument
end

local function target_of(env, i) return instruments_.target_of(env, "instrument", { i }) end

local function keep(env, i, instrument)
  local place = instruments_.place_of(env, "instrument", { i })
  place.label = label_of(i, instrument)
  return place
end

-- Its samples' frames (times channels), and how many are 32-bit (which FLAC keeps within 2^-31).
local function frames_of(instrument)
  local frames, floats = 0, 0
  for s = 1, #instrument.samples do
    -- (a sample without a buffer to read counts none)
    pcall(function()
      local buffer = instrument:sample(s).sample_buffer
      if buffer.has_sample_data then
        frames = frames + buffer.number_of_frames * buffer.number_of_channels
        if buffer.bit_depth == 32 then floats = floats + 1 end
      end
    end)
  end
  return frames, floats
end

local function too_big(i, instrument)
  local frames = frames_of(instrument)
  if frames > M.MOST_FRAMES then
    bad(string.format("%s holds %.1f M sample frames: saving and loading it again would hold Renoise for over a second each way, so Kumi maps its macros only through the API's ranges (set); map it in Renoise",
      q(label_of(i, instrument)), frames / 1000000))
  end
end

-- The macros and their mappings as the API reads them, and their print (what the producer's edits move).
local function macro_rows(instrument)
  local rows = json.array()
  for k = 1, 8 do
    local macro = instrument:macro(k)
    local mappings = json.array()
    for j = 1, #macro.mappings do
      local mapping = macro:mapping(j)
      mappings[j] = { param = mapping.parameter.name, min = mapping.parameter_min, max = mapping.parameter_max, scaling = SCALINGS[mapping.parameter_scaling] or "linear" }
    end
    rows[k] = { name = macro.name, mappings = mappings }
  end
  return rows
end

local function mappings_print(instrument)
  local parts = {}
  for k, row in ipairs(macro_rows(instrument)) do
    for _, mapping in ipairs(row.mappings) do
      parts[#parts + 1] = string.format("%d|%s|%.6f|%.6f|%s", k, mapping.param, mapping.min, mapping.max, mapping.scaling)
    end
    parts[#parts + 1] = k .. ":" .. #row.mappings
  end
  return presets.print_of(table.concat(parts, "\n"))
end

-- Its sample FX chains and modulation sets with their devices and parameters' names, as the API numbers them (a chain's
-- first device is its mixer device; a modulation set's list leaves out its hidden mixer).
local function chains_of(instrument)
  local function devices(list)
    local out = json.array()
    for d = 1, #list do
      local device = list[d]
      local params = json.array()
      for k = 1, #device.parameters do params[k] = device.parameters[k].name end
      out[d] = { name = device.name, params = params }
    end
    return out
  end
  local fx, modulation = json.array(), json.array()
  pcall(function()
    for c = 1, #instrument.sample_device_chains do
      local chain = instrument.sample_device_chains[c]
      fx[c] = { name = chain.name, devices = devices(chain.devices) }
    end
  end)
  pcall(function()
    for c = 1, #instrument.sample_modulation_sets do
      local set = instrument.sample_modulation_sets[c]
      modulation[c] = { name = set.name, devices = devices(set.devices) }
    end
  end)
  return { fx = fx, modulation = modulation }
end

-- Files in Kumi's work folder older than this are left from a change that never finished: the next export removes them.
local STALE_SECONDS = 3600
local serial = 0

local function work_dir(env)
  local dir = env.work_dir and env.work_dir()
  if not dir then fail("failed", "Kumi's bridge isn't set up (no work folder)") end
  return dir
end

-- A file in Kumi's work folder (the host names the one it made beside the export): an .xrni there, and nothing else.
local function work_file(env, file)
  local dir = work_dir(env)
  if type(file) ~= "string" or file:sub(1, #dir + 1) ~= dir .. "/" or not file:match("%.xrni$") or file:find("..", 1, true)
    or file:sub(#dir + 2):find("[/\\]") then
    bad("file: an instrument file in Kumi's work folder")
  end
  if not io.exists(file) then fail("failed", "the instrument file Kumi's host made isn't there") end
  return file
end

-- A new file name in the work folder (made if missing), its old leftovers removed.
local function new_work_file(env)
  local dir = work_dir(env)
  if not io.exists(dir) then os.mkdir(dir) end
  if not io.exists(dir) then fail("failed", "Kumi couldn't make its work folder " .. dir) end
  pcall(function()
    for _, name in ipairs(os.filenames(dir, { "*.xrni" })) do
      local stat = io.stat(dir .. "/" .. name)
      if stat and stat.mtime and os.time() - stat.mtime > STALE_SECONDS then os.remove(dir .. "/" .. name) end
    end
  end)
  serial = serial + 1
  return string.format("%s/instrument-%d-%d.xrni", dir, os.time(), serial)
end

-- Runs fn with the instrument selected (save_instrument and load_instrument act on the selected one), then selects what was.
local with_selected = import("changes_instruments").with_selected

-- macro_read {ref, epoch, layout}: an instrument's macros and their mappings as Renoise's API reads them (no file).
function M.read(params, env)
  local song = base.context(params, env)
  local i, instrument = instrument_by_ref(song, env, params.ref, "macro_read")
  local macros = macro_rows(instrument)
  for k, row in ipairs(macros) do
    row.macro, row.value = k, instrument:macro(k).value
    for j, mapping in ipairs(row.mappings) do mapping.mapping = j end
  end
  return { epoch = env.epoch(), layout = env.layout(), ref = refs.make(env.epoch(), "instrument", i), macros = macros, print = mappings_print(instrument) }
end

-- instrument_export {ref | at, epoch, layout}: the instrument saved to a new file in Kumi's work folder, with what the host
-- needs to edit it (its macros as the API reads them, its chains' devices and parameters' names, the print it fences).
function M.export(params, env)
  local song = base.context(params, env)
  local i, instrument
  if params.ref ~= nil then i, instrument = instrument_by_ref(song, env, params.ref, "instrument_export")
  else i, instrument = instrument_by_place(song, env, params.at) end
  too_big(i, instrument)
  local file = new_work_file(env)
  local saved
  with_selected(song, i, function() saved = renoise.app():save_instrument(file) end)
  if saved == false or not io.exists(file) then fail("failed", "Renoise couldn't save " .. q(label_of(i, instrument))) end
  local frames, floats = frames_of(instrument)
  return {
    epoch = env.epoch(), layout = env.layout(), ref = refs.make(env.epoch(), "instrument", i), file = file, name = instrument.name,
    label = label_of(i, instrument), frames = frames, floats = floats, print = mappings_print(instrument), macros = macro_rows(instrument),
    chains = chains_of(instrument), at = keep(env, i, instrument), target = target_of(env, i),
  }
end

-- instrument-macro-map

local function checked_set(macro, set)
  if type(set) ~= "table" or #set == 0 then bad("set: [{mapping, min?, max?, scaling?}]") end
  local out = {}
  for n, entry in ipairs(set) do
    local j = entry.mapping
    if type(j) ~= "number" or j ~= math.floor(j) or j < 1 or j > #macro.mappings then
      bad(string.format("set[%d].mapping is a number from 1 to %d (the macro's mappings)", n, #macro.mappings))
    end
    local row = { mapping = j }
    for _, field in ipairs({ "min", "max" }) do
      if entry[field] ~= nil then
        if type(entry[field]) ~= "number" or entry[field] < 0 or entry[field] > 1 then bad(string.format("set[%d].%s is from 0 to 1", n, field)) end
        row[field] = entry[field]
      end
    end
    if entry.scaling ~= nil then
      local found
      for k, word in ipairs(SCALINGS) do if word == entry.scaling then found = k end end
      if not found then bad("scaling is log-fast, log-slow, linear, exp-slow or exp-fast") end
      row.scaling = entry.scaling
    end
    out[#out + 1] = row
  end
  return out
end

local function ranges_of(macro, set)
  local out = {}
  for n, row in ipairs(set) do
    local mapping = macro:mapping(row.mapping)
    out[n] = { mapping = row.mapping, min = mapping.parameter_min, max = mapping.parameter_max, scaling = SCALINGS[mapping.parameter_scaling] or "linear" }
  end
  return out
end

local function write_ranges(macro, ranges)
  for _, row in ipairs(ranges) do
    local mapping = macro:mapping(row.mapping)
    if row.min ~= nil then mapping.parameter_min = row.min end
    if row.max ~= nil then mapping.parameter_max = row.max end
    if row.scaling ~= nil then
      for k, word in ipairs(SCALINGS) do if word == row.scaling then mapping.parameter_scaling = k end end
    end
  end
end

local function same_ranges(a, b)
  for n, row in ipairs(a) do
    local other = b[n]
    if not other or other.mapping ~= row.mapping then return false end
    for _, field in ipairs({ "min", "max" }) do
      if row[field] ~= nil and other[field] ~= nil and not base.close(row[field], other[field]) then return false end
    end
    if row.scaling ~= nil and other.scaling ~= nil and row.scaling ~= other.scaling then return false end
  end
  return true
end

local function macro_number(input)
  local m = input.macro
  if type(m) ~= "number" or m ~= math.floor(m) or m < 1 or m > 8 then bad("macro is a number from 1 to 8") end
  return m
end

local function ranges_title(name, m, set)
  local parts = {}
  for _, row in ipairs(set) do
    local bits = {}
    if row.min ~= nil or row.max ~= nil then bits[#bits + 1] = string.format("%s to %s", tostring(row.min or "…"), tostring(row.max or "…")) end
    if row.scaling ~= nil then bits[#bits + 1] = row.scaling end
    parts[#parts + 1] = string.format("mapping %d %s", row.mapping, table.concat(bits, ", "))
  end
  return string.format("%s macro %d: %s", name, m, table.concat(parts, "; "))
end

-- Loads the host's file into the slot, keeping the instrument's name and the producer's selection.
local function load_file(song, env, i, file, name)
  with_selected(song, i, function()
    env.replacing_instrument(i, function()
      local loaded = instruments_.load_xrni(song, i, file)
      if loaded == false then error({ code = "failed", message = "Renoise couldn't load the instrument file Kumi's host made" }, 0) end
    end)
    if type(name) == "string" then song:instrument(i).name = name end
  end)
end

-- Runs fn(file) with the host's file (one in Kumi's work folder, or nothing runs), which is removed after, loaded or not.
local function with_host_file(env, file, fn)
  file = work_file(env, file)
  local ok, result = pcall(fn, file)
  os.remove(file)
  if not ok then error(result, 0) end
  return result
end

base.register({
  kind = "instrument-macro-map", custom = true, places = true,
  preview = function(song, env, input)
    if input.file ~= nil then bad("instrument-macro-map with mappings is made by Kumi's host: ask it through the host's change_preview") end
    local i, instrument = instrument_by_ref(song, env, input.ref, "instrument-macro-map")
    local m = macro_number(input)
    local macro = instrument:macro(m)
    local set = checked_set(macro, input.set)
    local now = ranges_of(macro, set)
    return {
      target = target_of(env, i), prior = { ranges = now }, fence = { print = mappings_print(instrument) },
      display = { title = ranges_title(label_of(i, instrument), m, set), unchanged = same_ranges(now, set) or nil },
    }
  end,
  apply = function(song, env, input, fence, desc)
    local ok, i, instrument = pcall(instrument_by_ref, song, env, input.ref, "instrument-macro-map")
    if not ok then base.gone_is_changed(i) end
    local name = label_of(i, instrument)
    local m = macro_number(input)
    if input.file ~= nil then
      return with_host_file(env, input.file, function(file)
        -- The host's file: what it was made from must be what is there.
        if input.print ~= mappings_print(instrument) then fail("changed", string.format("%s's macros changed in Renoise since Kumi read it: read it again", q(name))) end
        too_big(i, instrument)
        local before = mappings_print(instrument)
        song:describe_undo(desc)
        load_file(song, env, i, file, instrument.name)
        instrument = song:instrument(i)
        local _, floats = frames_of(instrument)
        local display = type(input.display) == "table" and input.display or { title = string.format("%s macro %d mapped", name, m) }
        if floats > 0 then
          display.note = (display.note and (display.note .. " ") or "") .. "Renoise saved its 32-bit samples as FLAC, which keeps them within 2^-31."
        end
        return {
          applied = { print = mappings_print(instrument), edit = input.edit }, material = { at = keep(env, i, instrument) },
          target = target_of(env, i), display = display, undo = { desc = desc, entry = true },
          prior = { print = before }, journal_applied = { print = mappings_print(instrument) }, place = { instrument = i },
        }
      end)
    end
    local macro = instrument:macro(m)
    if fence.print ~= mappings_print(instrument) then fail("changed", string.format("%s's macros changed since the preview", q(name))) end
    local set = checked_set(macro, input.set)
    local now = ranges_of(macro, set)
    local unchanged = same_ranges(now, set)
    if not unchanged then
      song:describe_undo(desc)
      write_ranges(macro, set)
    end
    local after = ranges_of(macro, set)
    if not same_ranges(after, set) then
      pcall(write_ranges, macro, now)
      fail("failed", string.format("Renoise kept %s macro %d's mappings as they were", name, m))
    end
    return {
      applied = { ranges = after, print = mappings_print(instrument) }, material = { at = keep(env, i, instrument), macro = m },
      target = target_of(env, i), display = { title = ranges_title(name, m, set) }, undo = { desc = desc, entry = not unchanged },
      prior = { print = fence.print }, journal_applied = { print = mappings_print(instrument) }, place = { instrument = i },
    }
  end,
  restore = function(song, env, restore, desc)
    local material, applied = restore.material, restore.applied
    if type(material) ~= "table" or type(applied) ~= "table" then bad("material and applied: the instrument and what the change left") end
    local i, instrument = instrument_by_place(song, env, material.at)
    local name = label_of(i, instrument)
    if type(material.write) == "table" then
      return with_host_file(env, material.write.file, function(file)
        -- The host's restore file, made from the instrument as it is now (`print`).
        if material.write.print ~= mappings_print(instrument) then fail("changed", string.format("%s's macros changed in Renoise since Kumi read it", q(name))) end
        too_big(i, instrument)
        song:describe_undo(desc)
        load_file(song, env, i, file, instrument.name)
        base.note_restored_at(env.journal, "instrument-macro-map", { instrument = i })
        local display = type(material.write.display) == "table" and material.write.display or { title = name .. " macro mappings put back" }
        return { restored = { print = mappings_print(song:instrument(i)) }, display = display, undo = { desc = desc, entry = true } }
      end)
    end
    local prior = restore.prior
    if type(prior) ~= "table" or type(prior.ranges) ~= "table" or type(applied.ranges) ~= "table" then bad("prior and applied: the ranges") end
    local m = material.macro
    local macro = instrument:macro(m)
    for _, row in ipairs(prior.ranges) do
      if row.mapping > #macro.mappings then fail("changed", string.format("%s macro %d's mappings changed in Renoise since", name, m)) end
    end
    local now = ranges_of(macro, prior.ranges)
    if same_ranges(now, prior.ranges) then
      return { restored = { ranges = now }, display = { title = string.format("%s macro %d's mappings are as they were already", name, m) }, undo = { desc = desc, entry = false }, unwritten = true }
    end
    if not restore.discard and not same_ranges(now, applied.ranges) then
      fail("changed", string.format("%s macro %d's mappings changed in Renoise since Kumi set them", name, m))
    end
    song:describe_undo(desc)
    write_ranges(macro, prior.ranges)
    base.note_restored_at(env.journal, "instrument-macro-map", { instrument = i })
    return { restored = { ranges = ranges_of(macro, prior.ranges) }, display = { title = string.format("%s macro %d's mappings put back", name, m) }, undo = { desc = desc, entry = true } }
  end,
  reread = function(song, place)
    if place.instrument > #song.instruments then return nil end
    return song:instrument(place.instrument)
  end,
  read = function(instrument) return { print = mappings_print(instrument) } end,
  same = function(a, b) return a.print == b.print end,
})

return M
