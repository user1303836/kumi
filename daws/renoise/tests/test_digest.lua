-- The song as records for catch-up and watch_me (M6, digest.lua) against the mock Renoise: tracks with their devices,
-- patterns with each track's lines summed up, instruments, the song's own values; pages that add up whatever their
-- size; a cursor from another epoch refused; the lines budget; automated parameters left to their automation.
local T = ...
local mock = T.mock
local json = T.load_payload_module("json")
local test, eq, ok, same = T.test, T.eq, T.ok, T.same
local H = T.harness

local function song_spec(extra)
  local spec = {
    name = "Demo", file_name = "/s/demo.xrns", bpm = 128, lpb = 4, mixer_device = true,
    tracks = {
      { name = "Kick", color = { 255, 0, 0 } },
      { name = "Track 02" },
      { name = "Drums", kind = "group", members = 2 },
      { name = "Bass", note_columns = 2 },
    },
    send_tracks = 1,
    patterns = 3, pattern_lines = 16, sequence_length = 4, sequence = { 1, 2, 1, 3 },
    instruments = 2,
    instrument_specs = { { name = "Kit", samples = 2, phrases = 1 }, { name = "", samples = 1 } },
  }
  for k, v in pairs(extra or {}) do spec[k] = v end
  return spec
end

local next_id = 5000

local function drain(peer)
  repeat
    local taken = peer:take()
    if #taken > 0 then H.send(peer, { type = "credit", bytes = #taken }) end
  until #taken == 0
end

local function call(peer, method, params)
  next_id = next_id + 1
  local id = next_id
  H.send(peer, { type = "request", id = id, method = method, params = params or json.object() })
  for _ = 1, 2000 do
    drain(peer)
    for _, line in ipairs(peer:frames(nil)) do
      local frame = assert(json.decode(line))
      if frame.type == "response" and frame.id == id then return frame end
    end
    mock.advance(10)
  end
  error("no answer to " .. method)
end

local function setup(extra)
  local env = H.setup({ song = song_spec(extra) })
  local peer = H.connect(env)
  H.frames(peer)
  local data = mock.data
  data.tracks[1].devices[2] = { name = "EQ 5", params = { { name = "Gain", value = 0.5 }, { name = "Freq", value = 0.25 } } }
  data.tracks[4].devices[2] = { name = "Distortion", path = "Audio/Effects/Native/Distortion 2", params = { { name = "Drive", value = 0.1 } } }
  return env, peer
end

-- Every page: the first one's answer, and all the records.
local function whole(peer, params)
  params = params or {}
  local first = call(peer, "digest", params)
  ok(first.result, first.error and first.error.message)
  local records, pages = {}, 1
  for _, record in ipairs(first.result.records) do records[#records + 1] = record end
  local cursor = first.result.nextCursor
  while cursor do
    local next_params = { cursor = cursor }
    for k, v in pairs(params) do next_params[k] = v end
    local page = call(peer, "digest", next_params)
    ok(page.result, page.error and page.error.message)
    eq(page.result.project, nil, "only the first page names the song")
    for _, record in ipairs(page.result.records) do records[#records + 1] = record end
    cursor = page.result.nextCursor
    pages = pages + 1
  end
  return first.result, records, pages
end

local function find(records, kind, o)
  for _, record in ipairs(records) do
    if record.k == kind and record.o == o then return record end
  end
  error("no " .. kind .. " " .. o)
end

test("digest: the song is its tracks with their devices, then its patterns and instruments, with its own values", function()
  local _, peer = setup()
  mock.ops.set_note(2, 1, 3, 1, { note = 48, instrument = 0 })
  local first, records = whole(peer)
  eq(first.project.name, "demo.xrns")
  eq(first.project.path, "/s/demo.xrns")
  ok(#first.project.id == 32, "the song's id")
  ok(first.project.identity:match("^4242:%d+$"), first.project.identity)
  eq(first.facts.bpm, 128)
  eq(first.facts.lpb, 4)
  eq(first.facts.sequence.slots, 4)
  eq(first.summaries.device, "devices")
  local kinds = {}
  for _, record in ipairs(records) do kinds[#kinds + 1] = record.k .. (record.o or "") end
  eq(table.concat(kinds, ","), "track1,device2,track2,track3,track4,device2,track5,track6,pattern1,pattern2,pattern3,instrument1,instrument2")
  local kick = find(records, "track", 1)
  eq(kick.n, "Kick")
  eq(kick.dn, nil, "a name of its own")
  eq(kick.f.kind, "sequencer")
  eq(kick.f.color, "#ff0000")
  eq(kick.f.group.name, "Drums")
  eq(kick.f.mixer.mute, "active")
  eq(kick.f.devices.count, 1, "the mixer isn't counted: its parameters are the track's own")
  eq(find(records, "track", 2).dn, true, "Renoise's own numbering tells nothing")
  local eq5 = records[2]
  eq(eq5.p, "track#/1")
  eq(eq5.n, "EQ 5")
  eq(eq5.f.path, "Audio/Effects/Native/EQ 5")
  eq(eq5.f.active, true)
  eq(eq5.f.parameters.count, 2)
  ok(eq5.v == nil and eq5.r == nil, "no values or refs unless asked")
  local intro = find(records, "pattern", 2)
  eq(intro.n, "Pattern 01")
  eq(intro.dn, true)
  eq(intro.f.length, 16)
  ok(intro.f.lines.Kick:match("^1 notes, 0 effects #%x+$"), intro.f.lines.Kick)
  eq(find(records, "pattern", 1).f.lines.Kick, nil, "an empty pattern track has no line sum")
  eq(find(records, "instrument", 1).n, "Kit")
  eq(find(records, "instrument", 1).f.samples.count, 2)
  eq(find(records, "instrument", 2).n, "Instrument 01")
  eq(find(records, "instrument", 2).dn, true)
end)

test("digest: what the producer changes moves only its own record", function()
  local _, peer = setup()
  local _, before = whole(peer)
  mock.ops.set_note(3, 4, 5, 1, { note = 36, instrument = 1 })
  mock.data.tracks[1].devices[2].params[1].value = 0.75
  mock.data.tracks[2].name = "Hats"
  mock.data.instruments[1].name = "Kit 2"
  mock.data.transport_fields.bpm = 132
  local first, after = whole(peer)
  eq(#after, #before)
  local moved = {}
  for i = 1, #after do
    if json.encode(after[i]) ~= json.encode(before[i]) then moved[#moved + 1] = after[i].k .. after[i].o end
  end
  eq(table.concat(moved, ","), "device2,track2,pattern3,instrument1")
  eq(find(after, "track", 2).dn, nil, "named by the producer now")
  ok(find(after, "pattern", 3).f.lines.Bass:match("^1 notes"), "the Bass track's lines of the third pattern")
  eq(first.facts.bpm, 132)
end)

test("digest: watch_me's values and refs, and a quick look that reads no lines", function()
  local _, peer = setup()
  mock.ops.set_note(1, 1, 1, 1, { note = 48, instrument = 0 })
  local first, records = whole(peer, { values = true, content = false })
  local eq5 = records[2]
  same(eq5.v, { 0.5, 0.25 })
  ok(eq5.r:match("^%x+:device:1%.2$"), eq5.r)
  eq(find(records, "pattern", 1).f.lines.Kick, json.null, "not read: never compared")
  ok(first.bounded.pattern, "says lines weren't read")
end)

test("digest: an automated parameter follows its automation, so it is left out", function()
  local _, peer = setup()
  local _, before = whole(peer, { values = true })
  local param = mock.data.tracks[1].devices[2].params[1]
  param.automated = true
  local _, automated = whole(peer, { values = true })
  local eq5 = automated[2]
  eq(eq5.v[1], json.null, "its value is the automation's, not one the producer set")
  param.value = 0.9
  local _, moved = whole(peer, { values = true })
  eq(moved[2].f.parameters.parametersHash, eq5.f.parameters.parametersHash, "moving with its automation is no change")
  ok(before[2].f.parameters.parametersHash ~= eq5.f.parameters.parametersHash, "taken out of the fingerprint")
end)

test("digest: pages of any size add up to the same digest, and a cursor from another epoch is refused", function()
  local env, peer = setup()
  local digest_module = env.payload().digest
  mock.ops.set_note(2, 1, 3, 1, { note = 48, instrument = 0 })
  local _, all, pages = whole(peer)
  eq(pages, 1)
  local kept = digest_module.PAGE_BYTES
  digest_module.PAGE_BYTES = 300
  local _, paged, many = whole(peer)
  ok(many > 5, "pages: " .. many)
  same(paged, all)
  local first = call(peer, "digest", {}).result
  ok(first.nextCursor, "a cursor")
  -- (a song switch moves the epoch: the pages wouldn't belong together)
  mock.load_song(song_spec())
  local stale = call(peer, "digest", { cursor = first.nextCursor })
  eq(stale.error and stale.error.code, "stale")
  local bad = call(peer, "digest", { cursor = "g1.zz" })
  eq(bad.error and bad.error.code, "bad-params")
  digest_module.PAGE_BYTES = kept
end)

test("digest: past the lines budget a pattern's lines are left uncompared, and the digest says so", function()
  local env, peer = setup()
  local digest_module = env.payload().digest
  for p = 1, 3 do mock.ops.set_note(p, 1, 1, 1, { note = 48, instrument = 0 }) end
  local kept = digest_module.LINES_BUDGET
  digest_module.LINES_BUDGET = 20
  local first, records = whole(peer)
  digest_module.LINES_BUDGET = kept
  ok(find(records, "pattern", 1).f.lines.Kick:match("notes"), "the first one read")
  eq(find(records, "pattern", 2).f.lines.Kick, json.null)
  ok(first.bounded.lines:match("more than 20 pattern lines"), first.bounded.lines)
end)

test("digest: a big song comes in pages inside the frame bound, read a slice at a time", function()
  local tracks = {}
  for t = 1, 32 do tracks[t] = { name = "T" .. t, note_columns = 2 } end
  local env = H.setup({ song = { name = "Big", file_name = "/s/big.xrns", mixer_device = true, tracks = tracks, patterns = 64,
    pattern_lines = 64, instruments = 8 } })
  local peer = H.connect(env)
  H.frames(peer)
  for p = 1, 64 do
    for t = 1, 32 do
      for l = 1, 64, 4 do mock.ops.set_note(p, t, l, 1, { note = 36 + (l % 24), instrument = 0, volume = 64 }) end
    end
  end
  -- (the mock's clock is the tests' own, so this measures nothing of Renoise's time: the lab does)
  local first, records, pages = whole(peer)
  local largest = #json.encode(first)
  ok(pages >= 1 and largest > 0)
  eq(#records, 32 + 1 + 64 + 8, "the tracks, the master, the patterns, the instruments")
  ok(largest <= 256 * 1024, "inside the frame bound")
  ok(find(records, "pattern", 64).f.lines.T32:match("^16 notes"), find(records, "pattern", 64).f.lines.T32)
end)

test("digest: the fingerprint is the FNV-1a it always was, taken sixteen bytes at a time (perf N2)", function()
  local fnv = T.load_payload_module("fnv")
  local bit = require("bit")
  -- (the standard 32-bit FNV-1a vectors)
  eq(fnv.hex(fnv.feed(fnv.OFFSET, "")), "811c9dc5")
  eq(fnv.hex(fnv.feed(fnv.OFFSET, "a")), "e40c292c")
  eq(fnv.hex(fnv.feed(fnv.OFFSET, "foobar")), "bf9cf968")
  -- (the step as it was made before: five calls into the bit library a byte, a signed sum)
  local function before(h, text)
    for i = 1, #text do
      h = bit.bxor(h, text:byte(i))
      h = bit.tobit(bit.lshift(h, 24) + bit.lshift(h, 8) + h * 147)
    end
    return h
  end
  local seed = 3
  local function random_text(n)
    local t = {}
    for i = 1, n do
      seed = (seed * 1103515245 + 12345) % 2147483648
      t[i] = string.char(math.floor(seed / 65536) % 256)
    end
    return table.concat(t)
  end
  for n = 0, 70 do
    local text = random_text(n)
    eq(fnv.hex(fnv.feed(fnv.OFFSET, text)), bit.tohex(before(-2128831035, text)), n .. " bytes")
  end
  for _, n in ipairs({ 4095, 4096, 4097, 65537 }) do
    local text = random_text(n)
    eq(fnv.hex(fnv.feed(fnv.OFFSET, text)), bit.tohex(before(-2128831035, text)), n .. " bytes")
  end
  local biggest = string.rep(string.char(255), 100)
  eq(fnv.hex(fnv.feed(fnv.OFFSET, biggest)), bit.tohex(before(-2128831035, biggest)), "every byte at its biggest")
  -- (texts fed in turn, each ended with the digest's 31, as the digest and the scratch print do)
  local h, was = fnv.OFFSET, -2128831035
  for _, text in ipairs({ "1:48 0 255 255 255 0 0", "17.0000", "", "e1:12 255", random_text(40) }) do
    h = fnv.mark(fnv.feed(h, text), 31)
    was = before(before(was, text), string.char(31))
  end
  eq(fnv.hex(h), bit.tohex(was))
end)

test("digest: a pattern's lines print is what it was for the same lines (perf N2)", function()
  local _, peer = setup()
  mock.ops.set_note(2, 1, 3, 1, { note = 48, instrument = 0 })
  mock.ops.set_note(2, 1, 5, 1, { note = 50, instrument = 1, volume = 64 })
  local _, records = whole(peer)
  -- (the lines summed up as before: each text fed a byte at a time through the bit library, ended with a 31)
  local bit = require("bit")
  local h = -2128831035
  local function feed(text)
    for i = 1, #text do
      h = bit.bxor(h, text:byte(i))
      h = bit.tobit(bit.lshift(h, 24) + bit.lshift(h, 8) + h * 147)
    end
    h = bit.bxor(h, 31)
    h = bit.tobit(bit.lshift(h, 24) + bit.lshift(h, 8) + h * 147)
  end
  local pattern_track, track = mock.song:pattern(2):track(1), mock.song:track(1)
  local notes, effects = 0, 0
  for l = 1, 16 do
    local line = pattern_track:line(l)
    if not line.is_empty then
      feed(string.format("%.4f", l))
      for c = 1, track.visible_note_columns do
        local column = line:note_column(c)
        if not column.is_empty then
          notes = notes + ((column.note_value < 120) and 1 or 0)
          feed(string.format("%d:%d %d %d %d %d %d %d", c, column.note_value, column.instrument_value, column.volume_value,
            column.panning_value, column.delay_value, column.effect_number_value, column.effect_amount_value))
        end
      end
      for c = 1, track.visible_effect_columns do
        local column = line:effect_column(c)
        if not column.is_empty then
          effects = effects + 1
          feed(string.format("e%d:%d %d", c, column.number_value, column.amount_value))
        end
      end
    end
  end
  local want = string.format("%d notes, %d effects #%s", notes, effects, bit.tohex(h))
  ok(want:match("^2 notes, 0 effects #%x+$"), want)
  eq(find(records, "pattern", 2).f.lines.Kick, want)
end)
