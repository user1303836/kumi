-- Kumi's bridge for Renoise.
-- This loader stays small and stable: it owns the listening socket, the timer and Renoise's hooks,
-- and loads the payload (payload/*.lua), which a reload swaps without closing the port.

local tool = renoise.tool()
local BUNDLE = tool.bundle_path
if not BUNDLE:match("[/\\]$") then BUNDLE = BUNDLE .. "/" end

local TICK_MS = 50
-- While a read is pending its slices run on this faster timer (the 50 ms one alone would make a big read take minutes).
local JOB_TICK_MS = 10
-- A payload whose start raised is tried again after this long.
local START_RETRY_MS = 5000

-- Everything that must stay referenced (the sockets above all) hangs off this table.
local state = {
  server = nil,
  connections = {},
  payload = nil,
  retry_start_ms = 0,
  reload_requested = false,
}
KUMI_BRIDGE = state

local function report(where, err)
  local payload = state.payload
  local ok = payload and payload.log_error and pcall(payload.log_error, where, err)
  if not ok then print("Kumi bridge: " .. where .. ": " .. tostring(err)) end
end

-- Renoise disables a notifier behind a modal dialog when it raises, so nothing raises past here.
local function guard(where, fn)
  return function(...)
    local ok, result = xpcall(fn, debug.traceback, ...)
    if not ok then
      report(where, result)
      return nil
    end
    return result
  end
end

local function load_payload()
  local loaded = {}
  local function import(name)
    local module = loaded[name]
    if module == nil then
      -- Read with io.open, which Renoise makes take UTF-8 paths on Windows; loadfile would use the ANSI code page,
      -- and a user folder with letters outside it would leave the tool without its payload.
      local file, open_err = io.open(BUNDLE .. "payload/" .. name .. ".lua", "rb")
      if not file then error(open_err, 0) end
      local text = file:read("*a")
      file:close()
      local chunk, err = loadstring(text, "@payload/" .. name .. ".lua")
      if not chunk then error(err, 0) end
      module = chunk(import)
      loaded[name] = module
    end
    return module
  end
  return import("bridge")
end

local function call_payload(name, ...)
  local payload = state.payload
  if payload and payload[name] then return payload[name](...) end
end

local forwarder = {
  socket_accepted = guard("socket_accepted", function(client) call_payload("on_accept", client) end),
  socket_message = guard("socket_message", function(client, data) call_payload("on_message", client, data) end),
  socket_error = guard("socket_error", function(message) call_payload("on_socket_error", message) end),
}

local LOOPBACK = { ["127.0.0.1"] = true, ["::1"] = true, ["localhost"] = true }

local host = { bundle_path = BUNDLE, state = state, tick_ms = TICK_MS }

function host.close_server()
  local server = state.server
  state.server = nil
  if server then
    pcall(function()
      if server.is_running then server:stop() end
      server:close()
    end)
  end
end

-- Listens on 127.0.0.1 only; returns the server, or nil and why.
function host.listen(port)
  host.close_server()
  local server, err = renoise.Socket.create_server("127.0.0.1", port, renoise.Socket.PROTOCOL_TCP)
  if not server then return nil, err or "could not listen" end
  if not LOOPBACK[server.local_address] then
    pcall(function() server:close() end)
    return nil, "listening on " .. tostring(server.local_address) .. " is not loopback"
  end
  server:run(forwarder)
  state.server = server
  return server
end

function host.request_reload()
  state.reload_requested = true
end

local function now_ms()
  return os.clock() * 1000
end

-- The payload is in place only once its start has returned. One that raises is stopped and tried again in a while,
-- not left half started (its tick raising twenty times a second).
local function start()
  if state.payload then return end
  local ok, payload = xpcall(load_payload, debug.traceback)
  if not ok then
    report("load", payload)
    state.retry_start_ms = now_ms() + START_RETRY_MS
    return
  end
  local started, err = xpcall(payload.start, debug.traceback, host, false)
  if not started then
    report("start", err)
    pcall(payload.stop, host, false)
    host.close_server()
    state.retry_start_ms = now_ms() + START_RETRY_MS
    return
  end
  state.payload = payload
end

local function stop()
  if state.payload then guard("stop", state.payload.stop)(host, false) end
  state.payload = nil
  -- Closing the server closes its clients. Closing a client ourselves ends the server and can crash Renoise.
  host.close_server()
  for key in pairs(state.connections) do state.connections[key] = nil end
end

local function swap_payload()
  local ok, fresh = xpcall(load_payload, debug.traceback)
  if not ok then
    report("reload", fresh)
    call_payload("on_reload_failed", fresh)
    return
  end
  local old = state.payload
  if old then guard("stop", old.stop)(host, true) end
  local started, err = xpcall(fresh.start, debug.traceback, host, true)
  if started then
    state.payload = fresh
    return
  end
  -- The new payload couldn't start: the old one, which was working, takes over again.
  report("reload", err)
  pcall(fresh.stop, host, true)
  state.payload = nil
  if old then
    local back, why = xpcall(old.start, debug.traceback, host, true)
    if back then
      state.payload = old
      call_payload("on_reload_failed", err)
      return
    end
    report("reload", why)
  end
  state.retry_start_ms = now_ms() + START_RETRY_MS
end

local tick = guard("tick", function()
  if not state.payload and now_ms() >= state.retry_start_ms then start() end
  if state.reload_requested then
    state.reload_requested = false
    swap_payload()
  end
  call_payload("tick")
end)

local job_tick = guard("job_tick", function() call_payload("tick_jobs") end)

-- The payload says whether any read is pending; the fast timer runs only then.
function host.set_fast(active)
  local running = tool:has_timer(job_tick)
  if active and not running then
    tool:add_timer(job_tick, JOB_TICK_MS)
  elseif not active and running then
    tool:remove_timer(job_tick)
  end
end

-- main.lua runs once per Lua state, and a closure made here is never one already added: there's nothing to look for.
local hooks = {}
local function hook(observable, where, fn)
  local guarded = guard(where, fn)
  observable:add_notifier(guarded)
  hooks[#hooks + 1] = { observable = observable, fn = guarded }
end

hook(tool.tool_finished_loading_observable, "finished_loading", start)
hook(tool.tool_will_unload_observable, "will_unload", function()
  if tool:has_timer(tick) then tool:remove_timer(tick) end
  host.set_fast(false)
  stop()
  -- Renoise's own observers outlive this Lua state's tool object in no case we know of; taking ours off costs nothing.
  for _, entry in ipairs(hooks) do
    if entry.observable ~= tool.tool_will_unload_observable then
      pcall(function() entry.observable:remove_notifier(entry.fn) end)
    end
  end
end)
hook(tool.app_new_document_observable, "new_document", function()
  start()
  call_payload("on_new_document")
end)
hook(tool.app_release_document_observable, "release_document", function() call_payload("on_release_document") end)
hook(tool.app_will_save_document_observable, "will_save", function() call_payload("on_will_save") end)
hook(tool.app_saved_document_observable, "saved", function() call_payload("on_saved") end)
hook(tool.app_idle_observable, "idle", function() call_payload("on_idle") end)

if not tool:has_timer(tick) then tool:add_timer(tick, TICK_MS) end
guard("start", start)()
