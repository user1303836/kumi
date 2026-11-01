-- Kumi's changes to a track: its volume, pan, mute, solo, name and colour; and deleting it, with what undo needs to make it
-- again (see changes_base.lua for how a change goes).

local import = ...
local json = import("json")
local refs = import("refs")
local rows = import("rows")
local base = import("changes_base")
local material_ = import("changes_material")

local fail, bad, the_song = base.fail, base.bad, base.the_song
local close, db_of, hex_of, rgb_of = base.close, base.db_of, base.hex_of, base.rgb_of
local find, is_master, named, target, stage_of = base.find, base.is_master, base.named, base.target, base.stage_of
local same_table = base.same_table
local NOT_UNDOABLE = base.NOT_UNDOABLE

local M = {}

-- The track properties

-- A track property is found by the track's ref, and found again by its place.
local function track_reread(song, place)
  if place.track >= 1 and place.track <= rows.track_count(song) then return song:track(place.track) end
  return nil
end

-- (a title names the pre-FX stage; the post-FX fader is the one the producer knows as the track's volume and pan)
local function stage_text(state) return state.stage == "pre" and " pre-FX" or "" end

local function prop(spec)
  spec.locate = function(song, env, input)
    local index, track = find(song, env, input.ref, spec.kind)
    if is_master(track) and not spec.master then bad("the master track has no " .. spec.noun) end
    return { object = track, name = named(index, track), place = { track = index }, target = target(env, index), id = base.track_id(track) }
  end
  spec.reread = track_reread
  if spec.watch then
    spec.watch = function(env, loc) env.watch_params(loc.place.track) end
  end
  base.register(spec)
end

prop {
  kind = "track-volume", noun = "volume", master = true, undoable = true, watch = true,
  read = function(track, source)
    local stage = stage_of(source)
    local value = (stage == "post" and track.postfx_volume or track.prefx_volume).value
    return { stage = stage, volume = value, volumeDb = db_of(value) }
  end,
  wanted = function(track, input)
    local stage = stage_of(input)
    local db = input.volumeDb
    if type(db) ~= "number" or db ~= db then bad("volumeDb must be a number") end
    local parameter = stage == "post" and track.postfx_volume or track.prefx_volume
    local top = db_of(parameter.value_max) or 3
    if db < -150 or db > top + 1e-6 then bad(string.format("volumeDb must be from -150 (silent) to %.1f", top)) end
    local value = db <= -150 and 0 or math.min(10 ^ (db / 20), parameter.value_max)
    return { stage = stage, volume = value, volumeDb = db_of(value) }
  end,
  write = function(track, state)
    (state.stage == "post" and track.postfx_volume or track.prefx_volume).value = state.volume
  end,
  same = function(a, b) return a.stage == b.stage and close(a.volume, b.volume) end,
  title = function(name, _, to) return name .. stage_text(to) .. " volume " .. base.db_text(to.volumeDb) .. " dB" end,
}

prop {
  kind = "track-pan", noun = "pan", master = true, undoable = true, watch = true,
  read = function(track, source)
    local stage = stage_of(source)
    local value = (stage == "post" and track.postfx_panning or track.prefx_panning).value
    return { stage = stage, pan = value * 2 - 1 }
  end,
  wanted = function(_, input)
    local stage = stage_of(input)
    local pan = input.pan
    if type(pan) ~= "number" or pan ~= pan or pan < -1 or pan > 1 then bad("pan must be a number from -1 (left) to 1 (right)") end
    return { stage = stage, pan = pan }
  end,
  write = function(track, state)
    (state.stage == "post" and track.postfx_panning or track.prefx_panning).value = (state.pan + 1) / 2
  end,
  same = function(a, b) return a.stage == b.stage and close(a.pan, b.pan) end,
  title = function(name, _, to) return name .. stage_text(to) .. " pan " .. base.pan_text(to.pan) end,
}

prop {
  kind = "track-mute", noun = "mute", master = false, undoable = false,
  read = function(track)
    local state = track.mute_state
    return { mute = state ~= renoise.Track.MUTE_STATE_ACTIVE, muteState = state }
  end,
  wanted = function(_, input)
    if type(input.mute) ~= "boolean" then bad("mute must be true or false") end
    local T = renoise.Track
    local state = input.mute and T.MUTE_STATE_MUTED or T.MUTE_STATE_ACTIVE
    return { mute = input.mute, muteState = state }
  end,
  write = function(track, state) track.mute_state = state.muteState end,
  same = function(a, b) return a.muteState == b.muteState end,
  title = function(name, _, to) return name .. (to.mute and " muted" or " unmuted") end,
}

-- track-solo. Renoise solos by rewriting mute states (measured in 3.5.4, n-song lab 6 and 7): a solo turns every other
-- non-master track Off but what the soloed track plays through (its groups, a group's own tracks, the send tracks it feeds;
-- for a send, the tracks feeding it), which reads soloed with it; a script's solo is exclusive; an un-solo turns every track
-- Active, so the mutes from before the solo are gone. The solo setter toggles the track's own solo when the value differs
-- from what `solo_state` reads: `false` on a track soloed only along with another solos that track on its own, and `true` on
-- one that reads soloed changes nothing. So the record keeps every track's mute and solo (`prior.tracks`, by place and name;
-- the bridge carries the places to a restore), the fence is a fingerprint of them all, and the restore writes them back where
-- each track still holds what the apply left (`changed` otherwise, unless discard). A script can solo one track at a time:
-- `with`, the other tracks to hear alone with it, are turned Active with what each plays through (Renoise's own solo of each
-- says what that is). Neither mute nor solo is an undo step.

local SOLO_ROUNDS = rows.SOLO_ROUNDS -- (un-solo rounds: a solo takes two at most, the first making a track soloed along with another the soloed one)

local function solo_snapshot(song)
  local out = {}
  for t = 1, rows.track_count(song) do
    local track = song:track(t)
    if not is_master(track) then
      out[#out + 1] = { t = t, id = base.track_id(track), mute = track.mute_state, solo = track.solo_state == true }
    end
  end
  return out
end

-- What the fence keeps of a snapshot: a fingerprint of every track's place, name, mute and solo.
local function solo_print(list)
  local parts = {}
  for _, e in ipairs(list) do
    local text = string.format("%d:%s:%d:%s|", e.t, e.id, e.mute, e.solo and "1" or "0")
    parts[#parts + 1] = text
  end
  return base.hash(table.concat(parts))
end

-- The tracks that read soloed now, as a set and as a key ("2,6").
local function soloed_now(song)
  local set, key = {}, {}
  for t = 1, rows.track_count(song) do
    local track = song:track(t)
    if not is_master(track) and track.solo_state then
      set[t] = true
      key[#key + 1] = t
    end
  end
  return set, table.concat(key, ",")
end

local unsolo_all = rows.unsolo_all

-- Whether track `t` has the mute and solo of `state` ({mute, solo}).
local function solo_holds(song, t, state)
  local now = song:track(t)
  return state ~= nil and now.mute_state == state.mute and (now.solo_state == true) == (state.solo == true)
end

-- Tracks by name for a sentence: “Hat”, “Hat” and “Bass”, “Hat”, “Bass”, “Pad” and 2 more.
local function solo_names(song, list)
  local names = {}
  for i, t in ipairs(list) do
    if i > 3 then
      names[#names + 1] = (#list - 3) .. " more"
      break
    end
    names[#names + 1] = base.q(named(t, song:track(t)))
  end
  if #names <= 1 then return names[1] or "" end
  return table.concat(names, ", ", 1, #names - 1) .. " and " .. names[#names]
end

local function solo_input(song, env, input)
  local index, track = find(song, env, input.ref, "track-solo")
  if is_master(track) then bad("the master track has no solo") end
  if type(input.solo) ~= "boolean" then bad("solo must be true or false") end
  local with = {}
  if input.with ~= nil and input.with ~= json.null then
    if not input.solo then bad("with is for a solo: the other tracks to hear alone with it") end
    if type(input.with) ~= "table" then bad("with: a list of track refs") end
    -- (each one is a solo of its own while the change is made, every track's mute written each time)
    if #input.with > 64 then bad("with: 64 tracks at most") end
    local seen = { [index] = true }
    for _, reference in ipairs(input.with) do
      local w, other = find(song, env, reference, "track-solo")
      if is_master(other) then bad("with: the master track plays anyway") end
      if not seen[w] then
        seen[w] = true
        with[#with + 1] = w
      end
    end
    table.sort(with)
  end
  return { index = index, track = track, name = named(index, track), solo = input.solo, with = with }
end

local function solo_title(song, plan)
  if not plan.solo then return plan.name .. " unsoloed" end
  if #plan.with == 0 then return plan.name .. " soloed" end
  return plan.name .. " soloed, with " .. solo_names(song, plan.with)
end

local ACTIVE = function() return renoise.Track.MUTE_STATE_ACTIVE end
local OFF = function() return renoise.Track.MUTE_STATE_OFF end
local MUTED = function() return renoise.Track.MUTE_STATE_MUTED end

local function solo_preview(song, env, input)
  local plan = solo_input(song, env, input)
  local tracks = solo_snapshot(song)
  local soloed = plan.track.solo_state == true
  local display = { title = solo_title(song, plan), before = { solo = soloed }, after = { solo = plan.solo } }
  local notes = {}
  if plan.solo then
    local others = {}
    for _, e in ipairs(tracks) do
      if e.solo and e.t ~= plan.index then others[#others + 1] = e.t end
    end
    notes[#notes + 1] = "Renoise solos by turning every other track Off but what it plays through (its group, the send tracks it feeds); "
      .. "the end of a solo turns every track back on, muted ones too. Kumi's undo puts each track back as it was."
    -- (also when the track reads soloed only along with another's solo, a group's or a multi-solo made in Renoise: its own solo
    -- ends theirs)
    if #others > 0 then
      notes[#notes + 1] = string.format("Renoise's solo is exclusive: %s stops being soloed.", solo_names(song, others))
    end
    if #plan.with > 0 then
      notes[#notes + 1] = string.format("Renoise lets a script solo one track: %s %s turned on instead, with what %s through.",
        solo_names(song, plan.with), #plan.with == 1 and "is" or "are", #plan.with == 1 and "it plays" or "they play")
    end
  elseif soloed then
    local quiet = {}
    for _, e in ipairs(tracks) do
      if e.mute ~= ACTIVE() then quiet[#quiet + 1] = e.t end
    end
    notes[#notes + 1] = "Renoise ends the whole solo and turns every track back on, muted ones too"
      .. (#quiet > 0 and string.format(" (%s now)", solo_names(song, quiet) .. (#quiet == 1 and " is Off or muted" or " are Off or muted")) or "")
      .. ". Kumi's undo puts each track back as it was."
  else
    display.unchanged = true
  end
  notes[#notes + 1] = NOT_UNDOABLE
  display.note = table.concat(notes, " ")
  return {
    target = target(env, plan.index), prior = { solo = soloed, tracks = tracks },
    fence = { id = base.track_id(plan.track), label = plan.name, tracks = solo_print(tracks) },
    display = display,
  }
end

-- What Renoise did to the other tracks, from the snapshots before and after (the same tracks, in order).
local function solo_effects(song, plan, before, after)
  local off, on, unsoloed, muted = {}, {}, {}, {}
  for k, b in ipairs(before) do
    local a = after[k]
    if a and b.t ~= plan.index then
      if b.mute ~= a.mute and a.mute == OFF() then off[#off + 1] = b.t end
      if b.mute ~= a.mute and a.mute == ACTIVE() then
        on[#on + 1] = b.t
        if b.mute == MUTED() then muted[#muted + 1] = b.t end
      end
      if b.solo and not a.solo then unsoloed[#unsoloed + 1] = b.t end
    end
  end
  local notes = {}
  if #off > 0 then notes[#notes + 1] = string.format("Renoise turned %s Off.", #off > 3 and (#off .. " other tracks") or solo_names(song, off)) end
  if plan.solo and #unsoloed > 0 then
    notes[#notes + 1] = string.format("%s %s soloed any more (a solo is exclusive).", solo_names(song, unsoloed), #unsoloed == 1 and "isn't" or "aren't")
  end
  if #on > 0 then
    local muted_text = #muted > 0 and string.format(" (%s muted before)", #muted == 1 and solo_names(song, muted) or (#muted .. " of them")) or ""
    notes[#notes + 1] = string.format("Renoise turned %s back on%s.", #on > 3 and (#on .. " tracks") or solo_names(song, on), muted_text)
  end
  return notes
end

-- Writes `wanted` (a snapshot by place now: t -> {mute, solo}) back over the tracks `places`: every solo taken away, the solo
-- that makes its soloed tracks (the track whose own solo gives exactly them, tried from the last back, each from no solo), then
-- every mute state. The other tracks (made since, or not the ones they were) keep a mute the producer gave them. Returns the
-- tracks a script can't solo again along with the others (soloed in the snapshot by a solo of several, as Renoise's GUI can).
local function solo_write_back(song, wanted, places, x_index, x_solo)
  local count = rows.track_count(song)
  local ours, kept = {}, {}
  for _, t in ipairs(places) do ours[t] = true end
  for t = 1, count do
    local track = song:track(t)
    if not ours[t] and t ~= x_index and not is_master(track) and track.mute_state == MUTED() then kept[#kept + 1] = t end
  end
  if not unsolo_all(song) then fail("failed", "Renoise kept a solo on") end
  local soloed, set = {}, {}
  for _, t in ipairs(places) do
    if wanted[t].solo then
      soloed[#soloed + 1] = t
      set[t] = true
    end
  end
  if x_solo and not set[x_index] then
    soloed[#soloed + 1] = x_index
    set[x_index] = true
    table.sort(soloed)
  end
  local key = table.concat(soloed, ",")
  local found, best, best_score = false, nil, nil
  for k = #soloed, 1, -1 do
    local candidate = soloed[k]
    if not unsolo_all(song) then fail("failed", "Renoise kept a solo on") end
    song:track(candidate).solo_state = true
    local now, now_key = soloed_now(song)
    if now_key == key then
      found = true
      break
    end
    local score = 0
    for t in pairs(now) do score = score + (set[t] and 1 or -2) end
    if best_score == nil or score > best_score then best, best_score = candidate, score end
  end
  if not found and best then
    unsolo_all(song)
    song:track(best).solo_state = true
  end
  for _, t in ipairs(places) do
    local track = song:track(t)
    if track.mute_state ~= wanted[t].mute then track.mute_state = wanted[t].mute end
  end
  for _, t in ipairs(kept) do
    local track = song:track(t)
    if track.mute_state ~= MUTED() then track.mute_state = MUTED() end
  end
  local unsoloed = {}
  for _, t in ipairs(soloed) do
    if not song:track(t).solo_state then unsoloed[#unsoloed + 1] = t end
  end
  return unsoloed
end

local function soloed_key(list)
  local key = {}
  for _, e in ipairs(list) do
    if e.solo then key[#key + 1] = e.t end
  end
  return table.concat(key, ",")
end

local function solo_apply(song, env, input, fence, desc)
  local ok, plan = pcall(solo_input, song, env, input)
  if not ok then base.gone_is_changed(plan) end
  if fence.id ~= nil and fence.id ~= base.track_id(plan.track) then fail("changed", base.moved_text(fence.label or plan.name)) end
  local before = solo_snapshot(song)
  if solo_print(before) ~= fence.tracks then
    fail("changed", "a track's mute or solo changed since the preview, or a track was made, deleted or renamed: read again")
  end
  local soloed = plan.track.solo_state == true
  local same = false
  if plan.solo and (soloed or #plan.with > 0) then
    -- (each other track soloed first, from no solo: what Renoise solos with it is what it plays through)
    local needs = {}
    for _, w in ipairs(plan.with) do
      if not unsolo_all(song) then fail("failed", "Renoise kept a solo on") end
      song:track(w).solo_state = true
      for t in pairs((soloed_now(song))) do needs[t] = true end
    end
    -- (a track that reads soloed may be soloed only along with another: its own solo is made from no solo)
    if not unsolo_all(song) then fail("failed", "Renoise kept a solo on") end
    plan.track.solo_state = true
    for t in pairs(needs) do
      local track = song:track(t)
      if not track.solo_state and track.mute_state ~= ACTIVE() then track.mute_state = ACTIVE() end
    end
    -- (it was soloed on its own already: the producer's mutes since that solo go back as they were)
    if #plan.with == 0 and select(2, soloed_now(song)) == soloed_key(before) then
      for _, e in ipairs(before) do
        local track = song:track(e.t)
        if track.mute_state ~= e.mute then track.mute_state = e.mute end
      end
      same = true
    end
  elseif plan.solo then
    plan.track.solo_state = true
  elseif soloed then
    -- (a track soloed along with another becomes the soloed one at the first un-solo, and the second ends the solo)
    for _ = 1, SOLO_ROUNDS do
      if not plan.track.solo_state then break end
      plan.track.solo_state = false
    end
  end
  local after = solo_snapshot(song)
  if (plan.track.solo_state == true) ~= plan.solo then
    -- (Renoise took something else: every track goes back as it was, so nothing half-done stays)
    local wanted, places = {}, {}
    for _, e in ipairs(before) do
      wanted[e.t] = e
      places[#places + 1] = e.t
    end
    pcall(solo_write_back, song, wanted, places, plan.index, soloed)
    fail("failed", "Renoise kept " .. plan.name .. "'s solo as it was")
  end
  local display = { title = solo_title(song, plan), before = { solo = soloed }, after = { solo = plan.solo } }
  if same then
    -- (nothing the song keeps changed: no row in HISTORY, nothing to undo)
    display.unchanged, display.note = true, "It was soloed on its own already: nothing changed."
  else
    local notes = solo_effects(song, plan, before, after)
    if #plan.with > 0 then
      notes[#notes + 1] = string.format("A script solos one track at a time: %s %s on instead.", solo_names(song, plan.with), #plan.with == 1 and "is" or "are")
    end
    if #notes > 0 then notes[#notes + 1] = "Kumi's undo puts each track back as it was." end
    notes[#notes + 1] = NOT_UNDOABLE
    display.note = table.concat(notes, " ")
  end
  local applied = {}
  for k, e in ipairs(after) do applied[k] = { t = e.t, mute = e.mute, solo = e.solo } end
  return {
    applied = { solo = plan.track.solo_state == true, tracks = applied }, display = display,
    undo = { desc = desc, entry = false },
    target = target(env, plan.index),
  }
end

local function solo_restore(song, env, restore, desc)
  local reference = restore.target and restore.target.ref
  local ok, index, track = pcall(find, song, env, reference, "track-solo")
  if not ok then
    if type(index) == "table" and index.code == "failed" then index.code = "gone" end
    error(index, 0)
  end
  local prior, applied = restore.prior, restore.applied
  if type(prior) ~= "table" or type(applied) ~= "table" then bad("prior and applied: the solo's tracks") end
  -- (the snapshots' places are carried to now by the bridge, which leaves out a track deleted since: `prior.lost`)
  local wanted, left = {}, {}
  for _, e in ipairs(type(prior.tracks) == "table" and prior.tracks or {}) do
    if type(e) == "table" and type(e.t) == "number" then wanted[e.t] = e end
  end
  for _, e in ipairs(type(applied.tracks) == "table" and applied.tracks or {}) do
    if type(e) == "table" and type(e.t) == "number" then left[e.t] = e end
  end
  local count = rows.track_count(song)
  local places, strangers = {}, {}
  for t, e in pairs(wanted) do
    if t >= 1 and t <= count and not is_master(song:track(t)) and base.track_id(song:track(t)) == e.id then
      places[#places + 1] = t
    else
      strangers[#strangers + 1] = t
    end
  end
  table.sort(places)
  table.sort(strangers)
  local function holds(t, state) return solo_holds(song, t, state) end
  local name = named(index, track)
  local x_prior = prior.solo == true
  local back = (track.solo_state == true) == x_prior
  for _, t in ipairs(places) do back = back and holds(t, wanted[t]) end
  -- (a track that isn't the one it was is never written, so it doesn't keep a restore that went through from being back)
  if back then
    return {
      restored = { solo = x_prior }, display = { title = name .. (x_prior and " soloed" or " unsoloed") .. " (put back)" },
      undo = { desc = desc, entry = false }, unwritten = true,
    }
  end
  local moved = {}
  for _, t in ipairs(places) do
    if not holds(t, left[t]) then moved[#moved + 1] = t end
  end
  if (track.solo_state == true) ~= (applied.solo == true) and not wanted[index] then moved[#moved + 1] = index end
  if not restore.discard then
    if #moved > 0 then
      fail("changed", string.format("%s mute or solo changed in Renoise since Kumi's solo", #moved > 3 and (#moved .. " tracks'") or (solo_names(song, moved) .. "'s")))
    end
    if #strangers > 0 then
      fail("changed", string.format("%s at %s place%s isn't the track it was (renamed, or another track): discard puts back the rest",
        #strangers == 1 and "the track" or (#strangers .. " tracks"), #strangers == 1 and "its" or "their", #strangers == 1 and "" or "s"))
    end
  end
  local before = {}
  for _, t in ipairs(places) do before[t] = song:track(t).mute_state end
  local unsoloed = solo_write_back(song, wanted, places, index, x_prior)
  local notes, again, missed = {}, {}, {}
  for _, t in ipairs(places) do
    if wanted[t].mute == MUTED() and before[t] ~= MUTED() then again[#again + 1] = t end
    if not holds(t, wanted[t]) then missed[#missed + 1] = t end
  end
  if #missed == 0 then
    notes[#notes + 1] = "Every track's mute and solo are as they were" .. (#again > 0 and string.format(" (%s muted again).", solo_names(song, again)) or ".")
  end
  if #unsoloed > 0 then
    notes[#notes + 1] = string.format("Renoise lets a script solo one track at a time: %s %s but %s shown soloed.", solo_names(song, unsoloed),
      #unsoloed == 1 and "plays" or "play", #unsoloed == 1 and "isn't" or "aren't")
  end
  local other_missed = {}
  for _, t in ipairs(missed) do
    local in_unsoloed = false
    for _, u in ipairs(unsoloed) do in_unsoloed = in_unsoloed or u == t end
    if not in_unsoloed then other_missed[#other_missed + 1] = t end
  end
  if #other_missed > 0 then notes[#notes + 1] = string.format("Renoise kept %s as the solo left %s.", solo_names(song, other_missed), #other_missed == 1 and "it" or "them") end
  local lost = type(prior.lost) == "number" and prior.lost or 0
  if lost > 0 then notes[#notes + 1] = string.format("%s deleted since %s.", lost == 1 and "A track" or (lost .. " tracks"), lost == 1 and "isn't in it" or "aren't in it") end
  if #strangers > 0 then
    notes[#notes + 1] = string.format("%s that %s the track%s %s left as Renoise left %s.", #strangers == 1 and "A track" or (#strangers .. " tracks"),
      #strangers == 1 and "isn't" or "aren't", #strangers == 1 and " it was" or "s they were", #strangers == 1 and "is" or "are", #strangers == 1 and "it" or "them")
  end
  return {
    restored = { solo = track.solo_state == true },
    display = { title = name .. (x_prior and " soloed" or " unsoloed") .. " (put back)", note = #notes > 0 and table.concat(notes, " ") or nil },
    undo = { desc = desc, entry = false },
  }
end

base.register({ kind = "track-solo", custom = true, preview = solo_preview, apply = solo_apply, restore = solo_restore })

prop {
  kind = "track-rename", noun = "name", master = false, undoable = true,
  read = function(track) return { name = track.name } end,
  wanted = function(_, input)
    local name = input.name
    if type(name) ~= "string" then bad("name must be text") end
    if #name > 1024 or name:find("[%c]") then bad("name must be one line of text") end
    return { name = name }
  end,
  write = function(track, state) track.name = state.name end,
  same = function(a, b) return a.name == b.name end,
  title = function(_, from, to)
    if from.name == "" then return "Named track " .. base.q(to.name) end
    return "Renamed track " .. base.q(from.name) .. " \226\134\146 " .. base.q(to.name)
  end,
}

prop {
  kind = "track-color", noun = "colour", master = true, undoable = true,
  read = function(track) return { color = hex_of(track.color), colorBlend = track.color_blend } end,
  wanted = function(track, input)
    local color = rgb_of(input.color)
    if not color then bad("color must be #rrggbb") end
    local blend = input.colorBlend
    if blend == nil then
      blend = track.color_blend
    elseif type(blend) ~= "number" or blend ~= math.floor(blend) or blend < 0 or blend > 100 then
      bad("colorBlend must be a whole number from 0 to 100")
    end
    return { color = hex_of(color), colorBlend = blend }
  end,
  write = function(track, state)
    track.color = rgb_of(state.color)
    track.color_blend = state.colorBlend
  end,
  same = function(a, b) return a.color == b.color and a.colorBlend == b.colorBlend end,
  title = function(name) return name .. " colour changed" end,
}


-- Deleting a track, and making it again

local function lines_text(count) return count == 1 and "1 line" or count .. " lines" end

-- What tells a track apart from what the preview saw: its place, name and what it holds.
local function signature(song, index, track)
  local notes = 0
  for p = 1, #song.patterns do
    local pattern_track = song:pattern(p):track(index)
    if not pattern_track.is_empty then notes = notes + 1 end
  end
  return {
    name = track.name,
    index = index,
    kind = rows.track_type(track),
    devices = math.max(#track.devices - 1, 0),
    patternsWithLines = notes,
    noteColumns = track.visible_note_columns,
    effectColumns = track.visible_effect_columns,
  }
end

local function find_to_delete(song, env, input)
  local index, track = find(song, env, input.ref, "track-delete")
  local kind = rows.track_type(track)
  if kind == "master" then bad("the master track can't be deleted") end
  if kind == "group" then bad("a group track is deleted with its members; delete the group in Renoise") end
  return index, track, kind
end

local function delete_preview(song, env, input)
  local index, track = find_to_delete(song, env, input)
  local now = signature(song, index, track)
  local display = { title = "Deleted track " .. base.q(named(index, track)) }
  if now.devices > 0 or now.patternsWithLines > 0 then
    display.note = string.format("It holds %d devices and lines in %d patterns; Kumi keeps them to put back.", now.devices, now.patternsWithLines)
  end
  if track.solo_state then
    display.note = (display.note and (display.note .. " ") or "")
      .. "It's soloed: deleting it ends the solo, and Renoise turns every track back on, muted ones too; Kumi's undo brings the solo and every mute back."
  end
  return { target = target(env, index), prior = now, fence = now, display = display }
end

-- M4 (n-song): deleting a track that reads soloed ends its solo, and Renoise turns every track Active (the soloed track), or
-- leaves odd states (one soloed along with another: lab 8, a send of T1's solo deleted leaves every track Off and nothing soloed).
-- The record keeps every other track's mute and solo before the delete (`soloTracks`) and right after it (`soloLeft`), by
-- their places after the delete, so the restore can put the solo back where nothing changed since.
local function solo_around_delete(before, index, song)
  local kept, left = {}, {}
  for _, e in ipairs(before) do
    if e.t ~= index then kept[#kept + 1] = { t = e.t > index and e.t - 1 or e.t, id = e.id, mute = e.mute, solo = e.solo } end
  end
  for k, e in ipairs(solo_snapshot(song)) do left[k] = { t = e.t, mute = e.mute, solo = e.solo } end
  return kept, left
end

-- What the delete did to the other tracks, from those two snapshots (the same tracks, in order).
local function delete_solo_note(song, kept, left)
  local ended, off, on, muted = {}, {}, {}, {}
  for k, b in ipairs(kept) do
    local a = left[k]
    if a then
      if b.solo and not a.solo then ended[#ended + 1] = b.t end
      if b.mute ~= a.mute and a.mute == OFF() then off[#off + 1] = b.t end
      if b.mute ~= a.mute and a.mute == ACTIVE() then
        on[#on + 1] = b.t
        if b.mute == MUTED() then muted[#muted + 1] = b.t end
      end
    end
  end
  local notes = { "Its solo ended with it." }
  if #ended > 0 then notes[#notes + 1] = string.format("%s %s soloed any more.", solo_names(song, ended), #ended == 1 and "isn't" or "aren't") end
  if #off > 0 then notes[#notes + 1] = string.format("Renoise turned %s Off.", #off > 3 and (#off .. " tracks") or solo_names(song, off)) end
  if #on > 0 then
    local muted_text = #muted > 0 and string.format(" (%s muted before)", #muted == 1 and solo_names(song, muted) or (#muted .. " of them")) or ""
    notes[#notes + 1] = string.format("Renoise turned %s back on%s.", #on > 3 and (#on .. " tracks") or solo_names(song, on), muted_text)
  end
  notes[#notes + 1] = "Kumi's undo brings the solo back with every track's mute as it was."
  return table.concat(notes, " ")
end

-- The restore's half of it, once the track is made again at `index` with all its parts: every track's mute and solo from the
-- record, where each track still holds what the delete left (`changed` otherwise: nothing is touched, unless discard). A track
-- isn't found by its place alone: its name must be the one it had. Answers what to say.
local function solo_after_restore(song, env, index, material, since, discard)
  if not material.solo then return nil end
  if type(material.soloTracks) ~= "table" or type(material.soloLeft) ~= "table" then
    return "It was soloed: Kumi left every track's solo and mute as they are (the record has nothing of the other tracks)."
  end
  local count = rows.track_count(song)
  local then_left = {}
  for _, e in ipairs(material.soloLeft) do
    if type(e) == "table" and type(e.t) == "number" then then_left[e.t] = e end
  end
  local wanted, left, places, strangers, lost, unknown = {}, {}, {}, {}, 0, 0
  for _, e in ipairs(material.soloTracks) do
    -- (where the track is now: through the tracks made, deleted and swapped since, and this one made again, which the layout
    -- counts; without a layout, only this one moves them)
    local t, why
    if type(e) ~= "table" or type(e.t) ~= "number" then
      why = "unknown"
    elseif type(since) ~= "number" then
      t = e.t >= index and e.t + 1 or e.t
    else
      t, why = env.shifted(e.t, since)
    end
    if not t then
      if why == "gone" then lost = lost + 1 else unknown = unknown + 1 end
    elseif t >= 1 and t <= count and t ~= index and not is_master(song:track(t)) and base.track_id(song:track(t)) == e.id then
      wanted[t], left[t] = e, then_left[e.t]
      places[#places + 1] = t
    else
      strangers[#strangers + 1] = t
    end
  end
  table.sort(places)
  if unknown > 0 then
    return "It was soloed, but Kumi can't tell where the other tracks went since: it left every track's solo and mute as they are."
  end
  local changed = {}
  for _, t in ipairs(places) do
    if not solo_holds(song, t, left[t]) then changed[#changed + 1] = t end
  end
  if not discard and (#changed > 0 or #strangers > 0) then
    local why = #changed > 0 and string.format("%s mute or solo changed since", #changed > 3 and (#changed .. " tracks'") or (solo_names(song, changed) .. "'s"))
      or "a track at its place isn't the one it was"
    return string.format("It was soloed when it was deleted, but %s: Kumi left every track's solo and mute as they are (discard puts them back).", why)
  end
  local before = {}
  for _, t in ipairs(places) do before[t] = song:track(t).mute_state end
  wanted[index] = { t = index, id = base.track_id(song:track(index)), mute = material.muteState, solo = true }
  places[#places + 1] = index
  table.sort(places)
  local unsoloed = solo_write_back(song, wanted, places, index, true)
  local again, missed = {}, {}
  for _, t in ipairs(places) do
    if t ~= index and wanted[t].mute == MUTED() and before[t] ~= MUTED() then again[#again + 1] = t end
    if not solo_holds(song, t, wanted[t]) then missed[#missed + 1] = t end
  end
  local notes = {}
  if #missed == 0 then
    notes[#notes + 1] = "Its solo is back, and every track's mute is as it was" .. (#again > 0 and string.format(" (%s muted again).", solo_names(song, again)) or ".")
  end
  if #unsoloed > 0 then
    notes[#notes + 1] = string.format("Renoise lets a script solo one track at a time: %s %s but %s shown soloed.", solo_names(song, unsoloed),
      #unsoloed == 1 and "plays" or "play", #unsoloed == 1 and "isn't" or "aren't")
  end
  if #missed > #unsoloed then notes[#notes + 1] = "Renoise kept some tracks' mute or solo as it had them." end
  if lost > 0 then notes[#notes + 1] = string.format("%s deleted since %s.", lost == 1 and "A track" or (lost .. " tracks"), lost == 1 and "isn't in it" or "aren't in it") end
  if #strangers > 0 then notes[#notes + 1] = "A track that isn't the one it was is left as Renoise left it." end
  return table.concat(notes, " ")
end

-- The writes that make a track again, in phases. A phase is one callback; the first makes the track, its properties, devices,
-- columns and slot mutes; the rest write lines (and the automation of those patterns), up to SLICE_WRITES each.
local function plan_phases(material)
  local phases, current, writes = {}, {}, 0
  for _, entry in ipairs(material.patterns or {}) do
    local cost = entry.writes or 1
    if writes > 0 and writes + cost > base.SLICE_WRITES then
      phases[#phases + 1] = current
      current, writes = {}, 0
    end
    current[#current + 1] = entry
    writes = writes + cost
  end
  phases[#phases + 1] = current
  return phases
end

local function gather(song, env, index, track, kind)
  local sequencer = song.sequencer
  local material = {
    index = index, kind = kind, name = track.name, color = hex_of(track.color), colorBlend = track.color_blend,
    muteState = track.mute_state, solo = track.solo_state == true, collapsed = track.collapsed,
    volume = { pre = track.prefx_volume.value, post = track.postfx_volume.value },
    panning = { pre = track.prefx_panning.value, post = track.postfx_panning.value },
    width = track.prefx_width.value,
    outputRouting = track.output_routing, outputDelay = track.output_delay,
    columns = {
      note = track.visible_note_columns, effect = track.visible_effect_columns,
      volume = track.volume_column_visible, panning = track.panning_column_visible, delay = track.delay_column_visible,
      sampleEffects = track.sample_effects_column_visible, names = {}, muted = {},
    },
    devices = {}, automation = {}, patterns = {}, slotMutes = {},
  }
  for c = 1, track.max_note_columns do
    local name = track:column_name(c)
    if name ~= "" then material.columns.names[tostring(c)] = name end
    if track:column_is_muted(c) then material.columns.muted[#material.columns.muted + 1] = c end
  end
  local parents = rows.group_parents(song)
  if parents[index] then
    -- (the group comes after its tracks, so it is one place up once this track is gone: `at` is where the shifts since find it)
    material.group = { at = parents[index] - 1, name = song:track(parents[index]).name }
  end
  for d = 2, #track.devices do material.devices[#material.devices + 1] = material_.read_device(track:device(d), song, index, true, env) end
  material.automation = material_.read_automation(song, index, track)
  local writes, limit = 0, base.MAX_RESTORE_STEPS * base.SLICE_WRITES
  for p = 1, #song.patterns do
    local pattern_track = song:pattern(p):track(index)
    if pattern_track.is_alias then
      material.patterns[#material.patterns + 1] = { pattern = p, alias = pattern_track.alias_pattern_index }
    elseif not pattern_track.is_empty then
      local lines, cost = material_.read_lines(pattern_track, song:pattern(p).number_of_lines)
      if #lines > 0 then material.patterns[#material.patterns + 1] = { pattern = p, lines = lines, writes = cost } end
      writes = writes + cost
      -- (past what can be put back in time there is no point in reading on)
      if writes > limit then
        fail("failed", string.format("%s holds more than Kumi can put back in time (over %d writes), so it won't delete it", named(index, track), limit))
      end
    end
  end
  -- A pattern with automation of this track and no lines still has to be written: it gets an entry of its own.
  local with_entry = {}
  for _, entry in ipairs(material.patterns) do with_entry[entry.pattern] = true end
  for _, saved in ipairs(material.automation) do
    if not with_entry[saved.pattern] then
      with_entry[saved.pattern] = true
      material.patterns[#material.patterns + 1] = { pattern = saved.pattern, lines = {}, writes = 1 }
    end
  end
  table.sort(material.patterns, function(a, b) return a.pattern < b.pattern end)
  -- The patterns, the instruments the notes play and the slots it is muted in are numbers that move: each is kept with what
  -- finds it again (see places.lua and the restore).
  local numbers, instruments = {}, {}
  for _, entry in ipairs(material.patterns) do
    numbers[entry.pattern] = true
    if entry.alias then numbers[entry.alias] = true end
    for _, row in ipairs(entry.lines or {}) do
      for _, cell in ipairs(row.notes) do
        if cell.instrument ~= 255 then instruments[cell.instrument] = true end
      end
    end
  end
  material.pool = material_.keep_pool(song, env, numbers)
  material.instruments = material_.keep_instruments(song, env, instruments)
  for s = 1, #sequencer.pattern_sequence do
    if sequencer:track_sequence_slot_is_muted(index, s) then material.slotMutes[#material.slotMutes + 1] = s end
  end
  if #material.slotMutes > 0 then
    material.sequence = {}
    for s = 1, #sequencer.pattern_sequence do material.sequence[s] = sequencer.pattern_sequence[s] end
  end
  material.writes = writes
  if kind == "send" or kind == "sequencer" then material.routes = material_.read_routes(song, index, nil, env) end
  return material
end

local function delete_apply(song, env, input, fence, desc)
  local ok, index, track, kind = pcall(find_to_delete, song, env, input)
  if not ok then base.gone_is_changed(index) end
  local now = signature(song, index, track)
  if not same_table(now, fence) then
    -- (another track at the ref's place is not the one that was previewed: name that one)
    if type(fence.name) == "string" and (fence.name ~= now.name or fence.kind ~= now.kind) then fail("changed", base.moved_text(fence.name)) end
    fail("changed", named(index, track) .. " changed since the preview")
  end
  local name = named(index, track)
  local material = gather(song, env, index, track, kind)
  local size = #json.encode(material)
  if size > base.MAX_MATERIAL_BYTES then
    fail("failed", string.format("%s holds %.1f MB, more than Kumi can keep to put it back, so it won't delete it", name, size / 1048576))
  end
  local steps = #plan_phases(material)
  if steps > base.MAX_RESTORE_STEPS then
    fail("failed", string.format("%s would take %d steps to put back, more than Kumi takes (%d): it couldn't be undone in time, so it won't delete it",
      name, steps, base.MAX_RESTORE_STEPS))
  end
  -- (M4 n-song: a soloed track's solo ends with it; a track Off only because another is soloed is Off by the solo, not by the
  -- producer: it comes back as Renoise makes it then, Off while a solo is on)
  local soloing = false
  for t = 1, rows.track_count(song) do
    local other = song:track(t)
    soloing = soloing or (t ~= index and not is_master(other) and other.solo_state)
  end
  local before = material.solo and solo_snapshot(song) or nil
  if not material.solo and soloing and material.muteState == OFF() then material.offBySolo = true end
  song:describe_undo(desc)
  song:delete_track_at(index)
  local note = material_.routes_note(#(material.routes or {}))
  if before then
    material.soloTracks, material.soloLeft = solo_around_delete(before, index, song)
    note = (note and (note .. " ") or "") .. delete_solo_note(song, material.soloTracks, material.soloLeft)
  end
  return {
    applied = { deleted = true },
    material = material,
    display = { title = "Deleted track " .. base.q(name), note = note }, target = target(env, index),
    undo = { desc = desc, entry = true },
    prior = { deleted = false }, place = { track = index }, shape = "deleted",
  }
end

-- What Renoise lets a track of this kind have: a send track has no output delay (it raises), no note columns, and the other
-- settings are tried and left as they are if Renoise refuses (they are looks, not content).
local function restore_properties(song, track, index, material)
  track.name = material.name
  local color = rgb_of(material.color)
  if color then track.color = color end
  track.color_blend = material.colorBlend
  track.prefx_volume.value = material.volume.pre
  track.postfx_volume.value = material.volume.post
  track.prefx_panning.value = material.panning.pre
  track.postfx_panning.value = material.panning.post
  local function try(write) pcall(write) end
  try(function() track.prefx_width.value = material.width end)
  if material.outputRouting then try(function() track.output_routing = material.outputRouting end) end
  if material.kind == "sequencer" and type(material.outputDelay) == "number" then
    try(function() track.output_delay = material.outputDelay end)
  end
  try(function() track.collapsed = material.collapsed == true end)
  local columns = material.columns or {}
  if track.max_note_columns > 0 then
    track.visible_note_columns = columns.note
    try(function() track.volume_column_visible = columns.volume == true end)
    try(function() track.panning_column_visible = columns.panning == true end)
    try(function() track.delay_column_visible = columns.delay == true end)
    try(function() track.sample_effects_column_visible = columns.sampleEffects == true end)
  end
  try(function() track.visible_effect_columns = columns.effect end)
  for c, name in pairs(columns.names or {}) do try(function() track:set_column_name(tonumber(c), name) end) end
  for _, c in ipairs(columns.muted or {}) do try(function() track:set_column_is_muted(c, true) end) end
  -- (a soloed track's solo and mute come back with every other track's, once it is all made: solo_after_restore; a track Off
  -- only because another was soloed is left as Renoise makes it, Off while a solo is on)
  if material.muteState and not material.solo and not material.offBySolo and rows.track_type(track) ~= "master" then
    try(function() track.mute_state = material.muteState end)
  end
end

-- Where the track goes back: its old place, moved by the tracks made and deleted since. A track made at or before the master's
-- place is a sequencer track (the master moves up); one made after it is a send track (measured in Renoise 3.5.4: inserting AT
-- the master's place makes a sequencer track, not a send).
local function restore_index(song, material)
  local master = song.sequencer_track_count + 1
  if material.kind == "send" then
    return math.max(master + 1, math.min(material.index, rows.track_count(song) + 1))
  end
  return math.max(1, math.min(material.index, master))
end

local function make_track(song, material)
  local index = restore_index(song, material)
  song:insert_track_at(index)
  return index
end

-- A member of a group goes back into it. The group is the one found where it was (`group.at`, carried through the tracks made, deleted
-- and swapped since by `moved`): groups are numbered by place ("Group 02"), so a name is no identity. Renoise makes a track inserted
-- just before a group (but not before its first member) a member by itself; otherwise it is added, which puts it last among the
-- members, and moved to its own place by swaps. The track is at `index` afterwards, in the group or not. Answers a note when it
-- stayed outside, and why.
-- Whether group `g` is `ancestor` or inside it.
local function within(parents, g, ancestor)
  while g do
    if g == ancestor then return true end
    g = parents[g]
  end
  return false
end

local function put_in_group(song, env, index, material, moved)
  if not material.group then return nil end
  local count = rows.track_count(song)
  local parents = rows.group_parents(song)
  local group = type(material.group.at) == "number" and moved(material.group.at, index) or nil
  -- (a group comes after its members: one before the track, or not a group, is not the one it was in. Put back at its place, the
  -- track may be in a group around the one it was in: the first member of a group inside another group is.)
  if not group or group <= index or group > count or song:track(group).type ~= renoise.Track.TRACK_TYPE_GROUP then
    if parents[index] then error({ code = "failed", message = "Renoise put the track into a group it wasn't in" }, 0) end
    return string.format("The group %s it was in isn't there any more, so it is back outside any group.", base.q(material.group.name))
  end
  if parents[index] == group then return nil end
  if parents[index] and not within(parents, group, parents[index]) then
    error({ code = "failed", message = "Renoise put the track into a group it wasn't in" }, 0)
  end
  local before = env.layout()
  song:add_track_to_group(index, group)
  -- Renoise moves it by swaps, which the layout's shifts follow: it is found where they took it, then swapped back to its place.
  local at = env.shifted(index, before)
  if not at then error({ code = "failed", message = "Renoise moved the track into its group and Kumi lost where it went" }, 0) end
  while at > index do
    song:swap_tracks_at(at, at - 1)
    at = at - 1
  end
  if rows.group_parents(song)[index] ~= group or env.shifted(index, before) ~= index then
    error({ code = "failed", message = "Renoise didn't put the track back into its group" }, 0)
  end
  return nil
end

-- Phase one, in the callback that made the track. Nothing half-done stays: a track that couldn't be finished is deleted again, where
-- it is (followed through the swaps this callback made itself). Answers the track's place and what to say about where it went.
-- Whether a slot still holds the place it had (Renoise's sequence notifier can't say which slot moved where). It does when the
-- sequence is the one the record saw (its patterns found where they are now, by `pattern_now`); or when the sequence up to it
-- is, and its pattern is none of its neighbours': patterns repeat, and a slot made or deleted before a run of one pattern
-- leaves the sequence up to it as it was. A slot made or deleted after it doesn't move it.
local function slot_now(kept, now, s, pattern_now)
  if type(kept) ~= "table" or s > #kept then return nil end
  local then_ = {}
  for k = 1, #kept do then_[k] = pattern_now(kept[k]) or false end
  return base.follow_slot(then_, now, s)
end

local function add_note(note, more)
  if not more then return note end
  return note and (note .. " " .. more) or more
end

local function restore_first_phase(song, env, material, phases, moved, discard, deleted_layout)
  local index = make_track(song, material)
  local made, since = index, env.layout()
  local note
  local tally = material_.new_tally()
  local ok, err = pcall(function()
    local track = song:track(index)
    restore_properties(song, track, index, material)
    for i, saved in ipairs(material.devices or {}) do material_.restore_device(track, i + 1, saved) end
    note = put_in_group(song, env, index, material, moved)
    -- (where a meta device sends its values: after the track has its place, and its devices are all there)
    for i, saved in ipairs(material.devices or {}) do
      if saved.destinations or saved.destination then
        for _, status in ipairs(material_.restore_destinations(song, song:track(index):device(i + 1), saved, index, moved, env)) do material_.count(tally, status) end
      end
    end
    local sequencer = song.sequencer
    local lost = 0
    local now = {}
    for k = 1, #sequencer.pattern_sequence do now[k] = sequencer.pattern_sequence[k] end
    for _, s in ipairs(material.slotMutes or {}) do
      local at = slot_now(material.sequence, now, s, material.pattern_now)
      if at then
        sequencer:set_track_sequence_slot_is_muted(index, at, true)
      else
        lost = lost + 1
      end
    end
    if lost > 0 then
      note = add_note(note, string.format("Its mute in %s isn't put back: the sequence changed in Renoise since in a way that doesn't say where %s went.",
        lost == 1 and "1 slot" or (lost .. " slots"), lost == 1 and "that slot" or "those slots"))
    end
    local done = {}
    for _, entry in ipairs(phases[1]) do
      material_.write_lines(song, index, entry, material.instrument_map)
      done[entry.pattern] = true
    end
    material_.write_automation(song, song:track(index), index, material.automation, done)
    material_.write_routes(song, material.routes, { track = index }, moved, tally, env)
    note = add_note(note, material_.tally_note(tally))
    note = add_note(note, material.lost_note)
    -- (the solo last, with the routes back: a send's solo solos the tracks feeding it again)
    local solo_ok, solo_note = pcall(solo_after_restore, song, env, index, material, deleted_layout, discard)
    if solo_ok then
      note = add_note(note, solo_note)
    else
      note = add_note(note, "Kumi couldn't put its solo back: " .. (type(solo_note) == "table" and solo_note.message or tostring(solo_note)) .. ".")
    end
  end)
  if not ok then
    local at = (env.shifted(made, since))
    if at and at >= 1 and at <= rows.track_count(song) then
      pcall(function() song:delete_track_at(at) end)
    else
      err = { code = "uncertain", message = "Putting the track back stopped and Kumi can't tell where the half-made one is: look for it" }
    end
    if type(err) ~= "table" then err = { code = "failed", message = "Renoise wouldn't put the track back: " .. tostring(err):gsub("^[^%s:]*%.lua:%d+: ", "") } end
    error(err, 0)
  end
  return index, note
end

-- Where a track numbered as the song was after the delete (layout `since`) is now, with the deleted track put back at `inserted`
-- (the layout already counts that: it is made in this callback). Nil when it is gone.
local function moved_by(env, since)
  return function(at, inserted)
    if type(since) ~= "number" then return inserted and at >= inserted and at + 1 or at end
    return (env.shifted(at, since))
  end
end

local function restored_answer(env, song, index, material, note)
  local track = song:track(index)
  local display = { title = base.q(named(index, track)) .. " put back", note = note }
  return {
    restored = { ref = refs.make(env.epoch(), "track", index), name = track.name },
    display = display,
  }
end

-- A track with this name and kind is at the place already: it came back some other way, or was never gone. A name Renoise numbers
-- itself ("Track 03", "S01") tells nothing (the next track takes it when this one goes): then the same things on the track do, and
-- an empty track can't be told from another empty one, so it is made again.
local function already_there(song, material, prior)
  local top = rows.track_count(song)
  local want = base.name_id(material.kind, material.name)
  for _, at in ipairs({ material.index, material.indexThen or material.index }) do
    local track = at >= 1 and at <= top and song:track(at) or nil
    if track ~= nil and rows.track_type(track) == material.kind and base.track_id(track) == want then
      if not want:match(":$") then return true end
      if type(prior) == "table" and (prior.devices or 0) + (prior.patternsWithLines or 0) > 0 then
        local now = signature(song, at, track)
        if now.devices == prior.devices and now.patternsWithLines == prior.patternsWithLines and now.noteColumns == prior.noteColumns
          and now.effectColumns == prior.effectColumns then
          return true
        end
      end
    end
  end
  return false
end

-- The record's patterns and instruments where they are now (see places.lua): a copy of the material to write from (with
-- `pattern_now` for the sequence, `instrument_map` and `lost_note`), or a refusal that touches nothing when where they are
-- can't be told. Lines in a pattern deleted since, or one that isn't what it was (its name or length changed), aren't put back,
-- and the answer says so.
local function placed(song, env, material, discard)
  local q = base.q
  local pattern_now, follow = material_.pool_now(song, env, material.pool)
  if not pattern_now then
    fail("stale", string.format("the song's patterns changed in a way Kumi couldn't follow since %s was deleted, so it can't tell where its lines go: nothing was put back", q(material.name)))
  end
  local map, why = material_.instruments_now(song, env, material.instruments)
  if not map then
    if why == "unknown" then
      fail("stale", string.format("the instruments changed in a way Kumi couldn't follow since %s was deleted, so it can't tell which its notes play: nothing was put back", q(material.name)))
    end
    if not discard then
      fail("changed", string.format("the instrument %s that %s's notes play was deleted in Renoise since: Kumi won't put them back playing another", q(why), q(material.name)))
    end
    map = {}
  end
  local copy = {}
  for k, v in pairs(material) do copy[k] = v end
  copy.patterns, copy.automation = {}, {}
  local lost, count = {}, 0
  local function lose(p)
    if not lost[p] then lost[p], count = true, count + 1 end
  end
  for _, entry in ipairs(material.patterns or {}) do
    local p, alias = pattern_now(entry.pattern), entry.alias and pattern_now(entry.alias) or nil
    if p and (entry.alias == nil or alias) then
      local moved = {}
      for k, v in pairs(entry) do moved[k] = v end
      moved.pattern, moved.alias = p, alias
      copy.patterns[#copy.patterns + 1] = moved
    else
      lose(entry.pattern)
    end
  end
  for _, saved in ipairs(material.automation or {}) do
    local p = pattern_now(saved.pattern)
    if p then
      copy.automation[#copy.automation + 1] = { pattern = p, device = saved.device, parameter = saved.parameter, playmode = saved.playmode, points = saved.points }
    else
      lose(saved.pattern)
    end
  end
  if count > 0 then
    copy.lost_note = string.format("Its lines in %s aren't put back: %s deleted or changed in Renoise since.",
      count == 1 and "1 pattern" or (count .. " patterns"), count == 1 and "it was" or "they were")
  end
  copy.pattern_now, copy.instrument_map = follow, map
  return copy
end

local function delete_restore(song, env, restore, desc, journal)
  local material = restore.material
  if type(material) ~= "table" or type(material.name) ~= "string" then bad("material: what the delete removed") end
  material = placed(song, env, material, restore.discard)
  local phases = plan_phases(material)
  if already_there(song, material, restore.prior) and #phases >= 1 and not restore.discard then
    fail("changed", "a track with that name is in the song again")
  end
  if #phases > base.MAX_RESTORE_STEPS then
    fail("failed", string.format("that is %d steps to put back, more than Kumi takes (%d): it can't be undone in time", #phases, base.MAX_RESTORE_STEPS))
  end
  if #phases == 1 then
    song:describe_undo(desc)
    local index, note = restore_first_phase(song, env, material, phases, moved_by(env, restore.layout), restore.discard, restore.layout)
    local answer = restored_answer(env, song, index, material, note)
    answer.undo = { desc = desc, entry = true }
    base.note_restored(env.journal, "track-delete", { track = index })
    return answer
  end
  -- Too much for one callback: the track and its parts in the first, then lines, each its own numbered step.
  return {
    kumi_job = function(job)
      local total = #phases
      local function step(k) return string.format("%s (%d/%d)", desc, k, total) end
      song = the_song()
      song:describe_undo(step(1))
      local index, note = restore_first_phase(song, env, material, phases, moved_by(env, restore.layout), restore.discard, restore.layout)
      -- (from here a stop leaves a half-made track: a tool stopped or reloaded says so, see jobs.lua)
      job.wrote = string.format("Putting %s back", base.q(material.name))
      -- Phase one is one callback, so nothing else can land in it: the shape, the layout and the epoch it moved to (the devices
      -- and the swaps into a group move it) are the ones to hold to from here.
      job.rebase()
      local made = { index = index, since = env.layout(), id = base.track_id(song:track(index)) }
      -- The made track's place now (the tracks made and deleted since are followed), if that place holds a track like it.
      local function made_at()
        local at, why = env.shifted(made.index, made.since)
        if not at then return nil, why end
        local live = the_song()
        if at < 1 or at > rows.track_count(live) or base.track_id(live:track(at)) ~= made.id then return nil, "other" end
        return at
      end
      local ok, err = pcall(function()
        for k = 2, total do
          job.step(base.STEP_GAP_MS)
          base.wait_while_busy(job, env)
          local live = the_song()
          local at = made_at()
          if not at then fail("changed", "the track being put back isn't where it was made any more") end
          live:describe_undo(step(k))
          local done = {}
          for _, entry in ipairs(phases[k]) do
            material_.write_lines(live, at, entry, material.instrument_map)
            done[entry.pattern] = true
          end
          material_.write_automation(live, live:track(at), at, material.automation, done)
        end
      end)
      if not ok then
        -- Nothing half-done stays: the track this restore made goes again, if it is where it was made. When that can't be told
        -- (the producer undid or moved it, a dialog is open), Kumi deletes nothing and says so.
        local at = made_at()
        local code = type(err) == "table" and err.code or nil
        if at and not env.busy() then
          pcall(function() the_song():delete_track_at(at) end)
          job.rebase()
          local text = type(err) == "table" and err.message or tostring(err)
          error({
            code = type(err) == "table" and err.code or "failed",
            message = string.format("Putting %s back stopped (%s): the half-made track is taken out again, and nothing else was touched", base.q(material.name), text),
          }, 0)
        end
        job.rebase()
        local why = at and "a dialog is open, so Kumi left it" or "it isn't where Kumi made it any more, so Kumi deleted nothing"
        error({
          code = "uncertain",
          message = string.format("Putting %s back stopped (%s) and %s: a half-made one may be in the song, look for it", base.q(material.name),
            type(err) == "table" and err.message or tostring(err), why),
        }, 0)
      end
      local live = the_song()
      local answer = restored_answer(env, live, index, material, note)
      answer.undo = { desc = desc, entry = true, steps = total }
      answer.epoch, answer.layout = env.epoch(), env.layout()
      base.note_restored(env.journal, "track-delete", { track = index })
      return answer
    end,
  }
end

base.register({
  kind = "track-delete", custom = true, shape = "deleted",
  preview = delete_preview, apply = delete_apply, restore = delete_restore,
})

-- M4 (n-song): a track's collapse, width, output routing and delay, the columns it shows, and a note column's name and mute.
-- Each is a Renoise undo step (measured in 3.5.4, lab 2026-10-19; a column's mute is one, unlike the track's mute). A group's
-- `group_collapsed` has no observable, so Renoise's undo of it alone isn't seen: Kumi looks again at the next undo it sees.

local flag, text_of = base.flag, base.text_of
local T = renoise.Track

local function number_text(value)
  local text = string.format("%.2f", value):gsub("0+$", ""):gsub("%.$", "")
  return (text:gsub("^%-", base.MINUS))
end

prop(base.settings {
  kind = "track-collapse", noun = "collapse", master = true, undoable = true, named = true,
  fields = {
    {
      name = "collapsed", read = function(t) return t.collapsed end, write = function(t, v) t.collapsed = v end,
      check = flag, text = function(v) return v and "collapsed" or "expanded" end,
    },
    {
      name = "groupCollapsed",
      read = function(t) return t.type == T.TRACK_TYPE_GROUP and t.group_collapsed or false end,
      write = function(t, v) t.group_collapsed = v end,
      check = function(v, name, t)
        if t.type ~= T.TRACK_TYPE_GROUP then bad("groupCollapsed is a group track's (it folds the group's tracks)") end
        return flag(v, name)
      end,
      text = function(v) return v and "with its tracks folded" or "with its tracks shown" end,
    },
  },
})

prop(base.settings {
  kind = "track-width", noun = "width", master = true, undoable = true, named = true, watch = true,
  fields = {
    {
      name = "width",
      read = function(t)
        local w = t.prefx_width
        return w.value / w.value_max * 100
      end,
      write = function(t, v)
        local w = t.prefx_width
        w.value = v / 100 * w.value_max
      end,
      check = function(v)
        if type(v) ~= "number" or v ~= v or v < 0 or v > 100 then bad("width must be a number from 0 to 100 (percent, as Renoise shows it)") end
        return v
      end,
      text = function(v) return "width " .. number_text(v) .. "%" end,
    },
  },
})

-- The routings a track may take are names: the master, a group around it, the audio device's outputs. A group's name moves with
-- its place ("Group 04"), so a record keeps the group as a track ref (the layout carries it) and the name is read from it again.
local function routing_named(track, wanted)
  if type(wanted) ~= "string" or wanted == "" then bad("routing: one of the track's output routings, by name") end
  local available, found = track.available_output_routings, nil
  for _, name in ipairs(available) do
    if name == wanted then return name end
    if name:lower() == wanted:lower() then
      if found then bad(string.format("%q names more than one routing: give it as Renoise writes it", wanted)) end
      found = name
    end
  end
  if found then return found end
  bad(string.format("%s can't go to %q: its routings are %s", track.name, wanted, table.concat(available, ", ")))
end

local function group_named(song, name)
  local found
  for i = 1, rows.track_count(song) do
    local track = song:track(i)
    if track.type == T.TRACK_TYPE_GROUP and track.name == name then
      if found then return nil end
      found = i
    end
  end
  return found
end

prop {
  kind = "track-output", noun = "output routing", master = true, undoable = true,
  read = function(track) return { routing = track.output_routing } end,
  wanted = function(track, input) return { routing = routing_named(track, input.routing) } end,
  write = function(track, state) track.output_routing = state.routing end,
  same = function(a, b) return a.routing == b.routing end,
  title = function(name, _, to) return name .. " routed to " .. base.q(to.routing) end,
  wire = function(state, env)
    if type(state) ~= "table" or type(state.routing) ~= "string" then return state end
    local song = renoise.song()
    local group = song and group_named(song, state.routing)
    if not group then return state end
    return { routing = state.routing, at = refs.make(env.epoch(), "track", group) }
  end,
  unwire = function(state, env)
    if type(state) ~= "table" or type(state.at) ~= "string" then return state end
    local song = the_song()
    local group = refs.expect(state.at, "track", 1, env.epoch())[1]
    if group > rows.track_count(song) or song:track(group).type ~= T.TRACK_TYPE_GROUP then
      fail("changed", "the group " .. base.q(state.routing) .. " isn't where it was any more")
    end
    return { routing = song:track(group).name }
  end,
}

prop {
  kind = "track-delay", noun = "output delay", master = false, undoable = true,
  read = function(track) return { delayMs = track.output_delay } end,
  wanted = function(track, input)
    if track.type ~= T.TRACK_TYPE_SEQUENCER then
      bad("only a sequencer track has an output delay (Renoise refuses one on a group or a send track)")
    end
    local v = input.delayMs
    if type(v) ~= "number" or v ~= v or v < -100 or v > 100 then bad("delayMs must be a number from -100 to 100") end
    return { delayMs = v }
  end,
  write = function(track, state) track.output_delay = state.delayMs end,
  same = function(a, b) return close(a.delayMs, b.delayMs) end,
  title = function(name, _, to) return name .. " delay " .. number_text(to.delayMs) .. " ms" end,
}

-- The columns a track shows. Hiding a column hides what is in it (Renoise keeps it): the preview says so when it holds notes.
local SUBCOLUMNS = {
  { name = "volume", field = "volume_column_visible", word = "volume" },
  { name = "panning", field = "panning_column_visible", word = "panning" },
  { name = "delay", field = "delay_column_visible", word = "delay" },
  { name = "sampleEffects", field = "sample_effects_column_visible", word = "sample effects" },
}

-- (only a sequencer track has note columns; a group raises on them, measured in 3.5.4)
local function note_columns_of(track) return track.type == T.TRACK_TYPE_SEQUENCER and track.max_note_columns or 0 end

local function has_notes(track, name)
  if note_columns_of(track) == 0 then bad(name .. ": this track has no note columns (only a sequencer track has them)") end
end

local columns_fields = {
  {
    name = "noteColumns", read = function(t) return t.visible_note_columns end, write = function(t, v) t.visible_note_columns = v end,
    check = function(v, name, t)
      has_notes(t, name)
      if type(v) ~= "number" or v ~= math.floor(v) or v < 1 or v > note_columns_of(t) then bad(string.format("noteColumns is from 1 to %d", note_columns_of(t))) end
      return v
    end,
    text = function(v) return v == 1 and "1 note column" or (v .. " note columns") end,
  },
  {
    name = "effectColumns", read = function(t) return t.visible_effect_columns end, write = function(t, v) t.visible_effect_columns = v end,
    check = function(v, _, t)
      local low, high = t.min_effect_columns, t.max_effect_columns
      if type(v) ~= "number" or v ~= math.floor(v) or v < low or v > high then bad(string.format("effectColumns is from %d to %d on this track", low, high)) end
      return v
    end,
    text = function(v) return v == 1 and "1 effect column" or (v .. " effect columns") end,
  },
}
for _, sub in ipairs(SUBCOLUMNS) do
  columns_fields[#columns_fields + 1] = {
    name = sub.name, read = function(t) return note_columns_of(t) > 0 and t[sub.field] or false end, write = function(t, v) t[sub.field] = v end,
    check = function(v, name, t)
      has_notes(t, name)
      return flag(v, name)
    end,
    text = function(v) return (v and "the " or "no ") .. sub.word .. " column" end,
  }
end

-- What the track's patterns hold in the columns a change would hide: a note for the preview, or nil when they hold nothing; nil
-- and "unknown" past what is worth reading through.
M.HIDDEN_READS = 40000
local function hidden_note(loc, wanted)
  local song, track, index = renoise.song(), loc.object, loc.place.track
  if not song then return nil end
  local notes_from = wanted.noteColumns and wanted.noteColumns < track.visible_note_columns and wanted.noteColumns + 1 or nil
  local effects_from = wanted.effectColumns and wanted.effectColumns < track.visible_effect_columns and wanted.effectColumns + 1 or nil
  local subs = {}
  for _, sub in ipairs(SUBCOLUMNS) do
    if wanted[sub.name] == false and note_columns_of(track) > 0 and track[sub.field] then subs[#subs + 1] = sub end
  end
  if not notes_from and not effects_from and #subs == 0 then return nil end
  local patterns, reads = 0, 0
  for p = 1, #song.patterns do
    local pattern_track = song:pattern(p):track(index)
    local found = false
    if not pattern_track.is_alias and not pattern_track.is_empty then
      for l = 1, song:pattern(p).number_of_lines do
        local line = pattern_track:line(l)
        reads = reads + 1
        if not line.is_empty then
          for c = 1, note_columns_of(track) do
            local column = line:note_column(c)
            reads = reads + 1
            if not column.is_empty then
              if notes_from and c >= notes_from then found = true end
              for _, sub in ipairs(subs) do
                if (sub.name == "volume" and column.volume_string ~= "..") or (sub.name == "panning" and column.panning_string ~= "..")
                  or (sub.name == "delay" and column.delay_string ~= "..")
                  or (sub.name == "sampleEffects" and (column.effect_number_value ~= 0 or column.effect_amount_value ~= 0)) then
                  found = true
                end
              end
            end
          end
          if effects_from then
            for c = effects_from, track.max_effect_columns do
              if not line:effect_column(c).is_empty then found = true end
            end
          end
        end
        if found or reads > M.HIDDEN_READS then break end
      end
    end
    if found then patterns = patterns + 1 end
    if reads > M.HIDDEN_READS then return nil, "unknown" end
  end
  if patterns == 0 then return nil end
  return string.format("What it hides holds notes in %s: Renoise keeps them, hidden (and plays them).",
    patterns == 1 and "1 pattern" or (patterns .. " patterns"))
end

local columns_spec = base.settings({
  kind = "track-columns", noun = "columns", master = true, undoable = true, named = true, fields = columns_fields,
})
columns_spec.note = function(loc, _, wanted)
  local note, why = hidden_note(loc, wanted)
  if why == "unknown" then
    return "The track is too big for Kumi to look through what it hides: it may hold notes, which Renoise keeps, hidden (and plays)."
  end
  return note
end
-- (the undo hides columns again: what they held when Kumi showed them may be hidden again, but what the producer wrote in
-- them since is refused, unless discard; and so is hiding columns Kumi couldn't look through, then or now)
columns_spec.keep = function(loc, before)
  local note, why = hidden_note(loc, before)
  return { heldHidden = note ~= nil or (why == "unknown" and "unknown") or false }
end
columns_spec.before_restore = function(loc, _, prior, applied)
  if type(applied) ~= "table" or applied.heldHidden == true or applied.heldHidden == nil then return end
  local note, why = hidden_note(loc, prior)
  if applied.heldHidden == "unknown" or why == "unknown" then
    fail("changed", loc.name .. " is too big for Kumi to tell whether the columns it showed hold notes written since: hiding them again might hide those, so Kumi left them")
  end
  if note ~= nil then
    fail("changed", loc.name .. "'s columns Kumi showed hold notes written since: hiding them again would hide those, so Kumi left them")
  end
end
prop(columns_spec)

-- A note column's name and mute: the column is part of what the change is about, so the record keeps it (`col`).
local column_spec = base.settings({
  kind = "track-column", noun = "column", undoable = true, named = true,
  fields = {
    {
      name = "name", read = function(o) return o.track:column_name(o.c) end, write = function(o, v) o.track:set_column_name(o.c, v) end,
      check = function(v, name) return text_of(v, name, 64) end,
      text = function(v) return v == "" and "unnamed" or ("named " .. base.q(v)) end,
    },
    {
      name = "mute", read = function(o) return o.track:column_is_muted(o.c) end, write = function(o, v) o.track:set_column_is_muted(o.c, v) end,
      check = flag, text = function(v) return v and "muted" or "unmuted" end,
    },
  },
})
local column_fields = column_spec.read
column_spec.read = function(object, source)
  local state = column_fields(object, source)
  state.col = object.c
  return state
end
column_spec.locate = function(song, env, input)
  local index, track = find(song, env, input.ref, "track-column")
  if note_columns_of(track) == 0 then bad(named(index, track) .. " has no note columns: only a sequencer track has them") end
  local c = input.col
  if type(c) ~= "number" or c ~= math.floor(c) or c < 1 or c > note_columns_of(track) then
    bad(string.format("col is a note column from 1 to %d", note_columns_of(track)))
  end
  return {
    object = { track = track, c = c }, name = string.format("%s column %d", named(index, track), c), place = { track = index, column = c },
    target = target(env, index), id = base.track_id(track) .. "|" .. c,
  }
end
column_spec.reread = function(song, place)
  if place.track < 1 or place.track > rows.track_count(song) then return nil end
  local track = song:track(place.track)
  if place.column > note_columns_of(track) then return nil end
  return { track = track, c = place.column }
end
column_spec.restore_input = function(restore)
  return { ref = restore.target and restore.target.ref, col = type(restore.prior) == "table" and restore.prior.col or nil }
end
base.register(column_spec)

return M
