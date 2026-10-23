-- The kumi-daw/1 conformance vectors in daws/own/daw-bridge/tests/vectors, which every implementation passes.
local T = ...
local wire = T.load_payload_module("wire")
local json = T.load_payload_module("json")
local sha256 = T.load_payload_module("sha256")
local discovery = T.load_payload_module("discovery")
local test, eq, ok, same = T.test, T.eq, T.ok, T.same

local dir = os.getenv("KUMI_DAW_VECTORS") or (T.tests_dir .. "../../own/daw-bridge/tests/vectors/")
if dir:sub(-1) ~= "/" then dir = dir .. "/" end

local function vectors(name)
  local f = assert(io.open(dir .. name, "rb"), "no vectors at " .. dir .. name)
  local value = assert(json.decode(f:read("*a")))
  f:close()
  return value
end

local function unhex(h)
  return (h:gsub("%x%x", function(x) return string.char(tonumber(x, 16)) end))
end

test("vectors: crypto", function()
  local v = vectors("crypto.json")
  ok(#v.sha256 > 0 and #v.hmac > 0)
  for i, case in ipairs(v.sha256) do
    eq(sha256.hex(unhex(case.dataHex)), case.sha256, "sha256 case " .. i)
  end
  for i, case in ipairs(v.hmac) do
    eq(sha256.hmac_hex(unhex(case.keyHex), unhex(case.dataHex)), case.hmac, "hmac case " .. i)
  end
end)

test("vectors: framing", function()
  for _, case in ipairs(vectors("framing.json").cases) do
    local framer = wire.framer(case.maxBytes)
    local got = {}
    for _, chunk in ipairs(case.chunksHex) do
      wire.feed(framer, unhex(chunk))
      while true do
        local line, err = wire.next_line(framer)
        if err then got[#got + 1] = { error = err } break end
        if not line then break end
        got[#got + 1] = { line = line }
      end
      if got[#got] and got[#got].error then break end
    end
    same(got, case.expect, case.name)
  end
end)

test("vectors: frame limits", function()
  local v = vectors("frames.json")
  eq(v.maxJsonDepth, json.MAX_DEPTH)
  ok(#v.parse > 0 and #v.encode > 0)
  for _, case in ipairs(v.parse) do
    eq(wire.parse_frame(case.line) ~= nil, case.ok, case.name)
  end
  for _, case in ipairs(v.encode) do
    local result = json.array({})
    for _ = 2, case.resultDepth do result = json.array({ result }) end
    eq(wire.ok(1, result) ~= nil, case.sent, "a result " .. case.resultDepth .. " levels deep")
  end
end)

test("vectors: handshake proofs and secret files", function()
  local v = vectors("handshake.json")
  eq(v.protocol, wire.PROTOCOL)
  for i, case in ipairs(v.cases) do
    eq(wire.PROTOCOL .. "\nserver\n" .. case.clientNonce .. "\n" .. case.serverNonce, case.serverMessage)
    eq(wire.proof(case.secret, "server", case.clientNonce, case.serverNonce), case.serverProof, "server proof " .. i)
    eq(wire.proof(case.secret, "client", case.clientNonce, case.serverNonce), case.clientProof, "client proof " .. i)
  end
  for i, case in ipairs(v.secretFiles) do
    local secret, err = wire.secret_from_file_text(case.text)
    eq(secret, case.secret, "secret file " .. i)
    eq(err, case.error, "secret file " .. i)
  end
end)

test("vectors: discovery file names", function()
  for _, case in ipairs(vectors("discovery.json").cases) do
    eq(discovery.key(case.resource), case.key, case.resource)
    if case.daw == "renoise" then eq(discovery.file_name(case.pid, case.resource), case.fileName) end
  end
end)

local function welcome_fields(identity)
  local fields = {}
  for k, v in pairs(identity) do fields[k] = v end
  fields.methods = json.array({ "ping", "status" })
  fields.events = json.array()
  return fields
end

-- What a close action looks like in the vectors.
local function close_word(reason)
  if reason == "http" or reason == "not-json" then return "silent" end
  if reason:match("^refused:") then return reason end
  if reason:match("^protocol") then return "protocol" end
  return reason
end

test("vectors: the server side of a connection", function()
  local v = vectors("server.json")
  same(assert(json.decode(wire.over_capacity():sub(1, -2))), v.overCapacity, "over capacity")
  for _, case in ipairs(v.cases) do
    local conn = wire.connection(v.secret, 0, v.serverNonce, v.session)
    local closed, clock = false, 0
    for s, step in ipairs(case.steps) do
      local where = case.name .. ", step " .. s
      ok(not closed, where .. ": nothing is read after a close")
      local actions
      if step.inHex then
        clock = step.atMs or clock
        actions = conn:on_bytes(unhex(step.inHex), clock, function() return welcome_fields(v.welcomeIdentity) end)
      else
        clock = step.tickMs
        actions = conn:on_tick(clock)
      end
      eq(#actions, #step.expect, where .. ": actions")
      for i, expected in ipairs(step.expect) do
        local action = actions[i]
        if expected.send then
          ok(action.send, where .. ": a send")
          eq(action.send:sub(-1), "\n")
          local sent = assert(json.decode(action.send:sub(1, -2)))
          if expected.send.type == "welcome" then
            for _, key in ipairs({ "type", "session", "maxFrameBytes", "heartbeatMs" }) do
              eq(sent[key], expected.send[key], where .. ": welcome " .. key)
            end
          else
            same(sent, expected.send, where)
          end
        elseif expected.close then
          ok(action.close, where .. ": a close")
          eq(close_word(action.close), expected.close, where)
          closed = true
        elseif expected.authenticated then
          eq(action.authenticated, true, where)
        elseif expected.request then
          ok(action.request, where .. ": a request")
          eq(action.request.id, expected.request.id)
          eq(action.request.method, expected.request.method)
          same(action.request.params, expected.request.params, where)
        end
      end
    end
  end
end)
