local T = ...
local wire = T.load_payload_module("wire")
local json = T.load_payload_module("json")
local test, eq, ok = T.test, T.eq, T.ok

local function lines_of(framer, chunks)
  local out = {}
  for _, chunk in ipairs(chunks) do
    wire.feed(framer, chunk)
    while true do
      local line, err = wire.next_line(framer)
      if err then
        out[#out + 1] = "error:" .. err
        return out
      end
      if not line then break end
      out[#out + 1] = line
    end
  end
  return out
end

test("wire: framing keeps a limit change between lines", function()
  local framer = wire.framer(8)
  wire.feed(framer, '{"a":1}\n' .. string.rep("x", 20) .. "\n")
  eq(wire.next_line(framer), '{"a":1}')
  framer.max = 64
  eq(wire.next_line(framer), string.rep("x", 20), "the second line is measured against the new limit")
end)

test("wire: framing one byte at a time, with CRLF split across reads", function()
  local text = '{"k":"v"}\r\n\r\n[1]\r\n'
  local framer, out = wire.framer(100), {}
  for i = 1, #text do
    for _, line in ipairs(lines_of(framer, { text:sub(i, i) })) do out[#out + 1] = line end
  end
  eq(#out, 2)
  eq(out[1], '{"k":"v"}')
  eq(out[2], "[1]")
end)

test("wire: a big line in many reads", function()
  local framer = wire.framer(wire.MAX_FRAME)
  local piece = string.rep("y", 65536)
  for _ = 1, 64 do wire.feed(framer, piece) end
  eq(wire.next_line(framer), nil)
  wire.feed(framer, "\n")
  eq(#wire.next_line(framer), 64 * 65536)
end)

test("wire: nonces are 32 lowercase hex and don't repeat", function()
  local seen = {}
  for _ = 1, 1000 do
    local n = wire.nonce("s")
    ok(wire.valid_nonce(n), n)
    ok(not seen[n])
    seen[n] = true
  end
  ok(not wire.valid_nonce(string.rep("A", 32)))
  ok(not wire.valid_nonce(string.rep("a", 31)))
end)

test("wire: frames parse with the daw-bridge rules", function()
  eq(wire.parse_frame('{"type":"request","id":1,"method":"ping"}').params ~= nil, true)
  ok(json.is_array(json.decode("[]")))
  eq(wire.parse_frame('{"type":"later","x":1}').type, "unknown")
  for _, bad in ipairs({
    "[1]", '{"no":"type"}', '{"type":1}', '{"type":"request","id":0,"method":"ping"}',
    '{"type":"request","id":9007199254740992,"method":"ping"}', '{"type":"request","id":1.5,"method":"ping"}',
    '{"type":"request","id":1,"method":"ping","params":[]}', '{"type":"request","id":1,"method":"ping","params":null}',
    '{"type":"request","id":1}', '{"type":"response","id":1,"ok":true,"error":{"code":"x","message":"y"}}',
    '{"type":"response","id":1,"ok":false}', '{"type":"heartbeat","ms":-1}', '{"type":"event","seq":0,"name":"x"}',
    '{"type":"hello","protocol":"kumi-daw/1"}', '{"type":"bye"}',
  }) do
    eq(wire.parse_frame(bad), nil, bad)
  end
  ok(wire.parse_frame('{"type":"heartbeat","ms":0}'))
  ok(wire.parse_frame('{"type":"response","id":3,"ok":false,"error":{"code":"busy","message":"rendering"}}'))
  ok(wire.parse_frame('{"type":"hello","protocol":"x","nonce":"y","client":null}'))
end)

test("wire: frames encode with the type first", function()
  eq(wire.encode_frame("refused", { reason = "auth" }), '{"type":"refused","reason":"auth"}\n')
  eq(wire.encode_frame("bye", {}), '{"type":"bye"}\n')
  local frame = json.decode(wire.ok(7, nil))
  eq(frame.type, "response")
  eq(frame.id, 7)
  eq(frame.ok, true)
  eq(next(frame.result), nil)
  eq(wire.ok(8, json.array()):match('"result":%[%]') ~= nil, true)
end)

test("wire: a connection's events are numbered from 1, and only after auth", function()
  local conn = wire.connection(string.rep("k", 40), 0)
  eq(conn:event("project", json.null), nil)
  conn.state = "ready"
  eq(json.decode(conn:event("project", json.null)).seq, 1)
  local second = json.decode(conn:event("selection", { track = 2 }))
  eq(second.seq, 2)
  eq(second.name, "selection")
  eq(second.data.track, 2)
end)

test("wire: a credit frame after auth becomes a credit action; a bad one closes the connection", function()
  local conn = wire.connection(string.rep("k", 40), 0)
  conn.state = "ready"
  conn.framer.max = wire.MAX_FRAME
  local actions = conn:on_bytes('{"type":"credit","bytes":65536}\n{"type":"request","id":1,"method":"ping"}\n', 0, function() error("no") end)
  eq(#actions, 2)
  eq(actions[1].credit, 65536)
  eq(actions[2].request.id, 1, "in order with what follows")
  actions = conn:on_bytes('{"type":"credit","bytes":0}\n', 0, function() error("no") end)
  ok(actions[#actions].close, "a credit of nothing is not a frame")
end)

test("wire: an epoch past 2^32 goes to hex and back on a 32-bit long too (Windows)", function()
  local wire32 = T.load_with_32_bit_long("wire")
  for _, n in ipairs({ 0, 255, 2 ^ 31, 2 ^ 32 - 1, 2 ^ 32, 0x123456789ab, 2 ^ 44 - 1, 2 ^ 44 + 5, 2 ^ 52 + 1, 2 ^ 53 }) do
    local text = wire32.hex(n)
    eq(text, string.format("%x", n), "the text as on 64 bits")
    eq(wire32.from_hex(text), n, "read back: " .. text)
  end
  -- Anything else reads as tonumber reads it.
  for _, text in ipairs({ "", "zz", "ff", "12345678z" }) do
    eq(wire32.from_hex(text), wire.from_hex(text), text)
  end
end)
