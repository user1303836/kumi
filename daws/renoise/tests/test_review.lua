-- The M3 review's findings on the Renoise tool (M-1 .. M-7, S-2 .. S-7), each as a test that fails on the code it was found in.
local T = ...
local mock = T.mock
local json = T.load_payload_module("json")
local test, eq, ok = T.test, T.eq, T.ok
local H = T.harness

local next_id = 70000

local function drain(peer)
  repeat
    local taken = peer:take()
    if #taken > 0 then H.send(peer, { type = "credit", bytes = #taken }) end
  until #taken == 0
end

-- Looks for the response to `id` in what has come; `step` is how much time passes between looks.
local function wait(peer, id, ticks, step)
  peer.events = peer.events or {}
  for _ = 1, ticks or 400 do
    drain(peer)
    for _, line in ipairs(peer:frames(nil)) do
      local frame = assert(json.decode(line))
      if frame.type == "response" and frame.id == id then return frame end
      if frame.type == "event" then peer.events[#peer.events + 1] = frame end
    end
    mock.advance(step or 10)
  end
end

local function send(peer, method, params)
  next_id = next_id + 1
  H.send(peer, { type = "request", id = next_id, method = method, params = params or {} })
  return next_id
end

local function call(peer, method, params, ticks)
  return wait(peer, send(peer, method, params), ticks)
end

local function song_options(extra)
  local spec = {
    name = "Lab", file_name = "/s/lab.xrns", bpm = 120, lpb = 4, mixer_device = true,
    tracks = { { name = "Kick", color = { 255, 0, 0 } }, { name = "Hat" }, { name = "Bass", note_columns = 2 }, { name = "Pad" } },
    send_tracks = 1, patterns = 3, pattern_lines = 32, sequence_length = 4, sequence = { 1, 2, 1, 3 }, instruments = 1,
  }
  for k, v in pairs(extra or {}) do spec[k] = v end
  return spec
end

-- A patch that sets constants in the temporary copy of changes_base.lua.
local function constants(values)
  return function(env)
    local path = env.bundle .. "payload/changes_base.lua"
    local f = assert(io.open(path, "rb"))
    local text = f:read("*a")
    f:close()
    for name, value in pairs(values) do
      local lowered, n = text:gsub("M%." .. name .. " = [%d%* ]+", "M." .. name .. " = " .. value, 1)
      assert(n == 1, name .. " was not found to set")
      text = lowered
    end
    f = assert(io.open(path, "wb"))
    f:write(text)
    f:close()
  end
end

local function setup(options, extra)
  options = options or {}
  options.song = song_options(extra)
  local env = H.setup(options)
  local peer = H.connect(env)
  H.frames(peer)
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  return env, peer
end

local function place(peer)
  local focus = call(peer, "focus").result
  return focus.epoch, focus.layout
end

local function track_ref(epoch, i) return epoch .. ":track:" .. i end
local function seq_ref(epoch, s) return epoch .. ":seq:" .. s end
local function pattern_ref(epoch, p) return epoch .. ":pattern:" .. p end
local function steps() return #mock.data.undo.stack end

local function ask(peer, method, kind, input, more)
  local epoch, layout = place(peer)
  local params = { kind = kind, input = input, epoch = epoch, layout = layout }
  for k, v in pairs(more or {}) do params[k] = v end
  return call(peer, method, params), epoch, layout
end

local function preview(peer, kind, input)
  local response, epoch, layout = ask(peer, "change_preview", kind, input)
  ok(response and response.result, kind .. " preview: " .. json.encode(response and response.error or {}))
  return response.result, epoch, layout
end

local function apply(peer, kind, input, desc, fence, epoch, layout)
  return call(peer, "change_apply", { kind = kind, input = input, fence = fence, desc = desc, epoch = epoch, layout = layout })
end

local function restore_params(kind, record, desc, epoch, layout)
  return {
    kind = kind, target = record.target, prior = record.prior, applied = record.applied, created = record.created,
    material = record.material, desc = desc, epoch = epoch, layout = layout,
  }
end

local function restore(peer, kind, record, desc, epoch, layout)
  return call(peer, "change_restore", restore_params(kind, record, desc, epoch, layout), 800)
end

local function failure(response, code)
  ok(response and response.error, "expected an error, got " .. json.encode(response and response.result or "nothing"))
  eq(response.error.code, code, "error code (" .. tostring(response.error.message) .. ")")
end

local function names()
  local out = {}
  for i, t in ipairs(mock.data.tracks) do out[i] = t.name end
  return table.concat(out, ",")
end

local function order()
  local out = {}
  for i, t in ipairs(mock.data.tracks) do out[i] = t.name .. (t.kind == "group" and "[G]" or "") .. (mock.song:track(i).group_parent and "(in)" or "") end
  return table.concat(out, ",")
end

-- Lines in the first `pats` patterns of a track (the writes of one callback each when SLICE_WRITES is small).
local function fill(t, pats, lines)
  for p = 1, pats or 3 do
    for k = 1, lines or 10 do
      local column = mock.song:pattern(p):track(t):line(k):note_column(1)
      column.note_value, column.instrument_value, column.volume_value = 40 + k, 1, 64
    end
  end
end

-- Deletes track `t` the way Kumi does; returns the preview and the apply's answer.
local function delete_track(peer, t, desc)
  local epoch, layout = place(peer)
  local previewed = preview(peer, "track-delete", { ref = track_ref(epoch, t) })
  local done = apply(peer, "track-delete", { ref = track_ref(epoch, t) }, desc or "Kumi: d (kr01)", previewed.fence, epoch, layout)
  ok(done.result, json.encode(done.error or {}))
  return previewed, done.result
end

local function record_of(previewed, done)
  return { target = previewed.target, prior = previewed.prior, applied = done.applied, material = done.material }
end

-- Starts a sliced restore and runs the clock until its first step is written (the others wait 400 ms each).
local function start_restore(peer, previewed, done, desc)
  local id = send(peer, "change_restore", restore_params("track-delete", record_of(previewed, done), desc, done.epoch, done.layout))
  local before = steps()
  for _ = 1, 50 do
    mock.advance(10)
    if steps() > before then break end
  end
  eq(steps(), before + 1, "step 1 is written")
  return id
end

-- M-1: a stopped sliced restore cleans up only the track it made

test("review M-1: the producer takes step 1 back between the steps: the clean-up deletes nothing, and says uncertain", function()
  local env, peer = setup({ patch = constants({ SLICE_WRITES = 40 }) })
  fill(2)
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  local previewed, done = delete_track(peer, 2)
  local id = start_restore(peer, previewed, done, "Kumi: undo d (kr02)")
  eq(names(), "Kick,Hat,Bass,Pad,Master,Send 1")
  mock.song:undo() -- the producer's Cmd-Z: the made Hat is gone again
  eq(names(), "Kick,Bass,Pad,Master,Send 1")
  local response = wait(peer, id, 300, 20)
  failure(response, "uncertain")
  eq(names(), "Kick,Bass,Pad,Master,Send 1", "Bass is the producer's, and stays")
  H.no_dialogs()
end)

test("review M-1: the producer makes a track before the half-made one: the clean-up takes the half-made one, not the track now in its place", function()
  local env, peer = setup({ patch = constants({ SLICE_WRITES = 40 }) })
  fill(2)
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  local previewed, done = delete_track(peer, 2)
  -- (a line that Renoise refuses stops the restore at its second step)
  done.material.patterns[2].lines[1].notes[1].col = 99
  local id = start_restore(peer, previewed, done, "Kumi: undo d (kr03)")
  mock.ops.insert_track(1, { name = "Producer's" })
  eq(names(), "Producer's,Kick,Hat,Bass,Pad,Master,Send 1")
  local response = wait(peer, id, 300, 20)
  ok(response and response.error, "the restore stops")
  eq(names(), "Producer's,Kick,Bass,Pad,Master,Send 1", "the half-made Hat went, Kick stayed")
  H.no_dialogs()
end)

-- M-2: a sliced restore of a track with a device, or a group's member, ends well and answers the epoch it moved to

test("review M-2: a sliced restore of a track with a device finishes and answers the epoch the device moved", function()
  local env, peer = setup({ patch = constants({ SLICE_WRITES = 40 }) })
  mock.song:track(2):insert_device_at("Audio/Effects/Native/EQ 5", 2)
  fill(2)
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  local previewed, done = delete_track(peer, 2)
  local id = send(peer, "change_restore", restore_params("track-delete", record_of(previewed, done), "Kumi: undo d (kr04)", done.epoch, done.layout))
  local undone = wait(peer, id, 300, 20)
  ok(undone and undone.result, json.encode(undone and undone.error or "no answer"))
  eq(undone.result.undo.steps, 3)
  eq(names(), "Kick,Hat,Bass,Pad,Master,Send 1")
  eq(mock.data.tracks[2].devices[2].path, "Audio/Effects/Native/EQ 5")
  ok(undone.result.epoch ~= done.epoch, "the device moved the epoch")
  ok(undone.result.kept and undone.result.kept.from == done.epoch, "and the answer says what was kept: " .. json.encode(undone.result.kept))
  eq(undone.result.restored.ref, track_ref(undone.result.epoch, 2))
  H.no_dialogs()
end)

test("review M-2: a sliced restore of a group's first member finishes, and it is a member again", function()
  local env, peer = setup({ patch = constants({ SLICE_WRITES = 40 }) })
  mock.song:insert_group_at(5)
  mock.song:add_track_to_group(3, 5)
  mock.song:add_track_to_group(3, 5)
  fill(3)
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  local before = order()
  eq(before, "Kick,Hat,Bass(in),Pad(in),Group[G],Master,Send 1")
  local previewed, done = delete_track(peer, 3)
  local id = send(peer, "change_restore", restore_params("track-delete", record_of(previewed, done), "Kumi: undo d (kr05)", done.epoch, done.layout))
  local undone = wait(peer, id, 300, 20)
  ok(undone and undone.result, json.encode(undone and undone.error or "no answer"))
  eq(order(), before)
  H.no_dialogs()
end)

-- M-7: a dialog, a render

test("review M-7: a dialog is open when an apply or a preview arrives: busy, nothing written", function()
  local env, peer = setup()
  local epoch, layout = place(peer)
  local input = { ref = track_ref(epoch, 1), volumeDb = -6 }
  local previewed = preview(peer, "track-volume", input)
  mock.advance(100) -- (the timers run)
  mock.stall_timers = true -- the producer opened a dialog: Renoise's timers stop, its sockets still answer
  mock.advance(1000)
  failure(apply(peer, "track-volume", input, "Kumi: v (kr06)", previewed.fence, epoch, layout), "busy")
  failure(call(peer, "change_preview", { kind = "track-volume", input = input, epoch = epoch, layout = layout }), "busy")
  mock.stall_timers = false
  eq(mock.data.tracks[1].volume, 1, "nothing was written")
  eq(steps(), 0)
  -- the dialog closed: the same apply goes through
  mock.advance(200)
  local again = apply(peer, "track-volume", input, "Kumi: v (kr07)", previewed.fence, epoch, layout)
  ok(again.result, json.encode(again.error or {}))
  H.no_dialogs()
end)

test("review M-7: a request that takes long to receive and decode is not taken for a dialog", function()
  local env, peer = setup()
  local epoch, layout = place(peer)
  local input = { ref = track_ref(epoch, 1), volumeDb = -6 }
  local previewed = preview(peer, "track-volume", input)
  mock.advance(100)
  mock.clock_step_ms = 100 -- each look at the clock costs 100 ms, as the decoding of a megabyte of material does
  local done = apply(peer, "track-volume", input, "Kumi: v (kr53)", previewed.fence, epoch, layout)
  mock.clock_step_ms = 0
  ok(done.result, json.encode(done.error or {}))
  H.no_dialogs()
end)

test("review M-7: a restore asked behind a dialog is busy too, and writes nothing", function()
  local env, peer = setup()
  local epoch, layout = place(peer)
  local input = { ref = track_ref(epoch, 1), volumeDb = -6 }
  local previewed = preview(peer, "track-volume", input)
  local done = apply(peer, "track-volume", input, "Kumi: v (kr08)", previewed.fence, epoch, layout)
  ok(done.result, json.encode(done.error or {}))
  mock.advance(100)
  mock.stall_timers = true
  mock.advance(1000)
  local before = steps()
  failure(restore(peer, "track-volume", { target = previewed.target, prior = previewed.prior, applied = done.result.applied }, "Kumi: undo v (kr09)", epoch, layout), "busy")
  mock.stall_timers = false
  eq(steps(), before)
  ok((mock.data.tracks[1].post_volume or 1) < 1, "still at -6 dB")
  H.no_dialogs()
end)

test("review M-7: a render starts after step 1 of a sliced restore: no later step is written until it ends", function()
  local env, peer = setup({ patch = constants({ SLICE_WRITES = 40 }) })
  fill(2)
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  local previewed, done = delete_track(peer, 2)
  local id = start_restore(peer, previewed, done, "Kumi: undo d (kr10)")
  local written = steps()
  mock.data.song_fields.rendering = true
  for _ = 1, 40 do
    ok(not wait(peer, id, 1, 50), "no answer while it waits")
  end
  eq(steps(), written, "nothing is written behind the render")
  ok(mock.song:pattern(3):track(2).is_empty, "the last step's lines are not there")
  mock.data.song_fields.rendering = false
  local response = wait(peer, id, 300, 20)
  ok(response and response.result, json.encode(response and response.error or "no answer"))
  eq(response.result.undo.steps, 3)
  ok(not mock.song:pattern(3):track(2).is_empty, "the render ended and the restore finished")
  H.no_dialogs()
end)

test("review M-7: a render that outlasts the wait: uncertain, nothing written behind it, the half-made track left", function()
  local env, peer = setup({ patch = constants({ SLICE_WRITES = 40, BUSY_WAIT_MS = 600 }) })
  fill(2)
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  local previewed, done = delete_track(peer, 2)
  local id = start_restore(peer, previewed, done, "Kumi: undo d (kr11)")
  local written = steps()
  mock.data.song_fields.rendering = true
  local response = wait(peer, id, 100, 50)
  mock.data.song_fields.rendering = false
  failure(response, "uncertain")
  eq(steps(), written, "nothing was written behind the render, the clean-up included")
  eq(names(), "Kick,Hat,Bass,Pad,Master,Send 1", "the half-made track is left for the producer")
  H.no_dialogs()
end)

-- M-3 (the tool's half): what the preview located is what the apply writes

test("review M-3: an apply that locates another track than the preview did is refused `changed`, nothing written", function()
  local env, peer = setup()
  local epoch, layout = place(peer)
  local input = { ref = track_ref(epoch, 3), volumeDb = -6 } -- Bass, in Kumi's book
  mock.ops.insert_track(1, { name = "New" })
  H.tick(2)
  local previewed = call(peer, "change_preview", { kind = "track-volume", input = input, epoch = epoch, layout = layout }).result
  ok(previewed, "a preview in the older layout carries the ref")
  ok(previewed.target.ref:match(":track:4$"), previewed.target.ref)
  -- the host's old pairing: Kumi's old ref with the preview's (newer) layout, so the ref is taken as it is: Hat
  local wrong = call(peer, "change_apply", { kind = "track-volume", input = input, fence = previewed.fence, desc = "Kumi: v (kr12)",
    epoch = previewed.epoch, layout = previewed.layout })
  failure(wrong, "changed")
  ok(wrong.error.message:find("\226\128\156Bass\226\128\157 isn't at that place any more", 1, true), "it names the track that was previewed: " .. wrong.error.message)
  ok(not wrong.error.message:find("Hat", 1, true), "and not the one at its place now: " .. wrong.error.message)
  eq(mock.data.tracks[3].post_volume or 1, 1, "Hat was not changed")
  eq(mock.data.tracks[4].post_volume or 1, 1)
  eq(steps(), 0)
  -- paired as Kumi asked (its own layout), the ref is carried to Bass
  local right = call(peer, "change_apply", { kind = "track-volume", input = input, fence = previewed.fence, desc = "Kumi: v (kr13)",
    epoch = epoch, layout = layout })
  ok(right.result, json.encode(right.error or {}))
  ok(math.abs((mock.data.tracks[4].post_volume or 1) - 10 ^ (-6 / 20)) < 1e-6, "Bass")
  eq(mock.data.tracks[3].post_volume or 1, 1)
  eq(right.result.target.ref, track_ref(right.result.epoch, 4), "the answer names the track it located, in its own layout")
  H.no_dialogs()
end)

test("review M-3: notes written under the wrong pairing go to no other track", function()
  local env, peer = setup()
  local epoch, layout = place(peer)
  local input = {
    pattern = pattern_ref(epoch, 1), track = track_ref(epoch, 3),
    lines = { { line = 0, notes = { { col = 1, note = "C-4", instrument = "00" } } } },
  }
  mock.ops.insert_track(1, { name = "New" })
  H.tick(2)
  local previewed = call(peer, "change_preview", { kind = "notes-write", input = input, epoch = epoch, layout = layout }).result
  ok(previewed, "previewed")
  local wrong = call(peer, "change_apply", { kind = "notes-write", input = input, fence = previewed.fence, desc = "Kumi: n (kr14)",
    epoch = previewed.epoch, layout = previewed.layout })
  failure(wrong, "changed")
  ok(wrong.error.message:find("\226\128\156Bass\226\128\157 isn't at that place any more", 1, true), "it names the track that was previewed: " .. wrong.error.message)
  ok(not wrong.error.message:find("Hat", 1, true), "and not the one at its place now: " .. wrong.error.message)
  ok(mock.song:pattern(1):track(3):line(1).is_empty, "Hat's line is empty")
  ok(mock.song:pattern(1):track(4):line(1).is_empty, "and so is Bass's: nothing was written")
  eq(steps(), 0)
  local right = call(peer, "change_apply", { kind = "notes-write", input = input, fence = previewed.fence, desc = "Kumi: n (kr15)",
    epoch = epoch, layout = layout })
  ok(right.result, json.encode(right.error or {}))
  ok(not mock.song:pattern(1):track(4):line(1).is_empty, "Bass got the note")
  ok(right.result.target.ref:match(":ptrack:1%.4$"), json.encode(right.result.target))
  H.no_dialogs()
end)

test("review M-3: a track delete under the wrong pairing is refused naming the track that was previewed", function()
  local env, peer = setup()
  local epoch, layout = place(peer)
  local input = { ref = track_ref(epoch, 3) } -- Bass, in Kumi's book
  mock.ops.insert_track(1, { name = "New" })
  H.tick(2)
  local previewed = call(peer, "change_preview", { kind = "track-delete", input = input, epoch = epoch, layout = layout }).result
  ok(previewed, "previewed")
  local wrong = call(peer, "change_apply", { kind = "track-delete", input = input, fence = previewed.fence, desc = "Kumi: d (kr54)",
    epoch = previewed.epoch, layout = previewed.layout })
  failure(wrong, "changed")
  ok(wrong.error.message:find("\226\128\156Bass\226\128\157 isn't at that place any more", 1, true), wrong.error.message)
  eq(names(), "New,Kick,Hat,Bass,Pad,Master,Send 1", "nothing was deleted")
  H.no_dialogs()
end)

test("review M-3: the apply answer of a move names what it located", function()
  local env, peer = setup()
  local epoch, layout = place(peer)
  local move = { ref = track_ref(epoch, 1), index = 3 }
  local moved = apply(peer, "track-move", move, "Kumi: m (kr16)", preview(peer, "track-move", move).fence, epoch, layout)
  ok(moved.result, json.encode(moved.error or {}))
  ok(moved.result.target, "a target")
  eq(moved.result.target.ref, track_ref(moved.result.epoch, 3))
  H.no_dialogs()
end)

-- M-5: the mixer device's automation

local function automation_points(t)
  local points = 0
  for _, pattern in ipairs(mock.data.patterns) do
    local pt = pattern.tracks[t]
    for _, entry in ipairs(pt and pt.automation or {}) do points = points + #entry.points end
  end
  return points
end

test("review M-5: a fade on a track's own volume and panning comes back with the track, also in a pattern with no lines", function()
  local env, peer = setup()
  mock.ops.automate(1, 3, 1, 1, { { time = 1, value = 0, scaling = 0 }, { time = 17, value = 1, scaling = 0 } })
  mock.ops.automate(3, 3, 1, 2, { { time = 1, value = 0.2, scaling = 0 }, { time = 9, value = 0.8, scaling = 0 }, { time = 20, value = 0.5, scaling = 0 } })
  fill(3, 1, 4) -- lines in pattern 1 only: the fade in pattern 3 is in a pattern with none
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  eq(automation_points(3), 5)
  local previewed, done = delete_track(peer, 3)
  eq(#done.material.automation, 2, "both fades are kept")
  eq(automation_points(3), 0)
  local undone = restore(peer, "track-delete", record_of(previewed, done), "Kumi: undo d (kr17)", done.epoch, done.layout)
  ok(undone.result, json.encode(undone.error or {}))
  eq(automation_points(3), 5, "the volume and panning fades are back")
  H.no_dialogs()
end)

-- M-6: slot records hold epoch-categorised pattern refs, not bare numbers

local function seq_text() return table.concat(mock.data.sequence, ",") end

-- What Renoise's "delete unused patterns" does to the pool: the pattern goes, those after it move down one, and the sequence's
-- numbers follow.
local function delete_unused(p)
  for s, value in ipairs(mock.data.sequence) do
    if value > p then mock.data.sequence[s] = value - 1 end
  end
  mock.ops.remove_pattern(p)
  H.tick(2)
end

test("review M-6: a deleted slot, the pool renumbered by the producer, Kumi's undo: stale, not another pattern", function()
  local env, peer = setup()
  mock.data.patterns[2].name, mock.data.patterns[3].name = "Verse", "Drop"
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  local epoch, layout = place(peer)
  local previewed = preview(peer, "slot-delete", { ref = seq_ref(epoch, 2) })
  local done = apply(peer, "slot-delete", { ref = seq_ref(epoch, 2) }, "Kumi: delete slot 2 (kr18)", previewed.fence, epoch, layout)
  ok(done.result, json.encode(done.error or {}))
  delete_unused(2)
  local before = steps()
  local now, now_layout = place(peer)
  local undone = restore(peer, "slot-delete", record_of(previewed, done.result), "Kumi: undo delete slot 2 (kr19)", now, now_layout)
  failure(undone, "stale")
  eq(steps(), before, "nothing was written")
  H.no_dialogs()
end)

test("review M-6: a slot's pattern changed, the pool renumbered, Kumi's undo: stale", function()
  local env, peer = setup(nil, { patterns = 4, sequence = { 1, 3, 3, 4 }, sequence_length = 4 })
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  local epoch, layout = place(peer)
  local input = { ref = seq_ref(epoch, 2), pattern = pattern_ref(epoch, 1) }
  local previewed = preview(peer, "slot-pattern", input)
  local done = apply(peer, "slot-pattern", input, "Kumi: slot 2 plays 00 (kr20)", previewed.fence, epoch, layout)
  ok(done.result, json.encode(done.error or {}))
  eq(seq_text(), "1,1,3,4")
  delete_unused(2)
  local before = steps()
  local now, now_layout = place(peer)
  failure(restore(peer, "slot-pattern", record_of(previewed, done.result), "Kumi: undo (kr21)", now, now_layout), "stale")
  eq(steps(), before)
  H.no_dialogs()
end)

-- S-2: the delay column

local function write_notes(peer, cells, desc)
  local epoch, layout = place(peer)
  local input = { pattern = pattern_ref(epoch, 1), track = track_ref(epoch, 3), lines = { { line = 0, notes = cells } } }
  local previewed = call(peer, "change_preview", { kind = "notes-write", input = input, epoch = epoch, layout = layout })
  if not previewed.result then return previewed end
  return apply(peer, "notes-write", input, desc, previewed.result.fence, epoch, layout)
end

test("review S-2: delay 00 is no delay and FF is 255", function()
  local env, peer = setup()
  local column = function(c) return mock.song:pattern(1):track(3):line(1):note_column(c) end
  local none = write_notes(peer, { { col = 1, note = "C-4", instrument = "00", delay = "00" } }, "Kumi: n (kr22)")
  ok(none.result, json.encode(none.error or {}))
  eq(column(1).delay_value, 0)
  eq(column(1).delay_string, "..")
  local late = write_notes(peer, { { col = 2, note = "D-4", instrument = "00", delay = "FF" } }, "Kumi: n (kr23)")
  ok(late.result, json.encode(late.error or {}))
  eq(column(2).delay_value, 255)
  local number = write_notes(peer, { { col = 1, note = "E-4", instrument = "00", delay = 128 } }, "Kumi: n (kr24)")
  ok(number.result, json.encode(number.error or {}))
  eq(column(1).delay_value, 128)
  H.no_dialogs()
end)

-- N-5: letter commands, as Renoise reads them

test("review N-5: letter commands in the volume, panning and effect columns are written, read back, put back and kept by a delete", function()
  local env, peer = setup()
  local epoch, layout = place(peer)
  local input = {
    pattern = pattern_ref(epoch, 1), track = track_ref(epoch, 3),
    lines = { { line = 0, notes = { { col = 1, note = "C-4", instrument = "00", volume = "G3", panning = "U4", fx = "ZB10" } } } },
  }
  local previewed = call(peer, "change_preview", { kind = "notes-write", input = input, epoch = epoch, layout = layout })
  ok(previewed.result, json.encode(previewed.error or {}))
  local done = apply(peer, "notes-write", input, "Kumi: n (kr34)", previewed.result.fence, epoch, layout)
  ok(done.result, json.encode(done.error or {}))
  local column = mock.song:pattern(1):track(3):line(1):note_column(1)
  eq(column.volume_string, "G3")
  eq(column.panning_string, "U4")
  eq(column.effect_number_string, "ZB")
  -- a delete keeps them (as numbers) and puts them back
  local deleted, gone = delete_track(peer, 3, "Kumi: d (kr35)")
  local undone = restore(peer, "track-delete", record_of(deleted, gone), "Kumi: undo d (kr36)", gone.epoch, gone.layout)
  ok(undone.result, json.encode(undone.error or {}))
  column = mock.song:pattern(1):track(3):line(1):note_column(1)
  eq(column.volume_string, "G3")
  eq(column.effect_number_string, "ZB")
  -- the notes restore refuses now (the track Kumi put back wrote the pattern since), and one made again and undone at once
  -- puts the empty cell back
  failure(restore(peer, "notes-write", { target = done.result.target, prior = previewed.result.prior, applied = done.result.applied },
    "Kumi: undo n (kr37)", undone.result.epoch, undone.result.layout), "changed")
  epoch, layout = place(peer)
  input.pattern, input.track = pattern_ref(epoch, 1), track_ref(epoch, 3)
  input.lines[1].line = 1
  previewed = call(peer, "change_preview", { kind = "notes-write", input = input, epoch = epoch, layout = layout })
  done = apply(peer, "notes-write", input, "Kumi: n (kr38)", previewed.result.fence, epoch, layout)
  ok(done.result, json.encode(done.error or {}))
  local back = restore(peer, "notes-write", { target = done.result.target, prior = previewed.result.prior, applied = done.result.applied },
    "Kumi: undo n (kr39)", done.result.epoch, done.result.layout)
  ok(back.result, json.encode(back.error or {}))
  eq(mock.song:pattern(1):track(3):line(2):note_column(1).volume_string, "..")
  H.no_dialogs()
end)

-- M-1's sibling: the other sliced restore, of lines

test("review M-1: a sliced notes restore stops when the producer makes a track between the steps, writes nothing to the track that took the place, and says how far it got", function()
  local env, peer = setup({ patch = constants({ SLICE_WRITES = 30 }) })
  local epoch, layout = place(peer)
  local lines = {}
  for l = 1, 12 do lines[l] = { line = l - 1, notes = { { col = 1, note = "C-3", instrument = "01" } } } end
  local input = { pattern = pattern_ref(epoch, 1), track = track_ref(epoch, 2), lines = lines }
  local previewed = preview(peer, "notes-write", input)
  local done = apply(peer, "notes-write", input, "Kumi: many (kr38)", previewed.fence, epoch, layout)
  ok(done.result, json.encode(done.error or {}))
  local function cell(t, l) return mock.song:pattern(1):track(t):line(l):note_column(1).note_string end
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  local id = send(peer, "change_restore", restore_params("notes-write", { target = previewed.target, prior = previewed.prior, applied = done.result.applied },
    "Kumi: undo many (kr39)", done.result.epoch, done.result.layout))
  for _ = 1, 50 do
    mock.advance(10)
    if steps() > 0 then break end
  end
  eq(steps(), 1, "step 1 is written")
  mock.ops.insert_track(1, { name = "New" })
  local response = wait(peer, id, 300, 20)
  -- (lines half put back: neither as Kumi made them nor as they were)
  failure(response, "uncertain")
  ok(response.error.message:find("Kumi stopped part way and wrote nothing further", 1, true), response.error.message)
  ok(response.error.message:find("Renoise's own undo", 1, true), response.error.message)
  eq(steps(), 1, "no step after the insert")
  eq(cell(3, 1), "---", "Hat's first lines are clear (step 1)")
  eq(cell(3, 12), "C-3", "and its last are as Kumi left them: the rest waits")
  eq(cell(2, 5), "---", "the track that took Hat's place was not written to")
  H.no_dialogs()
end)

-- S-5 (lab): Renoise names the tracks it numbers itself by their place, so a send put back is not "in the song again"

test("review S-5: a deleted send called S01 comes back though the send after it is called S01 now (Renoise numbers default names)", function()
  local env, peer = setup(nil, { send_tracks = 2 })
  mock.ops.rename_track(6, "S01")
  mock.ops.rename_track(7, "S02")
  H.tick(2)
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  local previewed, done = delete_track(peer, 6)
  mock.ops.rename_track(6, "S01") -- (what was S02 is the first send now)
  H.tick(2)
  local undone = restore(peer, "track-delete", record_of(previewed, done), "Kumi: undo d (kr40)", done.epoch, done.layout)
  ok(undone.result, json.encode(undone.error or {}))
  eq(#mock.data.tracks, 7)
  H.no_dialogs()
end)

test("review S-5: a deleted track called Track 03 with lines comes back though the track after it is called Track 03 now", function()
  local env, peer = setup()
  mock.ops.rename_track(3, "Track 03")
  mock.ops.rename_track(4, "Track 04")
  fill(3, 1, 4)
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  local previewed, done = delete_track(peer, 3)
  mock.ops.rename_track(3, "Track 03") -- (Renoise renumbers what was Track 04)
  H.tick(2)
  local undone = restore(peer, "track-delete", record_of(previewed, done), "Kumi: undo d (kr42)", done.epoch, done.layout)
  ok(undone.result, json.encode(undone.error or {}))
  eq(#mock.data.tracks, 6)
  -- asked again, it is there: with the lines it had
  failure(restore(peer, "track-delete", record_of(previewed, done), "Kumi: undo d (kr43)", undone.result.epoch, undone.result.layout), "changed")
  H.no_dialogs()
end)

test("review S-4: a group called Group 04 is found again when a track made before it renumbered it", function()
  local env, peer = setup()
  mock.song:insert_group_at(5)
  mock.song:add_track_to_group(3, 5)
  mock.song:add_track_to_group(3, 5)
  mock.ops.rename_track(5, "Group 05")
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  local previewed, done = delete_track(peer, 3)
  mock.ops.insert_track(1, { name = "New" })
  mock.ops.rename_track(5, "Group 05") -- (Renoise renumbers: the group is at 5 again after the insert, was at 4 after the delete)
  H.tick(2)
  local undone = restore(peer, "track-delete", record_of(previewed, done), "Kumi: undo d (kr41)", done.epoch, done.layout)
  ok(undone.result, json.encode(undone.error or {}))
  ok(not undone.result.display.note, "it is a member again: " .. json.encode(undone.result.display))
  H.no_dialogs()
end)

-- S-5 (lab): routing aimed at what is deleted. Renoise clears it when the target goes and doesn't set it again when it comes back.

local function routing_catalog()
  mock.device_catalog = {
    ["Audio/Effects/Native/#Send"] = {
      { name = "Amount", value = 1, min = 0, max = 1.4 }, { name = "Panning", value = 0.5, min = 0, max = 1 },
      { name = "Receiver", value = 0, min = -1, max = 8 },
    },
    ["Audio/Effects/Native/*LFO"] = {
      { name = "Dest. Track", value = -1, min = -1, max = 16 }, { name = "Dest. Effect", value = -1, min = -1, max = 16 },
      { name = "Dest. Parameter", value = -1, min = -1, max = 16 },
    },
    ["Audio/Effects/Native/Gainer"] = { { name = "Gain", value = 1, min = 0, max = 4 } },
  }
end

local function receiver(t, d) return mock.data.tracks[t].devices[d].params[3].value end
local function dest(t, d, k) return mock.data.tracks[t].devices[d].params[k].value end

test("review S-5: the sends that fed a deleted send track are fed by it again when it is put back", function()
  routing_catalog()
  local env, peer = setup(nil, { send_tracks = 2 })
  for t = 1, 3 do mock.song:track(t):insert_device_at("Audio/Effects/Native/#Send", 2) end
  mock.data.tracks[1].devices[2].params[3].value = 0
  mock.data.tracks[2].devices[2].params[3].value = 0
  mock.data.tracks[3].devices[2].params[3].value = 1
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  local previewed, done = delete_track(peer, 6) -- the first send
  eq(receiver(1, 2), -1, "(Renoise lets go of what was aimed at it)")
  eq(receiver(3, 2), 0, "(and renumbers the other send)")
  ok(done.display.note and done.display.note:find("2 routings", 1, true), json.encode(done.display))
  local undone = restore(peer, "track-delete", record_of(previewed, done), "Kumi: undo d (kr44)", done.epoch, done.layout)
  ok(undone.result, json.encode(undone.error or {}))
  eq(receiver(1, 2), 0)
  eq(receiver(2, 2), 0)
  eq(receiver(3, 2), 1, "Renoise renumbers the other again")
  ok(undone.result.display.note and undone.result.display.note:find("2 routings aimed at it set again", 1, true), json.encode(undone.result.display))
  H.no_dialogs()
end)

test("review S-5: a send the producer re-aimed since is not aimed again", function()
  routing_catalog()
  local env, peer = setup(nil, { send_tracks = 2 })
  for t = 1, 2 do mock.song:track(t):insert_device_at("Audio/Effects/Native/#Send", 2) end
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  local previewed, done = delete_track(peer, 6)
  mock.data.tracks[2].devices[2].params[3].value = 0 -- the producer aims Hat at the send that is left
  local undone = restore(peer, "track-delete", record_of(previewed, done), "Kumi: undo d (kr45)", done.epoch, done.layout)
  ok(undone.result, json.encode(undone.error or {}))
  eq(receiver(1, 2), 0, "Kick is fed by the send again")
  eq(receiver(2, 2), 1, "Hat stays aimed at the send that was left, which is the second send again")
  H.no_dialogs()
end)

test("review S-5: a meta device aimed at a deleted track is aimed at it again, with its effect and parameter", function()
  routing_catalog()
  local env, peer = setup()
  mock.song:track(2):insert_device_at("Audio/Effects/Native/*LFO", 2)
  mock.data.tracks[2].devices[2].params[1].value = 0 -- track 1
  mock.data.tracks[2].devices[2].params[2].value = 1
  mock.data.tracks[2].devices[2].params[3].value = 0
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  local previewed, done = delete_track(peer, 1)
  eq(dest(1, 2, 1), -1, "(Hat is track 1 now, its LFO let go)")
  eq(dest(1, 2, 2), -1)
  local undone = restore(peer, "track-delete", record_of(previewed, done), "Kumi: undo d (kr46)", done.epoch, done.layout)
  ok(undone.result, json.encode(undone.error or {}))
  eq(dest(2, 2, 1), 0)
  eq(dest(2, 2, 2), 1)
  eq(dest(2, 2, 3), 0)
  H.no_dialogs()
end)

test("review S-5: a deleted device that a meta device was aimed at is aimed at again when it is put back", function()
  routing_catalog()
  local env, peer = setup()
  local bass = mock.song:track(3)
  bass:insert_device_at("Audio/Effects/Native/Gainer", 2)
  bass:insert_device_at("Audio/Effects/Native/Gainer", 3)
  bass:insert_device_at("Audio/Effects/Native/*LFO", 4)
  mock.data.tracks[3].devices[4].params[1].value = 2 -- track 3 (Bass)
  mock.data.tracks[3].devices[4].params[2].value = 2 -- device 3
  mock.data.tracks[3].devices[4].params[3].value = 0
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  local epoch, layout = place(peer)
  local previewed = preview(peer, "device-delete", { ref = epoch .. ":device:3.3" })
  local done = apply(peer, "device-delete", { ref = epoch .. ":device:3.3" }, "Kumi: dd (kr47)", previewed.fence, epoch, layout)
  ok(done.result, json.encode(done.error or {}))
  ok(done.result.display.note and done.result.display.note:find("1 routing aimed at it (sends", 1, true), json.encode(done.result.display))
  eq(dest(3, 3, 2), -1, "(the LFO is device 3 now and let go)")
  local undone = restore(peer, "device-delete", record_of(previewed, done.result), "Kumi: undo dd (kr48)", done.result.epoch, done.result.layout)
  ok(undone.result, json.encode(undone.error or {}))
  eq(dest(3, 4, 1), 2)
  eq(dest(3, 4, 2), 2)
  eq(dest(3, 4, 3), 0)
  H.no_dialogs()
end)

test("review S-5: a meta device put back with its track, or by itself, sends its values where it did", function()
  routing_catalog()
  local env, peer = setup()
  -- Bass holds a Gainer and an LFO aimed at it; Pad holds an LFO aimed at the Gainer on Bass
  local bass, pad = mock.song:track(3), mock.song:track(4)
  bass:insert_device_at("Audio/Effects/Native/Gainer", 2)
  bass:insert_device_at("Audio/Effects/Native/*LFO", 3)
  pad:insert_device_at("Audio/Effects/Native/*LFO", 2)
  local own = mock.data.tracks[3].devices[3].params
  own[1].value, own[2].value, own[3].value = 2, 1, 0
  local other = mock.data.tracks[4].devices[2].params
  other[1].value, other[2].value, other[3].value = 2, 1, 0
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  -- the LFO by itself
  local epoch, layout = place(peer)
  local previewed = preview(peer, "device-delete", { ref = epoch .. ":device:3.3" })
  local done = apply(peer, "device-delete", { ref = epoch .. ":device:3.3" }, "Kumi: dd (kr49)", previewed.fence, epoch, layout)
  ok(done.result, json.encode(done.error or {}))
  local undone = restore(peer, "device-delete", record_of(previewed, done.result), "Kumi: undo dd (kr50)", done.result.epoch, done.result.layout)
  ok(undone.result, json.encode(undone.error or {}))
  local params = mock.data.tracks[3].devices[3].params
  eq(params[1].value, 2, "its track")
  eq(params[2].value, 1, "its effect")
  eq(params[3].value, 0, "its parameter")
  -- the whole track, with the LFO on it and the other LFO aimed at its Gainer
  local deleted, gone = delete_track(peer, 3, "Kumi: d (kr51)")
  eq(mock.data.tracks[3].devices[2].params[1].value, -1, "(Renoise lets go of the Pad's LFO)")
  local back = restore(peer, "track-delete", record_of(deleted, gone), "Kumi: undo d (kr52)", gone.epoch, gone.layout)
  ok(back.result, json.encode(back.error or {}))
  params = mock.data.tracks[3].devices[3].params
  eq(params[1].value, 2, "the LFO on the track aims at the track again")
  eq(params[2].value, 1)
  params = mock.data.tracks[4].devices[2].params
  eq(params[1].value, 2, "so does the one on the next track")
  eq(params[2].value, 1)
  H.no_dialogs()
end)

-- S-3 / S-4: places that follow the tracks made since

test("review S-3: a move put back after the producer made a track before both places", function()
  local env, peer = setup()
  local epoch, layout = place(peer)
  local move = { ref = track_ref(epoch, 1), index = 3 }
  local previewed = preview(peer, "track-move", move)
  local done = apply(peer, "track-move", move, "Kumi: m (kr25)", previewed.fence, epoch, layout)
  ok(done.result, json.encode(done.error or {}))
  eq(names(), "Hat,Bass,Kick,Pad,Master,Send 1")
  mock.ops.insert_track(1, { name = "New" })
  H.tick(2)
  local undone = restore(peer, "track-move", record_of(previewed, done.result), "Kumi: undo m (kr26)", done.result.epoch, done.result.layout)
  ok(undone.result, json.encode(undone.error or {}))
  eq(names(), "New,Kick,Hat,Bass,Pad,Master,Send 1", "Kick is back among the tracks it was moved among")
  H.no_dialogs()
end)

test("review S-4: a deleted group member goes back into its group after a track was made before the group", function()
  local env, peer = setup()
  mock.song:insert_group_at(5)
  mock.song:add_track_to_group(3, 5)
  mock.song:add_track_to_group(3, 5)
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  local before = order()
  local previewed, done = delete_track(peer, 3)
  mock.ops.insert_track(1, { name = "New" })
  H.tick(2)
  local undone = restore(peer, "track-delete", record_of(previewed, done), "Kumi: undo d (kr27)", done.epoch, done.layout)
  ok(undone.result, json.encode(undone.error or {}))
  eq(order(), "New," .. before)
  H.no_dialogs()
end)

test("review S-4: when the group is gone the track comes back outside it, and the answer says so", function()
  local env, peer = setup()
  mock.song:insert_group_at(5)
  mock.song:add_track_to_group(3, 5)
  mock.song:add_track_to_group(3, 5)
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  local previewed, done = delete_track(peer, 3)
  mock.ops.remove_track(4) -- the producer deletes the group (Pad is track 3 now, the group is track 4)
  H.tick(2)
  local undone = restore(peer, "track-delete", record_of(previewed, done), "Kumi: undo d (kr28)", done.epoch, done.layout)
  ok(undone.result, json.encode(undone.error or {}))
  ok(undone.result.display.note and undone.result.display.note:find("isn't there any more", 1, true), json.encode(undone.result.display))
  H.no_dialogs()
end)

test("review S-3: a track put in a group is taken out again after the producer made a track before both places", function()
  local env, peer = setup()
  mock.song:insert_group_at(4)
  mock.song:track(4).name = "Bus"
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  local epoch, layout = place(peer)
  local input = { ref = track_ref(epoch, 3), group = track_ref(epoch, 4) }
  local previewed = preview(peer, "track-group", input)
  local done = apply(peer, "track-group", input, "Kumi: g (kr30)", previewed.fence, epoch, layout)
  ok(done.result, json.encode(done.error or {}))
  eq(order(), "Kick,Hat,Bass(in),Bus[G],Pad,Master,Send 1")
  mock.ops.insert_track(1, { name = "New" })
  H.tick(2)
  local undone = restore(peer, "track-group", record_of(previewed, done.result), "Kumi: undo g (kr31)", done.result.epoch, done.result.layout)
  ok(undone.result, json.encode(undone.error or {}))
  eq(order(), "New,Kick,Hat,Bass,Bus[G],Pad,Master,Send 1")
  H.no_dialogs()
end)

-- S-6: the loop range names slots

test("review S-6: a loop range put back after the sequence changed is refused: its slots are not those slots any more", function()
  local env, peer = setup()
  local epoch, layout = place(peer)
  local input = { first = 2, last = 3 }
  local previewed = preview(peer, "loop-range", input)
  local done = apply(peer, "loop-range", input, "Kumi: loop (kr32)", previewed.fence, epoch, layout)
  ok(done.result, json.encode(done.error or {}))
  mock.ops.insert_sequence_slot(1, 1) -- the producer makes a slot before them
  H.tick(2)
  mock.song.transport.loop_sequence_range = { 3, 4 } -- (Renoise moved the range along with the slots)
  local now, now_layout = place(peer)
  failure(restore(peer, "loop-range", record_of(previewed, done.result), "Kumi: undo (kr33)", now, now_layout), "stale")
  eq(mock.song.transport.loop_sequence_range[1], 3, "the loop is as the producer has it")
  H.no_dialogs()
end)

-- N-6: the journal's move places follow the tracks made and deleted

test("review N-6: the two places a move keeps move with the tracks made and deleted since", function()
  local base = T.load_payload_module("changes_base")
  local journal = base.journal_new()
  base.remember(journal, { desc = "Kumi: m", kind = "track-move", place = { track = 3 }, prior = { index = 1 }, applied = { index = 3 }, follows = "track", state = "applied" })
  base.journal_track_event(journal, "insert", 1, false)
  local entry = journal.entries[1]
  eq(entry.prior.index, 2, "the place it came from")
  eq(entry.applied.index, 4, "the place it went to")
  eq(entry.place.track, 4)
  base.journal_track_event(journal, "remove", 2, false) -- the track at 2 is deleted
  eq(entry.prior.index, 2, "(the place of the deleted track is its own)")
  eq(entry.applied.index, 3)
  eq(entry.place.track, 3)
end)

-- The reviewer's verify of 919993ab (probe_v919.lua): places followed through swaps, names that are no identity, routes as places

local function dest(t, d)
  local p = mock.data.tracks[t].devices[d].params
  return string.format("%d/%d/%d", p[1].value, p[2].value, p[3].value)
end

local function lines_on(t)
  local n = 0
  for p = 1, #mock.data.patterns do
    if not mock.song:pattern(p):track(t).is_empty then n = n + 1 end
  end
  return n
end

test("review V1: the producer swaps two default-named tracks during a sliced restore: the producer's track and lines stay", function()
  local env, peer = setup({ patch = constants({ SLICE_WRITES = 40 }) })
  mock.ops.rename_track(2, "Track 02")
  mock.ops.rename_track(3, "Track 03")
  fill(2)
  for p = 1, 3 do -- Track 03 holds the producer's notes
    local column = mock.song:pattern(p):track(3):line(1):note_column(1)
    column.note_value, column.instrument_value = 60, 1
  end
  H.tick(2)
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  local previewed, done = delete_track(peer, 2)
  local id = start_restore(peer, previewed, done, "Kumi: undo d (kr60)")
  mock.song:swap_tracks_at(2, 3) -- the producer moves Track 03 up one
  local response = wait(peer, id, 400, 20)
  ok(response and response.error, "the restore stops")
  local kept = false
  for i, t in ipairs(mock.data.tracks) do
    if t.name == "Track 03" and lines_on(i) == 3 then kept = true end
  end
  ok(kept, "the producer's Track 03 with its lines is in the song: " .. names())
  eq(#mock.data.tracks, 5, "and the half-made track is the one that went (Hat is deleted, the restore gone)")
  H.no_dialogs()
end)

test("review V1b: a swap by the restore's own group join does not lose the made track", function()
  local env, peer = setup({ patch = constants({ SLICE_WRITES = 40 }) })
  mock.song:insert_group_at(5)
  mock.song:add_track_to_group(3, 5)
  mock.song:add_track_to_group(3, 5)
  fill(3)
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  local before = order()
  local previewed, done = delete_track(peer, 3)
  local undone = call(peer, "change_restore", restore_params("track-delete", record_of(previewed, done), "Kumi: undo d (kr61)", done.epoch, done.layout), 600)
  ok(undone and undone.result, json.encode(undone and undone.error or "no answer"))
  eq(order(), before)
  H.no_dialogs()
end)

test("review V3: a delete that could not be put back in 16 steps is refused where it is made", function()
  local sequence = {}
  for i = 1, 20 do sequence[i] = i end
  local env, peer = setup({ patch = constants({ SLICE_WRITES = 100 }) }, { patterns = 20, sequence = sequence, sequence_length = 20 })
  for p = 1, 20 do
    for k = 1, 8 do -- 8 notes a pattern: 56 writes, more than half a slice
      local column = mock.song:pattern(p):track(2):line(k):note_column(1)
      column.note_value, column.instrument_value = 40 + k, 1
    end
  end
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  local epoch, layout = place(peer)
  local previewed = preview(peer, "track-delete", { ref = track_ref(epoch, 2) })
  local done = apply(peer, "track-delete", { ref = track_ref(epoch, 2) }, "Kumi: d (kr62)", previewed.fence, epoch, layout)
  failure(done, "failed")
  ok(done.error.message:find("20 steps", 1, true), done.error.message)
  eq(names(), "Kick,Hat,Bass,Pad,Master,Send 1", "nothing was deleted")
  H.no_dialogs()
end)

test("review V4: a meta device aimed at a deleted track: the producer adds another *LFO before it, and Kumi's undo aims the old one", function()
  routing_catalog()
  local env, peer = setup()
  mock.song:track(3):insert_device_at("Audio/Effects/Native/Gainer", 2)
  mock.song:track(1):insert_device_at("Audio/Effects/Native/*LFO", 2)
  local p = mock.data.tracks[1].devices[2].params
  p[1].value, p[2].value, p[3].value = 2, 1, 0 -- Kick's LFO drives Bass's Gainer
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  local previewed, done = delete_track(peer, 3)
  mock.song:track(1):insert_device_at("Audio/Effects/Native/*LFO", 2) -- the producer's own new LFO, aimed nowhere
  local undone = restore(peer, "track-delete", record_of(previewed, done), "Kumi: undo d (kr63)", done.epoch, done.layout)
  ok(undone.result, json.encode(undone.error or {}))
  eq(dest(1, 2), "-1/-1/-1", "the producer's new LFO was left alone")
  eq(dest(1, 3), "2/1/0", "and the old one, followed to its new place, drives Bass's Gainer again")
  ok(undone.result.display.note and undone.result.display.note:find("1 routing aimed at it set again", 1, true), json.encode(undone.result.display))
  H.no_dialogs()
end)

local function lfo_on_deleted_track(names_given)
  routing_catalog()
  local env, peer = setup()
  if names_given then
    for i, name in ipairs(names_given) do mock.ops.rename_track(i, name) end
    H.tick(2)
  end
  mock.song:track(4):insert_device_at("Audio/Effects/Native/Gainer", 2)
  mock.song:track(3):insert_device_at("Audio/Effects/Native/*LFO", 2)
  local p = mock.data.tracks[3].devices[2].params
  p[1].value, p[2].value, p[3].value = 3, 1, 0 -- the 3rd track's LFO drives the 4th track's Gainer
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  local previewed, done = delete_track(peer, 3)
  mock.ops.insert_track(1, { name = "New" })
  H.tick(2)
  local undone = restore(peer, "track-delete", record_of(previewed, done), "Kumi: undo d (kr64)", done.epoch, done.layout)
  ok(undone.result, json.encode(undone.error or {}))
  return dest(4, 2)
end

test("review V8: a deleted track's LFO drives a Gainer on the next track; the producer makes a track at 1; the undo follows", function()
  eq(lfo_on_deleted_track(nil), "4/1/0", "the LFO's destination (the Gainer's track is 5 now)")
end)

test("review V8b: the same with the default names Renoise gives", function()
  eq(lfo_on_deleted_track({ "Track 01", "Track 02", "Track 03", "Track 04" }), "4/1/0", "the LFO's destination")
end)

local function lfo_aimed_at_bass(names_given)
  routing_catalog()
  local env, peer = setup()
  if names_given then
    for i, name in ipairs(names_given) do mock.ops.rename_track(i, name) end
    H.tick(2)
  end
  mock.song:track(3):insert_device_at("Audio/Effects/Native/Gainer", 2)
  mock.song:track(1):insert_device_at("Audio/Effects/Native/*LFO", 2)
  local p = mock.data.tracks[1].devices[2].params
  p[1].value, p[2].value, p[3].value = 2, 1, 0 -- the first track's LFO drives the third track's Gainer
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  local epoch, layout = place(peer)
  local previewed = preview(peer, "device-delete", { ref = epoch .. ":device:1.2" })
  local done = apply(peer, "device-delete", { ref = epoch .. ":device:1.2" }, "Kumi: dd (kr65)", previewed.fence, epoch, layout)
  ok(done.result, json.encode(done.error or {}))
  mock.ops.insert_track(3, { name = names_given and "Track 03" or "New" }) -- the producer makes a track just before the Gainer's
  H.tick(2)
  local undone = restore(peer, "device-delete", record_of(previewed, done.result), "Kumi: undo dd (kr66)", done.result.epoch, done.result.layout)
  ok(undone.result, json.encode(undone.error or {}))
  return dest(1, 2)
end

test("review V7b: a deleted LFO aimed at the 3rd track; the producer makes a track at 3; the undo aims the track it was", function()
  eq(lfo_aimed_at_bass(nil), "3/1/0", "the LFO's destination (the Gainer's track is 4 now)")
end)

test("review V7c: the same with default names", function()
  eq(lfo_aimed_at_bass({ "Track 01", "Track 02", "Track 03", "Track 04" }), "3/1/0", "the LFO's destination")
end)

test("review V7: a deleted LFO aimed at its own track; the producer deletes the track before; the undo is not refused", function()
  routing_catalog()
  local env, peer = setup()
  local bass = mock.song:track(3)
  bass:insert_device_at("Audio/Effects/Native/Gainer", 2)
  bass:insert_device_at("Audio/Effects/Native/*LFO", 3)
  local p = mock.data.tracks[3].devices[3].params
  p[1].value, p[2].value, p[3].value = 2, 1, 0 -- its own track (0-based 2), the Gainer
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  local epoch, layout = place(peer)
  local previewed = preview(peer, "device-delete", { ref = epoch .. ":device:3.3" })
  local done = apply(peer, "device-delete", { ref = epoch .. ":device:3.3" }, "Kumi: dd (kr67)", previewed.fence, epoch, layout)
  ok(done.result, json.encode(done.error or {}))
  mock.ops.remove_track(2) -- the producer deletes Hat, the track before Bass
  H.tick(2)
  local undone = restore(peer, "device-delete", record_of(previewed, done.result), "Kumi: undo dd (kr68)", done.result.epoch, done.result.layout)
  ok(undone.result, json.encode(undone.error or {}))
  eq(dest(2, 3), "1/1/0", "the LFO aims at its own track, which is the 2nd now")
  H.no_dialogs()
end)

local function group_member_notes(second_group_name)
  local env, peer = setup()
  local song = mock.song
  song:insert_group_at(2)
  song:add_track_to_group(1, 2) -- Kick in the first group
  song:insert_group_at(6)
  song:add_track_to_group(4, 6) -- Bass joins the second
  song:add_track_to_group(4, 6) -- Pad joins it
  mock.ops.rename_track(2, "Group 02")
  mock.ops.rename_track(6, second_group_name)
  for k = 1, 4 do
    local column = song:pattern(1):track(4):line(k):note_column(1)
    column.note_value, column.instrument_value = 36 + k, 1
  end
  H.tick(2)
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  local previewed, done = delete_track(peer, 4)
  for i = 1, 4 do mock.ops.insert_track(1, { name = "N" .. i }) end
  H.tick(2)
  local undone = restore(peer, "track-delete", record_of(previewed, done), "Kumi: undo d (kr69)", done.epoch, done.layout)
  ok(undone.result, json.encode(undone.error or {}))
  local function parent_of(name)
    for i, t in ipairs(mock.data.tracks) do
      if t.name == name then
        local parent = song:track(i).group_parent
        return parent and parent.name or "none", i
      end
    end
  end
  local function notes_of(name)
    local _, i = parent_of(name)
    local n = 0
    for k = 1, 4 do
      if song:pattern(1):track(i):line(k):note_column(1).note_value < 120 then n = n + 1 end
    end
    return n
  end
  return parent_of, notes_of
end

test("review V5b: a member of a default-named group goes back into its own group, and its lines are written to it", function()
  local parent_of, notes_of = group_member_notes("Group 06")
  eq((parent_of("Bass")), "Group 06", "Bass's group (not the earlier Group 02)")
  eq(notes_of("Bass"), 4, "Bass has its notes")
  eq(notes_of("Hat"), 0, "the producer's Hat has none")
end)

test("review V5: the same with the second group named of its own", function()
  local parent_of, notes_of = group_member_notes("Strings")
  eq((parent_of("Bass")), "Strings", "Bass's group")
  eq(notes_of("Bass"), 4)
end)

-- S-7: budgets

test("review S-7: a track with more than can be put back in time is refused where it is read, and stays", function()
  local env, peer = setup({ patch = constants({ SLICE_WRITES = 40, MAX_RESTORE_STEPS = 4 }) })
  fill(2, 3, 10) -- 30 cells: well over the 160 writes 4 steps of 40 make
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  local epoch, layout = place(peer)
  local previewed = preview(peer, "track-delete", { ref = track_ref(epoch, 2) })
  failure(apply(peer, "track-delete", { ref = track_ref(epoch, 2) }, "Kumi: d (kr29)", previewed.fence, epoch, layout), "failed")
  eq(names(), "Kick,Hat,Bass,Pad,Master,Send 1", "nothing was deleted")
  eq(steps(), 0)
  H.no_dialogs()
end)

test("review S-7: one notes-write is no more than a callback's budget", function()
  local env, peer = setup(nil, { pattern_lines = 512 })
  mock.song:track(3).visible_note_columns = 12
  local epoch, layout = place(peer)
  local function lines(first, last)
    local out = {}
    for l = first, last do
      local notes = {}
      for c = 1, 12 do notes[c] = { col = c, note = "C-4", instrument = "00" } end
      out[#out + 1] = { line = l - 1, notes = notes }
    end
    return out
  end
  local function write(first, last)
    return call(peer, "change_preview", {
      kind = "notes-write", input = { pattern = pattern_ref(epoch, 1), track = track_ref(epoch, 3), lines = lines(first, last) },
      epoch = epoch, layout = layout,
    })
  end
  local fine = write(1, 100)
  ok(fine.result, json.encode(fine.error or {}))
  local refused = write(1, 400) -- 4,800 cells
  ok(refused and refused.error, "400 lines of twelve columns are not")
  eq(refused.error.code, "bad-params")
  ok(refused.error.message:find("split", 1, true), refused.error.message)
  H.no_dialogs()
end)

-- SW1: a deleted slot's mutes follow the tracks they muted

local function muted_in(s)
  local out = {}
  for t = 1, #mock.data.tracks do
    if mock.song.sequencer:track_sequence_slot_is_muted(t, s) then out[#out + 1] = mock.data.tracks[t].name end
  end
  return table.concat(out, ",")
end

-- Mutes `muted` (track numbers) in slot 2, deletes the slot the way Kumi does, lets the producer `act`, then undoes it.
local function slot_mutes_after(muted, act)
  local env, peer = setup()
  for _, t in ipairs(muted) do mock.song.sequencer:set_track_sequence_slot_is_muted(t, 2, true) end
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  local epoch, layout = place(peer)
  local previewed = preview(peer, "slot-delete", { ref = seq_ref(epoch, 2) })
  local done = apply(peer, "slot-delete", { ref = seq_ref(epoch, 2) }, "Kumi: delete slot 2 (kr70)", previewed.fence, epoch, layout)
  ok(done.result, json.encode(done.error or {}))
  act()
  H.tick(2)
  local undone = restore(peer, "slot-delete", record_of(previewed, done.result), "Kumi: undo (kr71)", done.result.epoch, done.result.layout)
  ok(undone.result, json.encode(undone.error or {}))
  H.no_dialogs()
  return muted_in(2), undone.result.display
end

test("review SW1: the producer swaps the muted track with another: the slot comes back muting the same track", function()
  local muted, display = slot_mutes_after({ 1 }, function() mock.song:swap_tracks_at(1, 2) end)
  eq(muted, "Kick")
  eq(display.note, nil)
end)

test("review SW1: two swaps in a row, then a track made before it: still the same track", function()
  local muted = slot_mutes_after({ 1 }, function()
    mock.song:swap_tracks_at(1, 2)
    H.tick(2)
    mock.song:swap_tracks_at(2, 3)
    H.tick(2)
    mock.ops.insert_track(1, { name = "New" })
  end)
  eq(muted, "Kick")
end)

test("review SW1b: the producer deletes the muted track: the slot comes back, and says the track is gone", function()
  local muted, display = slot_mutes_after({ 1 }, function() mock.ops.remove_track(1) end)
  eq(muted, "")
  eq(mock.data.sequence[2], 2, "slot 2 is back")
  ok(display.note and display.note:find("deleted since"), "the answer says: " .. tostring(display.note))
end)

test("review SW1b: a swap, then the producer deletes one of two muted tracks: the other is muted, nothing else", function()
  local muted, display = slot_mutes_after({ 1, 3 }, function()
    mock.song:swap_tracks_at(1, 2)
    H.tick(2)
    mock.ops.remove_track(2) -- Kick, after the swap
  end)
  eq(muted, "Bass")
  ok(display.note and display.note:find("A track muted in it"), tostring(display.note))
end)

-- The places a change keeps (see places.lua and the inventory): each kind of place, against what can move it. A place is either
-- followed to where its object is now, or, where what moved can't be told, the change refuses and touches nothing.

-- A patch that sets `name = value` in the temporary copy of a payload file.
local function patch_payload(file, name, value)
  return function(env)
    local path = env.bundle .. "payload/" .. file
    local f = assert(io.open(path, "rb"))
    local text = f:read("*a")
    f:close()
    local lowered, n = text:gsub("M%." .. name .. " = %d+", "M." .. name .. " = " .. value, 1)
    assert(n == 1, name .. " was not found to set")
    f = assert(io.open(path, "wb"))
    f:write(lowered)
    f:close()
  end
end

local function both(...)
  local patches = { ... }
  return function(env) for _, patch in ipairs(patches) do patch(env) end end
end

local function undo_track_delete(peer, previewed, done, tag)
  return restore(peer, "track-delete", record_of(previewed, done), "Kumi: undo d (" .. tag .. ")", done.epoch, done.layout)
end

local function another_song()
  mock.load_song({
    name = "Other", file_name = "/s/other.xrns", bpm = 120, lpb = 4, mixer_device = true,
    tracks = { { name = "Track 01" }, { name = "Track 02" }, { name = "Track 03" }, { name = "Track 04" } },
    send_tracks = 1, patterns = 2, pattern_lines = 32, sequence_length = 2, sequence = { 1, 2 }, instruments = 1,
  })
end

-- Tracks

test("places, track × song opened: a sliced restore stops uncertain, and nothing of the new song is touched", function()
  local env, peer = setup({ patch = constants({ SLICE_WRITES = 40 }) })
  mock.ops.rename_track(2, "Track 02")
  fill(2, 3, 10)
  local previewed, done = delete_track(peer, 2, "Kumi: d (kp01)")
  local id = start_restore(peer, previewed, done, "Kumi: undo d (kp02)")
  another_song()
  local response = wait(peer, id, 400, 20)
  ok(response and response.error, "an error: " .. json.encode(response and response.result or {}))
  eq(response.error.code, "uncertain", response.error.message)
  eq(names(), "Track 01,Track 02,Track 03,Track 04,Master,Send 1", "the new song's tracks")
end)

test("places, track × tool reloaded: a sliced restore that wrote answers uncertain and names what it was putting back", function()
  local env, peer = setup({ patch = constants({ SLICE_WRITES = 40 }) })
  fill(3, 3, 10)
  local previewed, done = delete_track(peer, 3, "Kumi: d (kp03)")
  local id = start_restore(peer, previewed, done, "Kumi: undo d (kp04)")
  local other = H.connect(env)
  H.frames(other)
  send(other, "renoise.reload", {})
  local response = wait(peer, id, 400, 20)
  ok(response and response.error, "an error")
  eq(response.error.code, "uncertain", response.error.message)
  ok(response.error.message:find("Putting \226\128\156Bass\226\128\157 back", 1, true) and response.error.message:find("look for it", 1, true), response.error.message)
end)

test("places, track × a change the tool can't follow: the undo of a delete made before is refused stale, nothing written", function()
  local env, peer = setup()
  local previewed, done = delete_track(peer, 3, "Kumi: d (kp05)")
  -- an insert is told but the count didn't move by one
  mock.fire(mock.song.tracks_observable, { type = "insert", index = 1 })
  H.tick(2)
  local before, tracks = steps(), names()
  failure(undo_track_delete(peer, previewed, done, "kp06"), "stale")
  eq(steps(), before)
  eq(names(), tracks)
end)

test("places, track × more shifts than kept: refused stale, nothing written", function()
  local env, peer = setup({ patch = patch_payload("bridge.lua", "SHIFT_HISTORY", 4) })
  local previewed, done = delete_track(peer, 3, "Kumi: d (kp07)")
  for k = 1, 5 do mock.ops.insert_track(1, { name = "New " .. k }) end
  H.tick(2)
  local before = steps()
  failure(undo_track_delete(peer, previewed, done, "kp08"), "stale")
  eq(steps(), before)
end)

-- Devices: a deleted device goes back after the device that was before it; a routing's device is followed on its track.

local function device_names(t)
  local out = {}
  for d = 2, #mock.data.tracks[t].devices do out[#out + 1] = mock.data.tracks[t].devices[d].display_name ~= "" and mock.data.tracks[t].devices[d].display_name or mock.data.tracks[t].devices[d].name end
  return table.concat(out, ",")
end

-- Bass with three devices, named A, B and C; Kumi deletes B. Answers the record.
local function bass_devices_delete_b(peer)
  local track = mock.song:track(3)
  for d, name in ipairs({ "A", "B", "C" }) do
    track:insert_device_at("Audio/Effects/Native/Gainer", d + 1).display_name = name
  end
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  local epoch, layout = place(peer)
  local previewed = preview(peer, "device-delete", { ref = epoch .. ":device:3.3" })
  local done = apply(peer, "device-delete", { ref = epoch .. ":device:3.3" }, "Kumi: dd (kp10)", previewed.fence, epoch, layout)
  ok(done.result, json.encode(done.error or {}))
  eq(device_names(3), "A,C")
  return previewed, done.result
end

local function undo_device_delete(peer, previewed, done, tag)
  return restore(peer, "device-delete", record_of(previewed, done), "Kumi: undo dd (" .. tag .. ")", done.epoch, done.layout)
end

test("places, device × insert: a deleted device goes back after the one that was before it", function()
  routing_catalog()
  local env, peer = setup()
  local previewed, done = bass_devices_delete_b(peer)
  mock.song:track(3):insert_device_at("Audio/Effects/Native/Gainer", 2).display_name = "New"
  local undone = undo_device_delete(peer, previewed, done, "kp11")
  ok(undone.result, json.encode(undone.error or {}))
  eq(device_names(3), "New,A,B,C")
end)

test("places, device × remove: the device before it gone, it goes back after the one before that", function()
  routing_catalog()
  local env, peer = setup()
  local previewed, done = bass_devices_delete_b(peer)
  mock.song:track(3):delete_device_at(2) -- A
  local undone = undo_device_delete(peer, previewed, done, "kp12")
  ok(undone.result, json.encode(undone.error or {}))
  eq(device_names(3), "B,C")
end)

test("places, device × swap: it goes back after the device that was before it, where the producer moved that", function()
  routing_catalog()
  local env, peer = setup()
  local previewed, done = bass_devices_delete_b(peer)
  mock.song:track(3):swap_devices_at(2, 3) -- C, A
  local undone = undo_device_delete(peer, previewed, done, "kp13")
  ok(undone.result, json.encode(undone.error or {}))
  eq(device_names(3), "C,A,B")
end)

test("places, device × tool reloaded: where it goes can't be told: refused stale, nothing written", function()
  routing_catalog()
  local env, peer = setup()
  local previewed, done = bass_devices_delete_b(peer)
  call(peer, "renoise.reload", {})
  H.tick(2)
  local before = steps()
  failure(undo_device_delete(peer, previewed, done, "kp14"), "stale")
  eq(steps(), before)
  eq(device_names(3), "A,C")
end)

test("places, device × more operations than kept: refused stale, nothing written", function()
  routing_catalog()
  local env, peer = setup({ patch = patch_payload("places.lua", "KEPT", 2) })
  local previewed, done = bass_devices_delete_b(peer)
  local track = mock.song:track(3)
  for _ = 1, 3 do track:swap_devices_at(2, 3) end
  local before = steps()
  failure(undo_device_delete(peer, previewed, done, "kp15"), "stale")
  eq(steps(), before)
end)

local function lfo_dest(t, d)
  local p = mock.data.tracks[t].devices[d].params
  return string.format("%d/%d/%d", p[1].value, p[2].value, p[3].value)
end

test("places, device × reorder of two of a kind: each LFO is aimed at the Gainer it drove", function()
  routing_catalog()
  local env, peer = setup()
  local bass = mock.song:track(3)
  bass:insert_device_at("Audio/Effects/Native/Gainer", 2)
  bass:insert_device_at("Audio/Effects/Native/Gainer", 3)
  local kick = mock.song:track(1)
  kick:insert_device_at("Audio/Effects/Native/*LFO", 2).display_name = "A"
  kick:insert_device_at("Audio/Effects/Native/*LFO", 3).display_name = "B"
  local a, b = mock.data.tracks[1].devices[2].params, mock.data.tracks[1].devices[3].params
  a[1].value, a[2].value, a[3].value = 2, 1, 0 -- A drives Bass's first Gainer
  b[1].value, b[2].value, b[3].value = 2, 2, 0 -- B its second
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  local previewed, done = delete_track(peer, 3, "Kumi: d (kp16)")
  kick:swap_devices_at(2, 3) -- B, A
  local undone = undo_track_delete(peer, previewed, done, "kp17")
  ok(undone.result, json.encode(undone.error or {}))
  eq(device_names(1), "B,A")
  eq(lfo_dest(1, 2), "2/2/0", "B drives the second Gainer")
  eq(lfo_dest(1, 3), "2/1/0", "A drives the first")
end)

test("places, device × the device it was aimed at replaced by another of its kind: left unaimed, and the answer says so", function()
  routing_catalog()
  local env, peer = setup()
  mock.song:track(3):insert_device_at("Audio/Effects/Native/Gainer", 2).display_name = "Duck"
  mock.song:track(1):insert_device_at("Audio/Effects/Native/*LFO", 2)
  local p = mock.data.tracks[1].devices[2].params
  p[1].value, p[2].value, p[3].value = 2, 1, 0 -- Kick's LFO drives Bass's Gainer
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  local epoch, layout = place(peer)
  local previewed = preview(peer, "device-delete", { ref = epoch .. ":device:1.2" })
  local done = apply(peer, "device-delete", { ref = epoch .. ":device:1.2" }, "Kumi: dd (kp18)", previewed.fence, epoch, layout)
  ok(done.result, json.encode(done.error or {}))
  -- the producer deletes the Gainer and puts another in its place (a name is no identity: a renamed one would be the same)
  mock.song:track(3):delete_device_at(2)
  mock.song:track(3):insert_device_at("Audio/Effects/Native/Gainer", 2)
  local undone = undo_device_delete(peer, previewed, done.result, "kp19")
  ok(undone.result, json.encode(undone.error or {}))
  eq(lfo_dest(1, 2), "-1/-1/-1")
  ok(undone.result.display.note and undone.result.display.note:find("could not be set again", 1, true), json.encode(undone.result.display))
end)

-- Patterns: a deleted track's lines go into the patterns they were in, wherever the pool moved them.

local function lines_in(p, t)
  local n = 0
  for k = 1, mock.song:pattern(p).number_of_lines do
    if mock.song:pattern(p):track(t):line(k):note_column(1).note_value < 120 then n = n + 1 end
  end
  return n
end

-- Bass has 10 lines in pattern 3 only (named Verse); pattern 2 is empty and unused once the sequence leaves it out.
local function bass_in_verse(peer, tag)
  mock.data.patterns[3].name = "Verse"
  mock.data.sequence = { 1, 3, 1, 3 }
  for k = 1, 10 do
    local column = mock.song:pattern(3):track(3):line(k):note_column(1)
    column.note_value, column.instrument_value, column.volume_value = 40 + k, 0, 64
  end
  H.tick(2)
  return delete_track(peer, 3, "Kumi: d (" .. tag .. ")")
end

test("places, pattern × remove before it: the lines go into the pattern they were in, at its new number", function()
  local env, peer = setup()
  local previewed, done = bass_in_verse(peer, "kp20")
  delete_unused(2)
  eq(mock.data.patterns[2].name, "Verse")
  local undone = undo_track_delete(peer, previewed, done, "kp21")
  ok(undone.result, json.encode(undone.error or {}))
  eq(lines_in(2, 3), 10, "Bass's lines are in the Verse")
end)

test("places, pattern × append: nothing moves", function()
  local env, peer = setup()
  local previewed, done = bass_in_verse(peer, "kp22")
  mock.ops.insert_pattern(32)
  H.tick(2)
  local undone = undo_track_delete(peer, previewed, done, "kp23")
  ok(undone.result, json.encode(undone.error or {}))
  eq(lines_in(3, 3), 10)
end)

test("places, pattern × the pattern itself removed or made shorter: the track comes back without those lines, and says so", function()
  for _, change in ipairs({ "removed", "shortened" }) do
    local env, peer = setup()
    local previewed, done = bass_in_verse(peer, "kp24")
    if change == "removed" then
      mock.data.sequence = { 1, 2, 1, 2 }
      mock.ops.remove_pattern(3)
    else
      mock.data.patterns[3].lines = 16
    end
    H.tick(2)
    local undone = undo_track_delete(peer, previewed, done, "kp25")
    ok(undone.result, change .. ": " .. json.encode(undone.error or {}))
    eq(names(), "Kick,Hat,Bass,Pad,Master,Send 1")
    ok(undone.result.display.note and undone.result.display.note:find("Its lines in 1 pattern aren't put back", 1, true), json.encode(undone.result.display))
    if change == "shortened" then eq(lines_in(3, 3), 0, "nothing written into the pattern that isn't what it was") end
  end
end)

test("places, pattern × a change the tool can't follow, or more than kept: refused stale, nothing written", function()
  for _, how in ipairs({ "unfollowable", "overflow" }) do
    local env, peer = setup(how == "overflow" and { patch = patch_payload("places.lua", "KEPT", 2) } or nil)
    local previewed, done = bass_in_verse(peer, "kp26")
    if how == "unfollowable" then
      mock.fire(mock.data.song_fields.patterns_observable, nil)
    else
      for _ = 1, 3 do mock.ops.insert_pattern(32) end
    end
    H.tick(2)
    local before, tracks = steps(), names()
    failure(undo_track_delete(peer, previewed, done, "kp27"), "stale")
    eq(steps(), before, how)
    eq(names(), tracks)
  end
end)

-- Slots: Renoise's sequence notifier can't say where a slot went, so a slot's place holds while the sequence up to it is the same.

-- (the mock keys slot mutes by track number and never moves them with a track deleted; in Renoise the deleted track's mutes go
-- with it, and the tracks after it keep theirs)
local function delete_track_mutes(t)
  local kept = {}
  for key in pairs(mock.data.muted) do
    local track, slot = key:match("^(%d+),(%d+)$")
    track = tonumber(track)
    if track ~= t then kept[(track > t and track - 1 or track) .. "," .. slot] = true end
  end
  mock.data.muted = kept
end

test("places, slot × append after: a deleted track's mute in a slot is put back", function()
  local env, peer = setup()
  mock.song.sequencer:set_track_sequence_slot_is_muted(3, 2, true)
  H.tick(2)
  local previewed, done = delete_track(peer, 3, "Kumi: d (kp30)")
  delete_track_mutes(3)
  mock.song.sequencer:insert_sequence_at(5, 1)
  H.tick(2)
  local undone = undo_track_delete(peer, previewed, done, "kp31")
  ok(undone.result, json.encode(undone.error or {}))
  ok(mock.song.sequencer:track_sequence_slot_is_muted(3, 2), "muted in slot 2 again")
end)

test("places, slot × insert before: the track comes back, muted in the slot its mute was in, followed to its new place", function()
  local env, peer = setup()
  mock.song.sequencer:set_track_sequence_slot_is_muted(3, 2, true)
  H.tick(2)
  local previewed, done = delete_track(peer, 3, "Kumi: d (kp32)")
  delete_track_mutes(3)
  mock.song.sequencer:insert_sequence_at(1, 3)
  H.tick(2)
  local undone = undo_track_delete(peer, previewed, done, "kp33")
  ok(undone.result, json.encode(undone.error or {}))
  -- (one slot made at 1 is the only block that explains the sequence now: slot 2 is slot 3)
  for s = 1, #mock.data.sequence do eq(mock.song.sequencer:track_sequence_slot_is_muted(3, s), s == 3, "slot " .. s) end
  eq(undone.result.display.note, nil)
end)

test("places, slot × a deleted slot: put back where it was after an append, and after an insert before it, followed", function()
  for _, at in ipairs({ 4, 1 }) do
    local env, peer = setup()
    local epoch, layout = place(peer)
    local previewed = preview(peer, "slot-delete", { ref = seq_ref(epoch, 3) })
    local done = apply(peer, "slot-delete", { ref = seq_ref(epoch, 3) }, "Kumi: delete slot 3 (kp34)", previewed.fence, epoch, layout)
    ok(done.result, json.encode(done.error or {}))
    mock.song.sequencer:insert_sequence_at(at, 2)
    H.tick(2)
    local before, sequence = steps(), seq_text()
    local undone = restore(peer, "slot-delete", record_of(previewed, done.result), "Kumi: undo (kp35)", done.result.epoch, done.result.layout)
    ok(undone.result, json.encode(undone.error or {}))
    eq(seq_text(), at == 4 and "1,2,1,3,2" or "2,1,2,1,3", "slot 3 (pattern 1) back between 2 and 3")
  end
end)

-- Instruments: a deleted track's notes play the instruments they played, wherever those moved.

local function bass_plays_bass_synth(peer, tag)
  for k = 1, 10 do
    local column = mock.song:pattern(1):track(3):line(k):note_column(1)
    column.note_value, column.instrument_value, column.volume_value = 40 + k, 1, 64 -- the second instrument
  end
  H.tick(2)
  return delete_track(peer, 3, "Kumi: d (" .. tag .. ")")
end

local INSTRUMENTS = { instruments = 3, instrument_specs = { { name = "Drums" }, { name = "Bass synth" }, { name = "Pad synth" } } }

local function instrument_played()
  return mock.song:pattern(1):track(3):line(1):note_column(1).instrument_value
end

test("places, instrument × insert and swap: the notes play the instrument they played", function()
  for _, how in ipairs({ "insert", "swap" }) do
    local env, peer = setup(nil, INSTRUMENTS)
    local previewed, done = bass_plays_bass_synth(peer, "kp40")
    if how == "insert" then mock.ops.insert_instrument_at(1, { name = "New" }) else mock.ops.swap_instruments(2, 3) end
    H.tick(2)
    local undone = undo_track_delete(peer, previewed, done, "kp41")
    ok(undone.result, how .. ": " .. json.encode(undone.error or {}))
    eq(mock.data.instruments[instrument_played() + 1].name, "Bass synth", how)
  end
end)

test("places, instrument × the instrument removed: refused changed, nothing written", function()
  local env, peer = setup(nil, INSTRUMENTS)
  local previewed, done = bass_plays_bass_synth(peer, "kp42")
  mock.ops.remove_instrument(2)
  H.tick(2)
  local before, tracks = steps(), names()
  local undone = undo_track_delete(peer, previewed, done, "kp43")
  failure(undone, "changed")
  ok(undone.error.message:find("Bass synth", 1, true), undone.error.message)
  eq(steps(), before)
  eq(names(), tracks)
end)

test("places, instrument × more operations than kept: refused stale, nothing written", function()
  local env, peer = setup({ patch = patch_payload("places.lua", "KEPT", 2) }, INSTRUMENTS)
  local previewed, done = bass_plays_bass_synth(peer, "kp44")
  for _ = 1, 3 do mock.ops.swap_instruments(1, 3) end
  H.tick(2)
  local before = steps()
  failure(undo_track_delete(peer, previewed, done, "kp45"), "stale")
  eq(steps(), before)
end)

-- Values that are places: a #Send's Receiver (a send) and a meta device's Dest. Track (a track). Renoise moves them with the tracks
-- made and deleted; a record keeps the track they name, and a write is of the track the preview saw.

-- Kick with a #Send feeding Send 1 (of Send 1 and Send 2).
local function kick_send(peer)
  routing_catalog()
  mock.song:track(1):insert_device_at("Audio/Effects/Native/#Send", 2)
  mock.data.tracks[1].devices[2].params[3].value = 0
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  return place(peer)
end

local function set_param(peer, ref, input, desc)
  local epoch, layout = place(peer)
  local merged = { ref = ref }
  for k, v in pairs(input) do merged[k] = v end
  local previewed = preview(peer, "device-param", merged)
  local done = apply(peer, "device-param", merged, desc, previewed.fence, epoch, layout)
  return previewed, done
end

test("places, value × a send deleted: the undo of a Receiver Kumi set says the send it fed is gone, and claims nothing", function()
  local env, peer = setup(nil, { send_tracks = 2 })
  local epoch = kick_send(peer)
  local previewed, done = set_param(peer, epoch .. ":device:1.2", { param = "Receiver", value = 1 }, "Kumi: Receiver (kp50)")
  ok(done.result, json.encode(done.error or {}))
  mock.ops.remove_track(6) -- Send 1: Renoise moves the Receiver to 0, still Send 2
  H.tick(2)
  eq(receiver(1, 2), 0)
  local undone = restore(peer, "device-param", record_of(previewed, done.result), "Kumi: undo (kp51)", done.result.epoch, done.result.layout)
  failure(undone, "gone")
  eq(receiver(1, 2), 0, "it still feeds Send 2")
end)

test("places, value × a send made before: the undo of a Receiver Kumi set feeds the send it fed before", function()
  local env, peer = setup(nil, { send_tracks = 2 })
  local epoch = kick_send(peer)
  local previewed, done = set_param(peer, epoch .. ":device:1.2", { param = "Receiver", value = 1 }, "Kumi: Receiver (kp52)")
  ok(done.result, json.encode(done.error or {}))
  mock.ops.insert_track(6, { kind = "send", name = "New send" })
  H.tick(2)
  eq(receiver(1, 2), 2, "Renoise moved it: Send 2 is the third send")
  local undone = restore(peer, "device-param", record_of(previewed, done.result), "Kumi: undo (kp53)", done.result.epoch, done.result.layout)
  ok(undone.result, json.encode(undone.error or {}))
  eq(receiver(1, 2), 1, "Send 1, the second send now")
  eq(mock.data.tracks[7].name, "Send 1")
end)

test("places, value × a send made between the preview and the apply: refused changed, nothing written", function()
  local env, peer = setup(nil, { send_tracks = 2 })
  local epoch, layout = kick_send(peer)
  local input = { ref = epoch .. ":device:1.2", param = "Receiver", value = 1 }
  local previewed = preview(peer, "device-param", input)
  mock.ops.insert_track(6, { kind = "send", name = "New send" })
  H.tick(2)
  local before = steps()
  failure(apply(peer, "device-param", input, "Kumi: Receiver (kp54)", previewed.fence, epoch, layout), "changed")
  eq(receiver(1, 2), 1, "Renoise's own move of Send 1, nothing of Kumi's")
  eq(steps(), before)
end)

test("places, value × device-add's receiver: a send made between the preview and the apply refuses, no device made", function()
  local env, peer = setup(nil, { send_tracks = 2 })
  routing_catalog()
  local epoch, layout = place(peer)
  local input = { ref = track_ref(epoch, 1), path = "Audio/Effects/Native/#Send", receiver = 1 }
  local previewed = preview(peer, "device-add", input)
  mock.ops.insert_track(6, { kind = "send", name = "New send" })
  H.tick(2)
  failure(apply(peer, "device-add", input, "Kumi: add (kp55)", previewed.fence, epoch, layout), "changed")
  eq(#mock.data.tracks[1].devices, 1, "no device made")
end)

test("places, value × Dest. Track: a track made between the preview and the apply refuses; the undo after a track deleted is true", function()
  local env, peer = setup()
  routing_catalog()
  mock.song:track(1):insert_device_at("Audio/Effects/Native/*LFO", 2)
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  local epoch, layout = place(peer)
  local input = { ref = epoch .. ":device:1.2", param = "Dest. Track", value = 2 } -- Bass
  local previewed = preview(peer, "device-param", input)
  mock.ops.insert_track(1, { name = "New" })
  H.tick(2)
  failure(apply(peer, "device-param", input, "Kumi: aim (kp56)", previewed.fence, epoch, layout), "changed")
  eq(mock.data.tracks[2].devices[2].params[1].value, -1, "aimed nowhere")
  -- Aimed at Bass, then the producer deletes the track before it: Renoise moves the value, and the undo takes Kumi's aim back.
  local previewed2, done = set_param(peer, place(peer) .. ":device:2.2", { param = "Dest. Track", value = 3 }, "Kumi: aim (kp57)")
  ok(done.result, json.encode(done.error or {}))
  mock.ops.remove_track(1)
  H.tick(2)
  eq(mock.data.tracks[1].devices[2].params[1].value, 2, "Renoise moved it with Bass")
  local undone = restore(peer, "device-param", record_of(previewed2, done.result), "Kumi: undo (kp58)", done.result.epoch, done.result.layout)
  ok(undone.result, json.encode(undone.error or {}))
  eq(mock.data.tracks[1].devices[2].params[1].value, -1)
end)

-- Cells: Renoise moves a track's lines with a row inserted or deleted in the pattern editor, and its columns with a column moved,
-- and tells neither apart from an edit. Kumi's lines moved as a whole are left where they are, and the undo says so.

local function notify_lines(p, t, line)
  for _, fn in ipairs(mock.data.patterns[p].notifiers) do pcall(fn, { pattern = p, track = t, line = line }) end
end

local function insert_row(p, t, r)
  local pattern = mock.data.patterns[p]
  local data = pattern.tracks[t]
  local moved = {}
  for k, line in pairs(data.lines) do
    if k < r then moved[k] = line elseif k + 1 <= pattern.lines then moved[k + 1] = line end
  end
  data.lines = moved
  notify_lines(p, t, r)
  H.tick(2)
end

local function swap_note_columns(t, a, b)
  for p, pattern in ipairs(mock.data.patterns) do
    local data = pattern.tracks[t]
    if data then
      for _, line in pairs(data.lines or {}) do
        line.notes = line.notes or {}
        line.notes[a], line.notes[b] = line.notes[b], line.notes[a]
      end
      notify_lines(p, t, 1)
    end
  end
  H.tick(2)
end

local function bass_notes(peer, first, last, tag)
  local epoch, layout = place(peer)
  local lines = {}
  for l = first, last do lines[#lines + 1] = { line = l - 1, notes = { { col = 1, note = "C-4", instrument = "00" } } } end
  local input = { pattern = pattern_ref(epoch, 1), track = track_ref(epoch, 3), lines = lines }
  local previewed = preview(peer, "notes-write", input)
  local done = apply(peer, "notes-write", input, "Kumi: n (" .. tag .. ")", previewed.fence, epoch, layout)
  ok(done.result, json.encode(done.error or {}))
  return previewed, done.result
end

local function c4_lines(col)
  local out = {}
  for k = 1, 32 do
    if mock.song:pattern(1):track(3):line(k):note_column(col or 1).note_string == "C-4" then out[#out + 1] = k end
  end
  return table.concat(out, ",")
end

test("places, cell × a row inserted above Kumi's lines: refused changed, the lines left where they are", function()
  local env, peer = setup()
  local previewed, done = bass_notes(peer, 2, 4, "kp60")
  insert_row(1, 3, 1)
  eq(c4_lines(), "3,4,5")
  local before = steps()
  local undone = restore(peer, "notes-write", record_of(previewed, done), "Kumi: undo n (kp61)", done.epoch, done.layout)
  failure(undone, "changed")
  ok(undone.error.message:find("the pattern changed in Renoise since", 1, true), undone.error.message)
  eq(c4_lines(), "3,4,5")
  eq(steps(), before)
end)

test("places, cell × Kumi's column moved: refused changed, nothing written", function()
  local env, peer = setup()
  local previewed, done = bass_notes(peer, 1, 4, "kp62")
  swap_note_columns(3, 1, 2)
  eq(c4_lines(1), "")
  eq(c4_lines(2), "1,2,3,4")
  failure(restore(peer, "notes-write", record_of(previewed, done), "Kumi: undo n (kp63)", done.epoch, done.layout), "changed")
  eq(c4_lines(2), "1,2,3,4")
end)

test("places, cell × nothing moved: the lines are put back as before", function()
  local env, peer = setup()
  local previewed, done = bass_notes(peer, 2, 4, "kp64")
  local undone = restore(peer, "notes-write", record_of(previewed, done), "Kumi: undo n (kp65)", done.epoch, done.layout)
  ok(undone.result, json.encode(undone.error or {}))
  eq(c4_lines(), "")
end)

test("places, cell × tool reloaded during a sliced notes restore: uncertain, naming the lines", function()
  local env, peer = setup({ patch = constants({ SLICE_WRITES = 40 }) })
  local previewed, done = bass_notes(peer, 1, 24, "kp66")
  local id = send(peer, "change_restore", restore_params("notes-write", record_of(previewed, done), "Kumi: undo n (kp67)", done.epoch, done.layout))
  local before = steps()
  for _ = 1, 50 do
    mock.advance(10)
    if steps() > before then break end
  end
  eq(steps(), before + 1, "step 1 is written")
  local other = H.connect(env)
  H.frames(other)
  send(other, "renoise.reload", {})
  local response = wait(peer, id, 400, 20)
  ok(response and response.error, "an error")
  eq(response.error.code, "uncertain", response.error.message)
  ok(response.error.message:find("Putting the lines back in", 1, true), response.error.message)
end)

test("places, cell × a row deleted above Kumi's lines: refused changed, the lines left where they are", function()
  local env, peer = setup()
  local previewed, done = bass_notes(peer, 3, 6, "kp68")
  local pattern = mock.data.patterns[1]
  local data = pattern.tracks[3]
  local moved = {}
  for k, line in pairs(data.lines) do
    if k < 1 then moved[k] = line elseif k > 1 then moved[k - 1] = line end
  end
  data.lines = moved
  notify_lines(1, 3, 1)
  H.tick(2)
  eq(c4_lines(), "2,3,4,5")
  failure(restore(peer, "notes-write", record_of(previewed, done), "Kumi: undo n (kp69)", done.epoch, done.layout), "changed")
  eq(c4_lines(), "2,3,4,5")
end)

-- Where a moved track came from is a place among the tracks, which a track deleted there doesn't take away.

test("places, track × the track at a moved track's old place deleted: the move is still put back", function()
  local env, peer = setup()
  local epoch, layout = place(peer)
  local input = { ref = track_ref(epoch, 1), index = 3 }
  local previewed = preview(peer, "track-move", input)
  local done = apply(peer, "track-move", input, "Kumi: move (kp70)", previewed.fence, epoch, layout)
  ok(done.result, json.encode(done.error or {}))
  eq(names(), "Hat,Bass,Kick,Pad,Master,Send 1")
  mock.ops.remove_track(1) -- Hat, now at Kick's old place
  H.tick(2)
  local undone = restore(peer, "track-move", record_of(previewed, done.result), "Kumi: undo (kp71)", done.result.epoch, done.result.layout)
  ok(undone.result, json.encode(undone.error or {}))
  eq(names(), "Kick,Bass,Pad,Master,Send 1")
end)

test("places, track × a member before a track taken out of its group deleted: it goes back in, last", function()
  local env, peer = setup()
  mock.song:insert_group_at(5)
  mock.song:add_track_to_group(3, 5) -- Bass
  mock.song:add_track_to_group(3, 5) -- Pad
  H.tick(2)
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  eq(order(), "Kick,Hat,Bass(in),Pad(in),Group[G],Master,Send 1")
  local epoch, layout = place(peer)
  local input = { ref = track_ref(epoch, 4), group = json.null }
  local previewed = preview(peer, "track-group", input)
  local done = apply(peer, "track-group", input, "Kumi: out (kp72)", previewed.fence, epoch, layout)
  ok(done.result, json.encode(done.error or {}))
  eq(order(), "Kick,Hat,Pad,Bass(in),Group[G],Master,Send 1")
  mock.ops.remove_track(4) -- Bass
  H.tick(2)
  local undone = restore(peer, "track-group", record_of(previewed, done.result), "Kumi: undo (kp73)", done.result.epoch, done.result.layout)
  ok(undone.result, json.encode(undone.error or {}))
  eq(order(), "Kick,Hat,Pad(in),Group[G],Master,Send 1")
end)

-- run-lua: Lua run in Renoise, one step of Renoise's undo; only Renoise's undo can take it back.

local function run_lua(peer, code, desc, extra)
  local epoch, layout = place(peer)
  local input = { code = code }
  for k, v in pairs(extra or {}) do input[k] = v end
  local previewed = call(peer, "change_preview", { kind = "run-lua", input = input, epoch = epoch, layout = layout })
  if not previewed.result then return previewed end
  return apply(peer, "run-lua", input, desc, previewed.result.fence, epoch, layout), previewed.result
end

test("run-lua: a chunk changes the song in one named step, says what it printed and returned, and Kumi's undo is Renoise's", function()
  local env, peer = setup()
  local before = steps()
  local done, previewed = run_lua(peer, "song.transport.bpm = 140\nprint('set', 140)\nreturn { bpm = song.transport.bpm, name = song.name }", "Kumi: Ran Lua (kl01)")
  ok(done.result, json.encode(done.error or {}))
  eq(previewed.display.title, "Ran Lua: song.transport.bpm = 140")
  eq(mock.song.transport.bpm, 140)
  eq(done.result.applied.changed, true)
  eq(done.result.applied.printed[1], "set\t140")
  eq(done.result.applied.result.bpm, 140)
  eq(done.result.undo.entry, true)
  eq(steps(), before + 1, "one step of Renoise's undo")
  local undone = restore(peer, "run-lua", { target = done.result.target, prior = {}, applied = done.result.applied }, "Kumi: undo (kl02)", done.result.epoch, done.result.layout)
  failure(undone, "daw-only")
  ok(undone.error.message:find("Renoise's own undo", 1, true), undone.error.message)
end)

test("run-lua: a chunk that only reads changes nothing and makes no step", function()
  local env, peer = setup()
  local before = steps()
  local done = run_lua(peer, "local names = {}\nfor i = 1, song.sequencer_track_count do names[i] = song:track(i).name end\nreturn names", "Kumi: Ran Lua (kl03)")
  ok(done.result, json.encode(done.error or {}))
  eq(done.result.applied.changed, false)
  eq(done.result.applied.result[1], "Kick")
  eq(done.result.undo.entry, false)
  eq(done.result.display.unchanged, true)
  eq(steps(), before)
end)

test("run-lua: Lua that doesn't compile is refused at the preview; an error while it runs is said, with what it changed", function()
  local env, peer = setup()
  local response = run_lua(peer, "song.transport.bpm = = 1", "Kumi: Ran Lua (kl04)")
  failure(response, "bad-params")
  ok(response.error.message:find("doesn't compile", 1, true), response.error.message)
  local done = run_lua(peer, "song.transport.bpm = 150\nerror('half way')", "Kumi: Ran Lua (kl05)")
  ok(done.result, json.encode(done.error or {}))
  eq(mock.song.transport.bpm, 150)
  ok(done.result.applied.failed and done.result.applied.failed.message:find("half way", 1, true), json.encode(done.result.applied))
  ok(done.result.display.note:find("what it changed before that stays", 1, true), done.result.display.note)
end)

test("run-lua: a chunk that runs too long is stopped where it is, inside its time", function()
  local env, peer = setup()
  local started = os.time()
  mock.clock_step_ms = 1 -- (the mock's clock moves only as it is read: each look costs a millisecond)
  local done = run_lua(peer, "local n = 0\nwhile true do n = n + 1 end", "Kumi: Ran Lua (kl06)", { timeoutMs = 50 })
  mock.clock_step_ms = nil
  ok(done.result, json.encode(done.error or {}))
  ok(done.result.applied.failed and done.result.applied.failed.message:find("ran longer than 50 ms", 1, true), json.encode(done.result.applied))
  ok(os.time() - started < 5, "stopped")
  failure(run_lua(peer, "return 1", "Kumi: Ran Lua (kl07)", { timeoutMs = 5000 }), "bad-params")
end)

test("run-lua: a huge returned text or printed line is bounded before it is encoded (#361 N-N1)", function()
  local env, peer = setup()
  local done = run_lua(peer, "print(string.rep('p', 100000))\nreturn string.rep('x', 3000000)", "Kumi: Ran Lua (kl20)")
  ok(done.result, json.encode(done.error or {}))
  eq(done.result.applied.result, "(a text of 3000000 bytes, not sent)")
  ok(#done.result.applied.printed[1] < 5000 and done.result.applied.printed[1]:find("100000 bytes in all", 1, true), "the printed line is cut")
end)

test("run-lua: the time limit stops a chunk whose calls fire the tool's notifiers, and each notifier runs whole (#361 N4)", function()
  local env, peer = setup()
  local patterns = #mock.data.patterns
  mock.clock_step_ms = 1
  local done = run_lua(peer, "for i = 1, 3000 do song.sequencer:insert_new_pattern_at(1) end", "Kumi: Ran Lua (kl10)", { timeoutMs = 20 })
  mock.clock_step_ms = nil
  ok(done.result, json.encode(done.error or {}))
  ok(done.result.applied.failed and done.result.applied.failed.message:find("ran longer than 20 ms", 1, true), json.encode(done.result.applied.failed or {}))
  ok(#mock.data.patterns - patterns < 3000, "stopped part way: " .. (#mock.data.patterns - patterns) .. " made")
end)

test("run-lua: a returned table with gaps, a 0 key or names among its numbers is answered, not lost after the chunk wrote", function()
  local env, peer = setup()
  local done = run_lua(peer, "song.transport.bpm = 141\nlocal t = {}\nt[3] = 'C-4'\nreturn t", "Kumi: Ran Lua (ks01)")
  ok(done.result, json.encode(done.error or {}))
  eq(mock.song.transport.bpm, 141)
  eq(done.result.applied.changed, true)
  eq(done.result.applied.result["3"], "C-4")
  local mixed = run_lua(peer, "return { [0] = 'zero', 'a', 'b', name = 'x', [2.5] = 'half', [true] = 'yes' }", "Kumi: Ran Lua (ks02)")
  ok(mixed.result, json.encode(mixed.error or {}))
  local r = mixed.result.applied.result
  eq(r["0"], "zero")
  eq(r["1"], "a")
  eq(r["2"], "b")
  eq(r.name, "x")
  eq(r["2.5"], "half")
  eq(r["true"], "yes")
  local list = run_lua(peer, "return { 'a', { 1, 2, 3 }, { x = { [5] = 1 } } }", "Kumi: Ran Lua (ks03)")
  ok(list.result, json.encode(list.error or {}))
  eq(list.result.applied.result[1], "a")
  eq(list.result.applied.result[2][3], 3)
  eq(list.result.applied.result[3].x["5"], 1)
  -- (an error whose own text can't be made is still answered)
  local odd = run_lua(peer, "song.transport.bpm = 142\nerror(setmetatable({}, { __tostring = function() error('no text') end }))", "Kumi: Ran Lua (ks04)")
  ok(odd.result, json.encode(odd.error or {}))
  eq(mock.song.transport.bpm, 142)
  ok(odd.result.applied.failed, json.encode(odd.result.applied))
end)

test("run-lua: a stop never lands in the guard around the tool's notifiers, before or after the notifier's own code (#361 N4, final review)", function()
  local env, peer = setup()
  -- (a different count of the chunk's own instructions each round moves where the hook's count runs out)
  for pad = 0, 59 do
    mock.clock_step_ms = 1
    local code = "local x = 0\nfor i = 1, 3000 do " .. string.rep("x = x + 1 ", pad) .. "song.sequencer:insert_new_pattern_at(1) end"
    local done = run_lua(peer, code, string.format("Kumi: Ran Lua (kq%02d)", pad), { timeoutMs = 5 })
    mock.clock_step_ms = nil
    ok(done.result, json.encode(done.error or {}))
    ok(done.result.applied.failed and done.result.applied.failed.message:find("ran longer than 5 ms", 1, true), json.encode(done.result.applied.failed or {}))
  end
  -- (Renoise disables a notifier that raises behind an error dialog: the mock records it and takes the notifier off)
  for _, err in ipairs(mock.errors) do ok(not err:find("^notifier"), err) end
end)

test("run-lua: a loop whose turns all take the same count, the hook's always landing in a notifier, is stopped too (#361 N4, final review)", function()
  local env, peer = setup()
  -- (one of these shapes, measured, meets the 1000-count hook inside the tool's bpm notifier on every turn)
  for pad = 0, 19 do
    mock.clock_step_ms = 1
    local code = "local x = 0\nfor i = 1, 20000 do " .. string.rep("x = x + 1 ", pad) .. "song.transport.bpm = 120 + i % 2 end"
    local done = run_lua(peer, code, string.format("Kumi: Ran Lua (kt%02d)", pad), { timeoutMs = 5 })
    mock.clock_step_ms = nil
    ok(done.result, json.encode(done.error or {}))
    ok(done.result.applied.failed and done.result.applied.failed.message:find("ran longer than 5 ms", 1, true), "shape " .. pad .. ": " .. json.encode(done.result.applied.failed or {}))
  end
  for _, err in ipairs(mock.errors) do ok(not err:find("^notifier"), err) end
end)

test("run-lua: a chunk's loop in a coroutine of its own is stopped too", function()
  local env, peer = setup()
  mock.clock_step_ms = 1
  local done = run_lua(peer, "local co = coroutine.wrap(function() local n = 0 while true do n = n + 1 end end)\nco()", "Kumi: Ran Lua (kq90)", { timeoutMs = 20 })
  mock.clock_step_ms = nil
  ok(done.result, json.encode(done.error or {}))
  ok(done.result.applied.failed and done.result.applied.failed.message:find("ran longer than 20 ms", 1, true), json.encode(done.result.applied.failed or {}))
end)

test("run-lua: code a chunk loads itself is under the time limit too: the JIT is off while it runs (#361 N18)", function()
  local env, peer = setup()
  mock.clock_step_ms = 1
  local code = "local f = loadstring('local n = 0 for i = 1, 2e8 do n = n + i % 7 end return n') return f()"
  local done = run_lua(peer, code, "Kumi: Ran Lua (kl11)", { timeoutMs = 50 })
  mock.clock_step_ms = nil
  ok(done.result, json.encode(done.error or {}))
  ok(done.result.applied.failed and done.result.applied.failed.message:find("ran longer than 50 ms", 1, true), json.encode(done.result.applied))
  ok(not jit or jit.status(), "the JIT is on again")
end)

test("run-lua: a chunk that sets a parameter changed the song; one that only mutes makes no Renoise step; one that reads changed nothing (#361 N8)", function()
  local env, peer = setup()
  local done = run_lua(peer, "song:track(1).postfx_volume.value = 0.5", "Kumi: Ran Lua (kl12)")
  ok(done.result, json.encode(done.error or {}))
  eq(done.result.applied.changed, true, "a parameter no notifier tells")
  eq(done.result.undo.entry, true)
  eq(done.result.display.unchanged, nil)
  local muted = run_lua(peer, "song:track(2).mute_state = renoise.Track.MUTE_STATE_MUTED", "Kumi: Ran Lua (kl13)")
  ok(muted.result, json.encode(muted.error or {}))
  eq(muted.result.applied.changed, true)
  eq(muted.result.undo.entry, false, "Renoise makes no step of a mute")
  ok(muted.result.display.note:find("nothing Renoise's undo takes back", 1, true), muted.result.display.note)
  local read = run_lua(peer, "local t = {}\nt.name = song:track(1).name:upper() -- t.x = 1\nt['k'] = \"a = b.c\"\nreturn t", "Kumi: Ran Lua (kl14)")
  ok(read.result, json.encode(read.error or {}))
  eq(read.result.display.unchanged, true, "it set only its own table's fields")
  eq(read.result.undo.entry, false)
end)

test("run-lua: mute, unmute and solo calls, notes played and playback scheduled make no Renoise step (#361 N8, final review)", function()
  local env, peer = setup(nil, { instruments = 1 })
  local codes = {
    "song:track(2):mute()", "song:track(2):unmute()", "song:track(1):solo()",
    "song:trigger_instrument_note_on(1, 1, 48, 1)\nsong:trigger_instrument_note_off(1, 1, 48)",
    "song.transport:trigger_sequence(1)", "song.transport:add_scheduled_sequence(2)\nsong.transport:set_scheduled_sequence(1)",
  }
  for k, code in ipairs(codes) do
    local before = steps()
    local done = run_lua(peer, code, string.format("Kumi: Ran Lua (kr%02d)", k))
    ok(done.result, json.encode(done.error or {}))
    eq(done.result.applied.failed, nil, code)
    eq(done.result.applied.writes, "unkept", code)
    eq(done.result.undo.entry, false, code .. ": Renoise makes no step of it")
    if done.result.applied.changed then
      ok(done.result.display.note:find("nothing Renoise's undo takes back", 1, true), done.result.display.note)
    end
    eq(steps(), before, code)
  end
  eq(mock.data.tracks[1].solo, true, "the solo was played")
  -- (with a write Renoise's undo keeps, it is a step again)
  local mixed = run_lua(peer, "song:track(2):mute()\nsong.transport.bpm = 99", "Kumi: Ran Lua (kr10)")
  ok(mixed.result, json.encode(mixed.error or {}))
  eq(mixed.result.undo.entry, true)
end)

test("places, names × a pattern or an instrument renamed is the same one: the lines go back into it, playing it", function()
  local env, peer = setup(nil, INSTRUMENTS)
  local previewed, done = bass_in_verse(peer, "kp80")
  mock.data.patterns[3].name = "Verse 2"
  mock.data.instruments[1].name = "Drums (loaded)"
  H.tick(2)
  local undone = undo_track_delete(peer, previewed, done, "kp81")
  ok(undone.result, json.encode(undone.error or {}))
  eq(lines_in(3, 3), 10)
  eq(undone.result.display.note, nil)
end)

-- Slots where patterns repeat: a slot made or deleted before a run of one pattern leaves the sequence up to the slot as it was.

local function mute_in_slot_3_after(sequence, act, tag)
  local env, peer = setup(nil, { patterns = 2, sequence = sequence, sequence_length = #sequence })
  mock.song.sequencer:set_track_sequence_slot_is_muted(3, 3, true)
  H.tick(2)
  local previewed, done = delete_track(peer, 3, "Kumi: d (" .. tag .. ")")
  delete_track_mutes(3)
  act()
  H.tick(2)
  local undone = undo_track_delete(peer, previewed, done, tag .. "u")
  ok(undone.result, json.encode(undone.error or {}))
  local muted = {}
  for s = 1, #mock.data.sequence do
    if mock.song.sequencer:track_sequence_slot_is_muted(3, s) then muted[#muted + 1] = s end
  end
  return table.concat(muted, ","), undone.result.display.note
end

test("places, slot × patterns that repeat: a slot made or deleted before a run of its pattern leaves the mute out, and says so", function()
  for _, act in ipairs({
    function() mock.song.sequencer:insert_sequence_at(1, 1) end,
    function() mock.song.sequencer:delete_sequence_at(1) end,
  }) do
    local muted, note = mute_in_slot_3_after({ 1, 1, 1, 1 }, act, "kp82")
    eq(muted, "")
    ok(note and note:find("isn't put back", 1, true), tostring(note))
  end
  -- A slot whose neighbours are other patterns: an append after it, or nothing at all, leaves its place as it was.
  for _, act in ipairs({ function() mock.song.sequencer:insert_sequence_at(5, 2) end, function() end }) do
    local muted, note = mute_in_slot_3_after({ 1, 2, 1, 2 }, act, "kp83")
    eq(muted, "3")
    eq(note, nil)
  end
  -- And when nothing changed, a run of one pattern is no doubt.
  local muted = mute_in_slot_3_after({ 1, 1, 1, 1 }, function() end, "kp84")
  eq(muted, "3")
end)

test("places, slot × a deleted slot where patterns repeat: refused stale when a block made before it can't say where it goes, put back when nothing changed", function()
  for _, changed in ipairs({ true, false }) do
    local env, peer = setup(nil, { patterns = 2, sequence = { 1, 2, 1, 2 }, sequence_length = 4 })
    local epoch, layout = place(peer)
    local previewed = preview(peer, "slot-delete", { ref = seq_ref(epoch, 3) })
    local done = apply(peer, "slot-delete", { ref = seq_ref(epoch, 3) }, "Kumi: delete slot 3 (kp85)", previewed.fence, epoch, layout)
    ok(done.result, json.encode(done.error or {}))
    if changed then
      -- (a block of 1, 2 made at the front of 1, 2, 2: made at 1 or at 3, the deleted slot's place is another)
      mock.song.sequencer:insert_sequence_at(1, 1)
      mock.song.sequencer:insert_sequence_at(2, 2)
      H.tick(2)
    end
    local undone = restore(peer, "slot-delete", record_of(previewed, done.result), "Kumi: undo (kp86)", done.result.epoch, done.result.layout)
    if changed then failure(undone, "stale") else ok(undone.result, json.encode(undone.error or {})) end
  end
end)

-- Cells moved in ways no shift or column move explains, and cleared lines: the undo finds Kumi's cells anywhere in the track.

local function rows_moved(t, map)
  local data = mock.data.patterns[1].tracks[t]
  local moved = {}
  for k, line in pairs(data.lines) do
    local to = map(k)
    if to and to >= 1 and to <= mock.data.patterns[1].lines then moved[to] = line end
  end
  data.lines = moved
  notify_lines(1, 3, 1)
  H.tick(2)
end

test("places, cell × Expand x2, many rows inserted, a row among like notes, a row and a column: refused changed, nothing written", function()
  for _, how in ipairs({ "expand", "rows 70", "like", "row and column" }) do
    local env, peer = setup(nil, { pattern_lines = 512 })
    local previewed, done = bass_notes(peer, 2, 5, "kp90")
    if how == "expand" then
      rows_moved(3, function(k) return 2 * k - 1 end)
    elseif how == "rows 70" then
      rows_moved(3, function(k) return k + 70 end)
    elseif how == "like" then
      rows_moved(3, function(k) return k >= 4 and k + 1 or k end) -- a row inserted among Kumi's four C-4
    else
      rows_moved(3, function(k) return k + 1 end)
      swap_note_columns(3, 1, 2)
    end
    local before = steps()
    local undone = restore(peer, "notes-write", record_of(previewed, done), "Kumi: undo n (kp91)", done.epoch, done.layout)
    failure(undone, "changed")
    eq(steps(), before, how)
  end
end)

local function bass_clear(peer, first, last, tag)
  local epoch, layout = place(peer)
  local input = { pattern = pattern_ref(epoch, 1), track = track_ref(epoch, 3), first = first, last = last }
  local previewed = preview(peer, "notes-clear", input)
  local done = apply(peer, "notes-clear", input, "Kumi: c (" .. tag .. ")", previewed.fence, epoch, layout)
  ok(done.result, json.encode(done.error or {}))
  return previewed, done.result
end

local function set_c4(lines_)
  for _, l in ipairs(lines_) do mock.ops.set_note(1, 3, l, 1, { note = 48, instrument = 0 }) end
  H.tick(2)
end

test("places, cleared lines × a row inserted above: refused changed when the lines around them moved, or with none, when the track was touched", function()
  for _, around in ipairs({ true, false }) do
    local env, peer = setup()
    set_c4(around and { 2, 3, 4, 5, 10 } or { 2, 3, 4, 5 })
    local previewed, done = bass_clear(peer, 2, 5, "kp92")
    rows_moved(3, function(k) return k + 1 end)
    local before = steps()
    failure(restore(peer, "notes-clear", record_of(previewed, done), "Kumi: undo c (kp93)", done.epoch, done.layout), "changed")
    eq(steps(), before)
  end
end)

test("places, cleared lines × nothing touched: put back where they were", function()
  local env, peer = setup()
  set_c4({ 2, 3, 4, 5 })
  local previewed, done = bass_clear(peer, 2, 5, "kp94")
  local undone = restore(peer, "notes-clear", record_of(previewed, done), "Kumi: undo c (kp95)", done.epoch, done.layout)
  ok(undone.result, json.encode(undone.error or {}))
  eq(c4_lines(), "2,3,4,5")
end)

test("places, value × given as text: a number is the send it names, fenced as a number; other text is refused", function()
  local env, peer = setup(nil, { send_tracks = 2 })
  local epoch, layout = kick_send(peer)
  local input = { ref = epoch .. ":device:1.2", param = "Receiver", valueString = "1" }
  local previewed = preview(peer, "device-param", input)
  mock.ops.insert_track(6, { kind = "send", name = "New send" })
  H.tick(2)
  failure(apply(peer, "device-param", input, "Kumi: Receiver (kp96)", previewed.fence, epoch, layout), "changed")
  local now, now_layout = place(peer)
  local response = call(peer, "change_preview", { kind = "device-param", input = { ref = now .. ":device:1.2", param = "Receiver", valueString = "Send 2" }, epoch = now, layout = now_layout })
  failure(response, "bad-params")
end)

test("places, track × a member made among those before a track taken out of its group: it goes back after the one it was after", function()
  local env, peer = setup(nil, { tracks = { { name = "Kick" }, { name = "Hat" }, { name = "Bass" }, { name = "Pad" }, { name = "Lead" },
    { kind = "group", name = "Group 06", members = 4 } } })
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  local epoch, layout = place(peer)
  local input = { ref = track_ref(epoch, 4), group = json.null } -- Pad, after Hat and Bass
  local previewed = preview(peer, "track-group", input)
  local done = apply(peer, "track-group", input, "Kumi: out (kp97)", previewed.fence, epoch, layout)
  ok(done.result, json.encode(done.error or {}))
  eq(names():sub(1, 23), "Kick,Pad,Hat,Bass,Lead,")
  mock.song:insert_track_at(4) -- a member made between Hat and Bass
  mock.ops.rename_track(4, "New")
  H.tick(2)
  local undone = restore(peer, "track-group", record_of(previewed, done.result), "Kumi: undo (kp98)", done.result.epoch, done.result.layout)
  ok(undone.result, json.encode(undone.error or {}))
  ok(names():find("Kick,Hat,New,Bass,Pad,Lead", 1, true), names())
end)

test("places, value × Dest. Track given as text is a track counted from 1, as Renoise reads it; text without a number is refused", function()
  local env, peer = setup()
  routing_catalog()
  mock.song:track(1):insert_device_at("Audio/Effects/Native/*LFO", 2)
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  local epoch, layout = place(peer)
  local input = { ref = epoch .. ":device:1.2", param = "Dest. Track", valueString = "3: Bass" }
  local previewed = preview(peer, "device-param", input)
  local done = apply(peer, "device-param", input, "Kumi: aim (kp99)", previewed.fence, epoch, layout)
  ok(done.result, json.encode(done.error or {}))
  eq(mock.data.tracks[1].devices[2].params[1].value, 2, "Bass, the third track: the value 2")
  for _, text in ipairs({ "None", "-1", "0" }) do
    local now, now_layout = place(peer)
    failure(call(peer, "change_preview", { kind = "device-param", input = { ref = now .. ":device:1.2", param = "Dest. Track", valueString = text }, epoch = now, layout = now_layout }), "bad-params")
  end
end)

test("places, nested groups × the first member of a group that is a group's first: it goes back first in its own group", function()
  local env, peer = setup(nil, { tracks = { { name = "Kick" }, { name = "A" }, { name = "B" }, { kind = "group", name = "Inner", members = 2 },
    { kind = "group", name = "Outer", members = 1 } } })
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  eq(order(), "Kick,A(in),B(in),Inner[G](in),Outer[G],Master,Send 1")
  local previewed, done = delete_track(peer, 2, "Kumi: d (kpa0)")
  local undone = restore(peer, "track-delete", record_of(previewed, done), "Kumi: undo d (kpa1)", done.epoch, done.layout)
  ok(undone.result, json.encode(undone.error or {}))
  eq(order(), "Kick,A(in),B(in),Inner[G](in),Outer[G],Master,Send 1")
  eq(mock.song:track(2).group_parent.name, "Inner")
end)

-- Touch rules: a notes change's undo refuses when lines at or above its region were touched since (any line, for lines it only
-- emptied); touches below, Kumi's own writes and the producer's undo are no touch.

test("touches × a run of like notes across Kumi's lines shifted one row: refused, though every place holds what Kumi wrote", function()
  local env, peer = setup(nil, { pattern_lines = 64 })
  set_c4({ 1, 2, 5, 6 })
  local previewed, done = bass_notes(peer, 3, 4, "kpb0") -- Kumi's C-4 make a run of six with the producer's
  rows_moved(3, function(k) return k + 1 end)
  failure(restore(peer, "notes-write", record_of(previewed, done), "Kumi: undo n (kpb1)", done.epoch, done.layout), "changed")
end)

test("touches × an edit anywhere in the pattern since, even far below Kumi's lines: refused, nothing written", function()
  local env, peer = setup(nil, { pattern_lines = 64 })
  local previewed, done = bass_notes(peer, 2, 4, "kpb2")
  mock.song:pattern(1):track(3):line(20):note_column(1).note_string = "E-4"
  failure(restore(peer, "notes-write", record_of(previewed, done), "Kumi: undo n (kpb3)", done.epoch, done.layout), "changed")
  eq(c4_lines(), "2,3,4")
end)

test("touches × lines Kumi emptied with lines on one side only, a row inserted between: refused", function()
  local env, peer = setup(nil, { pattern_lines = 64 })
  set_c4({ 2, 3, 10 })
  local previewed, done = bass_clear(peer, 2, 3, "kpb4")
  rows_moved(3, function(k) return k >= 5 and k + 1 or k end) -- a row made at 5, between the emptied lines and the C-4 at 10
  failure(restore(peer, "notes-clear", record_of(previewed, done), "Kumi: undo c (kpb5)", done.epoch, done.layout), "changed")
end)

test("touches × a notes change on a track Kumi deleted and put back since: refused (Kumi's put-back wrote the pattern)", function()
  local env, peer = setup()
  local previewed, done = bass_notes(peer, 2, 4, "kpb6")
  local deleted, gone = delete_track(peer, 3, "Kumi: d (kpb7)")
  local back = restore(peer, "track-delete", record_of(deleted, gone), "Kumi: undo d (kpb8)", gone.epoch, gone.layout)
  ok(back.result, json.encode(back.error or {}))
  failure(restore(peer, "notes-write", record_of(previewed, done), "Kumi: undo n (kpb9)", back.result.epoch, back.result.layout), "changed")
  eq(c4_lines(), "2,3,4")
end)

test("places, slot × patterns repeating every two: a block made or deleted at the front can't say where a mute went: left out, said", function()
  for _, act in ipairs({
    function() mock.song.sequencer:insert_sequence_at(1, 1); mock.song.sequencer:insert_sequence_at(2, 2) end,
    function() mock.song.sequencer:delete_sequence_at(1); mock.song.sequencer:delete_sequence_at(1) end,
  }) do
    local muted, note = mute_in_slot_3_after({ 1, 2, 1, 2, 1, 2 }, act, "kpc0")
    eq(muted, "")
    ok(note and note:find("isn't put back", 1, true), tostring(note))
  end
end)

test("touches × lines Kumi emptied: a line changed in another track of the pattern refuses the undo (a row inserted in the whole pattern shows only there)", function()
  local env, peer = setup(nil, { pattern_lines = 64 })
  set_c4({ 2, 3 })
  local previewed, done = bass_clear(peer, 2, 3, "kpc2")
  mock.song:pattern(1):track(1):line(30):note_column(1).note_string = "C-3" -- Kick, the row inserted in the pattern seen there
  failure(restore(peer, "notes-clear", record_of(previewed, done), "Kumi: undo c (kpc3)", done.epoch, done.layout), "changed")
end)

test("places, value × Dest. Track None is the value -1: set through value, and an undo of an aim made from None puts None back", function()
  local env, peer = setup()
  routing_catalog()
  mock.song:track(1):insert_device_at("Audio/Effects/Native/*LFO", 2)
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  local previewed, done = set_param(peer, place(peer) .. ":device:1.2", { param = "Dest. Track", value = 2 }, "Kumi: aim (kpc4)")
  ok(done.result, json.encode(done.error or {}))
  local undone = restore(peer, "device-param", record_of(previewed, done.result), "Kumi: undo (kpc5)", done.result.epoch, done.result.layout)
  ok(undone.result, json.encode(undone.error or {}))
  eq(mock.data.tracks[1].devices[2].params[1].value, -1, "None again")
  local _, again = set_param(peer, place(peer) .. ":device:1.2", { param = "Dest. Track", value = -1 }, "Kumi: none (kpc6)")
  ok(again.result, json.encode(again.error or {}))
end)

-- MUST 5–7 of the review at 7cd4cf4e: a shift that shows only below Kumi's lines, Renoise's undo past Kumi's step to an older
-- row insert, and a sliced undo with the producer between its steps.

local function copy(v)
  if type(v) ~= "table" then return v end
  local out = {}
  for k, x in pairs(v) do out[k] = copy(x) end
  return out
end

local function line_text(t, k)
  local line = mock.song:pattern(1):track(t):line(k)
  local parts = {}
  for c = 1, 12 do
    local n = line:note_column(c)
    parts[#parts + 1] = n.note_string .. n.instrument_string .. n.volume_string .. n.panning_string .. n.delay_string
      .. n.effect_number_string .. n.effect_amount_string
  end
  for c = 1, 8 do
    local e = line:effect_column(c)
    parts[#parts + 1] = e.number_string .. e.amount_string
  end
  return table.concat(parts, "|")
end

-- Track `t`'s lines in pattern 1 set to `to`, as Renoise's undo and redo do it: a line notifier for each line whose content
-- changed (lab).
local function set_lines(t, to)
  local n = mock.data.patterns[1].lines
  local old = {}
  for k = 1, n do old[k] = line_text(t, k) end
  mock.data.patterns[1].tracks[t].lines = copy(to)
  for k = 1, n do
    if line_text(t, k) ~= old[k] then notify_lines(1, t, k) end
  end
end

-- An edit of the producer's on track `t` as one undo step of Renoise's.
local function producer_step(t, edit)
  local before = copy(mock.data.patterns[1].tracks[t].lines)
  edit()
  local after = copy(mock.data.patterns[1].tracks[t].lines)
  mock.record(function() set_lines(t, before) end, function() set_lines(t, after) end)
  H.tick(2)
end

-- A row inserted or deleted at `r` in the pattern editor (in note column `column` alone, or the whole track), as Renoise does it:
-- one undo step, and a line notifier only for each line whose content changed (lab).
local function shift_rows(t, r, how, column)
  local pattern = mock.data.patterns[1]
  local data = pattern.tracks[t]
  local n = pattern.lines
  local before = copy(data.lines)
  local function from_of(k)
    if how == "insert" then
      if k < r then return k elseif k == r then return nil end
      return k - 1
    end
    if k < r then return k elseif k < n then return k + 1 end
    return nil
  end
  local after = {}
  for k = 1, n do
    local f = from_of(k)
    if column then
      local line = copy(before[k] or { notes = {}, effects = {} })
      line.notes = line.notes or {}
      line.notes[column] = f and before[f] and before[f].notes and copy(before[f].notes[column]) or nil
      after[k] = line
    else
      after[k] = f and copy(before[f]) or nil
    end
  end
  set_lines(t, after)
  mock.record(function() set_lines(t, before) end, function() set_lines(t, after) end)
  H.tick(2)
end

local function bass_column(first, last, col)
  local out = {}
  for k = first, last do
    local s = mock.song:pattern(1):track(3):line(k):note_column(col or 1).note_string
    if s ~= "---" then out[#out + 1] = k .. ":" .. s end
  end
  return table.concat(out, " ")
end

local function bass_set(lines_, note, col)
  for _, l in ipairs(lines_) do mock.ops.set_note(1, 3, l, col or 1, { note = note, instrument = 0 }) end
  H.tick(2)
end

local C4, D4, E4, F4, G4 = 48, 50, 52, 53, 55

test("review MUST 5: a row deleted inside a run of like notes that goes on below Kumi's: refused, the producer's notes as they are", function()
  for _, at in ipairs({ 98, 101 }) do
    local env, peer = setup(nil, { pattern_lines = 128 })
    bass_set({ 96, 97, 98, 99, 104, 105, 106, 107 }, C4)
    local previewed, done = bass_notes(peer, 100, 103, "km5a")
    shift_rows(3, at, "delete") -- (only 107 changes)
    local now = bass_column(90, 115)
    failure(restore(peer, "notes-write", record_of(previewed, done), "Kumi: undo n (km5b)", done.epoch, done.layout), "changed")
    eq(bass_column(90, 115), now, "nothing written (row deleted at " .. at .. ")")
  end
end)

test("review MUST 5: one like note just below Kumi's, Kumi's first row deleted: refused, the producer's note kept", function()
  local env, peer = setup(nil, { pattern_lines = 128 })
  bass_set({ 104 }, C4)
  local previewed, done = bass_notes(peer, 100, 103, "km5c")
  shift_rows(3, 100, "delete") -- (only 104 changes)
  failure(restore(peer, "notes-write", record_of(previewed, done), "Kumi: undo n (km5d)", done.epoch, done.layout), "changed")
  eq(bass_column(90, 115), "100:C-4 101:C-4 102:C-4 103:C-4")
end)

test("review MUST 5: a row inserted among the lines a write emptied, the empty stretch going on below: refused, nothing put on the wrong rows", function()
  for _, at in ipairs({ 101, 102 }) do
    local env, peer = setup(nil, { pattern_lines = 128 })
    bass_set({ 101 }, E4)
    bass_set({ 102 }, F4)
    bass_set({ 103 }, G4)
    bass_set({ 110 }, D4)
    local epoch, layout = place(peer)
    local lines = { { line = 99, notes = { { col = 1, note = "C-4", instrument = "00" } } } }
    for l = 101, 103 do lines[#lines + 1] = { line = l - 1, notes = { { col = 1, note = "---", instrument = ".." } } } end
    local input = { pattern = pattern_ref(epoch, 1), track = track_ref(epoch, 3), lines = lines }
    local previewed = preview(peer, "notes-write", input)
    local done = apply(peer, "notes-write", input, "Kumi: n (km5e)", previewed.fence, epoch, layout)
    ok(done.result, json.encode(done.error or {}))
    shift_rows(3, at, "insert") -- (only 110 and 111 change)
    local now = bass_column(90, 115)
    failure(restore(peer, "notes-write", record_of(previewed, done.result), "Kumi: undo n (km5f)", done.result.epoch, done.result.layout), "changed")
    eq(bass_column(90, 115), now, "nothing written (row inserted at " .. at .. ")")
  end
end)

test("review MUST 5: a row deleted in Kumi's column alone, where only that column's run goes on below: refused", function()
  local env, peer = setup(nil, { pattern_lines = 128 })
  bass_set({ 104, 105 }, C4, 2)
  bass_set({ 104 }, D4, 1) -- (the whole line differs at 104: only column 2's run reaches 105)
  local epoch, layout = place(peer)
  local lines = {}
  for l = 100, 103 do lines[#lines + 1] = { line = l - 1, notes = { { col = 2, note = "C-4", instrument = "00" } } } end
  local input = { pattern = pattern_ref(epoch, 1), track = track_ref(epoch, 3), lines = lines }
  local previewed = preview(peer, "notes-write", input)
  local done = apply(peer, "notes-write", input, "Kumi: n (km5g)", previewed.fence, epoch, layout)
  ok(done.result, json.encode(done.error or {}))
  shift_rows(3, 100, "delete", 2) -- (only 105 changes)
  failure(restore(peer, "notes-write", record_of(previewed, done.result), "Kumi: undo n (km5h)", done.result.epoch, done.result.layout), "changed")
  eq(bass_column(90, 115, 2), "100:C-4 101:C-4 102:C-4 103:C-4 104:C-4")
end)

test("plain rule: an untouched pattern undoes exactly, every cell as it was", function()
  local env, peer = setup(nil, { pattern_lines = 128 })
  bass_set({ 99, 104 }, C4)
  bass_set({ 101 }, E4, 2)
  local before = {}
  for k = 90, 115 do before[k] = line_text(3, k) end
  local previewed, done = bass_notes(peer, 100, 103, "km5i")
  local undone = restore(peer, "notes-write", record_of(previewed, done), "Kumi: undo n (km5j)", done.epoch, done.layout)
  ok(undone.result, json.encode(undone.error or {}))
  for k = 90, 115 do eq(line_text(3, k), before[k], "line " .. k) end
end)

test("plain rule: an edit in another pattern, or Kumi's change there, doesn't block the undo", function()
  local env, peer = setup(nil, { pattern_lines = 128 })
  local previewed, done = bass_notes(peer, 100, 103, "km5k")
  mock.song:pattern(2):track(3):line(100):note_column(1).note_string = "E-4"
  local epoch, layout = place(peer)
  local input = { pattern = pattern_ref(epoch, 3), track = track_ref(epoch, 3), lines = { { line = 0, notes = { { col = 1, note = "D-4", instrument = "00" } } } } }
  local other = preview(peer, "notes-write", input)
  ok(apply(peer, "notes-write", input, "Kumi: n (km5l)", other.fence, epoch, layout).result)
  H.tick(2)
  local undone = restore(peer, "notes-write", record_of(previewed, done), "Kumi: undo n (km5m)", done.epoch, done.layout)
  ok(undone.result, json.encode(undone.error or {}))
  eq(bass_column(90, 115), "")
end)

test("review MUST 6: Renoise's undo past Kumi's step to an older row insert puts like lines back: unsure, not redone, and Kumi's undo refuses", function()
  local env, peer = setup(nil, { pattern_lines = 128 })
  bass_set({ 100, 101, 102, 103 }, C4)
  shift_rows(3, 50, "insert") -- (the producer's C-4 on 101-104)
  local previewed, done = bass_notes(peer, 100, 103, "km6a") -- (only 100 changes)
  H.tick(2)
  H.events(peer, "undone_in_daw")
  mock.song:undo()
  H.tick(3)
  drain(peer)
  local told = H.events(peer, "undone_in_daw")
  eq(told[#told].data.how, "undo")
  mock.song:undo() -- (the row insert: the producer's C-4 back on 100-103, as Kumi wrote them)
  H.tick(3)
  drain(peer)
  told = H.events(peer, "undone_in_daw")
  eq(#told, 1)
  eq(told[1].data.how, "unsure")
  bass_set({ 120 }, E4) -- (far below: Renoise's redo is gone)
  failure(restore(peer, "notes-write", record_of(previewed, done), "Kumi: undo n (km6b)", done.epoch, done.layout), "changed")
  eq(bass_column(90, 115), "100:C-4 101:C-4 102:C-4 103:C-4")
end)

-- (#393) Renoise's redo is of Kumi's step for sure when that step was the newest at the undo and nothing is left to redo after it:
-- an undo past it would have left Kumi's step to redo.
test("#393: the producer's redo of Kumi's lines, the newest step: told redone, and Kumi's undo takes them back", function()
  local env, peer = setup(nil, { pattern_lines = 128 })
  local previewed, done = bass_notes(peer, 10, 13, "km6c")
  H.tick(2)
  H.events(peer, "undone_in_daw")
  mock.song:undo()
  H.tick(3)
  drain(peer)
  eq(H.events(peer, "undone_in_daw")[1].data.how, "undo")
  mock.song:redo()
  H.tick(3)
  drain(peer)
  eq(H.events(peer, "undone_in_daw")[1].data.how, "redo")
  local undone = restore(peer, "notes-write", record_of(previewed, done), "Kumi: undo n (km6d)", done.epoch, done.layout)
  ok(undone.result, json.encode(undone.error or {}))
  eq(bass_column(1, 30), "")
end)

test("#393: the producer's redo of Kumi's lines, then an edit of them: Kumi's undo refuses, the edit kept", function()
  local env, peer = setup(nil, { pattern_lines = 128 })
  local previewed, done = bass_notes(peer, 10, 13, "km6e")
  H.tick(2)
  mock.song:undo()
  H.tick(3)
  mock.song:redo()
  H.tick(3)
  drain(peer)
  local told = H.events(peer, "undone_in_daw")
  eq(#told, 2)
  eq(told[2].data.how, "redo")
  bass_set({ 12 }, E4)
  failure(restore(peer, "notes-write", record_of(previewed, done), "Kumi: undo n (km6f)", done.epoch, done.layout), "changed")
  eq(bass_column(1, 30), "10:C-4 11:C-4 12:E-4 13:C-4")
end)

test("#393: Kumi's step isn't the newest: the producer's redo of it leaves a step to redo, unsure, and Kumi's undo refuses", function()
  local env, peer = setup(nil, { pattern_lines = 128 })
  local previewed, done = bass_notes(peer, 10, 13, "km6g")
  H.tick(2)
  mock.song:pattern(2):track(3):line(100):note_column(1).note_string = "E-4" -- (the producer's step after Kumi's)
  H.tick(2)
  H.events(peer, "undone_in_daw")
  mock.song:undo()
  H.tick(3)
  mock.song:undo()
  H.tick(3)
  drain(peer)
  eq(H.events(peer, "undone_in_daw")[1].data.how, "undo")
  mock.song:redo()
  H.tick(3)
  drain(peer)
  eq(H.events(peer, "undone_in_daw")[1].data.how, "unsure")
  mock.song:redo() -- (the producer's step again: nothing left to redo, and Kumi's lines didn't change)
  H.tick(3)
  drain(peer)
  eq(#H.events(peer, "undone_in_daw"), 0)
  failure(restore(peer, "notes-write", record_of(previewed, done), "Kumi: undo n (km6h)", done.epoch, done.layout), "changed")
  eq(bass_column(1, 30), "10:C-4 11:C-4 12:C-4 13:C-4")
end)

test("#393: the producer's redo of Kumi's newest step, twice over: told redone each time, and Kumi's undo takes the lines back", function()
  local env, peer = setup(nil, { pattern_lines = 128 })
  local previewed, done = bass_notes(peer, 10, 13, "km6k")
  H.tick(2)
  for _ = 1, 2 do
    mock.song:undo()
    H.tick(3)
    mock.song:redo()
    H.tick(3)
  end
  local undone = restore(peer, "notes-write", record_of(previewed, done), "Kumi: undo n (km6l)", done.epoch, done.layout)
  ok(undone.result, json.encode(undone.error or {}))
  eq(bass_column(1, 30), "")
end)

test("#393: a track muted after Kumi's step isn't an edit (no step of Renoise's): the redo is told, and Kumi's undo works", function()
  local env, peer = setup(nil, { pattern_lines = 128 })
  local previewed, done = bass_notes(peer, 10, 13, "km6m")
  H.tick(2)
  mock.song:track(1):mute()
  H.tick(2)
  mock.song:undo()
  H.tick(3)
  mock.song:redo()
  H.tick(3)
  local undone = restore(peer, "notes-write", record_of(previewed, done), "Kumi: undo n (km6n)", done.epoch, done.layout)
  ok(undone.result, json.encode(undone.error or {}))
  eq(bass_column(1, 30), "")
end)

-- (#393) A later step of the producer's can leave the lines as before Kumi's change, and its redo leaves them as Kumi wrote them
-- with nothing left to redo: Renoise's undo and redo of that step mustn't lift the refusal, or Kumi's undo takes the producer's
-- lines.
local function cells_step(edit)
  mock.callback_begin()
  edit(function(l) return mock.song:pattern(1):track(3):line(l):note_column(1) end)
  mock.callback_end()
  H.tick(2)
end

local function still_refused(peer, previewed, done, tag, expected)
  failure(restore(peer, "notes-write", record_of(previewed, done), "Kumi: undo n (" .. tag .. ")", done.epoch, done.layout), "changed")
  eq(bass_column(90, 115), expected, "the producer's lines kept")
end

local function undo_and_redo_refused(peer, previewed, done, tag, before_redo)
  local expected = bass_column(90, 115)
  still_refused(peer, previewed, done, tag .. "a", expected)
  mock.song:undo()
  H.tick(3)
  eq(bass_column(90, 115), before_redo, "Renoise's undo")
  mock.song:redo()
  H.tick(3)
  eq(bass_column(90, 115), expected, "Renoise's redo")
  still_refused(peer, previewed, done, tag .. "b", expected)
end

test("#393: the producer clears Kumi's lines and moves their own like notes there, then undoes and redoes that: Kumi's undo refuses", function()
  local env, peer = setup(nil, { pattern_lines = 128 })
  bass_set({ 104, 105, 106, 107 }, C4)
  local previewed, done = bass_notes(peer, 100, 103, "km6o")
  H.tick(2)
  cells_step(function(column_at) for l = 100, 103 do column_at(l):clear() end end)
  cells_step(function(column_at)
    for l = 104, 107 do column_at(l):clear() end
    for l = 100, 103 do
      column_at(l).note_string = "C-4"
      column_at(l).instrument_string = "00"
    end
  end)
  undo_and_redo_refused(peer, previewed, done, "km6p", "104:C-4 105:C-4 106:C-4 107:C-4")
end)

test("#393: the producer clears Kumi's line and types the same note, then undoes and redoes that: Kumi's undo refuses", function()
  local env, peer = setup(nil, { pattern_lines = 128 })
  local previewed, done = bass_notes(peer, 100, 100, "km6q")
  H.tick(2)
  cells_step(function(column_at) column_at(100):clear() end)
  cells_step(function(column_at)
    column_at(100).note_string = "C-4"
    column_at(100).instrument_string = "00"
  end)
  undo_and_redo_refused(peer, previewed, done, "km6r", "")
end)

test("#393: over the producer's E-4, Kumi's C-4; the producer types E-4, then C-4, then undoes and redoes that: Kumi's undo refuses and E-4 doesn't come back", function()
  local env, peer = setup(nil, { pattern_lines = 128 })
  bass_set({ 100 }, E4)
  local previewed, done = bass_notes(peer, 100, 100, "km6s")
  H.tick(2)
  cells_step(function(column_at) column_at(100).note_string = "E-4" end)
  cells_step(function(column_at) column_at(100).note_string = "C-4" end)
  undo_and_redo_refused(peer, previewed, done, "km6t", "100:E-4")
end)

test("#393: Renoise's undo past Kumi's step to the producer's clear (unsure), then the producer redoes both: told redone, Kumi's undo works", function()
  local env, peer = setup(nil, { pattern_lines = 128 })
  local column = function() return mock.song:pattern(1):track(3):line(100):note_column(1) end
  local function step(edit) -- (one undo step of the producer's)
    mock.callback_begin()
    edit()
    mock.callback_end()
    H.tick(2)
  end
  step(function() -- (the producer's C-4, as Kumi will write it, then cleared)
    column().note_string = "C-4"
    column().instrument_string = "00"
  end)
  step(function() column():clear() end)
  local previewed, done = bass_notes(peer, 100, 100, "km6i")
  H.tick(2)
  H.events(peer, "undone_in_daw")
  mock.song:undo()
  H.tick(3)
  mock.song:undo() -- (the clear: the producer's C-4 back, as Kumi wrote it)
  H.tick(3)
  drain(peer)
  local told = H.events(peer, "undone_in_daw")
  eq(#told, 2)
  eq(told[2].data.how, "unsure")
  mock.song:redo() -- (the clear again: line 100 as before Kumi's step)
  H.tick(3)
  drain(peer)
  eq(H.events(peer, "undone_in_daw")[1].data.how, "undo")
  mock.song:redo() -- (Kumi's step, the newest)
  H.tick(3)
  drain(peer)
  eq(H.events(peer, "undone_in_daw")[1].data.how, "redo")
  eq(bass_column(90, 115), "100:C-4")
  local undone = restore(peer, "notes-write", record_of(previewed, done), "Kumi: undo n (km6j)", done.epoch, done.layout)
  ok(undone.result, json.encode(undone.error or {}))
  eq(bass_column(90, 115), "")
end)

-- (Kumi's C-4 on 100-111 undone in three steps of four; the producer's D-4 on 112-115; `disrupt` between steps 1 and 2)
local function between_steps(disrupt, check)
  local env, peer = setup({ patch = constants({ SLICE_WRITES = 30 }) }, { pattern_lines = 128 })
  bass_set({ 112, 113, 114, 115 }, D4)
  local previewed, done = bass_notes(peer, 100, 111, "km7a")
  mock.data.undo.stack, mock.data.undo.redo = {}, {}
  local id = send(peer, "change_restore", restore_params("notes-write", record_of(previewed, done), "Kumi: undo n (km7b)", done.epoch, done.layout))
  for _ = 1, 50 do
    mock.advance(10)
    if steps() > 0 then break end
  end
  eq(steps(), 1, "step 1 is written")
  disrupt()
  local response = wait(peer, id, 300, 20)
  failure(response, "uncertain")
  ok(response.error.message:find("Kumi stopped part way and wrote nothing further", 1, true), response.error.message)
  ok(not response.error.message:find("of 3 steps", 1, true), "no count: " .. response.error.message)
  local kumis = 0
  for _, step in ipairs(mock.data.undo.stack) do
    if step.desc:find("km7b", 1, true) then kumis = kumis + 1 end
  end
  eq(kumis, 1, "no step of Kumi's after the producer's change")
  check()
end

test("review MUST 7: a row deleted above between a sliced undo's steps: stopped uncertain, the producer's D-4 kept", function()
  between_steps(function() shift_rows(3, 50, "delete") end, function()
    eq(bass_column(90, 120), "103:C-4 104:C-4 105:C-4 106:C-4 107:C-4 108:C-4 109:C-4 110:C-4 111:D-4 112:D-4 113:D-4 114:D-4")
  end)
end)

test("review MUST 7: a row inserted above between a sliced undo's steps: stopped uncertain, nothing further written", function()
  between_steps(function() shift_rows(3, 50, "insert") end, function()
    eq(bass_column(90, 120), "105:C-4 106:C-4 107:C-4 108:C-4 109:C-4 110:C-4 111:C-4 112:C-4 113:D-4 114:D-4 115:D-4 116:D-4")
  end)
end)

test("review MUST 7: the producer types a note between a sliced undo's steps: stopped uncertain, the note kept", function()
  between_steps(function() bass_set({ 109 }, E4) end, function()
    eq(bass_column(108, 109), "108:C-4 109:E-4")
  end)
end)

-- The final verify at cefc5aeb: MUST 9 (Kumi's own writes hide a shift from an earlier change) and MUST 10 (Renoise's undo past
-- Kumi's step to an edit that also changed Kumi's lines). Under the plain rule, both refuse.

test("review MUST 9: Kumi's later write beside an earlier one, then a row deleted: the earlier undo refuses, nothing written", function()
  local env, peer = setup(nil, { pattern_lines = 128 })
  local previewed, done = bass_notes(peer, 100, 103, "km9a")
  bass_notes(peer, 104, 107, "km9b")
  shift_rows(3, 101, "delete") -- (only 107 changes)
  local now = bass_column(90, 115)
  failure(restore(peer, "notes-write", record_of(previewed, done), "Kumi: undo n (km9c)", done.epoch, done.layout), "changed")
  eq(bass_column(90, 115), now)
end)

test("review MUST 9: Kumi clears the producer's notes, writes beside them, and undoes the clear: refused, and so is the write's undo after a row deleted", function()
  local env, peer = setup(nil, { pattern_lines = 128 })
  bass_set({ 104, 105, 106, 107 }, C4)
  local cleared, clear_done = bass_clear(peer, 104, 107, "km9d")
  local previewed, done = bass_notes(peer, 100, 103, "km9e")
  failure(restore(peer, "notes-clear", record_of(cleared, clear_done), "Kumi: undo c (km9f)", clear_done.epoch, clear_done.layout), "changed")
  shift_rows(3, 101, "delete")
  local now = bass_column(90, 115)
  failure(restore(peer, "notes-write", record_of(previewed, done), "Kumi: undo n (km9g)", done.epoch, done.layout), "changed")
  eq(bass_column(90, 115), now)
end)

test("review MUST 10: the producer clears Kumi's lines and another in one step, Kumi writes the same, two Cmd-Z (one tick apart or within one): refused, the producer's notes kept", function()
  for _, apart in ipairs({ true, false }) do
    local env, peer = setup(nil, { pattern_lines = 512 })
    bass_set({ 100, 101, 102, 103, 300 }, C4)
    producer_step(3, function()
      for _, l in ipairs({ 100, 101, 102, 103, 300 }) do mock.ops.set_note(1, 3, l, 1, {}) end
    end)
    local previewed, done = bass_notes(peer, 100, 103, "km10a")
    mock.song:undo()
    if apart then H.tick(3) end
    mock.song:undo()
    H.tick(3)
    eq(bass_column(90, 310), "100:C-4 101:C-4 102:C-4 103:C-4 300:C-4", "the producer's notes back")
    failure(restore(peer, "notes-write", record_of(previewed, done), "Kumi: undo n (km10b)", done.epoch, done.layout), "changed")
    eq(bass_column(90, 310), "100:C-4 101:C-4 102:C-4 103:C-4 300:C-4", "nothing written")
  end
end)

test("review MUST 10: the producer clears two columns of lines, Kumi writes the same in one, two Cmd-Z: refused, both columns kept", function()
  local env, peer = setup(nil, { pattern_lines = 128 })
  bass_set({ 100, 101, 102, 103 }, C4, 1)
  bass_set({ 100, 101, 102, 103 }, E4, 2)
  producer_step(3, function()
    for l = 100, 103 do
      mock.ops.set_note(1, 3, l, 1, {})
      mock.ops.set_note(1, 3, l, 2, {})
    end
  end)
  local previewed, done = bass_notes(peer, 100, 103, "km10c")
  mock.song:undo()
  H.tick(3)
  mock.song:undo()
  H.tick(3)
  failure(restore(peer, "notes-write", record_of(previewed, done), "Kumi: undo n (km10d)", done.epoch, done.layout), "changed")
  eq(bass_column(90, 115, 1), "100:C-4 101:C-4 102:C-4 103:C-4")
  eq(bass_column(90, 115, 2), "100:E-4 101:E-4 102:E-4 103:E-4")
end)

-- The verify at 74c206e1: MUST 11 (a sliced undo's first step went unchecked), SHOULD 4 (a discard after the pattern changed),
-- and aliases (lab L1: an edit through an alias notifies the pattern it shows; making a track an alias notifies no line).

-- (Kumi's C-4 on 100-111 undone in three steps; the producer's D-4 on 112-115; `disrupt` after the request was handled, its
-- check passed, and before its first step runs on a later turn)
local function before_first_step(disrupt, check)
  local env, peer = setup({ patch = constants({ SLICE_WRITES = 30 }) }, { pattern_lines = 128 })
  bass_set({ 112, 113, 114, 115 }, D4)
  local previewed, done = bass_notes(peer, 100, 111, "km11a")
  local before = steps()
  local id = send(peer, "change_restore", restore_params("notes-write", record_of(previewed, done), "Kumi: undo n (km11b)", done.epoch, done.layout))
  eq(steps(), before, "the first step isn't written inside the request")
  disrupt()
  failure(wait(peer, id, 300, 20), "changed")
  for _, step in ipairs(mock.data.undo.stack) do ok(not step.desc:find("km11b", 1, true), "a step of Kumi's undo: " .. step.desc) end
  check()
end

test("review MUST 11: a row deleted before a sliced undo's first step: refused, nothing written", function()
  before_first_step(function() shift_rows(3, 50, "delete") end, function()
    eq(bass_column(90, 120), "99:C-4 100:C-4 101:C-4 102:C-4 103:C-4 104:C-4 105:C-4 106:C-4 107:C-4 108:C-4 109:C-4 110:C-4 "
      .. "111:D-4 112:D-4 113:D-4 114:D-4")
  end)
end)

test("review MUST 11: a note typed in Kumi's lines before a sliced undo's first step: refused, the note kept", function()
  before_first_step(function() bass_set({ 101 }, E4) end, function()
    eq(bass_column(100, 102), "100:C-4 101:E-4 102:C-4")
  end)
end)

test("review MUST 11: another tool's notes in the pattern before a sliced undo's first step: refused, its notes kept", function()
  before_first_step(function() bass_set({ 100, 101, 102, 103 }, D4) end, function()
    eq(bass_column(100, 104), "100:D-4 101:D-4 102:D-4 103:D-4 104:C-4")
  end)
end)

test("review SHOULD 4: a discard of a notes change after a row inserted refuses too: lines have no identity but their place", function()
  local env, peer = setup(nil, { pattern_lines = 128 })
  bass_set({ 104 }, C4)
  local previewed, done = bass_notes(peer, 100, 103, "km12a")
  shift_rows(3, 50, "insert")
  local now = bass_column(90, 115)
  local params = restore_params("notes-write", record_of(previewed, done), "Kumi: undo n (km12b)", done.epoch, done.layout)
  params.discard = true
  local response = call(peer, "change_restore", params, 800)
  failure(response, "changed")
  ok(response.error.message:find("can't tell where its lines are now", 1, true), response.error.message)
  eq(bass_column(90, 115), now, "nothing written")
end)

test("aliases: Kumi doesn't write lines through an alias (they are the pattern's it shows)", function()
  local env, peer = setup(nil, { pattern_lines = 128 })
  mock.ops.alias(2, 3, 1)
  H.tick(2)
  local epoch, layout = place(peer)
  local input = { pattern = pattern_ref(epoch, 2), track = track_ref(epoch, 3), lines = { { line = 0, notes = { { col = 1, note = "C-4", instrument = "00" } } } } }
  local response = call(peer, "change_preview", { kind = "notes-write", input = input, epoch = epoch, layout = layout })
  ok(response.error and response.error.message:find("alias", 1, true), json.encode(response))
end)

test("aliases (lab L1): the producer's edit through an alias of Kumi's lines refuses Kumi's undo", function()
  local env, peer = setup(nil, { pattern_lines = 128 })
  mock.ops.alias(2, 3, 1)
  H.tick(2)
  local previewed, done = bass_notes(peer, 100, 103, "km13a")
  mock.song:pattern(2):track(3):line(104):note_column(1).note_string = "E-4" -- (through the alias: pattern 1's line)
  H.tick(2)
  failure(restore(peer, "notes-write", record_of(previewed, done), "Kumi: undo n (km13b)", done.epoch, done.layout), "changed")
  eq(bass_column(100, 104), "100:C-4 101:C-4 102:C-4 103:C-4 104:E-4")
end)

test("aliases: Kumi's track in the pattern made an alias and back since refuses Kumi's undo (no line notifies it)", function()
  local env, peer = setup(nil, { pattern_lines = 128 })
  local previewed, done = bass_notes(peer, 100, 103, "km13c")
  mock.song:pattern(1):track(3).alias_pattern_index = 3
  mock.song:pattern(1):track(3).alias_pattern_index = 0
  H.tick(2)
  failure(restore(peer, "notes-write", record_of(previewed, done), "Kumi: undo n (km13d)", done.epoch, done.layout), "changed")
end)
