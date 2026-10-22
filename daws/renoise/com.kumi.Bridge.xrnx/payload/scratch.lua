-- Scratch copies (M5 contract §2.7, §7): copies of a track for a tune search, a probe or a goal to hear, made where they change
-- nothing the producer hears (sequencer tracks after the last one, outside any group, OFF so they process nothing until Kumi
-- solos one to render it), named "Kumi · try <tag> <n>", with the source's devices, mixer and lines (copy_from copies a pattern
-- track's lines and automation: measured). Each action is one callback with one `describe_undo`, sliced into numbered steps past
-- about a second; the answer says how many Renoise undo steps it left.
--
-- Every make is noted in `listening/scratch.json` before its first copy is made and forgotten once dropped, so copies a crash
-- left behind are found again: when the bridge loads, or the song they were made in opens again, copies still named as Kumi
-- named them are removed and Kumi is told (`scratch_swept`).

local import = ...
local json = import("json")
local refs = import("refs")
local rows = import("rows")
local base = import("changes_base")
local discovery = import("discovery")
local listening = import("listening")
local log = import("log")
local instruments_ = import("instruments")

local fail, bad, q = base.fail, base.bad, base.q

local M = {}

M.JOURNAL = "scratch.json"
M.MAX_COPIES = 16
M.MAX_VALUES = 256
-- The work one callback takes before the rest goes to the next numbered step, and the wait between steps (Renoise made one undo
-- step of two callbacks 10 ms apart: measured).
M.STEP_MS = 700
M.STEP_GAP_MS = 400
-- Journal entries of songs that never open again are forgotten after a week.
M.KEEP_MS = 7 * 24 * 3600 * 1000
M.PREFIX = "Kumi \194\183 try "

local S

-- Set by the bridge: its configuration (where Kumi's folder is).
M.cfg = function() return nil end

function M.init(state)
  state.scratch = state.scratch or {}
  S = state.scratch
  S.live = S.live or {}
  return S
end

-- The journal

local function journal_path(cfg)
  local folder, sep = listening.folder(cfg)
  if not folder then return nil end
  return folder .. sep .. M.JOURNAL
end

local function read_journal(cfg)
  local path = journal_path(cfg)
  local f = path and io.open(path, "rb")
  if not f then return { version = 1, entries = {} } end
  local text = f:read("*a")
  f:close()
  local value = json.decode(text)
  if type(value) ~= "table" or type(value.entries) ~= "table" then return { version = 1, entries = {} } end
  return value
end

local function write_journal(cfg, journal)
  local path = journal_path(cfg)
  if not path then return nil, "the bridge isn't set up" end
  local ok, err = listening.ensure_folder(cfg)
  if not ok then return nil, err end
  local entries = {}
  for _, entry in ipairs(journal.entries) do entries[#entries + 1] = entry end
  return discovery.write(path, { version = 1, entries = json.array(entries) })
end

local function forget(cfg, id)
  local journal = read_journal(cfg)
  local kept = {}
  for _, entry in ipairs(journal.entries) do
    if entry.scratchId ~= id then kept[#kept + 1] = entry end
  end
  journal.entries = kept
  write_journal(cfg, journal)
end

-- The copy a name belongs to, in the live scratch sessions: session, copy.
function M.copy_named(name)
  if not S or type(name) ~= "string" then return nil end
  for _, session in pairs(S.live) do
    for _, copy in ipairs(session.copies) do
      if copy.name == name and not copy.kept then return session, copy end
    end
  end
  return nil
end

-- A copy kept as the producer's own track (scratch-keep): out of the session and the journal, so no drop or sweep removes it.
function M.keep(session, copy)
  copy.kept = true
  local cfg = M.cfg()
  local journal = read_journal(cfg)
  for _, entry in ipairs(journal.entries) do
    if entry.scratchId == session.id then
      local left = {}
      for _, c in ipairs(entry.copies or {}) do
        if c.name ~= copy.name then left[#left + 1] = c end
      end
      entry.copies = json.array(left)
    end
  end
  write_journal(cfg, journal)
end

-- The copies, found

local function tag_of(id)
  local tag = id:sub(2)
  if #tag > 4 then tag = tag:sub(-4) end
  return tag
end

-- A sequencer track named `name` and in no group: where the copy is. `hint` (a place carried through the shifts since the copy
-- was made) is tried first; a copy renamed, or put into a group, is no longer Kumi's to remove.
local function find_copy(song, name, hint)
  local parents = rows.group_parents(song)
  local function fits(t)
    if t < 1 or t > song.sequencer_track_count then return false end
    local track = song:track(t)
    return track.type == renoise.Track.TRACK_TYPE_SEQUENCER and track.name == name and not parents[t]
  end
  if hint and fits(hint) then return hint end
  for t = 1, song.sequencer_track_count do
    if fits(t) then return t end
  end
  return nil
end

-- Making copies

local MASTER = function() return renoise.Track.TRACK_TYPE_MASTER end

-- The patterns to copy lines of: those the slots over `range` play (all the source has lines in, without one), sorted.
local function patterns_for(song, env, source, range)
  local wanted = {}
  if range then
    local timeline = listening.timeline(song, env.pattern_mark)
    local from, to = range.fromBeat, range.fromBeat + range.beats
    for _, slot in ipairs(timeline.slots) do
      if slot.endBeat > from and slot.beat < to then wanted[slot.pattern] = true end
    end
  else
    for p = 1, #song.patterns do wanted[p] = true end
  end
  local out = {}
  for p in pairs(wanted) do
    local pattern_track = song:pattern(p):track(source)
    if pattern_track.is_alias or not pattern_track.is_empty then out[#out + 1] = p end
  end
  table.sort(out)
  return out
end

local function parameter_named(device, name)
  for k = 1, #device.parameters do
    if device:parameter(k).name == name then return device:parameter(k) end
  end
  return nil
end

-- One copy of track `source`, made at `at` (just before the master): its mixer, columns, devices and lines.
-- A track made at `at`, named, and set up by `setup(track)`: when setting it up raises (a plug-in that isn't on this computer),
-- the track goes again at once, so nothing is left that the session, its undo and its journal don't know of.
local function made_track(song, at, name, setup)
  song:insert_track_at(at)
  local track = song:track(at)
  track.name = name
  local ok, err = pcall(setup, track)
  if not ok then
    pcall(function() song:delete_track_at(at) end)
    error(err, 0)
  end
  return track
end

local function make_copy(song, source, at, name, patterns)
  return made_track(song, at, name, function(copy)
    local from = song:track(source)
    -- (OFF processes nothing: no device and no #Send of a copy reaches what the producer hears; a solo plays it: measured)
    copy.mute_state = renoise.Track.MUTE_STATE_OFF
    copy.prefx_volume.value = from.prefx_volume.value
    copy.prefx_panning.value = from.prefx_panning.value
    copy.prefx_width.value = from.prefx_width.value
    copy.postfx_volume.value = from.postfx_volume.value
    copy.postfx_panning.value = from.postfx_panning.value
    copy.visible_note_columns = from.visible_note_columns
    copy.visible_effect_columns = from.visible_effect_columns
    pcall(function()
      copy.volume_column_visible = from.volume_column_visible
      copy.panning_column_visible = from.panning_column_visible
      copy.delay_column_visible = from.delay_column_visible
      copy.sample_effects_column_visible = from.sample_effects_column_visible
    end)
    for c = 1, from.max_note_columns do
      if from:column_is_muted(c) then copy:set_column_is_muted(c, true) end
    end
    local self_aimed = 0
    for d = 2, #from.devices do
      local device = from:device(d)
      local made = copy:insert_device_at(device.device_path, d)
      if device.active_preset_data ~= "" then pcall(function() made.active_preset_data = device.active_preset_data end) end
      made.is_active = device.is_active
      -- (a meta device made from its preset data aims nowhere (measured); one that modulated its own track does the copy's)
      local dest = device.device_path:find("/Native/*", 1, true) and parameter_named(device, "Dest. Track")
      if dest and dest.value == source - 1 then
        local made_dest = parameter_named(made, "Dest. Track")
        if made_dest then
          made_dest.value = at - 1
          for _, field in ipairs({ "Dest. Effect", "Dest. Parameter" }) do
            local theirs, ours = parameter_named(device, field), parameter_named(made, field)
            if theirs and ours and theirs.value >= 0 then ours.value = theirs.value end
          end
          self_aimed = self_aimed + 1
        end
      end
    end
    for _, p in ipairs(patterns) do
      local pattern = song:pattern(p)
      pattern:track(at):copy_from(pattern:track(source))
    end
  end)
end

-- M7 (variations): a player of one phrase instead of a copy: an empty track (no devices, OFF until Kumi solos it to render) whose
-- line `spot.line` of pattern `spot.pattern` plays the instrument's note with a 0Zxx naming the phrase. Its lines go with it when
-- the track goes.
local function make_player(song, at, name, spot)
  return made_track(song, at, name, function(track)
    track.mute_state = renoise.Track.MUTE_STATE_OFF
    if track.visible_effect_columns < 1 then track.visible_effect_columns = 1 end
    local line = song:pattern(spot.pattern):track(at):line(spot.line)
    local column = line:note_column(1)
    column.note_value = spot.note
    column.instrument_value = spot.instrument - 1
    line:effect_column(1).number_string = "0Z"
    line:effect_column(1).amount_value = spot.phrase
  end)
end

-- Where players of phrases play: the instrument, its phrases (from 1, or their places: followed to where each is now, so an edit
-- since they were looked up never plays another phrase) and the note, and the pattern and line the range starts at (a slot's
-- first line). Refused unless the range starts where a slot does.
local function players_of(song, env, players, range)
  if type(players) ~= "table" or type(players.instrument) ~= "string" or type(players.phrases) ~= "table" or #players.phrases == 0 then
    bad("phrases is {instrument, phrases, note?}: the instrument's ref and its phrases (numbers from 1, or places)")
  end
  local i = refs.expect(players.instrument, "instrument", 1, env.epoch())[1]
  if i > #song.instruments then fail("gone", "that instrument isn't in the song any more") end
  local instrument = song:instrument(i)
  local list = {}
  for k, ph in ipairs(players.phrases) do
    if type(ph) == "table" then
      local n, why = instruments_.follow(env, ph)
      if not n then fail(why == "gone" and "gone" or "stale", "a phrase to play isn't where Kumi found it any more: read again") end
      if n[1] ~= i then fail("changed", "a phrase to play is in another instrument now: read again") end
      ph = n[2]
    end
    if type(ph) ~= "number" or ph ~= math.floor(ph) or ph < 1 or ph > #instrument.phrases then
      bad(string.format("phrases are numbers from 1 to %d (the instrument's phrases), or their places", #instrument.phrases))
    end
    list[k] = ph
  end
  local note = players.note == nil and 48 or players.note
  if type(note) ~= "number" or note ~= math.floor(note) or note < 0 or note > 119 then bad("note is from 0 to 119 (48 is C-4)") end
  if not range then bad("a player of phrases needs range: where it plays") end
  local timeline = listening.timeline(song, env.pattern_mark)
  for _, slot in ipairs(timeline.slots) do
    if math.abs(slot.beat - range.fromBeat) < 1e-6 then
      return { instrument = i, list = list, note = note, pattern = slot.pattern, line = 1, name = instrument.name }
    end
  end
  bad("range must start where a slot of the sequence starts")
end

-- What one copy costs, about, in ms (measured: 8 copies of 5 devices and 4 dense 64-line patterns took 200 ms; a track insert
-- grows faster than the tracks, 11 ms at 90 and 385 ms at 566; a pattern's copy with its lines, about 4 ms a 64 lines).
local function cost_of(song, source, patterns)
  local tracks, lines = rows.track_count(song), 0
  for _, p in ipairs(patterns) do lines = lines + song:pattern(p).number_of_lines end
  return 2 + 0.05 * tracks + 11 * (tracks / 90) ^ 2 + 3 * (#song:track(source).devices - 1) + 4 * lines / 64
end
-- One copy is one callback: one that would hold Renoise longer than this is refused (Renoise's watchdog speaks after ~10 s).
M.MOST_COPY_MS = 3000

-- The song, if Renoise isn't busy (set and drop name no refs of an epoch of their own: a copy's ref is checked by its name).
local function ready(env)
  local song = rows.the_song()
  if song.rendering then fail("busy", "Renoise is rendering the song; nothing changed") end
  local reason = env.busy()
  if reason then fail("busy", reason .. "; nothing changed") end
  return song
end

-- Where a copy is now: its place, carried through the tracks made, deleted and moved since it was made.
local function copy_place(env, copy)
  if copy.layout == nil then return copy.index end
  return (env.shifted(copy.index, copy.layout))
end

local function check_range(range)
  if range == nil or range == json.null then return nil end
  if type(range) ~= "table" then bad("range is {fromBeat, beats}") end
  local from, beats = range.fromBeat, range.beats
  if type(from) ~= "number" or from < 0 or type(beats) ~= "number" or beats <= 0 then
    bad("range is {fromBeat, beats}: fromBeat from 0, beats more than 0")
  end
  return { fromBeat = from, beats = beats }
end

local function make(params, env)
  local song = base.context(params, env)
  local id = listening.need_id(params.scratchId, "scratchId", "s")
  if S.live[id] then bad("there is a scratch " .. id .. " already") end
  local range = check_range(params.range)
  -- (M7: players of phrases instead of copies of a track)
  local players = params.phrases ~= nil and players_of(song, env, params.phrases, range) or nil
  local source, track
  if not players then
    if type(params.ref) ~= "string" then bad("ref must be a track ref") end
    source = refs.expect(params.ref, "track", 1, env.epoch())[1]
    track = base.track_at(song, source)
    if track.type ~= renoise.Track.TRACK_TYPE_SEQUENCER then
      bad("Kumi copies a sequencer track (not a group, the master or a send)")
    end
  end
  local count = params.count
  if type(count) ~= "number" or count ~= math.floor(count) or count < 1 or count > M.MAX_COPIES then
    bad("count is from 1 to " .. M.MAX_COPIES)
  end
  if players and count ~= #players.list then bad("count is the number of phrases to play") end
  local label = params.label
  if type(label) ~= "string" or label == "" or #label > 320 or label:find("[%c]") then bad("label must be one line of text") end
  local patterns = players and {} or patterns_for(song, env, source, range)
  local tag = tag_of(id)
  local source_name = players and players.name or base.named(source, track)
  local per_copy = players and (2 + 0.05 * rows.track_count(song) + 11 * (rows.track_count(song) / 90) ^ 2) or cost_of(song, source, patterns)
  if per_copy > M.MOST_COPY_MS then
    bad(string.format("one copy of %s would hold Renoise about %.1f s (its lines in %d patterns, in a song of %d tracks): give a range, a few bars of it",
      q(source_name), per_copy / 1000, #patterns, rows.track_count(song)))
  end
  local names = {}
  for n = 1, count do names[n] = M.PREFIX .. tag .. " " .. n end
  for _, name in ipairs(names) do
    if find_copy(song, name) then bad("a track named " .. q(name) .. " is in the song already") end
  end
  -- The journal first: copies a crash leaves are found again by it.
  local planned = {}
  for n, name in ipairs(names) do planned[n] = { name = name, index = song.sequencer_track_count + n } end
  local journal = read_journal(M.cfg())
  journal.entries[#journal.entries + 1] = {
    scratchId = id, project = env.song_id(), label = label, source = { name = players and players.name or track.name, index = source or 0 },
    copies = json.array(planned), madeMs = env.unix_ms(),
  }
  local written, why = write_journal(M.cfg(), journal)
  if not written then fail("failed", "Kumi couldn't note its scratch copies in its folder (" .. tostring(why) .. "), so it made none") end
  local session = { id = id, label = label, source = source, source_name = source_name, copies = {}, devices = {} }
  if track then
    for d = 1, #track.devices do session.devices[d] = track:device(d).device_path end
  end
  S.live[id] = session
  local desc = players and string.format("Kumi: players of %s's phrases for %s (%s)", q(source_name), label, id)
    or string.format("Kumi: scratch copies of %s for %s (%s)", q(source_name), label, id)
  local total = math.max(1, math.ceil(per_copy * count / M.STEP_MS))
  local source_layout, source_id = env.layout(), track and base.track_id(track) or nil
  -- Makes copies until the step's time is used (at least one; `budget` false: all of them); the source is found where the tracks
  -- made and deleted since moved it, and must still be the track it was.
  local function step(live, k, budget)
    local from
    if not players then
      from = env.shifted(source, source_layout)
      if not from or from > rows.track_count(live) or base.track_id(live:track(from)) ~= source_id then
        fail("changed", q(source_name) .. " isn't where it was any more: read again")
      end
    end
    live:describe_undo(total > 1 and string.format("%s (%d/%d)", desc, k, total) or desc)
    local started = env.now_ms()
    repeat
      local n = #session.copies + 1
      local at = live.sequencer_track_count + 1
      if players then
        make_player(live, at, names[n], { pattern = players.pattern, line = players.line, note = players.note, instrument = players.instrument, phrase = players.list[n] })
      else
        make_copy(live, from, at, names[n], patterns)
      end
      session.copies[n] = { name = names[n], index = at, layout = env.layout() }
    until #session.copies >= count or (budget ~= false and env.now_ms() - started >= M.STEP_MS)
  end
  local function answer(steps)
    local copies, list = {}, {}
    for n, copy in ipairs(session.copies) do
      local at = copy_place(env, copy) or copy.index
      copies[n] = { ref = refs.make(env.epoch(), "track", at), name = copy.name }
      list[n] = { name = copy.name, index = at }
      -- (the place Kumi was given: a ref of it, with no layout, is read in this answer's layout)
      copy.answered, copy.index, copy.layout = at, at, env.layout()
    end
    -- (the journal now holds where each copy is)
    local updated = read_journal(M.cfg())
    for _, entry in ipairs(updated.entries) do
      if entry.scratchId == id then entry.copies = json.array(list) end
    end
    write_journal(M.cfg(), updated)
    log.write("info", string.format("scratch %s: %d copies of %s in %d steps", id, #session.copies, source_name, steps))
    return { scratchId = id, copies = json.array(copies), epoch = env.epoch(), layout = env.layout(), undo = { steps = steps } }
  end
  if total == 1 and per_copy * count < M.STEP_MS then
    local ok, err = pcall(step, song, 1, false)
    if not ok or #session.copies < count then
      M.unmake(song, env, session)
      S.live[id] = nil
      forget(M.cfg(), id)
      fail("failed", "Renoise wouldn't make the scratch copies: " .. tostring(type(err) == "table" and err.message or err):gsub("^[^%s:]*%.lua:%d+: ", ""))
    end
    return answer(1)
  end
  return {
    kumi_job = function(job)
      local k = 0
      local ok, err = pcall(function()
        while #session.copies < count do
          k = k + 1
          if k > 1 then
            job.step(M.STEP_GAP_MS)
            base.wait_while_busy(job, env, "scratch copies")
          end
          if k > total then total = k end
          step(rows.the_song(), k)
          job.wrote = "Making scratch copies"
          job.rebase()
        end
      end)
      if not ok then
        -- (nothing half-made stays: the copies made so far go again, where they are)
        local live = renoise.song()
        if live then M.unmake(live, env, session) end
        S.live[id] = nil
        forget(M.cfg(), id)
        job.rebase()
        error(type(err) == "table" and err or { code = "failed", message = "Renoise wouldn't make the scratch copies: " .. tostring(err) }, 0)
      end
      return answer(k)
    end,
  }
end

-- Removes a session's copies that are still Kumi's (named as made, in no group): answers how many, and the names of those left.
function M.unmake(song, env, session)
  local gone, left = 0, {}
  local places = {}
  for _, copy in ipairs(session.copies) do
    if not copy.kept then
      local hint = copy_place(env, copy)
      local t = find_copy(song, copy.name, hint)
      if t then
        places[#places + 1] = t
      elseif hint and hint >= 1 and hint <= rows.track_count(song) and song:track(hint).type ~= MASTER() and song:track(hint).name ~= copy.name then
        -- (renamed or grouped by the producer: theirs now)
        local name = song:track(hint).name
        if name ~= "" then left[#left + 1] = name end
      end
    end
  end
  table.sort(places, function(a, b) return a > b end)
  for _, t in ipairs(places) do
    song:delete_track_at(t)
    gone = gone + 1
  end
  return gone, left
end

local function session_of(params)
  local id = type(params) == "table" and params.scratchId or nil
  if type(id) ~= "string" then bad("scratchId must be a scratch's id") end
  local session = S.live[id]
  if not session then bad("there is no scratch " .. id .. " (dropped, or made before Renoise restarted)") end
  return session
end

local function set(params, env)
  local song = ready(env)
  local carried = params.layout ~= nil
  local session = session_of(params)
  local values = params.values
  if type(values) ~= "table" or #values < 1 or #values > M.MAX_VALUES then bad("values: 1 to " .. M.MAX_VALUES .. " of them") end
  -- every value checked before any is written
  local writes = {}
  for _, value in ipairs(values) do
    if type(value) ~= "table" or type(value.ref) ~= "string" then bad("each value names a copy's ref") end
    local parsed = refs.parse(value.ref)
    if not parsed or parsed.kind ~= "track" or #parsed.n ~= 1 then bad("each value's ref is a copy's track ref") end
    -- (a ref carried to now (the request gave its layout) names the copy at that place; one with no layout is the place make
    -- answered, followed to where that copy is now. Either way the copy there must still have its name.)
    local copy, t
    for _, candidate in ipairs(session.copies) do
      if not candidate.kept then
        local now = copy_place(env, candidate)
        local named = carried and now == parsed.n[1] or (not carried and candidate.answered == parsed.n[1])
        if named and now and find_copy(song, candidate.name, now) == now then copy, t = candidate, now end
      end
    end
    if not copy then fail("gone", "that track isn't one of this scratch's copies any more") end
    local d, k = value.device, value.param
    if type(d) ~= "number" or d ~= math.floor(d) or d < 1 then bad("device is a device's place on the source track, from 1 (its mixer device)") end
    if type(k) ~= "number" or k ~= math.floor(k) or k < 1 then bad("param is a parameter's number, from 1") end
    local track = song:track(t)
    if d > #track.devices or track:device(d).device_path ~= session.devices[d] then
      fail("changed", "the copy's device " .. d .. " isn't the source's any more")
    end
    local device = track:device(d)
    if k > #device.parameters then bad("the device has " .. #device.parameters .. " parameters") end
    local parameter = device:parameter(k)
    local raw = value.value
    if value.normalized ~= nil then
      if type(value.normalized) ~= "number" or value.normalized < 0 or value.normalized > 1 then bad("normalized is from 0 to 1") end
      raw = parameter.value_min + value.normalized * (parameter.value_max - parameter.value_min)
    end
    if type(raw) ~= "number" or raw ~= raw then bad("each value gives value (the parameter's own range) or normalized") end
    if raw < parameter.value_min - 1e-9 or raw > parameter.value_max + 1e-9 then
      bad(string.format("%s's value is from %s to %s", parameter.name, tostring(parameter.value_min), tostring(parameter.value_max)))
    end
    writes[#writes + 1] = { t = t, d = d, k = k, value = math.max(parameter.value_min, math.min(parameter.value_max, raw)) }
  end
  song:describe_undo(string.format("Kumi: scratch values (%s)", session.id))
  for _, w in ipairs(writes) do song:track(w.t):device(w.d):parameter(w.k).value = w.value end
  return { set = #writes, undo = { steps = 1 }, epoch = env.epoch(), layout = env.layout() }
end

-- A drop of a scratch this bridge doesn't hold (a retry after a lost answer, or one made before Renoise restarted): copies its journal
-- still notes in this song, named as Kumi named them, go; else nothing is dropped.
local function drop_unknown(song, env, id)
  local cfg = M.cfg()
  local journal = read_journal(cfg)
  local kept, places = {}, {}
  for _, entry in ipairs(journal.entries) do
    if entry.scratchId == id and entry.project == env.song_id() then
      for _, copy in ipairs(entry.copies or {}) do
        local t = type(copy.name) == "string" and copy.name:sub(1, #M.PREFIX) == M.PREFIX and find_copy(song, copy.name, copy.index)
        if t then places[#places + 1] = t end
      end
    else
      kept[#kept + 1] = entry
    end
  end
  if #places > 0 then
    table.sort(places, function(a, b) return a > b end)
    song:describe_undo(string.format("Kumi: dropped scratch copies (%s)", id))
    for _, t in ipairs(places) do song:delete_track_at(t) end
  end
  if #kept ~= #journal.entries then
    journal.entries = kept
    write_journal(cfg, journal)
  end
  return { dropped = #places, left = 0, undo = { steps = #places > 0 and 1 or 0 }, epoch = env.epoch(), layout = env.layout() }
end

local function drop(params, env)
  local song = ready(env)
  local id = listening.need_id(params.scratchId, "scratchId", "s")
  if not S.live[id] then return drop_unknown(song, env, id) end
  local session = session_of(params)
  local any = false
  for _, copy in ipairs(session.copies) do
    if not copy.kept then any = true end
  end
  local gone, left = 0, {}
  if any then
    song:describe_undo(string.format("Kumi: dropped the scratch copies of %s (%s)", q(session.source_name), session.id))
    gone, left = M.unmake(song, env, session)
  end
  S.live[session.id] = nil
  forget(M.cfg(), session.id)
  local answer = { dropped = gone, left = #left, undo = { steps = gone > 0 and 1 or 0 }, epoch = env.epoch(), layout = env.layout() }
  if #left > 0 then
    local named = {}
    for i, name in ipairs(left) do named[i] = q(name) end
    answer.notes = json.array({ string.format("%s renamed or moved in Renoise, so Kumi left %s: %s.", #left == 1 and "A copy was" or (#left .. " copies were"),
      #left == 1 and "it" or "them", table.concat(named, ", ")) })
  end
  return answer
end

function M.handle(params, env)
  if type(params) ~= "table" then bad("scratch needs {action}") end
  if params.action == "make" then return make(params, env) end
  if params.action == "set" then return set(params, env) end
  if params.action == "drop" then return drop(params, env) end
  bad("action is make, set or drop")
end

-- The sweep: copies a crash left in this song (by the journal, named as Kumi named them) are removed in one step; Kumi is told.
-- Answers what was swept ({count, names}) or nil.
function M.sweep(song, env)
  local cfg = M.cfg()
  if not cfg or not song then return nil end
  local journal = read_journal(cfg)
  if #journal.entries == 0 then return nil end
  local project = env.song_id()
  local now = env.unix_ms()
  local kept, names, places = {}, {}, {}
  local touched = false
  for _, entry in ipairs(journal.entries) do
    local live = S.live[entry.scratchId]
    if entry.project == project and not live then
      touched = true
      for _, copy in ipairs(entry.copies or {}) do
        local t = type(copy.name) == "string" and copy.name:sub(1, #M.PREFIX) == M.PREFIX and find_copy(song, copy.name, copy.index)
        if t then
          places[#places + 1] = t
          names[#names + 1] = copy.name
        end
      end
    elseif not live and type(entry.madeMs) == "number" and now - entry.madeMs > M.KEEP_MS then
      touched = true
    else
      kept[#kept + 1] = entry
    end
  end
  if #places > 0 then
    table.sort(places, function(a, b) return a > b end)
    song:describe_undo("Kumi: removed scratch copies left from before")
    for _, t in ipairs(places) do song:delete_track_at(t) end
  end
  if touched then
    journal.entries = kept
    write_journal(cfg, journal)
  end
  if #places == 0 then return nil end
  log.write("info", "swept " .. #places .. " scratch copies left from before")
  return { count = #places, names = json.array(names) }
end

return M
