-- Where an instrument, and what is in it, is for Kumi's changes (M4): its samples, phrases, modulation sets and their devices,
-- sample FX chains and their devices. A change finds its object by its ref when it is made; its record keeps the object's
-- place (`target.place`): each number with the mark of the list's log then (places.lua), so a restore finds the same object
-- wherever the producer's or Kumi's later inserts, deletes and swaps moved it, or says it can't (`gone`, `stale`) and touches
-- nothing. The tool's journal keeps plain numbers (`instrument`, `sample`, ...), which it moves with the same operations.

local import = ...
local refs = import("refs")
local rows = import("rows")

local M = {}

local function fail(code, message) error({ code = code, message = message }, 0) end
local function bad(message) fail("bad-params", message) end
M.fail, M.bad = fail, bad

-- A name inside a sentence is in curly quotes.
local function q(text) return "\226\128\156" .. tostring(text) .. "\226\128\157" end
M.q = q

-- What the producer calls each object: its name, or the number Renoise shows (instruments and samples count from 00, in hex;
-- phrases from 01, as Renoise names a new one).
function M.instrument_label(i, instrument)
  local name = instrument.name
  if name == nil or name == "" then return string.format("Instrument %02X", i - 1) end
  return name
end

function M.sample_label(s, sample)
  local name = sample.name
  if name == nil or name == "" then return string.format("Sample %02X", s - 1) end
  return name
end

function M.phrase_label(ph, phrase)
  local name = phrase.name
  if name == nil or name == "" then return string.format("Phrase %02d", ph) end
  return name
end

function M.set_label(m, set)
  local name = set.name
  if name == nil or name == "" then return "Modulation " .. m end
  return name
end

function M.chain_label(c, chain)
  local name = chain.name
  if name == nil or name == "" then return "FX " .. c end
  return name
end

function M.device_label(device)
  local name = device.display_name
  if name == nil or name == "" then name = device.name end
  return name
end

-- The kinds of object, each with its ref kind, how many numbers its ref has, the list each number is in and the key of its
-- number in a journal place.
local SHAPES = {
  instrument = { lists = { "instruments" }, keys = { "instrument" } },
  sample = { lists = { "instruments", "samples" }, keys = { "instrument", "sample" } },
  phrase = { lists = { "instruments", "phrases" }, keys = { "instrument", "phrase" } },
  modset = { lists = { "instruments", "modsets" }, keys = { "instrument", "modset" } },
  moddevice = { lists = { "instruments", "modsets", "moddevices" }, keys = { "instrument", "modset", "moddevice" } },
  chain = { lists = { "instruments", "chains" }, keys = { "instrument", "chain" } },
  chaindevice = { lists = { "instruments", "chains", "chaindevices" }, keys = { "instrument", "chain", "chaindevice" } },
}
M.SHAPES = SHAPES

local WHAT = {
  instruments = "instrument", samples = "sample", phrases = "phrase", modsets = "modulation set", moddevices = "modulation device",
  chains = "sample FX chain", chaindevices = "device",
}

-- The objects the numbers name, from the instrument down, or `gone` naming the first that isn't there.
function M.resolve(song, of, n)
  local shape = SHAPES[of]
  local objects = {}
  if n[1] < 1 or n[1] > #song.instruments then fail("gone", "that instrument isn't in the song any more") end
  local instrument = song:instrument(n[1])
  objects.instrument = instrument
  local list = shape.lists[2]
  if list == "samples" then
    if n[2] < 1 or n[2] > #instrument.samples then fail("gone", "that sample isn't in the instrument any more") end
    objects.sample = instrument:sample(n[2])
  elseif list == "phrases" then
    if n[2] < 1 or n[2] > #instrument.phrases then fail("gone", "that phrase isn't in the instrument any more") end
    objects.phrase = instrument:phrase(n[2])
  elseif list == "modsets" then
    if n[2] < 1 or n[2] > #instrument.sample_modulation_sets then fail("gone", "that modulation set isn't in the instrument any more") end
    objects.modset = instrument:sample_modulation_set(n[2])
    if n[3] then
      if n[3] < 1 or n[3] > #objects.modset.devices then fail("gone", "that modulation device isn't in the set any more") end
      objects.moddevice = objects.modset:device(n[3])
    end
  elseif list == "chains" then
    if n[2] < 1 or n[2] > #instrument.sample_device_chains then fail("gone", "that sample FX chain isn't in the instrument any more") end
    objects.chain = instrument:sample_device_chain(n[2])
    if n[3] then
      if n[3] < 1 or n[3] > #objects.chain.devices then fail("gone", "that device isn't in the chain any more") end
      objects.chaindevice = objects.chain:device(n[3])
    end
  end
  return objects
end

-- The numbers a ref of `of` names (in this epoch).
function M.numbers_of(env, of, reference, what)
  if type(reference) ~= "string" then bad((what or "ref") .. ": ref must be " .. (of == "instrument" and "an " or "a ") .. of .. " ref") end
  return refs.expect(reference, of, #SHAPES[of].lists, env.epoch())
end

-- The mark of each list the numbers are in, now: a place, `{of, at = {{n, mark}, ...}}` from the instrument down.
function M.place_of(env, of, n)
  local shape = SHAPES[of]
  local at = {}
  for k, list in ipairs(shape.lists) do
    local mark
    if list == "instruments" then
      mark = env.instrument_mark and env.instrument_mark() or nil
    else
      mark = env.list_mark and env.list_mark(list, n[1], n[2]) or nil
    end
    at[k] = { n = n[k], mark = mark }
  end
  return { of = of, at = at }
end

-- Where a place's object is now: its numbers, or nil and why ("gone": it was deleted; "unknown": what moved since can't be told).
function M.follow(env, place)
  local shape = type(place) == "table" and SHAPES[place.of]
  if not shape then return nil, "unknown" end
  local n = {}
  if type(place.at) ~= "table" then return nil, "unknown" end
  for k, list in ipairs(shape.lists) do
    local kept = place.at[k]
    if type(kept) ~= "table" or type(kept.n) ~= "number" or type(kept.mark) ~= "string" then return nil, "unknown" end
    local now, why
    if list == "instruments" then
      if not env.instrument_at then return nil, "unknown" end
      now, why = env.instrument_at(kept.n, kept.mark)
    else
      if not env.list_at then return nil, "unknown" end
      now, why = env.list_at(list, n[1], n[2], kept.n, kept.mark)
    end
    if not now then return nil, why, WHAT[list] end
    n[k] = now
  end
  return n
end

-- The refusal for a place that can't be followed, naming what it was.
function M.follow_failed(why, what, label)
  if why == "gone" then
    fail("gone", string.format("%s isn't in the song any more (the %s was deleted)", label and q(label) or "it", what or "object"))
  end
  fail("stale", string.format("the song's instruments changed in a way Kumi couldn't follow since (the tool watched the song anew, or more changed than it keeps), so it can't tell where %s is: nothing was written", label and q(label) or "it"))
end

-- The journal's place: plain numbers, which it moves itself.
function M.journal_place(of, n)
  local place = { of = of }
  for k, key in ipairs(SHAPES[of].keys) do place[key] = n[k] end
  return place
end

function M.ref_of(env, of, n)
  return refs.make(env.epoch(), of, unpack(n))
end

function M.target_of(env, of, n)
  return { ref = M.ref_of(env, of, n), kind = of, place = M.place_of(env, of, n) }
end

-- What a request names: by its ref (a preview, an apply), or by the place a record kept (a restore). Answers the numbers, the
-- objects and what the producer calls the object.
function M.locate(song, env, of, input, what)
  local n
  if type(input.place) == "table" then
    local now, why, list = M.follow(env, input.place)
    if not now then M.follow_failed(why, list, input.label) end
    n = now
  else
    n = M.numbers_of(env, of, input.ref, what)
  end
  local objects = M.resolve(song, of, n)
  return n, objects, M.label_of(of, n, objects)
end

function M.label_of(of, n, objects)
  local instrument = M.instrument_label(n[1], objects.instrument)
  if of == "instrument" then return instrument end
  if of == "sample" then return string.format("%s in %s", q(M.sample_label(n[2], objects.sample)), q(instrument)) end
  if of == "phrase" then return string.format("%s in %s", q(M.phrase_label(n[2], objects.phrase)), q(instrument)) end
  if of == "modset" then return string.format("%s in %s", q(M.set_label(n[2], objects.modset)), q(instrument)) end
  if of == "chain" then return string.format("%s in %s", q(M.chain_label(n[2], objects.chain)), q(instrument)) end
  if of == "moddevice" then
    return string.format("%s in %s of %s", q(M.device_label(objects.moddevice)), q(M.set_label(n[2], objects.modset)), q(instrument))
  end
  return string.format("%s in %s of %s", q(M.device_label(objects.chaindevice)), q(M.chain_label(n[2], objects.chain)), q(instrument))
end

-- The journal reads a place's object again (nil when it isn't there).
function M.reread(of)
  return function(song, place)
    local n = {}
    for k, key in ipairs(SHAPES[of].keys) do
      if type(place[key]) ~= "number" then return nil end
      n[k] = place[key]
    end
    local ok, objects = pcall(M.resolve, song, of, n)
    if not ok then return nil end
    return objects[of]
  end
end

-- An instrument's contents in brief: enough to tell it is empty, or the one Kumi made or left.
function M.instrument_print(instrument)
  local frames = 0
  for s = 1, #instrument.samples do
    local buffer = instrument:sample(s).sample_buffer
    if buffer.has_sample_data then frames = frames + buffer.number_of_frames end
  end
  local plugin = instrument.plugin_properties
  return {
    name = instrument.name, samples = #instrument.samples, phrases = #instrument.phrases, frames = frames,
    sets = #instrument.sample_modulation_sets, chains = #instrument.sample_device_chains,
    plugin = plugin.plugin_loaded and plugin.plugin_device.device_path or "",
  }
end

-- A sample's settings the producer works on (its slices, tuning, level, loop and keys): what the undo of a sample Kumi made
-- compares, so their later work in it is never taken away unsaid.
function M.sample_work(sample)
  local mapping = sample.sample_mapping
  return {
    name = sample.name, slices = #sample.slice_markers, transpose = sample.transpose, fine = sample.fine_tune,
    volume = string.format("%.5f", sample.volume), panning = string.format("%.5f", sample.panning), loop = sample.loop_mode,
    loopStart = sample.loop_start, loopEnd = sample.loop_end, base = mapping.base_note, notes = mapping.note_range[1] .. "-" .. mapping.note_range[2],
    velocity = mapping.velocity_range[1] .. "-" .. mapping.velocity_range[2],
  }
end

-- A plug-in's state in brief (a sum over every seventh byte, and its length): its state can be megabytes.
local function state_sum(data)
  local sum = 0
  for i = 1, #data, 7 do sum = (sum * 31 + data:byte(i)) % 4294967296 end
  return string.format("%08x:%d", sum, #data)
end

-- What the producer works on in an instrument, beyond its print: the devices of each modulation set and FX chain, its macros
-- and their mappings, its samples' settings and a plug-in's state. What the undo of an instrument Kumi made compares (M3 rule
-- 3: never take the producer's later work away unsaid).
function M.instrument_work(instrument)
  local out = M.instrument_print(instrument)
  local sets, chains, macros, samples = {}, {}, {}, {}
  for m = 1, #instrument.sample_modulation_sets do
    local set, names = instrument:sample_modulation_set(m), {}
    for d = 1, #set.devices do names[d] = set:device(d).name end
    sets[m] = table.concat(names, ",")
  end
  for c = 1, #instrument.sample_device_chains do
    local chain, paths = instrument:sample_device_chain(c), {}
    for d = 1, #chain.devices do paths[d] = chain:device(d).device_path end
    chains[c] = table.concat(paths, ",")
  end
  for k = 1, 8 do
    local ok, macro = pcall(function() return instrument:macro(k) end)
    if ok and macro then macros[k] = string.format("%s=%.4f/%d", macro.name, macro.value, #macro.mappings) end
  end
  for s = 1, #instrument.samples do samples[s] = M.sample_work(instrument:sample(s)) end
  out.setDevices, out.chainDevices, out.macros, out.sampleWork = sets, chains, macros, samples
  local plugin = instrument.plugin_properties
  if plugin.plugin_loaded then out.pluginState = state_sum(plugin.plugin_device.active_preset_data or "") end
  return out
end

-- The copies kept to put back, oldest first: past MAX_KEPT_FILES files or MAX_KEPT_BYTES the oldest go (their undo then says
-- the kept copy is gone), so a long session's deletes and replaces don't fill the disk before Renoise quits.
M.MAX_KEPT_FILES, M.MAX_KEPT_BYTES = 256, 2 * 1024 * 1024 * 1024
local kept_files = {}
local function prune_kept()
  local sizes, total = {}, 0
  for k, path in ipairs(kept_files) do
    local stat = io.exists(path) and io.stat(path) or nil
    sizes[k] = stat and stat.size or 0
    total = total + sizes[k]
  end
  while #kept_files > M.MAX_KEPT_FILES or (total > M.MAX_KEPT_BYTES and #kept_files > 1) do
    local path = table.remove(kept_files, 1)
    total = total - table.remove(sizes, 1)
    if io.exists(path) then pcall(os.remove, path) end
  end
end

-- Where Kumi keeps what it saves to put back (a deleted instrument's .xrni, a sample's audio): a folder of its own in Renoise's
-- temporary folder, which Renoise empties as it quits (Kumi's undo can't outlive the song anyway). Renoise's own name when that
-- folder can't be made.
function M.temp_file(extension)
  local name = os.tmpname(extension)
  local dir, sep, file = name:match("^(.*)([/\\])([^/\\]+)$")
  if not dir then return name end
  local own = dir .. sep .. "kumi"
  if not io.exists(own) then pcall(os.mkdir, own) end
  if not io.exists(own) then return name end
  if io.exists(name) then pcall(os.remove, name) end
  kept_files[#kept_files + 1] = own .. sep .. file
  prune_kept()
  return own .. sep .. file
end

-- Empty, as a new instrument is and clear() leaves one (measured): no samples, phrases, chains, plug-in or MIDI output device
-- (a hardware synth's slot is the producer's: clear() would reset its MIDI output), and at most its one modulation set, with
-- no devices in it.
function M.is_empty(instrument)
  local print = M.instrument_print(instrument)
  if print.samples > 0 or print.phrases > 0 or print.plugin ~= "" or print.chains > 0 or print.sets > 1 then return false end
  local midi_ok, device = pcall(function() return instrument.midi_output_properties.device_name end)
  if midi_ok and type(device) == "string" and device ~= "" then return false end
  return print.sets == 0 or #instrument:sample_modulation_set(1).devices == 0
end

-- Loads an .xrni into the selected instrument `i`: true when Renoise did. Renoise 3.5.4 (lab, 2026-10-20) first adds an empty
-- instrument after the last one when that is the one it loads into (told as insert@i+1 before its own insert@i+1 and remove@i)
-- and leaves it there; in the middle it adds none. Kumi takes that empty one out again, in the same step.
function M.load_xrni(song, i, path)
  local count = #song.instruments
  local loaded = renoise.app():load_instrument(path)
  if loaded and i == count and #song.instruments == count + 1 and M.is_empty(song:instrument(count + 1)) then
    song:delete_instrument_at(count + 1)
  end
  return loaded
end

-- The notes of the song that play instrument value `v` (counting from 0), within `budget` reads: a list of {pattern, track,
-- line, col, cell}, or nil when the song is too big to look through in one go. Every note column of every line with
-- something in it, hidden columns too. An alias's lines are its source pattern's: looked at there.
function M.notes_playing(song, v, budget)
  local found, reads = {}, 0
  local tracks = rows.track_count(song)
  for p = 1, #song.patterns do
    local pattern = song:pattern(p)
    for t = 1, tracks do
      local pattern_track = pattern:track(t)
      reads = reads + 1
      if not pattern_track.is_alias and not pattern_track.is_empty and song:track(t).max_note_columns > 0 then
        for l = 1, pattern.number_of_lines do
          local line = pattern_track:line(l)
          reads = reads + 1
          if not line.is_empty then
            for c = 1, 12 do
              local column = line:note_column(c)
              reads = reads + 1
              if column.instrument_value == v then
                found[#found + 1] = {
                  pattern = p, track = t, line = l, col = c, note = column.note_value, instrument = column.instrument_value,
                  volume = column.volume_value, panning = column.panning_value, delay = column.delay_value,
                  fxn = column.effect_number_value, fxa = column.effect_amount_value,
                }
              end
            end
          end
          if reads > budget then return nil end
        end
      end
    end
  end
  return found
end

return M
