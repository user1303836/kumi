-- M4 (n-song): Kumi's changes to a pattern itself (its name and number of lines) and to a pattern's track (its slot colour in the
-- pattern matrix). A pattern is named by its ref (`E:pattern:p`) or by a sequence slot's (`E:seq:s`, the pattern it plays); a
-- pattern's track by the pattern and a track ref, kept as `E:ptrack:p.t`. Records keep the pattern as a place in the pool (its
-- log since then followed, as the tracks are through the layout), so patterns made or deleted since leave them working; a pool
-- whose changes can't be followed makes them `stale`, and nothing is written.
--
-- Measured in Renoise 3.5.4 (lab, 2026-10-19): the name, the number of lines and the slot colour are undo steps; a pattern
-- made shorter keeps the lines past its end (they show again when it is made longer), and its automation points too.

local import = ...
local json = import("json")
local refs = import("refs")
local rows = import("rows")
local base = import("changes_base")

local bad, fail, the_song = base.bad, base.fail, base.the_song
local q = base.q
local M = {}

function M.pattern_label(song, p)
  local name = song:pattern(p).name
  if name == nil or name == "" then name = string.format("Pattern %02d", p - 1) end
  return name
end

-- The pattern a ref names: a pattern's, or a sequence slot's (the pattern it plays).
function M.pattern_of(song, env, reference, what)
  if type(reference) ~= "string" then bad(what .. ": a pattern ref, or the ref of a sequence slot that plays it") end
  local parsed = refs.parse(reference)
  if parsed and parsed.kind == "seq" and #parsed.n == 1 then
    local s = refs.expect(reference, "seq", 1, env.epoch())[1]
    local sequence = song.sequencer.pattern_sequence
    if s > #sequence then fail("gone", "that sequence slot isn't in the song any more") end
    return sequence[s]
  end
  local p = refs.expect(reference, "pattern", 1, env.epoch())[1]
  if p > #song.patterns then fail("gone", "that pattern isn't in the song any more") end
  return p
end

local function pattern_target(env, p) return { ref = refs.make(env.epoch(), "pattern", p), kind = "pattern" } end

local function pattern_reread(song, place)
  if place.pattern < 1 or place.pattern > #song.patterns then return nil end
  return song:pattern(place.pattern)
end

-- pattern-name

base.register({
  kind = "pattern-name", noun = "name", undoable = true,
  locate = function(song, env, input)
    local p = input.material and base.pattern_now(song, env, input.material.pattern) or M.pattern_of(song, env, input.ref, "pattern-name")
    return { object = song:pattern(p), name = M.pattern_label(song, p), place = { pattern = p }, target = pattern_target(env, p) }
  end,
  material = function(env, loc) return { pattern = base.pattern_place(env, loc.place.pattern) } end,
  restore_input = function(restore) return { material = restore.material } end,
  reread = pattern_reread,
  read = function(pattern) return { name = pattern.name } end,
  wanted = function(_, input) return { name = base.text_of(input.name, "name", 256) } end,
  write = function(pattern, state) pattern.name = state.name end,
  same = function(a, b) return a.name == b.name end,
  title = function(_, from, to)
    if to.name == "" then return "Pattern " .. q(from.name) .. " unnamed" end
    if from.name == "" then return "Named the pattern " .. q(to.name) end
    return "Renamed pattern " .. q(from.name) .. " \226\134\146 " .. q(to.name)
  end,
})

-- slot-color: the colour of a pattern's track in the pattern matrix (every slot that plays the pattern shows it).

-- A pattern's track: a ptrack ref, or a pattern (or slot) ref and a track ref.
function M.pattern_track_of(song, env, input, what)
  local parsed = type(input.ref) == "string" and refs.parse(input.ref) or nil
  local p, t
  if parsed and parsed.kind == "ptrack" then
    local numbers = refs.expect(input.ref, "ptrack", 2, env.epoch())
    p, t = numbers[1], numbers[2]
    if p > #song.patterns then fail("gone", "that pattern isn't in the song any more") end
    base.track_at(song, t)
  else
    p = M.pattern_of(song, env, input.ref, what)
    t = base.find(song, env, input.track, what)
  end
  return p, t, song:track(t)
end

local function color_text(color) return color and color or "none" end

base.register({
  kind = "slot-color", noun = "slot colour", undoable = true,
  locate = function(song, env, input)
    local p, t, track
    if input.material then
      p, t = base.pattern_now(song, env, input.material.pattern), input.material.track
      track = base.track_at(song, t)
    else
      p, t, track = M.pattern_track_of(song, env, input, "slot-color")
    end
    return {
      object = song:pattern(p):track(t), name = string.format("%s on %s", q(M.pattern_label(song, p)), q(base.named(t, track))),
      place = { pattern = p, track = t }, target = { ref = refs.make(env.epoch(), "ptrack", p, t), kind = "pattern-track" },
      id = base.track_id(track),
    }
  end,
  reread = function(song, place)
    if place.pattern > #song.patterns or place.track > rows.track_count(song) then return nil end
    return song:pattern(place.pattern):track(place.track)
  end,
  read = function(pattern_track) return { color = base.hex_of(pattern_track.color) or false } end,
  wanted = function(_, input)
    if input.color == json.null or input.color == false then return { color = false } end
    local color = base.rgb_of(input.color)
    if not color then bad("color must be #rrggbb, or null for none") end
    return { color = base.hex_of(color) }
  end,
  write = function(pattern_track, state)
    if state.color then pattern_track.color = base.rgb_of(state.color) else pattern_track.color = nil end
  end,
  same = function(a, b) return a.color == b.color end,
  title = function(name, _, to)
    if not to.color then return "Slot colour of " .. name .. " cleared" end
    return "Slot colour of " .. name .. " " .. color_text(to.color)
  end,
  watch = function(env, loc)
    if env.watch_pattern_track then env.watch_pattern_track(loc.place.pattern, loc.place.track, "color_observable") end
  end,
  -- (the track under `track`, which the layout carries; the pattern as a place in the pool)
  material = function(env, loc) return { pattern = base.pattern_place(env, loc.place.pattern), track = loc.place.track } end,
  restore_input = function(restore) return { material = restore.material } end,
})

-- pattern-length: lines past a shorter pattern's end are kept by Renoise, hidden; a longer pattern shows what was kept there.
-- Kumi's undo puts the length back only while those lines (the ones between the two lengths, in every track, with their
-- automation points) are as the change left them: the producer's lines written into a pattern Kumi made longer would go out of
-- sight.

-- The lines from `from` to `to` of every track of the pattern, and the automation points in them, as a short fingerprint ("none"
-- when they hold nothing). (An empty pattern track's lines aren't read: only its automation.) A line is read as its text, every
-- column of it and the hidden ones too, and hashed with its empty note columns packed to a mark that keeps their places: 28,672
-- dense lines took 549 ms, where reading each field took 2,086 and hashing the text whole 958 (Renoise 3.5.4, lab 2026-11-01).
-- Marked, so a record from before this form (16 hex digits, read field by field) can be told from lines that changed.
local EMPTY_NOTE_COLUMN = "%-%-%-%.%.%.%.%.%.%.%.0000 | "
local REGION_MARK = "t"
local function region_print(song, p, from, to)
  if from > to then return "none" end
  local pattern = song:pattern(p)
  local parts = {}
  for t = 1, rows.track_count(song) do
    local pattern_track = pattern:track(t)
    if not pattern_track.is_alias then
      for l = from, pattern_track.is_empty and from - 1 or to do
        local line = pattern_track:line(l)
        if not line.is_empty then parts[#parts + 1] = t .. ":" .. l .. "=" .. tostring(line) .. ";" end
      end
      for k, automation in ipairs(pattern_track.automation) do
        for _, point in ipairs(automation.points) do
          if point.time >= from and point.time < to + 1 then parts[#parts + 1] = string.format("a%d:%d %.9g %.9g %.9g;", t, k, point.time, point.value, point.scaling) end
        end
      end
    end
  end
  if #parts == 0 then return "none" end
  return REGION_MARK .. base.hash((table.concat(parts):gsub(EMPTY_NOTE_COLUMN, "_")))
end
M.region_print = region_print

-- (renoise.Pattern.MAX_NUMBER_OF_LINES)
local MAX_LINES = 512

local function length_input(input)
  local n = input.lines
  if type(n) ~= "number" or n ~= math.floor(n) or n < 1 or n > MAX_LINES then
    bad("lines must be a whole number from 1 to 512")
  end
  return n
end

local function length_title(label, lines) return string.format("%s %d lines", label, lines) end

-- (the lines by Renoise's numbers, from 00)
-- (`region`: the region's print, read once a callback)
local function length_note(old, new, region)
  if new < old then
    return string.format("Lines %02d to %02d are kept, hidden: making it longer again shows them.", new, old - 1)
  end
  if new > old and region ~= "none" then
    return string.format("Lines %02d to %02d hold what was kept there, hidden: it shows again.", old, new - 1)
  end
  return nil
end

local function length_locate(song, env, reference)
  local p = M.pattern_of(song, env, reference, "pattern-length")
  return p, song:pattern(p), M.pattern_label(song, p)
end

local function length_preview(song, env, input)
  local p, pattern, label = length_locate(song, env, input.ref)
  local old, new = pattern.number_of_lines, length_input(input)
  local low, high = math.min(old, new), math.max(old, new)
  local region = region_print(song, p, low + 1, high)
  local display = { title = length_title(label, new), before = { lines = old }, after = { lines = new }, note = length_note(old, new, region) }
  if old == new then display.unchanged = true end
  return {
    target = pattern_target(env, p), prior = { lines = old },
    fence = { lines = old, region = region, name = pattern.name },
    display = display,
  }
end

local function length_apply(song, env, input, fence, desc)
  local ok, p, pattern, label = pcall(length_locate, song, env, input.ref)
  if not ok then base.gone_is_changed(p) end
  local old, new = pattern.number_of_lines, length_input(input)
  local low, high = math.min(old, new), math.max(old, new)
  local region = region_print(song, p, low + 1, high)
  if old ~= fence.lines or pattern.name ~= fence.name or region ~= fence.region then
    fail("changed", label .. " changed since the preview")
  end
  local display = { title = length_title(label, new), before = { lines = old }, after = { lines = new }, note = length_note(old, new, region) }
  if old == new then
    display.unchanged = true
    return { applied = { lines = new }, display = display, undo = { desc = desc, entry = false }, prior = { lines = old } }
  end
  song:describe_undo(desc)
  pattern.number_of_lines = new
  if pattern.number_of_lines ~= new then fail("failed", "Renoise kept " .. label .. " at " .. pattern.number_of_lines .. " lines") end
  return {
    applied = { lines = new, region = fence.region, from = low + 1, to = high }, display = display, undo = { desc = desc, entry = true },
    prior = { lines = old }, place = { pattern = p }, target = pattern_target(env, p),
    material = { pattern = base.pattern_place(env, p) },
  }
end

local function length_restore(song, env, restore, desc)
  local prior, applied = restore.prior, restore.applied
  if type(prior) ~= "table" or type(applied) ~= "table" or type(prior.lines) ~= "number" or type(applied.lines) ~= "number" then
    bad("prior and applied: the pattern's lines before and after")
  end
  local p
  if type(restore.material) == "table" then
    p = base.pattern_now(song, env, restore.material.pattern)
  else
    p = M.pattern_of(song, env, restore.target and restore.target.ref, "pattern-length")
  end
  local pattern, label = song:pattern(p), M.pattern_label(song, p)
  local now = pattern.number_of_lines
  local title = length_title(label, prior.lines) .. " (put back)"
  if now == prior.lines then
    return { restored = { lines = now }, display = { title = title }, undo = { desc = desc, entry = false }, unwritten = true }
  end
  if now ~= applied.lines then fail("changed", label .. "'s length changed in Renoise since Kumi set it") end
  if not restore.discard and type(applied.from) == "number" and applied.region ~= "none" and tostring(applied.region):sub(1, 1) ~= REGION_MARK then
    fail("changed", string.format("an older version of Kumi's tool set %s's length, and this one can't tell whether lines %02d to %02d changed since: putting it back could hide them",
      label, applied.from - 1, applied.to - 1))
  end
  if not restore.discard and type(applied.from) == "number" and region_print(song, p, applied.from, applied.to) ~= applied.region then
    fail("changed", string.format("lines %02d to %02d of %s changed in Renoise since Kumi set its length: putting it back would hide them",
      applied.from - 1, applied.to - 1, label))
  end
  song:describe_undo(desc)
  pattern.number_of_lines = prior.lines
  base.note_restored(env.journal, "pattern-length", { pattern = p }, applied, base.KINDS["pattern-length"])
  return { restored = { lines = prior.lines }, display = { title = title }, undo = { desc = desc, entry = true } }
end

base.register({
  kind = "pattern-length", custom = true,
  preview = length_preview, apply = length_apply, restore = length_restore,
  reread = pattern_reread,
  read = function(pattern) return { lines = pattern.number_of_lines } end,
  same = function(a, b) return a.lines == b.lines end,
})

-- pattern-alias: a pattern's track plays another pattern's lines (an alias, as the matrix shows it), or stops being one.
--
-- Measured in Renoise 3.5.4: making a track an alias drops its own lines (only Renoise's undo brings them back), so Kumi keeps
-- them, and the automation of that pattern track, to write back; taking an alias away with `alias_pattern_index = 0` leaves the
-- track empty, and `copy_from` the source leaves a copy of its lines (`keep`, the default). An alias of a track that is itself an
-- alias is refused by Renoise: the source it shows is taken. A change of lines, so Kumi's undo goes through only while nothing
-- in the pattern changed since (the plain rule, changes_notes.lua), and while the alias is as Kumi left it.

local material_ = import("changes_material")
local notes_ = import("changes_notes")

-- The automation of one pattern's track, by the device's place on the track (followed through its devices' log: places.lua)
-- and the parameter's number.
local function read_pattern_automation(song, env, p, t)
  local track, pattern_track, out = song:track(t), song:pattern(p):track(t), {}
  for d = 1, #track.devices do
    local device = track:device(d)
    for k = 1, #device.parameters do
      local parameter = device:parameter(k)
      if parameter.is_automated then
        local automation = pattern_track:find_automation(parameter)
        if automation then
          local points = {}
          for i, point in ipairs(automation.points) do points[i] = { time = point.time, value = point.value, scaling = point.scaling } end
          out[#out + 1] = { device = material_.device_place(env, t, d, device), parameter = k, playmode = automation.playmode, points = points }
        end
      end
    end
  end
  return out
end

-- A pattern track's own lines and automation as a short fingerprint (what an apply checks the preview saw).
local function own_print(song, p, t)
  local pattern_track = song:pattern(p):track(t)
  if pattern_track.is_alias then return "alias:" .. pattern_track.alias_pattern_index end
  local lines = material_.read_lines(pattern_track, MAX_LINES)
  local parts = { tostring(#lines) }
  for _, row in ipairs(lines) do
    for _, cell in ipairs(row.notes) do
      parts[#parts + 1] = string.format("%d:n%d:%d:%d:%d:%d:%d:%d:%d", row.line, cell.col, cell.note, cell.instrument, cell.volume, cell.panning, cell.delay, cell.fxn, cell.fxa)
    end
    for _, cell in ipairs(row.effects) do parts[#parts + 1] = string.format("%d:e%d:%d:%d", row.line, cell.col, cell.number, cell.amount) end
  end
  for k, automation in ipairs(pattern_track.automation) do
    for _, point in ipairs(automation.points) do parts[#parts + 1] = string.format("a%d:%.9g:%.9g:%.9g", k, point.time, point.value, point.scaling) end
  end
  return base.hash(table.concat(parts, "|"))
end

local function alias_input(song, env, input)
  local p, t, track = M.pattern_track_of(song, env, input, "pattern-alias")
  local pattern_track = song:pattern(p):track(t)
  local now = pattern_track.alias_pattern_index
  local want
  if input.alias == json.null or input.alias == false then
    want = 0
  else
    local source = M.pattern_of(song, env, input.alias, "pattern-alias")
    -- (an alias of an alias is refused by Renoise: the source it shows is meant)
    local through = song:pattern(source):track(t).alias_pattern_index
    if through > 0 then source = through end
    if source == p then bad("a pattern's track can't be an alias of its own pattern") end
    want = source
  end
  if input.keep ~= nil and type(input.keep) ~= "boolean" then bad("keep must be true or false") end
  return { p = p, t = t, track = track, pattern_track = pattern_track, now = now, want = want, keep = input.keep ~= false }
end

local function alias_title(song, plan)
  local label = string.format("%s on %s", q(M.pattern_label(song, plan.p)), q(base.named(plan.t, plan.track)))
  if plan.want > 0 then return string.format("%s plays %s's lines (an alias)", label, q(M.pattern_label(song, plan.want))) end
  return string.format("%s is no alias any more%s", label, plan.keep and " (a copy of its lines)" or " (empty)")
end

local function alias_preview(song, env, input)
  local plan = alias_input(song, env, input)
  local display = { title = alias_title(song, plan), before = { alias = plan.now }, after = { alias = plan.want } }
  if plan.now == plan.want then display.unchanged = true end
  if plan.want > 0 and plan.now == 0 and not plan.pattern_track.is_empty then
    display.note = "Its own lines go out of sight (Renoise drops them); Kumi keeps them to put back."
  end
  return {
    target = { ref = refs.make(env.epoch(), "ptrack", plan.p, plan.t), kind = "pattern-track" }, prior = { alias = plan.now },
    fence = { alias = plan.now, own = own_print(song, plan.p, plan.t), id = base.track_id(plan.track), label = base.named(plan.t, plan.track) },
    display = display,
  }
end

local function alias_apply(song, env, input, fence, desc)
  local ok, plan = pcall(alias_input, song, env, input)
  if not ok then base.gone_is_changed(plan) end
  if fence.id ~= nil and fence.id ~= base.track_id(plan.track) then fail("changed", base.moved_text(fence.label)) end
  if plan.now ~= fence.alias or own_print(song, plan.p, plan.t) ~= fence.own then
    fail("changed", string.format("%s on %s changed since the preview", q(M.pattern_label(song, plan.p)), q(base.named(plan.t, plan.track))))
  end
  local display = { title = alias_title(song, plan), before = { alias = plan.now }, after = { alias = plan.want } }
  local target = { ref = refs.make(env.epoch(), "ptrack", plan.p, plan.t), kind = "pattern-track" }
  if plan.now == plan.want then
    display.unchanged = true
    return { applied = { alias = plan.want }, display = display, undo = { desc = desc, entry = false }, prior = { alias = plan.now }, target = target }
  end
  local material
  if plan.want > 0 and plan.now == 0 then
    local lines, writes = material_.read_lines(plan.pattern_track, MAX_LINES)
    if writes > base.MAX_APPLY_WRITES then
      fail("failed", string.format("its own lines are more than Kumi puts back in one step (%d writes): clear them first, or make the alias in Renoise", writes))
    end
    -- (the instruments its notes play, each to be found again where it is when the lines go back: Renoise renumbers notes as
    -- instruments are made, deleted or moved, but not kept lines)
    local values = {}
    for _, row in ipairs(lines) do
      for _, cell in ipairs(row.notes) do
        if cell.instrument ~= 255 then values[cell.instrument] = true end
      end
    end
    material = {
      lines = lines, automation = read_pattern_automation(song, env, plan.p, plan.t), instruments = material_.keep_instruments(song, env, values),
    }
  else
    material = { alias = plan.now }
  end
  -- (the pattern and the patterns it was and is an alias of, as places in the pool; the track under `track`, which the layout
  -- carries)
  material.track, material.pattern = plan.t, base.pattern_place(env, plan.p)
  if plan.now > 0 then material.was = base.pattern_place(env, plan.now) end
  if plan.want > 0 then material.source = base.pattern_place(env, plan.want) end
  song:describe_undo(desc)
  if plan.want > 0 then
    plan.pattern_track.alias_pattern_index = plan.want
  elseif plan.keep then
    plan.pattern_track:copy_from(song:pattern(plan.now):track(plan.t))
  else
    plan.pattern_track.alias_pattern_index = 0
  end
  if plan.pattern_track.alias_pattern_index ~= plan.want then fail("failed", "Renoise didn't take the alias as asked (Renoise's own undo takes back what it did)") end
  return {
    applied = {
      alias = plan.want, touch = env.pattern_touch_mark and env.pattern_touch_mark(plan.p) or nil, own = own_print(song, plan.p, plan.t),
      before = fence.own,
    },
    material = material, display = display, undo = { desc = desc, entry = true }, prior = { alias = plan.now },
    place = { pattern = plan.p, track = plan.t }, target = target,
  }
end

local function alias_restore(song, env, restore, desc)
  local applied, material, prior = restore.applied, restore.material, restore.prior
  if type(applied) ~= "table" or type(prior) ~= "table" or type(material) ~= "table" then bad("applied and material: what the change did and kept") end
  local p, t
  if type(material.pattern) == "table" then
    p, t = base.pattern_now(song, env, material.pattern), material.track
  else
    local numbers = refs.expect(restore.target and restore.target.ref, "ptrack", 2, env.epoch())
    p, t = numbers[1], numbers[2]
    if p > #song.patterns then fail("gone", "that pattern isn't in the song any more") end
  end
  local track = base.track_at(song, t)
  local pattern_track = song:pattern(p):track(t)
  -- (the patterns it was and is an alias of, where they are now)
  local was = prior.alias > 0 and (material.was and base.pattern_now(song, env, material.was) or material.alias) or 0
  local want = applied.alias > 0 and (material.source and base.pattern_now(song, env, material.source) or applied.alias) or 0
  local label = string.format("%s on %s", q(M.pattern_label(song, p)), q(base.named(t, track)))
  local title = label .. (prior.alias > 0 and " is an alias again" or " is no alias again") .. " (put back)"
  local now = pattern_track.alias_pattern_index
  -- back already: the alias it was, or (an alias Kumi made of a track with lines of its own) those lines as they were
  if (was > 0 and now == was) or (prior.alias == 0 and now == 0 and own_print(song, p, t) == applied.before) then
    return { restored = { alias = now }, display = { title = title }, undo = { desc = desc, entry = false }, unwritten = true }
  end
  if now ~= want then fail("changed", label .. " was made an alias or not in Renoise since Kumi changed it") end
  if env.pattern_touched_since then
    local since = env.pattern_touched_since(p, applied.touch)
    if since == nil then fail("changed", "Kumi can't tell whether the pattern changed in Renoise since (the tool watched the song anew), so Kumi left it as it is") end
    if since then fail("changed", notes_.CHANGED) end
  end
  if applied.alias == 0 and own_print(song, p, t) ~= applied.own then
    fail("changed", label .. "'s lines or automation changed in Renoise since Kumi took the alias away")
  end
  local instruments
  if material.lines then
    local why
    instruments, why = material_.instruments_now(song, env, material.instruments)
    if not instruments and why == "unknown" then
      fail("stale", string.format("the instruments changed in a way Kumi couldn't follow since it made %s an alias, so it can't tell which its notes play: nothing was put back", label))
    elseif not instruments and not restore.discard then
      fail("changed", string.format("the instrument %s that %s's notes play was deleted in Renoise since: Kumi won't put them back playing another", q(why), label))
    end
  end
  song:describe_undo(desc)
  local note
  if material.lines then
    pattern_track.alias_pattern_index = 0
    material_.write_lines(song, t, { pattern = p, lines = material.lines }, instruments)
    local lost = 0
    for _, saved in ipairs(material.automation or {}) do
      local d = material_.device_now(song, env, t, saved.device)
      if d then
        local parameter = track:device(d):parameter(saved.parameter)
        local automation = pattern_track:find_automation(parameter) or pattern_track:create_automation(parameter)
        automation.playmode = saved.playmode
        automation.points = saved.points
      else
        lost = lost + 1
      end
    end
    if lost > 0 then
      note = string.format("Its automation of %s isn't put back: the devices on the track changed in Renoise since.", lost == 1 and "1 parameter" or (lost .. " parameters"))
    end
  else
    pattern_track.alias_pattern_index = was
  end
  base.note_restored(env.journal, "pattern-alias", { pattern = p, track = t }, applied, base.KINDS["pattern-alias"])
  return { restored = { alias = pattern_track.alias_pattern_index }, display = { title = title, note = note }, undo = { desc = desc, entry = true } }
end

base.register({
  kind = "pattern-alias", custom = true, uses_patterns = true,
  preview = alias_preview, apply = alias_apply, restore = alias_restore,
  reread = function(song, place)
    if place.pattern > #song.patterns or place.track > rows.track_count(song) then return nil end
    return song:pattern(place.pattern):track(place.track)
  end,
  read = function(pattern_track) return { alias = pattern_track.alias_pattern_index } end,
  same = function(a, b) return a.alias == b.alias end,
})

-- automation-write: a device parameter's automation in one pattern (its envelope in that pattern's track): points added or
-- changed, removed, a range cleared, the play mode, or the whole envelope deleted. A request's time is in lines from 0, by
-- Renoise's numbers (fractions to 1/256 of a line, as Renoise keeps them; a tick is 1/TPL of a line), and Renoise's own from 1; a point's value is 0 to 1 of the parameter's range (measured:
-- linear in its own value: Gainer's gain 0..4, a point of 0.25 is 1.0), or `paramValue` in the parameter's own range.
-- The record keeps the envelope's points before and after, whole: Kumi's undo puts them back only while they are as it left them.

local PLAYMODES = { points = 1, linear = 2, curves = 3 }
local PLAYMODE_NAMES = { "points", "linear", "curves" }
local MOST_POINTS = 1024

function M.parameter_of(song, env, input, what)
  local parsed = type(input.ref) == "string" and refs.parse(input.ref) or nil
  local t, d, k
  if parsed and parsed.kind == "param" then
    local numbers = refs.expect(input.ref, "param", 3, env.epoch())
    t, d, k = numbers[1], numbers[2], numbers[3]
  elseif parsed and parsed.kind == "device" then
    local numbers = refs.expect(input.ref, "device", 2, env.epoch())
    t, d = numbers[1], numbers[2]
  else
    bad(what .. ": ref is the parameter, or its device with param")
  end
  local track = base.track_at(song, t)
  if d < 1 or d > #track.devices then fail("gone", "that device isn't on the track any more") end
  local device = track:device(d)
  if not k then
    local wanted, count = input.param, #device.parameters
    if type(wanted) == "number" then
      if wanted ~= math.floor(wanted) or wanted < 1 or wanted > count then bad(string.format("param is a number from 1 to %d", count)) end
      k = wanted
    elseif type(wanted) == "string" and wanted ~= "" then
      local lower = wanted:lower()
      for _, exact in ipairs({ true, false }) do
        for n = 1, count do
          local name = device:parameter(n).name:lower()
          if (exact and name == lower) or (not exact and name:sub(1, #lower) == lower) then
            if k and k ~= n then bad(string.format("%q names more than one parameter of this device: give its number", wanted)) end
            k = k or n
          end
        end
        if k then break end
      end
      if not k then bad(string.format("this device has no parameter %q", wanted)) end
    else
      bad("param: the parameter's number (from 1) or its name, with a device ref")
    end
  end
  if k < 1 or k > #device.parameters then fail("gone", "that parameter isn't on the device any more") end
  local parameter = device:parameter(k)
  if not parameter.is_automatable then bad(string.format("%s's %s can't be automated", device.name, parameter.name)) end
  local device_label = device.display_name ~= "" and device.display_name or device.name
  return t, d, k, track, device, parameter, device_label .. " " .. parameter.name
end

local function read_envelope(object)
  local automation = object.pattern_track:find_automation(object.parameter)
  if not automation then return { exists = false } end
  local points = {}
  for i, point in ipairs(automation.points) do points[i] = { time = point.time, value = point.value, scaling = point.scaling } end
  table.sort(points, function(a, b) return a.time < b.time end)
  return { exists = true, playmode = PLAYMODE_NAMES[automation.playmode] or "linear", points = points }
end

local function quantized(time) return math.floor(time * 256 + 0.5) / 256 end

-- (a point's time as Renoise keeps it, from 1)
local function time_of(point, lines, tpl)
  local time
  if point.time ~= nil then
    time = point.time
  elseif point.line ~= nil then
    local tick = point.tick or 0
    if type(tick) ~= "number" or tick < 0 or tick >= tpl then bad(string.format("tick is from 0 to %d (the song's ticks per line less one)", tpl - 1)) end
    time = (type(point.line) == "number" and point.line or 0) + tick / tpl
  end
  if type(time) ~= "number" or time ~= time then bad("each point needs time (lines from 0, a fraction too) or line (and tick)") end
  time = quantized(time)
  if time < 0 or time >= lines then bad(string.format("a point's time is from 0 to just before %d (the pattern's %d lines)", lines, lines)) end
  return time + 1
end

local function envelope_wanted(object, input)
  local now = read_envelope(object)
  local parameter, lines = object.parameter, object.lines
  if input.delete == true then
    for _, key in ipairs({ "points", "remove", "clear", "playmode" }) do
      if input[key] ~= nil then bad("delete takes the whole envelope away: give nothing else with it") end
    end
    return { exists = false }
  end
  if input.points == nil and input.remove == nil and input.clear == nil and input.playmode == nil then
    bad("give points, remove, clear, playmode or delete")
  end
  local points = {}
  for _, point in ipairs(now.points or {}) do points[#points + 1] = { time = point.time, value = point.value, scaling = point.scaling } end
  local function drop(test)
    local kept = {}
    for _, point in ipairs(points) do
      if not test(point) then kept[#kept + 1] = point end
    end
    points = kept
  end
  if input.clear ~= nil then
    if input.clear == true then
      points = {}
    elseif type(input.clear) == "table" then
      local from, to = input.clear.from, input.clear.to
      if type(from) ~= "number" or type(to) ~= "number" or from < 0 or to < from or to >= lines then
        bad(string.format("clear is true (every point) or {from, to}: lines from 0 to %d, from up to to", lines - 1))
      end
      drop(function(point) return point.time >= from + 1 and point.time < to + 2 end)
    else
      bad("clear is true (every point) or {from, to}")
    end
  end
  if input.remove ~= nil then
    if type(input.remove) ~= "table" then bad("remove: a list of the times of points to remove") end
    for _, time in ipairs(input.remove) do
      if type(time) ~= "number" then bad("remove: a list of the times of points to remove") end
      local at, found = quantized(time) + 1, false
      drop(function(point)
        if point.time == at then found = true end
        return point.time == at
      end)
      if not found then bad(string.format("there is no point at %s to remove", tostring(time))) end
    end
  end
  if input.points ~= nil then
    if type(input.points) ~= "table" or #input.points == 0 then bad("points: a list of {time or line, value or paramValue, scaling?}") end
    if #input.points > MOST_POINTS then bad(string.format("at most %d points a change", MOST_POINTS)) end
    local tpl = renoise.song().transport.tpl
    local given = {}
    for _, point in ipairs(input.points) do
      if type(point) ~= "table" then bad("each of points is an object {time or line, value or paramValue, scaling?}") end
      local time = time_of(point, lines, tpl)
      if given[time] then bad("two points at " .. (time - 1) .. ": one point a time") end
      given[time] = true
      local value = point.value
      if point.paramValue ~= nil then
        local v, low, high = point.paramValue, parameter.value_min, parameter.value_max
        if type(v) ~= "number" or v < low or v > high then bad(string.format("paramValue is from %s to %s for %s", tostring(low), tostring(high), parameter.name)) end
        value = high > low and (v - low) / (high - low) or 0
      end
      if type(value) ~= "number" or value ~= value or value < 0 or value > 1 then bad("a point's value is from 0 to 1 (of the parameter's range), or give paramValue") end
      local scaling = point.scaling or 0
      if type(scaling) ~= "number" or scaling < -1 or scaling > 1 then bad("scaling is from -1 to 1 (0 is straight; the linear play mode uses it)") end
      drop(function(other) return other.time == time end)
      points[#points + 1] = { time = time, value = value, scaling = scaling }
    end
  end
  table.sort(points, function(a, b) return a.time < b.time end)
  local playmode = now.playmode or "linear"
  if input.playmode ~= nil then
    if not PLAYMODES[input.playmode] then bad("playmode is points, linear or curves") end
    playmode = input.playmode
  end
  return { exists = true, playmode = playmode, points = points }
end

local function write_envelope(object, state)
  local automation = object.pattern_track:find_automation(object.parameter)
  if not state.exists then
    if automation then object.pattern_track:delete_automation(object.parameter) end
    return
  end
  automation = automation or object.pattern_track:create_automation(object.parameter)
  automation.playmode = PLAYMODES[state.playmode] or 2
  local points = {}
  for i, point in ipairs(state.points) do points[i] = { time = point.time, value = point.value, scaling = point.scaling } end
  automation.points = points
end

local function same_envelope(a, b)
  if a.exists ~= b.exists then return false end
  if not a.exists then return true end
  if a.playmode ~= b.playmode or #a.points ~= #b.points then return false end
  for i, point in ipairs(a.points) do
    local other = b.points[i]
    if point.time ~= other.time or not base.close(point.value, other.value) or not base.close(point.scaling or 0, other.scaling or 0) then return false end
  end
  return true
end

-- (a restore finds the parameter by its places: the track under `track`, the device by its place on the track (its devices' log),
-- the parameter's number and name, the pattern by its place in the pool; a device made on another track, or a pattern made
-- elsewhere in the pool, moves none of them)
local function parameter_at(song, env, material)
  local t = material.track
  local track = base.track_at(song, t)
  local d, why = material_.device_now(song, env, t, material.device)
  if not d then
    if why == "gone" then fail("gone", "the device isn't on the track any more") end
    if why == "changed" then fail("changed", "the device at its place on the track is another one now") end
    fail("stale", "the devices on the track changed in a way Kumi couldn't follow since (the tool watched them anew): read again")
  end
  local device = track:device(d)
  local k = material.param
  if type(k) ~= "number" or k > #device.parameters or device:parameter(k).name ~= material.paramName then
    fail("changed", "the device's parameters aren't what they were")
  end
  local parameter = device:parameter(k)
  local device_label = device.display_name ~= "" and device.display_name or device.name
  return t, d, k, track, device, parameter, device_label .. " " .. parameter.name
end

local envelope_spec = {
  kind = "automation-write", noun = "automation", undoable = true,
  locate = function(song, env, input)
    local t, d, k, track, parameter, label, p, _
    if input.material then
      t, d, k, track, _, parameter, label = parameter_at(song, env, input.material)
      p = base.pattern_now(song, env, input.material.pattern)
    else
      t, d, k, track, _, parameter, label = M.parameter_of(song, env, input, "automation-write")
      p = M.pattern_of(song, env, input.pattern, "automation-write")
    end
    local pattern_track = song:pattern(p):track(t)
    if pattern_track.is_alias then
      bad(string.format("that track in pattern %d is an alias of pattern %d: automate it there", p, pattern_track.alias_pattern_index))
    end
    return {
      object = { pattern_track = pattern_track, parameter = parameter, lines = song:pattern(p).number_of_lines, p = p },
      name = string.format("%s on %s in %s", label, q(base.named(t, track)), q(M.pattern_label(song, p))),
      place = { pattern = p, track = t, device = d, param = k },
      target = { ref = refs.make(env.epoch(), "param", t, d, k), kind = "param" }, id = base.track_id(track) .. "|" .. d .. "|" .. k,
    }
  end,
  reread = function(song, place)
    if place.pattern > #song.patterns or place.track > rows.track_count(song) then return nil end
    local track = song:track(place.track)
    if place.device > #track.devices or place.param > #track:device(place.device).parameters then return nil end
    return { pattern_track = song:pattern(place.pattern):track(place.track), parameter = track:device(place.device):parameter(place.param), lines = song:pattern(place.pattern).number_of_lines }
  end,
  read = function(object) return read_envelope(object) end,
  wanted = envelope_wanted,
  write = write_envelope,
  same = same_envelope,
  title = function(name, _, to)
    if not to.exists then return "Automation of " .. name .. " deleted" end
    return string.format("Automation of %s: %s, %s", name, #to.points == 1 and "1 point" or (#to.points .. " points"), to.playmode)
  end,
  watch = function(env, loc)
    if not env.watch_pattern_track then return end
    local place = loc.place
    env.watch_pattern_track(place.pattern, place.track, "automation_observable")
    local automation = loc.object.pattern_track:find_automation(loc.object.parameter)
    if automation then
      local key = "automation:" .. place.device .. ":" .. place.param
      env.watch_pattern_track(place.pattern, place.track, key .. ":points", automation.points_observable)
      env.watch_pattern_track(place.pattern, place.track, key .. ":playmode", automation.playmode_observable)
    end
  end,
  material = function(env, loc)
    local place = loc.place
    local device = renoise.song():track(place.track):device(place.device)
    return {
      track = place.track, device = material_.device_place(env, place.track, place.device, device), param = place.param,
      paramName = loc.object.parameter.name, pattern = base.pattern_place(env, place.pattern),
    }
  end,
  restore_input = function(restore) return { material = restore.material } end,
  -- (HISTORY's picture: the parameter's name, and the pattern's lines its points run over)
  shown = function(loc) return { parameter = loc.object.parameter.name, lines = loc.object.lines } end,
}
base.register(envelope_spec)

return M
