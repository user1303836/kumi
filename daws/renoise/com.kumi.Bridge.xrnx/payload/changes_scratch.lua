-- scratch-keep (M5 contract §9): a scratch copy made a track of the producer's (a goal's best): renamed, turned on (a copy is OFF,
-- processing nothing), out of the scratch journal so no drop or sweep removes it. Kumi's undo deletes it while it is as Kumi left
-- it. The rename is a Renoise undo step; the mute isn't (M3 fact), so Renoise's own undo takes back the name only, and says so.
-- What Kumi left is fenced by a print of everything the producer could work on there (`content_print`): any later work on it, or
-- a track too long to print in one callback, and Kumi's undo leaves the track where it is.

local import = ...
local refs = import("refs")
local rows = import("rows")
local base = import("changes_base")
local scratch = import("scratch")
local fnv = import("fnv")

local format = string.format

local fail, bad, q = base.fail, base.bad, base.q

local M = {}

local NOTE = "Renoise's own undo takes back its name, not its mute (mute is no undo step in Renoise); Kumi's undo removes the track."

-- What tells the kept track apart from what Kumi left: its own name and kind, its mute, its devices and the patterns it has lines in.
local function signature(song, index, track)
  local lines = 0
  for p = 1, #song.patterns do
    local pattern_track = song:pattern(p):track(index)
    if pattern_track.is_alias or not pattern_track.is_empty then lines = lines + 1 end
  end
  return { id = base.track_id(track), mute = track.mute_state, devices = #track.devices, patternsWithLines = lines }
end

-- The most pattern lines a print reads (it is one callback: well under a second); past it the kept track is too long to check.
M.LINES_BUDGET = 100000

-- A 32-bit FNV-1a print (as digest.lua's) of what the producer could work on in the kept track: its routing, mixer, each
-- device (path, on/off, each parameter that isn't automated, its preset data), its slot mutes, and in every pattern its lines
-- (the shown columns) and automation. nil when its lines are past LINES_BUDGET.
local function content_print(song, index, track)
  local h = fnv.OFFSET
  local function feed(value)
    h = fnv.mark(fnv.feed(h, tostring(value) or ""), 31)
  end
  local function value(parameter)
    -- (an automated parameter moves as the song plays: its automation is printed instead)
    feed(parameter.is_automated and "a" or format("%.5f", parameter.value))
  end
  feed(track.output_routing)
  feed(format("%.3f", track.output_delay or 0))
  value(track.prefx_volume)
  value(track.prefx_panning)
  value(track.prefx_width)
  value(track.postfx_volume)
  value(track.postfx_panning)
  for d = 1, #track.devices do
    local device = track:device(d)
    feed(device.device_path or "")
    feed(device.is_active and "on" or "off")
    for _, parameter in ipairs(device.parameters) do value(parameter) end
    -- (a plug-in keeps settings no parameter shows)
    if d > 1 then feed(device.active_preset_data or "") end
  end
  local sequencer = song.sequencer
  for s = 1, #sequencer.pattern_sequence do
    if sequencer:track_sequence_slot_is_muted(index, s) then feed("muted " .. s) end
  end
  local note_columns, effect_columns = track.visible_note_columns, track.visible_effect_columns
  feed(note_columns .. "/" .. effect_columns)
  local budget = M.LINES_BUDGET
  for p = 1, #song.patterns do
    local pattern = song:pattern(p)
    local pattern_track = pattern:track(index)
    feed("p" .. p)
    if pattern_track.is_alias then
      feed("alias " .. pattern_track.alias_pattern_index)
    elseif not pattern_track.is_empty then
      local length = pattern.number_of_lines
      budget = budget - length
      if budget < 0 then return nil end
      for l = 1, length do
        local line = pattern_track:line(l)
        if not line.is_empty then
          feed(l)
          for c = 1, note_columns do
            local column = line:note_column(c)
            if not column.is_empty then
              feed(format("%d:%d %d %d %d %d %d %d", c, column.note_value, column.instrument_value, column.volume_value,
                column.panning_value, column.delay_value, column.effect_number_value, column.effect_amount_value))
            end
          end
          for c = 1, effect_columns do
            local column = line:effect_column(c)
            if not column.is_empty then feed(format("e%d:%d %d", c, column.number_value, column.amount_value)) end
          end
        end
      end
    end
    for _, entry in ipairs(pattern_track.automation) do
      local parameter = entry.dest_parameter
      feed(format("auto %s %d", parameter and parameter.name or "", entry.playmode))
      for _, point in ipairs(entry.points) do feed(format("%.5f %.5f", point.time, point.value)) end
    end
  end
  return fnv.hex(h)
end

-- The copy the ref names: its place, the track, its scratch session and copy.
local function locate(song, env, input)
  if type(input.ref) ~= "string" then bad("ref must be a scratch copy's track ref") end
  local index = refs.expect(input.ref, "track", 1, env.epoch())[1]
  local track = base.track_at(song, index)
  local session, copy = scratch.copy_named(track.name)
  if not session or track.type ~= renoise.Track.TRACK_TYPE_SEQUENCER then bad("that track isn't one of Kumi's scratch copies") end
  return index, track, session, copy
end

local function wanted_name(input)
  local name = input.name
  if type(name) ~= "string" or name == "" or #name > 1024 or name:find("[%c]") then bad("name must be one line of text") end
  if name:sub(1, #scratch.PREFIX) == scratch.PREFIX then bad("a kept track takes a name of its own, not " .. q(scratch.PREFIX .. "…")) end
  return name
end

local function preview(song, env, input)
  local index, track, session = locate(song, env, input)
  local name = wanted_name(input)
  local now = signature(song, index, track)
  return {
    target = base.target(env, index),
    prior = { name = track.name, mute = track.mute_state },
    fence = {
      id = now.id, name = track.name, mute = now.mute, devices = now.devices, patternsWithLines = now.patternsWithLines, scratchId = session.id,
    },
    display = { title = "Kept " .. q(name), before = { name = track.name }, after = { name = name }, note = NOTE },
  }
end

local function apply(song, env, input, fence, desc)
  local name = wanted_name(input)
  local ok, index, track, session, copy = pcall(locate, song, env, input)
  if not ok then
    -- (the track at the ref's place is no copy of Kumi's any more: renamed, or another track moved there)
    if type(index) == "table" and index.code == "bad-params" then fail("changed", base.moved_text(fence.name)) end
    base.gone_is_changed(index)
  end
  local now = signature(song, index, track)
  if now.id ~= fence.id or track.name ~= fence.name then fail("changed", base.moved_text(fence.name)) end
  if now.mute ~= fence.mute or now.devices ~= fence.devices or now.patternsWithLines ~= fence.patternsWithLines or session.id ~= fence.scratchId then
    fail("changed", q(fence.name) .. " changed since the preview")
  end
  song:describe_undo(desc)
  local old = track.name
  track.name = name
  track.mute_state = renoise.Track.MUTE_STATE_ACTIVE
  scratch.keep(session, copy)
  local kept = signature(song, index, track)
  return {
    applied = { name = name, mute = kept.mute, id = kept.id },
    created = { ref = refs.make(env.epoch(), "track", index), kind = "track" },
    material = {
      track = index, copyName = old, scratchId = session.id, devices = kept.devices, patternsWithLines = kept.patternsWithLines,
      print = content_print(song, index, track) or "unread",
    },
    display = { title = "Kept " .. q(name), before = { name = old }, after = { name = name }, note = NOTE },
    undo = { desc = desc, entry = true },
    target = base.target(env, index), place = { track = index }, prior = { name = old }, journal_applied = { name = name },
  }
end

local function restore(song, env, record, desc)
  local created, applied, material = record.created, record.applied, record.material
  if type(created) ~= "table" or type(created.ref) ~= "string" then bad("created: the track Kumi kept") end
  if type(applied) ~= "table" or type(material) ~= "table" then bad("applied and material: what the keep wrote") end
  local index = refs.expect(created.ref, "track", 1, env.epoch())[1]
  local track = base.track_at(song, index)
  local label = base.named(index, track)
  if base.track_id(track) ~= applied.id or track.type ~= renoise.Track.TRACK_TYPE_SEQUENCER then
    fail("changed", q(label) .. " is not the track Kumi kept any more")
  end
  local now = signature(song, index, track)
  if not record.discard then
    if now.mute ~= applied.mute or now.devices ~= material.devices or now.patternsWithLines ~= material.patternsWithLines then
      fail("changed", q(label) .. " changed in Renoise since Kumi kept it (its devices, lines or mute), so Kumi leaves it")
    end
    -- (what the producer did there since: a note, a knob, automation, the fader; a record from before the print has none)
    if material.print == "unread" then
      fail("changed", q(label) .. " is too long for Kumi to check for the producer's work, so Kumi leaves it")
    end
    if type(material.print) == "string" and content_print(song, index, track) ~= material.print then
      fail("changed", q(label) .. " changed in Renoise since Kumi kept it (its notes, devices, automation or mixer), so Kumi leaves it")
    end
  end
  song:describe_undo(desc)
  song:delete_track_at(index)
  base.note_restored(env.journal, "scratch-keep", { track = index }, { name = applied.name }, base.KINDS["scratch-keep"])
  return {
    restored = { deleted = true },
    display = { title = "Removed " .. q(label) .. " (put back)" },
    undo = { desc = desc, entry = true },
  }
end

base.register({
  kind = "scratch-keep", custom = true,
  preview = preview, apply = apply, restore = restore,
  -- (the journal reads the name again after the producer's undo or redo: that is the part Renoise's undo takes back)
  reread = function(song, place)
    if place.track >= 1 and place.track <= rows.track_count(song) then return song:track(place.track) end
    return nil
  end,
  read = function(track) return { name = track.name } end,
  same = function(a, b) return type(a) == "table" and type(b) == "table" and a.name == b.name end,
})

return M
