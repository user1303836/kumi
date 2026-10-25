-- read: the song as rows (contract §3), paged. A page is cut to the byte bound by dropping rows off its end.

local import = ...
local json = import("json")
local refs = import("refs")
local rows = import("rows")
local wire = import("wire")
-- (M5 m5-renoise: spans, grid)
local reads_listening = import("reads_listening")

local fail, the_song, track_count, integer, array = rows.fail, rows.the_song, rows.track_count, rows.integer, rows.array
local group_parents, song_row, track_row, sequence_row, pattern_row = rows.group_parents, rows.song_row, rows.track_row,
  rows.sequence_row, rows.pattern_row
local instrument_row, sample_row, phrase_row, device_row, param_row, line_row = rows.instrument_row, rows.sample_row,
  rows.phrase_row, rows.device_row, rows.param_row, rows.line_row
local modset_row, chain_row, parameter_row = rows.modset_row, rows.chain_row, rows.parameter_row

local M = {}

M.DEFAULT_LIMIT = 50
M.MAX_LIMIT = 200
-- The most an encoded answer may be (the frame is a little more: its envelope).
M.PAGE_BUDGET = wire.MAX_PAGE_BYTES - 4096

-- Paging

-- A cursor is `<epoch>.<layout>.<position>`: a position in a list means nothing once the epoch moved, or a track was
-- inserted or removed (the rows after it moved with it).
local function cursor_start(params, epoch, layout)
  local cursor = params.cursor
  if cursor == nil then return 1 end
  local cursor_epoch, cursor_layout, position = tostring(cursor):match("^([^.]+)%.(%d+)%.(%d+)$")
  if not cursor_epoch or tonumber(position) < 1 then fail("bad-params", "not a cursor: " .. tostring(cursor)) end
  if cursor_epoch ~= epoch then fail("stale", "that cursor is from an earlier epoch: read again from the start") end
  if tonumber(cursor_layout) ~= layout then
    fail("stale", "tracks were inserted or removed since that cursor was made: read again from the start")
  end
  return tonumber(position)
end

local function cursor_for(epoch, layout, position)
  return epoch .. "." .. layout .. "." .. position
end

local function limit_of(params)
  return integer(params.limit, "limit", 1, M.MAX_LIMIT) or M.DEFAULT_LIMIT
end

-- The rows from `row_at(i)`, i from the cursor to `total`, up to the limit (nil rows are skipped, not counted), as
-- {items, nextCursor?, epoch, ...extra}, cut to the byte budget by dropping rows off the end. Each row is encoded once, as it
-- is taken, and goes into the answer as that text (json.raw): the budget's measure and the frame then cost no encoding of the
-- rows again, and the frame holds the bytes it always did.
local function page(job, epoch, params, total, row_at, sparse, extra)
  local from = cursor_start(params, epoch, job.layout)
  local limit = limit_of(params)
  local items, after = {}, {}
  local i = from
  while i <= total and #items < limit do
    local waited = job.slice()
    local row = row_at(i, waited)
    if row ~= nil then
      local text = json.encode(row, { max_depth = json.ROW_DEPTH })
      items[#items + 1] = text and json.raw(text) or row
      after[#items] = i + 1
    end
    i = i + 1
  end
  -- Where rows are few among empty places (lines), the next page starts at the next row, and there is none if no row is left.
  if sparse then
    while i <= total do
      if row_at(i, job.slice()) ~= nil then break end
      i = i + 1
    end
  end
  local resume = i <= total and i or nil
  local keep = #items
  while true do
    local next_at = keep < #items and after[keep] or resume
    local shown = {}
    for k = 1, keep do shown[k] = items[k] end
    local result = { items = array(shown), epoch = epoch }
    for key, value in pairs(extra or {}) do result[key] = value end
    if next_at then result.nextCursor = cursor_for(epoch, job.layout, next_at) end
    if keep <= 1 or #json.encode(result) <= M.PAGE_BUDGET then return result end
    keep = math.max(1, math.floor(keep * 0.6))
  end
end

-- Shrinks the named lists of an answer that has no pages (a snapshot, an instrument, a chain), each from its end by about a
-- third at a time, until it fits a page; `truncated` says it was cut.
function M.bound(result, keys)
  while #json.encode(result) > M.PAGE_BUDGET do
    local cut = false
    for _, key in ipairs(keys) do
      local list = result[key]
      if list and #list > 1 then
        local shorter = {}
        for i = 1, math.max(1, math.floor(#list * 0.7)) do shorter[i] = list[i] end
        result[key] = array(shorter)
        cut = true
      end
    end
    if not cut then break end
    result.truncated = true
  end
  return result
end

---@return integer
local function track_param(value, epoch, song)
  ---@type integer?
  local number
  if type(value) == "number" then
    number = integer(value, "track", 1, 10000)
  elseif type(value) == "string" then
    number = refs.expect(value, "track", 1, epoch)[1]
  end
  if not number then fail("bad-params", "line needs a track: a number or a track ref") end
  ---@cast number integer
  if number > track_count(song) then fail("bad-params", "no track " .. number) end
  return number
end

local function need_parent(params, kind, count, epoch)
  if params.parent == nil then fail("bad-params", "read of " .. params.kind .. " needs a parent (" .. kind .. " ref)") end
  return refs.expect(params.parent, kind, count, epoch)
end

-- How many numbers each kind of ref holds.
local REF_COUNTS = { track = 1, seq = 1, pattern = 1, instrument = 1, sample = 2, phrase = 2, device = 2, param = 3,
  -- n-instruments
  modset = 2, moddevice = 3, chain = 2, chaindevice = 3, chainparam = 4, pluginparam = 2 }

-- n-instruments: the objects inside an instrument a ref names (nil when it isn't there).
local function instrument_object(song, kind, n)
  if n[1] > #song.instruments then return nil end
  local instrument = song:instrument(n[1])
  if kind == "modset" or kind == "moddevice" then
    if n[2] > #instrument.sample_modulation_sets then return nil end
    local set = instrument:sample_modulation_set(n[2])
    if kind == "modset" then return set end
    return n[3] <= #set.devices and set:device(n[3]) or nil
  elseif kind == "chain" or kind == "chaindevice" or kind == "chainparam" then
    if n[2] > #instrument.sample_device_chains then return nil end
    local chain = instrument:sample_device_chain(n[2])
    if kind == "chain" then return chain end
    if n[3] > #chain.devices then return nil end
    local device = chain:device(n[3])
    if kind == "chaindevice" then return device end
    return n[4] <= #device.parameters and device:parameter(n[4]) or nil
  elseif kind == "pluginparam" then
    local plugin = instrument.plugin_properties
    if not plugin.plugin_loaded or n[2] > #plugin.plugin_device.parameters then return nil end
    return plugin.plugin_device:parameter(n[2])
  end
  return nil
end

-- read {ref}: the one object a ref names.
local function one(params, epoch)
  local parsed = refs.parse(params.ref)
  local count = parsed and REF_COUNTS[parsed.kind]
  if not count then fail("bad-params", "not a ref this tool reads: " .. tostring(params.ref)) end
  local kind = parsed.kind
  local n = refs.expect(params.ref, kind, count, epoch)
  local song = the_song()
  local function need(exists)
    if not exists then fail("bad-params", "nothing is at " .. params.ref) end
  end
  local row
  if kind == "track" then
    need(n[1] <= track_count(song))
    row = track_row(song, epoch, n[1], group_parents(song))
  elseif kind == "seq" then
    need(n[1] <= #song.sequencer.pattern_sequence)
    row = reads_listening.slot_beats()(sequence_row(song, epoch, n[1], track_count(song)), n[1])
  elseif kind == "pattern" then
    need(n[1] <= #song.patterns)
    row = pattern_row(song, epoch, n[1], track_count(song))
  elseif kind == "instrument" then
    need(n[1] <= #song.instruments)
    row = instrument_row(song, epoch, n[1])
  elseif kind == "sample" then
    need(n[1] <= #song.instruments and n[2] <= #song:instrument(n[1]).samples)
    row = sample_row(song, epoch, n[1], n[2])
  elseif kind == "phrase" then
    need(n[1] <= #song.instruments and n[2] <= #song:instrument(n[1]).phrases)
    row = phrase_row(song, epoch, n[1], n[2])
  elseif kind == "device" then
    need(n[1] <= track_count(song) and n[2] <= #song:track(n[1]).devices)
    row = device_row(song, epoch, n[1], n[2])
  elseif kind == "modset" then
    need(instrument_object(song, kind, n) ~= nil)
    row = modset_row(song, epoch, n[1], n[2])
  elseif kind == "moddevice" then
    need(instrument_object(song, kind, n) ~= nil)
    -- (a modulation device is shown in its set's row)
    local set = modset_row(song, epoch, n[1], n[2])
    row = set.devices[n[3]]
  elseif kind == "chain" then
    need(instrument_object(song, kind, n) ~= nil)
    row = chain_row(song, epoch, n[1], n[2])
  elseif kind == "chaindevice" then
    need(instrument_object(song, kind, n) ~= nil)
    row = chain_row(song, epoch, n[1], n[2]).devices[n[3]]
  elseif kind == "chainparam" or kind == "pluginparam" then
    local parameter = instrument_object(song, kind, n)
    need(parameter ~= nil)
    row = parameter_row(params.ref, n[#n], parameter)
  else
    need(n[1] <= track_count(song) and n[2] <= #song:track(n[1]).devices
      and n[3] <= #song:track(n[1]):device(n[2]).parameters)
    row = param_row(song, epoch, n[1], n[2], n[3])
    local with_grid = reads_listening.grid_of(params)
    if with_grid then row = with_grid(row, song:track(n[1]):device(n[2]):parameter(n[3])) end
  end
  return { items = array({ row }), epoch = epoch }
end

-- The tidy's look at the song's instruments (#401, sample mapping), in pages a callback can afford, each from `cursor` on:
-- `tidy` reads instruments (from the one numbered `cursor`, 1 first): each sample's print, where the sound is (its shape, 256
-- frames from the first that isn't silence, and 16 points over its first second), so padded hits don't print alike, and a
-- print of the instrument's settings (its devices, macros and samples' settings, not their names); `tidy-notes` counts the
-- notes of the song's patterns (from the one numbered `cursor`) that play each instrument, hidden columns too, and with byTrack
-- the same by track (its number, from 1): which tracks play an instrument (#401, layering breaks).
M.TIDY_READS = 300000
M.TIDY_CALLS = 120000
M.TIDY_SEEK = 32768
local function sum_of(sum, text)
  for i = 1, #text do sum = (sum * 33 + text:byte(i)) % 4294967296 end
  return sum
end

local function sample_print(sample, spent)
  local buffer = sample.sample_buffer
  if not buffer.has_sample_data then return nil, spent end
  local frames, rate = buffer.number_of_frames, buffer.sample_rate
  local function value(f)
    spent = spent + 1
    local v = buffer:sample_data(1, f)
    return tonumber(type(v) == "table" and v[1] or v) or 0
  end
  -- (where the sound starts: a coarse look, then frame by frame back to it)
  local start
  for f = 1, math.min(frames, M.TIDY_SEEK), 64 do
    if math.abs(value(f)) > 1e-4 then start = f; break end
  end
  if start then
    for f = math.max(1, start - 63), start do
      if math.abs(value(f)) > 1e-4 then start = f; break end
    end
  else
    start = 1
  end
  local sum = 5381
  for f = start, math.min(frames, start + 255) do sum = sum_of(sum, string.format("%.4f|", value(f))) end
  local reach = math.max(0, math.min(frames - start, math.floor(rate)))
  for k = 1, 16 do sum = sum_of(sum, string.format("%.3f|", value(start + math.floor(reach * k / 17)))) end
  return string.format("%d.%d.%d.%d.%08x", frames, buffer.number_of_channels, rate, start, sum), spent
end

local function settings_print(instruments_, instrument)
  local work = instruments_.instrument_work(instrument)
  work.name = nil
  for _, sample in ipairs(work.sampleWork or {}) do sample.name = nil end
  return string.format("%08x", sum_of(5381, json.encode(work, { sort_keys = true })))
end

local function tidy_page(song, epoch, from)
  local instruments_ = import("instruments")
  local out, spent, i = {}, 0, from
  while i <= #song.instruments and spent <= M.TIDY_CALLS do
    local instrument = song:instrument(i)
    local samples = {}
    for s = 1, #instrument.samples do
      local sample = instrument:sample(s)
      if not sample.is_slice_alias then
        local print
        print, spent = sample_print(sample, spent)
        samples[#samples + 1] = { name = sample.name, print = print or json.null }
      end
    end
    out[#out + 1] = {
      ref = refs.make(epoch, "instrument", i), index = i - 1, name = instrument.name, samples = array(samples),
      settings = settings_print(instruments_, instrument), plugin = instrument.plugin_properties.plugin_loaded,
      phrases = #instrument.phrases,
    }
    i = i + 1
  end
  return { items = array(out), epoch = epoch, nextCursor = i <= #song.instruments and tostring(i) or nil }
end

local function notes_page(song, from, by_track)
  local counts, tracks_counts, reads, p = {}, {}, 0, from
  local tracks = track_count(song)
  while p <= #song.patterns and reads <= M.TIDY_READS do
    local pattern = song:pattern(p)
    for t = 1, tracks do
      local pattern_track = pattern:track(t)
      reads = reads + 1
      -- (an alias's lines are its source pattern's, counted there)
      if not pattern_track.is_alias and not pattern_track.is_empty and song:track(t).max_note_columns > 0 then
        for l = 1, pattern.number_of_lines do
          local line = pattern_track:line(l)
          reads = reads + 1
          if not line.is_empty then
            -- (hidden columns too: a note there still names the instrument, and a tidy never calls it unplayed)
            for c = 1, 12 do
              local column = line:note_column(c)
              reads = reads + 1
              local v = column.instrument_value
              if v < 255 and column.note_value < 120 then
                counts[tostring(v)] = (counts[tostring(v)] or 0) + 1
                if by_track then
                  local mine = tracks_counts[tostring(t)] or {}
                  tracks_counts[tostring(t)] = mine
                  mine[tostring(v)] = (mine[tostring(v)] or 0) + 1
                end
              end
            end
          end
        end
      end
    end
    p = p + 1
  end
  local answer = {
    counts = json.object(counts), to = p - 1, patterns = #song.patterns,
    nextCursor = p <= #song.patterns and tostring(p) or nil,
  }
  if by_track then
    for t, mine in pairs(tracks_counts) do tracks_counts[t] = json.object(mine) end
    answer.tracks = json.object(tracks_counts)
  end
  return answer
end

local function tidy_from(params)
  local from = tonumber(params.cursor or 1)
  if not from or from < 1 or from ~= math.floor(from) then fail("bad-params", "cursor is where the last page ended") end
  return from
end

-- read {kind, parent?, cursor?, limit?, track?} or {ref}
function M.read(job, params, epoch)
  if type(params) ~= "table" then fail("bad-params", "read needs a kind") end
  if params.ref ~= nil then return one(params, epoch) end
  local kind = params.kind
  if params.grid ~= nil then reads_listening.check_kind(kind) end
  local song = the_song()
  if kind == "song" then
    return { items = array({ song_row(song) }), epoch = epoch }
  elseif kind == "track" then
    local count = track_count(song)
    local parents = group_parents(song)
    return page(job, epoch, params, count, function(i) return track_row(the_song(), epoch, i, parents) end)
  elseif kind == "sequence" then
    local tracks = track_count(song)
    local beats = reads_listening.slot_beats()
    return page(job, epoch, params, #song.sequencer.pattern_sequence, function(s) return beats(sequence_row(the_song(), epoch, s, tracks), s) end)
  elseif kind == "pattern" then
    local tracks = track_count(song)
    -- (#401: with prints, each track's lines summed up as the catch-up's digest does, and how many automation envelopes it
    -- has: patterns compared without their lines read; a page of at most 16 of them)
    if params.prints == true then
      if type(params.limit) ~= "number" or params.limit > 16 then params.limit = 16 end
      local digest = import("digest")
      return page(job, epoch, params, #song.patterns, function(p)
        local s = the_song()
        local row = pattern_row(s, epoch, p, tracks)
        local pattern, prints, automation = s:pattern(p), {}, 0
        for t = 1, tracks do
          local pattern_track = pattern:track(t)
          if pattern_track.alias_pattern_index > 0 then
            prints[tostring(t)] = string.format("alias of Pattern %02d", pattern_track.alias_pattern_index - 1)
          elseif not pattern_track.is_empty then
            prints[tostring(t)] = digest.lines_print(pattern_track, s:track(t), pattern.number_of_lines, nil)
          end
          automation = automation + #pattern_track.automation
        end
        row.prints = next(prints) and prints or json.object()
        row.automation = automation
        return row
      end)
    end
    return page(job, epoch, params, #song.patterns, function(p) return pattern_row(the_song(), epoch, p, tracks) end)
  elseif kind == "where" then
    -- M7 (variations): where what Kumi keeps by its place is now, and the place of what a ref names now, for Kumi to find it
    -- again after the producer's edits: a pattern of the pool ({pattern = {n, mark}} or a pattern or slot ref: its ref, number,
    -- the slots that play it, a place), an instrument or a phrase ({instrument|phrase = place or ref}: its ref, index, name, a
    -- place), and a track by its ref ({track = ref}: carried through the shifts since the request's layout, as every request's
    -- refs are).
    local env, items = M.places, params.items
    if type(items) ~= "table" or #items > M.MAX_LIMIT then fail("bad-params", "items: up to " .. M.MAX_LIMIT .. " places") end
    if not env then fail("busy", "the tool isn't watching the song yet") end
    local instruments_ = import("instruments")
    local function missing(why) return { [why == "gone" and "gone" or "unknown"] = true } end
    local out = {}
    for k, item in ipairs(items) do
      local row
      if type(item) ~= "table" then fail("bad-params", "each item is {pattern}, {instrument}, {phrase} or {track}") end
      if item.pattern ~= nil then
        local p, why
        if type(item.pattern) == "string" then
          local ref = refs.parse(item.pattern)
          if ref and ref.kind == "seq" then
            local s = refs.expect(item.pattern, "seq", 1, epoch)[1]
            p = song.sequencer.pattern_sequence[s]
            why = "gone"
          else
            p = refs.expect(item.pattern, "pattern", 1, epoch)[1]
            why = "gone"
          end
        elseif type(item.pattern) == "table" then
          p, why = env.pattern_at(item.pattern.n, item.pattern.mark)
        end
        if p and p <= #song.patterns then
          local slots = {}
          for s, value in ipairs(song.sequencer.pattern_sequence) do
            if value == p then slots[#slots + 1] = s end
          end
          row = { ref = refs.make(epoch, "pattern", p), number = p - 1, name = song:pattern(p).name, lines = song:pattern(p).number_of_lines,
            slots = array(slots), seqRef = slots[1] and refs.make(epoch, "seq", slots[1]) or nil, place = { n = p, mark = env.pattern_mark() } }
          -- (#401: the tracks holding lines or automation of their own, not an alias's: a variation's pattern has them on its
          -- varied track only, so another one is the producer's edit)
          local own = {}
          for t = 1, track_count(song) do
            local pattern_track = song:pattern(p):track(t)
            if pattern_track.alias_pattern_index == 0 and (not pattern_track.is_empty or #pattern_track.automation > 0) then own[#own + 1] = t end
          end
          row.ownTracks = array(own)
        else
          row = missing(why)
        end
      elseif item.phrase ~= nil or item.instrument ~= nil then
        local of = item.phrase ~= nil and "phrase" or "instrument"
        local given = item[of]
        local n, why
        if type(given) == "string" then
          n = refs.expect(given, of, #instruments_.SHAPES[of].lists, epoch)
        else
          n, why = instruments_.follow(env, given)
        end
        local ok_, objects = pcall(instruments_.resolve, song, of, n or { 0, 0 })
        if n and ok_ then
          row = { ref = refs.make(epoch, of, unpack(n)), index = n[#n], place = instruments_.place_of(env, of, n) }
          if of == "phrase" then
            row.name, row.lines, row.lpb = objects.phrase.name, objects.phrase.number_of_lines, objects.phrase.lpb
          else
            row.name, row.phrases = objects.instrument.name, #objects.instrument.phrases
          end
        else
          row = missing(n and "gone" or why)
        end
      elseif type(item.track) == "string" then
        local t = refs.expect(item.track, "track", 1, epoch)[1]
        if t > track_count(song) then row = { gone = true } else row = { ref = refs.make(epoch, "track", t), index = t, name = song:track(t).name } end
      else
        fail("bad-params", "each item is {pattern}, {instrument}, {phrase} or {track}")
      end
      out[k] = row
    end
    return { items = array(out), epoch = epoch }
  elseif kind == "line" and refs.parse(params.parent) and refs.parse(params.parent).kind == "phrase" then
    -- M7 (variations): a phrase's lines, as a pattern track's are read (its instrument values are the instrument's samples)
    local at = refs.expect(params.parent, "phrase", 2, epoch)
    if at[1] > #song.instruments or at[2] > #song:instrument(at[1]).phrases then fail("bad-params", "no such phrase") end
    local phrase = song:instrument(at[1]):phrase(at[2])
    local note_columns, effect_columns = phrase.visible_note_columns, phrase.visible_effect_columns
    -- (the phrase looked up once a slice: a line is the one call to Renoise a row makes, until the job waits)
    local live
    return page(job, epoch, params, phrase.number_of_lines, function(i, waited)
      if waited or not live then live = the_song():instrument(at[1]):phrase(at[2]) end
      return line_row(live:line(i), i - 1, note_columns, effect_columns, params.hidden == true)
    end, true, { lines = phrase.number_of_lines, lpb = phrase.lpb })
  elseif kind == "line" then
    local pattern_at = need_parent(params, "pattern", 1, epoch)[1]
    if pattern_at > #song.patterns then fail("bad-params", "no pattern " .. pattern_at) end
    local track_at = track_param(params.track, epoch, song)
    local track = song:track(track_at)
    local note_columns, effect_columns = track.visible_note_columns, track.visible_effect_columns
    local total = song:pattern(pattern_at).number_of_lines
    -- An alias shows the lines of another pattern: they are that pattern's, and a write to them changes it.
    local alias = song:pattern(pattern_at):track(track_at).alias_pattern_index
    local extra = alias > 0 and { aliasOf = alias - 1, aliasRef = refs.make(epoch, "pattern", alias) } or {}
    extra.mutedColumns = rows.column_state(track)
    -- (each line by Renoise's number, from 0; the pattern track looked up once a slice: a line is the one call to Renoise
    -- a row makes, until the job waits)
    local live
    return page(job, epoch, params, total, function(i, waited)
      if waited or not live then live = the_song():pattern(pattern_at):track(track_at) end
      return line_row(live:line(i), i - 1, note_columns, effect_columns, params.hidden == true)
    end, true, extra)
  elseif kind == "automation" then
    -- M4 (n-song): the automation of one pattern's track, an envelope a row
    local pattern_at = need_parent(params, "pattern", 1, epoch)[1]
    if pattern_at > #song.patterns then fail("bad-params", "no pattern " .. pattern_at) end
    local track_at = track_param(params.track, epoch, song)
    local list = rows.envelopes(song, epoch, pattern_at, track_at)
    return page(job, epoch, params, #list, function(i) return list[i] end)
  elseif kind == "instrument" then
    return page(job, epoch, params, #song.instruments, function(i) return instrument_row(the_song(), epoch, i) end)
  elseif kind == "sample" then
    local at = need_parent(params, "instrument", 1, epoch)[1]
    if at > #song.instruments then fail("bad-params", "no instrument " .. at) end
    return page(job, epoch, params, #song:instrument(at).samples, function(s) return sample_row(the_song(), epoch, at, s) end)
  elseif kind == "phrase" then
    local at = need_parent(params, "instrument", 1, epoch)[1]
    if at > #song.instruments then fail("bad-params", "no instrument " .. at) end
    return page(job, epoch, params, #song:instrument(at).phrases, function(p) return phrase_row(the_song(), epoch, at, p) end)
  elseif kind == "device" then
    local at = need_parent(params, "track", 1, epoch)[1]
    if at > track_count(song) then fail("bad-params", "no track " .. at) end
    return page(job, epoch, params, #song:track(at).devices, function(d) return device_row(the_song(), epoch, at, d) end)
  elseif kind == "param" then
    -- (n-instruments: a sample FX device's parameters, or a plug-in instrument's)
    local parent = refs.parse(params.parent)
    if parent and parent.kind == "chaindevice" then
      local at = refs.expect(params.parent, "chaindevice", 3, epoch)
      local device = instrument_object(song, "chaindevice", at)
      if not device then return fail("bad-params", "no such device") end
      return page(job, epoch, params, #device.parameters, function(k)
        local now = instrument_object(the_song(), "chainparam", { at[1], at[2], at[3], k })
        if not now then fail("stale", "the device changed while it was read") end
        return parameter_row(refs.make(epoch, "chainparam", at[1], at[2], at[3], k), k, now)
      end)
    elseif parent and parent.kind == "instrument" then
      local i = refs.expect(params.parent, "instrument", 1, epoch)[1]
      if i > #song.instruments or not song:instrument(i).plugin_properties.plugin_loaded then fail("bad-params", "that instrument has no plug-in") end
      return page(job, epoch, params, #song:instrument(i).plugin_properties.plugin_device.parameters, function(k)
        local device = the_song():instrument(i).plugin_properties.plugin_device
        return parameter_row(refs.make(epoch, "pluginparam", i, k), k, device:parameter(k))
      end)
    end
    local at = need_parent(params, "device", 2, epoch)
    local t, d = at[1], at[2]
    if t > track_count(song) or d > #song:track(t).devices then fail("bad-params", "no such device") end
    local with_grid = reads_listening.grid_of(params)
    return page(job, epoch, params, #song:track(t):device(d).parameters, function(k)
      local live = the_song()
      local row = param_row(live, epoch, t, d, k)
      return with_grid and with_grid(row, live:track(t):device(d):parameter(k)) or row
    end)
  elseif kind == "tidy" then
    return tidy_page(song, epoch, tidy_from(params))
  elseif kind == "tidy-notes" then
    return notes_page(song, tidy_from(params), params.byTrack == true)
  elseif kind == "spans" then
    return reads_listening.spans(job, params, epoch, page)
  elseif kind == "modset" or kind == "chain" then
    -- n-instruments: an instrument's modulation sets or sample FX chains
    local at = need_parent(params, "instrument", 1, epoch)[1]
    if at > #song.instruments then fail("bad-params", "no instrument " .. at) end
    local count = kind == "modset" and #song:instrument(at).sample_modulation_sets or #song:instrument(at).sample_device_chains
    return page(job, epoch, params, count, function(k)
      if kind == "modset" then return modset_row(the_song(), epoch, at, k) end
      return chain_row(the_song(), epoch, at, k)
    end)
  end
  fail("bad-params", "unknown kind " .. tostring(kind) .. " (song, track, sequence, pattern, line, automation, instrument, sample, phrase, device, param, modset, chain)")
end

return M
