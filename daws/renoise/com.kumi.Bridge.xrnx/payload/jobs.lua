-- Long reads as coroutines: each is resumed for a few milliseconds per timer tick, so a big read never holds Renoise's
-- GUI thread for long (its watchdog complains after about 10 s). A job re-looks-up what it reads on every slice
-- (it holds indices, never objects) and stops with "epoch-changed" if the song's refs changed meanwhile.

local M = {}
M.__index = M

M.BUDGET_MS = 8

-- ctx: now_ms(), unix_ms(), epoch() -> text, layout() -> number (the track layout within the epoch), shape() -> number
-- (moves when tracks, patterns or lines are inserted or removed), finish(job, ok, value), on_active(boolean)
function M.new(ctx)
  return setmetatable({ ctx = ctx, list = {}, last_run_ms = 0 }, M)
end

-- fn(job) does the work, calling job.slice() between steps (it answers true when the job waited there, so an object held
-- across the steps is looked up again), and returns the answer or raises {code, message}. `wrote` says
-- what a job wrote before it was made (a change written in its request's callback that then waits for Renoise), as
-- `job.wrote` does once a job writes.
function M:add(record, request, fn, wrote)
  local ctx = self.ctx
  local job = { record = record, request = request, epoch = ctx.epoch(), layout = ctx.layout(), shape = ctx.shape(), deadline = 0, wrote = wrote }
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

function M:pending()
  return #self.list
end

-- Runs a slice when none has run for `stale_ms`: the timers stop behind a modal dialog in Renoise, its sockets don't,
-- and a read waiting on the timers would wait for ever.
function M:run_if_stale(stale_ms, budget)
  if self.list[1] and self.ctx.now_ms() - self.last_run_ms >= stale_ms then self:run(budget) end
end

-- Drops the jobs of a connection that is gone. A job that has written to the song (`job.wrote`) runs to its end, unheard:
-- stopped there, it would leave half of what it does in the song (a track half put back, scratch copies never taken away).
function M:cancel(record)
  for i = #self.list, 1, -1 do
    local job = self.list[i]
    if job.record == record then
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
    if not job.unheard then self.ctx.finish(job, false, err) end
  end
  if any then self.ctx.on_active(false) end
end

-- Resumes each job for a slice, until the budget is used (the first always gets one); the job that went first goes last
-- next time.
function M:run(budget)
  local ctx = self.ctx
  budget = budget or M.BUDGET_MS
  local started = ctx.now_ms()
  self.last_run_ms = started
  local finished = {}
  for i, job in ipairs({ unpack(self.list) }) do
    if i == 1 or ctx.now_ms() - started < budget then
      job.deadline = ctx.now_ms() + budget
      local ok, value, ended
      if not job.started then
        -- A job is made when its request is handled and first runs on a later turn: what it names is checked before it reads.
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
      if ended then finished[#finished + 1] = { job = job, ok = ok, value = value } end
    end
  end
  for _, done in ipairs(finished) do
    for i, job in ipairs(self.list) do
      if job == done.job then table.remove(self.list, i) break end
    end
  end
  if #self.list > 1 and finished[1] == nil then table.insert(self.list, table.remove(self.list, 1)) end
  if #finished > 0 and #self.list == 0 then ctx.on_active(false) end
  for _, done in ipairs(finished) do
    if not done.job.unheard then ctx.finish(done.job, done.ok, done.value) end
  end
end

return M
