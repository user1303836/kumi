-- The kumi-daw/1 wire, as crates/daw-bridge defines it: framing, frames, proofs, and one connection's
-- state machine. No sockets here; server.lua does what a connection's actions say.

local import = ...
local json = import("json")
local sha256 = import("sha256")

local byte, find, sub, format = string.byte, string.find, string.sub, string.format
local concat = table.concat

local M = {}

M.PROTOCOL = "kumi-daw/1"
M.MAX_PREAUTH_LINE = 1024
M.MAX_FRAME = 64 * 1024 * 1024
-- What this bridge may send before the host's first grant of credit (the welcome says so), and the most credit it
-- will hold. It sends only within the credit the host grants, so output never outruns a reader: it cannot know how much
-- a host's socket holds, so it starts with a floor, the host's first grant tops that up to its receive buffer, and the
-- host then grants what it reads.
M.CREDIT_FLOOR = 64 * 1024
M.CREDIT_MAX = 4 * 1024 * 1024
-- No answer is longer than this. It may be longer than the credit at the moment: it goes out as the host grants.
M.MAX_PAGE_BYTES = 256 * 1024
M.HANDSHAKE_TIMEOUT_MS = 3000
M.HEARTBEAT_MS = 1000
M.MAX_UNAUTHENTICATED = 4
M.MAX_AUTHENTICATED = 4
M.MAX_SAFE_INTEGER = 2 ^ 53 - 1

-- Framing: lines without "\n" or a trailing "\r", empty lines skipped. An error ends the stream.
-- Empty lines count toward the limit of the line after them: a line and the empty lines before
-- it are at most max bytes together (the empty lines' "\r" and "\n" in, the line's own out).

function M.framer(max_bytes)
  return { max = max_bytes, chunks = {}, first = 1, last = 0, pos = 1, parts = {}, count = 0, size = 0, blank = 0 }
end

function M.feed(framer, bytes)
  if framer.failed or bytes == "" then return end
  framer.last = framer.last + 1
  framer.chunks[framer.last] = bytes
end

local function fail(framer, err)
  framer.failed = err
  framer.chunks, framer.parts, framer.count, framer.size = {}, {}, 0, 0
  framer.first, framer.last, framer.pos = 1, 0, 1
  return nil, err
end

-- The next whole line; nil when it needs more bytes; nil and "oversized" or "invalid-utf8".
function M.next_line(framer)
  if framer.failed then return nil, framer.failed end
  while framer.first <= framer.last do
    local chunk = framer.chunks[framer.first]
    local nl = find(chunk, "\n", framer.pos, true)
    if nl then
      local piece = sub(chunk, framer.pos, nl - 1)
      framer.pos = nl + 1
      if framer.pos > #chunk then
        framer.chunks[framer.first] = nil
        framer.first, framer.pos = framer.first + 1, 1
      end
      local raw = framer.size + #piece
      local ends_cr = (#piece > 0 and byte(piece, -1) == 13) or (#piece == 0 and framer.count > 0 and byte(framer.parts[framer.count], -1) == 13)
      local length = ends_cr and raw - 1 or raw
      if length == 0 then
        framer.parts, framer.count, framer.size = {}, 0, 0
        framer.blank = framer.blank + raw + 1
        if framer.blank > framer.max then return fail(framer, "oversized") end
      else
        if framer.blank + length > framer.max then return fail(framer, "oversized") end
        framer.blank = 0
        local line = piece
        if framer.count > 0 then
          framer.count = framer.count + 1
          framer.parts[framer.count] = piece
          line = concat(framer.parts, "", 1, framer.count)
          framer.parts, framer.count, framer.size = {}, 0, 0
        end
        if ends_cr then line = sub(line, 1, -2) end
        if not json.valid_utf8(line) then return fail(framer, "invalid-utf8") end
        return line
      end
    else
      local rest = sub(chunk, framer.pos)
      framer.chunks[framer.first] = nil
      framer.first, framer.pos = framer.first + 1, 1
      framer.count = framer.count + 1
      framer.parts[framer.count] = rest
      framer.size = framer.size + #rest
      -- A line that arrives a byte at a time would hold a table slot for each: join them now and then.
      if framer.count >= 256 then
        framer.parts = { concat(framer.parts, "", 1, framer.count) }
        framer.count = 1
      end
    end
  end
  local held = framer.size
  if held > 0 and byte(framer.parts[framer.count], -1) == 13 then held = held - 1 end
  if framer.blank + held > framer.max then return fail(framer, "oversized") end
  return nil
end

local HTTP_STARTS = { "GET ", "POST ", "PUT ", "HEAD ", "DELETE ", "OPTIONS ", "PATCH ", "CONNECT ", "TRACE ", "PRI * HTTP/2" }

function M.looks_like_http(bytes)
  for _, start in ipairs(HTTP_STARTS) do
    if sub(bytes, 1, #start) == start then return true end
  end
  return false
end

-- Proofs and nonces

function M.valid_nonce(text)
  return type(text) == "string" and #text == 32 and find(text, "^[0-9a-f]+$") ~= nil
end

-- HMAC-SHA256(secret, "kumi-daw/1\n<role>\n<client nonce>\n<server nonce>") as lowercase hex.
function M.proof(secret, role, client_nonce, server_nonce)
  return sha256.hmac_hex(secret, M.PROTOCOL .. "\n" .. role .. "\n" .. client_nonce .. "\n" .. server_nonce)
end

function M.verify_proof(secret, role, client_nonce, server_nonce, received)
  return type(received) == "string" and #received == 64 and sha256.equal(M.proof(secret, role, client_nonce, server_nonce), received)
end

local entropy, counter = nil, 0

local function address()
  local t = setmetatable({}, nil)
  return tostring(t)
end

local function gather_entropy()
  local f = io.open("/dev/urandom", "rb")
  if f then
    local bytes = f:read(32)
    f:close()
    if bytes and #bytes == 32 then return bytes end
  end
  return tostring(os.time()) .. tostring(os.clock()) .. address() .. tostring(math.random())
end

-- 32 lowercase hex, unpredictable without the secret even where /dev/urandom is missing (Windows).
function M.nonce(secret)
  entropy = entropy or gather_entropy()
  counter = counter + 1
  local input = format("%d:%d:%.6f:%s", counter, os.time(), os.clock(), address())
  return sub(sha256.tohex(sha256.hmac(entropy .. (secret or ""), input)), 1, 32)
end

-- A number below 2^28 nobody can guess (/dev/urandom, else the nonce's hash where it is missing: Windows).
function M.random28()
  local f = io.open("/dev/urandom", "rb")
  local bytes = f and f:read(4)
  if f then f:close() end
  if bytes and #bytes == 4 then
    local b1, b2, b3, b4 = byte(bytes, 1, 4)
    return ((b1 % 16 * 256 + b2) * 256 + b3) * 256 + b4
  end
  return tonumber(sub(M.nonce(), 1, 7), 16)
end

-- Secrets: one line of 32 to 512 printable ASCII characters without spaces.
function M.secret_from_file_text(text)
  if sub(text, -1) == "\n" then
    text = sub(text, 1, -2)
    if sub(text, -1) == "\r" then text = sub(text, 1, -2) end
  end
  if #text < 32 then return nil, "too-short" end
  if #text > 512 then return nil, "too-long" end
  if find(text, "[^\33-\126]") then return nil, "bad-byte" end
  return text
end

-- Frames

local null = json.null

local function is_string(v) return type(v) == "string" end
local function optional_string(v) return v == nil or v == null or type(v) == "string" end
local function is_object(v) return type(v) == "table" and v ~= null and not json.is_array(v) end

-- An unsigned integer member, judged by how it was written: 1.0, 1e2 and -0 are the same numbers as 1, 100 and 0 to Lua
-- and refused for a u64 by serde_json, so the two ends of the wire agree by looking at the text. max is the largest
-- value, or nil for 2^64-1 (compared as text, a double can't tell it from 2^64).
local U64_MAX = "18446744073709551615"
local function uint(numbers, key, max)
  local lexeme = numbers[key]
  if not lexeme or not find(lexeme, "^%d+$") then return false end
  if max then return tonumber(lexeme) <= max end
  return #lexeme < 20 or (#lexeme == 20 and lexeme <= U64_MAX)
end
local function safe(numbers, key, allow_zero)
  return uint(numbers, key, M.MAX_SAFE_INTEGER) and (allow_zero or tonumber(numbers[key]) > 0)
end

local function strings(v)
  if v == nil then return true end
  if type(v) ~= "table" or not json.is_array(v) then return false end
  for _, item in ipairs(v) do
    if type(item) ~= "string" then return false end
  end
  return true
end

local function project_ok(v)
  if v == nil or v == null then return true end
  return is_object(v) and optional_string(v.id) and optional_string(v.name) and optional_string(v.path)
end

-- Each takes the frame and the text of its top-level numbers.
local CHECKS = {
  hello = function(f) return is_string(f.protocol) and is_string(f.nonce) and optional_string(f.client) end,
  challenge = function(f) return is_string(f.nonce) and is_string(f.proof) end,
  auth = function(f) return is_string(f.proof) end,
  welcome = function(f, n)
    return is_string(f.session) and is_string(f.daw) and is_string(f.bridge) and is_string(f.dawVersion)
      and uint(n, "pid", 4294967295) and is_string(f.resource) and project_ok(f.project)
      and uint(n, "maxFrameBytes") and uint(n, "heartbeatMs") and strings(f.methods) and strings(f.events)
      and (f.creditBytes == nil or f.creditBytes == null or uint(n, "creditBytes"))
  end,
  refused = function(f) return is_string(f.reason) and optional_string(f.protocol) end,
  request = function(f, n)
    if f.params == nil then f.params = json.object() end
    return safe(n, "id") and is_string(f.method) and is_object(f.params)
      and (f.deadlineMs == nil or f.deadlineMs == null or uint(n, "deadlineMs"))
  end,
  response = function(f, n)
    local has_error = f.error ~= nil and f.error ~= null
    if has_error and not (is_object(f.error) and is_string(f.error.code) and is_string(f.error.message)) then return false end
    return safe(n, "id") and type(f.ok) == "boolean" and f.ok ~= has_error
  end,
  event = function(f, n) return safe(n, "seq") and is_string(f.name) end,
  heartbeat = function(f, n) return safe(n, "ms", true) end,
  credit = function(f, n) return safe(n, "bytes") end,
  bye = function(f) return is_string(f.reason) end,
}

-- A frame read from one line, or nil and why. Unknown types come back with type "unknown".
function M.parse_frame(line)
  local numbers = {}
  local value, err = json.decode(line, { numbers = numbers })
  if value == nil then return nil, "not JSON: " .. tostring(err) end
  if not is_object(value) then return nil, "not a JSON object" end
  if type(value.type) ~= "string" then return nil, "no type" end
  local check = CHECKS[value.type]
  if not check then return { type = "unknown" } end
  if not check(value, numbers) then return nil, "invalid " .. value.type .. " frame" end
  return value
end

-- One line: {"type":<kind>, ...fields}, newline included. nil and why when fields can't be encoded.
function M.encode_frame(kind, fields)
  local head = '{"type":' .. json.encode_string(kind)
  if fields == nil or next(fields) == nil then return head .. "}\n" end
  local body, err = json.encode(json.object(fields))
  if not body then return nil, err end
  return head .. "," .. sub(body, 2) .. "\n"
end

function M.ok(id, result)
  return M.encode_frame("response", { id = id, ok = true, result = result == nil and json.object() or result })
end

-- Sent at accept, before reading, to a connection over the limits.
function M.over_capacity()
  return M.encode_frame("refused", { reason = "connections" })
end

function M.error(id, code, message)
  return M.encode_frame("response", { id = id, ok = false, error = { code = code, message = message } })
end

-- One connection, on the bridge's side. Its methods return actions, in order:
-- {send = line}, {close = reason}, {authenticated = true}, {request = frame}.

local Connection = {}
Connection.__index = Connection

function M.connection(secret, now_ms, server_nonce, session)
  return setmetatable({
    secret = secret,
    server_nonce = server_nonce or M.nonce(secret),
    session = session or M.nonce(secret),
    client_nonce = "",
    accepted_ms = now_ms,
    framer = M.framer(M.MAX_PREAUTH_LINE),
    state = "hello",
    first_byte_seen = false,
    next_seq = 1,
  }, Connection)
end

function Connection:authenticated()
  return self.state == "ready"
end

function Connection:closed()
  return self.state == "closed"
end

function Connection:close(actions, reason)
  self.state = "closed"
  actions[#actions + 1] = { close = reason }
  return actions
end

function Connection:refuse(actions, reason, protocol)
  actions[#actions + 1] = { send = M.encode_frame("refused", { reason = reason, protocol = protocol }) }
  return self:close(actions, "refused:" .. reason)
end

-- What to do with bytes read at now_ms (the clock the connection was made with). The handshake's
-- deadline is checked first, so a peer that keeps sending can't outlast it. identity() gives the
-- welcome's fields besides type, session and maxFrameBytes; it's called only once the host has
-- proved it knows the secret.
function Connection:on_bytes(bytes, now_ms, identity)
  local actions = self:on_tick(now_ms, {})
  if self.state == "closed" or bytes == "" then return actions end
  if not self.first_byte_seen then
    self.first_byte_seen = true
    if byte(bytes, 1) ~= 123 then
      return self:close(actions, M.looks_like_http(bytes) and "http" or "not-json")
    end
  end
  M.feed(self.framer, bytes)
  while self.state ~= "closed" do
    local line, err = M.next_line(self.framer)
    if err then
      if self.state == "ready" then return self:close(actions, "protocol:" .. err) end
      return self:refuse(actions, err == "oversized" and "limit" or "handshake")
    end
    if not line then break end
    local frame, why = M.parse_frame(line)
    if self.state == "hello" then
      if not frame or frame.type ~= "hello" then return self:refuse(actions, "handshake") end
      if frame.protocol ~= M.PROTOCOL then return self:refuse(actions, "protocol", M.PROTOCOL) end
      if not M.valid_nonce(frame.nonce) then return self:refuse(actions, "handshake") end
      local proof = M.proof(self.secret, "server", frame.nonce, self.server_nonce)
      self.client_nonce = frame.nonce
      self.state = "auth"
      actions[#actions + 1] = { send = M.encode_frame("challenge", { nonce = self.server_nonce, proof = proof }) }
    elseif self.state == "auth" then
      if not frame or frame.type ~= "auth" then return self:refuse(actions, "handshake") end
      if not M.verify_proof(self.secret, "client", self.client_nonce, self.server_nonce, frame.proof) then
        return self:refuse(actions, "auth")
      end
      local welcome = identity()
      welcome.session = self.session
      welcome.maxFrameBytes = M.MAX_FRAME
      welcome.heartbeatMs = welcome.heartbeatMs or M.HEARTBEAT_MS
      welcome.creditBytes = welcome.creditBytes or M.CREDIT_FLOOR
      welcome.methods = json.array(welcome.methods or {})
      welcome.events = json.array(welcome.events or {})
      local text, encode_err = M.encode_frame("welcome", welcome)
      if not text then return self:close(actions, "welcome: " .. tostring(encode_err)) end
      self.state = "ready"
      self.framer.max = M.MAX_FRAME
      actions[#actions + 1] = { send = text }
      actions[#actions + 1] = { authenticated = true }
    else
      if not frame then return self:close(actions, "protocol:" .. tostring(why)) end
      if frame.type == "request" then
        actions[#actions + 1] = { request = frame }
      elseif frame.type == "credit" then
        actions[#actions + 1] = { credit = frame.bytes }
      end
    end
  end
  return actions
end

-- The handshake's deadline, for the timer (a peer that sends nothing); on_bytes checks it too, with the list it then adds
-- its own actions to (`actions`). A tick with nothing to do and no list of its own answers the one empty list, which the
-- server only reads: twenty ticks a second made a table each.
local NO_ACTIONS = {}
function Connection:on_tick(now_ms, actions)
  if (self.state == "hello" or self.state == "auth") and now_ms - self.accepted_ms >= M.HANDSHAKE_TIMEOUT_MS then
    return self:refuse(actions or {}, "timeout")
  end
  return actions or NO_ACTIONS
end

-- An event line, numbered from 1 on this connection; nil before auth.
function Connection:event(name, data)
  if self.state ~= "ready" then return nil end
  local text, err = M.encode_frame("event", { seq = self.next_seq, name = name, data = data == nil and null or data })
  if not text then return nil, err end
  self.next_seq = self.next_seq + 1
  return text
end

return M
