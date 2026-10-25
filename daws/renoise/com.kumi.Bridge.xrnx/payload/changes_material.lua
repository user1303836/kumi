-- What a change reads from the song to put it back, and writes to put it back: pattern lines, devices and their automation.
-- Values are Renoise's exact numbers.

local import = ...
local base = import("changes_base")
local rows = import("rows")

local fail = base.fail
local M = {}

-- One note column's content, as exact values.
function M.note_cell(column, c)
  return {
    col = c, note = column.note_value, instrument = column.instrument_value, volume = column.volume_value,
    panning = column.panning_value, delay = column.delay_value, fxn = column.effect_number_value, fxa = column.effect_amount_value,
  }
end

function M.effect_cell(column, c)
  return { col = c, number = column.number_value, amount = column.amount_value }
end

-- The lines of a pattern track that hold anything, and how many writes it takes to put them back.
function M.read_lines(pattern_track, total)
  local out, writes = {}, 0
  for l = 1, total do
    local line = pattern_track:line(l)
    if not line.is_empty then
      local row = { line = l, notes = {}, effects = {} }
      for c = 1, 12 do
        local column = line:note_column(c)
        if not column.is_empty then
          row.notes[#row.notes + 1] = M.note_cell(column, c)
          writes = writes + 7
        end
      end
      for c = 1, 8 do
        local column = line:effect_column(c)
        if not column.is_empty then
          row.effects[#row.effects + 1] = M.effect_cell(column, c)
          writes = writes + 2
        end
      end
      out[#out + 1] = row
    end
  end
  return out, writes
end

-- (`instruments` maps the instrument values kept to those to write, where the instruments moved since)
function M.write_lines(song, index, entry, instruments)
  local pattern_track = song:pattern(entry.pattern):track(index)
  if entry.alias then
    pattern_track.alias_pattern_index = entry.alias
    return
  end
  for _, row in ipairs(entry.lines) do
    local line = pattern_track:line(row.line)
    for _, cell in ipairs(row.notes) do
      local column = line:note_column(cell.col)
      local instrument = instruments and instruments[cell.instrument] or cell.instrument
      column.note_value, column.instrument_value, column.volume_value = cell.note, instrument, cell.volume
      column.panning_value, column.delay_value = cell.panning, cell.delay
      column.effect_number_value, column.effect_amount_value = cell.fxn, cell.fxa
    end
    for _, cell in ipairs(row.effects) do
      local column = line:effect_column(cell.col)
      column.number_value, column.amount_value = cell.number, cell.amount
    end
  end
end

local function parameter_named(device, name)
  for k = 1, #device.parameters do
    local parameter = device:parameter(k)
    if parameter.name == name then return parameter end
  end
  return nil
end

-- A device as a record keeps it: its place on its track, the mark of that track's devices' log then, and what it is (its path
-- and name, to check the place found is that device).
function M.device_print(device)
  return { path = device.device_path, name = device.display_name ~= "" and device.display_name or device.name }
end

function M.device_place(env, t, d, device)
  local mark = env and env.device_mark and env.device_mark(t) or nil
  if not mark then return nil end
  return { n = d, mark = mark, print = M.device_print(device) }
end

-- Where a kept device is now on the track now at `t`: its number, or nil and why ("unknown": what moved on the track since
-- can't be told; "gone"; "changed": the device at its place is not what it was).
function M.device_now(song, env, t, place)
  if type(place) ~= "table" or not env.device_at then return nil, "unknown" end
  local d, why = env.device_at(t, place.n, place.mark)
  if not d then return nil, why end
  local track = song:track(t)
  if d < 1 or d > #track.devices then return nil, "unknown" end
  -- (the track's devices' log follows the device: its name is no part of what it is; what kind of device it is, is)
  if track:device(d).device_path ~= (place.print or {}).path then return nil, "changed" end
  return d
end

-- The device a record keeps (`kept` = {track, dev (device_place), label}), where it is now: on the track its `track` number
-- was carried to (the layout), at its place followed through that track's device log. A gap in the log refuses `stale`, a
-- device deleted `gone`, another one at its place `changed`; nothing is written then. Returns t, d, track, device.
function M.device_by_place(song, env, kept, what)
  if type(kept) ~= "table" or type(kept.track) ~= "number" or type(kept.dev) ~= "table" then base.bad("material: where the device was") end
  local q = base.q
  local label = type(kept.label) == "string" and kept.label or "the device"
  local t = kept.track
  if t < 1 or t > rows.track_count(song) then fail("gone", string.format("the track %s was on isn't in the song any more", q(label))) end
  local track = song:track(t)
  local d, why = M.device_now(song, env, t, kept.dev)
  if not d then
    if why == "gone" then fail("gone", string.format("%s was deleted from %s since", q(label), q(base.named(t, track)))) end
    if why == "changed" then fail("changed", string.format("the device at %s's place on %s is another one now", q(label), q(base.named(t, track)))) end
    fail("stale", string.format("the devices on %s changed in a way Kumi couldn't follow since %s, so it can't tell where %s is: nothing was written",
      q(base.named(t, track)), what or "then", q(label)))
  end
  return t, d, track, track:device(d)
end

-- What was put back, and what could not be (a track or a device that is not the one the record was made for is left alone).
function M.new_tally() return { set = 0, failed = 0, skipped = 0, unknown = 0 } end

function M.count(tally, status)
  if status == "set" then tally.set = tally.set + 1
  elseif status == "failed" then tally.failed = tally.failed + 1
  elseif status == "unknown" then tally.unknown = tally.unknown + 1
  elseif status then tally.skipped = tally.skipped + 1 end
end

local function routings(count) return count == 1 and "1 routing" or (count .. " routings") end

function M.tally_note(tally)
  local parts = {}
  if tally.set > 0 then parts[#parts + 1] = string.format("%s aimed at it set again.", routings(tally.set)) end
  if tally.skipped > 0 then
    parts[#parts + 1] = string.format("%s could not be set again, as the tracks or devices %s changed in Renoise since: look at %s.",
      routings(tally.skipped), tally.skipped == 1 and "it names" or "they name", tally.skipped == 1 and "it" or "them")
  end
  if tally.unknown > 0 then
    parts[#parts + 1] = string.format("%s could not be set again: the song changed in a way Kumi couldn't follow, or the tool was reloaded, so it can't tell where %s now: look at %s.",
      routings(tally.unknown), tally.unknown == 1 and "it is" or "they are", tally.unknown == 1 and "it" or "them")
  end
  if tally.failed > 0 then parts[#parts + 1] = string.format("%s could not be set again: look at %s.", routings(tally.failed), tally.failed == 1 and "it" or "them") end
  return #parts > 0 and table.concat(parts, " ") or nil
end

-- Where a device sends values: each destination it has, as its parameters {prefix, track, effect, parameter?}: a meta
-- device's (and #Sidechain's) "Dest. Track/Effect/Parameter", *Hydra's and *XY Pad's "OutN Track/Effect/Parameter" (lab,
-- 3.5.4: #Sidechain has no Parameter). Track is a track's number less one, Effect a device's (the mixer device is 0), -1 none.
function M.destinations(device)
  local path = device.device_path
  if not (path:find("/Native/*", 1, true) or path:find("/Native/#Sidechain", 1, true)) then return {} end
  local named, numbered = {}, {}
  for k = 1, #device.parameters do
    local parameter = device:parameter(k)
    named[parameter.name], numbered[parameter.name] = parameter, k
  end
  local out = {}
  for k = 1, #device.parameters do
    local name = device:parameter(k).name
    local prefix = name:match("^(.*)Track$")
    if prefix and named[prefix .. "Effect"] then
      -- (and their numbers on the device, from 1)
      out[#out + 1] = {
        prefix = prefix, track = named[name], effect = named[prefix .. "Effect"], parameter = named[prefix .. "Parameter"],
        numbers = { k, numbered[prefix .. "Effect"], numbered[prefix .. "Parameter"] },
      }
    end
  end
  return out
end

local function destination_named(device, prefix)
  for _, destination in ipairs(M.destinations(device)) do
    if destination.prefix == prefix then return destination end
  end
  return nil
end

-- Where a device that is deleted sends its values, to aim it again when it is made again: its preset data doesn't hold them
-- (measured in Renoise 3.5.4: a device put back has none). `own` is the place of the track it is on; with `removing` that
-- track goes, and the tracks after it move up. Each is kept as the track's place (`to`, carried through the shifts since)
-- and the device's place on it (`dev`), never as Renoise's numbers.
local function read_destinations(device, song, own, removing, env)
  local out = {}
  for _, destination in ipairs(M.destinations(device)) do
    local target = destination.track.value + 1
    if destination.track.value >= 0 then
      local saved = { prefix = destination.prefix, effect = destination.effect.value, parameter = destination.parameter and destination.parameter.value }
      if target == own then
        saved.self = true
        out[#out + 1] = saved
      elseif target >= 1 and target <= rows.track_count(song) then
        saved.to = (removing and target > own) and target - 1 or target
        -- (the device it is aimed at, as a place on that track's devices: Effect counts from 0, the mixer device)
        local aimed = type(saved.effect) == "number" and saved.effect >= 0 and saved.effect + 1 or nil
        local target_track = song:track(target)
        if aimed and aimed <= #target_track.devices then
          saved.dev = M.device_place(env, target, aimed, target_track:device(aimed))
        end
        out[#out + 1] = saved
      end
    end
  end
  return #out > 0 and out or nil
end

-- A device as it is: enough to make it again. (`song` and `own`, the track's place, add where a meta device sends its values.)
function M.read_device(device, song, own, removing, env)
  return {
    path = device.device_path, name = device.name, display = device.display_name, active = device.is_active,
    maximized = device.is_maximized, preset = device.active_preset, presetData = device.active_preset_data,
    destinations = song and read_destinations(device, song, own, removing, env) or nil,
  }
end

-- Sets a device's destinations again, once the devices around it are there: `own` is the place of the track it is on,
-- now, and `moved(at, own)` where a track numbered as at the delete is now (nil when it is gone). Answers a status for each:
-- "set", or why it was left alone: "gone" (the track or the device is gone), "changed" (the device at its place is another),
-- "unknown" (what moved can't be told) or "failed".
function M.restore_destinations(song, device, saved, own, moved, env)
  local kept = saved.destinations
  if type(kept) ~= "table" and type(saved.destination) == "table" then
    kept = { saved.destination }
    kept[1].prefix = kept[1].prefix or "Dest. "
  end
  local statuses = {}
  for _, d in ipairs(type(kept) == "table" and kept or {}) do
    local destination = type(d) == "table" and destination_named(device, d.prefix or "Dest. ") or nil
    local status
    if destination and destination.track.value == -1 then
      local target = d.self and own or (type(d.to) == "number" and moved(d.to, own) or nil)
      if not target or target < 1 or target > rows.track_count(song) then
        status = "gone"
      else
        local effect = d.effect
        if not d.self then
          -- (the device it was aimed at, followed through the devices made, deleted and moved on that track since)
          if not d.dev then
            status = "unknown"
          else
            local at, why = M.device_now(song, env, target, d.dev)
            if not at then status = why else effect = at - 1 end
          end
        end
        if not status then
          local ok = pcall(function()
            destination.track.value = target - 1
            for _, pair in ipairs({ { destination.effect, effect }, { destination.parameter, d.parameter } }) do
              local parameter, value = pair[1], pair[2]
              if parameter and type(value) == "number" and value >= 0 and parameter.value == -1 then parameter.value = value end
            end
          end)
          status = ok and "set" or "failed"
        end
      end
    end
    if status then statuses[#statuses + 1] = status end
  end
  return statuses
end

-- A device put in at `at` as it was read (the state is the preset data; a plug-in that refuses it keeps its defaults).
function M.restore_device(track, at, saved)
  local device = track:insert_device_at(saved.path, at)
  if not device then fail("failed", "Renoise wouldn't make the device " .. tostring(saved.path)) end
  if saved.presetData and saved.presetData ~= "" then pcall(function() device.active_preset_data = saved.presetData end) end
  if saved.display and saved.display ~= "" then device.display_name = saved.display end
  device.is_active = saved.active ~= false
  if saved.maximized ~= nil then pcall(function() device.is_maximized = saved.maximized end) end
  return device
end

-- The patterns a record keeps by number: the pool's mark then, and what each was (its name and number of lines), to find each
-- again where it is now.
function M.keep_pool(song, env, numbers)
  local prints = {}
  for p in pairs(numbers) do
    if p >= 1 and p <= #song.patterns then
      local pattern = song:pattern(p)
      prints[tostring(p)] = { name = pattern.name, lines = pattern.number_of_lines }
    end
  end
  return { mark = env.pattern_mark and env.pattern_mark() or nil, prints = prints }
end

-- Where the kept patterns are now: a function from a pattern's number then to its number now (nil when it is gone or isn't
-- what it was), and one that only follows the pool (for any pattern, kept or not); nil when what happened to the pool since
-- can't be told.
function M.pool_now(song, env, pool)
  if type(pool) ~= "table" or not pool.mark or not env.pattern_at then return nil end
  local found = {}
  for key, print in pairs(pool.prints or {}) do
    local now, why = env.pattern_at(tonumber(key), pool.mark)
    if why == "unknown" then return nil end
    if now and now <= #song.patterns then
      local pattern = song:pattern(now)
      -- (the pool's log follows the pattern: a name is no part of what it is, and the producer renames freely; its length is)
      if pattern.number_of_lines == print.lines then found[tonumber(key)] = now end
    end
  end
  local followed = {}
  local function follow(p)
    if followed[p] == nil then followed[p] = env.pattern_at(p, pool.mark) or false end
    return followed[p] or nil
  end
  return function(p) return found[p] end, follow
end

-- The instruments the notes of a record play (a cell's instrument value counts from 0; 255 is none): the instruments' mark
-- then and the name of each, to find each again where it is now.
function M.keep_instruments(song, env, values)
  local prints = {}
  for v in pairs(values) do
    if v + 1 <= #song.instruments then prints[tostring(v)] = song:instrument(v + 1).name end
  end
  return { mark = env.instrument_mark and env.instrument_mark() or nil, prints = prints }
end

-- The instrument values to write for the kept ones: a table from then to now. Nil and why when one can't be put back as the
-- same instrument: "unknown" (what happened to the instruments can't be told), or the name (then) of one that is gone. The
-- instruments' log follows each: its name is no part of what it is (Renoise renames an instrument as a sample loads into it).
function M.instruments_now(song, env, kept)
  local map = {}
  if type(kept) ~= "table" or not next(kept.prints or {}) then return map end
  if not kept.mark or not env.instrument_at then return nil, "unknown" end
  for key, name in pairs(kept.prints) do
    local now, why = env.instrument_at(tonumber(key) + 1, kept.mark)
    if why == "unknown" then return nil, "unknown" end
    if not now or now > #song.instruments then return nil, name end
    map[tonumber(key)] = now - 1
  end
  return map
end

-- The automation of the track's parameters that have any, in every pattern: by device and parameter place, the mixer device's
-- (device 1: the track's volume, pan and width) too. `only` limits it to one device.
function M.read_automation(song, index, track, only)
  local out = {}
  for d = only or 1, only or #track.devices do
    local device = track:device(d)
    for k = 1, #device.parameters do
      local parameter = device:parameter(k)
      if parameter.is_automated then
        for p = 1, #song.patterns do
          local automation = song:pattern(p):track(index):find_automation(parameter)
          if automation then
            local points = {}
            for i, point in ipairs(automation.points) do
              points[i] = { time = point.time, value = point.value, scaling = point.scaling }
            end
            out[#out + 1] = { pattern = p, device = d, parameter = k, playmode = automation.playmode, points = points }
          end
        end
      end
    end
  end
  return out
end

-- The automation read above, for the patterns in `patterns` (a set), written to the devices now at the places it names.
function M.write_automation(song, track, index, list, patterns)
  for _, saved in ipairs(list or {}) do
    if patterns == nil or patterns[saved.pattern] then
      local parameter = track:device(saved.device):parameter(saved.parameter)
      local pattern_track = song:pattern(saved.pattern):track(index)
      local automation = pattern_track:find_automation(parameter) or pattern_track:create_automation(parameter)
      automation.playmode = saved.playmode
      automation.points = saved.points
    end
  end
end

-- A text's print: its two sums (changes_base.lua `sums`) and its length. (Kumi's host computes the same: presets.rs
-- `print_of`.)
local format, sums = string.format, base.sums
local function print_now(text)
  local a, b = sums(text)
  return format("%08x%08x:%d", a, b, #text)
end
-- (a text's print made now and kept nowhere: for a text derived from another, whose prints are kept under that one)
M.print_now = print_now

-- The prints of a text, kept by the text (`print` below, `shape` in changes_presets.lua, `content`; each made when it is
-- first asked for). A change prints what it replaces and what it wrote several times over (its fence, its record, the
-- journal's entry), every producer undo reads every journalled device's data again, and the journal polls its containers
-- about once a second: Lua gives the same text the same string, so a print made once serves them all. A kept text stays
-- in memory by its key, so past PRINTS_KEPT texts, or PRINTS_BYTES of them, the kept are let go together (and a text that
-- big by itself isn't kept).
M.PRINTS_KEPT, M.PRINTS_BYTES = 64, 32 * 1048576
local kept_prints, kept_count, kept_bytes = {}, 0, 0
function M.prints_of(text)
  local kept = kept_prints[text]
  if kept then return kept end
  kept = {}
  if #text > M.PRINTS_BYTES then return kept end
  if kept_count >= M.PRINTS_KEPT or kept_bytes + #text > M.PRINTS_BYTES then kept_prints, kept_count, kept_bytes = {}, 0, 0 end
  kept_prints[text], kept_count, kept_bytes = kept, kept_count + 1, kept_bytes + #text
  return kept
end

function M.print_of(text)
  local kept = M.prints_of(text)
  local printed = kept.print
  if printed == nil then
    printed = print_now(text)
    kept.print = printed
  end
  return printed
end

-- Preset data as it reads, values and all, with what moves by itself left out: the inner devices' SelectedPresetIsModified and
-- IsSelected (Renoise 3.5.4 turns the first true when it writes a container's data again, and the second follows the
-- producer's clicks, lab 2026-10-20), and the view (a device folded or not, a Doofer's devices shown, the chain a Splitter
-- shows or monitors, where a plug-in's editor sits): what the journal compares to tell Renoise's undo and redo of a Doofer's,
-- a Splitter's or a Notepad's contents, and what a change's fence of them holds.
M.VIEW_STATE = { "IsMaximized", "ShowDevices", "CurrentlyVisibleChain", "MonitorVisibleChain", "PluginEditorWindowPosition" }
function M.content_of(xml)
  local kept = M.prints_of(xml)
  local printed = kept.content
  if printed == nil then
    local content = xml:gsub("<SelectedPresetIsModified>[^<]*</SelectedPresetIsModified>", "<SelectedPresetIsModified/>")
    content = content:gsub("<IsSelected>[^<]*</IsSelected>", "<IsSelected/>")
    for _, tag in ipairs(M.VIEW_STATE) do
      content = content:gsub("<" .. tag .. ">[^<]*</" .. tag .. ">", "<" .. tag .. "/>")
    end
    printed = print_now(content)
    kept.content = printed
  end
  return printed
end

-- A device whose settings hold more than its parameters: a Doofer's or a Splitter's devices inside, a Notepad's text, a
-- plug-in's state. Its whole content is fenced, not only its parameters (nil for any other device).
local DEEP = { ["Audio/Effects/Native/Doofer"] = true, ["Audio/Effects/Native/Splitter"] = true, ["Audio/Effects/Native/Notepad"] = true }
function M.is_deep(device)
  local path = device.device_path
  return DEEP[path] == true or not path:find("^Audio/Effects/Native/")
end
-- (`data`: the device's preset data as the caller read it a moment ago, else it is read here)
function M.content_fence(device, data)
  if M.is_deep(device) then return M.content_of(data or device.active_preset_data) end
  return nil
end

-- A device told apart from another by what it is and how it is set: a state that two reads of a device that nobody touched
-- agree on (its parameters' values, not the preset text, which can be megabytes).
function M.device_state(device)
  local count = #device.parameters
  local values = {}
  for k = 1, count do values[k] = format("%.9g|", device:parameter(k).value) end
  return {
    path = device.device_path, display = device.display_name, active = device.is_active, parameters = count,
    state = base.hash(table.concat(values)),
  }
end

-- Routing aimed at a track or a device: a #Send's Receiver (the number of a send track among the sends) and a meta device's
-- destination (Dest. Track, Dest. Effect, Dest. Parameter: 0-based, -1 is none). Renoise follows tracks and devices being made, moved
-- and swapped, but clears these when their target is deleted and doesn't set them again when it comes back (measured in 3.5.4).
-- Kumi keeps what was aimed at what it deletes, and sets it again where nothing has touched it since: the device is found by its
-- place on its track (`dev`, followed through that track's devices made, deleted and moved since), on the track found through the
-- shifts since (`at`).

-- The number of the send track at `index` among the sends.
local function send_number(song, index)
  local n = 0
  for i = 1, index - 1 do
    if song:track(i).type == renoise.Track.TRACK_TYPE_SEND then n = n + 1 end
  end
  return n
end

-- The devices (on other tracks, or on the track itself for a device) aimed at track `victim` (a send, for receivers; a sequencer
-- track for destinations) or, with `device_at`, at that device of the track. Their tracks are numbered as they are when the target is
-- gone: tracks after a deleted track are one less.
function M.read_routes(song, victim, device_at, env)
  local routes = {}
  local victim_track = song:track(victim)
  local is_send = victim_track.type == renoise.Track.TRACK_TYPE_SEND
  local number = (is_send and not device_at) and send_number(song, victim) or nil
  for t = 1, rows.track_count(song) do
    if device_at or t ~= victim then
      local track = song:track(t)
      for d = 2, #track.devices do
        if not (t == victim and d == device_at) then
          local device = track:device(d)
          local path = device.device_path
          local route
          if number and path:find("#", 1, true) then
            for k = 1, #device.parameters do
              local parameter = device:parameter(k)
              if parameter.name:match("^Receiver") and parameter.value == number then
                route = route or {}
                route.receivers = route.receivers or {}
                route.receivers[#route.receivers + 1] = parameter.name
              end
            end
          end
          -- (destinations count every track: sequencer, group, send, master)
          for _, destination in ipairs(M.destinations(device)) do
            if destination.track.value == victim - 1 then
              local effect, parameter = destination.effect, destination.parameter
              if not device_at or effect.value == device_at - 1 then
                route = route or {}
                route.destinations = route.destinations or {}
                route.destinations[#route.destinations + 1] = { prefix = destination.prefix, effect = effect.value, parameter = parameter and parameter.value }
              end
            end
          end
          if route then
            route.at = (not device_at and t > victim) and t - 1 or t
            route.device, route.path = d, path
            -- (the device the route is on, as a place on its track's devices)
            route.dev = M.device_place(env, t, d, device)
            routes[#routes + 1] = route
          end
        end
      end
    end
  end
  return routes
end

-- Sets the routes again after the target came back: the track at `now.track`, or the device at `now.device` of the track
-- `now.on`. `moved(at, inserted)` is where a route's track is now (with the track put back in), or nil when it is gone. Counts in
-- `tally` how many were set and how many were left alone or refused by Renoise.
function M.write_routes(song, routes, now, moved, tally, env)
  for _, route in ipairs(routes or {}) do
    local t = type(route.at) == "number" and moved(route.at, now.track) or nil
    local track = t and t >= 1 and t <= rows.track_count(song) and song:track(t) or nil
    -- (the device the route is on, followed through the devices made, deleted and moved on its track since)
    local d, why
    if not track then
      why = "gone"
    elseif not route.dev then
      why = "unknown"
    else
      d, why = M.device_now(song, env, t, route.dev)
    end
    if not d or not track then
      M.count(tally, why)
    else
      local device = track:device(d)
      if device.device_path ~= route.path then
        M.count(tally, "changed")
      else
        local ok, set = pcall(function()
          local set = false
          if route.receivers and now.track then
            local number = send_number(song, now.track)
            for _, name in ipairs(route.receivers) do
              local parameter = parameter_named(device, name)
              if parameter and parameter.value == -1 then
                parameter.value = number
                set = true
              end
            end
          end
          local kept = route.destinations
          if type(kept) ~= "table" and type(route.destination) == "table" then
            kept = { route.destination }
            kept[1].prefix = kept[1].prefix or "Dest. "
          end
          for _, saved in ipairs(type(kept) == "table" and kept or {}) do
            local destination = destination_named(device, saved.prefix or "Dest. ")
            local function put(p, value)
              if p and type(value) == "number" and value >= 0 and p.value == -1 then p.value = value end
            end
            if destination then
              local track_p, effect_p, parameter_p = destination.track, destination.effect, destination.parameter
              if now.track then
                if track_p.value == -1 then
                  track_p.value = now.track - 1
                  put(effect_p, saved.effect)
                  put(parameter_p, saved.parameter)
                  set = true
                end
              elseif track_p.value == now.on - 1 and effect_p.value == -1 then
                put(effect_p, now.device - 1)
                put(parameter_p, saved.parameter)
                set = true
              end
            end
          end
          return set
        end)
        M.count(tally, not ok and "failed" or (set and "set" or nil))
      end
    end
  end
end

-- What to say about the routes when something is deleted: how many are aimed at it.
function M.routes_note(count)
  if count == 0 then return nil end
  return string.format("%s aimed at it (sends, modulation targets): Renoise lets go of them when it goes, and Kumi sets them again if it is put back.", routings(count))
end

return M
