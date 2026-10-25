-- Renoise's sockets on one side, wire connections on the other: loopback peers only, connection
-- limits, requests answered in order, heartbeats, events, and an outbox that sends big frames a
-- slice at a time.
--
-- This never closes a client's socket. In Renoise 3.5.4 closing one the server accepted (from a callback or a timer,
-- while the peer is still connected or after it has gone) ends the server: its next select fails with "Bad file
-- descriptor", is_running turns false, every connection is lost, and closing one the peer already closed crashes
-- Renoise outright. A connection the bridge is done with is retired instead: refused or told goodbye, then ignored, not
-- counted against the limits, and forgotten when the peer closes its end (a Kumi host does, on reading a refusal).

local import = ...
local wire = import("wire")

local sub = string.sub

local M = {}
M.__index = M

M.SEND_CHUNK = 64 * 1024
M.SEND_NOW_BYTES = 256 * 1024
M.SEND_TICK_BYTES = 1024 * 1024
M.MAX_FRAME_BYTES = 64 * 1024 * 1024 + 1024
-- Output the host didn't ask for (events, heartbeats) queued to a connection since the host last sent anything. Past
-- it the host has stopped reading (suspended, stopped in a debugger), and the socket's buffers would fill. Measured on
-- macOS, Renoise 3.5.4: a send to a full socket writes what fits and returns false, with no block and no queue. Renoise's
-- documentation says nothing of it, and Windows is unmeasured (to be measured in the Windows testing): a block there
-- would freeze the GUI, so output is held well short of a full socket on every platform. The connection is retired
-- instead; a host that is only listening says so now and then (crates/daw-mcp-server does), and one that was
-- retired reconnects when it next needs the bridge.
M.UNSOLICITED_LIMIT = 64 * 1024
-- Responses waiting to be sent to one connection, beyond the one going out (events and heartbeats are counted in bytes, by
-- UNSOLICITED_LIMIT, not here): a host that sends requests and doesn't read the answers is closed past this. Likewise requests waiting to be answered, and the time spent answering one
-- connection's requests in a go (the rest wait for the next tick), so a flood can't hold Renoise's GUI thread.
M.MAX_QUEUED_RESPONSES = 64
M.MAX_WAITING_REQUESTS = 256
M.REQUEST_BUDGET_MS = 8

local LOOPBACK_PEERS = { ["127.0.0.1"] = true, ["::1"] = true, ["::ffff:127.0.0.1"] = true }

-- A client as "address:port", read while its socket is open, or nil. Renoise answers "" and 65535 for a socket the peer
-- has already closed, and a key made of those is nobody's.
local function key_of(client)
  local ok, key = pcall(function()
    if not client.is_open then return nil end
    local address, port = client.peer_address, client.peer_port
    if type(address) ~= "string" or address == "" or type(port) ~= "number" or port <= 0 or port >= 65535 then return nil end
    return address .. ":" .. port
  end)
  return ok and key or nil
end

-- Connections retired but not yet closed by their peers are file descriptors Renoise can't get back (this side may not
-- close them), and a program's descriptors are few (a macOS app starts with 256, shared with its samples and songs).
-- Past this many open ones the server is made anew, which closes them all and drops Kumi's connection too: it
-- reconnects, and this only happens when something holds connections open on purpose or by a bug.
M.MAX_RETIRED_OPEN = 32
-- Retired records kept to be ignored, open or not; the oldest are forgotten.
M.MAX_RETIRED = 128

-- ctx: connections (kept by main.lua across payload reloads), now_ms(), unix_ms(), secret() -> secret or
-- nil and why, identity() -> the welcome's fields, on_request(record, request) -> result or raises
-- {code, message}, on_authenticated(record), log(level, message).
function M.new(ctx)
  return setmetatable({ ctx = ctx }, M)
end

-- Whether the peer has closed its end. (Asked of every connection two or three times a tick: the pcall takes a function of
-- the module's, not a closure made for it.)
local function client_open(record) return record.client.is_open end
local function is_gone(record)
  local ok, open = pcall(client_open, record)
  return not ok or not open
end

function M:counts()
  local waiting, authenticated = 0, 0
  for _, record in pairs(self.ctx.connections) do
    if not record.closing and record.conn then
      if record.conn:authenticated() then authenticated = authenticated + 1 else waiting = waiting + 1 end
    end
  end
  return waiting, authenticated
end

-- Retires a connection: whatever is queued for it still goes out, then nothing is read from it or sent to it.
function M:close(record, reason)
  if record.closing then return end
  record.closing = true
  record.retired_ms = self.ctx.now_ms()
  -- Its reads are of no use to anyone now.
  record.requests = {}
  if self.ctx.cancel_jobs then self.ctx.cancel_jobs(record) end
  record.job = nil
  self.ctx.log("info", "retiring " .. record.key .. ": " .. tostring(reason))
end

-- Retires an authenticated connection after telling its host (a bye): the host ends it on its side, which this side
-- can't do for it.
function M:dismiss(record, reason)
  if record.closing then return end
  if record.conn and record.conn:authenticated() then
    -- A line already part sent is finished first (it would otherwise end in a torn line, and the bye glued to it), then the
    -- bye, which needs no credit. What the host is waiting for from the tool but has not begun to get is dropped. A
    -- retired connection still takes `credit` frames, so a host that reads again completes the line and gets the bye.
    local head = record.outbox[1]
    record.outbox = (head and head.offset > 1) and { head } or {}
    record.requests = {}
    record.latest = nil
    record.outbox[#record.outbox + 1] = { text = wire.encode_frame("bye", { reason = "overload" }), offset = 1, uncounted = true }
    if #record.outbox == 1 then self:flush(record, M.SEND_NOW_BYTES) end
  end
  self:close(record, reason)
end

local function raw_send(self, record, text)
  local ok, sent = pcall(function()
    if not record.client.is_open then return false end
    return record.client:send(text)
  end)
  if not ok or not sent then
    record.outbox = {}
    record.gone = true
    self:close(record, "send failed")
    return false
  end
  return true
end

-- Sends what is queued, a chunk at a time, within the budget of this call and within the credit the host has granted
-- (nil credit: no limit, before the host is authenticated). Out of credit the rest waits, a line half out and all: the
-- host reads it when it grants more, and a host that has paused is never sent into a full socket.
function M:flush(record, budget)
  local sent = 0
  while record.outbox[1] and sent < budget do
    local item = record.outbox[1]
    -- (The welcome and a bye are not counted: the host counts what it reads after the welcome, and a bye is the end.)
    local credit = item ~= record.welcome_item and not item.uncounted and record.credit or nil
    if credit and credit <= 0 then break end
    local size = M.SEND_CHUNK
    if credit and credit < size then size = credit end
    local piece = sub(item.text, item.offset, item.offset + size - 1)
    if not raw_send(self, record, piece) then return sent end
    sent = sent + #piece
    if credit then record.credit = credit - #piece end
    item.offset = item.offset + #piece
    if item.offset > #item.text then
      table.remove(record.outbox, 1)
      if item == record.welcome_item then record.welcome_item = nil end
    end
  end
  return sent
end

-- The host has read `bytes` more: more may be sent, and what was held goes out.
function M:grant(record, bytes)
  if not record.credit then return end
  record.credit = math.min(record.credit + bytes, wire.CREDIT_MAX)
  if record.outbox[1] then self:flush(record, M.SEND_TICK_BYTES) end
end

-- The answers waiting in an outbox.
local function responses_waiting(record)
  local n = 0
  for _, item in ipairs(record.outbox) do
    if item.response then n = n + 1 end
  end
  return n
end

-- Queues text for a connection. `response` marks the answer to a request, which is counted against
-- MAX_QUEUED_RESPONSES; events and heartbeats are counted in bytes by `push`.
function M:send(record, text, response)
  if not text or record.gone then return end
  if response and responses_waiting(record) > M.MAX_QUEUED_RESPONSES then
    self:dismiss(record, "too many responses waiting to be read")
    return
  end
  record.outbox[#record.outbox + 1] = { text = text, offset = 1, response = response }
  if #record.outbox == 1 then self:flush(record, M.SEND_NOW_BYTES) end
end

-- Output nobody asked for. False, and the connection closed, once the host has let too much pile up unread.
function M:push(record, text)
  if not text or record.gone or record.closing then return false end
  local total = (record.unsolicited or 0) + #text
  if total > M.UNSOLICITED_LIMIT then
    self:dismiss(record, "the host stopped reading (" .. total .. " bytes of events and heartbeats unread)")
    return false
  end
  record.unsolicited = total
  self:send(record, text)
  return true
end

-- What can't wait for the timer, which stops behind a modal dialog and may run slowly while Renoise is in the
-- background (its sockets still call in): handshakes past their deadline are refused, and connections the peer has
-- closed stop counting against the limits. Closing a socket still waits for the tick.
function M:reap()
  local now = self.ctx.now_ms()
  for key, record in pairs(self.ctx.connections) do
    if record.conn and not record.closing then self:apply(record, record.conn:on_tick(now)) end
    local ok, open = pcall(function() return record.client.is_open end)
    if not ok or not open then self.ctx.connections[key] = nil end
  end
end

function M:accept(client)
  self:reap()
  -- Peer and port are read here, once, while the socket is surely open; one already closed is nobody to talk to.
  local key = key_of(client)
  if not key then return nil end
  local address = key:match("^(.*):%d+$")
  local now = self.ctx.now_ms()
  local record = { key = key, client = client, anchor = client, outbox = {}, requests = {}, accepted_ms = now }
  self.ctx.connections[record.key] = record
  if not LOOPBACK_PEERS[address] then
    self:close(record, "peer " .. address .. " is not loopback")
    return record
  end
  local waiting, authenticated = self:counts()
  if waiting >= wire.MAX_UNAUTHENTICATED or authenticated >= wire.MAX_AUTHENTICATED then
    self:send(record, wire.over_capacity())
    self:close(record, "too many connections")
    return record
  end
  local secret, why = self.ctx.secret()
  if not secret then
    self:close(record, "no secret: " .. tostring(why))
    return record
  end
  record.conn = wire.connection(secret, now)
  return record
end

function M:apply(record, actions)
  for _, action in ipairs(actions) do
    if action.send then
      self:send(record, action.send)
    elseif action.close then
      self:close(record, action.close)
    elseif action.credit then
      self:grant(record, action.credit)
    elseif action.authenticated then
      record.credit = wire.CREDIT_FLOOR
      record.welcome_item = record.outbox[#record.outbox] -- the welcome just queued, if it is not all out yet
      record.heartbeat_ms = self.ctx.now_ms() + wire.HEARTBEAT_MS
      if self.ctx.on_authenticated then self.ctx.on_authenticated(record) end
    elseif action.request then
      local waiting = record.requests
      waiting[#waiting + 1] = action.request
      if #waiting > M.MAX_WAITING_REQUESTS then
        self:dismiss(record, "too many requests waiting (" .. #waiting .. ")")
        return
      end
    end
  end
  self:run_requests(record)
end

-- Short methods that are answered at once, even behind a long read: the host matches answers by id, and the producer's
-- FOCUS pane follows what they say.
M.IMMEDIATE = { ping = true, status = true, focus = true }
-- (M5 m5-renoise: a listening job's state, its cancel and an audition's stop)
M.IMMEDIATE.listen_status, M.IMMEDIATE.listen_cancel, M.IMMEDIATE.audition_stop = true, true, true

-- A read is running: what is short among the waiting requests is answered now, out of order; the rest waits for the job.
function M:run_immediate(record)
  if record.closing or record.gone then return end
  local quick, rest = {}, {}
  for _, request in ipairs(record.requests) do
    if M.IMMEDIATE[request.method] then quick[#quick + 1] = request else rest[#rest + 1] = request end
  end
  if not quick[1] then return end
  record.requests = rest
  for _, request in ipairs(quick) do
    if record.closing or record.gone then return end
    self:answer(record, request)
  end
end

-- Answers a connection's waiting requests in order, for as long as the budget lasts (at least one): the rest wait for
-- the next tick.
function M:run_requests(record)
  local waiting = record.requests
  if not waiting[1] then return end
  if record.job then return self:run_immediate(record) end
  if record.closing then
    record.requests = {}
    return
  end
  local started = self.ctx.now_ms()
  local done = 0
  while waiting[done + 1] and not record.closing and not record.gone and not record.job do
    done = done + 1
    self:answer(record, waiting[done])
    if self.ctx.now_ms() - started >= M.REQUEST_BUDGET_MS then break end
  end
  if done > 0 then
    -- (One table.remove per request would make a long queue quadratic.)
    local rest = {}
    for i = done + 1, #waiting do rest[#rest + 1] = waiting[i] end
    record.requests = rest
  end
end

function M:answer(record, request)
  local deadline = request.deadlineMs
  if type(deadline) == "number" and self.ctx.unix_ms() >= deadline then
    return self:send(record, wire.error(request.id, "deadline", "the deadline passed before the work started"), true)
  end
  local ok, result = xpcall(self.ctx.on_request, debug.traceback, record, request)
  -- A long read comes back as a job to be run a slice at a time; the connection's later requests wait behind it, so
  -- answers keep their order.
  if ok and type(result) == "table" and result.kumi_job then
    record.job = self.ctx.start_job(record, request, result.kumi_job, result.kumi_wrote)
    return
  end
  self:respond(record, request, ok, result)
end

-- A job has ended: its answer goes out, then the requests that waited behind it are answered.
function M:finish_job(record, request, ok, value)
  record.job = nil
  if self.ctx.connections[record.key] ~= record or record.closing then return end
  self:respond(record, request, ok, value)
  self:run_requests(record)
end

-- What a request came to (the handler's result, or what it raised), as the response.
function M:respond(record, request, ok, result)
  if ok then
    local text, err = wire.ok(request.id, result)
    if not text then
      self.ctx.log("error", request.method .. ": could not encode the result: " .. tostring(err))
      text = wire.error(request.id, "internal", "could not encode the result: " .. tostring(err))
    end
    return self:send(record, text, true)
  end
  if type(result) == "table" then
    -- (A code that isn't text would fail the host's frame check and cost it the connection.)
    return self:send(record, wire.error(request.id, result.code ~= nil and tostring(result.code) or "failed", tostring(result.message or "")), true)
  end
  self.ctx.log("error", request.method .. " failed: " .. tostring(result))
  -- (the log has the traceback; the host gets the first line without the file and line it came from)
  local first = tostring(result):match("^[^\n]*")
  self:send(record, wire.error(request.id, "internal", (first:gsub("^[^%s:]*%.lua:%d+: ", ""))), true)
end

function M:receive(client, data)
  local key = key_of(client)
  -- A socket the peer closed as it sent: its record is reaped, and there is nothing to answer.
  if not key then return end
  local record = self.ctx.connections[key]
  if not record then record = self:accept(client) end
  if not record then return end
  record.client = client
  record.unsolicited = 0
  self:reap()
  -- The rest of a big frame goes out as the host talks, not only on the next tick.
  if record.outbox[1] then self:flush(record, M.SEND_TICK_BYTES) end
  if record.closing then
    -- A retired connection that still has output (a line part sent, a bye) takes the host's `credit` frames, and only those:
    -- the line is finished, and the bye reaches a host that reads again.
    if record.conn and record.outbox[1] and not record.gone then
      local ok, actions = xpcall(record.conn.on_bytes, debug.traceback, record.conn, data, self.ctx.now_ms(), self.ctx.identity)
      if ok then
        for _, action in ipairs(actions) do
          if action.credit then self:grant(record, action.credit) end
        end
      end
    end
    return
  end
  if not record.conn then return end
  local ok, actions = xpcall(record.conn.on_bytes, debug.traceback, record.conn, data, self.ctx.now_ms(), self.ctx.identity)
  if not ok then
    self.ctx.log("error", "connection " .. record.key .. ": " .. tostring(actions))
    return self:close(record, "internal error")
  end
  self:apply(record, actions)
end

-- Events about the state of something (not about a shift, which is told one by one, in order) are worth only their latest
-- value: while a connection's output waits (a host that has paused), only the latest of each is kept and sent after what is
-- held, so a producer who keeps working can't fill the host's queue.
M.LATEST_ONLY = { selection = true, transport = true, project = true }
local LATEST_ORDER = { "selection", "transport", "project" }

local function push_event(self, record, name, data)
  local text, err = record.conn:event(name, data)
  if text then
    self:push(record, text)
  else
    self.ctx.log("error", "could not encode event " .. name .. ": " .. tostring(err))
  end
end

-- The latest values kept while output waited, sent (in the order the events had, one each) once it has gone.
local function send_latest(self, record)
  local latest = record.latest
  if not latest then return end
  record.latest = nil
  for _, name in ipairs(LATEST_ORDER) do
    if latest[name] ~= nil and not record.closing then push_event(self, record, name, latest[name]) end
  end
end

-- An event to every authenticated connection, each numbered on its own connection.
function M:event(name, data)
  for _, record in pairs(self.ctx.connections) do
    if record.conn and not record.closing and record.conn:authenticated() then
      if M.LATEST_ONLY[name] and record.outbox[1] then
        record.latest = record.latest or {}
        record.latest[name] = data
      else
        -- (what was kept comes first: events keep the order they happened in)
        send_latest(self, record)
        push_event(self, record, name, data)
      end
    end
  end
end

function M:authenticated_count()
  local _, authenticated = self:counts()
  return authenticated
end

-- From the timer: handshake deadlines, heartbeats, outboxes, deferred closes.
function M:tick()
  local now = self.ctx.now_ms()
  for key, record in pairs(self.ctx.connections) do
    local conn = record.conn
    if conn and not record.closing then
      self:apply(record, conn:on_tick(now))
      if conn:authenticated() and not record.closing and now >= (record.heartbeat_ms or now) then
        -- (Not while output waits: the connection isn't idle, and heartbeats would pile up behind a paused host.)
        if not record.outbox[1] then
          self:push(record, wire.encode_frame("heartbeat", { ms = math.floor(now - record.accepted_ms) }))
        end
        record.heartbeat_ms = math.max((record.heartbeat_ms or now) + wire.HEARTBEAT_MS, now + wire.HEARTBEAT_MS / 2)
      end
    end
    if record.requests and record.requests[1] and not is_gone(record) then self:run_requests(record) end
    if record.outbox[1] then self:flush(record, M.SEND_TICK_BYTES) end
    if record.latest and not record.outbox[1] and not record.closing then send_latest(self, record) end
    if is_gone(record) then
      self.ctx.connections[key] = nil
      if self.ctx.cancel_jobs then self.ctx.cancel_jobs(record) end
      if not record.closing then self.ctx.log("info", "connection " .. key .. " went away") end
    end
  end
  self:forget_oldest_retired()
  self:check_descriptors()
end

-- Tells the owner (ctx.on_too_many_open) when too many retired connections are still open.
function M:check_descriptors()
  if not self.ctx.on_too_many_open then return end
  local open = 0
  for _, record in pairs(self.ctx.connections) do
    if record.closing and not is_gone(record) then open = open + 1 end
  end
  if open > M.MAX_RETIRED_OPEN then self.ctx.on_too_many_open(open) end
end

-- A peer that never closes its end of a retired connection can't make the list grow without end. (Counted first: a tick
-- with few retired builds no list.)
function M:forget_oldest_retired()
  local count = 0
  for _, record in pairs(self.ctx.connections) do
    if record.closing then count = count + 1 end
  end
  if count <= M.MAX_RETIRED then return end
  local retired = {}
  for key, record in pairs(self.ctx.connections) do
    if record.closing then retired[#retired + 1] = { key = key, at = record.retired_ms or 0 } end
  end
  table.sort(retired, function(a, b) return a.at < b.at end)
  for i = 1, #retired - M.MAX_RETIRED do self.ctx.connections[retired[i].key] = nil end
end

-- Says bye to authenticated connections and lets go of every connection (Renoise closes them with the server).
function M:close_all(reason)
  for key, record in pairs(self.ctx.connections) do
    if record.conn and record.conn:authenticated() and not record.closing then
      -- The bye must not wait behind output that hasn't started: only a line already part sent is finished first.
      local head = record.outbox[1]
      record.outbox = (head and head.offset > 1) and { head } or {}
      self:send(record, wire.encode_frame("bye", { reason = reason }))
      record.credit = nil -- the host is being let go of: nothing is held back for want of credit
      -- All of it, a line part sent included however big, then the bye. Measured on macOS, Renoise 3.5.4: a send to a
      -- full socket writes what fits, then returns false (no block, no queue). Unmeasured on Windows.
      self:flush(record, M.MAX_FRAME_BYTES)
    end
    self.ctx.connections[key] = nil
  end
end

return M
