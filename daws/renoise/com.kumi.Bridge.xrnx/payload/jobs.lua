-- Long reads as coroutines: each is resumed for a few milliseconds per timer tick (and a new read once in the callback of
-- the message that asked for it: M:run_first), so a big read never holds Renoise's GUI thread for long (its watchdog
-- complains after about 10 s). A job re-looks-up what it reads on every slice
-- (it holds indices, never objects) and stops with "epoch-changed" if the song's refs changed meanwhile.

local M = {}
M.__index = M

M.BUDGET_MS = 8
-- A slice while Renoise's transport is stopped. Renoise fires the tool's timers once a frame, about every 20 ms whatever
-- their interval (3.5.4, measured), so 8 ms a frame gave reads 40% of the time where the 10 ms job timer meant 80%; 14 ms
-- is 70%, and the frame holds (measured, the song stopped: p50 19.7 ms with no slice, 19.9 with 14-16 ms of Lua in
-- every frame).
M.STOPPED_BUDGET_MS = 14
-- A slice while it plays, one a frame: when a producer feels the GUI most.
M.PLAYING_BUDGET_MS = 11
-- A background job (a digest: ctx.background says which) waits behind the reads someone is waiting for, but never for
-- longer than this without a slice.
M.BACKGROUND_WAIT_MS = 250

-- ctx: now_ms(), unix_ms(), epoch() -> text, layout() -> number (the track layout within the epoch), shape() -> number
-- (moves when tracks, patterns or lines are inserted or removed), finish(job, ok, value), on_active(boolean), playing() ->
-- whether Renoise's transport plays (nil when it can't tell: a slice is then BUDGET_MS), background(request) -> whether
-- its job is one nobody waits on (it runs after the others)
function M.new(ctx)
  return setmetatable({ ctx = ctx, list = {}, last_run_ms = 0, made = 0 }, M)
end

-- fn(job) does the work, calling job.slice() between steps (it answers true when the job waited there, so an object held
-- across the steps is looked up again), and returns the answer or raises {code, message}. `wrote` says
-- what a job wrote before it was made (a change written in its request's callback that then waits for Renoise), as
-- `job.wrote` does once a job writes.
function M:add(record, request, fn, wrote)
  local ctx = self.ctx
  self.made = self.made + 1
  local job = { record = record, request = request, epoch = ctx.epoch(), layout = ctx.layout(), shape = ctx.shape(), deadline = 0, wrote = wrote,
    number = self.made, made_ms = ctx.now_ms(), background = ctx.background ~= nil and ctx.background(request) == true }
  -- Whether the song is still the one the request was made against; raises the error to answer with if it isn't.
  function job.check()
    if ctx.epoch() ~= job.epoch then
      error({ code = "epoch-changed", message = "the song changed while this was being read: read again" }, 0)
    end
    local deadline = request.deadlineMs
    if type(deadline) == "number" and ctx.unix_ms() >= deadline then
      error({ code = "deadline", message = "the deadline passed while this was being read" }, 0)
    end
    if ctx.shape() ~= job.shape then
      error({ code = "changed", message = "tracks, patterns or lines were inserted or removed while this was being read: read again" }, 0)
    end
  end
  -- True when the job waited here (and was checked as it resumed): what it held of the song is to be looked up again.
  function job.slice()
    -- (A job that yielded is checked as it resumes: the song may have changed while it waited.)
    if ctx.now_ms() >= job.deadline then
      coroutine.yield()
      job.check()
      return true
    end
    job.check()
    return false
  end
  -- The next step of work in a callback of its own (a restore's numbered steps), checked as it resumes. `gap_ms` is how
  -- long to wait first: Renoise has been seen to take callbacks 10 ms apart for one undo step.
  function job.step(gap_ms)
    local resume_at = ctx.now_ms() + (gap_ms or 0)
    repeat coroutine.yield() until ctx.now_ms() >= resume_at
    job.check()
  end
  -- The work changed the song's shape itself (it made a track, and a device on it moved the epoch): that is not a change to stop for.
  function job.rebase()
    job.shape, job.layout, job.epoch = ctx.shape(), ctx.layout(), ctx.epoch()
  end
  job.co = coroutine.create(fn)
  self.list[#self.list + 1] = job
  if #self.list == 1 then ctx.on_active(true) end
  return job
end

-- Pure background work has no connection or wire request to answer. Its completion can fill an immutable-data cache,
-- but must not use an old captured value to change the song or its journal. It retains the normal epoch/shape checks.
function M:add_internal(fn, finished)
  assert(type(finished) == "function", "internal work has its own completion")
  local job = self:add(nil, {}, fn)
  job.background, job.internal_finished = true, finished
  return job
end

local function finish(self, job, ok, value)
  if job.internal_finished then job.internal_finished(ok, value)
  else self.ctx.finish(job, ok, value) end
end

function M:pending()
  return #self.list
end

-- A slice's budget now: STOPPED_BUDGET_MS while Renoise's transport is stopped, PLAYING_BUDGET_MS while it plays,
-- BUDGET_MS when that can't be told.
function M:slice_budget()
  local playing = self.ctx.playing and self.ctx.playing()
  if playing == false then return M.STOPPED_BUDGET_MS end
  if playing == true then return M.PLAYING_BUDGET_MS end
  return M.BUDGET_MS
end

-- The timers ran, and left the jobs to the fast timer (bridge.lua's tick): for run_if_stale that is as good as a run.
function M:timers_ran()
  self.last_run_ms = self.ctx.now_ms()
end

-- Runs a slice when none has run for `stale_ms`: the timers stop behind a modal dialog in Renoise, its sockets don't,
-- and a read waiting on the timers would wait for ever. True when it ran.
function M:run_if_stale(stale_ms, budget)
  if self.list[1] and self.ctx.now_ms() - self.last_run_ms >= stale_ms then
    self:run(budget)
    return true
  end
  return false
end

-- Drops the jobs of a connection that is gone. A job that has written to the song (`job.wrote`) runs to its end, unheard:
-- stopped there, it would leave half of what it does in the song (a track half put back, scratch copies never taken away).
function M:cancel(record)
  for i = #self.list, 1, -1 do
    local job = self.list[i]
    if job.record == record and not job.internal_finished then
      if job.wrote then job.unheard = true else table.remove(self.list, i) end
    end
  end
  if #self.list == 0 then self.ctx.on_active(false) end
end

-- Ends every job with an error (the payload is going away). A job that wrote to the song (`job.wrote` says what it was doing)
-- can't take back what it wrote any more: it answers `uncertain`, so the change isn't asked again over what is half there.
function M:abort_all(code, message)
  local any = false
  -- (Ending one answers the requests that waited behind it, which may start more.)
  while self.list[1] do
    any = true
    local job = table.remove(self.list, 1)
    local err = { code = code, message = message }
    if job.wrote then
      err = { code = "uncertain", message = job.wrote .. " stopped when " .. message:gsub(": ask again$", "") .. ": part of it may be in the song, look for it" }
    end
    if not job.unheard then finish(self, job, false, err) end
  end
  if any then self.ctx.on_active(false) end
end

-- One slice of `job`: whether it ended, and how (ok, value).
local function resume(self, job, budget)
  local ctx = self.ctx
  job.deadline = ctx.now_ms() + budget
  job.resumed_ms = ctx.now_ms()
  local ok, value, ended
  if not job.started then
    -- A job is made when its request is handled and first runs after that request's handler has returned (on a timer, or
    -- in the same callback: M:run_first): what it names is checked before it reads.
    -- (One whose change is written already runs on whatever the producer did meanwhile: ended here, it would answer as if
    -- nothing was written.)
    job.started = true
    if not job.wrote then
      ok, value = pcall(job.check)
      ended = not ok
    end
  end
  if not ended then
    ok, value = coroutine.resume(job.co, job)
    ended = coroutine.status(job.co) == "dead"
    if ended and not ok and type(value) == "string" then value = debug.traceback(job.co, value) end
  end
  return ended, ok, value
end

-- Takes the ended jobs off the list, then answers them (which may start the requests that waited behind them).
local function settle(self, finished)
  for _, done in ipairs(finished) do
    for i, job in ipairs(self.list) do
      if job == done.job then table.remove(self.list, i) break end
    end
  end
  if #finished > 0 and #self.list == 0 then self.ctx.on_active(false) end
  for _, done in ipairs(finished) do
    if not done.job.unheard then finish(self, done.job, done.ok, done.value) end
  end
end

-- Resumes each job for a slice, until the budget is used (the first always gets one); the job that went first goes last
-- next time.
function M:run(budget)
  local ctx = self.ctx
  budget = budget or self:slice_budget()
  local started = ctx.now_ms()
  self.last_run_ms = started
  -- (every tick runs the jobs: with none, nothing to make)
  if self.list[1] == nil then return end
  local finished = {}
  -- The jobs someone waits on first (a FOCUS body, a read), then the background ones: but a background job that has
  -- waited BACKGROUND_WAIT_MS since its last slice goes first.
  local first, rest = {}, {}
  for _, job in ipairs(self.list) do
    local waited = started - (job.resumed_ms or job.made_ms)
    if not job.background or waited >= M.BACKGROUND_WAIT_MS then first[#first + 1] = job else rest[#rest + 1] = job end
  end
  for _, job in ipairs(rest) do first[#first + 1] = job end
  for i, job in ipairs(first) do
    if i == 1 or ctx.now_ms() - started < budget then
      local ended, ok, value = resume(self, job, budget)
      if ended then finished[#finished + 1] = { job = job, ok = ok, value = value } end
    end
  end
  if #self.list > 1 and finished[1] == nil then table.insert(self.list, table.remove(self.list, 1)) end
  settle(self, finished)
end

-- The first slice of the job made after the `after`th (M.made counts them), when `wanted(job)` says so: in the callback
-- that made it, not on the next timer. Renoise fires its timers once a frame, about every 20 ms whatever their interval,
-- so a read waited 8-25 ms for its first slice (measured, 3.5.4). One job and one slice: whatever its end lets start
-- (the requests that waited behind it) waits for the timer, and the other jobs keep their turns. True when it ran.
function M:run_first(after, wanted, budget)
  local job
  for _, j in ipairs(self.list) do
    if j.number > after and not j.started and (job == nil or j.number < job.number) then job = j end
  end
  if not job or not wanted(job) then return false end
  local ended, ok, value = resume(self, job, budget or M.BUDGET_MS)
  if ended then settle(self, { { job = job, ok = ok, value = value } }) end
  return true
end

return M
