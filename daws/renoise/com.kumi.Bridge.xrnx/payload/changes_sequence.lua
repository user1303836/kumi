-- M4 (n-song): Kumi's changes to the order of the pattern sequence: cloning slots (clone_range), moving a slot, making slots'
-- patterns unique (make_range_unique).
--
-- Measured in Renoise 3.5.4 (lab, 2026-10-19): `clone_range(a, b)` puts copies of slots a..b right after b, each with a NEW
-- pattern (a copy named "#" + the name, at the end of the pool), the slots' mutes and section starts copied (a section's name
-- with "#" in front); `make_range_unique(a, b)` gives every slot in the range whose pattern another slot plays a copy of it (the
-- first such slot too: [1, 2, 1] becomes [3, 2, 4]); a slot made in the middle takes the section start and mutes of the slot it
-- pushes up, and deleting a slot that starts a section passes the start to the next one (M3). A pattern is removed from the pool
-- when a slot lets go of it while it is the pool's last, empty and played by no other slot.
--
-- Slots have no identity, and Renoise's sequence notifier names the wrong slot when patterns repeat (measured), so these records
-- find their slots by the sequence they left (`applied.sequence`, pattern numbers read through the pool's log since: places.lua)
-- and base.follow_slot: a sequence changed since by one block of slots made or deleted elsewhere is followed; anything else is
-- `stale`, with nothing touched. Their target is the song (a sequence ref would go stale with any sequence change), and a restore
-- drops a record's slot refs (bridge.lua).

local import = ...
local refs = import("refs")
local rows = import("rows")
local base = import("changes_base")
local patterns_ = import("changes_patterns")
local notes_ = import("changes_notes")
local material_ = import("changes_material")

local bad, fail = base.bad, base.fail
local q = base.q
local M = {}

-- The kinds whose records find their slots this way (bridge.lua drops the slot refs of their restores).
M.KINDS = { ["slot-clone"] = true, ["slot-move"] = true, ["slot-unique"] = true, ["slot-variation"] = true, ["slots-drop"] = true }

local function slot_count(song) return #song.sequencer.pattern_sequence end

local function sequence_of(song)
  local out = {}
  for s, p in ipairs(song.sequencer.pattern_sequence) do out[s] = p end
  return out
end

local function same_list(a, b)
  if type(a) ~= "table" or type(b) ~= "table" or #a ~= #b then return false end
  for i = 1, #a do
    if a[i] ~= b[i] then return false end
  end
  return true
end

local function seq_of(song, env, reference, what)
  if type(reference) ~= "string" then bad(what .. ": a sequence slot ref") end
  local s = refs.expect(reference, "seq", 1, env.epoch())[1]
  if s < 1 or s > slot_count(song) then fail("gone", "that sequence slot isn't in the song any more") end
  return s
end

local function song_target(env) return { ref = refs.make(env.epoch(), "song", 1), kind = "song" } end

-- A slot's own state: its section start and name, and the tracks muted in it.
local function state_of(song, s)
  local sequencer = song.sequencer
  local start = sequencer:sequence_is_start_of_section(s)
  local mutes = {}
  for t = 1, rows.track_count(song) do
    if sequencer:track_sequence_slot_is_muted(t, s) then mutes[#mutes + 1] = t end
  end
  return { start = start, name = start and sequencer:sequence_section_name(s) or "", mutes = mutes }
end

-- The name of the section slot `s` is in (the nearest start at or before it), "" before any.
local function section_at(song, s)
  local sequencer = song.sequencer
  for at = s, 1, -1 do
    if sequencer:sequence_is_start_of_section(at) then return sequencer:sequence_section_name(at) end
  end
  return ""
end

local function set_state(song, s, state, sections_only)
  local sequencer = song.sequencer
  if sequencer:sequence_is_start_of_section(s) ~= state.start then sequencer:set_sequence_is_start_of_section(s, state.start) end
  if state.start and sequencer:sequence_section_name(s) ~= state.name then sequencer:set_sequence_section_name(s, state.name) end
  if sections_only then return end
  local muted = {}
  for _, t in ipairs(state.mutes) do muted[t] = true end
  for t = 1, rows.track_count(song) do
    if sequencer:track_sequence_slot_is_muted(t, s) ~= (muted[t] == true) then sequencer:set_track_sequence_slot_is_muted(t, s, muted[t] == true) end
  end
end

local function same_state(a, b)
  return a.start == b.start and a.name == b.name and same_list(a.mutes, b.mutes)
end

-- The kept sequence's patterns where they are now (the pool's log since `mark`): a list with false for a pattern gone, or nil
-- when what happened to the pool can't be told.
local function kept_now(env, kept, mark)
  if type(kept) ~= "table" or not env.pattern_at then return nil end
  local out = {}
  for i, p in ipairs(kept) do
    local now, why = env.pattern_at(p, mark)
    if why == "unknown" then return nil end
    out[i] = now or false
  end
  return out
end

-- Once Renoise's redo put a change back (`restore.redone`: the journal held it, the sequence and the pool changed by nothing but
-- Renoise's undo and redo since, and the sequence is the one the change left), the journal's own copy of the sequences before and
-- after it, numbered as now. Nil otherwise.
local function redone_sequences(song, restore)
  local entry = restore.redone
  if not entry or entry.redo_patterns ~= #song.patterns or type(entry.applied) ~= "table" or type(entry.prior) ~= "table" then return nil end
  local now, after, before = sequence_of(song), entry.applied.sequence, entry.prior.sequence
  if type(after) ~= "table" or type(before) ~= "table" or not same_list(now, after) then return nil end
  for _, p in ipairs(before) do
    if type(p) ~= "number" then return nil end
  end
  return after, before
end

local function stale(what)
  fail("stale", "the sequence changed in Renoise since Kumi " .. what .. " in a way that doesn't say where its slots went: nothing was changed")
end

local function slots_text(a, b)
  if a == b then return "slot " .. a end
  return string.format("slots %d to %d", a, b)
end

-- What a pattern Kumi made holds, to tell whether anyone changed it since: its name, length, slot colours, aliases and
-- automation points (its lines are told by the pattern's count of changed lines, `touch`).
local function pattern_print(song, p)
  local pattern = song:pattern(p)
  local parts = { pattern.name, tostring(pattern.number_of_lines) }
  for t = 1, rows.track_count(song) do
    local pattern_track = pattern:track(t)
    local color = pattern_track.color
    if color then parts[#parts + 1] = string.format("c%d:%d,%d,%d", t, color[1], color[2], color[3]) end
    if pattern_track.alias_pattern_index > 0 then parts[#parts + 1] = string.format("a%d:%d", t, pattern_track.alias_pattern_index) end
    for k, automation in ipairs(pattern_track.automation) do
      local points = {}
      for i, point in ipairs(automation.points) do points[i] = string.format("%.9g/%.9g/%.9g", point.time, point.value, point.scaling) end
      parts[#parts + 1] = string.format("u%d:%d:%d:%s", t, k, automation.playmode, table.concat(points, ";"))
    end
  end
  return base.hash(table.concat(parts, "|"))
end

M.pattern_print = pattern_print

local function keep_pattern(song, env, p)
  return { print = pattern_print(song, p), touch = env.pattern_touch_mark and env.pattern_touch_mark(p) or nil }
end

-- Whether a pattern Kumi made is as it made it: nil, or why not.
local function pattern_changed(song, env, p, kept)
  if type(kept) ~= "table" then return "Kumi doesn't know what it held" end
  if pattern_print(song, p) ~= kept.print then return "it was changed" end
  if env.pattern_touched_since then
    local since = env.pattern_touched_since(p, kept.touch)
    if since == nil then return "Kumi can't tell whether its lines changed (the tool watched the song anew)" end
    if since then return "its lines changed" end
  end
  return nil
end

-- Whether another pattern's track plays pattern `p` as an alias of it (then a copy Kumi made stays: clearing it would silence
-- that track).
local function aliased(song, p)
  for q = 1, #song.patterns do
    if q ~= p then
      local pattern = song:pattern(q)
      for t = 1, rows.track_count(song) do
        if pattern:track(t).alias_pattern_index == p then return true end
      end
    end
  end
  return false
end

local function used_by_others(song, p, mine)
  for s, value in ipairs(song.sequencer.pattern_sequence) do
    if value == p and not mine[s] then return true end
  end
  return false
end

-- The journal follows these by the whole sequence (see changes_base.lua, `follows = "sequence"`).
-- (the host keeps the preview's prior: the sequence before is kept in `applied` too, for the restore)
local function journal_fields(prior_sequence, after_sequence)
  return { sequence = prior_sequence }, { sequence = after_sequence, before = prior_sequence }
end

-- slot-clone

local function clone_range_of(song, env, input)
  local a = seq_of(song, env, input.ref, "slot-clone")
  local b = input.last ~= nil and seq_of(song, env, input.last, "slot-clone") or a
  if b < a then bad("the last slot comes before the first") end
  return a, b
end

local function clone_title(song, a, b)
  if a == b then return string.format("Cloned slot %d (pattern %s)", a, q(patterns_.pattern_label(song, song.sequencer.pattern_sequence[a]))) end
  return "Cloned " .. slots_text(a, b)
end

local CLONE_NOTE = "Renoise copies each slot's pattern into a new one (\"#\" and its name), with the slot's mutes and section."

local function clone_preview(song, env, input)
  local a, b = clone_range_of(song, env, input)
  local n, range = slot_count(song), {}
  for s = a, b do range[#range + 1] = song.sequencer.pattern_sequence[s] end
  return {
    target = song_target(env), prior = { slots = n },
    fence = { slots = n, patterns = #song.patterns, range = range },
    display = { title = clone_title(song, a, b), before = { slots = n }, after = { slots = n + b - a + 1 }, note = CLONE_NOTE },
  }
end

local function clone_apply(song, env, input, fence, desc)
  local ok, a, b = pcall(clone_range_of, song, env, input)
  if not ok then base.gone_is_changed(a) end
  local before = sequence_of(song)
  local range = {}
  for s = a, b do range[#range + 1] = before[s] end
  if #before ~= fence.slots or #song.patterns ~= fence.patterns or not same_list(range, fence.range) then
    fail("changed", "the sequence changed since the preview")
  end
  local title = clone_title(song, a, b)
  local pool_before = #song.patterns
  song:describe_undo(desc)
  song.sequencer:clone_range(a, b)
  local after, k = sequence_of(song), b - a + 1
  local made = {}
  for i = 1, k do
    local s = b + i
    local p = after[s]
    made[i] = {
      state = state_of(song, s),
      -- (a pattern of its own when Renoise made one: past the pool's end before the clone)
      pattern = p > pool_before and keep_pattern(song, env, p) or nil,
    }
  end
  local prior, applied = journal_fields(before, after)
  applied.pool, applied.at, applied.count, applied.made = env.pattern_mark and env.pattern_mark() or nil, b + 1, k, made
  return {
    applied = applied, target = song_target(env),
    created = { ref = refs.make(env.epoch(), "seq", b + 1), kind = "seq" },
    display = { title = title, before = { slots = #before }, after = { slots = #after }, note = CLONE_NOTE },
    undo = { desc = desc, entry = true }, prior = prior, place = {}, follows = "sequence",
  }
end

-- The block of slots a record made, where it is now: its first slot, or nil.
local function block_now(kept, now, at, count)
  local first
  for i = 0, count - 1 do
    local s = base.follow_slot(kept, now, at + i)
    if not s or (first and s ~= first + i) then return nil end
    first = first or s
  end
  return first
end

-- The restore of a change that made a block of slots, each with a new pattern of its own or not (slot-clone, slot-variation):
-- the slots and patterns go while they are as the change left them. `words` = {what, title(applied), after(song, env, restore)}.
local function made_restore(kind, words)
  return function(song, env, restore, desc)
  local applied = restore.applied
  if type(applied) ~= "table" or type(applied.sequence) ~= "table" or type(applied.before) ~= "table" or type(applied.made) ~= "table" then
    bad("applied: the sequence before and after the change")
  end
  local now = sequence_of(song)
  local kept, before = redone_sequences(song, restore)
  if not kept then kept, before = kept_now(env, applied.sequence, applied.pool), kept_now(env, applied.before, applied.pool) end
  if not kept or not before then stale(words.what) end
  local title = words.title(applied)
  if same_list(now, before) then
    return { restored = { done = true }, display = { title = title }, undo = { desc = desc, entry = false }, unwritten = true }
  end
  local first = block_now(kept, now, applied.at, applied.count)
  if not first then stale(words.what) end
  local mine, count = {}, applied.count
  for i = 0, count - 1 do mine[first + i] = true end
  -- What the producer made of them since stays: Kumi takes back only slots and patterns as the clone left them.
  for i, made in ipairs(applied.made) do
    local s = first + i - 1
    local now_state = state_of(song, s)
    local same = words.mutes_only and same_list(now_state.mutes, made.state.mutes) or (not words.mutes_only and same_state(now_state, made.state))
    if not restore.discard and not same then
      fail("changed", string.format("slot %d's %s changed in Renoise since Kumi %s", s, words.mutes_only and "mutes" or "mutes or section", words.did))
    end
    -- (#401: a section that starts there under another name than the one the slot was made in is the producer's: deleting the
    -- slot would take its name, as the slot after gets its own state back)
    if words.mutes_only and not restore.discard and now_state.start and made.section ~= nil and now_state.name ~= made.section then
      fail("changed", string.format("slot %d starts the section %s in Renoise now, named since Kumi %s", s, q(now_state.name), words.did))
    end
    if made.pattern then
      local why = pattern_changed(song, env, now[s], made.pattern)
      if why then fail("changed", string.format("the pattern %s that slot %d plays: %s since Kumi made it", q(patterns_.pattern_label(song, now[s])), s, why)) end
    end
  end
  if slot_count(song) <= count then fail("changed", "the cloned slots are all the song has now") end
  song:describe_undo(desc)
  -- (the slot after the block: deleting a section's start passes it on, so it is put back as it is now)
  local after_block = first + count <= slot_count(song) and state_of(song, first + count) or nil
  local kept_patterns, played = 0, 0
  for i = count, 1, -1 do
    local s = first + i - 1
    local p = song.sequencer.pattern_sequence[s]
    local made = applied.made[i]
    if made.pattern and aliased(song, p) then
      played = played + 1
    elseif made.pattern and p == #song.patterns and not used_by_others(song, p, { [s] = true }) then
      -- (emptied first: Renoise then removes it from the pool as the slot lets go of it)
      song:pattern(p):clear()
    elseif made.pattern then
      kept_patterns = kept_patterns + 1
    end
    song.sequencer:delete_sequence_at(s)
  end
  if after_block then set_state(song, first, after_block, true) end
  if words.after then words.after(song, env, restore) end
  base.note_restored(env.journal, kind, {}, applied, base.KINDS[kind])
  local display = { title = title }
  if kept_patterns > 0 then
    display.note = string.format("%s Renoise made for %s in the pool, unused: patterns were made after %s.",
      kept_patterns == 1 and "The pattern" or (kept_patterns .. " patterns"), kept_patterns == 1 and "it stays" or "them stay",
      kept_patterns == 1 and "it" or "them")
  end
  if played > 0 then
    local said = string.format("%s Kumi made %s: another pattern's track plays %s as an alias.", played == 1 and "The copy" or (played .. " copies"),
      played == 1 and "stays" or "stay", played == 1 and "it" or "them")
    display.note = display.note and (display.note .. " " .. said) or said
  end
  return { restored = { done = true }, display = display, undo = { desc = desc, entry = true } }
  end
end

local clone_restore = made_restore("slot-clone", {
  what = "cloned slots", did = "cloned it",
  title = function(applied) return string.format("Removed the cloned %s (put back)", applied.count == 1 and "slot" or "slots") end,
})

-- slot-move: a slot goes to another place (`index`, its place once moved), with its own section start and mutes. Renoise has no
-- move: the slot is made again at its new place (insert_sequence_at), set as it was, and the old one deleted, with the slots
-- Renoise touches on the way put back as they were.

local function move_to(song, from, to)
  local sequencer = song.sequencer
  local p, own = sequencer.pattern_sequence[from], state_of(song, from)
  local insert_at = to > from and to + 1 or to
  local delete_at = to > from and from or from + 1
  local pushed = insert_at <= slot_count(song) and state_of(song, insert_at) or nil
  sequencer:insert_sequence_at(insert_at, p)
  set_state(song, insert_at, own)
  if pushed then set_state(song, insert_at + 1, pushed) end
  local next_slot = delete_at + 1 <= slot_count(song) and state_of(song, delete_at + 1) or nil
  sequencer:delete_sequence_at(delete_at)
  if next_slot then set_state(song, delete_at, next_slot, true) end
end

local function moved(list, from, to)
  local out = {}
  for i, p in ipairs(list) do out[i] = p end
  local p = table.remove(out, from)
  table.insert(out, to, p)
  return out
end

-- The states (section start and name, mutes) of slots `first` to `last`: what a move between slots that play one pattern
-- changes when the list of patterns stays the same.
local function states_of(song, first, last)
  local out = {}
  for s = first, math.min(last, slot_count(song)) do out[#out + 1] = state_of(song, s) end
  return out
end

local function same_states(a, b)
  if type(a) ~= "table" or type(b) ~= "table" or #a ~= #b then return false end
  for k = 1, #a do
    if not same_state(a[k], b[k]) then return false end
  end
  return true
end

local function move_plan(song, env, input)
  local s = seq_of(song, env, input.ref, "slot-move")
  local to = input.index
  local n = slot_count(song)
  if type(to) ~= "number" or to ~= math.floor(to) or to < 1 or to > n then bad(string.format("index is the slot's place once moved, from 1 to %d", n)) end
  return s, to
end

local function move_title(song, s, to, p)
  return string.format("Moved slot %d (pattern %s) to place %d", s, q(patterns_.pattern_label(song, p)), to)
end

local function move_preview(song, env, input)
  local s, to = move_plan(song, env, input)
  local sequence = sequence_of(song)
  local display = { title = move_title(song, s, to, sequence[s]), before = { index = s }, after = { index = to } }
  if s == to then display.unchanged = true end
  return { target = song_target(env), prior = { index = s }, fence = { sequence = sequence, state = state_of(song, s) }, display = display }
end

local function move_apply(song, env, input, fence, desc)
  local ok, s, to = pcall(move_plan, song, env, input)
  if not ok then base.gone_is_changed(s) end
  local before = sequence_of(song)
  if not same_list(before, fence.sequence) or not same_state(state_of(song, s), fence.state or {}) then
    fail("changed", "the sequence changed since the preview")
  end
  local display = { title = move_title(song, s, to, before[s]), before = { index = s }, after = { index = to } }
  if s == to then
    display.unchanged = true
    return { applied = { index = to }, display = display, undo = { desc = desc, entry = false }, prior = { index = s } }
  end
  local low, high = math.min(s, to), math.max(s, to)
  local states_before = states_of(song, low, high)
  song:describe_undo(desc)
  move_to(song, s, to)
  local after = sequence_of(song)
  if not same_list(after, moved(before, s, to)) then
    fail("failed", "Renoise didn't move the slot as asked (Renoise's own undo takes back what it did)")
  end
  local prior, applied = journal_fields(before, after)
  prior.index, applied.index, applied.from = s, to, s
  applied.pool = env.pattern_mark and env.pattern_mark() or nil
  applied.states = { low = low, before = states_before, after = states_of(song, low, high) }
  return {
    applied = applied, target = song_target(env), display = display, undo = { desc = desc, entry = true },
    prior = prior, place = {}, follows = "sequence",
  }
end

local function move_restore(song, env, restore, desc)
  local applied = restore.applied
  if type(applied) ~= "table" or type(applied.sequence) ~= "table" or type(applied.before) ~= "table" or type(applied.index) ~= "number" then
    bad("applied: the sequence before and after the move")
  end
  local now = sequence_of(song)
  local kept, before = kept_now(env, applied.sequence, applied.pool), kept_now(env, applied.before, applied.pool)
  if not kept or not before then stale("moved the slot") end
  if same_list(now, before) then
    -- (slots that play one pattern: the list alone can't tell the move from no move; their sections and mutes can)
    local states = type(applied.states) == "table" and applied.states or nil
    local now_states = states and states_of(song, states.low, states.low + #states.before - 1)
    if not states or not same_list(before, kept) or same_states(now_states, states.before) then
      return { restored = { index = applied.from }, display = { title = "The slot is back already" }, undo = { desc = desc, entry = false }, unwritten = true }
    end
    if not restore.discard and not same_states(now_states, states.after) then
      fail("changed", "the sections or mutes of the slots Kumi moved changed in Renoise since: Kumi left them")
    end
  end
  local at = base.follow_slot(kept, now, applied.index)
  -- (where it goes back: the gap it left, followed through what the producer made or deleted since, among the slots without it)
  local without_kept, without_now = moved(kept, applied.index, #kept), moved(now, at or 1, #now)
  without_kept[#without_kept], without_now[#without_now] = nil, nil
  local back = at and base.follow_slot(without_kept, without_now, applied.from, true) or nil
  if not at or not back then stale("moved the slot") end
  local title = string.format("Moved slot %d back to place %d", at, back)
  if at == back then
    return { restored = { index = back }, display = { title = title }, undo = { desc = desc, entry = false }, unwritten = true }
  end
  song:describe_undo(desc)
  move_to(song, at, back)
  base.note_restored(env.journal, "slot-move", {}, applied, base.KINDS["slot-move"])
  return { restored = { index = back }, display = { title = title }, undo = { desc = desc, entry = true } }
end

-- slot-unique

local function unique_title(a, b) return "Made the patterns of " .. slots_text(a, b) .. " unique" end

local function unique_plan(song, env, input)
  local a = seq_of(song, env, input.ref, "slot-unique")
  local b = input.last ~= nil and seq_of(song, env, input.last, "slot-unique") or a
  if b < a then bad("the last slot comes before the first") end
  local sequence, played = sequence_of(song), {}
  for _, p in ipairs(sequence) do played[p] = (played[p] or 0) + 1 end
  local shared = 0
  for s = a, b do
    if played[sequence[s]] > 1 then shared = shared + 1 end
  end
  return a, b, sequence, shared
end

local function unique_preview(song, env, input)
  local a, b, sequence, shared = unique_plan(song, env, input)
  local display = { title = unique_title(a, b), before = { patterns = #song.patterns }, after = { patterns = #song.patterns + shared } }
  if shared == 0 then display.unchanged = true else display.note = "Each of those slots whose pattern another slot plays gets a copy of it (\"#\" and its name)." end
  return { target = song_target(env), prior = { slots = #sequence }, fence = { sequence = sequence, patterns = #song.patterns }, display = display }
end

local function unique_apply(song, env, input, fence, desc)
  local ok, a, b, sequence, shared = pcall(unique_plan, song, env, input)
  if not ok then base.gone_is_changed(a) end
  if not same_list(sequence, fence.sequence) or #song.patterns ~= fence.patterns then fail("changed", "the sequence changed since the preview") end
  local display = { title = unique_title(a, b), before = { patterns = #song.patterns } }
  if shared == 0 then
    display.unchanged, display.after = true, { patterns = #song.patterns }
    return { applied = { sequence = sequence }, display = display, undo = { desc = desc, entry = false }, prior = { sequence = sequence } }
  end
  local pool_before = #song.patterns
  song:describe_undo(desc)
  song.sequencer:make_range_unique(a, b)
  local after = sequence_of(song)
  local changed = {}
  for s = a, b do
    if after[s] ~= sequence[s] then
      changed[#changed + 1] = { slot = s, from = sequence[s], to = after[s], pattern = after[s] > pool_before and keep_pattern(song, env, after[s]) or nil }
    end
  end
  display.after = { patterns = #song.patterns }
  local prior, applied = journal_fields(sequence, after)
  applied.pool, applied.changed = env.pattern_mark and env.pattern_mark() or nil, changed
  return {
    applied = applied, target = song_target(env), display = display, undo = { desc = desc, entry = true },
    prior = prior, place = {}, follows = "sequence",
  }
end

local function unique_restore(song, env, restore, desc)
  local applied = restore.applied
  if type(applied) ~= "table" or type(applied.sequence) ~= "table" or type(applied.before) ~= "table" or type(applied.changed) ~= "table" then
    bad("applied: the sequence before and after")
  end
  local now = sequence_of(song)
  local redone_after, redone_before = redone_sequences(song, restore)
  local kept, before = redone_after, redone_before
  if not kept then kept, before = kept_now(env, applied.sequence, applied.pool), kept_now(env, applied.before, applied.pool) end
  if not kept or not before then stale("made the patterns unique") end
  local title = "Slots play their shared patterns again (put back)"
  if same_list(now, before) then
    return { restored = { done = true }, display = { title = title }, undo = { desc = desc, entry = false }, unwritten = true }
  end
  -- each changed slot where it is now, playing the copy Kumi made, and the pattern it played before, where that is now
  local todo = {}
  for _, change in ipairs(applied.changed) do
    local s = base.follow_slot(kept, now, change.slot)
    if not s then stale("made the patterns unique") end
    local copy, original
    if redone_after and redone_before then
      copy, original = redone_after[change.slot], redone_before[change.slot]
    else
      copy, original = env.pattern_at(change.to, applied.pool), env.pattern_at(change.from, applied.pool)
    end
    if not copy or now[s] ~= copy then fail("changed", string.format("slot %d plays another pattern now", s)) end
    if not original then fail("changed", string.format("the pattern slot %d played before is gone since", s)) end
    if change.pattern then
      local why = pattern_changed(song, env, copy, change.pattern)
      if why then fail("changed", string.format("the copy %s that slot %d plays: %s since Kumi made it", q(patterns_.pattern_label(song, copy)), s, why)) end
    end
    todo[#todo + 1] = { slot = s, copy = copy, original = original, made = change.pattern ~= nil }
  end
  -- (the copies from the pool's end down: each removed as its slot lets go of it, which leaves the one before last)
  table.sort(todo, function(x, y) return x.copy > y.copy end)
  song:describe_undo(desc)
  local kept_patterns, played = 0, 0
  for _, item in ipairs(todo) do
    local mine = { [item.slot] = true }
    if item.made and aliased(song, item.copy) then
      played = played + 1
    elseif item.made and item.copy == #song.patterns and not used_by_others(song, item.copy, mine) then
      song:pattern(item.copy):clear()
    elseif item.made then
      kept_patterns = kept_patterns + 1
    end
    song.sequencer:set_pattern(item.slot, item.original)
  end
  base.note_restored(env.journal, "slot-unique", {}, applied, base.KINDS["slot-unique"])
  local display = { title = title }
  if kept_patterns > 0 then
    display.note = string.format("%s stay in the pool, unused: patterns were made after them.", kept_patterns == 1 and "The copy" or (kept_patterns .. " copies"))
  end
  if played > 0 then
    local said = string.format("%s Kumi made %s: another pattern's track plays %s as an alias.", played == 1 and "The copy" or (played .. " copies"),
      played == 1 and "stays" or "stay", played == 1 and "it" or "them")
    display.note = display.note and (display.note .. " " .. said) or said
  end
  return { restored = { done = true }, display = display, undo = { desc = desc, entry = true } }
end

-- slot-variation {pattern, track, first?, last?, content, name?, section?, index?} (M7: a variation take as a pattern): a new
-- pattern at a new slot (the sequence's end by default) that plays `pattern` with one track's lines first..last replaced by
-- `content` (cells as notes-write takes them). Its other tracks are aliases of `pattern`'s, so they follow the producer's edits
-- there; the varied track is a copy of its own (an alias in the source is read through, never written through). The slot starts
-- a section named `section`, or joins the one before it, and mutes what the source slot mutes (the slot `pattern` names, or the
-- first that plays it). Told by the whole sequence, as slot-clone is: Kumi's undo removes the slot and its pattern while both are
-- as Kumi left them, and hides again the track's columns it showed.

local cells = notes_.cells
local MAX_NAME = 256

local function variation_plan(song, env, input)
  local p = cells.pattern_of(song, env, input.pattern)
  local t, track = base.find(song, env, input.track, "slot-variation")
  if track.max_note_columns == 0 then bad("track: a sequencer track (its lines are what varies)") end
  local pattern = song:pattern(p)
  local first, last = cells.line_range(input, pattern.number_of_lines)
  if type(input.content) ~= "table" then bad("content: the varied lines, as notes_write takes them (an empty list clears them)") end
  local wants, count = {}, 0
  if #input.content > 0 then
    wants, count = cells.parse_cells(pattern, track, { lines = input.content }, env)
    for _, want in pairs(wants) do
      if want.at.line < first or want.at.line > last then
        bad(string.format("content's line %02d is outside the lines it varies (%02d to %02d)", want.at.line - 1, first - 1, last - 1))
      end
    end
  end
  if count * 7 > notes_.MAX_WRITES then bad(string.format("content is up to %d writes, more than one change takes (%d)", count * 7, notes_.MAX_WRITES)) end
  local n = slot_count(song)
  local index = input.index == nil and n + 1 or input.index
  if type(index) ~= "number" or index ~= math.floor(index) or index < 1 or index > n + 1 then bad(string.format("index is a whole number from 1 to %d", n + 1)) end
  for _, field in ipairs({ "name", "section" }) do
    local value = input[field]
    if value ~= nil and (type(value) ~= "string" or #value > MAX_NAME or value:find("[%c]")) then bad(field .. " must be one line of text") end
  end
  -- (the matrix mutes belong to a slot: the source slot's when a slot names the pattern, else the first slot that plays it)
  local mutes = {}
  local named = refs.parse(input.pattern)
  if named and named.kind == "seq" then
    mutes = state_of(song, named.n[1]).mutes
  else
    for slot, value in ipairs(song.sequencer.pattern_sequence) do
      if value == p then
        mutes = state_of(song, slot).mutes
        break
      end
    end
  end
  return { p = p, t = t, track = track, first = first, last = last, wants = wants, index = index, slots = n, mutes = mutes }
end

local function variation_title(song, plan, name)
  local made = name and name ~= "" and (" " .. q(name)) or ""
  return string.format("Added slot %d%s: a variation of %s on %s", plan.index, made, q(patterns_.pattern_label(song, plan.p)), q(base.named(plan.t, plan.track)))
end

local function variation_note(song, plan)
  return string.format("Its other tracks are aliases of %s's, so they play your edits there.", q(patterns_.pattern_label(song, plan.p)))
end

local function variation_preview(song, env, input)
  local plan = variation_plan(song, env, input)
  return {
    target = base.target(env, plan.t), prior = { slots = plan.slots },
    fence = { slots = plan.slots, patterns = #song.patterns, pattern = plan.p },
    display = { title = variation_title(song, plan, input.name), before = { slots = plan.slots }, after = { slots = plan.slots + 1 }, note = variation_note(song, plan) },
  }
end

-- The variation made in one go (inside the apply's step): the slot and its pattern, its tracks aliased or copied, the range
-- written. Answers the pattern's number, the cells written and the columns shown.
local function make_variation(song, plan, name)
  local sequencer = song.sequencer
  local pool = #song.patterns
  sequencer:insert_new_pattern_at(plan.index)
  local np = sequencer.pattern_sequence[plan.index]
  local new, source = song:pattern(np), song:pattern(plan.p)
  -- (what was made, for a cleanup if Renoise refuses part of it: the slot's pattern, and when Renoise gave it a free one of the
  -- pool rather than a new one, that pattern's name and length)
  plan.made = { p = np, reused = np <= pool and { name = new.name, lines = new.number_of_lines } or nil }
  if new.number_of_lines ~= source.number_of_lines then new.number_of_lines = source.number_of_lines end
  new.name = name
  for t = 1, rows.track_count(song) do
    local from = source:track(t)
    if t == plan.t then
      -- (copied from the lines an alias shows, never from the alias: a copy of an alias plays the same lines, and a write to it
      -- would land in the pattern it shows; M7 lab, 3.5.4)
      local real = from.alias_pattern_index > 0 and song:pattern(from.alias_pattern_index):track(t) or from
      local varied = new:track(t)
      varied:copy_from(real)
      if varied.alias_pattern_index > 0 then
        varied.alias_pattern_index = 0
        varied:copy_from(real)
      end
      if varied.alias_pattern_index > 0 then error("Renoise kept the varied track an alias", 0) end
    else
      local alias = from.alias_pattern_index
      pcall(function() new:track(t).alias_pattern_index = alias > 0 and alias or plan.p end)
    end
  end
  local varied = new:track(plan.t)
  for l = plan.first, plan.last do
    local line = varied:line(l)
    if not line.is_empty then
      for c = 1, 12 do line:note_column(c):clear() end
      for c = 1, 8 do line:effect_column(c):clear() end
    end
  end
  local _, after = cells.plan_of(varied, plan.wants, false)
  local needs = cells.needs_of(after)
  local hidden = cells.hidden_use(song, plan.t, plan.track, needs)
  cells.raise_columns(plan.track, needs)
  for _, cell in ipairs(after) do cells.write_cell(varied, cell) end
  for _, cell in ipairs(after) do
    if not cells.same_cell(cells.read_cell(varied, cell), cell) then error(string.format("Renoise didn't keep line %02d as written", cell.line - 1), 0) end
  end
  return np, after, hidden
end

-- A variation Renoise refused part of, taken back (#401: Renoise may give the slot a free pattern of the pool, not a new one at
-- its end): the slot it inserted goes, its pattern emptied so Renoise lets go of a new one, a free one's name and length put back.
local function unmake_variation(song, plan, before)
  local now = song.sequencer.pattern_sequence
  if not plan.made or #now ~= #before + 1 or now[plan.index] ~= plan.made.p then return end
  local pattern = song:pattern(plan.made.p)
  if plan.made.reused then
    -- (a free pattern of the pool: its slot goes first, while it holds the variation's lines, so Renoise keeps it; then it's
    -- emptied, with its name and length back)
    song.sequencer:delete_sequence_at(plan.index)
    pattern:clear()
    pattern.name, pattern.number_of_lines = plan.made.reused.name, plan.made.reused.lines
  else
    pattern:clear()
    song.sequencer:delete_sequence_at(plan.index)
  end
end
M.unmake_variation = unmake_variation

local function variation_apply(song, env, input, fence, desc)
  local ok, plan = pcall(variation_plan, song, env, input)
  if not ok then base.gone_is_changed(plan) end
  local before = sequence_of(song)
  if #before ~= fence.slots or #song.patterns ~= fence.patterns or plan.p ~= fence.pattern then
    fail("changed", "the sequence or its patterns changed since the preview")
  end
  local columns = cells.read_columns(plan.track)
  local name = input.name or ("Variation of " .. patterns_.pattern_label(song, plan.p))
  song:describe_undo(desc)
  local made, np, after, hidden = pcall(make_variation, song, plan, name)
  if not made then
    -- (what was made goes again: the slot, its pattern emptied so Renoise lets go of it, the columns as they were)
    local why = np
    pcall(unmake_variation, song, plan, before)
    cells.set_columns(plan.track, columns)
    fail("failed", "Renoise wouldn't make the variation: " .. tostring(type(why) == "table" and why.message or why):gsub("^[^%s:]*%.lua:%d+: ", ""))
  end
  set_state(song, plan.index, { start = input.section ~= nil, name = input.section or "", mutes = plan.mutes })
  local now = sequence_of(song)
  local prior, applied = journal_fields(before, now)
  local shown = cells.read_columns(plan.track)
  shown.hidden = hidden
  applied.pool, applied.at, applied.count = env.pattern_mark and env.pattern_mark() or nil, plan.index, 1
  applied.made = { { state = state_of(song, plan.index), pattern = keep_pattern(song, env, np), section = section_at(song, plan.index) } }
  applied.columns, applied.priorColumns, applied.cells = shown, columns, #after
  return {
    applied = applied, target = base.target(env, plan.t),
    created = {
      ref = refs.make(env.epoch(), "seq", plan.index), kind = "seq", pattern = refs.make(env.epoch(), "pattern", np),
      -- (the pattern as a place in the pool: Kumi finds the variation by it after other patterns came and went)
      place = base.pattern_place(env, np),
    },
    display = { title = variation_title(song, plan, name), before = { slots = #before }, after = { slots = #now }, note = variation_note(song, plan) },
    undo = { desc = desc, entry = true }, prior = prior, place = {}, follows = "sequence",
  }
end

-- (after the slot and its pattern went: the track's columns Kumi showed are hidden again, as notes-write's undo hides them)
local function variation_columns_back(song, env, restore)
  local target, applied = restore.target, restore.applied
  if type(target) ~= "table" or type(target.ref) ~= "string" or type(applied.columns) ~= "table" then return end
  -- (the request's refs were made current as it came in; the slot this restore deleted moved the epoch since, not the tracks)
  local ref = refs.parse(target.ref)
  local t = ref and ref.kind == "track" and ref.n[1] or nil
  if t and t <= rows.track_count(song) then cells.lower_columns(song, t, song:track(t), applied.priorColumns, applied.columns) end
end

-- (a variation's section may move to it from a variation dropped before it, as Renoise passes a section on: its mutes are what
-- Kumi keeps from the producer's edits; a section start goes with the slot, and the next slot keeps its own)
local variation_restore = made_restore("slot-variation", {
  what = "made the variation's slot", did = "made it", after = variation_columns_back, mutes_only = true,
  title = function() return "Removed the variation's slot (put back)" end,
})

-- slots-drop {refs}: several slots of the sequence deleted in one step (M7: the variations not kept, in one undo step). Their
-- patterns stay in the pool, unused, unless Renoise removes one (the pool's last, empty, played by no other slot: then Kumi keeps
-- its name, length and aliases to make it again). Told by the whole sequence: Kumi's undo puts every slot back at its place, with
-- its section and mutes, while the sequence is as the drop left it.

local MAX_DROPPED = 64

local function drop_plan(song, env, input)
  if type(input.refs) ~= "table" or #input.refs == 0 then bad("refs: the sequence slots to delete") end
  if #input.refs > MAX_DROPPED then bad(string.format("refs: at most %d slots in one step", MAX_DROPPED)) end
  local slots, seen = {}, {}
  for _, reference in ipairs(input.refs) do
    local s = seq_of(song, env, reference, "slots-drop")
    if seen[s] then bad(string.format("slot %d is named twice", s)) end
    seen[s] = true
    slots[#slots + 1] = s
  end
  table.sort(slots)
  if #slots >= slot_count(song) then bad("those are all the song's slots: one must stay") end
  return slots, seen
end

local function slots_list_text(slots)
  if #slots == 1 then return "slot " .. slots[1] end
  local shown = {}
  for i, s in ipairs(slots) do shown[i] = tostring(s) end
  return "slots " .. table.concat(shown, ", ", 1, #shown - 1) .. " and " .. shown[#shown]
end

local DROP_NOTE = "Their patterns stay in the pool, unused."

-- Renoise removes pattern `p` as slot `s` lets go of it: the pool's last, empty, played by no other slot (M3, measured).
local function goes_with_pattern(song, p, s)
  return p == #song.patterns and not used_by_others(song, p, { [s] = true }) and song:pattern(p).is_empty
end

local function drop_preview(song, env, input)
  local slots = drop_plan(song, env, input)
  local n = slot_count(song)
  return {
    target = song_target(env), prior = { slots = n }, fence = { sequence = sequence_of(song), patterns = #song.patterns },
    display = { title = "Deleted " .. slots_list_text(slots), before = { slots = n }, after = { slots = n - #slots }, note = DROP_NOTE },
  }
end

-- A pattern Renoise is about to remove as its slot lets go of it: what makes it again (its name, length and aliases; it is empty).
local function gone_pattern(song, p)
  local pattern = song:pattern(p)
  local aliases = {}
  for t = 1, rows.track_count(song) do
    local alias = pattern:track(t).alias_pattern_index
    if alias > 0 then aliases[tostring(t)] = alias end
  end
  return { name = pattern.name, lines = pattern.number_of_lines, aliases = aliases }
end

local function drop_apply(song, env, input, fence, desc)
  local ok, slots, dropped = pcall(drop_plan, song, env, input)
  if not ok then base.gone_is_changed(slots) end
  local before = sequence_of(song)
  if not same_list(before, fence.sequence) or #song.patterns ~= fence.patterns then fail("changed", "the sequence changed since the preview") end
  local n = #before
  local items = {}
  for k, s in ipairs(slots) do
    items[k] = { slot = s, pattern = before[s], state = state_of(song, s) }
    -- (deleting a section's start passes it to the next slot, which keeps it: the undo puts that slot's own back)
    if s < n and not dropped[s + 1] then items[k].next = state_of(song, s + 1) end
  end
  song:describe_undo(desc)
  local sequencer = song.sequencer
  for k = #items, 1, -1 do
    local item = items[k]
    local pool = #song.patterns
    local p = sequencer.pattern_sequence[item.slot]
    local goes = goes_with_pattern(song, p, item.slot)
    local kept = goes and gone_pattern(song, p) or nil
    sequencer:delete_sequence_at(item.slot)
    if #song.patterns < pool then item.gone = kept or { name = "", lines = 64, aliases = {} } end
  end
  -- (what each next slot's section reads once all went, at slot - k: the undo puts its own back only while it still reads so)
  for k, item in ipairs(items) do
    if item.next then
      local after = state_of(song, item.slot - k + 1)
      item.nextAfter = { start = after.start, name = after.name }
    end
  end
  local now = sequence_of(song)
  local prior, applied = journal_fields(before, now)
  applied.pool, applied.items = env.pattern_mark and env.pattern_mark() or nil, items
  local removed = 0
  for _, item in ipairs(items) do if item.gone then removed = removed + 1 end end
  local note = DROP_NOTE
  if removed > 0 then
    note = string.format("Renoise removed %d empty %s with %s; Kumi makes %s again on undo. The other patterns stay in the pool, unused.",
      removed, removed == 1 and "pattern" or "patterns", removed == 1 and "its slot" or "their slots", removed == 1 and "it" or "them")
  end
  return {
    applied = applied, target = song_target(env),
    display = { title = "Deleted " .. slots_list_text(slots), before = { slots = n }, after = { slots = #now }, note = note },
    undo = { desc = desc, entry = true }, prior = prior, place = {}, follows = "sequence",
  }
end

local function drop_restore(song, env, restore, desc)
  local applied = restore.applied
  if type(applied) ~= "table" or type(applied.sequence) ~= "table" or type(applied.before) ~= "table" or type(applied.items) ~= "table" then
    bad("applied: the sequence before and after the drop")
  end
  local now = sequence_of(song)
  local kept, before = redone_sequences(song, restore)
  if not kept then kept, before = kept_now(env, applied.sequence, applied.pool), kept_now(env, applied.before, applied.pool) end
  if not kept or not before then stale("deleted slots") end
  local slots = {}
  for i, item in ipairs(applied.items) do slots[i] = item.slot end
  local title = slots_list_text(slots):gsub("^%l", string.upper) .. " put back"
  if same_list(now, before) then
    return { restored = { done = true }, display = { title = title .. " already" }, undo = { desc = desc, entry = false }, unwritten = true }
  end
  if not same_list(now, kept) then stale("deleted slots") end
  -- (each slot's pattern where the pool has it now; a removed one is made again)
  local patterns = {}
  for i, item in ipairs(applied.items) do
    if not item.gone then
      local p = before and before[item.slot]
      if type(p) ~= "number" or p > #song.patterns then fail("gone", string.format("the pattern slot %d played isn't in the song any more", item.slot)) end
      patterns[i] = p
    end
  end
  song:describe_undo(desc)
  local sequencer = song.sequencer
  local kept_sections = {}
  for i, item in ipairs(applied.items) do
    if item.gone then
      sequencer:insert_new_pattern_at(item.slot)
      local pattern = song:pattern(sequencer.pattern_sequence[item.slot])
      if item.gone.lines and pattern.number_of_lines ~= item.gone.lines then pattern.number_of_lines = item.gone.lines end
      if item.gone.name and item.gone.name ~= "" then pattern.name = item.gone.name end
      for key, alias in pairs(item.gone.aliases or {}) do
        pcall(function() pattern:track(tonumber(key)).alias_pattern_index = alias end)
      end
    else
      sequencer:insert_sequence_at(item.slot, patterns[i])
    end
    local state = item.state or { start = false, name = "", mutes = {} }
    local mutes = {}
    for _, t in ipairs(state.mutes or {}) do
      local moved = t
      if type(restore.layout) == "number" and env.shifted then moved = env.shifted(t, restore.layout) end
      if moved and moved <= rows.track_count(song) then mutes[#mutes + 1] = moved end
    end
    set_state(song, item.slot, { start = state.start, name = state.name, mutes = mutes })
    if item.next and item.slot < slot_count(song) then
      -- (a section the producer made, renamed or ended on the next slot since stays theirs)
      local now_next = state_of(song, item.slot + 1)
      local was = item.nextAfter
      if not was or (now_next.start == was.start and now_next.name == was.name) then
        set_state(song, item.slot + 1, item.next, true)
      else
        kept_sections[#kept_sections + 1] = item.slot + 1
      end
    end
  end
  base.note_restored(env.journal, "slots-drop", {}, applied, base.KINDS["slots-drop"])
  local display = { title = title }
  if #kept_sections > 0 then
    display.note = string.format("The section on %s changed since the drop: Kumi left it as the producer made it.", slots_list_text(kept_sections))
  end
  return { restored = { done = true }, display = display, undo = { desc = desc, entry = true } }
end

-- (the journal compares the whole sequence; see changes_base.lua)
local function sequence_spec(spec)
  spec.custom = true
  spec.same = function(a, b) return same_list(a.sequence, b.sequence) end
  base.register(spec)
end

-- The patterns a change made, where they are once Renoise's redo made them again (`redo_marks`): their counts of changed lines
-- start again there (the journal's `retouched_at`).
local function clone_marks(entry)
  local out, applied = {}, entry.applied
  for i, made in ipairs(type(applied.made) == "table" and applied.made or {}) do
    if type(made.pattern) == "table" then out[#out + 1] = { mark = made.pattern.touch, p = applied.sequence[applied.at + i - 1] } end
  end
  return out
end

local function unique_marks(entry)
  local out, applied = {}, entry.applied
  for _, change in ipairs(type(applied.changed) == "table" and applied.changed or {}) do
    if type(change.pattern) == "table" then out[#out + 1] = { mark = change.pattern.touch, p = applied.sequence[change.slot] } end
  end
  return out
end

sequence_spec { kind = "slot-clone", preview = clone_preview, apply = clone_apply, restore = clone_restore, redo_marks = clone_marks }
sequence_spec { kind = "slot-move", preview = move_preview, apply = move_apply, restore = move_restore }
sequence_spec { kind = "slot-unique", preview = unique_preview, apply = unique_apply, restore = unique_restore, redo_marks = unique_marks }
-- M7 (variations)
sequence_spec { kind = "slot-variation", preview = variation_preview, apply = variation_apply, restore = variation_restore, redo_marks = clone_marks }
sequence_spec { kind = "slots-drop", preview = drop_preview, apply = drop_apply, restore = drop_restore }

return M
