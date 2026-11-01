-- Kumi's changes to the lines of a pattern: the note and effect columns of one track in one pattern (notes-write, notes-clear).
-- Cells are written and read as Renoise's own text ("C-4", "01", "40", ".."), cell by cell, so a restore puts back exactly the
-- cells Kumi changed and leaves the rest of the pattern to the producer.

local import = ...
local refs = import("refs")
local rows = import("rows")
local base = import("changes_base")
local material_ = import("changes_material")

local bad, fail, the_song = base.bad, base.fail, base.the_song
local q = base.q
local M = {}

-- The most writes one change makes: about a second of Renoise's time, the callback's budget (a whole 512-line pattern of 12 note
-- columns is 43,000: two changes).
M.MAX_WRITES = base.MAX_APPLY_WRITES
local NOTE_WRITES, EFFECT_WRITES = 7, 2

local NOTE_FIELDS = { "note", "instrument", "volume", "panning", "delay", "fx" }
local EMPTY_NOTE = { note = "---", instrument = "..", volume = "..", panning = "..", delay = "..", fx = "0000" }
local EMPTY_EFFECT = { number = "00", amount = "00" }

-- Text, as the tracker writes it

local function text_of(value, name, shape)
  if type(value) ~= "string" then bad(name .. ": text such as " .. shape) end
  return value:upper()
end

-- The delay column: 00 is no delay, FF is 255 (the latest a note can be delayed), unlike the instrument's FF (none).
local function delay_text(value)
  if value == "" or value == ".." then return ".." end
  if type(value) == "number" then
    if value < 0 or value > 255 or value ~= math.floor(value) then bad("delay: a whole number from 0 to 255 (decimal: 128 is half a line), or two hex digits as Renoise shows them (\"80\")") end
    return value == 0 and ".." or string.format("%02X", value)
  end
  local text = text_of(value, "delay", "10 or ..")
  if not text:match("^%x%x$") then bad("delay: two hex digits (00 to FF), or .. for none; got " .. value) end
  return text == "00" and ".." or text
end

local function hex2(value, name, empty)
  if value == "" or value == ".." then return empty end
  if type(value) == "number" then
    -- (255 is the instrument's "none"; an effect's amount goes to FF)
    local top = empty == ".." and 254 or 255
    if value < 0 or value > top or value ~= math.floor(value) then
      bad(string.format("%s: a whole number from 0 to %d (decimal: 10 is 0A), or two hex digits as Renoise shows them", name, top))
    end
    return string.format("%02X", value)
  end
  local text = text_of(value, name, "01 or ..")
  if not text:match("^%x%x$") then bad(name .. ": two hex digits (00 to FE), or .. for none; got " .. value) end
  if text == "FF" and empty == ".." then return ".." end -- (255 is Renoise's "none" for these)
  return text
end

local function note_text(value)
  local text = text_of(value, "note", "C-4, C#5 or OFF")
  if text == "" or text == "---" then return "---" end
  if text == "OFF" then return "OFF" end
  local letter, sep, octave = text:match("^([A-G])([-#]?)(%d)$")
  if not letter then bad("note: C-4, C#5, OFF or --- (a note name, # or - and an octave 0 to 9); got " .. value) end
  if sep == "" then sep = "-" end
  if sep == "#" and (letter == "E" or letter == "B") then bad("note: there is no " .. text .. " (E and B have no sharp)") end
  return letter .. sep .. octave
end

-- The volume and panning columns take a level (00 to 80) or a command (a letter and a hex digit, such as G3); FF is none.
local function column_text(value, name)
  if value == "" or value == ".." then return ".." end
  local text = text_of(value, name, "40 or ..")
  if not text:match("^%w%x$") then bad(name .. ": two characters such as 40 or G3 (the second a hex digit), or .. for none; got " .. value) end
  if text == "FF" then return ".." end
  return text
end

-- Older spellings of Renoise's sample offset, 0Sxx: 09xx (early Renoise's and ProTracker's), Sxx and -Sxx. Each is read as
-- 0Sxx, and `read_as` keeps what was read how, for the change to say.
local function offset_number(number)
  if number == "09" or number == "S" or number == "-S" then return "0S" end
end

local function fx_text(value, read_as)
  if value == "" then return "0000" end
  local text = text_of(value, "fx", "0A20")
  local amount = text:match("^%-?S(%x%x)$") or text:match("^09(%x%x)$")
  if amount then
    read_as[#read_as + 1] = text .. " as 0S" .. amount
    text = "0S" .. amount
  end
  if not text:match("^%w%w%x%x$") then bad("fx: an effect number and amount such as 0A20, or 0000 for none; got " .. value) end
  return text
end

-- What the change says of the spellings it read as Renoise's.
local function read_as_note(read_as)
  if #read_as == 0 then return nil end
  return "Read " .. table.concat(read_as, ", ") .. ": Renoise's sample offset is 0Sxx."
end

-- Cells

local function read_note(column)
  return {
    note = column.note_string, instrument = column.instrument_string, volume = column.volume_string,
    panning = column.panning_string, delay = column.delay_string, fx = column.effect_number_string .. column.effect_amount_string,
  }
end

local function read_effect(column)
  return { number = column.number_string, amount = column.amount_string }
end

-- A cell by its place {line, kind = "note" | "effect", col}, as its text with the place in it.
local function read_cell(pattern_track, at)
  local line = pattern_track:line(at.line)
  local state = at.kind == "note" and read_note(line:note_column(at.col)) or read_effect(line:effect_column(at.col))
  state.line, state.kind, state.col = at.line, at.kind, at.col
  return state
end

local function same_cell(a, b)
  if a.kind == "note" then
    for _, field in ipairs(NOTE_FIELDS) do
      if a[field] ~= b[field] then return false end
    end
    return true
  end
  return a.number == b.number and a.amount == b.amount
end

local function is_empty_cell(cell)
  return same_cell(cell, cell.kind == "note" and { kind = "note", note = "---", instrument = "..", volume = "..", panning = "..", delay = "..", fx = "0000" }
    or { kind = "effect", number = "00", amount = "00" })
end

local function same_cells(a, b)
  if type(a) ~= "table" or type(b) ~= "table" or #a ~= #b then return false end
  for i = 1, #a do
    if a[i].line ~= b[i].line or a[i].kind ~= b[i].kind or a[i].col ~= b[i].col or not same_cell(a[i], b[i]) then return false end
  end
  return true
end

local function write_cell(pattern_track, cell)
  local line = pattern_track:line(cell.line)
  if cell.kind == "note" then
    local column = line:note_column(cell.col)
    column:clear()
    if cell.note ~= "---" then column.note_string = cell.note end
    if cell.instrument ~= ".." then column.instrument_string = cell.instrument end
    if cell.volume ~= ".." then column.volume_string = cell.volume end
    if cell.panning ~= ".." then column.panning_string = cell.panning end
    if cell.delay ~= ".." then column.delay_string = cell.delay end
    if cell.fx:sub(1, 2) ~= "00" then column.effect_number_string = cell.fx:sub(1, 2) end
    if cell.fx:sub(3, 4) ~= "00" then column.effect_amount_string = cell.fx:sub(3, 4) end
  else
    local column = line:effect_column(cell.col)
    column:clear()
    if cell.number ~= "00" then column.number_string = cell.number end
    if cell.amount ~= "00" then column.amount_string = cell.amount end
  end
end

local function writes_of(cells)
  local total = 0
  for _, cell in ipairs(cells) do total = total + (cell.kind == "note" and NOTE_WRITES or EFFECT_WRITES) end
  return total
end

-- A short fingerprint of cells that two reads of cells nobody touched agree on.
local function fingerprint(cells)
  local parts = {}
  for _, cell in ipairs(cells) do
    local text
    if cell.kind == "note" then
      text = table.concat({ cell.line, "n", cell.col, cell.note, cell.instrument, cell.volume, cell.panning, cell.delay, cell.fx }, "|")
    else
      text = table.concat({ cell.line, "e", cell.col, cell.number, cell.amount }, "|")
    end
    parts[#parts + 1] = text
  end
  return base.hash(table.concat(parts))
end

-- The instruments note cells play (two hex digits; ".." is none), kept to find each again at Kumi's undo: Renoise renumbers
-- the song's notes as instruments are made, deleted or moved, but not the cells a record keeps.
local function instruments_of(song, env, ...)
  local values = {}
  for _, cells in ipairs({ ... }) do
    for _, cell in ipairs(cells) do
      local v = cell.kind == "note" and tonumber(cell.instrument, 16)
      if v and v < 255 then values[v] = true end
    end
  end
  return material_.keep_instruments(song, env, values)
end

-- The kept instruments where they are now (a table from then to now), or a refusal: one that can't be followed, or one that
-- is gone (Kumi won't put notes back playing another).
local function instruments_back(song, env, kept, label)
  local map, why = material_.instruments_now(song, env, kept)
  if map then return map end
  if why == "unknown" then
    fail("stale", "the instruments changed in a way Kumi couldn't follow since its step, so it can't tell which instruments the notes in " .. label .. " play: nothing was put back")
  end
  fail("changed", string.format("the instrument %s that notes in %s play was deleted in Renoise since: Kumi won't put them back playing another", q(why), label))
end

-- The cells as they read with their instruments where those are now.
local function remap(cells, map)
  if not next(map) then return cells end
  local out = {}
  for i, cell in ipairs(cells) do
    local v = cell.kind == "note" and tonumber(cell.instrument, 16)
    if v and map[v] and map[v] ~= v then
      local moved = {}
      for k, x in pairs(cell) do moved[k] = x end
      moved.instrument = string.format("%02X", map[v])
      out[i] = moved
    else
      out[i] = cell
    end
  end
  return out
end

local function cell_order(a, b)
  if a.line ~= b.line then return a.line < b.line end
  if a.kind ~= b.kind then return a.kind == "note" end
  return a.col < b.col
end

-- The columns a track shows

local function read_columns(track)
  local columns = { effect = track.visible_effect_columns }
  if track.max_note_columns > 0 then
    columns.note = track.visible_note_columns
    columns.volume, columns.panning = track.volume_column_visible, track.panning_column_visible
    columns.delay, columns.sampleEffects = track.delay_column_visible, track.sample_effects_column_visible
  end
  return columns
end

-- What the written cells need to be seen: the columns up to the highest used and the sub-columns with something in them.
local function needs_of(cells)
  local needs = { note = 0, effect = 0 }
  for _, cell in ipairs(cells) do
    if cell.kind == "note" then
      if not is_empty_cell(cell) then
        needs.note = math.max(needs.note, cell.col)
        if cell.volume ~= ".." then needs.volume = true end
        if cell.panning ~= ".." then needs.panning = true end
        if cell.delay ~= ".." then needs.delay = true end
        if cell.fx ~= "0000" then needs.sampleEffects = true end
      end
    elseif not is_empty_cell(cell) then
      needs.effect = math.max(needs.effect, cell.col)
    end
  end
  return needs
end

local function raise_columns(track, needs)
  if needs.note > 0 and track.visible_note_columns < needs.note then track.visible_note_columns = needs.note end
  if needs.effect > 0 and track.visible_effect_columns < needs.effect then track.visible_effect_columns = needs.effect end
  if needs.volume and not track.volume_column_visible then track.volume_column_visible = true end
  if needs.panning and not track.panning_column_visible then track.panning_column_visible = true end
  if needs.delay and not track.delay_column_visible then track.delay_column_visible = true end
  if needs.sampleEffects and not track.sample_effects_column_visible then track.sample_effects_column_visible = true end
end

local function set_columns(track, columns)
  pcall(function()
    if columns.note and track.visible_note_columns ~= columns.note then track.visible_note_columns = columns.note end
    if columns.effect and track.visible_effect_columns ~= columns.effect then track.visible_effect_columns = columns.effect end
    if columns.volume ~= nil then track.volume_column_visible = columns.volume end
    if columns.panning ~= nil then track.panning_column_visible = columns.panning end
    if columns.delay ~= nil then track.delay_column_visible = columns.delay end
    if columns.sampleEffects ~= nil then track.sample_effects_column_visible = columns.sampleEffects end
  end)
end

local READ_BUDGET = 40000

-- What is in the track's columns above the ones Kumi found shown, in every pattern: nil when that is too much to look through.
local function in_use(song, index, prior)
  -- (`notes`, `effects`: the columns holding something, each once)
  local used, reads = { note = 0, effect = 0, notes = {}, effects = {} }, 0
  local seen = { notes = {}, effects = {} }
  local function mark(list, c)
    local columns = list == "notes" and used.notes or used.effects
    if not seen[list][c] then seen[list][c] = true columns[#columns + 1] = c end
  end
  for p = 1, #song.patterns do
    local pattern = song:pattern(p)
    local pattern_track = pattern:track(index)
    if not pattern_track.is_alias and not pattern_track.is_empty then
      for l = 1, pattern.number_of_lines do
        local line = pattern_track:line(l)
        reads = reads + 1
        if not line.is_empty then
          for c = 1, 12 do
            local column = line:note_column(c)
            reads = reads + 5
            if not column.is_empty then
              if c > used.note then used.note = c end
              mark("notes", c)
              if column.volume_string ~= ".." then used.volume = true end
              if column.panning_string ~= ".." then used.panning = true end
              if column.delay_string ~= ".." then used.delay = true end
              if column.effect_number_value ~= 0 or column.effect_amount_value ~= 0 then used.sampleEffects = true end
            end
          end
          for c = 1, 8 do
            reads = reads + 1
            if not line:effect_column(c).is_empty then
              if c > used.effect then used.effect = c end
              mark("effects", c)
            end
          end
          if reads > READ_BUDGET then return nil end
        end
      end
    end
  end
  return used
end

-- What was in the track's columns before Kumi showed more of them (`applied.hidden`), when it is about to: the producer's,
-- hidden as they had it. Nil when the track shows what the cells need already, or the song is too big to look through.
local function hidden_use(song, index, track, needs)
  local now = read_columns(track)
  local raises = (needs.note > 0 and (now.note or 0) < needs.note) or (needs.effect > 0 and (now.effect or 0) < needs.effect)
  for _, flag in ipairs({ "volume", "panning", "delay", "sampleEffects" }) do
    if needs[flag] and now[flag] == false then raises = true end
  end
  if not raises then return nil end
  return in_use(song, index)
end

-- The columns Kumi showed are hidden again when they are still as Kumi left them and nothing is in them but what was there,
-- hidden, before Kumi showed them (`applied.hidden`): what the producer put in them since keeps them shown.
-- (`on_write`, when given, runs just before anything is written: a restore that found the lines back names its step there)
local function lower_columns(song, index, track, prior, applied, on_write)
  if type(prior) ~= "table" or type(applied) ~= "table" then return end
  local now = read_columns(track)
  local used = in_use(song, index, prior)
  if not used then return end
  local before = type(applied.hidden) == "table" and applied.hidden or {}
  -- (the highest column Kumi showed that holds something it didn't before it was shown)
  local function kept(list, from, to)
    local had = {}
    for _, c in ipairs(type(before[list]) == "table" and before[list] or {}) do had[c] = true end
    local holds = {}
    for _, c in ipairs(list == "notes" and used.notes or used.effects) do holds[c] = true end
    for c = to, from + 1, -1 do
      if holds[c] and not had[c] then return c end
    end
    return 0
  end
  local back = {}
  if prior.note and now.note == applied.note and applied.note > prior.note then back.note = math.max(prior.note, kept("notes", prior.note, applied.note)) end
  if prior.effect and now.effect == applied.effect and applied.effect > prior.effect then
    back.effect = math.max(prior.effect, kept("effects", prior.effect, applied.effect))
  end
  for _, flag in ipairs({ "volume", "panning", "delay", "sampleEffects" }) do
    if prior[flag] == false and applied[flag] == true and now[flag] == true and (not used[flag] or before[flag]) then back[flag] = false end
  end
  if next(back) then
    if on_write then on_write() end
    set_columns(track, back)
    return true
  end
end

-- What a request names

local function pattern_of(song, env, reference)
  if type(reference) ~= "string" then bad("pattern: a pattern ref, or the ref of a sequence slot") end
  local parsed = refs.parse(reference)
  if not parsed then bad("not a ref: " .. reference) end
  if parsed.kind == "seq" and #parsed.n == 1 then
    local slot = refs.expect(reference, "seq", 1, env.epoch())[1]
    local sequence = song.sequencer.pattern_sequence
    if slot > #sequence then fail("gone", "that sequence slot isn't in the song any more") end
    return sequence[slot]
  end
  local p = refs.expect(reference, "pattern", 1, env.epoch())[1]
  if p > #song.patterns then fail("gone", "that pattern isn't in the song any more") end
  return p
end

local function pattern_label(pattern, p)
  local name = pattern.name
  if name == nil or name == "" then name = string.format("Pattern %02d", p - 1) end
  return name
end

local function pattern_track_of(song, p, index)
  local pattern_track = song:pattern(p):track(index)
  if pattern_track.is_alias then
    bad(string.format("that track in pattern %02d is an alias of pattern %02d: its lines are that pattern's, so write them there", p - 1, pattern_track.alias_pattern_index - 1))
  end
  return pattern_track
end

local function target_of(env, p, index)
  return { ref = refs.make(env.epoch(), "ptrack", p, index), kind = "pattern-track" }
end

-- Where a request writes: the pattern, the track and the pattern's track.
local function locate(song, env, input, kind)
  local p = pattern_of(song, env, input.pattern)
  local index, track = base.find(song, env, input.track, kind)
  return p, index, track, pattern_track_of(song, p, index), song:pattern(p)
end

-- A line as a request names it, by Renoise's number (from 0), to its place in the pattern (from 1).
local function line_at(pattern, line)
  if type(line) ~= "number" or line ~= math.floor(line) or line < 0 or line >= pattern.number_of_lines then
    bad(string.format("line must be a whole number from 0 to %d (this pattern's lines, numbered as Renoise shows them)", pattern.number_of_lines - 1))
  end
  return line + 1
end

-- The lines `first` to `last` a request names by Renoise's numbers (the whole pattern when left out), as places from 1.
local function line_range(input, lines)
  local first, last = input.first or 0, input.last or lines - 1
  for _, value in ipairs({ first, last }) do
    if type(value) ~= "number" or value ~= math.floor(value) or value < 0 or value >= lines then
      bad(string.format("first and last are line numbers from 0 to %d", lines - 1))
    end
  end
  if first > last then bad("first must not come after last") end
  return first + 1, last + 1
end

-- (`line` is a place from 1)
local function cell_at(pattern, track, line, kind, col)
  if type(col) ~= "number" or col ~= math.floor(col) then bad("col must be a whole number") end
  if kind == "note" then
    if track.max_note_columns == 0 then bad("that track has no note columns: only sequencer tracks do") end
    if col < 1 or col > track.max_note_columns then bad(string.format("note col must be from 1 to %d", track.max_note_columns)) end
  else
    if col < 1 or col > track.max_effect_columns then bad(string.format("effect col must be from 1 to %d", track.max_effect_columns)) end
  end
  return { line = line, kind = kind, col = col }
end

-- The cells a request names, with what it gives for each (`given`) and whether to clear first: [key] = want.
local function key_of(at) return at.line * 100 + (at.kind == "note" and 0 or 50) + at.col end

-- A note's instrument named by its ref (a pattern's notes only: `env` is there), as the two hex digits Renoise shows.
local function instrument_of_ref(env, reference)
  if env == nil then bad("instrumentRef is for a pattern's notes: give instrument") end
  if type(reference) ~= "string" then bad("instrumentRef must be an instrument ref") end
  local index = refs.expect(reference, "instrument", 1, env.epoch())[1]
  if index > #the_song().instruments then fail("gone", "that instrument isn't in the song any more") end
  return string.format("%02X", index - 1)
end

-- (`env` for refs inside the cells: a pattern's notes may name their instrument by its ref)
local function parse_cells(pattern, track, input, env)
  local wants, count, read_as = {}, 0, {}
  local function add(at, given, clear)
    local key = key_of(at)
    if wants[key] then bad(string.format("line %02d %s column %d is given twice", at.line - 1, at.kind, at.col)) end
    wants[key] = { at = at, given = given, clear = clear }
    count = count + 1
  end
  if type(input.lines) ~= "table" or #input.lines == 0 then bad("lines: a list of {line, notes?, effects?, clear?}") end
  local seen = {}
  for _, entry in ipairs(input.lines) do
    if type(entry) ~= "table" then bad("each of lines is an object {line, notes?, effects?, clear?}") end
    local line = line_at(pattern, entry.line)
    if seen[line] then bad(string.format("line %02d is given twice: put its cells in one entry", entry.line)) end
    seen[line] = true
    local any = false
    for _, cell in ipairs(entry.notes or {}) do
      if type(cell) ~= "table" then bad("each of notes is an object {col, note?, instrument? (or instrumentRef), volume?, panning?, delay?, fx?, clear?}") end
      local at = cell_at(pattern, track, line, "note", cell.col)
      local given = {}
      if cell.note ~= nil then given.note = note_text(cell.note) end
      if cell.instrument ~= nil and cell.instrumentRef ~= nil then bad("give instrument or instrumentRef, not both") end
      if cell.instrument ~= nil then given.instrument = hex2(cell.instrument, "instrument", "..") end
      if cell.instrumentRef ~= nil then given.instrument = instrument_of_ref(env, cell.instrumentRef) end
      if cell.volume ~= nil then given.volume = column_text(cell.volume, "volume") end
      if cell.panning ~= nil then given.panning = column_text(cell.panning, "panning") end
      if cell.delay ~= nil then given.delay = delay_text(cell.delay) end
      if cell.fx ~= nil then given.fx = fx_text(cell.fx, read_as) end
      if next(given) == nil and cell.clear ~= true then bad(string.format("line %02d note column %d says nothing to write: give a field, or clear: true", entry.line, at.col)) end
      add(at, given, cell.clear == true)
      any = true
    end
    for _, cell in ipairs(entry.effects or {}) do
      if type(cell) ~= "table" then bad("each of effects is an object {col, number?, amount?, clear?}") end
      local at = cell_at(pattern, track, line, "effect", cell.col)
      local given = {}
      if cell.amount ~= nil then given.amount = hex2(cell.amount, "amount", "00") end
      if cell.number ~= nil then
        given.number = text_of(cell.number, "number", "0A")
        local offset = offset_number(given.number)
        if offset then
          read_as[#read_as + 1] = given.number .. (given.amount or "") .. " as 0S" .. (given.amount or "")
          given.number = offset
        end
        if not given.number:match("^%w%w$") then bad("number: two characters such as 0A; got " .. cell.number) end
      end
      if next(given) == nil and cell.clear ~= true then bad(string.format("line %02d effect column %d says nothing to write: give a field, or clear: true", entry.line, at.col)) end
      add(at, given, cell.clear == true)
      any = true
    end
    if entry.clear ~= true and not any then
      bad(string.format("line %02d says nothing to write: give notes or effects, or clear: true", entry.line))
    end
  end
  return wants, count, read_as
end

-- The cells a clear of whole lines takes: every cell with something in it, in all columns (hidden ones too). (After
-- parse_cells, which checks the lines.)
local function add_cleared(pattern_track, input, wants)
  for _, entry in ipairs(input.lines) do
    if entry.clear == true then
      local place = entry.line + 1
      local line = pattern_track:line(place)
      if not line.is_empty then
        for c = 1, 12 do
          local at = { line = place, kind = "note", col = c }
          if not wants[key_of(at)] and not line:note_column(c).is_empty then
            wants[key_of(at)] = { at = at, given = {}, clear = true }
          end
        end
        for c = 1, 8 do
          local at = { line = place, kind = "effect", col = c }
          if not wants[key_of(at)] and not line:effect_column(c).is_empty then
            wants[key_of(at)] = { at = at, given = {}, clear = true }
          end
        end
      end
    end
  end
end

-- The plan: for each named cell, what it holds now and what it will hold, in order; only the cells that differ are `changes`.
local function plan_of(pattern_track, wants, merge)
  local keys = {}
  for key in pairs(wants) do keys[#keys + 1] = key end
  table.sort(keys)
  local prior, after = {}, {}
  for _, key in ipairs(keys) do
    local want = wants[key]
    local now = read_cell(pattern_track, want.at)
    local target = { line = want.at.line, kind = want.at.kind, col = want.at.col }
    local base_state
    if want.at.kind == "note" then
      base_state = (merge and not want.clear) and now or EMPTY_NOTE
      for _, field in ipairs(NOTE_FIELDS) do target[field] = want.given[field] or base_state[field] end
    else
      base_state = (merge and not want.clear) and now or EMPTY_EFFECT
      target.number, target.amount = want.given.number or base_state.number, want.given.amount or base_state.amount
    end
    if not same_cell(now, target) then
      prior[#prior + 1] = now
      after[#after + 1] = target
    end
  end
  return prior, after
end

local function counts_of(cells)
  local notes, effects = 0, 0
  for _, cell in ipairs(cells) do
    if not is_empty_cell(cell) then
      if cell.kind == "note" then notes = notes + 1 else effects = effects + 1 end
    end
  end
  return { notes = notes, effects = effects }
end

local function lines_in(cells)
  local seen, n = {}, 0
  for _, cell in ipairs(cells) do
    if not seen[cell.line] then
      seen[cell.line] = true
      n = n + 1
    end
  end
  return n
end

local function lines_text(n) return n == 1 and "1 line" or n .. " lines" end

local function title_of(verb, cells, pattern, p, track, index)
  return string.format("%s %s in %s on %s", verb, lines_text(lines_in(cells)), q(pattern_label(pattern, p)), q(base.named(index, track)))
end

-- (`planner(ctx, input)` answers the cells before and after, and a title, for kinds that work the cells out themselves: M4 n-song's
-- transpose and humanize; `ctx` is {p, index, track, pattern_track, pattern})
local function preview_of(song, env, input, kind, verb, planner)
  local p, index, track, pattern_track, pattern = locate(song, env, input, kind)
  local prior, after, title, read_as
  if planner then
    prior, after, title = planner({ p = p, index = index, track = track, pattern_track = pattern_track, pattern = pattern }, input)
  else
    local parsed = { parse_cells(pattern, track, input, env) }
    local wants = parsed[1]
    read_as = parsed[3]
    add_cleared(pattern_track, input, wants)
    prior, after = plan_of(pattern_track, wants, input.merge == true)
  end
  if writes_of(after) > M.MAX_WRITES then
    bad(string.format("that is %d writes, more than one change takes (%d): split it into several", writes_of(after), M.MAX_WRITES))
  end
  local columns = read_columns(track)
  local display = {
    title = title or title_of(verb, #after > 0 and after or prior, pattern, p, track, index),
    before = counts_of(prior), after = counts_of(after),
  }
  if #after == 0 then display.unchanged = true end
  local needs = needs_of(after)
  if needs.note > (columns.note or 0) then display.note = "It shows note column " .. needs.note .. " first." end
  local said = read_as_note(read_as or {})
  if said then display.note = display.note and (said .. " " .. display.note) or said end
  return {
    target = target_of(env, p, index), prior = { cells = prior, columns = columns },
    fence = { cells = #prior, hash = fingerprint(prior), id = base.track_id(track), label = base.named(index, track) },
    display = display,
  }, { p = p, index = index, track = track, pattern_track = pattern_track, pattern = pattern, prior = prior, after = after, columns = columns }
end

-- Whether Kumi may put its lines back: only when nothing in their pattern changed since Kumi's step. Renoise moves lines with a
-- row inserted or deleted (in a column, a track or the whole pattern), Expand, a column moved, its own undo past Kumi's step,
-- and tells none of that apart from an edit; a run of like notes hides where lines went. So any line changed in the pattern
-- since counts: the producer's edits (through an alias too: lab, they notify the pattern shown), Renoise's undo and redo (lab: its
-- line notifiers fire inside them), a track of the pattern made an alias or not, and Kumi's other changes. The tool counts them
-- per pattern (bridge.lua), and a change keeps the count as its mark. Lines have no identity but their place, so a discard
-- doesn't pass over this either.
local CHANGED = "the pattern changed in Renoise since Kumi's step (an edit, an undo or redo, or another change of Kumi's), so Kumi can't tell where its lines are now and left them as they are: Renoise's own undo can take them back, after the edits made since, which it takes back first"

local function apply_of(song, env, input, fence, desc, kind, verb, planner)
  local ok, preview, plan = pcall(preview_of, song, env, input, kind, verb, planner)
  if not ok then base.gone_is_changed(preview) end
  if fence.id ~= nil and fence.id ~= preview.fence.id then
    fail("changed", base.moved_text(fence.label or base.named(plan.index, plan.track)))
  end
  if preview.fence.cells ~= fence.cells or preview.fence.hash ~= fence.hash then
    fail("changed", string.format("%s in %s changed since the preview", base.named(plan.index, plan.track), pattern_label(plan.pattern, plan.p)))
  end
  local display = preview.display
  if #plan.after == 0 then
    return {
      applied = { cells = {}, columns = plan.columns }, display = display, undo = { desc = desc, entry = false }, prior = preview.prior,
    }
  end
  local hidden = hidden_use(song, plan.index, plan.track, needs_of(plan.after))
  song:describe_undo(desc)
  local written = 0
  local wrote, err = pcall(function()
    raise_columns(plan.track, needs_of(plan.after))
    for i, cell in ipairs(plan.after) do
      write_cell(plan.pattern_track, cell)
      written = i
    end
  end)
  if not wrote then
    -- Nothing half-done stays: the cells written go back to what they held, and the columns.
    pcall(function()
      for i = 1, written + 1 do
        if plan.prior[i] then write_cell(plan.pattern_track, plan.prior[i]) end
      end
    end)
    set_columns(plan.track, plan.columns)
    local message = type(err) == "table" and err.message or tostring(err):gsub("^[^%s:]*%.lua:%d+: ", "")
    fail("failed", "Renoise wouldn't take those cells: " .. message)
  end
  local result = {}
  for i, cell in ipairs(plan.after) do result[i] = read_cell(plan.pattern_track, cell) end
  for i, cell in ipairs(result) do
    if not same_cell(cell, plan.after[i]) then
      -- (Renoise read the text as something else: nothing half-done stays)
      for j = 1, #plan.after do write_cell(plan.pattern_track, plan.prior[j]) end
      set_columns(plan.track, plan.columns)
      fail("failed", string.format("Renoise didn't keep line %02d column %d as written", cell.line - 1, cell.col))
    end
  end
  local columns = read_columns(plan.track)
  columns.hidden = hidden
  return {
    applied = {
      cells = result, columns = columns,
      -- (what the undo needs to know whether the pattern changed since: its count of changed lines from here, and its length)
      touch = env.pattern_touch_mark and env.pattern_touch_mark(plan.p) or nil, lines = plan.pattern.number_of_lines,
      instruments = instruments_of(song, env, plan.prior, result),
    },
    display = { title = display.title, before = display.before, after = counts_of(result), note = display.note },
    undo = { desc = desc, entry = true },
    prior = preview.prior, place = { pattern = plan.p, track = plan.index }, target = target_of(env, plan.p, plan.index),
  }
end

-- Restore

-- (`changed` is the env's pattern_touched_since for this pattern; `map` the kept instruments' numbers now, from instruments_back)
local function restore_plan(pattern_track, restore, lines, changed, map)
  local prior, applied = restore.prior and restore.prior.cells, restore.applied and restore.applied.cells
  if type(prior) ~= "table" or type(applied) ~= "table" or #prior ~= #applied then bad("prior and applied: the cells the change wrote") end
  prior, applied = remap(prior, map or {}), remap(applied, map or {})
  local kept = restore.applied
  if #applied > 0 then
    if type(kept.lines) == "number" and kept.lines ~= lines then
      fail("changed", "the pattern's length changed in Renoise since Kumi wrote these lines, so Kumi left them as they are")
    end
    local since = changed(kept.touch)
    if since == nil then
      fail("changed", "Kumi can't tell whether the pattern changed in Renoise since (the tool watched the song anew), so Kumi left these lines as they are")
    end
    if since then fail("changed", CHANGED) end
  end
  local todo = {}
  for i = 1, #prior do
    local now = read_cell(pattern_track, prior[i])
    if not same_cell(now, prior[i]) then
      if not same_cell(now, applied[i]) then
        fail("changed", string.format("line %02d was changed in Renoise since Kumi wrote it", prior[i].line - 1))
      end
      todo[#todo + 1] = prior[i]
    end
  end
  return todo
end

local function locate_restored(song, env, restore)
  local reference = restore.target and restore.target.ref
  local numbers = refs.expect(reference, "ptrack", 2, env.epoch())
  local p, index = numbers[1], numbers[2]
  if p > #song.patterns then fail("gone", "that pattern isn't in the song any more") end
  local track = base.track_at(song, index)
  local pattern_track = song:pattern(p):track(index)
  if pattern_track.is_alias then fail("changed", "that track in the pattern is an alias now") end
  return p, index, track, pattern_track, song:pattern(p)
end

local function chunks_of(todo)
  local chunks, current, writes = {}, {}, 0
  for _, cell in ipairs(todo) do
    local cost = cell.kind == "note" and NOTE_WRITES or EFFECT_WRITES
    if writes > 0 and writes + cost > base.SLICE_WRITES then
      chunks[#chunks + 1] = current
      current, writes = {}, 0
    end
    current[#current + 1] = cell
    writes = writes + cost
  end
  chunks[#chunks + 1] = current
  return chunks
end

local function restored_answer(env, display, desc, steps)
  local answer = { restored = { done = true }, display = display, undo = { desc = desc, entry = true } }
  if steps then answer.undo.steps = steps end
  return answer
end

local function restore_of(spec_kind, noun)
  return function(song, env, restore, desc)
    local p, index, track, pattern_track, pattern = locate_restored(song, env, restore)
    local function changed(mark)
      if not env.pattern_touched_since then return false end
      return env.pattern_touched_since(p, mark)
    end
    local label = string.format("%s on %s", q(pattern_label(pattern, p)), q(base.named(index, track)))
    local map = instruments_back(song, env, restore.applied and restore.applied.instruments, label)
    local todo = restore_plan(pattern_track, restore, pattern.number_of_lines, changed, map)
    local place = { pattern = p, track = index }
    local spec = base.KINDS[spec_kind]
    if #todo == 0 then
      -- (the lines are back; hiding the columns Kumi showed is a step of Renoise's undo, named as Kumi's)
      local hid = lower_columns(song, index, track, restore.prior.columns, restore.applied.columns, function() song:describe_undo(desc) end)
      base.note_restored(env.journal, spec_kind, place, restore.applied, spec)
      if hid then
        return { restored = { done = true }, display = { title = noun .. " in " .. label .. " is back already: the columns Kumi showed are hidden again" }, undo = { desc = desc, entry = true } }
      end
      return { restored = { done = true }, display = { title = noun .. " in " .. label .. " is back already" }, undo = { desc = desc, entry = false }, unwritten = true }
    end
    local chunks = chunks_of(todo)
    local display = { title = string.format("%s put back in %s", noun, label), before = counts_of(todo), after = counts_of(restore.prior.cells) }
    local function finish(live_song, live_track)
      lower_columns(live_song, index, live_track, restore.prior.columns, restore.applied.columns)
      base.note_restored(env.journal, spec_kind, place, restore.applied, spec)
    end
    if #chunks == 1 then
      song:describe_undo(desc)
      for _, cell in ipairs(chunks[1]) do write_cell(pattern_track, cell) end
      finish(song, track)
      return restored_answer(env, display, desc)
    end
    -- Too much for one callback: numbered steps, each its own callback and undo step, the first on a later turn. The job stops when
    -- tracks, patterns or lines were inserted or removed between them (it holds a track's number, and the producer's change may
    -- have moved it), or when anything in the pattern changed since the check above or its own last step (an edit, Renoise's
    -- undo: the steps would write over it). Nothing further is written then, and once a step is written what is put back can't
    -- be told (Renoise's undo may have taken Kumi's steps back).
    local mark = restore.applied.touch
    local function still()
      if changed(mark) ~= false then
        fail("changed", string.format("the pattern changed in Renoise while Kumi put the lines back in %s", label))
      end
    end
    return {
      kumi_job = function(job)
        local total = #chunks
        local done_steps = 0
        local ok, err = pcall(function()
          for k = 1, total do
            if k > 1 then
              job.step(base.STEP_GAP_MS)
              base.wait_while_busy(job, env)
            end
            still()
            local live = the_song()
            local live_pattern_track = live:pattern(p):track(index)
            if live_pattern_track.is_alias then fail("changed", "that track in the pattern is an alias now") end
            live:describe_undo(string.format("%s (%d/%d)", desc, k, total))
            for _, cell in ipairs(chunks[k]) do write_cell(live_pattern_track, cell) end
            mark = env.pattern_touch_mark and env.pattern_touch_mark(p) or nil
            done_steps = k
            if k == 1 then job.rebase() end
            -- (a stop from here leaves lines half put back: a tool stopped or reloaded says so, see jobs.lua)
            job.wrote = string.format("Putting the lines back in %s (stopped part way)", label)
          end
        end)
        if not ok then
          if type(err) == "table" and err.message and done_steps > 0 then
            -- (lines half put back: what Kumi changed is neither as it made it nor as it was)
            err = { code = "uncertain", message = err.message .. "; Kumi stopped part way and wrote nothing further, so which of "
              .. "these lines are put back can't be told: Renoise's own undo can take Kumi's steps back" }
          end
          error(err, 0)
        end
        local live = the_song()
        finish(live, live:track(index))
        local answer = restored_answer(env, display, desc, total)
        answer.epoch, answer.layout = env.epoch(), env.layout()
        return answer
      end,
    }
  end
end

-- notes-write

local function same_state(a, b) return same_cells(a.cells, b.cells) end

local function register(kind, verb, noun, planner)
  local spec = {
    kind = kind, custom = true,
    preview = function(song, env, input) return (preview_of(song, env, input, kind, verb, planner)) end,
    apply = function(song, env, input, fence, desc) return apply_of(song, env, input, fence, desc, kind, verb, planner) end,
    restore = restore_of(kind, noun),
    same = same_state,
    unsure_redo = true,
    reread = function(song, place)
      if place.pattern > #song.patterns or place.track > rows.track_count(song) then return nil end
      local pattern_track = song:pattern(place.pattern):track(place.track)
      if pattern_track.is_alias then return nil end
      return pattern_track
    end,
    read = function(pattern_track, shape)
      local cells = {}
      for i, at in ipairs(shape.cells or {}) do cells[i] = read_cell(pattern_track, at) end
      return { cells = cells }
    end,
  }
  base.register(spec)
end

register("notes-write", "Wrote", "Lines")

-- notes-clear: the cells with something in them in a range of lines (all lines when none is given), in every column.
local function clear_input(song, env, input, kind)
  local p, _, _, pattern_track, pattern = locate(song, env, input, kind)
  local first, last = line_range(input, pattern.number_of_lines)
  local lines = {}
  -- (as notes-write takes them: by Renoise's numbers)
  for l = first, last do
    if not pattern_track:line(l).is_empty then lines[#lines + 1] = { line = l - 1, clear = true } end
  end
  return { pattern = input.pattern, track = input.track, lines = lines, epoch = input.epoch }, p
end

local clear = {
  kind = "notes-clear", custom = true,
  same = same_state,
  unsure_redo = true,
}
clear.reread = base.KINDS["notes-write"].reread
clear.read = base.KINDS["notes-write"].read
clear.preview = function(song, env, input)
  local built = clear_input(song, env, input, "notes-clear")
  if #built.lines == 0 then
    local p, index, track, _, pattern = locate(song, env, input, "notes-clear")
    return {
      target = target_of(env, p, index), prior = { cells = {}, columns = read_columns(track) }, fence = { cells = 0, hash = fingerprint({}) },
      display = { title = title_of("Cleared", {}, pattern, p, track, index), unchanged = true, before = { notes = 0, effects = 0 }, after = { notes = 0, effects = 0 } },
    }
  end
  return (preview_of(song, env, built, "notes-clear", "Cleared"))
end
clear.apply = function(song, env, input, fence, desc)
  local built = clear_input(song, env, input, "notes-clear")
  if #built.lines == 0 then
    local p, index, track, _, pattern = locate(song, env, input, "notes-clear")
    return {
      applied = { cells = {}, columns = read_columns(track) }, undo = { desc = desc, entry = false },
      display = { title = title_of("Cleared", {}, pattern, p, track, index), unchanged = true, before = { notes = 0, effects = 0 }, after = { notes = 0, effects = 0 } },
      prior = { cells = {}, columns = read_columns(track) },
    }
  end
  return apply_of(song, env, built, fence, desc, "notes-clear", "Cleared")
end
clear.restore = restore_of("notes-clear", "Cells")
base.register(clear)
-- M4 (n-song): notes transposed, notes humanized, and a block of lines copied or moved. They write cells as notes-write does
-- (whole cells, the visible columns raised first, read back), and Kumi's undo puts them back by the same plain rule.

local note_name = base.note_name

-- The lines a request names (`first`, `last`: the whole pattern when left out) and the note columns (`columns`, the shown
-- ones when left out).
local function range_of(ctx, input)
  local first, last = line_range(input, ctx.pattern.number_of_lines)
  local track = ctx.track
  if track.max_note_columns == 0 or track.type ~= renoise.Track.TRACK_TYPE_SEQUENCER then
    bad("that track has no note columns: only sequencer tracks do")
  end
  local columns = {}
  if input.columns ~= nil then
    if type(input.columns) ~= "table" or #input.columns == 0 then bad("columns: a list of note columns, 1 to 12") end
    local seen = {}
    for _, c in ipairs(input.columns) do
      if type(c) ~= "number" or c ~= math.floor(c) or c < 1 or c > track.max_note_columns or seen[c] then
        bad(string.format("columns: note columns from 1 to %d, each once", track.max_note_columns))
      end
      seen[c] = true
      columns[#columns + 1] = c
    end
    table.sort(columns)
  else
    for c = 1, track.visible_note_columns do columns[c] = c end
  end
  return first, last, columns
end

local function notes_text(n) return n == 1 and "1 note" or n .. " notes" end

local function place_text(ctx) return string.format("%s on %s", q(pattern_label(ctx.pattern, ctx.p)), q(base.named(ctx.index, ctx.track))) end

-- notes-transpose: the notes in the range move by `semitones`; OFF and empty cells stay, and a note that would leave C-0..B-9
-- refuses the whole change.
local function transpose_planner(ctx, input)
  local semitones = input.semitones
  if type(semitones) ~= "number" or semitones ~= math.floor(semitones) or semitones == 0 or math.abs(semitones) > 119 then
    bad("semitones is a whole number from -119 to 119, not 0")
  end
  local first, last, columns = range_of(ctx, input)
  local prior, after = {}, {}
  for l = first, last do
    local line = ctx.pattern_track:line(l)
    if not line.is_empty then
      for _, c in ipairs(columns) do
        local value = line:note_column(c).note_value
        if value <= 119 then
          local moved = value + semitones
          if moved < 0 or moved > 119 then
            bad(string.format("line %02d column %d: %s %+d semitones is outside C-0 to B-9, so nothing was transposed", l - 1, c, note_name(value), semitones))
          end
          local now = read_cell(ctx.pattern_track, { line = l, kind = "note", col = c })
          local target = {}
          for k, v in pairs(now) do target[k] = v end
          target.note = note_name(moved)
          prior[#prior + 1], after[#after + 1] = now, target
        end
      end
    end
  end
  local title = string.format("Transposed %s %+d in %s", notes_text(#after), semitones, place_text(ctx))
  return prior, after, title
end

register("notes-transpose", "Transposed", "Notes", transpose_planner)

-- notes-humanize: each note's delay is moved later by up to `delay` (256ths of a line; a note can't play before its line), its
-- volume by up to `volume` either way (00..80; an empty volume counts as 80, a volume command is left alone), from a seeded
-- random sequence: the same request gives the same lines, so a retry or the apply after the preview writes what was shown.
local function seeded(seed)
  local state = bit.band(seed, 0x7fffffff)
  if state == 0 then state = 0x2545F491 end
  return function(n)
    -- (xorshift32: the same on every LuaJIT)
    state = bit.bxor(state, bit.lshift(state, 13))
    state = bit.bxor(state, bit.rshift(state, 17))
    state = bit.bxor(state, bit.lshift(state, 5))
    return (state % 4294967296) % (n + 1)
  end
end

local function seed_of(input, ctx)
  if input.seed ~= nil then
    if type(input.seed) ~= "number" or input.seed ~= math.floor(input.seed) or input.seed < 0 or input.seed > 2147483647 then
      bad("seed is a whole number from 0 to 2147483647")
    end
    return input.seed
  end
  -- (the lines by their places from 1, as before Kumi took Renoise's numbers: the same request draws the same)
  local function place(line) return type(line) == "number" and line + 1 or 0 end
  local text = table.concat({ ctx.p, ctx.index, place(input.first), place(input.last), input.delay or 0, input.volume or 0 }, ":")
  local hash = 5381
  for i = 1, #text do hash = (hash * 33 + text:byte(i)) % 2147483647 end
  return hash
end

local function humanize_planner(ctx, input)
  local delay, volume = input.delay or 0, input.volume or 0
  if type(delay) ~= "number" or delay ~= math.floor(delay) or delay < 0 or delay > 255 then bad("delay is the most a note moves later, in 256ths of a line: 0 to 255") end
  if type(volume) ~= "number" or volume ~= math.floor(volume) or volume < 0 or volume > 128 then bad("volume is the most a note's volume moves either way: 0 to 128 (80 hex is full)") end
  if delay == 0 and volume == 0 then bad("give delay or volume (or both)") end
  local first, last, columns = range_of(ctx, input)
  local random = seeded(seed_of(input, ctx))
  local prior, after = {}, {}
  for l = first, last do
    local line = ctx.pattern_track:line(l)
    if not line.is_empty then
      for _, c in ipairs(columns) do
        local column = line:note_column(c)
        if column.note_value <= 119 then
          local now = read_cell(ctx.pattern_track, { line = l, kind = "note", col = c })
          local target = {}
          for k, v in pairs(now) do target[k] = v end
          -- (one draw each, whether or not it changes anything: the sequence doesn't depend on what is in the cells)
          local late, swing = random(delay), random(volume * 2) - volume
          if delay > 0 then
            local moved = math.min(255, column.delay_value + late)
            target.delay = moved == 0 and ".." or string.format("%02X", moved)
          end
          if volume > 0 and (column.volume_value <= 0x80 or column.volume_value == 255) then
            local base_volume = column.volume_value == 255 and 0x80 or column.volume_value
            local moved = math.max(0, math.min(0x80, base_volume + swing))
            if moved ~= base_volume then target.volume = string.format("%02X", moved) end
          end
          if not same_cell(now, target) then prior[#prior + 1], after[#after + 1] = now, target end
        end
      end
    end
  end
  local parts = {}
  if delay > 0 then parts[#parts + 1] = "delay up to " .. delay end
  if volume > 0 then parts[#parts + 1] = "volume \194\177" .. volume end
  local title = string.format("Humanized %s in %s (%s)", notes_text(#after), place_text(ctx), table.concat(parts, ", "))
  return prior, after, title
end

register("notes-humanize", "Humanized", "Notes", humanize_planner)

-- lines-copy: a block of lines (every note and effect column) copied, or moved, to another place: the same pattern track or
-- another (the same track by default). The source may be an alias (its lines are its source's); a destination that is an alias
-- is refused (its lines are another pattern's: write there). The destination's columns are raised to show what it gets.
local function block_of(song, env, input)
  local p = pattern_of(song, env, input.pattern)
  local index, track = base.find(song, env, input.track, "lines-copy")
  local to_p = input.toPattern ~= nil and pattern_of(song, env, input.toPattern) or p
  local to_index, to_track = index, track
  if input.toTrack ~= nil then to_index, to_track = base.find(song, env, input.toTrack, "lines-copy") end
  local pattern, to_pattern = song:pattern(p), song:pattern(to_p)
  local first, last = line_range(input, pattern.number_of_lines)
  local count = last - first + 1
  local at = first
  if input.line ~= nil then
    local line = input.line
    if type(line) ~= "number" or line ~= math.floor(line) or line < 0 or line + count > to_pattern.number_of_lines then
      bad(string.format("the block is %d lines: line is where it goes, from 0 to %d in that pattern", count, to_pattern.number_of_lines - count))
    end
    at = line + 1
  elseif at + count - 1 > to_pattern.number_of_lines then
    bad(string.format("the block is %d lines: line is where it goes, from 0 to %d in that pattern", count, to_pattern.number_of_lines - count))
  end
  local to_pattern_track = to_pattern:track(to_index)
  if to_pattern_track.is_alias then
    bad(string.format("that track in pattern %02d is an alias of pattern %02d: its lines are that pattern's, so write them there", to_p - 1, to_pattern_track.alias_pattern_index - 1))
  end
  if input.move ~= nil and type(input.move) ~= "boolean" then bad("move must be true or false") end
  local move = input.move == true
  if move and pattern:track(index).is_alias then
    bad(string.format("that track in pattern %02d is an alias of pattern %02d: move its lines there", p - 1, pattern:track(index).alias_pattern_index - 1))
  end
  if p == to_p and index == to_index and at == first then bad("the block is where it would go") end
  return {
    p = p, index = index, track = track, pattern = pattern, pattern_track = pattern:track(index), first = first, last = last,
    to_p = to_p, to_index = to_index, to_track = to_track, to_pattern = to_pattern, to_pattern_track = to_pattern_track, at = at,
    count = count, move = move,
  }
end

-- What the copy writes: the destination's cells that differ from the block's, and (a move) the source's cells left behind
-- emptied. The block is read first, so a destination over its own source takes the block as it was.
local function block_plan(block)
  local source = {}
  for l = 0, block.count - 1 do
    local line = block.pattern_track:line(block.first + l)
    source[l] = line.is_empty and false or {}
    if source[l] then
      for c = 1, 12 do source[l]["n" .. c] = read_cell(block.pattern_track, { line = block.first + l, kind = "note", col = c }) end
      for c = 1, 8 do source[l]["e" .. c] = read_cell(block.pattern_track, { line = block.first + l, kind = "effect", col = c }) end
    end
  end
  -- (what the block holds, for the fence: a copy writes what the preview read)
  local held = {}
  for l = 0, block.count - 1 do
    for _, cell in pairs(source[l] or {}) do
      if not is_empty_cell(cell) then held[#held + 1] = cell end
    end
  end
  table.sort(held, cell_order)
  local to_prior, to_after, written = {}, {}, {}
  local has_notes = false
  for l = 0, block.count - 1 do
    local dst_line = block.at + l
    local dst_empty = block.to_pattern_track:line(dst_line).is_empty
    if source[l] or not dst_empty then
      for _, kind in ipairs({ "note", "effect" }) do
        for c = 1, kind == "note" and 12 or 8 do
          local now = read_cell(block.to_pattern_track, { line = dst_line, kind = kind, col = c })
          local from = source[l] and source[l][(kind == "note" and "n" or "e") .. c]
          local target = { line = dst_line, kind = kind, col = c }
          if from then
            for k, v in pairs(from) do if k ~= "line" and k ~= "kind" and k ~= "col" then target[k] = v end end
          else
            local empty = kind == "note" and EMPTY_NOTE or EMPTY_EFFECT
            for k, v in pairs(empty) do target[k] = v end
          end
          if kind == "note" and not is_empty_cell(target) then has_notes = true end
          if not same_cell(now, target) then to_prior[#to_prior + 1], to_after[#to_after + 1] = now, target end
          if block.p == block.to_p and block.index == block.to_index then written[dst_line] = true end
        end
      end
    end
  end
  if has_notes and (block.to_track.type ~= renoise.Track.TRACK_TYPE_SEQUENCER or block.to_track.max_note_columns == 0) then
    bad(base.named(block.to_index, block.to_track) .. " has no note columns, and the block has notes")
  end
  local from_prior, from_after = {}, {}
  if block.move then
    for l = 0, block.count - 1 do
      local src_line = block.first + l
      if source[l] and not written[src_line] then
        for key, cell in pairs(source[l]) do
          if not is_empty_cell(cell) then
            local target = { line = src_line, kind = cell.kind, col = cell.col }
            for k, v in pairs(key:sub(1, 1) == "n" and EMPTY_NOTE or EMPTY_EFFECT) do target[k] = v end
            from_prior[#from_prior + 1], from_after[#from_after + 1] = cell, target
          end
        end
      end
    end
    table.sort(from_prior, cell_order)
    table.sort(from_after, cell_order)
  end
  return to_prior, to_after, from_prior, from_after, fingerprint(held)
end

local function block_title(block, count)
  local verb = block.move and "Moved" or "Copied"
  local from = string.format("%s on %s", q(pattern_label(block.pattern, block.p)), q(base.named(block.index, block.track)))
  -- (lines by Renoise's numbers, as it shows them)
  local lines = string.format("lines %02d to %02d", block.first - 1, block.last - 1)
  if block.first == block.last then lines = string.format("line %02d", block.first - 1) end
  local to
  if block.p == block.to_p and block.index == block.to_index then
    to = string.format("line %02d", block.at - 1)
  else
    to = string.format("line %02d of %s on %s", block.at - 1, q(pattern_label(block.to_pattern, block.to_p)), q(base.named(block.to_index, block.to_track)))
  end
  return string.format("%s %s of %s to %s", verb, lines, from, to)
end

local function block_preview(song, env, input)
  local block = block_of(song, env, input)
  local to_prior, to_after, from_prior, from_after, held = block_plan(block)
  local writes = writes_of(to_after) + writes_of(from_after)
  if writes > M.MAX_WRITES then bad(string.format("that is %d writes, more than one change takes (%d): split it into several", writes, M.MAX_WRITES)) end
  local display = { title = block_title(block), before = counts_of(to_prior), after = counts_of(to_after) }
  if #to_after == 0 and #from_after == 0 then display.unchanged = true end
  local needs = needs_of(to_after)
  if needs.note > (block.to_track.visible_note_columns or 0) then display.note = "It shows note column " .. needs.note .. " first." end
  return {
    target = target_of(env, block.to_p, block.to_index),
    prior = { cells = to_prior, columns = read_columns(block.to_track), source = { cells = from_prior } },
    fence = {
      cells = #to_prior, hash = fingerprint(to_prior), source = #from_prior, sourceHash = fingerprint(from_prior), block = held,
      id = base.track_id(block.to_track), from = base.track_id(block.track), label = base.named(block.to_index, block.to_track),
    },
    display = display,
  }, block, to_prior, to_after, from_prior, from_after
end

local function block_apply(song, env, input, fence, desc)
  local ok, preview, block, to_prior, to_after, from_prior, from_after = pcall(block_preview, song, env, input)
  if not ok then base.gone_is_changed(preview) end
  if fence.id ~= preview.fence.id or fence.from ~= preview.fence.from then fail("changed", base.moved_text(fence.label)) end
  if fence.hash ~= preview.fence.hash or fence.sourceHash ~= preview.fence.sourceHash or fence.cells ~= preview.fence.cells
    or fence.block ~= preview.fence.block then
    fail("changed", "the lines changed since the preview")
  end
  local display = preview.display
  local applied = { cells = {}, columns = preview.prior.columns, source = { cells = {} } }
  if #to_after == 0 and #from_after == 0 then
    return { applied = applied, display = display, undo = { desc = desc, entry = false }, prior = preview.prior, target = preview.target }
  end
  local hidden = hidden_use(song, block.to_index, block.to_track, needs_of(to_after))
  song:describe_undo(desc)
  local wrote, err = pcall(function()
    raise_columns(block.to_track, needs_of(to_after))
    for _, cell in ipairs(to_after) do write_cell(block.to_pattern_track, cell) end
    for _, cell in ipairs(from_after) do write_cell(block.pattern_track, cell) end
  end)
  local function back()
    pcall(function()
      for _, cell in ipairs(to_prior) do write_cell(block.to_pattern_track, cell) end
      for _, cell in ipairs(from_prior) do write_cell(block.pattern_track, cell) end
    end)
    set_columns(block.to_track, preview.prior.columns)
  end
  if not wrote then
    back()
    fail("failed", "Renoise wouldn't take those lines: " .. (type(err) == "table" and err.message or tostring(err)))
  end
  local to_result, from_result = {}, {}
  for i, cell in ipairs(to_after) do
    to_result[i] = read_cell(block.to_pattern_track, cell)
    if not same_cell(to_result[i], cell) then back() fail("failed", string.format("Renoise didn't keep line %02d column %d as written", cell.line - 1, cell.col)) end
  end
  for i, cell in ipairs(from_after) do from_result[i] = read_cell(block.pattern_track, cell) end
  local columns = read_columns(block.to_track)
  columns.hidden = hidden
  applied = {
    cells = to_result, columns = columns,
    touch = env.pattern_touch_mark and env.pattern_touch_mark(block.to_p) or nil, lines = block.to_pattern.number_of_lines,
    source = {
      cells = from_result, touch = env.pattern_touch_mark and env.pattern_touch_mark(block.p) or nil, lines = block.pattern.number_of_lines,
    },
    instruments = instruments_of(song, env, to_prior, to_result, from_prior, from_result),
  }
  return {
    applied = applied, display = { title = display.title, before = display.before, after = counts_of(to_result), note = display.note },
    -- (where the block came from: the source pattern's track, a ref the layout carries)
    material = { source = refs.make(env.epoch(), "ptrack", block.p, block.index) },
    undo = { desc = desc, entry = true }, prior = preview.prior, place = { pattern = block.to_p, track = block.to_index },
    target = preview.target,
  }
end

local function block_restore(song, env, restore, desc)
  local p, index, track, pattern_track, pattern = locate_restored(song, env, restore)
  local prior, applied = restore.prior, restore.applied
  if type(prior) ~= "table" or type(applied) ~= "table" or type(prior.source) ~= "table" or type(applied.source) ~= "table" then
    bad("prior and applied: the lines the copy wrote")
  end
  local source_track, source_pattern_track, source_pattern, sp
  if #applied.source.cells > 0 then
    if type(restore.material) ~= "table" then bad("material: where the block came from") end
    local numbers = refs.expect(restore.material.source, "ptrack", 2, env.epoch())
    sp = numbers[1]
    if sp > #song.patterns then fail("gone", "the pattern the block came from isn't in the song any more") end
    source_track = base.track_at(song, numbers[2])
    source_pattern, source_pattern_track = song:pattern(sp), song:pattern(sp):track(numbers[2])
    if source_pattern_track.is_alias then fail("changed", "the track the block came from is an alias now") end
  end
  local function changed(p_, mark)
    if not env.pattern_touched_since then return false end
    return env.pattern_touched_since(p_, mark)
  end
  local label = string.format("%s on %s", q(pattern_label(pattern, p)), q(base.named(index, track)))
  local map = instruments_back(song, env, applied.instruments, label)
  local todo = restore_plan(pattern_track, restore, pattern.number_of_lines, function(mark) return changed(p, mark) end, map)
  local source_todo = {}
  if source_pattern_track then
    source_todo = restore_plan(source_pattern_track, { prior = prior.source, applied = applied.source }, source_pattern.number_of_lines,
      function(mark) return changed(sp, mark) end, map)
  end
  local spec = base.KINDS["lines-copy"]
  if #todo == 0 and #source_todo == 0 then
    lower_columns(song, index, track, prior.columns, applied.columns)
    base.note_restored(env.journal, "lines-copy", { pattern = p, track = index }, applied, spec)
    return { restored = { done = true }, display = { title = "Lines in " .. label .. " are back already" }, undo = { desc = desc, entry = false }, unwritten = true }
  end
  song:describe_undo(desc)
  for _, cell in ipairs(todo) do write_cell(pattern_track, cell) end
  for _, cell in ipairs(source_todo) do write_cell(source_pattern_track, cell) end
  lower_columns(song, index, track, prior.columns, applied.columns)
  base.note_restored(env.journal, "lines-copy", { pattern = p, track = index }, applied, spec)
  return { restored = { done = true }, display = { title = "Lines put back in " .. label }, undo = { desc = desc, entry = true } }
end

base.register({
  kind = "lines-copy", custom = true, preview = function(song, env, input) return (block_preview(song, env, input)) end,
  apply = block_apply, restore = block_restore,
  same = same_state, unsure_redo = true,
  reread = base.KINDS["notes-write"].reread, read = base.KINDS["notes-write"].read,
})

-- (the plain rule's words, for the other kinds that write lines: M4 n-song)
M.CHANGED = CHANGED

-- n-instruments: the cells' text, plan and writes, for a phrase's lines (changes_phrases.lua): a phrase's lines are PatternLines.
M.cells = {
  read_as_note = read_as_note,
  line_range = line_range, parse_cells = parse_cells, add_cleared = add_cleared, plan_of = plan_of, read_cell = read_cell, write_cell = write_cell,
  same_cell = same_cell, same_cells = same_cells, is_empty_cell = is_empty_cell, fingerprint = fingerprint, writes_of = writes_of,
  counts_of = counts_of, needs_of = needs_of, lines_in = lines_in,
  -- M7 (variations): a pattern track's columns, for slot-variation (changes_sequence.lua)
  read_columns = read_columns, raise_columns = raise_columns, set_columns = set_columns, hidden_use = hidden_use, lower_columns = lower_columns,
  pattern_of = pattern_of,
}

return M
