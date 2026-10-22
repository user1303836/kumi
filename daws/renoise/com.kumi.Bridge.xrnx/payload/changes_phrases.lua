-- Kumi's changes to an instrument's phrases (M4): phrases made and deleted, their settings and key mappings, the instrument's
-- phrase playback (off, program, keymap), a phrase's lines written and cleared (in Renoise's own cell text, as a pattern's: see
-- changes_notes.lua), and phrase scripts: written and committed (with the compile and runtime errors Renoise reports), and
-- rendered to lines. Records keep the phrase by its place (instruments.lua); a phrase's lines are undone only while nothing in
-- that phrase changed since Kumi's step (the plain rule of a pattern's lines, counted per phrase).

local import = ...
local json = import("json")
local base = import("changes_base")
local places_ = import("instruments")
local instruments_ = import("changes_instruments")
local notes_ = import("changes_notes")

local bad, fail = base.bad, base.fail
local q, close = base.q, base.close
local whole, is_boolean, named = instruments_.whole, instruments_.is_boolean, instruments_.named
local cells = notes_.cells
local M = {}

-- The most writes a phrase change makes, and a phrase delete may need to put back, in one callback.
M.MAX_WRITES = base.SLICE_WRITES
-- How long a script's apply waits for Renoise to compile it (it compiles in its player, after the commit), and a render for
-- its result.
M.COMPILE_WAIT_MS = 600
M.RENDER_WAIT_MS = 5000
M.MAX_SCRIPT_BYTES = 64 * 1024

local KEY_TRACKING = named({ none = 1, transpose = 2, offset = 3 })
local PLAYBACK = named({ pattern = 1, script = 2 })

local function phrase_label_of(n, objects) return places_.label_of("phrase", n, objects) end

-- Settings

local function bool_field(field, says, on, off)
  return { get = function(p) return p[field] end, set = function(p, v) p[field] = v end, field = field, check = is_boolean, says = says,
    text = function(v) return v and on or off end }
end

-- A phrase switched to play its script takes its default name in Renoise ("Phrase 01": lab 2026-10-19): Kumi switches it and
-- keeps the name the phrase had, in the same step.
function M.set_playback(phrase, mode)
  if phrase.playback_mode == mode then return end
  local name = phrase.name
  phrase.playback_mode = mode
  if phrase.name ~= name then phrase.name = name end
end

local PHRASE_FIELDS = {
  name = { get = function(p) return p.name end, set = function(p, v) p.name = v end, field = "name",
    check = function(v) return type(v) == "string" and #v <= 64 and not v:find("[%c]") end, says = "name must be one line of text", text = function(v) return "named " .. q(v) end },
  lines = { get = function(p) return p.number_of_lines end, set = function(p, v) p.number_of_lines = v end, field = "number_of_lines",
    check = whole(1, 512), says = "lines is from 1 to 512", text = function(v) return v .. " lines" end },
  lpb = { get = function(p) return p.lpb end, set = function(p, v) p.lpb = v end, field = "lpb", check = whole(1, 256), says = "lpb is from 1 to 256",
    text = function(v) return "LPB " .. v end },
  looping = bool_field("looping", "looping is true or false", "looping", "one-shot"),
  -- (the loop's lines are checked with the length the change gives: see phrase-set's wanted)
  -- (lines by Renoise's numbers, from 0)
  loopStart = { get = function(p) return p.loop_start - 1 end, set = function(p, v) p.loop_start = v + 1 end, field = "loop_start",
    check = whole(0, 511), says = "loopStart is a line of the phrase, from 0", text = function(v) return string.format("loop from line %02d", v) end },
  loopEnd = { get = function(p) return p.loop_end - 1 end, set = function(p, v) p.loop_end = v + 1 end, field = "loop_end",
    check = whole(0, 511), says = "loopEnd is a line of the phrase, from 0", text = function(v) return string.format("loop to line %02d", v) end },
  shuffle = { get = function(p) return p.shuffle end, set = function(p, v) p.shuffle = v end, field = "shuffle",
    check = function(v) return type(v) == "number" and v >= 0 and v <= 1 end, says = "shuffle is from 0 to 1", text = function(v) return "shuffle " .. math.floor(v * 100 + 0.5) .. "%" end },
  keyTracking = { get = function(p) return KEY_TRACKING.get(p.key_tracking) end, set = function(p, v) p.key_tracking = KEY_TRACKING.parse(v) end, field = "key_tracking",
    check = function(v) return KEY_TRACKING.parse(v) ~= nil end, says = "keyTracking is none, transpose or offset", text = function(v) return "key tracking " .. v end },
  baseNote = { get = function(p) return p.base_note end, set = function(p, v) p.base_note = v end, field = "base_note", check = whole(0, 119),
    says = "baseNote is a note from 0 to 119 (C-4 is 48)", text = function(v) return "base note " .. v end },
  autoseek = bool_field("autoseek", "autoseek is true or false", "autoseek on", "autoseek off"),
  noteColumns = { get = function(p) return p.visible_note_columns end, set = function(p, v) p.visible_note_columns = v end, field = "visible_note_columns",
    check = whole(1, 12), says = "noteColumns is from 1 to 12", text = function(v) return v .. " note columns" end },
  effectColumns = { get = function(p) return p.visible_effect_columns end, set = function(p, v) p.visible_effect_columns = v end, field = "visible_effect_columns",
    check = whole(0, 8), says = "effectColumns is from 0 to 8", text = function(v) return v .. " effect columns" end },
  instrumentColumn = bool_field("instrument_column_visible", "instrumentColumn is true or false", "sample column shown", "sample column hidden"),
  volumeColumn = bool_field("volume_column_visible", "volumeColumn is true or false", "volume column shown", "volume column hidden"),
  panningColumn = bool_field("panning_column_visible", "panningColumn is true or false", "panning column shown", "panning column hidden"),
  delayColumn = bool_field("delay_column_visible", "delayColumn is true or false", "delay column shown", "delay column hidden"),
  sampleEffectsColumn = bool_field("sample_effects_column_visible", "sampleEffectsColumn is true or false", "sample effects shown", "sample effects hidden"),
  playback = { get = function(p) return PLAYBACK.get(p.playback_mode) end, set = function(p, v) M.set_playback(p, PLAYBACK.parse(v)) end, field = "playback_mode",
    check = function(v) return PLAYBACK.parse(v) ~= nil end, says = "playback is pattern or script", text = function(v) return "plays its " .. v end },
}
local PHRASE_ORDER = { "name", "lines", "lpb", "looping", "loopStart", "loopEnd", "shuffle", "keyTracking", "baseNote", "autoseek", "noteColumns",
  "effectColumns", "instrumentColumn", "volumeColumn", "panningColumn", "delayColumn", "sampleEffectsColumn", "playback" }

-- Writes settings: the length first, then the loop's ends (within it) in the order that keeps its start before its end, then
-- the rest. A setting Renoise refuses doesn't keep the others from being written (a restore puts back what it can).
local function write_settings(phrase, state)
  local refused
  local function put(fn)
    local ok, err = pcall(fn)
    if not ok then refused = refused or err end
  end
  if state.lines ~= nil then put(function() phrase.number_of_lines = state.lines end) end
  local lines = phrase.number_of_lines
  local start = state.loopStart and math.min(state.loopStart + 1, lines)
  local finish = state.loopEnd and math.min(state.loopEnd + 1, lines)
  put(function()
    if start ~= nil and finish ~= nil and start > phrase.loop_end then
      phrase.loop_end = finish
      phrase.loop_start = start
    else
      if start ~= nil then phrase.loop_start = start end
      if finish ~= nil then phrase.loop_end = finish end
    end
  end)
  for _, key in ipairs(PHRASE_ORDER) do
    if state[key] ~= nil and key ~= "lines" and key ~= "loopStart" and key ~= "loopEnd" then put(function() PHRASE_FIELDS[key].set(phrase, state[key]) end) end
  end
  if refused then error(refused, 0) end
end

local phrase_set = instruments_.settings_kind({
  kind = "phrase-set", of = "phrase", noun = "settings", undoable = true, order = PHRASE_ORDER, fields = PHRASE_FIELDS,
})
phrase_set.write = write_settings
do
  local inner = phrase_set.wanted
  phrase_set.wanted = function(phrase, input)
    local state = inner(phrase, input)
    local lines = state.lines or phrase.number_of_lines
    local start, finish = state.loopStart or math.min(phrase.loop_start, lines) - 1, state.loopEnd or math.min(phrase.loop_end, lines) - 1
    if start >= lines or finish >= lines then bad("the loop must be within the phrase's lines") end
    if start > finish then bad("loopStart must not be after loopEnd") end
    return state
  end
end
instruments_.instrument_spec(phrase_set)

local function settings_of(phrase)
  local out = {}
  for _, key in ipairs(PHRASE_ORDER) do out[key] = PHRASE_FIELDS[key].get(phrase) end
  return out
end

-- A phrase's lines, as exact values (every column, hidden ones too), and how many writes put them back.
local function read_lines(phrase)
  local out, writes = {}, 0
  for l = 1, phrase.number_of_lines do
    local line = phrase:line(l)
    if not line.is_empty then
      local row = { line = l, notes = {}, effects = {} }
      for c = 1, 12 do
        local column = line:note_column(c)
        if not column.is_empty then
          row.notes[#row.notes + 1] = {
            col = c, note = column.note_value, instrument = column.instrument_value, volume = column.volume_value, panning = column.panning_value,
            delay = column.delay_value, fxn = column.effect_number_value, fxa = column.effect_amount_value,
          }
          writes = writes + 7
        end
      end
      for c = 1, 8 do
        local column = line:effect_column(c)
        if not column.is_empty then
          row.effects[#row.effects + 1] = { col = c, number = column.number_value, amount = column.amount_value }
          writes = writes + 2
        end
      end
      out[#out + 1] = row
    end
  end
  return out, writes
end
M.read_lines = read_lines

local function write_lines(phrase, rows)
  for _, row in ipairs(rows or {}) do
    local line = phrase:line(row.line)
    for _, cell in ipairs(row.notes or {}) do
      local column = line:note_column(cell.col)
      column.note_value, column.instrument_value, column.volume_value = cell.note, cell.instrument, cell.volume
      column.panning_value, column.delay_value = cell.panning, cell.delay
      column.effect_number_value, column.effect_amount_value = cell.fxn, cell.fxa
    end
    for _, cell in ipairs(row.effects or {}) do
      local column = line:effect_column(cell.col)
      column.number_value, column.amount_value = cell.number, cell.amount
    end
  end
end

local function lines_print(phrase)
  local rows = read_lines(phrase)
  local text = json.encode(rows, { sort_keys = true }) or ""
  return base.hash(text)
end

-- The phrase's mapping (its keys in keymap mode): its note range, or nil.
local function mapping_of(phrase)
  local mapping = phrase.mapping
  if not mapping then return nil end
  return { noteRange = { mapping.note_range[1], mapping.note_range[2] } }
end

-- The mapping of the instrument whose keys start at `low` (mappings don't overlap: a start names one), its index.
local function mapping_index(instrument, low)
  for m = 1, #instrument.phrase_mappings do
    if instrument:phrase_mapping(m).note_range[1] == low then return m end
  end
  return nil
end

-- Where a mapping of [low, high] goes among the instrument's (sorted by key), or nil when it would overlap one.
local function mapping_slot(instrument, low, high, except_low)
  local index = 1
  for m = 1, #instrument.phrase_mappings do
    local range = instrument:phrase_mapping(m).note_range
    if range[1] ~= except_low then
      if not (high < range[1] or low > range[2]) then return nil end
      if range[2] < low then index = m + 1 end
    end
  end
  return index
end

-- Maps a phrase to [low, high] (or unmaps it with nil), as Renoise allows: answers true, or false and why.
local function set_mapping(instrument, phrase, wanted)
  local current = mapping_of(phrase)
  if wanted == nil then
    if not current then return true end
    local m = mapping_index(instrument, current.noteRange[1])
    if not m then return false, "its mapping wasn't found" end
    instrument:delete_phrase_mapping_at(m)
    return true
  end
  local low, high = wanted[1], wanted[2]
  if current then
    if not mapping_slot(instrument, low, high, current.noteRange[1]) then return false, "those keys overlap another phrase's" end
    phrase.mapping.note_range = { low, high }
    return true
  end
  local index = mapping_slot(instrument, low, high)
  if not index then return false, "those keys overlap another phrase's" end
  local made = instrument:insert_phrase_mapping_at(index, phrase)
  made.note_range = { low, high }
  return true
end

local function range_ok(v) return type(v) == "table" and whole(0, 119)(v[1]) and whole(0, 119)(v[2]) and v[1] <= v[2] end

-- phrase-add {ref: instrument, index?, ...settings, content?}: `content` is its lines, as phrase-write takes them, written in the
-- same step (M7: a variation take is one change). The restore's print of the phrase covers them.

-- (set below, once the column helpers are there)
local write_content
local zero_z_cells, zero_z_scan

local function add_plan(song, env, input)
  local n, objects, label = places_.locate(song, env, "instrument", input, "phrase-add")
  local instrument = objects.instrument
  local count = #instrument.phrases
  if count >= 126 then bad(q(label) .. " has 126 phrases: Renoise takes no more") end
  local index = input.index == nil and count + 1 or input.index
  if not whole(1, count + 1)(index) then bad(string.format("index is a place from 1 to %d among the phrases", count + 1)) end
  local settings = {}
  for _, key in ipairs(PHRASE_ORDER) do
    if input[key] ~= nil then
      local value = input[key]
      local field = PHRASE_FIELDS[key]
      if not field.check(value) then bad(field.says) end
      settings[key] = value
    end
  end
  -- (a new phrase has 32 lines unless given, lab 3.5.4: its loop must fit what it will have)
  local lines = settings.lines or M.NEW_PHRASE_LINES
  if (settings.loopStart or 0) >= lines or (settings.loopEnd or 0) >= lines then bad("the loop must be within the phrase's lines") end
  if settings.loopStart and settings.loopEnd and settings.loopStart > settings.loopEnd then bad("loopStart must not be after loopEnd") end
  local content, cells_count
  if input.content ~= nil then
    content, cells_count = cells.parse_cells({ number_of_lines = lines }, { max_note_columns = 12, max_effect_columns = 8 }, { lines = input.content })
    if cells_count * 7 > M.MAX_WRITES then
      bad(string.format("content is up to %d writes, more than one phrase change takes (%d): add the phrase with fewer lines, then phrase_write the rest", cells_count * 7, M.MAX_WRITES))
    end
  end
  return { n = n, label = label, count = count, index = index, settings = settings, content = content }
end

-- What Renoise makes a new phrase with (lab 3.5.4): 32 lines, 4 note columns, key tracking transpose, autoseek on.
M.NEW_PHRASE_LINES = 32

local function phrase_print(phrase)
  return { name = phrase.name, lines = phrase.number_of_lines, empty = phrase.is_empty, mode = phrase.playback_mode, print = lines_print(phrase) }
end

-- The phrase the instrument's program plays (its number, 1 to the count) when it plays phrases by program; nil for none.
local function program_of(instrument)
  if instrument.phrase_playback_mode ~= renoise.Instrument.PHRASES_PLAY_SELECTIVE then return nil end
  local program = instrument.phrase_program
  if program >= 1 and program <= #instrument.phrases then return program end
  return nil
end

-- After a phrase was made at `index` (or deleted from it: `deleted`), the program plays the phrase it played (`program`, its
-- number before): Renoise leaves the number as a phrase is made at or before it, and moves it, not always with its phrase, as
-- one is deleted (lab 2026-10-21). The place of that phrase and the program as left, for the undo to put it back on that
-- phrase; nil when there's no program, or (a delete) it was the phrase deleted.
local function keep_program(env, instrument, i, program, index, deleted)
  if not program or (deleted and program == index) then return nil, nil end
  local plays
  if deleted then plays = program > index and program - 1 or program else plays = program >= index and program + 1 or program end
  if instrument.phrase_program ~= plays then pcall(function() instrument.phrase_program = plays end) end
  return places_.place_of(env, "phrase", { i, plays }), instrument.phrase_program
end

-- Whether the program is as Kumi's change left it (`after`), or names no phrase: unless the producer chose another since,
-- Kumi's undo may put it back.
local function program_unchosen(instrument, after)
  local program = instrument.phrase_program
  return program == 0 or program > #instrument.phrases or (after ~= nil and program == after)
end

-- The program on the phrase now at `on`, unless the producer chose another since the change left it at `after`. `unchosen`:
-- program_unchosen as read before the undo wrote anything (a delete may move the program itself).
local function program_back(instrument, on, after, unchosen)
  if not on then return end
  pcall(function()
    if unchosen == nil then unchosen = program_unchosen(instrument, after) end
    if instrument.phrase_playback_mode == renoise.Instrument.PHRASES_PLAY_SELECTIVE and unchosen and instrument.phrase_program ~= on then
      instrument.phrase_program = on
    end
  end)
end

-- The program on the phrase kept as `place` (keep_program's), wherever that is now.
local function program_back_to(env, instrument, i, place, after, unchosen)
  if type(place) ~= "table" then return end
  local now = places_.follow(env, place)
  if now and now[1] == i then program_back(instrument, now[2], after, unchosen) end
end
M.program_of, M.keep_program, M.program_back_to = program_of, keep_program, program_back_to

-- (" with 12 lines" in a title, when the phrase is made with its lines)
local function with_lines(input)
  if type(input.content) ~= "table" then return "" end
  return #input.content == 1 and " with 1 line" or string.format(" with %d lines", #input.content)
end

local function add_preview(song, env, input)
  local plan = add_plan(song, env, input)
  local title = string.format("Added phrase %s to %s%s", input.name and q(input.name) or string.format("%02d", plan.index), q(plan.label), with_lines(input))
  return {
    target = places_.target_of(env, "instrument", plan.n), prior = { phrases = plan.count }, fence = { phrases = plan.count, index = plan.index },
    display = {
      title = title, before = { phrases = plan.count }, after = { phrases = plan.count + 1 },
      -- (Renoise renumbers the 0Z commands that name phrases it moves: lab 2026-10-19, 0Z02 became 0Z03)
      note = plan.index <= plan.count and "The phrases after it move up one, and Renoise renumbers the 0Z commands in patterns that name them." or nil,
    },
  }
end

local function add_apply(song, env, input, fence, desc)
  local ok, plan = pcall(add_plan, song, env, input)
  if not ok then base.gone_is_changed(plan) end
  if plan.count ~= fence.phrases or plan.index ~= fence.index then fail("changed", q(plan.label) .. "'s phrases changed since the preview") end
  local instrument = song:instrument(plan.n[1])
  local program_before = program_of(instrument)
  song:describe_undo(desc)
  local phrase = instrument:insert_phrase_at(plan.index)
  local set, err = pcall(write_settings, phrase, plan.settings)
  if not set then
    pcall(function() instrument:delete_phrase_at(plan.index) end)
    fail("failed", "Renoise wouldn't take those settings: " .. tostring(type(err) == "table" and err.message or err))
  end
  if plan.content then
    local wrote, why = pcall(write_content, phrase, plan.content)
    if not wrote then
      pcall(function() instrument:delete_phrase_at(plan.index) end)
      fail("failed", "Renoise wouldn't take those lines: " .. tostring(type(why) == "table" and why.message or why):gsub("^[^%s:]*%.lua:%d+: ", ""))
    end
  end
  local m = { plan.n[1], plan.index }
  -- (the program plays the phrase it played before the new one came)
  local program_place, program_after = keep_program(env, instrument, plan.n[1], program_before, plan.index)
  -- (whether the new phrase plays as the program now: Renoise's doing, not the producer's, so Kumi's undo may take it)
  local program = instrument.phrase_playback_mode == renoise.Instrument.PHRASES_PLAY_SELECTIVE and instrument.phrase_program == plan.index
  return {
    applied = { index = plan.index, print = phrase_print(phrase), program = program or nil },
    -- (M7: its place too, which Kumi finds it by after other phrases came and went)
    created = { ref = places_.ref_of(env, "phrase", m), kind = "phrase", place = places_.place_of(env, "phrase", m) },
    material = { made = places_.place_of(env, "phrase", m), program = program_place, programAfter = program_after, zeroZScan = zero_z_scan(song, env) },
    target = places_.target_of(env, "phrase", m),
    display = { title = string.format("Added phrase %s to %s%s", q(places_.phrase_label(plan.index, phrase)), q(plan.label), with_lines(input)), before = { phrases = plan.count }, after = { phrases = plan.count + 1 } },
    undo = { desc = desc, entry = true }, prior = { phrases = plan.count }, place = places_.journal_place("phrase", m), shape = "made",
  }
end

-- What a phrase Kumi makes keeps for the look at 0Z commands its undo makes (#400 review notes): the pool's mark and every
-- pattern's count of changed lines then. A 0Z command naming it is written after it came (Renoise renumbers those before), so
-- in a song too big to look through whole, the patterns changed or made since are enough.
zero_z_scan = function(song, env)
  if not env.pattern_mark or not env.pattern_touch_mark then return nil end
  local patterns = {}
  for p = 1, #song.patterns do patterns[p] = true end
  return { mark = env.pattern_mark(), touches = base.touch_marks(env, patterns) }
end

-- The patterns (numbered now) changed or made since `scan` was kept, or nil when that can't be told.
local function zero_z_scope(song, env, scan)
  if type(scan) ~= "table" or type(scan.touches) ~= "table" or not scan.mark or not env.pattern_at or not env.pattern_touched_since then
    return nil
  end
  local scope, known = {}, {}
  for key, mark in pairs(scan.touches) do
    local now, why = env.pattern_at(tonumber(key), scan.mark)
    if why == "unknown" then return nil end
    if now then
      known[now] = true
      if not mark or env.pattern_touched_since(now, mark) ~= false then scope[now] = true end
    end
  end
  for p = 1, #song.patterns do
    if not known[p] then scope[p] = true end
  end
  return scope
end

-- How the producer plays phrase `ph` of `instrument` now, in words, or nil: keys mapped to it, the program (unless it was the
-- program when Kumi made it: `was_program`), or 0Z commands naming it (looked for in `scope`'s patterns when the song is too
-- big to look through whole; or a song too big for that too).
local function in_use(song, instrument, phrase, ph, was_program, scope)
  if phrase.mapping then return "is mapped to keys" end
  if instrument.phrase_playback_mode == renoise.Instrument.PHRASES_PLAY_SELECTIVE and instrument.phrase_program == ph and not was_program then
    return "is the instrument's phrase program"
  end
  local named = zero_z_cells(song, ph)
  if named == nil and scope then named = zero_z_cells(song, ph, scope) end
  if named == nil then return "may be named by 0Z commands (the song is too big for Kumi to look)" end
  if #named > 0 then return string.format("is named by %d 0Z %s in the patterns", #named, #named == 1 and "command" or "commands") end
  return nil
end

-- (the restore of a change that made a phrase: phrase-add, and phrase-load in changes_loaders.lua)
local function remove_made(song, env, restore, desc, kind)
  local material = restore.material
  if type(material) ~= "table" or type(material.made) ~= "table" then bad("material: the phrase the change made") end
  local n, why, which = places_.follow(env, material.made)
  if not n then
    if why == "gone" then fail("gone", "the phrase Kumi made isn't in the instrument any more") end
    places_.follow_failed(why, which, nil)
  end
  local objects = places_.resolve(song, "phrase", n)
  local label = phrase_label_of(n, objects)
  if not restore.discard and not base.same_table(phrase_print(objects.phrase), (restore.applied or {}).print) then
    fail("changed", label .. " changed in Renoise since Kumi made it (lines, length, name or script)")
  end
  -- (one the producer plays since, by keys, as the program or by 0Z commands, stays: deleting it would undo their work, and
  -- Renoise clears the 0Z cells that name it)
  if not restore.discard then
    local use = in_use(song, objects.instrument, objects.phrase, n[2], (restore.applied or {}).program, zero_z_scope(song, env, material.zeroZScan))
    if use then fail("changed", label .. " " .. use .. " since Kumi made it: Kumi left it, so nothing that plays it is lost") end
  end
  -- (the program back on the phrase it played when Kumi made this one: Renoise moves it as it deletes one, so whether the
  -- producer chose another since is read before the delete)
  local unchosen = program_unchosen(objects.instrument, material.programAfter)
  song:describe_undo(desc)
  objects.instrument:delete_phrase_at(n[2])
  program_back_to(env, objects.instrument, n[1], material.program, material.programAfter, unchosen)
  base.note_restored_at(env.journal, kind, places_.journal_place("phrase", n))
  return { restored = { deleted = true }, display = { title = "Removed " .. label .. " (put back)" }, undo = { desc = desc, entry = true } }
end
M.phrase_print, M.remove_made, M.zero_z_scan = phrase_print, remove_made, zero_z_scan

local function add_restore(song, env, restore, desc) return remove_made(song, env, restore, desc, "phrase-add") end

base.register({ kind = "phrase-add", custom = true, shape = "made", by_place = true, preview = add_preview, apply = add_apply, restore = add_restore })

-- phrase-delete {ref: phrase}: Kumi keeps its lines, settings, column names and mutes, key mapping and script, and whether it
-- was the instrument's program; the undo makes it again after the phrase that was before it.

local function columns_of(phrase)
  local names, muted = {}, {}
  for c = 1, 12 do
    local name = phrase:column_name(c)
    if name ~= "" then names[tostring(c)] = name end
    if phrase:column_is_muted(c) then muted[#muted + 1] = c end
  end
  return { names = next(names) and names or nil, muted = #muted > 0 and muted or nil }
end

local function delete_preview(song, env, input)
  local n, objects, label = places_.locate(song, env, "phrase", input, "phrase-delete")
  local _, writes = read_lines(objects.phrase)
  local display = { title = "Deleted phrase " .. label, note = "Kumi keeps its lines, settings, keys and script to put back. The phrases after it move down one, and Renoise renumbers the 0Z commands in patterns that name them." }
  if writes > M.MAX_WRITES then display.note = "It holds more lines than Kumi puts back in one step: the apply will refuse." end
  return { target = places_.target_of(env, "phrase", n), prior = { deleted = false }, fence = phrase_print(objects.phrase), display = display }
end

-- (the mapping's fields and how a delete keeps them: below, with phrase-map)
local map_state, write_map, ALL_MAPPING

-- The values of the script's input parameters, by name (none until Renoise compiled it).
local function script_parameters(phrase)
  local ok, kept = pcall(function()
    local out = {}
    for _, parameter in ipairs(phrase.script.parameters) do out[#out + 1] = { name = parameter.name, value = parameter.value } end
    return out
  end)
  return ok and #kept > 0 and kept or nil
end

-- Those values set again on the script's parameters of the same names: how many couldn't be (Renoise compiles a committed
-- script in its player a moment later, so its parameters may not be there yet).
local function write_script_parameters(phrase, kept)
  local missed = 0
  for _, saved in ipairs(kept or {}) do
    local done = pcall(function()
      for _, parameter in ipairs(phrase.script.parameters) do
        if parameter.name == saved.name then
          parameter.value = saved.value
          return
        end
      end
      error("not there")
    end)
    if not done then missed = missed + 1 end
  end
  return missed
end

-- The 0Z commands that name phrase `ph` (an effect column's, or a note column's sample effect), as places {p, t, line, col,
-- kind}; nil when the song is too big to look through in one callback. Renoise 3.5.4 clears those that name a phrase it deletes
-- (and renumbers the later ones), and an insert renumbers again but never fills a cleared one (lab 2026-10-21).
M.ZERO_Z_READS = 200000
-- The cells whose 0Z command names phrase `wanted` (a number, or a set of numbers: one look for them all), each with `ph`, in
-- every pattern or those in `only` (a set of numbers); nil when they are too big to look through.
zero_z_cells = function(song, wanted, only)
  local rows_ = import("rows")
  local named = type(wanted) == "table" and wanted or { [wanted] = true }
  local found, reads = {}, 0
  for p = 1, #song.patterns do
    local pattern = song:pattern(p)
    for t = 1, (not only or only[p]) and rows_.track_count(song) or 0 do
      local pattern_track = pattern:track(t)
      if not pattern_track.is_alias and not pattern_track.is_empty then
        for l = 1, pattern.number_of_lines do
          local line = pattern_track:line(l)
          reads = reads + 1
          if not line.is_empty then
            for c = 1, 8 do
              local column = line:effect_column(c)
              if column.number_string == "0Z" and named[column.amount_value] then found[#found + 1] = { p = p, t = t, line = l, col = c, kind = "effect", ph = column.amount_value } end
            end
            if song:track(t).max_note_columns > 0 then
              for c = 1, 12 do
                local column = line:note_column(c)
                local ok, number = pcall(function() return column.effect_number_string end)
                if ok and number == "0Z" and named[column.effect_amount_value] then found[#found + 1] = { p = p, t = t, line = l, col = c, kind = "note", ph = column.effect_amount_value } end
              end
            end
            reads = reads + 20
          end
          if reads > M.ZERO_Z_READS then return nil end
        end
      end
    end
  end
  return found
end

local function zero_z_column(song, cell)
  local line = song:pattern(cell.p):track(cell.t):line(cell.line)
  return cell.kind == "effect" and line:effect_column(cell.col) or line:note_column(cell.col)
end

local function zero_z_empty(song, cell)
  local column = zero_z_column(song, cell)
  if cell.kind == "effect" then return column.number_value == 0 and column.amount_value == 0 end
  return column.effect_number_value == 0 and column.effect_amount_value == 0
end

local function delete_apply(song, env, input, fence, desc)
  local ok, n, objects, label = pcall(places_.locate, song, env, "phrase", input, "phrase-delete")
  if not ok then base.gone_is_changed(n) end
  local phrase, instrument = objects.phrase, objects.instrument
  if not base.same_table(phrase_print(phrase), fence) then fail("changed", label .. " changed since the preview") end
  local lines, writes = read_lines(phrase)
  if writes > M.MAX_WRITES then fail("failed", label .. " holds more lines than Kumi puts back in one step, so it won't delete it: clear some of its lines first, or delete it in Renoise") end
  local material = {
    index = n[2], settings = settings_of(phrase), lines = lines, columns = columns_of(phrase),
    -- (its keys and how they play it: key tracking, base note, looping and loop, which are the mapping's own)
    mapping = phrase.mapping and map_state(phrase, ALL_MAPPING) or nil,
    script = phrase.script.paragraphs, scriptParameters = script_parameters(phrase), program = (instrument.phrase_playback_mode == renoise.Instrument.PHRASES_PLAY_SELECTIVE and instrument.phrase_program == n[2]) or nil,
    -- (its count of changed lines, for the one Kumi puts back to take on)
    logs = env.logs_of and env.logs_of("phrase", n) or nil,
  }
  -- (the 0Z commands that name it: those Renoise clears are kept, to be written again when it comes back)
  local named = zero_z_cells(song, n[2])
  local program_before = program_of(instrument)
  song:describe_undo(desc)
  instrument:delete_phrase_at(n[2])
  material.instrument = places_.place_of(env, "instrument", { n[1] })
  material.after = env.list_mark and env.list_mark("phrases", n[1]) or nil
  -- (a program on another phrase plays that one still: Renoise may move it as it deletes this one)
  material.programPhrase = keep_program(env, instrument, n[1], program_before, n[2], true)
  -- (the program Renoise moved it to, for the undo to know the producer didn't choose another since)
  if material.program or material.programPhrase then material.programAfter = instrument.phrase_program end
  local note
  if named then
    local cleared, patterns = {}, {}
    for _, cell in ipairs(named) do
      if zero_z_empty(song, cell) then
        cleared[#cleared + 1] = cell
        patterns[cell.p] = true
      end
    end
    if #cleared > 0 then
      material.zeroZ = cleared
      material.pool = import("changes_material").keep_pool(song, env, patterns)
      material.touches = base.touch_marks(env, patterns)
      note = string.format("Renoise cleared the %d 0Z %s that named it; Kumi puts %s back with it.", #cleared, #cleared == 1 and "command" or "commands",
        #cleared == 1 and "it" or "them")
    end
  else
    note = "The song is too big for Kumi to look for the 0Z commands that name it: Renoise clears them, and Kumi's undo won't write them again."
  end
  return {
    applied = { deleted = true }, material = material, target = places_.target_of(env, "instrument", { n[1] }),
    display = { title = "Deleted phrase " .. label, note = note }, undo = { desc = desc, entry = true }, prior = { deleted = false },
    place = places_.journal_place("phrase", n), shape = "deleted",
  }
end

local function delete_restore(song, env, restore, desc)
  local material = restore.material
  if type(material) ~= "table" or type(material.instrument) ~= "table" or type(material.settings) ~= "table" then bad("material: what the delete kept") end
  local label = q(material.settings.name ~= "" and material.settings.name or string.format("Phrase %02d", material.index))
  local n, why, which = places_.follow(env, material.instrument)
  if not n then places_.follow_failed(why, which, material.settings.name) end
  local instrument = song:instrument(n[1])
  local at = env.list_after and env.list_after("phrases", n[1], nil, material.index, material.after) or nil
  if not at then fail("stale", string.format("the phrases changed in a way Kumi couldn't follow since it deleted %s: nothing was put back", label)) end
  at = math.max(1, math.min(at or 1, #instrument.phrases + 1))
  if at <= #instrument.phrases then
    local there = instrument:phrase(at)
    if there.name == material.settings.name and there.number_of_lines == material.settings.lines and json.encode(read_lines(there), { sort_keys = true }) == json.encode(material.lines, { sort_keys = true }) then
      base.note_restored_at(env.journal, "phrase-delete", places_.journal_place("phrase", { n[1], at }))
      return { restored = { ref = places_.ref_of(env, "phrase", { n[1], at }) }, display = { title = label .. " is back already" }, undo = { desc = desc, entry = false }, unwritten = true }
    end
  end
  if #instrument.phrases >= 126 then fail("changed", "the instrument has 126 phrases now: there's no room to put it back") end
  -- (the patterns its cleared 0Z commands were in, and whose lines changed since: read before anything is written, as Kumi's
  -- own insert renumbers 0Z commands too)
  local zero_z = type(material.zeroZ) == "table" and #material.zeroZ > 0
  local pattern_now = zero_z and import("changes_material").pool_now(song, env, material.pool) or nil
  local touched = zero_z and base.touched_patterns(env, material.touches, pattern_now) or {}
  song:describe_undo(desc)
  local phrase = instrument:insert_phrase_at(at)
  local notes = {}
  pcall(write_settings, phrase, material.settings)
  write_lines(phrase, material.lines)
  for key, name in pairs((material.columns or {}).names or {}) do pcall(function() phrase:set_column_name(tonumber(key), name) end) end
  for _, c in ipairs((material.columns or {}).muted or {}) do pcall(function() phrase:set_column_is_muted(c, true) end) end
  if type(material.script) == "table" then
    pcall(function()
      phrase.script.paragraphs = material.script
      phrase.script:commit()
    end)
    local missed = write_script_parameters(phrase, material.scriptParameters)
    if missed > 0 then
      notes[#notes + 1] = string.format("Its script's %s back at %s defaults (Renoise compiles the script a moment later): set %s again in its script panel.",
        missed == 1 and "parameter comes" or (missed .. " parameters come"), missed == 1 and "its" or "their", missed == 1 and "it" or "them")
    end
  end
  if material.mapping then
    local mapped, why_not = write_map(instrument, phrase, material.mapping)
    if not mapped then notes[#notes + 1] = string.format("Its keys (%d-%d) weren't mapped again: %s.", material.mapping.noteRange[1], material.mapping.noteRange[2], why_not) end
  end
  if material.program then
    -- (Renoise moved the program off the phrase it deleted: unless the producer chose another since, it plays this one again)
    program_back(instrument, at, material.programAfter)
  else
    -- (a program on another phrase: the phrase put back below it leaves its number, which then names the phrase before it)
    program_back_to(env, instrument, n[1], material.programPhrase, material.programAfter)
  end
  -- The 0Z commands Renoise cleared, written again naming it where they still are empty (the patterns followed through the pool,
  -- the tracks through the layout).
  if zero_z then
    local skipped = 0
    for _, cell in ipairs(material.zeroZ) do
      local p = pattern_now and not touched[cell.p] and pattern_now(cell.p) or nil
      local t = cell.t
      if type(restore.layout) == "number" and env.shifted then t = env.shifted(cell.t, restore.layout) end
      local placed = p and t and { p = p, t = t, line = cell.line, col = cell.col, kind = cell.kind } or nil
      local written = placed and pcall(function()
        if placed.line > song:pattern(placed.p).number_of_lines or not zero_z_empty(song, placed) then error("taken") end
        local column = zero_z_column(song, placed)
        if placed.kind == "effect" then
          column.number_string, column.amount_value = "0Z", at
        else
          column.effect_number_string, column.effect_amount_value = "0Z", at
        end
      end)
      if not written then skipped = skipped + 1 end
    end
    if skipped > 0 then
      notes[#notes + 1] = string.format("%d of the 0Z commands that named it weren't written again (their pattern, track or cell changed since).", skipped)
    end
  end
  -- (the phrase Kumi deleted, at its place again with its lines: what was kept by its place, and its lines' count, follow it)
  if env.back then env.back("phrases", n[1], nil, material.after, at) end
  if env.adopt and material.logs then env.adopt("phrase", { n[1], at }, material.logs) end
  base.note_restored_at(env.journal, "phrase-delete", places_.journal_place("phrase", { n[1], at }))
  return {
    restored = { ref = places_.ref_of(env, "phrase", { n[1], at }) }, display = { title = label .. " put back", note = #notes > 0 and table.concat(notes, " ") or nil },
    undo = { desc = desc, entry = true },
  }
end

base.register({ kind = "phrase-delete", custom = true, shape = "deleted", by_place = true, preview = delete_preview, apply = delete_apply, restore = delete_restore })

-- phrases-drop {refs: phrases of one instrument} (M7: the variations not kept, in one undo step): several phrases deleted in one
-- step, each kept (its settings, lines and column names, and whether it was the instrument's program) to put back. A phrase in
-- use otherwise is refused (keys mapped, playing a script, or named by a 0Z command): phrase_delete keeps those. Each delete takes
-- its own mark of the phrases' log, so Kumi's undo puts each back after the phrase that was before it, wherever the producer's
-- edits moved that one. (Renoise 3.5.4 plays new instruments' phrases by program, and moves the program off a phrase it deletes:
-- lab 2026-10-21.)

M.MAX_DROPPED = 64

local function drop_plan(song, env, input)
  if type(input.refs) ~= "table" or #input.refs == 0 then bad("refs: the phrases to delete, all of one instrument") end
  if #input.refs > M.MAX_DROPPED then bad(string.format("refs: at most %d phrases in one step", M.MAX_DROPPED)) end
  local items, seen, instrument_at = {}, {}, nil
  for _, reference in ipairs(input.refs) do
    local n, objects, label = places_.locate(song, env, "phrase", { ref = reference }, "phrases-drop")
    if instrument_at and n[1] ~= instrument_at then bad("refs: phrases of one instrument") end
    instrument_at = n[1]
    if seen[n[2]] then bad(label .. " is named twice") end
    seen[n[2]] = true
    items[#items + 1] = { n = n, phrase = objects.phrase, instrument = objects.instrument, label = label }
  end
  table.sort(items, function(a, b) return a.n[2] < b.n[2] end)
  local instrument = items[1].instrument
  for _, item in ipairs(items) do
    local phrase = item.phrase
    if phrase.mapping then bad(item.label .. " is mapped to keys: delete it with phrase_delete, which keeps its keys") end
    if phrase.playback_mode == renoise.InstrumentPhrase.PLAY_SCRIPT then bad(item.label .. " plays a script: delete it with phrase_delete, which keeps it") end
  end
  return instrument_at, items
end

-- (the 0Z commands naming any of them, looked for once the request is otherwise fine: one look through the song's patterns for
-- them all, bounded)
local function drop_unused(song, items)
  local wanted = {}
  for _, item in ipairs(items) do wanted[item.n[2]] = true end
  local named = zero_z_cells(song, wanted)
  if named == nil then bad("the song is too big for Kumi to look for 0Z commands naming those phrases: delete them with phrase_delete") end
  for _, item in ipairs(items) do
    local count = 0
    for _, cell in ipairs(named or {}) do if cell.ph == item.n[2] then count = count + 1 end end
    if count > 0 then
      bad(string.format("%s is named by %d 0Z %s in the patterns: delete it with phrase_delete, which writes %s back on undo", item.label, count,
        count == 1 and "command" or "commands", count == 1 and "it" or "them"))
    end
  end
end

local function drop_title(items)
  if #items == 1 then return "Deleted phrase " .. items[1].label end
  local names = {}
  for i, item in ipairs(items) do names[i] = q(places_.phrase_label(item.n[2], item.phrase)) end
  return string.format("Deleted %d phrases (%s) in %s", #items, table.concat(names, ", "), q(places_.instrument_label(items[1].n[1], items[1].instrument)))
end

local DROP_NOTE = "Kumi keeps their lines and settings to put back. The phrases after them move down, and Renoise renumbers the 0Z commands that name those."

local function drops_preview(song, env, input)
  local i, items = drop_plan(song, env, input)
  drop_unused(song, items)
  local prints = {}
  for k, item in ipairs(items) do prints[k] = phrase_print(item.phrase) end
  return {
    target = places_.target_of(env, "instrument", { i }), prior = { deleted = false },
    fence = { phrases = #items[1].instrument.phrases, prints = prints }, display = { title = drop_title(items), note = DROP_NOTE },
  }
end

local function drops_apply(song, env, input, fence, desc)
  local ok, i, items = pcall(drop_plan, song, env, input)
  if not ok then base.gone_is_changed(i) end
  local instrument = items[1].instrument
  if #instrument.phrases ~= fence.phrases or #items ~= #(fence.prints or {}) then fail("changed", "the instrument's phrases changed since the preview") end
  for k, item in ipairs(items) do
    if not base.same_table(phrase_print(item.phrase), fence.prints[k]) then fail("changed", item.label .. " changed since the preview") end
  end
  drop_unused(song, items)
  local kept, writes = {}, 0
  for k, item in ipairs(items) do
    local lines, cost = read_lines(item.phrase)
    writes = writes + cost
    kept[k] = { index = item.n[2], settings = settings_of(item.phrase), lines = lines, columns = columns_of(item.phrase), logs = env.logs_of and env.logs_of("phrase", item.n) or nil,
      program = (instrument.phrase_playback_mode == renoise.Instrument.PHRASES_PLAY_SELECTIVE and instrument.phrase_program == item.n[2]) or nil }
  end
  if writes > M.MAX_WRITES then fail("failed", "those phrases hold more lines than Kumi puts back in one step: drop fewer at once") end
  local title = drop_title(items)
  -- (a program on a phrase that stays: Renoise may move it as phrases go, so Kumi puts it back on that phrase)
  local program_before, stays = instrument.phrase_program, nil
  if instrument.phrase_playback_mode == renoise.Instrument.PHRASES_PLAY_SELECTIVE and program_before >= 1 and program_before <= fence.phrases then
    stays = program_before
    for _, item in ipairs(items) do
      if item.n[2] == program_before then stays = nil break end
      if item.n[2] < program_before then stays = stays - 1 end
    end
  end
  song:describe_undo(desc)
  -- (from the last down, each with the phrases' mark just after it went: what follows it once Kumi puts it back)
  for k = #items, 1, -1 do
    instrument:delete_phrase_at(items[k].n[2])
    kept[k].after = env.list_mark and env.list_mark("phrases", i) or nil
  end
  if stays and instrument.phrase_program ~= stays then instrument.phrase_program = stays end
  -- (the program as the drop left it: unless the producer chooses another since, Kumi's undo plays the one it was again)
  local program_after = instrument.phrase_program
  return {
    applied = { deleted = true, count = #items },
    -- (`mark`: the phrases' log once all had gone, which every place to come back to is counted from)
    material = { instrument = places_.place_of(env, "instrument", { i }), phrases = kept, programAfter = program_after,
      mark = env.list_mark and env.list_mark("phrases", i) or nil, program = stays and places_.place_of(env, "phrase", { i, stays }) or nil },
    target = places_.target_of(env, "instrument", { i }), display = { title = title, note = DROP_NOTE }, undo = { desc = desc, entry = true },
    -- (the journal tells Renoise's undo of it by the first phrase coming back: its undo puts them back from the first up)
    prior = { deleted = false }, place = places_.journal_place("phrase", { i, items[1].n[2] }), shape = "deleted",
  }
end

-- Whether the phrase at `at` is the one kept (back by Renoise's undo, or an earlier restore).
local function is_kept(instrument, at, saved)
  if at < 1 or at > #instrument.phrases then return false end
  local there = instrument:phrase(at)
  return there.name == saved.settings.name and there.number_of_lines == saved.settings.lines
    and json.encode(read_lines(there), { sort_keys = true }) == json.encode(saved.lines, { sort_keys = true })
end

-- Where dropped phrase `k` goes back now, or nil when the log can't say: just after the phrase that stayed before it, wherever
-- that is, and after the phrases dropped next to it before it. It counts from one mark taken once all had gone
-- (`material.mark`), so phrases dropped next to each other come back in their order, put back by Kumi or by Renoise's undo.
local function drop_spot(env, i, material, k)
  if not env.list_after then return nil end
  local mark = material.mark or (material.phrases[1] or {}).after
  -- (the phrase that stayed before it was at index - k once the k phrases up to it had gone)
  local anchor = material.phrases[k].index - k
  local at = env.list_after("phrases", i, nil, anchor + 1, mark)
  if not at then return nil end
  for j = 1, k - 1 do
    if material.phrases[j].index - j == anchor then at = at + 1 end
  end
  return at
end

local function drops_restore(song, env, restore, desc)
  local material = restore.material
  if type(material) ~= "table" or type(material.instrument) ~= "table" or type(material.phrases) ~= "table" or #material.phrases == 0 then
    bad("material: what the drop kept")
  end
  local n, why, which = places_.follow(env, material.instrument)
  if not n then places_.follow_failed(why, which, nil) end
  local instrument = song:instrument(n[1])
  local count = #material.phrases
  local title = count == 1 and "Phrase put back" or (count .. " phrases put back")
  local spots = {}
  for k = 1, count do
    spots[k] = drop_spot(env, n[1], material, k)
    if not spots[k] then fail("stale", "the phrases changed in a way Kumi couldn't follow since the drop: nothing was put back") end
  end
  -- (back already: each where it goes, as it was)
  local back = 0
  for k, saved in ipairs(material.phrases) do
    if is_kept(instrument, spots[k], saved) then back = back + 1 end
  end
  if back == count then
    for k = 1, count do base.note_restored_at(env.journal, "phrases-drop", places_.journal_place("phrase", { n[1], spots[k] })) end
    return { restored = { done = true }, display = { title = title .. " already" }, undo = { desc = desc, entry = false }, unwritten = true }
  end
  if back > 0 then fail("changed", "some of the phrases are back in the instrument already and some aren't: Kumi left them as they are") end
  if #instrument.phrases + count > 126 then fail("changed", "the instrument has no room for them any more (126 phrases at most)") end
  song:describe_undo(desc)
  local program_at
  -- (from the first up, each at its spot once the ones before it are back)
  for k, saved in ipairs(material.phrases) do
    local at = math.max(1, math.min(drop_spot(env, n[1], material, k) or spots[k], #instrument.phrases + 1))
    local phrase = instrument:insert_phrase_at(at)
    pcall(write_settings, phrase, saved.settings)
    write_lines(phrase, saved.lines)
    for key, name in pairs((saved.columns or {}).names or {}) do pcall(function() phrase:set_column_name(tonumber(key), name) end) end
    for _, c in ipairs((saved.columns or {}).muted or {}) do pcall(function() phrase:set_column_is_muted(c, true) end) end
    if env.back then env.back("phrases", n[1], nil, saved.after, at) end
    if env.adopt and saved.logs then env.adopt("phrase", { n[1], at }, saved.logs) end
    base.note_restored_at(env.journal, "phrases-drop", places_.journal_place("phrase", { n[1], at }))
    if saved.program then program_at = at end
  end
  -- (the program the drop moved, back on the phrase it played, unless the producer chose another since: a dropped one, or one
  -- that stayed, which the phrases put back below it moved)
  local program_on = program_at
  if not program_on and type(material.program) == "table" then
    local now = places_.follow(env, material.program)
    program_on = now and now[2] or nil
  end
  if program_on then
    pcall(function()
      local program = instrument.phrase_program
      if instrument.phrase_playback_mode == renoise.Instrument.PHRASES_PLAY_SELECTIVE and program ~= program_on
        and (program == 0 or program > #instrument.phrases or (material.programAfter ~= nil and program == material.programAfter)) then
        instrument.phrase_program = program_on
      end
    end)
  end
  return { restored = { done = true }, display = { title = title }, undo = { desc = desc, entry = true } }
end

base.register({ kind = "phrases-drop", custom = true, shape = "deleted", by_place = true, preview = drops_preview, apply = drops_apply, restore = drops_restore })

-- phrase-map {ref: phrase, noteRange?: [low, high] | null, keyTracking?, baseNote?, looping?, loopStart?, loopEnd?}: the phrase's
-- keys in keymap mode (mappings may not overlap) and the mapping's own key tracking, base note and loop, which are its own, not
-- the phrase's (lab: setting one leaves the other).

local MAPPING_FIELDS = {
  keyTracking = { get = function(m) return KEY_TRACKING.get(m.key_tracking) end, set = function(m, v) m.key_tracking = KEY_TRACKING.parse(v) end,
    check = function(v) return KEY_TRACKING.parse(v) ~= nil end, says = "keyTracking is none, transpose or offset" },
  baseNote = { get = function(m) return m.base_note end, set = function(m, v) m.base_note = v end, check = whole(0, 119), says = "baseNote is a note from 0 to 119" },
  looping = { get = function(m) return m.looping end, set = function(m, v) m.looping = v end, check = is_boolean, says = "looping is true or false" },
  -- (lines by Renoise's numbers, from 0)
  loopStart = { get = function(m) return m.loop_start - 1 end, set = function(m, v) m.loop_start = v + 1 end, check = whole(0, 511), says = "loopStart is a line of the phrase, from 0" },
  loopEnd = { get = function(m) return m.loop_end - 1 end, set = function(m, v) m.loop_end = v + 1 end, check = whole(0, 511), says = "loopEnd is a line of the phrase, from 0" },
}
local MAPPING_ORDER = { "keyTracking", "baseNote", "looping", "loopStart", "loopEnd" }
ALL_MAPPING = {}
for _, key in ipairs(MAPPING_ORDER) do ALL_MAPPING[key] = true end

local function same_range(a, b)
  if not a or not b then return (not a) == (not b) end
  return a[1] == b[1] and a[2] == b[2]
end

-- The mapping's state for the keys a change names: its range (false when unmapped) and the fields asked.
function map_state(phrase, keys)
  local mapping = phrase.mapping
  local state = { noteRange = mapping and { mapping.note_range[1], mapping.note_range[2] } or false }
  for _, key in ipairs(MAPPING_ORDER) do
    if keys[key] ~= nil and mapping then state[key] = MAPPING_FIELDS[key].get(mapping) end
  end
  return state
end

local function same_map(a, b)
  if not same_range(a.noteRange or nil, b.noteRange or nil) then return false end
  for _, key in ipairs(MAPPING_ORDER) do
    if (a[key] ~= nil or b[key] ~= nil) and a[key] ~= b[key] then return false end
  end
  return true
end

function write_map(instrument, phrase, state)
  local done, why = set_mapping(instrument, phrase, state.noteRange or nil)
  if not done then return false, why end
  local mapping = phrase.mapping
  if mapping then
    -- (the loop's ends in the order that keeps its start before its end)
    if state.loopStart and state.loopEnd and state.loopStart + 1 > mapping.loop_end then
      mapping.loop_end = state.loopEnd + 1
      mapping.loop_start = state.loopStart + 1
      state = (function() local c = {} for k, v in pairs(state) do c[k] = v end c.loopStart, c.loopEnd = nil, nil return c end)()
    end
    for _, key in ipairs(MAPPING_ORDER) do
      if state[key] ~= nil then MAPPING_FIELDS[key].set(mapping, state[key]) end
    end
  end
  return true
end

local function map_plan(song, env, input)
  local n, objects, label = places_.locate(song, env, "phrase", input, "phrase-map")
  local phrase = objects.phrase
  local current = map_state(phrase, input)
  local wanted = {}
  local range = input.noteRange
  if range == nil then
    wanted.noteRange = current.noteRange
  elseif range == json.null or range == false then
    wanted.noteRange = false
  elseif range_ok(range) then
    wanted.noteRange = { range[1], range[2] }
  else
    bad("noteRange is [low, high], notes from 0 to 119 (C-4 is 48), or null to unmap it")
  end
  local any = range ~= nil
  for _, key in ipairs(MAPPING_ORDER) do
    if input[key] ~= nil then
      if not MAPPING_FIELDS[key].check(input[key]) then bad(MAPPING_FIELDS[key].says) end
      if not wanted.noteRange then bad("the phrase has no keys: give noteRange to map it (its key tracking, base note and loop are the mapping's)") end
      wanted[key], any = input[key], true
    end
  end
  if not any then bad("give noteRange (or null to unmap), keyTracking, baseNote, looping, loopStart or loopEnd") end
  if wanted.loopStart and wanted.loopEnd and wanted.loopStart > wanted.loopEnd then bad("loopStart must not be after loopEnd") end
  for _, key in ipairs({ "loopStart", "loopEnd" }) do
    if wanted[key] and wanted[key] >= phrase.number_of_lines then bad(string.format("%s is a line of the phrase, from 0 to %d", key, phrase.number_of_lines - 1)) end
  end
  if wanted.noteRange and not mapping_slot(objects.instrument, wanted.noteRange[1], wanted.noteRange[2], current.noteRange and current.noteRange[1] or nil) then
    bad("those keys overlap another phrase's mapping")
  end
  -- (the prior names every field the change sets: what the undo puts back)
  local prior = map_state(phrase, wanted)
  return { n = n, label = label, wanted = wanted, prior = prior, instrument = objects.instrument, phrase = phrase }
end

local function map_title(plan)
  local wanted = plan.wanted
  if not wanted.noteRange then return "Unmapped " .. plan.label end
  local parts = {}
  if not same_range(wanted.noteRange, plan.prior.noteRange or nil) then parts[#parts + 1] = string.format("notes %d-%d", wanted.noteRange[1], wanted.noteRange[2]) end
  if wanted.keyTracking then parts[#parts + 1] = "key tracking " .. wanted.keyTracking end
  if wanted.baseNote then parts[#parts + 1] = "base note " .. wanted.baseNote end
  if wanted.looping ~= nil then parts[#parts + 1] = wanted.looping and "looping" or "one-shot" end
  if wanted.loopStart then parts[#parts + 1] = string.format("loop from line %02d", wanted.loopStart) end
  if wanted.loopEnd then parts[#parts + 1] = string.format("loop to line %02d", wanted.loopEnd) end
  if #parts == 0 then parts[1] = string.format("notes %d-%d", wanted.noteRange[1], wanted.noteRange[2]) end
  return string.format("Mapped %s: %s", plan.label, table.concat(parts, ", "))
end

base.register({
  kind = "phrase-map", custom = true, by_place = true,
  preview = function(song, env, input)
    local plan = map_plan(song, env, input)
    local display = { title = map_title(plan), before = plan.prior, after = plan.wanted }
    if same_map(plan.prior, plan.wanted) then display.unchanged = true end
    return { target = places_.target_of(env, "phrase", plan.n), prior = plan.prior, fence = plan.prior, display = display }
  end,
  apply = function(song, env, input, fence, desc)
    local ok, plan = pcall(map_plan, song, env, input)
    if not ok then base.gone_is_changed(plan) end
    if not same_map(plan.prior, fence) then fail("changed", plan.label .. "'s mapping changed since the preview") end
    local display = { title = map_title(plan), before = plan.prior, after = plan.wanted }
    if same_map(plan.prior, plan.wanted) then
      display.unchanged = true
      return { applied = plan.prior, display = display, undo = { desc = desc, entry = false }, prior = plan.prior, target = places_.target_of(env, "phrase", plan.n) }
    end
    song:describe_undo(desc)
    local done, why = write_map(plan.instrument, plan.phrase, plan.wanted)
    if not done then fail("failed", "Renoise didn't map it: " .. tostring(why)) end
    local now = map_state(plan.phrase, plan.wanted)
    if env.watch_undo then
      env.watch_undo(plan.instrument.phrase_mappings_observable)
      local mapping = plan.phrase.mapping
      if mapping then
        for _, field in ipairs({ "note_range", "key_tracking", "base_note", "looping", "loop_start", "loop_end" }) do env.watch_undo(mapping[field .. "_observable"]) end
      end
    end
    return {
      applied = now, target = places_.target_of(env, "phrase", plan.n), display = display,
      undo = { desc = desc, entry = true }, prior = plan.prior, place = places_.journal_place("phrase", plan.n),
    }
  end,
  restore = function(song, env, restore, desc)
    local prior, applied = restore.prior, restore.applied
    if type(prior) ~= "table" or type(applied) ~= "table" then bad("prior and applied: the mapping") end
    local n, objects, label = places_.locate(song, env, "phrase", { place = restore.target and restore.target.place }, "phrase-map")
    local now = map_state(objects.phrase, applied)
    if same_map(now, prior) then
      return { restored = prior, display = { title = label .. "'s mapping is back already" }, undo = { desc = desc, entry = false }, unwritten = true }
    end
    if not restore.discard and not same_map(now, applied) then fail("changed", label .. "'s mapping changed in Renoise since Kumi set it") end
    song:describe_undo(desc)
    local done, why = write_map(objects.instrument, objects.phrase, prior)
    if not done then fail("changed", "its old keys can't be mapped again: " .. tostring(why)) end
    base.note_restored(env.journal, "phrase-map", places_.journal_place("phrase", n), applied, base.KINDS["phrase-map"])
    return { restored = prior, display = { title = label .. "'s mapping put back" }, undo = { desc = desc, entry = true } }
  end,
  reread = places_.reread("phrase"),
  read = function(phrase, shape) return map_state(phrase, shape or {}) end,
  same = same_map,
})

-- phrase-playback {ref: instrument, mode: off | program | keymap, programRef?}: the instrument's phrase playback; in program
-- mode, the phrase every key plays (kept as its place).

local MODES = named({ off = 1, program = 2, keymap = 3 })

local playback = instruments_.settings_kind({
  kind = "phrase-playback", noun = "phrase playback", undoable = true, order = { "mode", "program" },
  fields = {
    mode = { get = function(i) return MODES.get(i.phrase_playback_mode) end, set = function(i, v) i.phrase_playback_mode = MODES.parse(v) end,
      field = "phrase_playback_mode", check = function(v) return MODES.parse(v) ~= nil end, says = "mode is off, program or keymap",
      text = function(v) return v == "off" and "phrases off" or ("phrases by " .. v) end },
    program = { get = function(i) return i.phrase_program end, set = function(i, v) if v >= 1 and v <= 126 then i.phrase_program = v end end, field = "phrase_program",
      text = function(v) return "phrase " .. string.format("%02d", v) end },
  },
})
-- (program mode with no phrase chosen, program 0, is what a new instrument has: lab. A state keeps both as Renoise has them)
playback.read = function(instrument)
  return { mode = MODES.get(instrument.phrase_playback_mode), program = instrument.phrase_program }
end
playback.wanted_at = function(loc, input)
  local instrument = loc.object
  local mode = input.mode
  if MODES.parse(mode) == nil then bad("mode is off, program or keymap") end
  local state = { mode = mode, program = mode == "off" and 0 or 127 }
  if mode == "program" then
    local program
    if input.programRef ~= nil then
      local found = places_.numbers_of(loc.env, "phrase", input.programRef, "programRef")
      if found[1] ~= loc.numbers[1] then bad("programRef must be a phrase of that instrument") end
      program = found[2]
    elseif instrument.phrase_program >= 1 and instrument.phrase_program <= #instrument.phrases then
      program = instrument.phrase_program
    else
      program = 1
    end
    if program > #instrument.phrases then bad("the instrument has no phrase to play: add one first") end
    state.program = program
  elseif input.programRef ~= nil then
    bad("programRef goes with mode program")
  end
  return state
end
playback.wanted = nil
-- (the program first: Renoise sets the mode from it; then the mode, which keeps a program of program mode)
playback.write = function(instrument, state)
  if type(state.program) == "number" and instrument.phrase_program ~= state.program then instrument.phrase_program = state.program end
  local mode = MODES.parse(state.mode)
  if instrument.phrase_playback_mode ~= mode then instrument.phrase_playback_mode = mode end
end
playback.same = function(a, b) return a.mode == b.mode and a.program == b.program end
playback.title = function(name, _, to)
  if to.mode == "program" and type(to.program) == "number" and to.program >= 1 then return string.format("%s plays phrase %02d on every key", name, to.program) end
  if to.mode == "program" then return name .. ": phrases by program, none chosen" end
  return name .. (to.mode == "off" and ": phrases off" or ": phrases by keymap")
end
playback.observables = function(instrument) return { instrument.phrase_playback_mode_observable, instrument.phrase_program_observable } end
-- (the program's phrase on the wire is its place)
playback.wire = function(state, env, loc)
  if type(state) ~= "table" or type(state.program) ~= "number" or state.program < 1 or not loc or not loc.numbers then return state end
  local out = {}
  for k, v in pairs(state) do out[k] = v end
  out.program = { place = places_.place_of(env, "phrase", { loc.numbers[1], state.program }) }
  return out
end
playback.unwire = function(state, env)
  if type(state) ~= "table" or type(state.program) ~= "table" then return state end
  local n, why, which = places_.follow(env, state.program.place)
  if not n then places_.follow_failed(why, which, "the program's phrase") end
  local out = {}
  for k, v in pairs(state) do out[k] = v end
  out.program = n[2]
  return out
end
instruments_.instrument_spec(playback)
do
  local locate = playback.locate
  playback.locate = function(song, env, input)
    local loc = locate(song, env, input)
    loc.env = env
    return loc
  end
end

-- phrase-write {ref: phrase, lines: [...], merge?} and phrase-clear {ref: phrase, first?, last?}: a phrase's cells, as
-- notes-write writes a pattern's (instrument values in a phrase are its samples, counted from 00). The columns the cells need
-- are shown first and hidden again by the undo when nothing else is in them.

local function phrase_columns(phrase)
  return {
    note = phrase.visible_note_columns, effect = phrase.visible_effect_columns, volume = phrase.volume_column_visible,
    panning = phrase.panning_column_visible, delay = phrase.delay_column_visible, sampleEffects = phrase.sample_effects_column_visible,
    instrument = phrase.instrument_column_visible,
  }
end

local function phrase_needs(after)
  local needs = cells.needs_of(after)
  for _, cell in ipairs(after) do
    if cell.kind == "note" and cell.instrument ~= ".." and not cells.is_empty_cell(cell) then needs.instrument = true end
  end
  return needs
end

local function raise(phrase, needs)
  if needs.note > 0 and phrase.visible_note_columns < needs.note then phrase.visible_note_columns = needs.note end
  if needs.effect > 0 and phrase.visible_effect_columns < needs.effect then phrase.visible_effect_columns = needs.effect end
  if needs.volume and not phrase.volume_column_visible then phrase.volume_column_visible = true end
  if needs.panning and not phrase.panning_column_visible then phrase.panning_column_visible = true end
  if needs.delay and not phrase.delay_column_visible then phrase.delay_column_visible = true end
  if needs.sampleEffects and not phrase.sample_effects_column_visible then phrase.sample_effects_column_visible = true end
  if needs.instrument and not phrase.instrument_column_visible then phrase.instrument_column_visible = true end
end

local function set_phrase_columns(phrase, columns)
  pcall(function()
    if columns.note and phrase.visible_note_columns ~= columns.note then phrase.visible_note_columns = columns.note end
    if columns.effect and phrase.visible_effect_columns ~= columns.effect then phrase.visible_effect_columns = columns.effect end
    for key, field in pairs({ volume = "volume_column_visible", panning = "panning_column_visible", delay = "delay_column_visible",
      sampleEffects = "sample_effects_column_visible", instrument = "instrument_column_visible" }) do
      if columns[key] ~= nil and phrase[field] ~= columns[key] then phrase[field] = columns[key] end
    end
  end)
end

-- What is in the phrase's columns (every line): the highest note and effect columns used, and the sub-columns with something.
local function used_columns(phrase)
  local used = { note = 0, effect = 0 }
  for l = 1, phrase.number_of_lines do
    local line = phrase:line(l)
    if not line.is_empty then
      for c = 1, 12 do
        local column = line:note_column(c)
        if not column.is_empty then
          if c > used.note then used.note = c end
          if column.volume_string ~= ".." then used.volume = true end
          if column.panning_string ~= ".." then used.panning = true end
          if column.delay_string ~= ".." then used.delay = true end
          if column.instrument_string ~= ".." then used.instrument = true end
          if column.effect_number_value ~= 0 or column.effect_amount_value ~= 0 then used.sampleEffects = true end
        end
      end
      for c = 1, 8 do
        if not line:effect_column(c).is_empty and c > used.effect then used.effect = c end
      end
    end
  end
  return used
end

-- A new phrase's lines (phrase-add's content): written, the columns they need shown, and read back as written.
write_content = function(phrase, wants)
  local _, after = cells.plan_of(phrase, wants, false)
  raise(phrase, phrase_needs(after))
  for _, cell in ipairs(after) do cells.write_cell(phrase, cell) end
  for _, cell in ipairs(after) do
    if not cells.same_cell(cells.read_cell(phrase, cell), cell) then error(string.format("Renoise didn't keep line %02d column %d as written", cell.line - 1, cell.col), 0) end
  end
end

-- (`on_write`, when given, runs just before anything is written: a restore that found the lines back names its step there)
local function lower(phrase, prior, applied, on_write)
  if type(prior) ~= "table" or type(applied) ~= "table" then return end
  local now, used = phrase_columns(phrase), used_columns(phrase)
  local back = {}
  if prior.note and now.note == applied.note and applied.note > prior.note then back.note = math.max(prior.note, used.note, 1) end
  if prior.effect and now.effect == applied.effect and applied.effect > prior.effect then back.effect = math.max(prior.effect, used.effect) end
  for _, flag in ipairs({ "volume", "panning", "delay", "sampleEffects", "instrument" }) do
    if prior[flag] == false and applied[flag] == true and now[flag] == true and not used[flag] then back[flag] = false end
  end
  if next(back) then
    if on_write then on_write() end
    set_phrase_columns(phrase, back)
    return true
  end
end

-- What a request's lines name in the phrase: the cells and what they get (cells.parse_cells takes a pattern and a track:
-- a phrase has its lines and 12 note and 8 effect columns).
local function lines_plan(phrase, input)
  local shape_pattern = { number_of_lines = phrase.number_of_lines }
  local shape_track = { max_note_columns = 12, max_effect_columns = 8 }
  local wants, _, read_as = cells.parse_cells(shape_pattern, shape_track, input)
  cells.add_cleared(phrase, input, wants)
  local prior, after = cells.plan_of(phrase, wants, input.merge == true)
  if cells.writes_of(after) > M.MAX_WRITES then
    bad(string.format("that is %d writes, more than one phrase change takes (%d): split it into several", cells.writes_of(after), M.MAX_WRITES))
  end
  return prior, after, read_as
end

local function lines_title(verb, cells_, label)
  local n = cells.lines_in(cells_)
  return string.format("%s %s in %s", verb, n == 1 and "1 line" or (n .. " lines"), label)
end

local function clear_input(phrase, input)
  local first, last = cells.line_range(input, phrase.number_of_lines)
  local lines = {}
  -- (as phrase-write takes them: by Renoise's numbers)
  for l = first, last do
    if not phrase:line(l).is_empty then lines[#lines + 1] = { line = l - 1, clear = true } end
  end
  return { lines = lines }
end

local function lines_kind(spec)
  local kind, verb, noun, build = spec.kind, spec.verb, spec.noun, spec.build
  local function plan(song, env, input)
    local n, objects, label = places_.locate(song, env, "phrase", input, kind)
    local phrase = objects.phrase
    local request = build(phrase, input)
    local prior, after, read_as
    if #(request.lines or {}) == 0 and kind == "phrase-clear" then
      prior, after = {}, {}
    else
      prior, after, read_as = lines_plan(phrase, request)
    end
    return { n = n, label = label, phrase = phrase, prior = prior, after = after, columns = phrase_columns(phrase), note = cells.read_as_note(read_as or {}) }
  end
  local function preview(song, env, input)
    local p = plan(song, env, input)
    local display = { title = lines_title(verb, #p.after > 0 and p.after or p.prior, p.label), before = cells.counts_of(p.prior), after = cells.counts_of(p.after), note = p.note }
    if #p.after == 0 then display.unchanged = true end
    return {
      target = places_.target_of(env, "phrase", p.n), prior = { cells = p.prior, columns = p.columns },
      fence = { cells = #p.prior, hash = cells.fingerprint(p.prior) }, display = display,
    }
  end
  local function apply(song, env, input, fence, desc)
    local ok, p = pcall(plan, song, env, input)
    if not ok then base.gone_is_changed(p) end
    if #p.prior ~= fence.cells or cells.fingerprint(p.prior) ~= fence.hash then fail("changed", p.label .. " changed since the preview") end
    local display = { title = lines_title(verb, #p.after > 0 and p.after or p.prior, p.label), before = cells.counts_of(p.prior), after = cells.counts_of(p.after) }
    if #p.after == 0 then
      display.unchanged = true
      return { applied = { cells = {}, columns = p.columns }, display = display, undo = { desc = desc, entry = false }, prior = { cells = p.prior, columns = p.columns }, target = places_.target_of(env, "phrase", p.n) }
    end
    song:describe_undo(desc)
    local written = 0
    local wrote, err = pcall(function()
      raise(p.phrase, phrase_needs(p.after))
      for i, cell in ipairs(p.after) do
        cells.write_cell(p.phrase, cell)
        written = i
      end
    end)
    if not wrote then
      pcall(function() for i = 1, written + 1 do if p.prior[i] then cells.write_cell(p.phrase, p.prior[i]) end end end)
      set_phrase_columns(p.phrase, p.columns)
      fail("failed", "Renoise wouldn't take those cells: " .. tostring(type(err) == "table" and err.message or err):gsub("^[^%s:]*%.lua:%d+: ", ""))
    end
    local result = {}
    for i, cell in ipairs(p.after) do result[i] = cells.read_cell(p.phrase, cell) end
    for i, cell in ipairs(result) do
      if not cells.same_cell(cell, p.after[i]) then
        for j = 1, #p.after do cells.write_cell(p.phrase, p.prior[j]) end
        set_phrase_columns(p.phrase, p.columns)
        fail("failed", string.format("Renoise didn't keep line %02d column %d as written", cell.line - 1, cell.col))
      end
    end
    return {
      applied = {
        cells = result, columns = phrase_columns(p.phrase),
        touch = env.phrase_touch_mark and env.phrase_touch_mark(p.n[1], p.n[2]) or nil, lines = p.phrase.number_of_lines,
      },
      target = places_.target_of(env, "phrase", p.n), display = { title = display.title, before = display.before, after = cells.counts_of(result), note = p.note },
      undo = { desc = desc, entry = true }, prior = { cells = p.prior, columns = p.columns }, place = places_.journal_place("phrase", p.n),
    }
  end
  local function restore(song, env, restore_, desc)
    local prior, applied = restore_.prior and restore_.prior.cells, restore_.applied and restore_.applied.cells
    if type(prior) ~= "table" or type(applied) ~= "table" or #prior ~= #applied then bad("prior and applied: the cells the change wrote") end
    local n, objects, label = places_.locate(song, env, "phrase", { place = restore_.target and restore_.target.place }, kind)
    local phrase = objects.phrase
    if #applied > 0 then
      if type(restore_.applied.lines) == "number" and restore_.applied.lines ~= phrase.number_of_lines then
        fail("changed", label .. "'s length changed in Renoise since Kumi wrote these lines, so Kumi left them as they are")
      end
      local since = env.phrase_touched_since and env.phrase_touched_since(n[1], n[2], restore_.applied.touch)
      if since == nil then fail("changed", "Kumi can't tell whether " .. label .. " changed in Renoise since (the tool watched the song anew), so Kumi left its lines as they are") end
      if since then
        fail("changed", label .. " changed in Renoise since Kumi's step (an edit, an undo or redo, or another change of Kumi's), so Kumi left its lines as they are: Renoise's own undo can take them back, after the edits made since")
      end
    end
    local todo = {}
    for i = 1, #prior do
      local now = cells.read_cell(phrase, prior[i])
      if not cells.same_cell(now, prior[i]) then
        if not cells.same_cell(now, applied[i]) then fail("changed", string.format("line %02d of %s was changed in Renoise since Kumi wrote it", prior[i].line - 1, label)) end
        todo[#todo + 1] = prior[i]
      end
    end
    local place = places_.journal_place("phrase", n)
    if #todo == 0 then
      -- (the lines are back; hiding the columns Kumi showed is a step of Renoise's undo, named as Kumi's)
      local hid = lower(phrase, restore_.prior.columns, restore_.applied.columns, function() song:describe_undo(desc) end)
      base.note_restored(env.journal, kind, place, restore_.applied, base.KINDS[kind])
      if hid then
        return { restored = { done = true }, display = { title = noun .. " in " .. label .. " are back already: the columns Kumi showed are hidden again" }, undo = { desc = desc, entry = true } }
      end
      return { restored = { done = true }, display = { title = noun .. " in " .. label .. " are back already" }, undo = { desc = desc, entry = false }, unwritten = true }
    end
    song:describe_undo(desc)
    for _, cell in ipairs(todo) do cells.write_cell(phrase, cell) end
    lower(phrase, restore_.prior.columns, restore_.applied.columns)
    base.note_restored(env.journal, kind, place, restore_.applied, base.KINDS[kind])
    return { restored = { done = true }, display = { title = noun .. " put back in " .. label, before = cells.counts_of(todo), after = cells.counts_of(prior) }, undo = { desc = desc, entry = true } }
  end
  base.register({
    kind = kind, custom = true, by_place = true, preview = preview, apply = apply, restore = restore, unsure_redo = true,
    reread = places_.reread("phrase"),
    read = function(phrase, shape)
      local out = {}
      for i, at in ipairs(shape.cells or {}) do out[i] = cells.read_cell(phrase, at) end
      return { cells = out }
    end,
    same = function(a, b) return cells.same_cells(a.cells, b.cells) end,
  })
end

lines_kind({ kind = "phrase-write", verb = "Wrote", noun = "Lines", build = function(_, input) return input end })
lines_kind({ kind = "phrase-clear", verb = "Cleared", noun = "Cells", build = clear_input })

-- phrase-script {ref: phrase, code, commit?}: the script's text (as its lines), committed for playback unless commit: false;
-- the phrase plays its script. Renoise compiles a committed script in its player, a moment later: the apply waits for that
-- (a job; the one write is its first step) and says what Renoise reported.

local function split_lines(code)
  local out = {}
  for line in (code .. "\n"):gmatch("([^\n]*)\n") do out[#out + 1] = (line:gsub("\r$", "")) end
  if #out > 1 and out[#out] == "" then out[#out] = nil end
  return out
end

local function script_state(phrase)
  local script = phrase.script
  return { mode = PLAYBACK.get(phrase.playback_mode), paragraphs = script.paragraphs }
end

local function same_paragraphs(a, b)
  if type(a) ~= "table" or type(b) ~= "table" or #a ~= #b then return false end
  for k = 1, #a do if a[k] ~= b[k] then return false end end
  return true
end

local function script_plan(song, env, input)
  local n, objects, label = places_.locate(song, env, "phrase", input, "phrase-script")
  if type(input.code) ~= "string" or input.code == "" then bad("code (the script's text: pattrns Lua, returning a pattern or cycle) or path (a .lua phrase script file)") end
  if #input.code > M.MAX_SCRIPT_BYTES then bad("code is longer than 64 KB") end
  if input.commit ~= nil and type(input.commit) ~= "boolean" then bad("commit is true or false") end
  -- (`codeName`: the name of the phrase script file Kumi's host read it from, M4 n-devices)
  local name = type(input.codeName) == "string" and input.codeName ~= "" and input.codeName or nil
  return { n = n, label = label, phrase = objects.phrase, paragraphs = split_lines(input.code), state = script_state(objects.phrase), commit = input.commit ~= false, name = name }
end

local function script_title(plan)
  local lines = string.format("%d %s", #plan.paragraphs, #plan.paragraphs == 1 and "line" or "lines")
  if plan.name then return string.format("Loaded the phrase script %s into %s (%s)", q(plan.name), plan.label, lines) end
  return string.format("Wrote the script of %s (%s)", plan.label, lines)
end

base.register({
  kind = "phrase-script", custom = true, by_place = true,
  preview = function(song, env, input)
    local plan = script_plan(song, env, input)
    local display = { title = script_title(plan), note = plan.commit and "Renoise compiles it as it is committed and says if it can't." or "Not committed: Renoise plays the script it had until it is." }
    if plan.state.mode == "script" and same_paragraphs(plan.state.paragraphs, plan.paragraphs) and plan.commit == false then display.unchanged = true end
    return { target = places_.target_of(env, "phrase", plan.n), prior = plan.state, fence = plan.state, display = display }
  end,
  apply = function(song, env, input, fence, desc)
    local ok, plan = pcall(script_plan, song, env, input)
    if not ok then base.gone_is_changed(plan) end
    if plan.state.mode ~= fence.mode or not same_paragraphs(plan.state.paragraphs, fence.paragraphs) then fail("changed", plan.label .. "'s script changed since the preview") end
    local phrase = plan.phrase
    local script = phrase.script
    local before = { compile = script.compile_error, runtime = script.runtime_error }
    song:describe_undo(desc)
    M.set_playback(phrase, renoise.InstrumentPhrase.PLAY_SCRIPT)
    script.paragraphs = plan.paragraphs
    if plan.commit then script:commit() end
    if env.watch_undo then env.watch_undo(script.paragraphs_assignment_observable) env.watch_undo(script.paragraphs_observable) env.watch_undo(phrase.playback_mode_observable) end
    local n = plan.n
    -- (`live`: the phrase where it is now)
    local function answer(compile, runtime, waited, live)
      local applied = script_state(live)
      applied.compileError = compile ~= "" and compile or json.null
      applied.runtimeError = runtime ~= "" and runtime or json.null
      local note
      if compile ~= "" then note = "Renoise couldn't compile it: " .. compile
      elseif runtime ~= "" then note = "It compiled, and fails as it runs: " .. runtime
      elseif plan.commit and not waited then note = "Renoise hadn't compiled it yet when Kumi looked: it says errors in the phrase editor." end
      return {
        applied = applied, target = places_.target_of(env, "phrase", n),
        display = { title = script_title(plan), note = note }, undo = { desc = desc, entry = true },
        prior = plan.state, journal_applied = { mode = applied.mode, paragraphs = applied.paragraphs }, place = places_.journal_place("phrase", n),
      }
    end
    if not plan.commit then return answer("", "", true, phrase) end
    -- (the compile lands a moment later: Kumi waits, writing nothing more. The change is written already, so what the producer
    -- does meanwhile never makes it answer as if nothing was: the wait checks nothing, and the phrase is found by its place)
    local at = places_.place_of(env, "phrase", n)
    return {
      -- (written already: the job runs whatever the producer does before its first turn)
      kumi_wrote = "The script of " .. plan.label,
      kumi_job = function(job)
        job.wrote = "The script of " .. plan.label
        local function live_phrase()
          local now = places_.follow(env, at)
          if not now then return nil end
          local found, objects = pcall(places_.resolve, renoise.song() or song, "phrase", now)
          if not found or not objects.phrase then return nil end
          n = now
          return objects.phrase
        end
        local started, waited = env.now_ms(), false
        while env.now_ms() - started < M.COMPILE_WAIT_MS do
          local resume = env.now_ms() + 50
          repeat coroutine.yield() until env.now_ms() >= resume
          local live = live_phrase()
          if not live then break end
          local script_now = live.script
          -- (an error that changed is the new script's; errors that stay as they were are only trusted after the whole wait: the
          -- lab saw a compile's errors within 0.5 s)
          if script_now.pending_changes == 0 and (script_now.compile_error ~= before.compile or script_now.runtime_error ~= before.runtime) then
            waited = true
            break
          end
        end
        if not waited and env.now_ms() - started >= M.COMPILE_WAIT_MS then waited = true end
        job.rebase()
        local live = live_phrase()
        if not live then
          error({ code = "uncertain", message = "Kumi wrote the script of " .. plan.label .. ", and the song changed while Renoise compiled it so that Kumi can't find the phrase now: look at it in Renoise" }, 0)
        end
        return answer(live.script.compile_error, live.script.runtime_error, waited, live)
      end,
    }
  end,
  restore = function(song, env, restore, desc)
    local prior, applied = restore.prior, restore.applied
    if type(prior) ~= "table" or type(applied) ~= "table" then bad("prior and applied: the script") end
    local n, objects, label = places_.locate(song, env, "phrase", { place = restore.target and restore.target.place }, "phrase-script")
    local phrase = objects.phrase
    local now = script_state(phrase)
    if now.mode == prior.mode and same_paragraphs(now.paragraphs, prior.paragraphs) then
      -- (Renoise's own undo puts the text back uncommitted, lab: the player keeps Kumi's script until it is committed)
      if (phrase.script.pending_changes or 0) > 0 then
        song:describe_undo(desc)
        phrase.script:commit()
        base.note_restored(env.journal, "phrase-script", places_.journal_place("phrase", n), { mode = applied.mode, paragraphs = applied.paragraphs }, base.KINDS["phrase-script"])
        return {
          restored = { done = true }, undo = { desc = desc, entry = true },
          display = { title = label .. "'s script put back", note = "Its text was back (Renoise's undo), but not committed, so Kumi's script still played: Kumi committed it." },
        }
      end
      return { restored = { done = true }, display = { title = label .. "'s script is back already" }, undo = { desc = desc, entry = false }, unwritten = true }
    end
    if not restore.discard and (now.mode ~= applied.mode or not same_paragraphs(now.paragraphs, applied.paragraphs)) then
      fail("changed", label .. "'s script changed in Renoise since Kumi wrote it")
    end
    song:describe_undo(desc)
    phrase.script.paragraphs = prior.paragraphs
    phrase.script:commit()
    local mode = PLAYBACK.parse(prior.mode)
    if mode then M.set_playback(phrase, mode) end
    base.note_restored(env.journal, "phrase-script", places_.journal_place("phrase", n), { mode = applied.mode, paragraphs = applied.paragraphs }, base.KINDS["phrase-script"])
    return { restored = { done = true }, display = { title = label .. "'s script put back" }, undo = { desc = desc, entry = true } }
  end,
  reread = places_.reread("phrase"), read = function(phrase) local s = script_state(phrase) return { mode = s.mode, paragraphs = s.paragraphs } end,
  same = function(a, b) return a.mode == b.mode and same_paragraphs(a.paragraphs, b.paragraphs) end,
})

-- phrase-render {ref: phrase, lpb?, maxEvents?}: the committed script rendered to the phrase's lines (Renoise's render, which
-- answers later); the phrase plays its lines after. Kumi keeps the lines it had and puts them back on undo.

local function render_plan(song, env, input)
  local n, objects, label = places_.locate(song, env, "phrase", input, "phrase-render")
  local phrase = objects.phrase
  if phrase.playback_mode ~= renoise.InstrumentPhrase.PLAY_SCRIPT then bad(label .. " plays its lines, not a script: write its script first (phrase-script)") end
  if phrase.script.compile_error ~= "" then bad(label .. "'s script doesn't compile: " .. phrase.script.compile_error) end
  if input.lpb ~= nil and not whole(1, 256)(input.lpb) then bad("lpb is from 1 to 256") end
  if input.maxEvents ~= nil and not whole(1, 65536)(input.maxEvents) then bad("maxEvents is from 1 to 65536") end
  local lines, writes = read_lines(phrase)
  if writes > M.MAX_WRITES then bad(label .. " holds more lines than Kumi puts back in one step: clear it first") end
  return { n = n, label = label, phrase = phrase, lines = lines, settings = settings_of(phrase) }
end

base.register({
  kind = "phrase-render", custom = true, by_place = true,
  preview = function(song, env, input)
    local plan = render_plan(song, env, input)
    return {
      target = places_.target_of(env, "phrase", plan.n), prior = { lines = plan.lines, settings = plan.settings },
      fence = { print = lines_print(plan.phrase), mode = plan.phrase.playback_mode },
      display = { title = "Rendered the script of " .. plan.label .. " to its lines", note = "Kumi keeps the lines it had, to put back." },
    }
  end,
  apply = function(song, env, input, fence, desc)
    local ok, plan = pcall(render_plan, song, env, input)
    if not ok then base.gone_is_changed(plan) end
    if lines_print(plan.phrase) ~= fence.print or plan.phrase.playback_mode ~= fence.mode then fail("changed", plan.label .. " changed since the preview") end
    local n = plan.n
    local options = { lpb = input.lpb, max_events = input.maxEvents }
    -- (the render starts in the job's first step, checked as it starts: Renoise rewrites the phrase, its length too, before it
    -- answers, so the wait after it looks for no other change, and the phrase is found by its place after it)
    return {
      kumi_job = function(job)
        local now_song = renoise.song() or song
        local phrase = places_.resolve(now_song, "phrase", n).phrase
        if lines_print(phrase) ~= fence.print or phrase.playback_mode ~= fence.mode then
          error({ code = "changed", message = plan.label .. " changed since the preview" }, 0)
        end
        local at = places_.place_of(env, "phrase", n)
        local result
        -- (the render switches the phrase to play its lines: Renoise's undo of it switches it back, which the journal hears)
        if env.watch_undo then env.watch_undo(phrase.playback_mode_observable) env.watch_undo(phrase.number_of_lines_observable) end
        now_song:describe_undo(desc)
        local started, accepted, why = pcall(function()
          return phrase.script:render_to_pattern(options, function(error_text, rendered, skipped)
            result = { error = error_text, rendered = rendered, skipped = skipped }
          end)
        end)
        if not started then error({ code = "failed", message = "Renoise wouldn't render it: " .. tostring(type(accepted) == "table" and accepted.message or accepted) }, 0) end
        -- (Renoise refusing at once answers false and why: nothing was rendered, and nothing waits)
        if accepted == false and not result then error({ code = "failed", message = "Renoise wouldn't render it: " .. tostring(why or "it refused") }, 0) end
        job.wrote = "The render of " .. plan.label
        local from = env.now_ms()
        while not result do
          if env.now_ms() - from >= M.RENDER_WAIT_MS then
            error({ code = "uncertain", message = "Renoise didn't say it had rendered " .. plan.label .. " in time: look at its lines; Kumi keeps what it had" }, 0)
          end
          local resume = env.now_ms() + 50
          repeat coroutine.yield() until env.now_ms() >= resume
        end
        job.rebase()
        if result.error then error({ code = "failed", message = "Renoise couldn't render " .. plan.label .. ": " .. tostring(result.error) }, 0) end
        local now, why = places_.follow(env, at)
        if not now then
          error({ code = "uncertain", message = "Renoise rendered " .. plan.label .. ", and the song changed meanwhile (" .. tostring(why) .. "): look at its lines" }, 0)
        end
        n = now
        phrase = places_.resolve(renoise.song() or song, "phrase", n).phrase
        local display = { title = string.format("Rendered the script of %s to %d events", plan.label, result.rendered or 0) }
        if (result.skipped or 0) > 0 then display.note = string.format("%d events didn't fit its lines (raise lpb to fit them).", result.skipped) end
        local applied = { print = lines_print(phrase), mode = phrase.playback_mode, rendered = result.rendered, skipped = result.skipped }
        return {
          applied = applied,
          material = { at = places_.place_of(env, "phrase", n), lines = plan.lines, settings = plan.settings, print = fence.print, mode = fence.mode },
          target = places_.target_of(env, "phrase", n), display = display, undo = { desc = desc, entry = true },
          -- (the journal compares the phrase's lines and playback mode with what they were and what the render left)
          prior = { lines = plan.lines, settings = plan.settings, print = fence.print, mode = fence.mode },
          journal_applied = { print = applied.print, mode = applied.mode }, place = places_.journal_place("phrase", n),
        }
      end,
    }
  end,
  restore = function(song, env, restore, desc)
    local material, applied = restore.material, restore.applied
    if type(material) ~= "table" or type(material.at) ~= "table" or type(applied) ~= "table" then bad("material: what the render replaced") end
    local n, why, which = places_.follow(env, material.at)
    if not n then places_.follow_failed(why, which, nil) end
    local objects = places_.resolve(song, "phrase", n)
    local phrase, label = objects.phrase, places_.label_of("phrase", n, objects)
    local now = lines_print(phrase)
    -- (Renoise's own undo of the render brought its lines and mode back already)
    if material.print ~= nil and now == material.print and phrase.playback_mode == material.mode then
      base.note_restored(env.journal, "phrase-render", places_.journal_place("phrase", n), { print = applied.print, mode = applied.mode }, base.KINDS["phrase-render"])
      return { restored = { done = true }, display = { title = label .. "'s lines are back already" }, undo = { desc = desc, entry = false }, unwritten = true }
    end
    if not restore.discard and (now ~= applied.print or phrase.playback_mode ~= applied.mode) then fail("changed", label .. " changed in Renoise since Kumi rendered it") end
    song:describe_undo(desc)
    phrase:clear()
    write_lines(phrase, material.lines)
    pcall(write_settings, phrase, material.settings)
    base.note_restored(env.journal, "phrase-render", places_.journal_place("phrase", n), { print = applied.print, mode = applied.mode }, base.KINDS["phrase-render"])
    return { restored = { done = true }, display = { title = label .. "'s lines put back as they were before the render" }, undo = { desc = desc, entry = true } }
  end,
  reread = places_.reread("phrase"),
  read = function(phrase) return { print = lines_print(phrase), mode = phrase.playback_mode } end,
  same = function(a, b) return a.print == b.print and a.mode == b.mode end,
})

return M
