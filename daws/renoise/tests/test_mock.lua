-- What the mock Renoise promises to do wrong in the same ways the real one does, so the bridge's tests mean something.
-- Each of these was seen in Renoise 3.5.4 (docs/notes: the Kumi tool's server.lua header).
local T = ...
local mock = T.mock
local test, eq, ok = T.test, T.eq, T.ok

-- A running server with one peer connected to it.
local function server_with_a_peer()
  mock.reset()
  local server = assert(renoise.Socket.create_server("127.0.0.1", 9780, renoise.Socket.PROTOCOL_TCP))
  local accepted
  server:run({ socket_accepted = function(client) accepted = client end, socket_message = function() end })
  local peer = mock.connect()
  return server, peer, accepted
end

test("mock: closing a client the server accepted ends the server, as in Renoise 3.5.4", function()
  local server, peer, client = server_with_a_peer()
  ok(server.is_running)
  client:close()
  ok(not server.is_running, "its select fails with a bad file descriptor and the server stops")
  ok(not peer.open, "and the connection is gone")
  eq(#mock.crashes, 1)
  mock.crashes = {} -- (expected here: this is the check that the mock notices)
end)

test("mock: closing a client the peer had already closed is fatal, not a quiet no-op", function()
  local _, peer, client = server_with_a_peer()
  peer:close()
  local raised = not pcall(function() client:close() end)
  ok(raised, "it raises, and a pcall around it doesn't make it harmless")
  ok(#mock.crashes >= 1 and mock.crashes[1]:find("peer had closed"), "and is recorded, which fails any test that does it")
  mock.crashes = {}
end)

test("mock: the properties of a closed client raise", function()
  local _, peer, client = server_with_a_peer()
  ok(client.peer_port > 0)
  peer:close()
  ok(not client.is_open)
  ok(not pcall(function() return client.peer_address end), "peer_address")
  ok(not pcall(function() return client.peer_port end), "peer_port")
end)

test("mock: a send can fail, and one to a peer that stopped reading counts when it would block", function()
  local _, peer, client = server_with_a_peer()
  eq(client:send("ok"), true)
  peer.send_fails = true
  eq(client:send("lost"), false)
  peer.send_fails = false
  peer:stop_reading()
  mock.socket_buffer_bytes = 1000
  client:send(string.rep("x", 600))
  eq(mock.blocked_sends, 0)
  eq(client:send(string.rep("x", 600)), false, "into a socket with room for 400 more, a send writes what fits and returns false")
  eq(mock.blocked_sends, 1, "the buffer is full: a real send would block the GUI thread")
  eq(#table.concat(peer.inbox), 2 + 1000, "the peer holds \"ok\" and what fitted: 600 and 400 bytes, the line torn")
  eq(client:send("y"), false)
  eq(mock.blocked_sends, 2)
  eq(#table.concat(peer.inbox), 2 + 1000, "and nothing more")
end)

test("mock: timers can be held while time and the sockets go on", function()
  mock.reset()
  local fired = 0
  local tick = function() fired = fired + 1 end
  mock.tool:add_timer(tick, 50)
  mock.advance(500)
  eq(fired, 10)
  mock.stall_timers = true
  local before = mock.time
  mock.advance(5000)
  eq(fired, 10, "none ran while they were held")
  eq(mock.time, before + 5000, "but time passed")
  mock.stall_timers = false
  mock.advance(100)
  ok(fired > 10, "and they run again")
end)

test("mock: a closed server closes its clients with it", function()
  local server, peer = server_with_a_peer()
  server:close()
  ok(not peer.open)
  eq(#mock.crashes, 0, "which is not a client being closed")
end)

test("mock: a line's text is every column of it, the hidden ones too", function()
  mock.reset({})
  mock.song = mock.make_song()
  local line = mock.song:pattern(1):track(1):line(1)
  local note, effect = line:note_column(1), line:effect_column(1)
  note.note_value, note.instrument_value, note.volume_value, note.panning_value = 48, 1, 0x40, 0x20
  note.delay_value, note.effect_number_value, note.effect_amount_value = 0x10, 0x0A, 5
  effect.number_value, effect.amount_value = 0x0D, 9
  local cells = { "C-4014020100A05" }
  for c = 2, 12 do cells[c] = "---........0000" end
  cells[13] = "0D09"
  for c = 14, 20 do cells[c] = "0000" end
  eq(tostring(line), table.concat(cells, " | "))
end)
