-- Shared by Kumi's changes to a Renoise song (M3; see changes.lua): the helpers, the property framework (a change of one
-- value of an object, found again from the request), the journal of what the producer's undo could take back.
--
-- Every change is written in ONE synchronous callback named by `describe_undo(desc)` (`desc` carries the host's nonce), so the
-- producer's own undo takes it back whole. Kumi's own undo never calls Renoise's: it writes the prior values back as a new
-- step. Each kind knows what it would do (`preview`), how to do it (`apply`, which reads what the preview fenced again and
-- refuses `changed` before it writes), and how to put it back (`restore`, which refuses `changed` if the fields it touched no
-- longer hold what the apply wrote). Mute, solo, loop and selection are not Renoise undo steps: they answer `entry: false`.

local import = ...
local json = import("json")
local refs = import("refs")
local rows = import("rows")

local M = {}

local fail, the_song = rows.fail, rows.the_song
M.fail, M.the_song = fail, the_song

local function bad(message) fail("bad-params", message) end
M.bad = bad

-- The kinds of change, by name. A kind is either a property (a value of an object: `locate`, `read`, `wanted`, `write`, `same`,
-- `title`) or custom (`preview`, `apply`, `restore` of its own).
M.KINDS = {}
function M.register(spec) M.KINDS[spec.kind] = spec end


local MINUS = "\226\136\146" -- the true minus sign, U+2212
-- A name inside a sentence of a title is in curly quotes, as Live's titles have it: Deleted track “Kick”. (A title that opens
-- with the name of what changed, "Kick volume −6 dB", leaves it bare.)
function M.q(text) return "\226\128\156" .. tostring(text) .. "\226\128\157" end
local INFINITY = "\226\136\158" -- U+221E

-- Writes that make one slice of a restore (about a second of Renoise's time), and the most a journalled change may hold.
M.SLICE_WRITES = 15000
-- The wait between the numbered steps of a restore. Renoise made one undo step of two callbacks 10 ms apart (measured), and
-- kept callbacks 300 ms apart apart.
M.STEP_GAP_MS = 400
M.MAX_MATERIAL_BYTES = 8 * 1024 * 1024
-- The most numbered steps a restore may take (each about half a second of Renoise's time and 400 ms of waiting): it must answer
-- well inside the 30 s the host and Kumi wait. A track with more to put back than this isn't deleted by Kumi.
M.MAX_RESTORE_STEPS = 16
-- How long a sliced restore waits for a dialog to close, or a render to end, before it gives up (the host waits 30 s).
M.BUSY_WAIT_MS = 8000
-- The most writes one change makes in its one callback: about a second of Renoise's time (measured: 21,500 writes in 0.7 s).
M.MAX_APPLY_WRITES = 25000
M.JOURNAL_MAX = 64

-- Text

local function trimmed(value)
  local text = string.format("%.1f", value)
  return (text:gsub("%.0$", ""))
end

-- dB with the true minus sign, as the titles show it: `−6`, `+3`, `0`, `−6.5`; `−∞` at silence.
function M.db_text(db)
  if db == nil then return MINUS .. INFINITY end
  local rounded = math.floor(db * 10 + 0.5) / 10
  if rounded == 0 then return "0" end
  if rounded < 0 then return MINUS .. trimmed(-rounded) end
  return "+" .. trimmed(rounded)
end

-- A text's two 32-bit sums (a from 5381, b from 0; each byte in turn: a × 33 + the byte, b × 65599 + the byte, modulo 2^32).
-- Every print and hash the changes keep is made of them, and Kumi's host computes the same of preset data (presets.rs
-- `print_of`), so the numbers are fixed. Renoise runs plain Lua 5.1, where a loop over a text's bytes is the slowest thing
-- this tool does, and a device's preset data runs to megabytes with a plug-in's state in it: the bytes come sixteen to a
-- call, and the first sum takes four at a time (a × 33^4 + c1 × 33^3 + c2 × 33^2 + c3 × 33 + c4 stays under 2^53, which a
-- double holds exactly; the second sum's 65599^2 wouldn't). The same numbers as a byte at a time, in half the time (Lua
-- 5.1.5: 36 ms a megabyte, then 20).
local byte = string.byte
local M32 = 4294967296
function M.sums(text)
  local a, b = 5381, 0
  local n, i = #text, 1
  while i + 15 <= n do
    local c1, c2, c3, c4, c5, c6, c7, c8, c9, c10, c11, c12, c13, c14, c15, c16 = byte(text, i, i + 15)
    a = (a * 1185921 + c1 * 35937 + c2 * 1089 + c3 * 33 + c4) % M32
    b = (b * 65599 + c1) % M32
    b = (b * 65599 + c2) % M32
    b = (b * 65599 + c3) % M32
    b = (b * 65599 + c4) % M32
    a = (a * 1185921 + c5 * 35937 + c6 * 1089 + c7 * 33 + c8) % M32
    b = (b * 65599 + c5) % M32
    b = (b * 65599 + c6) % M32
    b = (b * 65599 + c7) % M32
    b = (b * 65599 + c8) % M32
    a = (a * 1185921 + c9 * 35937 + c10 * 1089 + c11 * 33 + c12) % M32
    b = (b * 65599 + c9) % M32
    b = (b * 65599 + c10) % M32
    b = (b * 65599 + c11) % M32
    b = (b * 65599 + c12) % M32
    a = (a * 1185921 + c13 * 35937 + c14 * 1089 + c15 * 33 + c16) % M32
    b = (b * 65599 + c13) % M32
    b = (b * 65599 + c14) % M32
    b = (b * 65599 + c15) % M32
    b = (b * 65599 + c16) % M32
    i = i + 16
  end
  for j = i, n do
    local c = byte(text, j)
    a = (a * 33 + c) % M32
    b = (b * 65599 + c) % M32
  end
  return a, b
end

-- A text's short print: its two sums as 16 hex digits (the fingerprints the changes keep).
function M.hash(text)
  local a, b = M.sums(text)
  return string.format("%08x%08x", a, b)
end

-- A note value as Renoise writes it: C-4 is 48, C#4 is 49 (its own naming, with the octave last).
local NOTE_NAMES = { "C-", "C#", "D-", "D#", "E-", "F-", "F#", "G-", "G#", "A-", "A#", "B-" }
function M.note_name(value) return NOTE_NAMES[value % 12 + 1] .. math.floor(value / 12) end

-- A range of notes, as a title says it: "C-5–B-5", or one note.
function M.notes_text(low, high)
  if low == high then return M.note_name(low) end
  return M.note_name(low) .. "\226\128\147" .. M.note_name(high)
end

function M.pan_text(pan)
  local percent = math.floor(math.abs(pan) * 100 + 0.5)
  if percent == 0 then return "centre" end
  if pan < 0 then return percent .. "% left" end
  return percent .. "% right"
end

local function close(a, b)
  if type(a) ~= "number" or type(b) ~= "number" then return a == b end
  return math.abs(a - b) <= 1e-6 * math.max(1, math.abs(a), math.abs(b))
end

local function db_of(linear)
  if type(linear) == "number" and linear > 0 then return 20 * math.log10(linear) end
  return nil
end

local function hex_of(color)
  if type(color) ~= "table" or not color[1] then return nil end
  return string.format("#%02x%02x%02x", color[1], color[2], color[3])
end

local function rgb_of(text)
  local r, g, b = tostring(text):match("^#(%x%x)(%x%x)(%x%x)$")
  if not r then return nil end
  return { tonumber(r, 16), tonumber(g, 16), tonumber(b, 16) }
end

-- A sliced change waits, without writing, while Renoise is busy (a dialog holds its timers, or it renders), before each slice; after
-- BUSY_WAIT_MS it gives up with `busy` (the host waits 30 s for the answer).
function M.wait_while_busy(job, env, what)
  local from = env.now_ms()
  while true do
    local reason = env.busy()
    if not reason then return end
    if env.now_ms() - from >= M.BUSY_WAIT_MS then
      error({ code = "busy", message = reason .. " for longer than Kumi waits" .. (what and (": " .. what) or "") }, 0)
    end
    job.step(50)
  end
end

-- The song and the track a request names

local function need_text(params, name)
  local value = params[name]
  if type(value) ~= "string" or value == "" then bad(name .. " must be a string") end
  return value
end

local function need_object(params, name)
  local value = params[name]
  if type(value) ~= "table" then bad(name .. " must be an object") end
  return value
end

-- The song, if the request's epoch is the song's and Renoise isn't busy.
local function context(params, env)
  local epoch = params.epoch
  if type(epoch) ~= "string" or epoch == "" then bad("epoch: the epoch the refs were read in") end
  if epoch ~= env.epoch() then
    fail("stale", "those refs are from an earlier epoch (" .. epoch .. "; now " .. env.epoch() .. "): the song changed, read again")
  end
  local song = the_song()
  if song.rendering then fail("busy", "Renoise is rendering the song; nothing changed") end
  -- A dialog holds Renoise's timers (measured: none runs, while the sockets answer): a change behind it waits for the producer.
  local busy = env.busy and env.busy()
  if busy then fail("busy", busy .. "; nothing changed") end
  return song
end

-- A track told apart from another by more than its place: its kind and its name. The names Renoise numbers itself ("Track 04", "S01",
-- "Group 03") follow the track's place, which moves, so they tell nothing and have none here (measured in Renoise 3.5.4).
local DEFAULT_NAME = { sequencer = "^Track %d+$", send = "^S%d+$", group = "^Group %d+$" }
local KIND_WORD = { sequencer = "track", group = "group", send = "send", master = "master" }

-- (`kind` is a track's kind as the rows name it: sequencer, group, send or master.)
function M.name_id(kind, name)
  local pattern = DEFAULT_NAME[kind]
  if name == nil or name == "" or (pattern and name:match(pattern)) then name = "" end
  return (KIND_WORD[kind] or "track") .. ":" .. name
end

function M.track_id(track)
  return M.name_id(rows.track_type(track), track.name)
end

local function track_at(song, index)
  if index < 1 or index > rows.track_count(song) then fail("gone", "that track isn't in the song any more") end
  return song:track(index)
end

local function find(song, env, reference, what)
  if type(reference) ~= "string" then bad(what .. ": ref must be a track ref") end
  local index = refs.expect(reference, "track", 1, env.epoch())[1]
  local track = track_at(song, index)
  return index, track
end

local function is_master(track) return track.type == renoise.Track.TRACK_TYPE_MASTER end

local function named(index, track)
  local name = track.name
  if name == nil or name == "" then return "Track " .. string.format("%02d", index) end
  return name
end

local function target(env, index)
  return { ref = refs.make(env.epoch(), "track", index), kind = "track" }
end

-- A track's volume or pan stage: post (the fader Renoise's mixer shows and the producer rides; the default) or pre (the
-- Vol/Pan before its devices).
local function stage_of(source)
  local stage = type(source) == "table" and source.stage or nil
  if stage == nil then return "post" end
  if stage ~= "pre" and stage ~= "post" then bad("stage must be \"pre\" or \"post\"") end
  return stage
end


M.MINUS, M.INFINITY = MINUS, INFINITY

-- A value as Renoise writes it ("-6.021 dB", "-INF dB", "-0.5"), with the true minus sign before its numbers, as Kumi's titles
-- write them.
function M.minus_text(text)
  if type(text) ~= "string" then return text end
  return (text:gsub("^%-([%d%.])", MINUS .. "%1"):gsub("([%s%(])%-([%d%.])", "%1" .. MINUS .. "%2")
    :gsub("^%-([Ii][Nn][Ff])", MINUS .. "%1"):gsub("([%s%(])%-([Ii][Nn][Ff])", "%1" .. MINUS .. "%2"))
end
M.close, M.db_of, M.hex_of, M.rgb_of = close, db_of, hex_of, rgb_of
M.need_text, M.need_object, M.context = need_text, need_object, context
M.track_at, M.find, M.is_master, M.named, M.target, M.stage_of = track_at, find, is_master, named, target, stage_of

-- The properties of an object

local NOT_UNDOABLE = "Renoise's own undo won't take this back; Kumi can."
M.NOT_UNDOABLE = NOT_UNDOABLE

function M.same_table(a, b)
  if type(a) ~= "table" or type(b) ~= "table" then return a == b end
  for k, v in pairs(a) do
    if not M.same_table(v, b[k]) then return false end
  end
  for k in pairs(b) do
    if a[k] == nil then return false end
  end
  return true
end

-- What a change is about: found from the request by `spec.locate(song, env, input)` as {object, name, place, target}, where
-- `place` is what the journal remembers to find it again (`spec.reread(song, place)`).
-- (A kind may keep numbers that mean something only in an epoch as refs on the wire: `wire` makes the state's wire form,
-- `unwire` takes it back and raises `stale` for a ref that is no longer one.)
-- (`loc` is what was located: a value may be a place, by what it is on)
local function wire(spec, state, env, loc)
  if spec.wire then return spec.wire(state, env, loc) end
  return state
end

local function unwire(spec, state, env, loc)
  if spec.unwire then return spec.unwire(state, env, loc) end
  return state
end

-- (`label` is what the producer calls the located object, for the refusal that names it)
local function with_id(state, id, label)
  if id == nil then return state end
  local copy = {}
  for k, v in pairs(state) do copy[k] = v end
  copy.id, copy.label = id, label
  return copy
end

-- The refusal for a ref that names another track than the one previewed: the previewed one is named, not the one at its place now.
function M.moved_text(label)
  return string.format("%s isn't at that place any more (a track was made, deleted or renamed): read again", M.q(label or "the track"))
end

-- What a property change wants: by what was located, where the kind needs that (`wanted_at`), else by the object.
local function wanted_of(spec, loc, input)
  if spec.wanted_at then return spec.wanted_at(loc, input) end
  return spec.wanted(loc.object, input)
end

-- What HISTORY's picture needs that the title only says (a parameter's name and range): a kind's `shown(loc)`, into its
-- display. For the display only: nothing restores from it.
local function add_shown(spec, loc, display)
  if not spec.shown then return end
  local ok, extra = pcall(spec.shown, loc)
  if ok and type(extra) == "table" then
    for k, v in pairs(extra) do display[k] = v end
  end
end

function M.prop_preview(song, env, spec, input)
  local loc = spec.locate(song, env, input)
  local now = spec.read(loc.object, input)
  local wanted = wanted_of(spec, loc, input)
  local display = { title = spec.title(loc.name, now, wanted), before = now, after = wanted }
  if spec.same(now, wanted) then display.unchanged = true end
  if not spec.undoable then display.note = NOT_UNDOABLE end
  -- (what the producer should know before it is made: columns hidden that hold notes)
  if spec.note then display.note = spec.note(loc, now, wanted) or display.note end
  add_shown(spec, loc, display)
  local shown = wire(spec, now, env, loc)
  -- (the fence names what was located as well as its value: another track at the ref's place is not this one; and, where the
  -- value is a place, what the wanted value names)
  local fence = with_id(shown, loc.id, loc.name)
  if spec.wire then
    local copy = {}
    for k, v in pairs(fence) do copy[k] = v end
    copy.wanted = wire(spec, wanted, env, loc)
    fence = copy
  end
  return { target = loc.target, prior = shown, fence = fence, display = display }
end

local function gone_is_changed(err)
  if type(err) == "table" and err.code == "gone" then
    error({ code = "changed", message = err.message .. ": it went since the preview" }, 0)
  end
  error(err, 0)
end
M.gone_is_changed = gone_is_changed

-- Writes `state` with the kind's own write; when Renoise refuses part of it (a field past what this device takes), what was
-- there (`was`) is written back, so nothing half-done stays, and the change fails with Renoise's words.
local function write_whole(spec, loc, state, was)
  local ok, err = pcall(spec.write, loc.object, state)
  if ok then return end
  pcall(spec.write, loc.object, was)
  local why = type(err) == "table" and err.message or tostring(err)
  fail("failed", string.format("Renoise refused %s's %s as asked (%s): nothing was changed", loc.name, spec.noun, (tostring(why):gsub("^[^%s:]*%.lua:%d+: ", ""))))
end

function M.prop_apply(song, env, spec, input, fence, desc)
  local ok, loc = pcall(spec.locate, song, env, input)
  if not ok then gone_is_changed(loc) end
  if loc.id ~= nil and fence.id ~= nil and fence.id ~= loc.id then
    fail("changed", M.moved_text(fence.label or loc.name))
  end
  local fenced = unwire(spec, fence, env, loc)
  local now = spec.read(loc.object, fenced)
  if not spec.same(now, fenced) then
    fail("changed", loc.name .. "'s " .. spec.noun .. " changed since the preview")
  end
  local wanted = wanted_of(spec, loc, input)
  -- (a value that is a place: what it named at the preview, where that is now, is what is written, or nothing)
  if type(fence.wanted) == "table" and not spec.same(unwire(spec, fence.wanted, env, loc), wanted) then
    fail("changed", string.format("what %s's %s names moved since the preview: read again", loc.name, spec.noun))
  end
  if spec.undoable then song:describe_undo(desc) end
  if spec.watch then spec.watch(env, loc) end
  if not spec.same(now, wanted) then write_whole(spec, loc, wanted, now) end
  -- (a setting Renoise applies a moment later answers its old value when read at once: `eventual` trusts the write)
  local after = spec.eventual and wanted or spec.read(loc.object, wanted)
  if not spec.same(after, wanted) then
    -- (Renoise took something else: what was there goes back, so nothing half-done stays)
    pcall(spec.write, loc.object, now)
    fail("failed", "Renoise kept " .. loc.name .. "'s " .. spec.noun .. " as it was")
  end
  local display = { title = spec.title(loc.name, now, after), before = now, after = after }
  if not spec.undoable then display.note = NOT_UNDOABLE end
  add_shown(spec, loc, display)
  -- (`restore_by_id`: what the undo writes into must be what was set, discard or not: a plug-in's parameter after another plug-in
  -- was loaded is another one)
  local applied = wire(spec, after, env, loc)
  if spec.restore_by_id then applied = with_id(applied, loc.id, loc.name) end
  -- (`keep(loc, before, after)`: what else the undo needs to know of the place as the change left it)
  if spec.keep then
    local copy = {}
    for k, v in pairs(applied) do copy[k] = v end
    for k, v in pairs(spec.keep(loc, now, after) or {}) do copy[k] = v end
    applied = copy
  end
  return {
    applied = applied, journal_applied = after,
    display = display,
    undo = { desc = desc, entry = spec.undoable },
    spec = spec, place = loc.place, prior = now, target = loc.target,
    -- (M4 n-song: where a kind keeps its object by the place mechanism rather than by its ref, for the restore to find it)
    material = spec.material and spec.material(env, loc) or nil,
  }
end

function M.prop_restore(song, env, spec, restore, desc)
  local reference = restore.target and restore.target.ref
  -- (a kind whose object takes more than its target's ref to find, a track's note column, finds it from the record: `restore_input`;
  -- a kind found by its place, not its ref, gets the place it kept: n-instruments)
  local input = spec.restore_input and spec.restore_input(restore)
    or { ref = reference, place = spec.by_place and restore.target and restore.target.place or nil }
  local ok, loc = pcall(spec.locate, song, env, input)
  if not ok then
    if type(loc) == "table" and loc.code == "failed" then loc.code = "gone" end
    error(loc, 0)
  end
  if spec.restore_by_id and type(restore.applied) == "table" and restore.applied.id ~= nil and restore.applied.id ~= loc.id then
    fail("changed", string.format("%s isn't what Kumi set any more (another plug-in or device is there): Kumi won't write into it", loc.name))
  end
  local prior, applied = unwire(spec, restore.prior, env, loc), unwire(spec, restore.applied, env, loc)
  local now = spec.read(loc.object, prior)
  if not spec.same(now, prior) then
    if not restore.discard and not spec.same(now, applied) then
      fail("changed", loc.name .. "'s " .. spec.noun .. " changed in Renoise since Kumi set it")
    end
    -- (`before_restore`: what else must hold for the old value to go back, refusing when it doesn't)
    if spec.before_restore and not restore.discard then spec.before_restore(loc, now, prior, restore.applied) end
    if spec.undoable then song:describe_undo(desc) end
    if spec.watch then spec.watch(env, loc) end
    write_whole(spec, loc, prior, now)
    local after = spec.eventual and prior or spec.read(loc.object, prior)
    if not spec.same(after, prior) then fail("failed", "Renoise kept " .. loc.name .. "'s " .. spec.noun .. " as it was") end
  end
  local after = spec.eventual and prior or spec.read(loc.object, prior)
  local display = { title = spec.title(loc.name, now, after) .. " (put back)", before = now, after = after }
  return {
    restored = wire(spec, after, env, loc), display = display, undo = { desc = desc, entry = spec.undoable }, place = loc.place,
    applied = applied, unwritten = spec.same(now, prior) or nil,
  }
end

-- Where slot `s` of the sequence `old` (a list of pattern numbers) is in `new`, when one block of slots made or deleted turns
-- one into the other and every such block puts it at the same place; nil when none does, or two put it at different places
-- (patterns repeat: a block of A, B made at the front of A B A B is also one made after the first two). With `gap`, `s` is
-- the place between slots s-1 and s, where a deleted slot goes back. Renoise's sequence notifier can't say which slot moved
-- where (measured), so the sequence itself is all there is to go by.
function M.follow_slot(old, new, s, gap)
  local n, m = #old, #new
  if n == m then
    for k = 1, n do
      if old[k] ~= new[k] then return nil end
    end
    return s
  end
  local front, back = 0, 0
  while front < n and front < m and old[front + 1] == new[front + 1] do front = front + 1 end
  while back < n and back < m and old[n - back] == new[m - back] do back = back + 1 end
  local found
  local function agree(at)
    if at == nil or (found ~= nil and found ~= at) then return false end
    found = at
    return true
  end
  local k = math.abs(m - n)
  if m > n then
    -- a block made at i: old[1..i-1] before it, old[i..n] after it
    for i = math.max(1, n + 1 - back), math.min(front + 1, n + 1) do
      local at
      if gap then at = (i < s and s + k) or (i > s and s) or nil else at = i <= s and s + k or s end
      if not agree(at) then return nil end
    end
  else
    -- the block old[i..i+k-1] deleted: old[1..i-1] before it, old[i+k..n] after it
    for i = math.max(1, n - k + 1 - back), math.min(front + 1, n - k + 1) do
      local at
      if gap then
        at = (i + k - 1 < s and s - k) or (i >= s and s) or nil
      else
        at = (s < i and s) or (s >= i + k and s - k) or nil
      end
      if not agree(at) then return nil end
    end
  end
  return found
end

-- The journal: the changes this tool made that a producer's undo could take back. An entry: {desc, kind, place (what finds the
-- object again: {track}, {track, device}…), spec (with `reread(song, place)` and `read`, to look at it again), prior, applied,
-- state ("applied" | "undone"), shape? ("deleted": the change removed an object; "made": it made one)}.

function M.journal_new()
  return { entries = {}, events = {} }
end

function M.journal_reset(journal)
  journal.entries, journal.events = {}, {}
end

function M.remember(journal, entry)
  entry.edits_made = journal.edits and journal.edits() or nil -- (#393: `redo_sure`)
  local entries = journal.entries
  entries[#entries + 1] = entry
  while #entries > M.JOURNAL_MAX do table.remove(entries, 1) end
end

local function queue(journal, desc, how)
  local events = journal.events
  local last = events[#events]
  if last and last.how == how then
    last.descs[#last.descs + 1] = desc
  else
    events[#events + 1] = { how = how, descs = { desc } }
  end
end

-- A change that was undone can be redone only until something else is edited (Renoise clears its redo steps then): the journal
-- is told how many edits the notifiers counted (`journal.edits`), and a change remembers the count when it was undone.
local function stamp(journal, entry)
  entry.edits_at = journal.edits and journal.edits() or nil
end

local function redoable(journal, entry)
  if entry.state ~= "undone" then return true end
  if entry.edits_at == nil or not journal.edits then return false end -- (undone before the count was kept: not known to be redoable)
  return journal.edits() <= entry.edits_at
end

-- (accept round 2) The change whose step is `desc`, when Renoise's redo put it back (told for certain) and nothing has moved it
-- since: the journal still holds it, applied. Its restore may then find what it made where the change left it, though Renoise's
-- undo and redo moved the epoch or the logs (each restore says what that allows, and checks what is there).
function M.redone_entry(journal, desc)
  if type(desc) ~= "string" or not journal then return nil end
  for i = #journal.entries, 1, -1 do
    local entry = journal.entries[i]
    if entry.desc == desc then return entry.state == "applied" and entry.redone and entry or nil end
  end
  return nil
end

-- The events to tell Kumi of, once. (With none, the empty list itself, which the caller only reads: most ticks have none.)
function M.journal_take(journal)
  local events = journal.events
  if events[1] == nil then return events end
  journal.events = {}
  return events
end

function M.journal_active(journal)
  return #journal.entries > 0
end

-- An object that the change removed comes back (the producer's undo) or goes again (redo); one that the change made goes (undo)
-- or comes back (redo). `type_` is what happened ("insert" or "remove") to the object at `at`.
-- Renoise's redo makes again exactly what its undo of the change removed, and the list's place log sees another object come:
-- with `linker` (the log the operation was just noted in: {mark = fn(), back = fn(mark, at)}), the redo's insert is linked back
-- to the undo's remove, so the places kept before (Kumi's own record of the change among them) follow it again. What the
-- object holds is each restore's to check, as ever.
local function structural(journal, entry, type_, at, undoing, where, linker)
  local returns = (entry.shape == "deleted") == (type_ == "insert")
  if undoing and where == at then
    if returns and entry.state == "applied" then
      entry.state = "undone"
      stamp(journal, entry)
      entry.redone = nil
      queue(journal, entry.desc, "undo")
      entry.gone_mark = linker and entry.shape == "made" and linker.mark() or nil
      return true
    elseif not returns and entry.state == "undone" and redoable(journal, entry) then
      entry.state = "applied"
      entry.redone = true
      queue(journal, entry.desc, "redo")
      if linker and entry.gone_mark then linker.back(entry.gone_mark, at) end
      entry.gone_mark = nil
      return true
    end
  end
  return false
end

-- A track made or deleted at `index`: the places the journal knows move with them, and the changes that removed or made a
-- track are told when the producer's undo or redo does it again.
-- (A change that moved a track keeps the two places it moved between, and they move with the tracks made and deleted.)
local function shift_places(entry, type_, index)
  if entry.follows ~= "track" then return end
  for _, state in ipairs({ entry.prior, entry.applied }) do
    if type(state) == "table" and type(state.index) == "number" then
      if type_ == "insert" then
        if state.index >= index then state.index = state.index + 1 end
      elseif state.index > index then
        state.index = state.index - 1
      end
    end
  end
end

function M.journal_track_event(journal, type_, index, undoing, index2, linker)
  if type_ == "swap" then
    -- (two tracks changed places: what is on them goes with them; a deleted track's place is where it will come back)
    for _, entry in ipairs(journal.entries) do
      local place = entry.place
      if place and place.track and entry.shape ~= "deleted" then
        if place.track == index then place.track = index2 elseif place.track == index2 then place.track = index end
      end
    end
    return
  end
  for i = #journal.entries, 1, -1 do
    local entry = journal.entries[i]
    local place = entry.place
    shift_places(entry, type_, index)
    if place and place.track then
      if entry.shape and place.device == nil then
        -- (an object that is a track itself: undone when it is made or removed again at its place)
        if not structural(journal, entry, type_, index, undoing, place.track, linker) then
          if type_ == "insert" then
            if place.track >= index then place.track = place.track + 1 end
          elseif place.track > index then
            place.track = place.track - 1
          end
        end
      elseif type_ == "insert" then
        if place.track >= index then place.track = place.track + 1 end
      elseif place.track == index then
        table.remove(journal.entries, i) -- (the track is gone: nothing left to compare)
      elseif place.track > index then
        place.track = place.track - 1
      end
    end
  end
end

-- A device made, deleted or swapped on a track: the places of the devices the journal knows move with it, and a change that
-- made or removed a device is told when the producer's undo or redo does it again.
function M.journal_device_event(journal, track, type_, index, index2, undoing, linker)
  for i = #journal.entries, 1, -1 do
    local entry = journal.entries[i]
    local place = entry.place
    if place and place.device and place.track == track then
      local handled = false
      if entry.shape and type_ ~= "swap" then handled = structural(journal, entry, type_, index, undoing, place.device, linker) end
      if not handled then
        if type_ == "insert" then
          if place.device >= index then place.device = place.device + 1 end
        elseif type_ == "remove" then
          if place.device == index and not entry.shape then
            table.remove(journal.entries, i) -- (the device is gone: nothing left to compare)
          elseif place.device > index then
            place.device = place.device - 1
          end
        elseif type_ == "swap" then
          if place.device == index then place.device = index2 elseif place.device == index2 then place.device = index end
        end
      end
    end
  end
end

-- A slot made or deleted in the sequence: Renoise's list operations name the wrong slot when patterns repeat (measured: an
-- insert at slot 1 is told as an insert at slot 2), so the slots the journal knows can't be followed; what is known of a slot
-- is dropped. A change that made or removed a slot is told by the length of the sequence instead (`follows = "slots"`); that
-- holds only while nothing else changed the length, so a change of the sequence that is no undo or redo drops those too.
function M.journal_sequence_event(journal, undoing)
  for i = #journal.entries, 1, -1 do
    local entry = journal.entries[i]
    if entry.place and entry.place.seq and (entry.follows ~= "slots" or not undoing) then table.remove(journal.entries, i) end
    -- (M4 n-song: a change told by the whole sequence holds only while nothing but Renoise's undo and redo changes it)
    if entry.follows == "sequence" and not undoing then table.remove(journal.entries, i) end
  end
end

-- M4 (n-song): the pattern numbers of the sequences a change keeps, moved with a pattern made or deleted in the pool. A pattern
-- deleted becomes a mark (so the sequence it was in matches nothing now), kept on a stack: Renoise's redo makes its patterns again
-- in the reverse order its undo deleted them, so a pattern made where the newest mark was is that pattern again.
local function shift_sequences(entry, type_, index)
  local lists = {}
  for _, state in ipairs({ entry.prior, entry.applied }) do
    if type(state) == "table" and type(state.sequence) == "table" then lists[#lists + 1] = state.sequence end
  end
  entry.gone = entry.gone or {}
  local stack = entry.gone
  if type_ == "remove" then
    entry.marks = (entry.marks or 0) + 1
    local mark = "gone" .. entry.marks
    for _, list in ipairs(lists) do
      for k, p in ipairs(list) do
        if p == index then list[k] = mark elseif type(p) == "number" and p > index then list[k] = p - 1 end
      end
    end
    for _, item in ipairs(stack) do
      if item.index > index then item.index = item.index - 1 end
    end
    stack[#stack + 1] = { index = index, mark = mark }
  elseif type_ == "insert" then
    local top = stack[#stack]
    local revive = top and top.index == index and top.mark or nil
    if revive then stack[#stack] = nil end
    for _, list in ipairs(lists) do
      for k, p in ipairs(list) do
        if type(p) == "number" and p >= index then list[k] = p + 1 elseif revive and p == revive then list[k] = index end
      end
    end
    for _, item in ipairs(stack) do
      if item.index >= index then item.index = item.index + 1 end
    end
  end
end

function M.journal_pattern_event(journal, type_, index)
  for i = #journal.entries, 1, -1 do
    local place = journal.entries[i].place
    if journal.entries[i].follows == "sequence" then
      shift_sequences(journal.entries[i], type_, index)
    elseif journal.entries[i].uses_patterns then
      table.remove(journal.entries, i) -- (it holds pattern numbers, which the pool's change moved)
    elseif place and place.pattern then
      if type_ == "insert" then
        if place.pattern >= index then place.pattern = place.pattern + 1 end
      elseif type_ == "remove" then
        if place.pattern == index then
          table.remove(journal.entries, i) -- (the pattern is gone: nothing left to compare)
        elseif place.pattern > index then
          place.pattern = place.pattern - 1
        end
      end
    end
  end
end

-- The changes that made or deleted a slot are told by the length of the sequence (Renoise's list operations name the wrong
-- slot). That says little when there are several: a change is told only when it is the one length fits.
local function check_slots(journal, song)
  local now = #song.sequencer.pattern_sequence
  local candidates = {}
  for _, entry in ipairs(journal.entries) do
    if entry.follows == "slots" then
      if entry.state == "applied" and now == entry.prior.slots and now ~= entry.applied.slots then
        candidates[#candidates + 1] = { entry = entry, how = "undo", state = "undone" }
      elseif entry.state == "undone" and now == entry.applied.slots and redoable(journal, entry) then
        candidates[#candidates + 1] = { entry = entry, how = "redo", state = "applied" }
      end
    end
  end
  if #candidates == 1 then
    candidates[1].entry.state = candidates[1].state
    if candidates[1].state == "undone" then stamp(journal, candidates[1].entry) end
    candidates[1].entry.redone = candidates[1].how == "redo" or nil
    candidates[1].entry.redo_patterns = #song.patterns
    queue(journal, candidates[1].entry.desc, candidates[1].how)
  end
end

-- After a producer's undo or redo: the journalled properties are read again and compared with what Kumi wrote and what it
-- replaced.
local check_read

function M.journal_check(journal)
  local song = renoise.song()
  if not song then return end
  local sequence
  for _, entry in ipairs(journal.entries) do
    local spec = entry.spec
    if entry.follows == "slots" then
      -- (decided below, over all of them)
    elseif entry.follows == "sequence" then
      -- (M4 n-song: the whole sequence is the one before Kumi's change, or the one it left)
      if not sequence then
        sequence = {}
        for k, p in ipairs(song.sequencer.pattern_sequence) do sequence[k] = p end
      end
      local same = M.same_value
      if entry.state == "applied" and same(sequence, entry.prior.sequence) and not same(sequence, entry.applied.sequence) then
        entry.state = "undone"
        stamp(journal, entry)
        entry.redone = nil
        queue(journal, entry.desc, "undo")
      elseif entry.state == "undone" and same(sequence, entry.applied.sequence) and redoable(journal, entry) then
        entry.state = "applied"
        entry.redone, entry.redo_patterns = true, #song.patterns
        -- (the patterns it made are made again: their counts of changed lines start from here)
        if entry.spec and entry.spec.redo_marks and journal.retouched_at then
          for _, made in ipairs(entry.spec.redo_marks(entry)) do journal.retouched_at(made.mark, made.p) end
        end
        queue(journal, entry.desc, "redo")
      end
    elseif entry.follows then
      -- (a device or track moved: its place is followed through the swaps, and is where it started or where Kumi put it)
      local now = entry.place[entry.follows]
      if entry.state == "applied" and now == entry.prior.index and now ~= entry.applied.index then
        entry.state = "undone"
        stamp(journal, entry)
        entry.redone = nil
        queue(journal, entry.desc, "undo")
      elseif entry.state == "undone" and now == entry.applied.index and redoable(journal, entry) then
        entry.state = "applied"
        entry.redone = true
        queue(journal, entry.desc, "redo")
      end
    elseif spec and spec.reread and not entry.shape then
      check_read(journal, song, entry)
    end
  end
  check_slots(journal, song)
end

-- (#393) Whether Renoise's last undo or redo was its redo of this change, for sure. Kumi's step was the newest when Renoise's
-- undo took it: nothing was edited from the change to the undo, or that undo may have been of a later step of the producer's
-- that left the lines as before the change, and its redo the producer's own lines. And nothing is left to redo: an undo always
-- leaves the step it took back to redo, so an undo past Kumi's step would have left Kumi's step there.
local function redo_sure(journal, entry, song)
  if entry.edits_made == nil or entry.edits_made ~= entry.edits_at then return false end
  local ok, sure = pcall(function() return song:can_undo() and not song:can_redo() end)
  return ok and sure == true
end

-- An entry the journal reads again (its spec's `reread`, `read`, `same`): undone when it reads as before the change and not as
-- the change left it, redone when it reads as the change left it again.
check_read = function(journal, song, entry)
  local spec = entry.spec
  local ok, now = pcall(function()
    local object = spec.reread(song, entry.place)
    if object == nil then return nil end
    return spec.read(object, entry.applied)
  end)
  if ok and now ~= nil then
    if (entry.state == "applied" or entry.state == "unsure") and spec.same(now, entry.prior) and not spec.same(now, entry.applied) then
      entry.state = "undone"
      stamp(journal, entry)
      entry.redone = nil
      queue(journal, entry.desc, "undo")
    elseif entry.state == "undone" and spec.same(now, entry.applied) and redoable(journal, entry) then
      -- (what Kumi wrote is there again, from Renoise's redo: the lines' count of edits since Kumi's step starts again here,
      -- so Kumi's undo isn't refused for the redo's own writes; an edit after it still is. Not where the redo is unsure:
      -- lines the producer had or typed read the same as Kumi's, review MUST 10)
      local unsure = spec.unsure_redo and not redo_sure(journal, entry, song)
      if journal.retouched and not unsure and type(entry.applied) == "table" then journal.retouched(entry.applied) end
      if unsure then
        -- (Renoise doesn't say which step its undo or redo took: lines that read as Kumi wrote them again may be its redo
        -- of Kumi's step, its undo past it of an older row insert that put the producer's like lines there, or its redo of
        -- the producer's own later lines)
        entry.state = "unsure"
        queue(journal, entry.desc, "unsure")
      else
        entry.state = "applied"
        entry.redone = true
        queue(journal, entry.desc, "redo")
      end
    end
  end
end

-- (#361 N-S16) The entries of one device, checked as `check_read` checks each, with one read of the device: each through its
-- own spec, but for `read`, which gives them all the first entry's read.
local function read_device(journal, song, entries)
  local read = entries[1].spec.read
  local done, value = false, nil
  local function once(object, applied)
    if not done then done, value = true, read(object, applied) end
    return value
  end
  for _, entry in ipairs(entries) do
    local spec = entry.spec
    entry.spec = setmetatable({ read = once }, { __index = spec })
    local ok, err = pcall(check_read, journal, song, entry)
    entry.spec = spec
    if not ok then error(err, 0) end
  end
end

-- (accept round 2) What Renoise tells no notifier of when its undo or redo changes it (what a Doofer, Splitter or Notepad holds:
-- `spec.polled`) is read again now and then instead (the tool's timer, about once a second): the newest `most` such entries.
-- (#361 N-S16) A device is read once for all its entries, and a call reads at most `each` devices, going on from where the
-- last call stopped, so a round of reads is spread over the tool's ticks. True when the round is done. (Entries that come
-- or go between calls are taken as the list is then: a device a round misses, the next one reads.)
function M.journal_poll(journal, most, each)
  local song = renoise.song()
  if not song then
    journal.poll_next = nil
    return true
  end
  local devices, by_read, count = {}, {}, 0
  for i = #journal.entries, 1, -1 do
    local entry = journal.entries[i]
    if count >= (most or 8) then break end
    local spec = entry.spec
    if spec and spec.polled and spec.reread and not entry.shape and (entry.state == "applied" or entry.state == "undone") then
      count = count + 1
      -- (entries at one place that read alike read one device: a polled kind's `read` takes only the device)
      local place = entry.place
      local key = entry
      if type(place) == "table" and type(place.track) == "number" and type(place.device) == "number" then key = place.track .. "." .. place.device end
      local of = by_read[spec.read] or {}
      by_read[spec.read] = of
      if not of[key] then
        of[key] = {}
        devices[#devices + 1] = of[key]
      end
      table.insert(of[key], entry)
    end
  end
  local first = journal.poll_next or 1
  local last = math.min(#devices, first + (each or 2) - 1)
  for k = first, last do read_device(journal, song, devices[k]) end
  journal.poll_next = last < #devices and last + 1 or nil
  return journal.poll_next == nil
end

-- n-instruments: an operation on a list of instruments, or of what is in one (its samples, phrases, sets, chains, a set's or a
-- chain's devices): the journal's places of that key (`key`) in the instrument, set or chain whose list it is (`scope`) move with
-- it, and a change that made or removed one of them (`place.of` names what a change's place is of) is told when the producer's
-- undo or redo does it again. An instrument removed takes everything in it with it.
local function in_scope(place, scope)
  for k, v in pairs(scope) do
    if place[k] ~= v then return false end
  end
  return true
end

function M.journal_list_event(journal, key, scope, type_, index, index2, undoing, linker)
  for i = #journal.entries, 1, -1 do
    local entry = journal.entries[i]
    local place = entry.place
    if type(place) == "table" and type(place[key]) == "number" and in_scope(place, scope) then
      local own = place.of == key
      local handled = false
      if own and entry.shape and type_ ~= "swap" then handled = structural(journal, entry, type_, index, undoing, place[key], linker) end
      if not handled then
        if type_ == "insert" then
          if place[key] >= index then place[key] = place[key] + 1 end
        elseif type_ == "remove" then
          if place[key] == index and not (own and entry.shape) then
            table.remove(journal.entries, i) -- (it is gone: nothing left to compare)
          elseif place[key] > index then
            place[key] = place[key] - 1
          end
        elseif type_ == "swap" then
          if place[key] == index then place[key] = index2 elseif place[key] == index2 then place[key] = index end
        end
      end
    end
  end
end

-- n-instruments: Kumi put back a change that made or removed something itself, found by its place (the journal followed it).
-- The journal entry of the change Kumi is putting back: the one the restore names (`journal.restoring`, its desc), else the
-- newest of the kind that `matches`. (Two changes of one kind and place are told apart by their desc, not by the first found.)
local function restored_entry(journal, kind, matches)
  local function open(entry) return entry.kind == kind and (entry.state == "applied" or entry.state == "unsure") end
  if type(journal.restoring) == "string" then
    for i = #journal.entries, 1, -1 do
      local entry = journal.entries[i]
      if entry.desc == journal.restoring and open(entry) then return entry end
    end
  end
  for i = #journal.entries, 1, -1 do
    local entry = journal.entries[i]
    if open(entry) and matches(entry) then return entry end
  end
  return nil
end

function M.note_restored_at(journal, kind, place)
  local entry = restored_entry(journal, kind, function(e) return M.same_table(e.place, place) end)
  if not entry then return false end
  entry.state, entry.redone = "undone", nil
  stamp(journal, entry)
  return true
end

-- Kumi put a change back itself: the change is not the producer's to undo any more (the producer's undo of that restore then
-- brings it back, which is told as a redo).
function M.note_restored(journal, kind, place, applied, spec)
  local entry = restored_entry(journal, kind, function(e)
    return e.shape or (M.same_table(e.place, place) and spec and spec.same(e.applied, applied))
  end)
  if not entry then return end
  entry.state, entry.redone = "undone", nil
  stamp(journal, entry)
  if entry.shape and place then entry.place = place end
end

-- M4 (n-song): a kind of several settings of one object (the song's groove, a track's columns), any of which a change gives:
-- only those are read, fenced, written and put back, so a setting of the same kind the producer changes meanwhile is left to
-- them. `fields`: {name, read(object), write(object, value), check(value, name, object), text(value)}, in the order a title
-- lists them; the title opens with `heading`, or with the located object's name when `named`.
function M.same_value(a, b)
  if type(a) == "table" and type(b) == "table" then
    for k, v in pairs(a) do
      if not M.same_value(v, b[k]) then return false end
    end
    for k in pairs(b) do
      if a[k] == nil then return false end
    end
    return true
  end
  return close(a, b)
end

function M.flag(value, name)
  if type(value) ~= "boolean" then bad(name .. " must be true or false") end
  return value
end

function M.text_of(value, name, most)
  if type(value) ~= "string" or #value > most or value:find("[%c]") then bad(string.format("%s must be one line of text, at most %d bytes", name, most)) end
  return value
end

function M.settings(spec)
  local fields, names = spec.fields, {}
  for i, field in ipairs(fields) do names[i] = field.name end
  spec.read = function(object, source)
    local state = {}
    for _, field in ipairs(fields) do
      if type(source) == "table" and source[field.name] ~= nil then state[field.name] = field.read(object) end
    end
    return state
  end
  spec.wanted = function(object, input)
    local state, any = {}, false
    for _, field in ipairs(fields) do
      if input[field.name] ~= nil then
        state[field.name] = field.check(input[field.name], field.name, object)
        any = true
      end
    end
    if not any then bad("give any of " .. table.concat(names, ", ")) end
    return state
  end
  spec.write = function(object, state)
    for _, field in ipairs(fields) do
      local value = state[field.name]
      if value ~= nil and not M.same_value(field.read(object), value) then field.write(object, value) end
    end
  end
  spec.same = function(a, b)
    for _, field in ipairs(fields) do
      local x, y = a[field.name], b[field.name]
      if (x == nil) ~= (y == nil) or (x ~= nil and not M.same_value(x, y)) then return false end
    end
    return true
  end
  spec.title = function(name, _, to)
    local parts = {}
    for _, field in ipairs(fields) do
      if to[field.name] ~= nil then parts[#parts + 1] = field.text(to[field.name]) end
    end
    local text = table.concat(parts, ", ")
    local heading = spec.named and name or spec.heading
    if heading then return heading .. " " .. text end
    return (text:gsub("^%l", string.upper))
  end
  return spec
end

-- M4 (n-song): a pattern as a place (the pool's log, places.lua): a record of it outlives patterns made or deleted elsewhere in the
-- pool, which a pattern ref doesn't. Where it is now, or a refusal that touches nothing.
function M.pattern_place(env, p)
  return { n = p, mark = env.pattern_mark and env.pattern_mark() or nil }
end

function M.pattern_now(song, env, place)
  if type(place) ~= "table" or not env.pattern_at then bad("material: where the pattern was") end
  local p, why = env.pattern_at(place.n, place.mark)
  if not p then
    if why == "gone" then fail("gone", "that pattern was deleted in Renoise since") end
    fail("stale", "the song's patterns changed in a way Kumi couldn't follow since (the tool watched the song anew): read again")
  end
  if p > #song.patterns then fail("gone", "that pattern isn't in the song any more") end
  return p
end

-- The count of changed lines of each pattern in `patterns` (a set of numbers), as a record keeps it beside cells it writes back
-- later by line (false when the tool can't count a pattern's lines).
function M.touch_marks(env, patterns)
  if not env.pattern_touch_mark then return nil end
  local marks = {}
  for p in pairs(patterns) do marks[tostring(p)] = env.pattern_touch_mark(p) or false end
  return marks
end

-- The patterns (by their numbers then) whose lines changed since `marks` were kept, or can't be told: rows inserted or deleted
-- there move every line, so cells kept by line have no place Kumi can tell. `pattern_now` follows a number then to now. Nothing
-- is touched for a record that kept no marks.
function M.touched_patterns(env, marks, pattern_now)
  local touched = {}
  if type(marks) ~= "table" then return touched end
  for key, mark in pairs(marks) do
    local p = tonumber(key)
    if p then
      local now = pattern_now and pattern_now(p) or nil
      -- (a pattern whose lines the tool couldn't count then, `false`, is one Kumi can't tell about either)
      local since = now and mark and env.pattern_touched_since and env.pattern_touched_since(now, mark)
      if since ~= false or not mark then touched[p] = true end
    end
  end
  return touched
end

return M
