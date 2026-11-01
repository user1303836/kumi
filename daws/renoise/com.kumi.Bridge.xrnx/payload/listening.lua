-- Listening (M5): the song or one track rendered offline as a job (render_start, listen_status, listen_cancel), the audio behind
-- a ref (audio_file), and what plans them: the song's timeline (the beat and second of every line) and Kumi's listening folder.
--
-- Measured in Renoise 3.5.4 (m5/renoise.md): `song:render` renders in the background while the tool's sockets and timers keep
-- running, calls back once the render ends (also after `cancel_rendering`, with the file cut short), raises for a line out of
-- range or a second render, takes `end_pos` inclusive, stops playback, and reads `transport.playing` true meanwhile. Its
-- `cancel_rendering` leaves Renoise's undo dead until the app loses focus, so nothing here calls it: a render in flight runs to
-- its end (renders run 50-200 times faster than real time). Renoise renders no single track: one is soloed, and Renoise solos by
-- rewriting every other track's mute state (an un-solo sets every track ACTIVE), so a job snapshots every track's mute and solo
-- first and writes them all back after its last render. Neither a render nor mute or solo is an undo step or marks the song
-- modified.

local import = ...
local json = import("json")
local refs = import("refs")
local rows = import("rows")
local base = import("changes_base")
local log = import("log")

local fail, bad = base.fail, base.bad
local q = base.q

local M = {}

M.PROTOCOL = 1
M.FOLDER = "listening"
M.MAX_SOURCES = 18
M.MAX_SECONDS = 3600
M.RATES = { [44100] = true, [48000] = true, [88200] = true, [96000] = true }
M.DEFAULT_RATE = 48000
-- How long a render may read as ended with no callback before it counts as stopped (the callback comes as `rendering` turns
-- false: measured).
M.STOPPED_GRACE_MS = 1000
-- Ended jobs kept for listen_status, and the files a job rendered, kept for audition (where each came from).
M.KEEP_ENDED = 16
M.KEEP_FILES = 64
M.DEMO_TEXT = "The Renoise demo can't render to a file, so Kumi can't hear the song: a licensed Renoise can"

-- The listening state, kept across payload reloads (the bridge hands its own table).
local S

function M.init(state)
  state.listen = state.listen or {}
  S = state.listen
  S.ended = S.ended or {}
  S.ended_order = S.ended_order or {}
  S.files = S.files or {}
  S.files_order = S.files_order or {}
  S.zcache = S.zcache or {}
  return S
end

local MASTER = function() return renoise.Track.TRACK_TYPE_MASTER end

-- Files and the folder

local function windows()
  return os.platform ~= nil and os.platform() == "WINDOWS"
end

-- <Kumi's folder>/bridges/renoise/listening: beside the secret the reference names, which is the bridge's own folder.
function M.folder(cfg)
  local secret = type(cfg) == "table" and cfg.secret_file or nil
  if type(secret) ~= "string" then return nil end
  local root, sep = secret:match("^(.*)([/\\])[^/\\]+$")
  if not root or root == "" then return nil end
  return root .. sep .. M.FOLDER, sep
end

-- The folder, made owner-only when it isn't there yet; nil and why when it can't be.
function M.ensure_folder(cfg)
  local folder = M.folder(cfg)
  if not folder then return nil, "the bridge isn't set up (no reference)" end
  if not io.exists(folder) then
    local ok = os.mkdir(folder)
    if not ok and not io.exists(folder) then return nil, "Kumi's listening folder can't be made: " .. folder end
  end
  if not windows() and io.chmod then pcall(io.chmod, folder, 700) end
  return folder
end

local function remove(path)
  if type(path) ~= "string" or not io.exists(path) then return true end
  local ok = os.remove(path)
  -- (stock os.remove takes Windows' ANSI code page: a folder with other letters isn't found by it. The host prunes the rest.)
  return ok ~= nil
end
M.remove = remove

local function u16(s, i)
  local a, b = s:byte(i, i + 1)
  return a + b * 256
end

local function u32(s, i)
  local a, b, c, d = s:byte(i, i + 3)
  return a + b * 256 + c * 65536 + d * 16777216
end

-- What a WAV file holds, from its header: {channels, rate, bits, format, frames, seconds, bytes}; nil and why.
function M.wav_info(path)
  local f = io.open(path, "rb")
  if not f then return nil, "missing" end
  local size = f:seek("end")
  f:seek("set", 0)
  local head = f:read(8192) or ""
  f:close()
  if #head < 12 or head:sub(1, 4) ~= "RIFF" or head:sub(9, 12) ~= "WAVE" then return nil, "not a WAV file" end
  local pos, fmt, data_at, data_size = 13, nil, nil, nil
  while pos + 7 <= #head do
    local id, length = head:sub(pos, pos + 3), u32(head, pos + 4)
    if id == "fmt " and pos + 23 <= #head then
      local tag = u16(head, pos + 8)
      if tag == 0xFFFE and pos + 33 <= #head then tag = u16(head, pos + 32) end
      fmt = { tag = tag, channels = u16(head, pos + 10), rate = u32(head, pos + 12), align = u16(head, pos + 20), bits = u16(head, pos + 22) }
    elseif id == "data" then
      data_at, data_size = pos + 8, length
      break
    end
    pos = pos + 8 + length + (length % 2)
  end
  if not fmt or not data_at or fmt.align == 0 or fmt.rate == 0 then return nil, "no audio in it" end
  -- (a writer that streams leaves the size open: what follows the header is the audio)
  if data_size == 0xFFFFFFFF or data_at - 1 + data_size > size then data_size = size - (data_at - 1) end
  local frames = math.floor(data_size / fmt.align)
  return {
    channels = fmt.channels, rate = fmt.rate, bits = fmt.bits, format = fmt.tag == 3 and "float" or "pcm", frames = frames,
    seconds = frames / fmt.rate, bytes = size,
  }
end

-- The timeline: every slot's lines with their beats and seconds

-- How long the timeline reads patterns in one request (rule 6: well under Renoise's 10 s watchdog): past it the request is
-- answered busy, and what was read is kept for the next. A read job's timeline waits for its next turn every YIELD_MS instead.
-- While nothing else runs, the bridge's ticks read the song's patterns ahead (WARM_MS a tick, every WARM_EVERY_MS).
M.SCAN_MS = 800
M.YIELD_MS = 8
M.WARM_MS = 15
M.WARM_EVERY_MS = 200

-- The clock (os.clock in ms): Renoise's is a high-precision timer.
local function clock_ms() return os.clock() * 1000 end

-- The global commands of a pattern that move time, in any track's effect columns (they act from the line they are on, measured:
-- a ZL08 on line 1 makes line 17 two beats): into `into.changes` ({line = {lpb?, bpm?}}), and `into.irregular` when it breaks or
-- delays its lines (ZB, ZD), which this timeline doesn't follow. Read a track at a time from `into.next_t`; `over()` after each
-- says the slice is used up: then the next track to read is kept in `into.next_t` and it answers false (true when all is read).
local function scan_pattern(song, p, tracks, into, over)
  local pattern = song:pattern(p)
  local lines = pattern.number_of_lines
  local changes = into.changes
  for t = into.next_t or 1, tracks do
    local columns = song:track(t).visible_effect_columns
    if columns > 0 then
      local pattern_track = pattern:track(t)
      -- (an alias shows its source's lines, and whether it is empty is the source's to say)
      if pattern_track.is_alias or not pattern_track.is_empty then
        for l = 1, lines do
          local line = pattern_track:line(l)
          if not line.is_empty then
            for c = 1, columns do
              local column = line:effect_column(c)
              local number = column.number_string
              if number:sub(1, 1) == "Z" then
                local amount = column.amount_value
                if number == "ZL" and amount > 0 then
                  changes[l] = changes[l] or {}
                  changes[l].lpb = amount
                elseif number == "ZT" and amount >= 32 then
                  changes[l] = changes[l] or {}
                  changes[l].bpm = amount
                elseif number == "ZB" or number == "ZD" then
                  into.irregular = true
                end
              end
            end
          end
        end
      end
    end
    if t < tracks and over() then
      into.next_t = t + 1
      return false
    end
  end
  into.next_t = nil
  return true
end

-- Which of pattern p's tracks read for commands (those with effect columns) are aliases, and of what pattern: `{ {t, q}, ... }`.
-- Kept in `S.aliases[p]` while the pattern's own mark and the tracks' columns hold: a track made an alias or not counts for its
-- pattern (bridge.lua `watch_alias`), so a held mark means the same aliases. Otherwise every timeline read, and every warm pass
-- after it, looked at every track of every pattern (a `track`, a `pattern:track` and an `is_alias` each) to learn nothing new.
local function alias_tracks(song, p, tracks, mark, columns)
  local kept = S and S.aliases and S.aliases[p]
  if kept and kept.mark == mark and kept.tracks == tracks and kept.columns == columns then return kept.list end
  local pattern, list = song:pattern(p), {}
  for t = 1, tracks do
    if song:track(t).visible_effect_columns > 0 then
      local pattern_track = pattern:track(t)
      if pattern_track.is_alias then list[#list + 1] = { t, pattern_track.alias_pattern_index } end
    end
  end
  if S then
    S.aliases = S.aliases or {}
    S.aliases[p] = { mark = mark, tracks = tracks, columns = columns, list = list }
  end
  return list
end

-- The tracks' effect columns as one text (which tracks the commands are read from): part of what a kept alias list holds for.
local function columns_text(song, tracks)
  local parts = {}
  for t = 1, tracks do parts[t] = song:track(t).visible_effect_columns > 0 and "1" or "0" end
  return table.concat(parts)
end

-- Pattern p's mark for the cache: the count of its changed lines (`mark_of(p)`, by the bridge's line notifiers) and, for each
-- track read that is an alias, the pattern it shows with that one's count: an edit in the source tells the source's notifiers
-- only (measured). Nil when one is unknown. `columns` is `columns_text`'s, worked out once a read.
local function worked_out_mark(song, p, tracks, mark_of, columns)
  local mark = mark_of and mark_of(p) or nil
  if not mark then return nil end
  local aliases = alias_tracks(song, p, tracks, mark, columns or columns_text(song, tracks))
  if #aliases == 0 then return mark end
  local parts = { mark }
  for _, alias in ipairs(aliases) do
    local source = mark_of(alias[2])
    if not source then return nil end
    parts[#parts + 1] = alias[1] .. ":" .. alias[2] .. "=" .. source
  end
  return table.concat(parts, "|")
end

-- The same, once per pattern in one read of the song (`memo`: a pattern plays in many slots; `memo.columns` the tracks'
-- columns, worked out once).
local function cache_mark(song, p, tracks, mark_of, memo)
  if memo and memo[p] ~= nil then return memo[p] or nil end
  if memo and not memo.columns then memo.columns = columns_text(song, tracks) end
  local mark = worked_out_mark(song, p, tracks, mark_of, memo and memo.columns or nil)
  if memo then memo[p] = mark or false end
  return mark
end

-- Pattern p's cache entry while its lines (and those its aliases show) haven't changed (nil when unknown, which reads again),
-- whole or part read; nil when there's none.
local function cached_changes(song, p, tracks, mark_of, memo)
  local mark = cache_mark(song, p, tracks, mark_of, memo)
  local cached = S and S.zcache[p]
  if mark and cached and cached.mark == mark and cached.tracks == tracks then return cached end
  return nil, mark
end

-- A pattern's commands, from the cache, else read, going on from where a slice stopped. nil when `over()` stopped it part way.
local function pattern_changes(song, p, tracks, mark_of, over, memo)
  local cached, mark = cached_changes(song, p, tracks, mark_of, memo)
  if cached and not cached.next_t then return cached.changes, cached.irregular end
  local reading = cached or { mark = mark, tracks = tracks, changes = {}, irregular = false }
  local done = scan_pattern(song, p, tracks, reading, over)
  if mark and S then S.zcache[p] = reading end
  if not done then return nil end
  return reading.changes, reading.irregular
end

-- Reads the song's patterns ahead into the cache (in the sequence's order), WARM_MS at a time while nothing else runs, once
-- listening has asked for the timeline in this song: a render, an audition or a scratch copy then finds them read (every pattern
-- is read again after a song opens or is watched again). Stops when all are read, until the timeline is asked for again.
function M.warm(mark_of)
  if not S or not S.warm_wanted or not mark_of then return end
  local now = clock_ms()
  if S.warmed_ms and now - S.warmed_ms < M.WARM_EVERY_MS then return end
  S.warmed_ms = now
  local song = renoise.song()
  if not song or song.rendering then return end
  local stop = now + M.WARM_MS
  local function over() return clock_ms() >= stop end
  local tracks = rows.track_count(song)
  local sequencer = song.sequencer
  local seen, memo = {}, {}
  for s = 1, #sequencer.pattern_sequence do
    local p = sequencer:pattern(s)
    if not seen[p] then
      seen[p] = true
      local cached = cached_changes(song, p, tracks, mark_of, memo)
      if not cached or cached.next_t then
        if pattern_changes(song, p, tracks, mark_of, over, memo) == nil or over() then return end
      end
    end
  end
  S.warm_wanted = false
end

-- The song's timeline: `slots[s] = {pattern, lines, segments = {...}, beat, second, endBeat, endSecond}` in sequence order, a
-- segment holding the lines from `from` to the next one's start at one LPB and BPM: {from, beat, second, lpb, bpm, line_seconds,
-- sets_lpb, sets_bpm (the segment starts on such a command), lpb_met, bpm_met (one has come by its start, in sequence order)}.
-- Beats and seconds are as Renoise plays the song from its start (its own beats, edit_pos_beats, follow them: measured), from the
-- transport's LPB and BPM until the first commands; `first_lpb`, `first_bpm` are the song's first ZL and ZT values (a render
-- starts at them: see render_state). `irregular` when a pattern breaks or delays its lines, or the song keeps Renoise's old speed
-- timing: then the lines' times are Renoise's own and only said, not checked.
-- Read a slice at a time: inside a read job (a coroutine) it waits for the job's next turn every YIELD_MS; a request answers busy
-- past SCAN_MS (what was read is kept, so asking again goes on from there).
function M.timeline(song, mark_of)
  local transport = song.transport
  local speed = transport.timing_model == renoise.Transport.TIMING_MODEL_SPEED
  local lpb, bpm, tpl = transport.lpb, transport.bpm, transport.tpl
  local timeline = { lpb0 = lpb, bpm0 = bpm, tpl = tpl, speed = speed }
  local tracks = rows.track_count(song)
  local sequencer = song.sequencer
  local count = #sequencer.pattern_sequence
  local patterns = #song.patterns
  if S then S.warm_wanted = true end
  local over
  if coroutine.running() then
    local slice_end = clock_ms() + M.YIELD_MS
    over = function()
      if clock_ms() < slice_end then return false end
      coroutine.yield()
      -- (the song may have changed while the job waited)
      local now = renoise.song()
      if not now or rows.track_count(now) ~= tracks or #now.patterns ~= patterns or #now.sequencer.pattern_sequence ~= count then
        fail("changed", "tracks or patterns changed while Renoise read the song's tempo commands: read again")
      end
      slice_end = clock_ms() + M.YIELD_MS
      return false
    end
  else
    local stop = clock_ms() + M.SCAN_MS
    over = function() return clock_ms() >= stop end
  end
  local slots, beat, second = {}, 0, 0
  local marks = {}
  local irregular = speed
  local lpb_met, bpm_met = false, false
  local sets_lpb, sets_bpm = false, false
  for s = 1, count do
    local p = sequencer:pattern(s)
    local lines = song:pattern(p).number_of_lines
    local changes, broken = pattern_changes(song, p, tracks, mark_of, over, marks)
    if not changes then
      return fail("busy", string.format("Renoise is still reading the song's tempo commands for Kumi (pattern %d of %d in the sequence): ask again in a moment", s, count))
    end
    if broken then irregular = true end
    local slot = { pattern = p, lines = lines, segments = {}, beat = beat, second = second }
    local from = 1
    local function close_segment(to)
      if to <= from then return end
      local count_lines = to - from
      local line_seconds = M.line_seconds(timeline, bpm, lpb)
      slot.segments[#slot.segments + 1] = {
        from = from, beat = beat, second = second, lpb = lpb, bpm = bpm, line_seconds = line_seconds,
        sets_lpb = sets_lpb, sets_bpm = sets_bpm, lpb_met = lpb_met, bpm_met = bpm_met,
      }
      beat = beat + count_lines / lpb
      second = second + count_lines * line_seconds
      from = to
      sets_lpb, sets_bpm = false, false
    end
    for l = 1, lines do
      local change = changes[l]
      if change then
        close_segment(l)
        if change.lpb then
          lpb, lpb_met, sets_lpb = change.lpb, true, true
          timeline.first_lpb = timeline.first_lpb or change.lpb
        end
        if change.bpm then
          bpm, bpm_met, sets_bpm = change.bpm, true, true
          timeline.first_bpm = timeline.first_bpm or change.bpm
        end
      end
    end
    close_segment(lines + 1)
    slot.endBeat, slot.endSecond = beat, second
    slots[s] = slot
  end
  timeline.slots, timeline.beats, timeline.seconds, timeline.irregular = slots, beat, second, irregular
  return timeline
end

-- How long a line lasts at a BPM and LPB (Renoise's old speed timing: tpl ticks of 2.5/bpm s each).
function M.line_seconds(timeline, bpm, lpb)
  if timeline.speed then return timeline.tpl * 2.5 / bpm end
  return 60 / (bpm * lpb)
end

-- The segment of a slot holding line `l`.
local function segment_of(slot, l)
  local found = slot.segments[1]
  for _, segment in ipairs(slot.segments) do
    if segment.from <= l then found = segment else break end
  end
  return found
end

-- Where line `l` of slot `s` starts: its beat and second.
function M.line_start(timeline, s, l)
  local slot = timeline.slots[s]
  local segment = segment_of(slot, l)
  return segment.beat + (l - segment.from) / segment.lpb, segment.second + (l - segment.from) * segment.line_seconds
end

-- The line a beat falls in: slot, line, and that line's start (beat, second); a beat at the song's end or past it answers the
-- last line. A beat exactly at a line's start is in that line.
function M.line_at(timeline, wanted)
  local slots = timeline.slots
  for s, slot in ipairs(slots) do
    if wanted < slot.endBeat or s == #slots then
      for k = #slot.segments, 1, -1 do
        local segment = slot.segments[k]
        if wanted >= segment.beat or k == 1 then
          local next_from = slot.segments[k + 1] and slot.segments[k + 1].from or (slot.lines + 1)
          local l = segment.from + math.floor((wanted - segment.beat) * segment.lpb + 1e-9)
          l = math.max(segment.from, math.min(next_from - 1, l))
          local b, sec = M.line_start(timeline, s, l)
          return s, l, b, sec
        end
      end
    end
  end
  return nil
end

-- How Renoise renders time (measured, 3.5.4): a render starts at the BPM and LPB the last ZT and ZL before its first line set,
-- as playback would; but for a kind with no command before it, at the song's FIRST command of that kind anywhere in its sequence
-- (even one after the part), not at the transport's; then it applies the commands it meets. So a part before a song's first ZT
-- renders at that ZT's tempo where playback plays it at the transport's. Answers {bpm, lpb} the render starts at, and whether
-- that differs from how playback plays that line.
function M.render_state(timeline, s, l)
  local segment = segment_of(timeline.slots[s], l)
  -- (a segment that starts on a command has it met: one on the render's first line is the render's own)
  local state = {
    bpm = segment.bpm_met and segment.bpm or (timeline.first_bpm or segment.bpm),
    lpb = segment.lpb_met and segment.lpb or (timeline.first_lpb or segment.lpb),
  }
  state.differs = state.bpm ~= segment.bpm or state.lpb ~= segment.lpb
  return state
end

-- Walks the lines from (s1, l1) on as a render that started there plays them, a stretch of one line length at a time:
-- visit(s, first, last, line_seconds) answers true to stop.
local function walk_render(timeline, s1, l1, state, visit)
  for s = s1, #timeline.slots do
    local slot = timeline.slots[s]
    for k, segment in ipairs(slot.segments) do
      local last = (slot.segments[k + 1] and slot.segments[k + 1].from or (slot.lines + 1)) - 1
      local first = (s == s1) and math.max(segment.from, l1) or segment.from
      if first <= last then
        -- (once a kind's command has come, before the render or in it, the render has it as playback does; until then it keeps
        -- the value it started at)
        local bpm = segment.bpm_met and segment.bpm or state.bpm
        local lpb = segment.lpb_met and segment.lpb or state.lpb
        if visit(s, first, last, M.line_seconds(timeline, bpm, lpb), segment) then return end
      end
    end
  end
end

-- The seconds a render of lines (s1, l1) to (s2, l2), both in, lasts.
function M.render_time(timeline, s1, l1, s2, l2, state)
  local total = 0
  walk_render(timeline, s1, l1, state, function(s, first, last, line_seconds)
    local stop = s == s2 and math.min(last, l2) or last
    if s > s2 or (s == s2 and first > l2) then return true end
    total = total + (stop - first + 1) * line_seconds
    return s == s2 and stop >= l2
  end)
  return total
end

-- Where a beat of the song is in a render that started at (s1, l1): seconds from its start.
function M.render_offset(timeline, s1, l1, state, wanted)
  local s, l, b = M.line_at(timeline, wanted)
  if not s then return 0 end
  local before = 0
  if s > s1 or l > l1 then
    local ps, pl = s, l - 1
    if pl < 1 then ps, pl = s - 1, timeline.slots[s - 1].lines end
    before = M.render_time(timeline, s1, l1, ps, pl, state)
  end
  -- (and the part of its line before the beat, at that line's rendered length)
  local line_seconds = M.render_time(timeline, s1, l1, s, l, state) - before
  local segment = segment_of(timeline.slots[s], l)
  return before + math.max(0, wanted - b) * segment.lpb * line_seconds
end

-- The line a render that started at (s1, l1) is at after `seconds`: slot, line.
function M.render_line_at(timeline, s1, l1, state, seconds)
  local at_s, at_l = s1, l1
  local elapsed = 0
  walk_render(timeline, s1, l1, state, function(s, first, last, line_seconds)
    local span = (last - first + 1) * line_seconds
    if elapsed + span > seconds + 1e-9 then
      at_s, at_l = s, first + math.floor((seconds - elapsed) / line_seconds + 1e-9)
      return true
    end
    elapsed = elapsed + span
    at_s, at_l = s, last
  end)
  return at_s, at_l
end

local function beat_text(beat)
  local text = string.format("%.3f", beat):gsub("0+$", ""):gsub("%.$", "")
  return text
end

-- The lines to render for a range: from the line `fromBeat - preRoll` falls in to the last line that starts before `fromBeat +
-- beats` (Renoise's end_pos is inclusive), with what the answer says: the file's length and where the part begins in it.
function M.plan_range(timeline, from_beat, beats, pre_roll)
  if #timeline.slots == 0 then bad("the song has no sequence") end
  local song_end = timeline.beats
  if from_beat >= song_end then
    bad("the song ends at beat " .. beat_text(song_end) .. ": fromBeat must be before it")
  end
  local start_beat = math.max(0, from_beat - (pre_roll or 0))
  local end_beat = math.min(from_beat + beats, song_end)
  local s1, l1, b1, sec1 = M.line_at(timeline, start_beat)
  -- (the last line starting before the end: the line the end falls in, or the one before when it falls on a line's start)
  local s2, l2, b2 = M.line_at(timeline, end_beat)
  if b2 >= end_beat - 1e-9 and not (s2 == s1 and l2 == l1) then
    -- the line before (s2, l2)
    if l2 > 1 then
      l2 = l2 - 1
    else
      s2 = s2 - 1
      l2 = timeline.slots[s2].lines
    end
  end
  return M.plan_lines(timeline, s1, l1, s2, l2, math.max(from_beat, b1), from_beat + beats > song_end + 1e-9)
end

-- A render of lines (s1, l1) to (s2, l2), timed as Renoise renders them (render_state): its length, where `part_beat` is in it,
-- and how it starts.
function M.plan_lines(timeline, s1, l1, s2, l2, part_beat, clipped)
  local lines = 0
  for s = s1, s2 do
    local first = s == s1 and l1 or 1
    local last = s == s2 and l2 or timeline.slots[s].lines
    lines = lines + (last - first + 1)
  end
  local state = M.render_state(timeline, s1, l1)
  local start_beat = M.line_start(timeline, s1, l1)
  local segment = segment_of(timeline.slots[s1], l1)
  return {
    start = { s1, l1 }, ["end"] = { s2, l2 }, lines = lines, startBeat = start_beat, state = state,
    played = { bpm = segment.bpm, lpb = segment.lpb },
    seconds = M.render_time(timeline, s1, l1, s2, l2, state),
    -- (where the part begins in the file: the pre-roll, and the part of the first line before it)
    offset = part_beat and M.render_offset(timeline, s1, l1, state, part_beat) or 0,
    clipped = clipped,
  }
end

-- Solo and mute: a snapshot of every track, and writing it back

-- Every non-master track's mute state and solo, with what tells the track apart (its kind and own name).
local function snapshot(song)
  local out = {}
  for t = 1, rows.track_count(song) do
    local track = song:track(t)
    if track.type ~= MASTER() then
      out[t] = { mute = track.mute_state, solo = track.solo_state == true, id = base.track_id(track) }
    end
  end
  return out
end
M.snapshot = snapshot

local function same_state(a, b)
  return a ~= nil and b ~= nil and a.mute == b.mute and a.solo == b.solo
end


-- Every solo taken away (Renoise turns every track Active); false when one stays. The setter toggles a track's own solo, so `false`
-- on a track soloed only along with another (a group's member) solos it on its own first (measured, 3.5.4).
local unsolo_all = rows.unsolo_all

-- The tracks that read soloed now, as a key ("2,6"), and how close they are to `set` (each wanted one counts, each other against).
local function soloed_now(song, set)
  local key, score = {}, 0
  for t = 1, rows.track_count(song) do
    local track = song:track(t)
    if track.type ~= MASTER() and track.solo_state then
      key[#key + 1] = t
      score = score + (set[t] and 1 or -2)
    end
  end
  return table.concat(key, ","), score
end

-- Writes `wanted` back (a snapshot, carried to the tracks' places now by `place_now(t)`), except on the tracks someone changed
-- since Kumi's last write (`others`: track -> their state, kept). Renoise solos by rewriting mutes and an un-solo sets every track
-- ACTIVE, so the order is: every solo off, the producer's solo on again (the one track whose solo makes exactly the tracks that read
-- soloed: a group's brings its members), then every mute state as it was. Returns the notes.
local function write_back(song, wanted, kumi_solo, others, place_now)
  local notes = {}
  local placed = {}
  local lost = 0
  for t, state in pairs(wanted) do
    local now_t = place_now(t)
    if now_t and now_t >= 1 and now_t <= rows.track_count(song) and song:track(now_t).type ~= MASTER()
      and base.track_id(song:track(now_t)) == state.id then
      placed[now_t] = state
    else
      lost = lost + 1
    end
  end
  -- 1. every solo off (every track ACTIVE, Renoise's doing; what someone else changed is written again below)
  if kumi_solo and not unsolo_all(song) then notes[#notes + 1] = "Renoise kept a solo on that Kumi couldn't take off." end
  -- 2. the producer's solo on again: solo through the API is exclusive, so the one track whose solo makes exactly the tracks that
  -- read soloed (tried from the last back, each from no solo; a group's solo brings its members), else the closest, said
  local order = {}
  for t in pairs(placed) do order[#order + 1] = t end
  table.sort(order)
  local soloed, set = {}, {}
  for _, t in ipairs(order) do
    if (others[t] or placed[t]).solo then
      soloed[#soloed + 1] = t
      set[t] = true
    end
  end
  if #soloed > 0 then
    local key = table.concat(soloed, ",")
    local found, best, best_score = false, nil, nil
    for k = #soloed, 1, -1 do
      unsolo_all(song)
      song:track(soloed[k]).solo_state = true
      local now_key, score = soloed_now(song, set)
      if now_key == key then
        found = true
        break
      end
      if best_score == nil or score >= best_score then best, best_score = soloed[k], score end
    end
    if not found and best then
      unsolo_all(song)
      song:track(best).solo_state = true
    end
    for _, t in ipairs(soloed) do
      if not song:track(t).solo_state then
        notes[#notes + 1] = string.format("Renoise lets a script solo one track at a time: %s is soloed again, %s isn't (it sounds as it did).",
          q(base.named(best, song:track(best))), q(base.named(t, song:track(t))))
      end
    end
  end
  -- 3. every mute state as it was
  for _, t in ipairs(order) do
    local state = others[t] or placed[t]
    if song:track(t).mute_state ~= state.mute then song:track(t).mute_state = state.mute end
  end
  if lost > 0 then
    notes[#notes + 1] = string.format("Kumi couldn't put back the mute and solo of %s: the tracks changed in Renoise during the render.",
      lost == 1 and "1 track" or (lost .. " tracks"))
  end
  return notes, placed
end

-- The job

local function now_ms() return os.clock() * 1000 end

local function ended(job)
  return job.state == "done" or job.state == "failed" or job.state == "cancelled"
end

local function remember_end(job)
  S.ended[job.id] = job
  S.ended_order[#S.ended_order + 1] = job.id
  while #S.ended_order > M.KEEP_ENDED do
    S.ended[table.remove(S.ended_order, 1)] = nil
  end
end

-- What audition plays: the part of the song a file was rendered from.
local function remember_file(job, source)
  S.files[source.file] = {
    job = job.id, source = source.id, what = source.what, track = source.track, document = job.document, epoch = job.epoch,
    start = { job.range.start[1], job.range.start[2] }, ["end"] = { job.range["end"][1], job.range["end"][2] },
    startBeat = job.range.startBeat, seconds = source.seconds, offset = job.range.offset, name = source.name, state = job.range.state,
  }
  S.files_order[#S.files_order + 1] = source.file
  while #S.files_order > M.KEEP_FILES do S.files[table.remove(S.files_order, 1)] = nil end
end

function M.origin(file)
  return S and S.files[file] or nil
end

function M.active()
  local job = S and S.job
  return job ~= nil and not ended(job) and job or nil
end

-- Why other requests wait while a job runs.
function M.busy_reason()
  if M.active() then return "Renoise is rendering for Kumi" end
  return nil
end

-- The job's sources in render order: the mix first (the song as the producer has it), then the tracks, each soloed.
local function render_order(sources)
  local order = {}
  for i, source in ipairs(sources) do
    if source.what == "master" then order[#order + 1] = i end
  end
  for i, source in ipairs(sources) do
    if source.what ~= "master" then order[#order + 1] = i end
  end
  return order
end

local function file_answer(job, source)
  return {
    id = source.id, file = source.file, start = job.range.offset, seconds = source.seconds, channels = source.channels,
    sampleRate = source.rate, stage = source.what == "master" and "master" or "post-fader", throughMaster = true,
    kind = source.kind, name = source.name,
  }
end

-- What someone else changed since Kumi's last write (`job.after`, numbered as the job's tracks are): kept in `job.others`, the
-- latest state each such track had, to be left as it was set.
local function note_others(job, song, env)
  if not job.after then return end
  local now = snapshot(song)
  for t, after in pairs(job.after) do
    local at = env.shifted(t, job.layout)
    local current = at and now[at]
    if current and not same_state(current, after) then job.others[t] = current end
  end
end

-- Puts back every mute and solo the job changed (`env` from the bridge: shifted, quiet). Idempotent.
local function restore(job, env, song)
  if job.restored or not job.snapshot or not job.changed then
    job.restored = true
    return
  end
  job.restored = true
  if not song or env.document() ~= job.document then return end
  local function place_now(t) return (env.shifted(t, job.layout)) end
  note_others(job, song, env)
  local others, changed_by_others = {}, 0
  for t, state in pairs(job.others) do
    local at = place_now(t)
    if at then
      others[at] = state
      changed_by_others = changed_by_others + 1
    end
  end
  local notes
  env.quiet(function()
    notes = write_back(song, job.snapshot, job.solo_at, others, place_now)
  end)
  for _, note in ipairs(notes) do job.notes[#job.notes + 1] = note end
  if changed_by_others > 0 then
    job.notes[#job.notes + 1] = string.format("%s mute or solo was changed during the render and left as it was set.",
      changed_by_others == 1 and "1 track's" or (changed_by_others .. " tracks'"))
  end
  -- read back: the tracks are as they were (or as someone else set them)
  local back = snapshot(song)
  local wrong = 0
  for t, state in pairs(job.snapshot) do
    local at = place_now(t)
    local want = at and (others[at] or state)
    if at and back[at] and want and back[at].mute ~= want.mute then wrong = wrong + 1 end
  end
  if wrong > 0 then
    job.notes[#job.notes + 1] = string.format("Renoise kept %s mute state as the render left it: look at %s.", wrong == 1 and "1 track's" or (wrong .. " tracks'"), wrong == 1 and "it" or "them")
    log.write("error", "listening: " .. wrong .. " mute states didn't read back")
  end
end

local function delete_files(job)
  for _, source in ipairs(job.sources) do remove(source.file) end
end

-- A render plays the song's ZT and ZL commands into the transport, as playback does (measured: no undo step, the song not marked
-- modified), from the tempo it starts at (render_state): what it leaves is said, for Kumi to set back if it wants (a song-bpm or
-- song-lpb change of its own).
local function note_tempo(job, env, song)
  local before = job.tempo_before
  if not before or not song or env.document() ~= job.document then return end
  local transport = song.transport
  if transport.bpm == before.bpm and transport.lpb == before.lpb then return end
  job.tempo = { bpm = transport.bpm, lpb = transport.lpb, before = { bpm = before.bpm, lpb = before.lpb } }
  job.notes[#job.notes + 1] = string.format("Renoise's render played the song's tempo commands into its transport: it is at %s BPM, LPB %d now (it was at %s BPM, LPB %d).",
    beat_text(transport.bpm), transport.lpb, beat_text(before.bpm), before.lpb)
end

local function finish(job, env, state, code, message)
  local song = renoise.song()
  restore(job, env, song)
  note_tempo(job, env, song)
  job.state = state
  job.rendering = nil
  if state ~= "done" then
    delete_files(job)
    job.error, job.code = message, code
  else
    for _, source in ipairs(job.sources) do remember_file(job, source) end
  end
  job.ended_ms = now_ms()
  remember_end(job)
  if env.on_end then env.on_end(job) end
  log.write(state == "failed" and "error" or "info", string.format("listening job %s %s%s", job.id, state, message and (": " .. message) or ""))
end
M.finish = finish

-- Solos source `k`'s track on its own (a master source wants none of Kumi's: the song as the producer has it, so any of Kumi's
-- solo goes first). Every solo goes first: a track that reads soloed may be so along with others (a group's solo, a GUI solo of
-- several), and the setter does nothing on it. Answers nothing; raises with the error to fail with.
local function set_up(job, env, song, source)
  if source.what == "master" then
    if job.solo_at then
      restore(job, env, song)
      job.restored, job.solo_at, job.changed, job.after = false, nil, false, nil
    end
    return
  end
  -- (the job's tracks are numbered as when it started: a track made, deleted or moved meanwhile ends it, before anything is written)
  if env.layout() ~= job.layout then
    fail("stale", "tracks were made, deleted or moved in Renoise during the render: read again")
  end
  local at = source.track
  if at > rows.track_count(song) or base.track_id(song:track(at)) ~= source.id_of then
    fail("stale", "the track " .. q(source.label) .. " isn't where it was when the render started: read again")
  end
  note_others(job, song, env)
  local track = song:track(at)
  if job.solo_at ~= at then
    local alone
    env.quiet(function()
      alone = unsolo_all(song)
      if alone then track.solo_state = true end
    end)
    job.changed = true
    job.solo_at = source.track
    if not alone then fail("failed", "Renoise kept a solo on, so Kumi couldn't render " .. q(source.label) .. " on its own") end
  end
  -- what Kumi (and Renoise, for it) left, to tell another's change from it later
  job.after = snapshot(song)
end

local start_render

-- The render of source `k` (in render order) ended: its file is checked, then the next starts, or the job ends.
local function rendered(job, env, k)
  if ended(job) or job.dead then
    -- (a job that failed or was stopped meanwhile: what Renoise wrote isn't wanted)
    local source = job.sources[job.order[k]]
    if source and job.state ~= "done" then remove(source.file) end
    return
  end
  job.rendering = nil
  job.done = k
  local source = job.sources[job.order[k]]
  local info, why = M.wav_info(source.file)
  if not info then
    return finish(job, env, "failed", "failed", "Renoise stopped the render (its file is " .. tostring(why) .. ")")
  end
  -- (a render cut short still calls back: measured after cancel_rendering. One line's worth is slack for rounding.)
  local expected = job.range.seconds * info.rate
  local slack = info.rate * job.line_seconds + 64
  if not job.irregular and info.frames + slack < expected then
    return finish(job, env, "failed", "failed", string.format("Renoise stopped the render after %.1f of %.1f s", info.seconds, job.range.seconds))
  end
  source.seconds, source.channels, source.rate, source.frames = info.seconds, info.channels, info.rate, info.frames
  if not windows() and io.chmod then pcall(io.chmod, source.file, 600) end
  if job.stopping then
    job.notes[#job.notes + 1] = "Renoise finished the render before it could stop."
    return finish(job, env, "cancelled")
  end
  if k >= #job.order then
    return finish(job, env, "done")
  end
  start_render(job, env, k + 1)
end

-- Starts the render of source `k`. A failure ends the job (everything put back, files deleted).
start_render = function(job, env, k)
  local song = renoise.song()
  if not song or env.document() ~= job.document then
    return finish(job, env, "failed", "stale", "the song was closed during the render")
  end
  local source = job.sources[job.order[k]]
  local ok, err = pcall(set_up, job, env, song, source)
  if not ok then
    local code = type(err) == "table" and err.code or "failed"
    return finish(job, env, "failed", code, type(err) == "table" and err.message or tostring(err))
  end
  local range = job.range
  local options = {
    start_pos = renoise.SongPos(range.start[1], range.start[2]), end_pos = renoise.SongPos(range["end"][1], range["end"][2]),
    sample_rate = job.rate, bit_depth = 32, interpolation = "precise", priority = "high",
  }
  job.current, job.rendering = k, { started_ms = now_ms() }
  local this_job = job
  local started, result, why = pcall(song.render, song, options, source.file, function()
    -- (a callback of Renoise's: nothing raises past here)
    local fine, problem = xpcall(rendered, debug.traceback, this_job, env, k)
    if not fine then
      log.write("error", "listening: " .. tostring(problem))
      if not ended(this_job) then finish(this_job, env, "failed", "failed", "Kumi's tool failed after the render: " .. tostring(problem):match("^[^\n]*")) end
    end
  end)
  if not started or result == false then
    local text = not started and tostring(result) or tostring(why)
    text = text:gsub("^std::logic_error: '", ""):gsub("'$", ""):gsub("^[^%s:]*%.lua:%d+: ", "")
    return finish(job, env, "failed", "failed", "Renoise wouldn't render: " .. text)
  end
end

-- The room the host says Kumi's folder has (`freeBytes`, sent with every job), against what is about to be written (contract §2.1:
-- seconds × rate × 8 bytes × sources × 1.5): short of it, nothing is started.
function M.check_room(free, needed, what)
  if type(free) ~= "number" or free ~= free or needed <= free then return end
  fail("failed", string.format("Kumi's folder has %d MB free; %s needs about %d MB", math.floor(math.max(0, free) / 1000000), what,
    math.ceil(needed / 1000000)))
end

-- render_start {jobId, sources, range, preRollBeats, sampleRate, epoch, layout?, freeBytes?}

local function need_id(value, name, prefix)
  if type(value) ~= "string" or not value:match("^" .. prefix .. "[0-9a-z]+$") or #value > 32 then
    bad(name .. " must be the host's id (" .. prefix .. " and base-36 letters)")
  end
  return value
end
M.need_id = need_id

local function number(value, name)
  if type(value) ~= "number" or value ~= value or value == math.huge or value == -math.huge then bad(name .. " must be a number") end
  return value
end

-- The sources asked for, checked: {id, what, track?, label?, id_of?}.
local function read_sources(song, env, list)
  if type(list) ~= "table" or #list < 1 or #list > M.MAX_SOURCES then bad("sources: 1 to " .. M.MAX_SOURCES .. " of them") end
  local seen, out = {}, {}
  for _, source in ipairs(list) do
    if type(source) ~= "table" then bad("each source is an object") end
    local id = source.id
    if type(id) ~= "string" or not id:match("^[A-Za-z0-9_%-]+$") or #id > 32 then bad("a source's id is 1 to 32 letters, digits, - or _") end
    if seen[id] then bad("two sources are named " .. id) end
    seen[id] = true
    local what = source.what
    if what == "master" then
      if source.beforeChain == true then bad("Renoise renders the mix after its Master track's devices") end
      out[#out + 1] = { id = id, what = "master", name = "Master" }
    elseif what == "track" then
      if source.stage ~= nil and source.stage ~= "pre-fader" and source.stage ~= "post-fader" then
        bad("a track source's stage is \"pre-fader\" or \"post-fader\"")
      end
      if source.beforeChain ~= nil and source.beforeChain ~= false then bad("beforeChain is for the master only") end
      local t = refs.expect(source.ref, "track", 1, env.epoch())[1]
      if t > rows.track_count(song) then fail("gone", "that track isn't in the song any more") end
      local track = song:track(t)
      if track.type == MASTER() then bad("the Master track is the mix: ask for what \"master\"") end
      out[#out + 1] = { id = id, what = "track", track = t, label = base.named(t, track), id_of = base.track_id(track), name = track.name }
    else
      bad("a source's what is \"master\" or \"track\"")
    end
  end
  return out
end

-- The checks a render job's start makes before anything is written; answers the job (not yet started).
local function plan(params, env, kind)
  if renoise.RENOISE_VERSION:find("[Dd]emo") then fail("failed", M.DEMO_TEXT) end
  local song = rows.the_song()
  local job_id = need_id(params.jobId, "jobId", "j")
  if M.active() then fail("busy", "Renoise is rendering for Kumi") end
  if song.rendering then fail("busy", "Renoise is rendering the song") end
  local reason = env.busy()
  if reason then fail("busy", reason) end
  if S.ended[job_id] then bad("there was a job " .. job_id .. " already") end
  local rate = params.sampleRate
  if rate == nil or rate == json.null then rate = M.DEFAULT_RATE end
  if not M.RATES[rate] then bad("sampleRate is 44100, 48000, 88200 or 96000") end
  local folder, why = M.ensure_folder(env.cfg())
  if not folder then fail("failed", why) end
  local _, sep = M.folder(env.cfg())
  local sources = params.sources
  if kind == "render" then sources = read_sources(song, env, params.sources) end
  local timeline = M.timeline(song, env.pattern_mark)
  local range
  if params.range then
    local r = params.range
    if type(r) ~= "table" then bad("range is {fromBeat, beats}") end
    local from_beat, beats = number(r.fromBeat, "range.fromBeat"), number(r.beats, "range.beats")
    if from_beat < 0 then bad("range.fromBeat must be 0 or more") end
    if beats <= 0 then bad("range.beats must be more than 0") end
    local pre_roll = params.preRollBeats
    if pre_roll == nil or pre_roll == json.null then pre_roll = 0 end
    pre_roll = number(pre_roll, "preRollBeats")
    if pre_roll < 0 then bad("preRollBeats must be 0 or more") end
    range = M.plan_range(timeline, from_beat, beats, pre_roll)
  else
    range = params.planned
  end
  if range.seconds > M.MAX_SECONDS then
    bad(string.format("that is %d minutes of music; Kumi renders at most %d at a time", math.ceil(range.seconds / 60), M.MAX_SECONDS / 60))
  end
  M.check_room(params.freeBytes, range.seconds * rate * 8 * #sources * 1.5, "this render")
  for _, source in ipairs(sources) do
    source.file = folder .. sep .. job_id .. "-" .. source.id .. ".wav"
    if io.exists(source.file) then bad("a file of job " .. job_id .. " is in Kumi's folder already") end
    source.seconds = range.seconds
  end
  return {
    id = job_id, kind = "render", state = "running", sources = sources, order = render_order(sources), range = range,
    rate = rate, epoch = env.epoch(), layout = env.layout(), document = env.document(), notes = {}, done = 0, others = {},
    irregular = timeline.irregular, line_seconds = M.line_seconds(timeline, range.state.bpm, range.state.lpb), started_ms = now_ms(),
  }
end
M.plan = plan

-- Starts a planned job: the snapshot, then the first render, in this callback.
function M.launch(job, env)
  local song = rows.the_song()
  if job.range.clipped then job.notes[#job.notes + 1] = "The song ends before the end of the part: the render stops there." end
  if job.irregular then job.notes[#job.notes + 1] = "The part has pattern breaks or delays (ZB, ZD), or the song keeps Renoise's old speed timing: its times are Renoise's." end
  if song.transport.playing then
    job.notes[#job.notes + 1] = "Renoise stopped playback to render."
  end
  local state, played = job.range.state, job.range.played
  if state and state.differs and played then
    job.notes[#job.notes + 1] = string.format("Renoise renders the part before the song's first tempo command at that command's %s BPM, LPB %d, not at the %s BPM, LPB %d playback has there: what Kumi heard there differs from playback.",
      beat_text(state.bpm), state.lpb, beat_text(played.bpm), played.lpb)
  end
  job.tempo_before = { bpm = song.transport.bpm, lpb = song.transport.lpb }
  for _, source in ipairs(job.sources) do
    if source.what == "track" then job.snapshot = job.snapshot or snapshot(song) end
  end
  -- (a change made while the mix renders, before Kumi's first solo, is someone else's too)
  job.after = job.snapshot
  if env.before_render then env.before_render() end
  S.job = job
  start_render(job, env, 1)
  return job
end

function M.start_answer(job, env)
  local files = {}
  for _, source in ipairs(job.sources) do files[#files + 1] = { id = source.id, file = source.file } end
  local answer = {
    jobId = job.id, kind = job.kind, state = job.state, seconds = job.range.seconds, files = json.array(files), epoch = env.epoch(),
    layout = env.layout(),
  }
  if job.state == "failed" then answer.error, answer.code = job.error, job.code end
  return answer
end

function M.render_start(params, env)
  if type(params) ~= "table" then bad("render_start needs {jobId, sources, range}") end
  if params.range == nil then bad("range is {fromBeat, beats}") end
  local job = plan(params, env, "render")
  M.launch(job, env)
  if job.state == "failed" then fail(job.code or "failed", job.error) end
  return M.start_answer(job, env)
end

-- A job's state now. A render that reads as ended with no callback for a while stopped (cancelled in Renoise, or failed).
function M.check(env)
  local job = M.active()
  if not job then
    -- (never raises: this runs before every request)
    local ok, err = pcall(M.warm, env.pattern_mark)
    if not ok then log.write("error", "listening: reading the tempo commands ahead: " .. tostring(err)) end
  end
  if not job or not job.rendering then return end
  local song = renoise.song()
  if not song or env.document() ~= job.document then
    job.dead = true
    return finish(job, env, "failed", "stale", "the song was closed during the render")
  end
  if song.rendering then
    job.rendering.quiet_since = nil
    return
  end
  local since = job.rendering.quiet_since
  if not since then
    job.rendering.quiet_since = now_ms()
  elseif now_ms() - since >= M.STOPPED_GRACE_MS then
    job.dead = true
    finish(job, env, "failed", "failed", "Renoise stopped the render")
  end
end

local function job_of(params)
  local id = type(params) == "table" and params.jobId or nil
  if type(id) ~= "string" then bad("jobId must be a job's id") end
  local job = (S.job and S.job.id == id) and S.job or S.ended[id]
  if not job then bad("there is no job " .. id) end
  return job
end

function M.status(params, env)
  M.check(env)
  local job = job_of(params)
  local song = renoise.song()
  local renders = #job.order
  local progress = job.done / math.max(1, renders)
  if job.state == "running" and job.rendering and song and song.rendering then
    progress = (job.done + math.min(1, math.max(0, song.rendering_progress))) / renders
  end
  if job.state == "done" then progress = 1 end
  local answer = { jobId = job.id, kind = job.kind, state = job.state, progress = progress, epoch = env.epoch(), layout = env.layout() }
  if job.stopping and job.state == "running" then answer.stopping = true end
  if job.state == "done" then
    local files = {}
    for _, source in ipairs(job.sources) do files[#files + 1] = file_answer(job, source) end
    answer.files = json.array(files)
  end
  if job.error then answer.error, answer.code = job.error, job.code end
  if #job.notes > 0 then answer.notes = json.array(job.notes) end
  if job.tempo then answer.tempo = job.tempo end
  return answer
end

-- listen_cancel: a job between renders stops at once; a render in flight runs to its end (Renoise's cancel_rendering breaks its
-- undo), then the job ends cancelled.
function M.cancel(params, env)
  M.check(env)
  local job = job_of(params)
  if ended(job) then return { jobId = job.id, state = job.state } end
  local song = renoise.song()
  if job.rendering and song and song.rendering then
    job.stopping = true
    return { jobId = job.id, state = "running", stopping = true }
  end
  job.dead = true
  finish(job, env, "cancelled")
  return { jobId = job.id, state = "cancelled" }
end

-- The tool is stopped or reloaded during a job: what it changed goes back now, its files go, and a render still in flight is
-- left to end with nothing listening.
function M.abort(env, why)
  local job = M.active()
  if not job then return end
  job.dead = true
  finish(job, env, "failed", "failed", why)
end

-- The song went (another opened): nothing to put back in it.
function M.song_closed(env)
  local job = M.active()
  if not job then return end
  job.dead = true
  job.restored = true
  finish(job, env, "failed", "stale", "the song was closed during the render")
end

-- audio_file {ref, epoch, layout?, jobId} (contract §2.6): a sample's audio saved as a WAV in Kumi's folder; a pattern's, a slot's or
-- a pattern track's lines rendered (a job, waited for with listen_status).

-- The first slot playing pattern `p`, or nil.
local function slot_of(song, p)
  local sequencer = song.sequencer
  for s = 1, #sequencer.pattern_sequence do
    if sequencer:pattern(s) == p then return s end
  end
  return nil
end

-- A slice's frames written as a 16-bit WAV, read value by value from its sample (Renoise saves only whole samples): for a
-- slice up to SLICE_VALUES values (a few seconds), so a tune loop on slices of a long break doesn't write the whole break each time.
M.SLICE_VALUES = 600000
local function write_slice(path, buffer, first, last)
  local channels, rate = buffer.number_of_channels, buffer.sample_rate
  local frames = last - first + 1
  local function u32le(n) return string.char(n % 256, math.floor(n / 256) % 256, math.floor(n / 65536) % 256, math.floor(n / 16777216) % 256) end
  local function u16le(n) return string.char(n % 256, math.floor(n / 256) % 256) end
  local bytes = frames * channels * 2
  local f = io.open(path, "wb")
  if not f then return false end
  f:write("RIFF", u32le(36 + bytes), "WAVE", "fmt ", u32le(16), u16le(1), u16le(channels), u32le(rate), u32le(rate * channels * 2),
    u16le(channels * 2), u16le(16), "data", u32le(bytes))
  local chunk = {}
  for frame = first, last do
    for channel = 1, channels do
      local value = buffer:sample_data(channel, frame)
      value = tonumber(type(value) == "table" and value[1] or value) or 0
      local n = math.floor(math.max(-1, math.min(1, value)) * 32767 + 0.5)
      if n < 0 then n = n + 65536 end
      chunk[#chunk + 1] = string.char(n % 256, math.floor(n / 256))
    end
    if #chunk >= 4096 then
      f:write(table.concat(chunk))
      chunk = {}
    end
  end
  f:write(table.concat(chunk))
  f:close()
  return true
end

local function export_sample(song, env, i, s, job_id, free)
  local instrument = song:instrument(i)
  local sample = instrument:sample(s)
  local name = sample.name ~= "" and sample.name or string.format("Sample %02d", s - 1)
  local offset, length, span
  local source = sample
  if sample.is_slice_alias then
    -- (a slice plays a part of the instrument's first sample: its frames alone are written when they're few enough, else
    -- that sample is saved, with where the slice is in it)
    source = instrument:sample(1)
    local markers = source.slice_markers
    local k = s - 1
    local rate = source.sample_buffer.has_sample_data and source.sample_buffer.sample_rate or nil
    if markers[k] and rate then
      local last = markers[k + 1] or (source.sample_buffer.number_of_frames + 1)
      offset, length = (markers[k] - 1) / rate, (last - markers[k]) / rate
      if (last - markers[k]) * source.sample_buffer.number_of_channels <= M.SLICE_VALUES then span = { markers[k], last - 1 } end
    end
  end
  local buffer = source.sample_buffer
  if not buffer.has_sample_data then fail("failed", "That sample has no audio") end
  local frames = span and (span[2] - span[1] + 1) or buffer.number_of_frames
  M.check_room(free, frames * buffer.number_of_channels * 4 * 1.5, "this sample")
  local folder, why = M.ensure_folder(env.cfg())
  if not folder then fail("failed", why) end
  local _, sep = M.folder(env.cfg())
  local path = folder .. sep .. job_id .. "-sample.wav"
  if io.exists(path) then bad("a file of job " .. job_id .. " is in Kumi's folder already") end
  if span then offset, length = nil, nil end
  -- (Renoise shows its own errors from save_as in a dialog: the folder is checked first, and that is all Kumi can do)
  local ok
  if span then ok = write_slice(path, buffer, span[1], span[2]) else ok = buffer:save_as(path, "wav") end
  local info = ok and M.wav_info(path) or nil
  if not info then
    remove(path)
    fail("failed", "Renoise couldn't save the sample's audio to Kumi's folder")
  end
  ---@cast info -nil
  if not windows() and io.chmod then pcall(io.chmod, path, 600) end
  local answer = { file = path, seconds = info.seconds, channels = info.channels, sampleRate = info.rate, kind = "exported", name = name }
  if offset then answer.offset, answer.length = offset, length end
  return answer
end

function M.audio_file(params, env)
  if type(params) ~= "table" then bad("audio_file needs {ref, jobId}") end
  local song = rows.the_song()
  local job_id = need_id(params.jobId, "jobId", "j")
  local parsed = refs.parse(params.ref)
  local kind = parsed and parsed.kind
  if kind == "sample" then
    local n = refs.expect(params.ref, "sample", 2, env.epoch())
    if n[1] > #song.instruments or n[2] > #song:instrument(n[1]).samples then fail("gone", "that sample isn't in the song any more") end
    return export_sample(song, env, n[1], n[2], job_id, params.freeBytes)
  end
  local s, p, t, label
  if kind == "seq" then
    s = refs.expect(params.ref, "seq", 1, env.epoch())[1]
    if s > #song.sequencer.pattern_sequence then fail("gone", "that slot isn't in the sequence any more") end
    p = song.sequencer:pattern(s)
  elseif kind == "pattern" or kind == "ptrack" then
    local n = refs.expect(params.ref, kind, kind == "pattern" and 1 or 2, env.epoch())
    p, t = n[1], n[2]
    if p > #song.patterns then fail("gone", "that pattern isn't in the song any more") end
    if t and t > rows.track_count(song) then fail("gone", "that track isn't in the song any more") end
    s = slot_of(song, p)
    if not s then fail("failed", "That pattern isn't in the song's sequence, so Renoise can't render it") end
  else
    bad("ref is a sample, a pattern, a sequence slot or a pattern track ref")
  end
  local pattern = song:pattern(p)
  label = pattern.name ~= "" and pattern.name or string.format("Pattern %02d", p - 1)
  local timeline = M.timeline(song, env.pattern_mark)
  local slot = timeline.slots[s]
  local sources
  if t then
    local track = song:track(t)
    if track.type == MASTER() then bad("the Master track's part is the mix: ask for the pattern") end
    sources = { { id = "ptrack", what = "track", track = t, label = base.named(t, track), id_of = base.track_id(track),
      name = base.named(t, track) .. " · " .. label, kind = "rendered" } }
  else
    sources = { { id = kind == "seq" and "slot" or "pattern", what = "master", name = label, kind = "rendered" } }
  end
  local planned = M.plan_lines(timeline, s, 1, s, slot.lines, nil, false)
  local job = plan({ jobId = job_id, sources = sources, planned = planned, sampleRate = params.sampleRate, freeBytes = params.freeBytes }, env, "audio_file")
  M.launch(job, env)
  if job.state == "failed" then fail(job.code or "failed", job.error) end
  return M.start_answer(job, env)
end

-- The listening flags status gives (contract §2.8).
function M.flags(cfg, audition)
  local demo = renoise.RENOISE_VERSION:find("[Dd]emo") ~= nil
  local folder = M.folder(cfg) ~= nil
  return {
    render = not demo and folder, capture = false, audition = not demo and folder and audition ~= false, grid = false,
    scratch = folder, version = M.PROTOCOL,
  }
end

return M
