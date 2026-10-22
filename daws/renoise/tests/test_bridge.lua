-- The tool as Renoise runs it (main.lua and the payload) against the mock API, with a Lua client
-- playing Kumi's host.
local T = ...
local mock = T.mock
local json = T.load_payload_module("json")
local sha256 = T.load_payload_module("sha256")
local test, eq, ok = T.test, T.eq, T.ok

local PROTOCOL = "kumi-daw/1"
-- The tool's own version (a release that changes the tool moves it).
local VERSION = T.load_payload_module("version")
local PORT = 9780
local CLIENT_NONCE = "0123456789abcdef0123456789abcdef"

local function write(path, text)
  local f = assert(io.open(path, "wb"))
  f:write(text)
  f:close()
end

local function read(path)
  local f = io.open(path, "rb")
  if not f then return nil end
  local text = f:read("*a")
  f:close()
  return text
end

local function reference(env, fields)
  local value = { schema = "kumi-daw-reference/1", daw = "renoise", secretFile = env.secret_file,
    discoveryDir = env.discovery_dir, port = PORT, logDir = env.log_dir }
  for k, v in pairs(fields or {}) do value[k] = v end
  write(env.reference, assert(json.encode(value)))
end

-- Installs a copy of the tool into a fresh preferences folder and loads it in a fresh mock Renoise.
local function setup(options)
  options = options or {}
  local env = {}
  env.dir = T.temp_dir(options.name)
  env.resource = env.dir .. "/Renoise/V3.5.4"
  env.bundle = env.resource .. "/Scripts/Tools/com.kumi.Bridge.xrnx/"
  local kumi = env.dir .. "/kumi/bridges/renoise"
  env.discovery_dir = env.dir .. "/kumi/discovery"
  env.log_dir = env.dir .. "/kumi/logs"
  env.secret = options.secret or "kumi-test-secret-0123456789abcdefghijklmnopqrstuvwxyz"
  assert(os.execute(string.format("mkdir -p '%s' '%s' '%s' && cp -R '%s'. '%s'", env.bundle, kumi, env.log_dir, T.tool_dir, env.bundle)) == 0)
  env.secret_file = kumi .. "/bridge.secret"
  write(env.secret_file, env.secret .. "\n")
  env.reference = env.bundle .. "bridge-reference.json"
  if not options.no_reference then reference(env, options.reference) end
  if options.patch then options.patch(env) end
  mock.reset({ bundle_path = env.bundle, version = options.version, platform = options.platform, fresh_wrappers = options.fresh_wrappers,
    move_replaces = options.move_replaces, temp_dir = env.dir, real_random = options.real_random })
  for _, port in ipairs(options.busy_ports or {}) do mock.busy_ports[port] = true end
  if options.song ~= false then mock.song = mock.make_song(options.song) end
  dofile(env.bundle .. "main.lua")
  env.payload = function() return KUMI_BRIDGE.payload end
  mock.fire(mock.tool.tool_finished_loading_observable)
  if mock.song then mock.fire(mock.tool.app_new_document_observable) end
  env.log_file = function() return env.log_dir .. "/renoise-bridge-pid-4242.log" end
  env.discovery_file = function(pid)
    return env.discovery_dir .. "/renoise-" .. (pid or 4242) .. "-" .. sha256.hex(env.resource):sub(1, 16) .. ".json"
  end
  return env
end

local function no_dialogs()
  eq(#mock.crashes, 0, "calls that crash Renoise: " .. table.concat(mock.crashes, " | "))
  eq(#mock.errors, 0, "errors Renoise would show: " .. table.concat(mock.errors, " | "))
  eq(#mock.modals, 0, "modal calls")
end

local function send(peer, value)
  peer:send(assert(json.encode(value)) .. "\n")
end

local function frames(peer)
  return peer:frames(json.decode)
end

local function tick(n)
  mock.advance(50 * (n or 1))
end

local function hmac(env, role, server_nonce, client_nonce)
  return sha256.hmac_hex(env.secret, PROTOCOL .. "\n" .. role .. "\n" .. (client_nonce or CLIENT_NONCE) .. "\n" .. server_nonce)
end

local function handshake(env, peer)
  send(peer, { type = "hello", protocol = PROTOCOL, nonce = CLIENT_NONCE, client = "test/1" })
  local challenge = frames(peer)[1]
  eq(challenge.type, "challenge")
  ok(challenge.nonce:match("^[0-9a-f]+$") and #challenge.nonce == 32, "server nonce")
  eq(challenge.proof, hmac(env, "server", challenge.nonce), "the bridge proves it knows the secret")
  send(peer, { type = "auth", proof = hmac(env, "client", challenge.nonce) })
  local welcome = frames(peer)[1]
  if not welcome then
    tick(4)
    welcome = frames(peer)[1]
  end
  eq(welcome.type, "welcome")
  ok(welcome.session:match("^[0-9a-f]+$") and #welcome.session == 32, "session")
  eq(welcome.maxFrameBytes, 64 * 1024 * 1024)
  eq(welcome.heartbeatMs, 1000)
  return welcome
end

local function connect(env, options)
  local peer = mock.connect(options)
  local welcome = handshake(env, peer)
  return peer, welcome
end

-- What a host does as it reads: takes what the socket holds and gives credit back for those bytes, until that releases
-- no more.
local function give_back(peer)
  repeat
    local taken = peer:take()
    if #taken > 0 then send(peer, { type = "credit", bytes = #taken }) end
  until #taken == 0
end

local next_id = 0
local function request(peer, method, params, extra)
  next_id = next_id + 1
  local frame = { type = "request", id = next_id, method = method, params = params }
  for k, v in pairs(extra or {}) do frame[k] = v end
  send(peer, frame)
  give_back(peer)
  local got = frames(peer)
  local reply = got[#got]
  eq(reply.type, "response")
  eq(reply.id, next_id)
  return reply
end

local function events(peer, name)
  local out = {}
  for _, f in ipairs(frames(peer)) do
    if f.type == "event" and (not name or f.name == name) then out[#out + 1] = f end
  end
  return out
end

-- Whether the bridge is done with a connection: it never closes one (that ends Renoise's server), it retires it.
local function retired(peer)
  local record = KUMI_BRIDGE.connections[peer.address .. ":" .. peer.port]
  return record ~= nil and record.closing == true
end

-- Retired with nothing said, and whatever the peer sends after is ignored.
local function closed_silently(peer)
  tick()
  if not (retired(peer) and peer:take() == "") then return false end
  if peer.open then peer:send('{"type":"hello","protocol":"kumi-daw/1","nonce":"' .. CLIENT_NONCE .. '"}\n') end
  tick()
  return peer:take() == ""
end

local function refused(peer, reason)
  local got = frames(peer)
  eq(#got, 1, "one frame")
  eq(got[1].type, "refused")
  eq(got[1].reason, reason)
  tick()
  ok(retired(peer), "retired after the refusal")
  return got[1]
end

-- (For test_reads.lua, which plays the host against a song.)
T.harness = {
  setup = setup, connect = connect, send = send, frames = frames, tick = tick, request = request, events = events,
  no_dialogs = no_dialogs, retired = retired, read = read, PORT = PORT,
}

test("bridge: listens on 127.0.0.1:9780 and writes the discovery file", function()
  local env = setup()
  local server = mock.server()
  eq(server.address, "127.0.0.1")
  eq(server.port, PORT)
  local discovery = json.decode(read(env.discovery_file()))
  ok(discovery, "discovery file " .. env.discovery_file())
  eq(discovery.schema, "kumi-daw-discovery/1")
  eq(discovery.daw, "renoise")
  eq(discovery.pid, 4242)
  eq(discovery.host, "127.0.0.1")
  eq(discovery.port, PORT)
  eq(discovery.protocol, PROTOCOL)
  eq(discovery.version, VERSION)
  eq(discovery.dawVersion, "3.5.4")
  eq(discovery.resource, env.resource)
  ok(discovery.project.id and discovery.project.path == nil, "an unsaved song has no path")
  ok(discovery.startedMs > 1.7e12, "unix ms")
  eq(#mock.popen_calls, 1, "the pid is looked up once")
  ok(read(env.log_file()):find("listening on 127.0.0.1:9780"), "logged")
  no_dialogs()
end)

test("bridge: two Renoise started in the same second start their epochs apart", function()
  setup({ real_random = true })
  local first, first_run = KUMI_BRIDGE.bridge.epoch_n, KUMI_BRIDGE.bridge.run
  setup({ real_random = true })
  local second, second_run = KUMI_BRIDGE.bridge.epoch_n, KUMI_BRIDGE.bridge.run
  ok(first ~= second, "both start at " .. first)
  ok(first < 2 ^ 44 and second < 2 ^ 44 and first == math.floor(first), "44 random bits, whole")
  -- (#361 N-S10: the place logs carry each run's own name, so a mark kept in one run never resolves in another's)
  ok(first_run ~= second_run and #first_run == 8, "each run names its logs apart: " .. tostring(first_run) .. ", " .. tostring(second_run))
  ok(KUMI_BRIDGE.bridge.ilog.id:sub(1, 8) == second_run, "the instruments' log is this run's: " .. tostring(KUMI_BRIDGE.bridge.ilog.id))
  -- (without /dev/urandom, as on Windows: from the nonce's hash)
  mock.urandom = false
  local wire = T.load_payload_module("wire")
  local n = wire.random28()
  ok(n >= 0 and n < 2 ^ 28 and n == math.floor(n), "from the hash: " .. n)
  no_dialogs()
end)

test("bridge: the handshake proves both sides and the welcome says who it is", function()
  local env = setup({ song = { name = "Tune Title", file_name = "/songs/tune.xrns", playing = true } })
  local _, welcome = connect(env)
  eq(welcome.daw, "renoise")
  eq(welcome.bridge, VERSION)
  eq(welcome.dawVersion, "3.5.4")
  eq(welcome.pid, 4242)
  eq(welcome.resource, env.resource)
  eq(welcome.demo, false)
  eq(welcome.apiVersion, 6.2)
  eq(welcome.project.name, "tune.xrns")
  eq(welcome.project.title, "Tune Title")
  eq(welcome.project.path, "/songs/tune.xrns")
  ok(welcome.project.id:match("^%x+$") and #welcome.project.id == 32)
  eq(table.concat(welcome.methods, ","), "ping,status,renoise.reload,read,snapshot,focus,focus_body,change_preview,change_apply,change_restore,undo_step_begin,undo_step_end,transport_stop,transport_play,audition_note,select,project_save,instrument_save,show_status,digest,render_start,listen_status,listen_cancel,audition,audition_stop,audio_file,scratch,preset_read,instrument_export,macro_read,device_editor,preset_save,script_install,script_remove")
  eq(table.concat(welcome.events, ","), "project,tracks,selection,transport,undone_in_daw,scratch_swept")
  eq(mock.statuses[#mock.statuses], "Kumi connected")
  no_dialogs()
end)

test("bridge: a line in Renoise's status bar, never a dialog", function()
  local env = setup()
  local peer = connect(env)
  local reply = request(peer, "show_status", { text = "Kumi · done · the bass\nis darker" })
  eq(reply.ok, true)
  eq(mock.statuses[#mock.statuses], "Kumi · done · the bass is darker", "one line")
  eq(request(peer, "show_status", { text = "" }).error.code, "bad-params")
  eq(request(peer, "show_status", {}).error.code, "bad-params")
  no_dialogs()
end)

test("bridge: a wrong proof is refused", function()
  setup()
  local peer = mock.connect()
  send(peer, { type = "hello", protocol = PROTOCOL, nonce = CLIENT_NONCE })
  frames(peer)
  send(peer, { type = "auth", proof = string.rep("0", 64) })
  refused(peer, "auth")
  no_dialogs()
end)

test("bridge: an HTTP request, a TLS hello or a byte order mark gets no reply", function()
  setup()
  for _, first in ipairs({ "GET / HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n", "POST /mcp HTTP/1.1\r\n", "PRI * HTTP/2.0\r\n", "\22\3\1\0\165", "\239\187\191{}\n", " {}\n" }) do
    local peer = mock.connect()
    peer:send(first)
    ok(closed_silently(peer), "closed without a reply: " .. string.format("%q", first))
  end
  no_dialogs()
end)

test("bridge: a peer that isn't loopback is closed before anything", function()
  setup()
  local peer = mock.connect({ address = "192.168.1.20" })
  send(peer, { type = "hello", protocol = PROTOCOL, nonce = CLIENT_NONCE })
  ok(closed_silently(peer))
  no_dialogs()
end)

test("bridge: at most four connections wait to authenticate", function()
  setup()
  local peers = {}
  for i = 1, 5 do peers[i] = mock.connect() end
  for i = 1, 4 do eq(#frames(peers[i]), 0, "peer " .. i .. " waits") end
  refused(peers[5], "connections")
  for i = 1, 4 do ok(not peers[i].closed_by_server, "peer " .. i .. " kept") end
  no_dialogs()
end)

test("bridge: at most four authenticated connections", function()
  local env = setup()
  for _ = 1, 4 do connect(env) end
  refused(mock.connect(), "connections")
  no_dialogs()
end)

test("bridge: the handshake must finish within 3 seconds", function()
  local env = setup()
  local slow = mock.connect()
  send(slow, { type = "hello", protocol = PROTOCOL, nonce = CLIENT_NONCE })
  frames(slow)
  local fine = connect(env)
  mock.advance(2900)
  ok(not retired(slow))
  mock.advance(200)
  local got = frames(slow)
  eq(got[1].type, "refused")
  eq(got[1].reason, "timeout")
  tick()
  ok(retired(slow), "closed after the deadline")
  ok(not retired(fine), "an authenticated connection has no deadline")
  no_dialogs()
end)

test("bridge: a peer that keeps sending is timed out at 3 seconds, timer or not", function()
  setup()
  local slow = mock.connect()
  send(slow, { type = "hello", protocol = PROTOCOL, nonce = CLIENT_NONCE })
  frames(slow)
  mock.stall_timers = true -- the clock moves, no timer runs
  for _ = 1, 29 do
    mock.advance(100)
    slow:send("\n")
  end
  eq(#frames(slow), 0, "still waiting at 2.9 s")
  mock.advance(100)
  slow:send("\n")
  mock.stall_timers = false
  refused(slow, "timeout")
  no_dialogs()
end)

test("bridge: blank lines before auth count toward the 1024-byte limit", function()
  setup()
  local peer = mock.connect()
  send(peer, { type = "hello", protocol = PROTOCOL, nonce = CLIENT_NONCE })
  frames(peer)
  peer:send(string.rep("\n", 1024))
  eq(#frames(peer), 0)
  peer:send("\n")
  refused(peer, "limit")
  no_dialogs()
end)

test("bridge: a first line over 1024 bytes is refused", function()
  setup()
  local peer = mock.connect()
  peer:send("{" .. string.rep(" ", 1100))
  refused(peer, "limit")
  no_dialogs()
end)

test("bridge: bad hellos are refused", function()
  setup()
  local cases = {
    { { type = "hello", protocol = "kumi-daw/2", nonce = CLIENT_NONCE }, "protocol" },
    { { type = "hello", protocol = PROTOCOL, nonce = "short" }, "handshake" },
    { { type = "hello", protocol = PROTOCOL, nonce = CLIENT_NONCE:upper() }, "handshake" },
    { { type = "auth", proof = "x" }, "handshake" },
    { { type = "request", id = 1, method = "status" }, "handshake" },
  }
  for _, case in ipairs(cases) do
    local peer = mock.connect()
    send(peer, case[1])
    local frame = refused(peer, case[2])
    if case[2] == "protocol" then eq(frame.protocol, PROTOCOL) end
  end
  no_dialogs()
end)

test("bridge: requests after auth", function()
  local env = setup()
  local peer = connect(env)
  local reply = request(peer, "status")
  eq(reply.ok, true)
  eq(reply.result.daw, "renoise")
  eq(reply.result.busy, false)
  eq(reply.result.pid, 4242)
  eq(reply.result.methods, nil, "status is the identity plus busy")
  reply = request(peer, "ping", {})
  eq(reply.ok, true)
  eq(next(reply.result), nil)
  reply = request(peer, "nope")
  eq(reply.ok, false)
  eq(reply.error.code, "unknown-method")
  send(peer, { type = "later", anything = true })
  eq(#frames(peer), 0, "unknown frame types are ignored")
  reply = request(peer, "status", nil, { deadlineMs = 1000 })
  eq(reply.ok, false)
  eq(reply.error.code, "deadline")
  ok(not retired(peer))
  no_dialogs()
end)

test("bridge: after auth a line that isn't a frame closes without a reply", function()
  local env = setup()
  for _, line in ipairs({ "not json\n", '{"type":"request","id":0,"method":"ping"}\n', "[1]\n", '{"a":"\255"}\n' }) do
    local peer = connect(env)
    peer:send(line)
    ok(closed_silently(peer), string.format("%q", line))
  end
  no_dialogs()
end)

test("bridge: a result nested too deep is answered with an error, not sent and not fatal", function()
  local server = T.load_payload_module("server")
  local sent = {}
  local client = { is_open = true, send = function(_, text) sent[#sent + 1] = text; return true end }
  local logged = {}
  local result
  local ctx = {
    connections = {},
    unix_ms = function() return 0 end,
    log = function(level, message) logged[#logged + 1] = level .. ": " .. message end,
    on_request = function() return result end,
  }
  local s = server.new(ctx)
  local record = { key = "k", client = client, outbox = {} }
  local function nested(levels)
    local t = json.array({})
    for _ = 2, levels do t = json.array({ t }) end
    return t
  end
  local function answer(levels)
    result = nested(levels)
    s:answer(record, { id = 5, method = "x", params = {} })
    return assert(json.decode(sent[#sent]:sub(1, -2)))
  end
  local reply = answer(json.MAX_DEPTH - 1)
  eq(reply.ok, true, "a result at the limit is sent")
  reply = answer(json.MAX_DEPTH)
  eq(reply.ok, false)
  eq(reply.id, 5)
  eq(reply.error.code, "internal")
  ok(reply.error.message:find("deeply"), reply.error.message)
  ok(not record.closing and not record.gone, "the connection goes on")
  ok(logged[1] and logged[1]:find("could not encode the result"), "logged")
  eq(answer(1).ok, true, "and answers the next request")
end)

test("bridge: selection changes are one event per tick, numbered per connection", function()
  local env = setup()
  local peer = connect(env)
  local song = mock.song
  song.selected_track_index = 2
  song.selected_track_index = 3
  song.selected_instrument_index = 4
  tick()
  local got = events(peer, "selection")
  eq(#got, 1)
  eq(got[1].seq, 1)
  eq(got[1].data.track, 3)
  eq(got[1].data.instrument, 4)
  eq(got[1].data.pattern, 1)
  song.selected_track_index = 4
  song.selected_track_index = 3
  tick()
  eq(#events(peer, "selection"), 0, "back to the same selection is no event")
  local late = connect(env)
  song.selected_pattern_index = 7
  tick()
  got = events(peer, "selection")
  eq(got[1].seq, 2)
  eq(got[1].data.pattern, 7)
  eq(events(late, "selection")[1].seq, 1, "a new connection counts from 1")
  no_dialogs()
end)

test("bridge: transport changes are events", function()
  local env = setup()
  local peer = connect(env)
  mock.song.transport.playing = true
  mock.song.transport.bpm = 140
  tick()
  local got = events(peer, "transport")
  eq(#got, 1)
  eq(got[1].data.playing, true)
  eq(got[1].data.bpm, 140)
  eq(got[1].data.lpb, 4)
  no_dialogs()
end)

test("bridge: a new song detaches the old observers and attaches new ones", function()
  local env = setup({ song = { name = "Old" } })
  local peer = connect(env)
  local old = mock.song
  eq(mock.notifier_count(old.selected_track_index_observable), 1)
  mock.load_song({ name = "New", file_name = "/s/new.xrns" })
  eq(mock.notifier_count(old.selected_track_index_observable), 0, "old song detached")
  eq(mock.notifier_count(old.transport.playing_observable), 0)
  eq(mock.notifier_count(mock.song.selected_track_index_observable), 1, "new song attached once")
  local got = events(peer, "project")
  eq(#got, 2)
  eq(got[1].data, json.null, "the old song closed")
  eq(got[2].data.name, "new.xrns")
  eq(got[2].data.path, "/s/new.xrns")
  eq(got[2].seq, got[1].seq + 1)
  old.selected_track_index = 5
  tick()
  eq(#events(peer, "selection"), 0, "the old song is silent")
  eq(json.decode(read(env.discovery_file())).project.path, "/s/new.xrns", "discovery follows the song")
  no_dialogs()
end)

test("bridge: song ids are stored in tool_data on save and survive reopening", function()
  local env = setup()
  local peer, welcome = connect(env)
  local id = welcome.project.id
  mock.save_song("/s/saved.xrns")
  eq(json.decode(mock.song.tool_data).songId, id, "tool_data holds the id")
  local saved = events(peer, "project")[1]
  eq(saved.data.id, id)
  eq(saved.data.path, "/s/saved.xrns")
  mock.load_song({ file_name = "/s/saved.xrns", tool_data = mock.song.tool_data })
  eq(events(peer, "project")[2].data.id, id, "the same id after reopening")
  mock.load_song({ name = "Other" })
  ok(events(peer, "project")[2].data.id ~= id, "another song, another id")
  no_dialogs()
end)

test("bridge: a saved song without a stored id keeps one id across reopening", function()
  local env = setup({ song = { file_name = "/s/older.xrns" } })
  local _, welcome = connect(env)
  local id = welcome.project.id
  mock.load_song({ file_name = "/s/older.xrns" })
  local peer = connect(env)
  eq(request(peer, "status").result.project.id, id)
  no_dialogs()
end)

test("bridge: a song file Renoise opens over its empty startup song is named by its path, not the empty song's id", function()
  -- Started with a file: the empty song first, then the file, with no release in between (as real Renoise does).
  local env = setup({ song = { name = "" } })
  local _, welcome = connect(env)
  local empty = welcome.project.id
  mock.open_over_startup_song({ file_name = "/s/bears.xrns" })
  local peer = connect(env)
  local id = request(peer, "status").result.project.id
  ok(id ~= empty, "not the empty song's id")
  -- The same file opened in another run (a restart): the same id.
  mock.load_song({ file_name = "/s/bears.xrns" })
  eq(request(connect(env), "status").result.project.id, id)
  no_dialogs()
end)

test("bridge: other keys in tool_data are kept", function()
  local env = setup({ song = { tool_data = '{"note":"keep me"}' } })
  connect(env)
  mock.save_song("/s/x.xrns")
  local data = json.decode(mock.song.tool_data)
  eq(data.note, "keep me")
  ok(data.songId)
  no_dialogs()
end)

-- Selection changes that always differ, one per tick: events of about 110 bytes.
local function churn(song, from, to, every, on_every)
  for i = from, to do
    song.selected_track_index = i % 90 + 1
    tick()
    if every and i % every == 0 then on_every(i) end
  end
end

test("bridge: a host that stopped reading is cut off before the socket fills, never blocked on", function()
  local env = setup()
  local peer = connect(env)
  frames(peer)
  peer:stop_reading()
  churn(mock.song, 1, 3000)
  ok(retired(peer), "retired")
  local last = frames(peer)
  eq(last[#last].type, "bye", "told, so it ends the connection on its side")
  eq(mock.blocked_sends, 0, "sends that would have blocked Renoise")
  ok(peer.unread <= 64 * 1024, "at most 64 KiB went to a peer that wasn't reading, not " .. peer.unread)
  ok(peer.unread > 60 * 1024, "and it was allowed that much: " .. peer.unread)
  ok(read(env.log_file()):find("the host stopped reading"), "logged")
  local other = connect(env)
  eq(request(other, "status").ok, true, "a new connection works")
  no_dialogs()
end)

test("bridge: a host that sends now and then is never cut off, however many events there are", function()
  local env = setup()
  local peer = connect(env)
  local count = 0
  churn(mock.song, 1, 3000, 100, function()
    give_back(peer)
    for _, f in ipairs(frames(peer)) do
      if f.type == "event" then count = count + 1 end
    end
    eq(request(peer, "ping", {}).ok, true)
  end)
  ok(not retired(peer), "open")
  ok(count > 2000, "the events kept coming: " .. count)
  no_dialogs()
end)

test("bridge: a host that never says a word is closed after the heartbeats pile up", function()
  local env = setup()
  local peer = connect(env)
  peer:stop_reading()
  mock.advance(600 * 1000)
  ok(not retired(peer), "ten minutes of heartbeats are well within the cap")
  mock.advance(3000 * 1000)
  ok(retired(peer), "closed")
  eq(mock.blocked_sends, 0)
  no_dialogs()
end)

test("bridge: a send that fails closes that connection", function()
  local env = setup()
  local peer = connect(env)
  peer.send_fails = true
  mock.song.selected_track_index = 5
  tick()
  ok(retired(peer), "the socket is closed, not just forgotten")
  local other = connect(env)
  eq(request(other, "status").ok, true, "the others go on")
  no_dialogs()
end)

test("bridge: heartbeats about once a second, only after auth", function()
  local env = setup()
  local waiting = mock.connect()
  local peer = connect(env)
  mock.advance(1000)
  local beats = {}
  for _, f in ipairs(frames(peer)) do
    if f.type == "heartbeat" then beats[#beats + 1] = f end
  end
  eq(#beats, 1)
  ok(beats[1].ms >= 1000 and beats[1].ms <= 1100, "ms since the connection opened")
  mock.advance(3000)
  local count = 0
  for _, f in ipairs(frames(peer)) do
    if f.type == "heartbeat" then count = count + 1 end
  end
  eq(count, 3)
  ok(not waiting:take():find("heartbeat"), "no heartbeats before auth")
  no_dialogs()
end)

test("bridge: unloading says bye, closes everything and removes the discovery file", function()
  local env = setup()
  local peer = connect(env)
  local song = mock.song
  ok(read(env.discovery_file()))
  mock.fire(mock.tool.tool_will_unload_observable)
  local got = frames(peer)
  eq(got[#got].type, "bye")
  eq(got[#got].reason, "shutdown", "disabled or reloaded")
  eq(mock.server(), nil, "server closed")
  ok(not peer.open, "the server took its connections with it")
  eq(next(KUMI_BRIDGE.connections), nil, "and the bridge let go of them")
  eq(read(env.discovery_file()), nil, "discovery removed")
  eq(#mock.timers, 0, "timer removed")
  eq(mock.notifier_count(song.selected_track_index_observable), 0, "observers detached")
  ok(read(env.log_file()):find("stopped"), "logged")
  no_dialogs()
end)

test("bridge: quitting Renoise says bye with quit", function()
  local env = setup()
  local peer = connect(env)
  mock.fire(mock.tool.app_release_document_observable)
  mock.fire(mock.tool.tool_will_unload_observable)
  local got = frames(peer)
  eq(got[#got].type, "bye")
  eq(got[#got].reason, "quit")
  no_dialogs()
end)

test("bridge: track count changes are events", function()
  local env = setup()
  local peer = connect(env)
  local fields = mock.impl(mock.song)
  fields.sequencer_track_count = 3
  fields.send_track_count = 1
  mock.fire(mock.song.tracks_observable) -- (a change that says nothing of what it was)
  local got = events(peer, "tracks")
  eq(#got, 1, "told at once, with no tick")
  eq(got[1].data.count, 5, "sequencer and send tracks plus the master")
  eq(#got[1].data.made + #got[1].data.gone, 0, "nothing to shift by")
  ok(got[1].data.epoch, "and the epoch moved")
  no_dialogs()
end)

test("bridge: waits for setup, then starts without a restart", function()
  local env = setup({ no_reference = true })
  eq(mock.server(), nil)
  eq(#mock.statuses, 1, "says so once")
  ok(mock.statuses[1]:find("kumi setup"))
  mock.advance(2000)
  eq(#mock.statuses, 1, "not repeated")
  reference(env)
  mock.advance(6000)
  ok(mock.server(), "listening once set up")
  connect(env)
  no_dialogs()
end)

test("bridge: a reference for another DAW or schema isn't used", function()
  setup({ reference = { daw = "reaper" } })
  eq(mock.server(), nil)
  setup({ reference = { schema = "kumi-daw-reference/2" } })
  eq(mock.server(), nil)
  no_dialogs()
end)

test("bridge: busy ports move along Renoise's range and wrap", function()
  local env = setup({ busy_ports = { PORT, PORT + 1 } })
  eq(mock.server().port, PORT + 2)
  eq(json.decode(read(env.discovery_file())).port, PORT + 2)
  connect(env)
  setup({ reference = { port = 9789 }, busy_ports = { 9789 } })
  eq(mock.server().port, 9780, "wraps to the range's start")
  setup({ reference = { port = 9765 } })
  eq(mock.server().port, 9780, "a port outside the range is ignored")
  no_dialogs()
end)

test("bridge: the server is recreated after it dies", function()
  local env = setup()
  local first = mock.server()
  mock.socket_error("connection reset")
  ok(mock.server() == first, "an error while it runs keeps the server")
  first.running = false
  first.open = false
  mock.advance(1500)
  local second = mock.server()
  ok(second and second ~= first, "a new server")
  eq(second.port, PORT)
  connect(env)
  no_dialogs()
end)

test("bridge: renoise.reload swaps the payload and keeps the port and the connection", function()
  local env = setup()
  local peer = connect(env)
  mock.song.selected_track_index = 2
  tick()
  eq(events(peer, "selection")[1].seq, 1)
  local server = mock.server()
  local old_payload = env.payload()
  eq(request(peer, "renoise.reload").result.reloading, true)
  tick()
  ok(env.payload() ~= old_payload, "new payload")
  ok(mock.server() == server and server.running, "same server")
  eq(request(peer, "status").ok, true)
  eq(mock.notifier_count(mock.song.selected_track_index_observable), 1, "attached once")
  mock.song.selected_track_index = 9
  tick()
  local selection = events(peer, "selection")[1]
  eq(selection.data.track, 9)
  eq(selection.seq, 2, "the connection's numbering goes on")
  ok(read(env.log_file()):find("reloaded bridge"), "the new payload logs")
  eq(#mock.popen_calls, 1, "the pid isn't looked up again")
  no_dialogs()
end)

test("bridge: an error while welcoming closes that connection, nothing else", function()
  local env = setup()
  local good = connect(env)
  local fields = mock.impl(mock.song)
  local saved = fields.name
  fields.name = nil
  local peer = mock.connect()
  send(peer, { type = "hello", protocol = PROTOCOL, nonce = CLIENT_NONCE })
  local challenge = frames(peer)[1]
  send(peer, { type = "auth", proof = hmac(env, "client", challenge.nonce) })
  tick()
  ok(retired(peer), "closed")
  fields.name = saved
  eq(request(good, "status").ok, true, "the other connection still answers")
  no_dialogs()
end)

test("bridge: a broken payload on reload keeps the running one", function()
  local env = setup()
  local peer = connect(env)
  local old_payload = env.payload()
  write(env.bundle .. "payload/status.lua", "this is not lua")
  request(peer, "renoise.reload")
  tick()
  ok(env.payload() == old_payload, "kept")
  eq(request(peer, "status").ok, true)
  ok(read(env.log_file()):find("reload failed"), "logged")
  no_dialogs()
end)

test("bridge: an error reading the song is logged, never shown", function()
  local env = setup()
  local peer = connect(env)
  local playing = mock.song.transport.playing_observable
  mock.impl(mock.song).transport = nil
  mock.fire(playing)
  tick(25)
  ok(read(env.log_file()):find("not mocked"), "logged")
  ok(not retired(peer))
  no_dialogs()
end)

test("bridge: a secret file others can read refuses connections", function()
  local env = setup()
  mock.stat_modes[env.secret_file] = 33188
  local peer = mock.connect()
  send(peer, { type = "hello", protocol = PROTOCOL, nonce = CLIENT_NONCE })
  ok(closed_silently(peer))
  tick(20)
  ok(read(env.log_file()):find("other accounts can read it"), "logged")
  no_dialogs()
end)

test("bridge: an invalid secret refuses connections", function()
  setup({ secret = "short" })
  local peer = mock.connect()
  send(peer, { type = "hello", protocol = PROTOCOL, nonce = CLIENT_NONCE })
  ok(closed_silently(peer))
  no_dialogs()
end)

test("bridge: the demo is detected", function()
  local env = setup({ version = "3.5.4 Demo" })
  local _, welcome = connect(env)
  eq(welcome.demo, true)
  eq(welcome.dawVersion, "3.5.4 Demo")
  no_dialogs()
end)

test("bridge: works with no song yet, then attaches when one arrives", function()
  local env = setup({ song = false })
  local peer, welcome = connect(env)
  eq(welcome.project, json.null)
  mock.load_song({ name = "First" })
  eq(events(peer, "project")[1].data.name, "First")
  mock.song.selected_track_index = 2
  tick()
  eq(#events(peer, "selection"), 1)
  no_dialogs()
end)

-- What a host does as it reads: gives the bridge back the credit for the bytes it has taken in.
local function grant(peer, bytes)
  send(peer, { type = "credit", bytes = bytes })
end

test("bridge: a big frame goes out in slices over ticks, as the host grants credit", function()
  local env = setup()
  env.payload().handlers["test.big"] = function() return { blob = string.rep("n", 600 * 1024) } end
  local peer = connect(env)
  send(peer, { type = "request", id = 99, method = "test.big" })
  for _ = 1, 20 do
    tick(2)
    local taken = peer:take() -- (it keeps what it took, for frames() to split)
    if #taken > 0 then grant(peer, #taken) end
  end
  local response
  for _, frame in ipairs(frames(peer)) do
    if frame.type == "response" and frame.id == 99 then response = frame end
  end
  ok(response, "the answer came whole")
  eq(#response.result.blob, 600 * 1024)
  ok(peer.sent_calls > 5, "sent in pieces")
  no_dialogs()
end)

test("bridge: the welcome gives a floor of credit, to be topped up by the host's first grant", function()
  local env = setup()
  local _, welcome = connect(env)
  eq(welcome.creditBytes, 64 * 1024)
  no_dialogs()
end)

-- A host's window is what its receive buffer holds: the credit it lets a bridge have unread. Its first grant tops the
-- welcome's floor (64 KiB) up to that, and it grants back what it reads.
for _, window in ipairs({ 64 * 1024, 256 * 1024 }) do
  test("bridge: a host with a " .. math.floor(window / 1024) .. " KiB receive buffer that pauses mid-answer is held back from, never torn, never retired, and finishes when it reads", function()
    local env = setup()
    local peer = connect(env)
    frames(peer)
    local size = 1500 * 1024
    env.payload().handlers["test.big"] = function() return { blob = string.rep("z", size) } end
    -- A socket whose buffers hold no more than the host's window: a send past it would block Renoise.
    mock.socket_buffer_bytes = window
    if window > 64 * 1024 then grant(peer, window - 64 * 1024) end
    peer:stop_reading()
    send(peer, { type = "request", id = 7, method = "test.big", params = {} })
    tick(4)
    ok(peer.unread <= window, "no more than the window was sent: " .. peer.unread)
    ok(peer.unread >= window - 4096, "but the window was used: " .. peer.unread)
    eq(mock.blocked_sends, 0, "nothing sent into a full socket")
    -- A long pause: heartbeats and ticks go by; the host is not retired, and nothing more is sent.
    mock.advance(20 * 1000)
    eq(peer.unread <= window, true)
    ok(not retired(peer), "not retired")
    eq(mock.blocked_sends, 0)
    -- It reads again: what it read is granted back, and the answer comes to an end, whole. It grants as the bytes come
    -- off the socket (the answer is longer than the window, so a line is never whole before the credit is given).
    peer:resume_reading()
    local text = ""
    for _ = 1, 400 do
      repeat
        local taken = peer:take()
        text = text .. taken
        if #taken > 0 then grant(peer, #taken) end
      until #taken == 0
      mock.advance(20)
      -- (what the host reads is never more than the window past what it granted)
      ok(peer.unread <= window, "unread " .. peer.unread)
    end
    text = text .. peer:take()
    local lines = {}
    for line in text:gmatch("[^\n]+") do lines[#lines + 1] = line end
    local response
    for _, line in ipairs(lines) do
      local frame = json.decode(line)
      ok(frame, "every line is whole JSON")
      if frame.type == "response" and frame.id == 7 then response = frame end
    end
    ok(response, "the answer arrived")
    eq(#response.result.blob, size)
    ok(not retired(peer), "never retired")
    eq(mock.blocked_sends, 0)
    no_dialogs()
  end)
end

test("bridge: no heartbeats pile up behind output that waits for credit", function()
  local env = setup()
  local peer = connect(env)
  frames(peer)
  env.payload().handlers["test.big"] = function() return { blob = string.rep("z", 900 * 1024) } end
  mock.socket_buffer_bytes = 600 * 1024
  peer:stop_reading()
  send(peer, { type = "request", id = 7, method = "test.big", params = {} })
  tick(4)
  mock.advance(120 * 1000) -- two minutes
  local waiting = 0
  for _, record in pairs(KUMI_BRIDGE.connections) do waiting = math.max(waiting, #record.outbox) end
  ok(waiting <= 2, "items waiting: " .. waiting)
  ok(not retired(peer))
  no_dialogs()
end)

test("bridge: Windows starts no process", function()
  local env = setup({ platform = "WINDOWS" })
  eq(#mock.popen_calls, 0, "no process started")
  ok(read(env.discovery_file(0)), "discovery without a pid")
  no_dialogs()
end)

-- Windows' stock os.rename and os.remove take the ANSI code page: a user folder like "Kümi" is out of their reach.
local function utf8_discovery(options)
  options = options or {}
  options.platform = "WINDOWS"
  local dir = T.temp_dir("utf8") .. "/K\195\188mi/discovery"
  options.reference = { discoveryDir = dir }
  options.song = { name = "Demo song" }
  local env = setup(options)
  return env, dir .. "/renoise-0-" .. sha256.hex(env.resource):sub(1, 16) .. ".json"
end

test("bridge: on Windows a discovery folder with letters outside ASCII is written and replaced with Renoise's own move", function()
  local env, file = utf8_discovery()
  local first = read(file)
  ok(first, "discovery written")
  eq(json.decode(first).project.name, "Demo song")
  mock.song.name = "Renamed"
  tick(2)
  eq(json.decode(read(file)).project.name, "Renamed", "replaced")
  eq(read(file .. ".tmp"), nil, "no temporary file left")
  no_dialogs()
end)

test("bridge: on Windows, where move doesn't replace, the discovery file is written where it goes", function()
  local env, file = utf8_discovery({ move_replaces = false })
  ok(read(file), "written the first time")
  mock.song.name = "Renamed"
  tick(2)
  eq(json.decode(read(file)).project.name, "Renamed", "and again")
  eq(read(file .. ".tmp"), "", "the temporary file is emptied")
  no_dialogs()
end)

test("bridge: on Windows a log in a folder with letters outside ASCII is rotated with os.move", function()
  local log = T.load_payload_module("log")
  local dir = T.temp_dir("utf8log") .. "/K\195\188mi"
  assert(os.execute("mkdir -p '" .. dir .. "'") == 0)
  mock.reset({ platform = "WINDOWS" })
  log.MAX_BYTES = 200
  local file = dir .. "/renoise-bridge-pid-1.log"
  log.configure(file)
  for _ = 1, 12 do
    log.write("info", string.rep("x", 60))
    log.flush()
  end
  ok(read(file .. ".1"), "the full log was moved aside")
  ok(#read(file) < 400, "and the log is kept to its size: " .. #read(file))
  no_dialogs()
end)

test("bridge: one socket wrapped in several Lua objects is one connection", function()
  local env = setup({ fresh_wrappers = true })
  local peer = connect(env)
  eq(request(peer, "status").ok, true)
  local count = 0
  for _ in pairs(KUMI_BRIDGE.connections) do count = count + 1 end
  eq(count, 1)
  no_dialogs()
end)

-- Housekeeping from the socket callbacks

test("bridge: connections whose peers left stop counting at once, with no timer running", function()
  setup()
  local peers = {}
  for i = 1, 4 do peers[i] = mock.connect() end
  for i = 1, 4 do peers[i]:close() end
  local fifth = mock.connect()
  eq(#frames(fifth), 0, "taken, not refused for connections")
  ok(not retired(fifth))
  no_dialogs()
end)

test("bridge: a stalled handshake is refused by the next connection's callback, no timer needed", function()
  setup()
  local idle = {}
  for i = 1, 4 do idle[i] = mock.connect() end
  mock.stall_timers = true
  mock.advance(3500) -- the clock moves, no timer runs
  mock.stall_timers = false
  local late = mock.connect()
  eq(#frames(late), 0, "the slots were free by then")
  for i = 1, 4 do eq(frames(idle[i])[1].reason, "timeout", "idle peer " .. i) end
  no_dialogs()
end)

test("bridge: a socket error is only noted; the server is made anew from the tick when it says it stopped", function()
  local env = setup()
  local first = mock.server()
  mock.socket_error("boom")
  ok(first.running and first.open, "nothing closed inside the callback")
  eq(#mock.servers, 1, "and nothing remade for it")
  first.running = false
  tick()
  eq(#mock.servers, 2, "remade once it says it isn't running")
  ok(mock.server() ~= nil and mock.server() ~= first, "a new server listens")
  local peer = connect(env)
  eq(request(peer, "ping", {}).ok, true)
  no_dialogs()
end)

test("bridge: an authenticated connection survives fifty rounds of churn from peers with no secret", function()
  local env = setup()
  local kumi = connect(env)
  local server = mock.server()
  for round = 1, 50 do
    local kind = round % 5
    if kind == 0 then
      -- connect and vanish
      local a, b = mock.connect(), mock.connect()
      a:close()
      b:close()
    elseif kind == 1 then
      -- an HTTP line, then gone
      local peer = mock.connect()
      peer:send("GET / HTTP/1.1\r\n\r\n")
      peer:close()
    elseif kind == 2 then
      -- more than fit, each refused
      local many = {}
      for i = 1, 6 do many[i] = mock.connect() end
      for _, peer in ipairs(many) do peer:close() end
    elseif kind == 3 then
      -- never speaks: timed out by the timer
      local idle = {}
      for i = 1, 3 do idle[i] = mock.connect() end
      mock.advance(3100)
      for _, peer in ipairs(idle) do peer:close() end
    else
      -- a wrong proof
      local peer = mock.connect()
      send(peer, { type = "hello", protocol = PROTOCOL, nonce = CLIENT_NONCE })
      frames(peer)
      send(peer, { type = "auth", proof = string.rep("0", 64) })
    end
    mock.socket_error("connection reset by peer")
    tick(2)
    eq(request(kumi, "ping", {}).ok, true, "Kumi still answers after round " .. round)
  end
  ok(not retired(kumi), "never closed")
  eq(#mock.servers, 1, "the server was never remade")
  ok(server.running and server.open)
  no_dialogs()
end)

test("bridge: a client that has gone before it is looked at is left alone, never closed", function()
  local env = setup()
  local early = mock.connect()
  early:close() -- gone before the accept callback could read it
  mock.advance(200)
  local fine = connect(env)
  eq(request(fine, "ping", {}).ok, true)
  eq(#mock.crashes, 0)
  no_dialogs()
end)

test("bridge: a host that sends requests and never reads the answers is cut off", function()
  local env = setup()
  local peer = connect(env)
  peer:stop_reading()
  for _ = 1, 400 do send(peer, { type = "request", id = next_id + 1, method = "status", params = {} }); next_id = next_id + 1 end
  tick(4)
  ok(retired(peer) or peer.unread > 0, "answered or cut off")
  eq(mock.blocked_sends, 0)
  no_dialogs()
end)

test("bridge: a flood of requests is answered a few at a time, in order", function()
  local server = T.load_payload_module("server")
  local sent = {}
  local client = { is_open = true, send = function(_, text) sent[#sent + 1] = text; return true end }
  local clock = 0
  local answered = {}
  local ctx = {
    connections = {},
    now_ms = function() clock = clock + 3 return clock end, -- every look at the clock costs 3 ms
    unix_ms = function() return 0 end,
    log = function() end,
    on_request = function(_, request) answered[#answered + 1] = request.id return {} end,
  }
  local s = server.new(ctx)
  local record = { key = "k", client = client, outbox = {}, requests = {} }
  local actions = {}
  for id = 1, 40 do actions[id] = { request = { id = id, method = "x", params = {} } } end
  s:apply(record, actions)
  ok(#answered >= 1 and #answered < 40, "a share of them: " .. #answered)
  local first = #answered
  s:run_requests(record)
  ok(#answered > first, "more on the next go")
  for _ = 1, 100 do s:run_requests(record) end
  eq(#answered, 40)
  for i = 1, 40 do eq(answered[i], i, "in order") end
  -- Far too many waiting is a peer to close.
  local big = {}
  for id = 1, 300 do big[id] = { request = { id = id, method = "x", params = {} } } end
  local flood = { key = "j", client = client, outbox = {}, requests = {} }
  s:apply(flood, big)
  ok(flood.closing, "closed")
end)

test("bridge: a deadline that is now has passed, and a code that isn't text is sent as text", function()
  local server = T.load_payload_module("server")
  local sent = {}
  local client = { is_open = true, send = function(_, text) sent[#sent + 1] = text; return true end }
  local ctx = {
    connections = {},
    unix_ms = function() return 1000 end,
    log = function() end,
    on_request = function() error({ code = 5, message = "five" }) end,
  }
  local s = server.new(ctx)
  local record = { key = "k", client = client, outbox = {} }
  s:answer(record, { id = 1, method = "x", params = {}, deadlineMs = 1000 })
  eq(json.decode(sent[1]).error.code, "deadline")
  s:answer(record, { id = 2, method = "x", params = {}, deadlineMs = 1001 })
  local reply = json.decode(sent[2])
  eq(reply.error.code, "5")
  eq(reply.error.message, "five")
end)

test("bridge: the bye goes out ahead of output that hasn't started, behind a line already part sent", function()
  local server = T.load_payload_module("server")
  local wire = T.load_payload_module("wire")
  local sent = {}
  local client = { is_open = true, send = function(_, text) sent[#sent + 1] = text; return true end }
  local record = {
    key = "k", client = client, closing = false,
    conn = { authenticated = function() return true end },
    outbox = { { text = "first-line\n", offset = 4 }, { text = "never-started\n", offset = 1 } },
  }
  local s = server.new({ connections = { k = record }, log = function() end })
  s:close_all("quit")
  eq(table.concat(sent), "st-line\n" .. wire.encode_frame("bye", { reason = "quit" }))
  local fresh = {
    key = "j", client = client, conn = { authenticated = function() return true end },
    outbox = { { text = "waiting\n", offset = 1 } },
  }
  sent = {}
  s = server.new({ connections = { j = fresh }, log = function() end })
  s:close_all("shutdown")
  eq(table.concat(sent), wire.encode_frame("bye", { reason = "shutdown" }), "nothing else ahead of it")
end)

test("bridge: a line that arrives a byte at a time doesn't hold a table slot for each", function()
  local wire = T.load_payload_module("wire")
  local framer = wire.framer(2000)
  for _ = 1, 1500 do wire.feed(framer, "x") end
  eq(wire.next_line(framer), nil)
  ok(framer.count <= 256, "parts held: " .. framer.count)
  wire.feed(framer, "\n")
  local line = wire.next_line(framer)
  eq(#line, 1500)
end)

test("bridge: an error reading the song for events costs that step, not the connections' tick", function()
  local env = setup()
  local peer = connect(env)
  frames(peer)
  mock.impl(mock.song).selected_track_index = nil -- reading it raises
  mock.song.selected_instrument_index = 3
  tick()
  mock.advance(1100)
  local beats = 0
  for _, f in ipairs(frames(peer)) do
    if f.type == "heartbeat" then beats = beats + 1 end
  end
  ok(beats >= 1, "heartbeats go on: " .. beats)
  ok(read(env.log_file()):find("events:"), "logged")
  eq(#mock.errors, 0, "nothing for Renoise to show")
end)

-- The payload's start

local function raise_in_sync(env)
  write(env.bundle .. "payload/sync.lua", 'return { new = function() error("boom at start") end }')
end

test("bridge: a payload whose start raises is dropped, and tried again once the cause is gone", function()
  local fixed
  local env = setup({ patch = function(e) fixed = e; raise_in_sync(e) end })
  ok(env.payload() == nil, "no half-started payload")
  eq(mock.server(), nil, "no server")
  tick(20)
  eq(#mock.errors, 0)
  -- Put the real file back; five seconds later the tool starts without a restart.
  local source = T.tool_dir .. "payload/sync.lua"
  write(fixed.bundle .. "payload/sync.lua", read(source))
  mock.advance(5200)
  ok(env.payload() ~= nil, "started")
  local peer = connect(env)
  eq(request(peer, "ping", {}).ok, true)
  no_dialogs()
end)

test("bridge: a reload whose new payload can't start puts the old one back, connections and all", function()
  local env = setup()
  local peer = connect(env)
  local old = env.payload()
  raise_in_sync(env)
  request(peer, "renoise.reload")
  tick()
  ok(env.payload() == old, "the old payload is serving again")
  eq(request(peer, "ping", {}).ok, true, "the same connection answers")
  ok(read(env.log_file()):find("reload failed"), "logged")
  no_dialogs()
end)

test("bridge: the payload is read with io.open, which takes UTF-8 paths on Windows, not loadfile", function()
  local real = loadfile
  _G.loadfile = function() error("loadfile used") end
  local ok_, err = pcall(setup)
  _G.loadfile = real
  ok(ok_, tostring(err))
end)

test("bridge: a reload reads the reference again", function()
  local env = setup()
  local peer = connect(env)
  local moved = env.dir .. "/elsewhere"
  assert(os.execute("mkdir -p '" .. moved .. "'") == 0)
  reference(env, { logDir = moved })
  request(peer, "renoise.reload")
  tick()
  mock.advance(1100)
  ok(read(moved .. "/renoise-bridge-pid-4242.log"), "the log is in the new folder now")
  no_dialogs()
end)

test("bridge: log lines say which Renoise wrote them", function()
  local env = setup()
  connect(env)
  local text = read(env.log_file())
  ok(text:find("info %[pid 4242%] listening"), text)
  no_dialogs()
end)

test("bridge: a song born from a template doesn't share the template's id", function()
  local env = setup({ song = { tool_data = '{"songId":"0123456789abcdef0123456789abcdef"}' } })
  local peer = connect(env)
  local id = request(peer, "status").result.project.id
  ok(id ~= "0123456789abcdef0123456789abcdef", "a new id for a song never saved: " .. tostring(id))
  mock.save_song(env.dir .. "/a.xrns")
  tick()
  local stored = json.decode(mock.song.tool_data)
  eq(stored.songId, id, "stored at the first save")
  -- A second song from the same template gets its own.
  mock.load_song({ tool_data = '{"songId":"0123456789abcdef0123456789abcdef"}' })
  tick()
  local second = request(peer, "status").result.project.id
  ok(second ~= id and second ~= "0123456789abcdef0123456789abcdef", "its own: " .. tostring(second))
  -- Reopened from its file, a saved song keeps what it stored.
  mock.load_song({ file_name = env.dir .. "/a.xrns", tool_data = mock.song and '{"songId":"' .. id .. '"}' })
  tick()
  eq(request(peer, "status").result.project.id, id)
  no_dialogs()
end)

test("bridge: a song born from a template keeps its own id whichever comes first at its first save, the name or the announcement", function()
  local env = setup({ song = { tool_data = '{"songId":"0123456789abcdef0123456789abcdef"}' } })
  mock.name_first = true
  local peer = connect(env)
  local id = request(peer, "status").result.project.id
  ok(id ~= "0123456789abcdef0123456789abcdef", "a new id for a song never saved: " .. tostring(id))
  mock.save_song(env.dir .. "/b.xrns")
  tick()
  eq(json.decode(mock.song.tool_data).songId, id, "its own is stored, not the template's")
  eq(request(peer, "status").result.project.id, id, "and it is still its id")
  -- Reopened from its file, it keeps what it stored.
  mock.load_song({ file_name = env.dir .. "/b.xrns", tool_data = mock.song and '{"songId":"' .. id .. '"}' })
  tick()
  eq(request(peer, "status").result.project.id, id)
  no_dialogs()
end)

test("bridge: status says why it is busy", function()
  local env = setup()
  local peer = connect(env)
  mock.impl(mock.song).rendering = true
  local reply = request(peer, "status")
  eq(reply.result.busy, "rendering")
  eq(reply.result.busyReason, "Renoise is rendering the song")
  no_dialogs()
end)

test("bridge: unix times are whole numbers whatever the clock does", function()
  local env = setup()
  mock.time = mock.time + 0.37
  tick()
  local discovery = json.decode(read(env.discovery_file()))
  eq(discovery.startedMs, math.floor(discovery.startedMs))
  no_dialogs()
end)

test("config: io.stat's mode is read in whichever format Renoise gives it", function()
  local config = T.load_payload_module("config")
  local access = config.group_or_other_access
  -- /dev/null is 0666: it names the format, then a mode is read in that format.
  local formats = {
    { null = 8630, private = 33152, readable = 33188, group = 33184, name = "st_mode" },
    { null = 438, private = 384, readable = 420, group = 416, name = "permission bits" },
    { null = 666, private = 600, readable = 644, group = 640, name = "octal digits" },
  }
  for _, f in ipairs(formats) do
    mock.stat_modes["/dev/null"] = f.null
    eq(access(f.private), false, f.name .. " 0600")
    eq(access(f.readable), true, f.name .. " 0644")
    eq(access(f.group), true, f.name .. " 0640")
  end
  -- 0400 and 0604 in permission bits (256, 388) are what the old guess got wrong.
  mock.stat_modes["/dev/null"] = 438
  eq(access(256), false, "0400")
  eq(access(388), true, "0604")
end)

test("bridge: connections it has retired don't count against the limits, and are forgotten when their peers close", function()
  local env = setup()
  local kumi = connect(env)
  local peers = {}
  for i = 1, 30 do
    peers[i] = mock.connect()
    peers[i]:send("GET / HTTP/1.1\r\n\r\n") -- retired at once, the peer stays connected
  end
  tick()
  local retired_count = 0
  for _, record in pairs(KUMI_BRIDGE.connections) do
    if record.closing then retired_count = retired_count + 1 end
  end
  eq(retired_count, 30)
  -- Retired ones leave room for the living: a new host still gets in, and Kumi is still there.
  local fresh = connect(env)
  eq(request(fresh, "ping", {}).ok, true)
  eq(request(kumi, "ping", {}).ok, true)
  eq(#mock.servers, 1, "the server was not remade: 30 is under the bound")
  for _, peer in ipairs(peers) do peer:close() end
  tick()
  retired_count = 0
  for _, record in pairs(KUMI_BRIDGE.connections) do
    if record.closing then retired_count = retired_count + 1 end
  end
  eq(retired_count, 0, "forgotten once their peers closed")
  no_dialogs()
end)

test("bridge: refused connections held open past the bound make the server anew, and Kumi reconnects", function()
  local env = setup()
  local kumi = connect(env)
  local first = mock.server()
  for i = 1, 40 do
    local peer = mock.connect()
    peer:send("GET / HTTP/1.1\r\n\r\n")
  end
  tick()
  mock.advance(1100)
  eq(#mock.servers, 2, "the server was made anew")
  ok(not first.running and not first.open, "the old one is closed, and its connections with it")
  ok(not kumi.open, "Kumi's connection went with it")
  local got = frames(kumi)
  eq(got[#got].type, "bye", "but it was told")
  eq(next(KUMI_BRIDGE.connections), nil, "nothing of the old connections is kept")
  ok(read(env.log_file()):find("making the server anew"), "logged")
  local again = connect(env)
  eq(request(again, "ping", {}).ok, true, "and Kumi gets back in")
  -- A bridge that has been remade doesn't go on remaking: nothing is held open now.
  mock.advance(3000)
  eq(#mock.servers, 2)
  no_dialogs()
end)

test("bridge: each Renoise logs to a file of its own, and logs of other launches a week old go", function()
  local env = setup({ name = "logs" })
  local ours = env.log_file()
  ok(read(ours), "this Renoise's log: " .. ours)
  -- Left by other launches (one the old shared name), one fresh, one that is not a bridge log.
  local old_one, old_shared, fresh, other = env.log_dir .. "/renoise-bridge-pid-111.log", env.log_dir .. "/renoise-bridge.log",
    env.log_dir .. "/renoise-bridge-pid-222.log", env.log_dir .. "/notes.log"
  for _, path in ipairs({ old_one, old_shared, fresh, other }) do write(path, "x\n") end
  local week_and_a_day = os.time() - 8 * 24 * 60 * 60
  mock.stat_mtimes[old_one], mock.stat_mtimes[old_shared], mock.stat_mtimes[other] = week_and_a_day, week_and_a_day, week_and_a_day
  request(connect(env), "renoise.reload")
  tick()
  eq(read(old_one), nil, "an old launch's log is removed")
  eq(read(old_shared), nil, "so is the old shared one")
  ok(read(fresh), "a recent one stays")
  ok(read(other), "and a file that is not a bridge log")
  ok(read(ours), "and its own")
  no_dialogs()
end)

test("bridge: a reload follows a changed discovery folder", function()
  local env = setup()
  local peer = connect(env)
  ok(read(env.discovery_file()), "written in the first folder")
  local moved = env.dir .. "/elsewhere-discovery"
  assert(os.execute("mkdir -p '" .. moved .. "'") == 0)
  reference(env, { discoveryDir = moved })
  request(peer, "renoise.reload")
  tick()
  eq(read(env.discovery_file()), nil, "removed from the old folder")
  local name = env.discovery_file():match("[^/]+$")
  ok(read(moved .. "/" .. name), "and written in the new one")
  no_dialogs()
end)

test("bridge: the bye goes out even behind a response of nearly two megabytes already part sent", function()
  local server = T.load_payload_module("server")
  local wire = T.load_payload_module("wire")
  local sent = {}
  local client = { is_open = true, send = function(_, text) sent[#sent + 1] = text; return true end }
  local big = string.rep("x", 1800 * 1024) .. "\n"
  local record = {
    key = "k", client = client, conn = { authenticated = function() return true end },
    outbox = { { text = big, offset = 1000 }, { text = "never-started\n", offset = 1 } },
  }
  local s = server.new({ connections = { k = record }, log = function() end, now_ms = function() return 0 end })
  s:close_all("quit")
  local all = table.concat(sent)
  eq(all, big:sub(1000) .. wire.encode_frame("bye", { reason = "quit" }), "the rest of the line, then the bye, nothing between")
end)

test("bridge: a huge song title is cut to 256 characters in the discovery file, the welcome and status", function()
  local env = setup({ song = { name = string.rep("n", 1024 * 1024) } })
  local peer, welcome = connect(env)
  local function check(project, where)
    eq(project.title, string.rep("n", 256) .. "\226\128\166", where .. " title")
    eq(project.name, project.title, where .. " name")
  end
  check(welcome.project, "welcome")
  check(request(peer, "status").result.project, "status")
  local discovery_text = read(env.discovery_file())
  ok(#discovery_text < 2048, "discovery file: " .. #discovery_text .. " bytes")
  check(json.decode(discovery_text).project, "discovery")
  no_dialogs()
end)

-- Names the files of a folder (the mock's os.filenames).
local function files_in(dir)
  return os.filenames(dir, { "*.json" })
end

test("bridge: a resource folder cut for the wire names the discovery file by the text written in it, as a host hashes it", function()
  -- (the cut is made small here so an ordinary path is cut; the real one is 1,024 characters)
  local env = setup({ patch = function(env)
    local path = env.bundle .. "payload/status.lua"
    local text = read(path)
    local changed = text:gsub("M%.MAX_PATH_CHARS = 1024", "M.MAX_PATH_CHARS = 40")
    assert(changed ~= text, "the cap was not found to lower")
    write(path, changed)
  end })
  local names = files_in(env.discovery_dir)
  eq(#names, 1, "one discovery file: " .. table.concat(names, ","))
  local written = json.decode(read(env.discovery_dir .. "/" .. names[1]))
  ok(written.resource:sub(-3) == "\226\128\166", "the resource was cut: " .. written.resource)
  eq(names[1], "renoise-4242-" .. sha256.hex(written.resource):sub(1, 16) .. ".json", "named by the text in the file")
  ok(names[1] ~= "renoise-4242-" .. sha256.hex(env.resource):sub(1, 16) .. ".json", "not by the whole path")
  no_dialogs()
end)

test("bridge: the discovery key of a cut resource is the one a host computes (the same vector as the host's test)", function()
  local discovery = T.load_payload_module("discovery")
  local cut = "/Users/K\195\188mi/Library/Preferences/Renoise/V3.5.4/\230\151\165\230\151\165\230\151\165\226\128\166"
  eq(discovery.key(cut), "7596818ce6a714b1")
end)

test("bridge: a huge song path or resource folder is cut too, so the discovery file stays small", function()
  local path = "/music/" .. string.rep("\230\151\165", 40000) .. ".xrns" -- 120 KB of CJK
  local env = setup({ song = { name = "", file_name = path } })
  local peer, welcome = connect(env)
  eq(#welcome.project.path, 7 + 1017 * 3 + 3, "the path is cut to 1,024 characters and an ellipsis")
  ok(welcome.project.path:sub(-3) == "\226\128\166")
  local discovery_text = read(env.discovery_file())
  ok(#discovery_text < 8192, "discovery file: " .. #discovery_text .. " bytes")
  local status = T.load_payload_module("status")
  local resource = "C:\\Users\\x\\" .. string.rep("\230\151\165", 40000) .. "\\Renoise\\V3.5.4"
  local cut = status.resource_text(resource .. "\\Scripts\\Tools\\com.kumi.Bridge.xrnx\\")
  eq(#cut, 11 + 1013 * 3 + 3, "the resource folder too")
  eq(status.resource_text("/Users/a/Renoise/V3.5.4/Scripts/Tools/com.kumi.Bridge.xrnx/"), "/Users/a/Renoise/V3.5.4", "a normal one is as it was")
  no_dialogs()
end)

test("bridge: a cut title never splits a character", function()
  local status = T.load_payload_module("status")
  local json_ = T.load_payload_module("json")
  local title = string.rep("\240\159\142\181", 300) -- 300 musical notes, 4 bytes each
  local cut = status.cap(title)
  ok(json_.valid_utf8(cut), "still UTF-8")
  eq(cut, string.rep("\240\159\142\181", 256) .. "\226\128\166")
  eq(status.cap("short"), "short")
  eq(status.cap(string.rep("a", 256)), string.rep("a", 256), "exactly the limit stays")
  eq(status.cap(string.rep("\195\169", 256)), string.rep("\195\169", 256), "256 two-byte characters stay")
end)
