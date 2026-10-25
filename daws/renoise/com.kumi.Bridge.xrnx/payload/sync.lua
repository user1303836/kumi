-- Observers for one song: attached on a new document, detached on release.
--
-- A notifier does the least that keeps the tool true, at once, because a request can arrive between it and the next tick:
--   watch            marks a key dirty (selection, transport, project): the tick sends an event if the value moved.
--   watch_tracks     hands each insert, remove or swap of the track list to `hooks.tracks`, which moves the layout (or the
--                    epoch) and tells Kumi before any later answer.
--   watch_structural hands a change refs index into (instruments, patterns, the sequence, devices, samples, phrases) to
--                    `hooks.structural`, which moves the epoch at once.
-- Everything counts toward `changed`, the counter Kumi polls to know it should read again (line notifiers and plain
-- property observers only count). What changes the shape of the song moves `shape` too: a read in progress stops when it
-- moves.

local M = {}
M.__index = M

-- guard(where, fn) wraps a notifier so it never raises. state: a table that outlives the song (the counters).
-- hooks: { structural = function(reason, kind, op, owner, group), tracks = function(op) }.
function M.new(guard, state, hooks)
  return setmetatable({ entries = {}, dirty = {}, last = {}, guard = guard, state = state or {}, hooks = hooks or {}, dead = 0 }, M)
end

-- (`unkept`: state Renoise's undo doesn't keep, as mute, solo, the selection and playback: a change of it is no step, and leaves
-- Renoise's redo as it was. A function says so as the change comes: the tempo the player or a render moves, but not a hand)
local function count(self, light, unkept)
  if type(unkept) == "function" then unkept = unkept() end
  local state = self.state
  -- (M5 m5-renoise: Kumi's own solo, mute and loop writes for a render or an audition, put back after: not edits)
  if state.quiet and state.quiet > 0 then return end
  state.changed = (state.changed or 0) + 1
  -- (M6: the song's own content, not what's selected or where the transport is: a digest read across a selection move holds)
  if not light then state.content = (state.content or 0) + 1 end
  local undoing = self.hooks.undoing
  if undoing and undoing() then
    state.undone = true
  elseif not unkept then
    -- (an edit of the producer's or of Kumi's: what an undo took back can't be redone any more)
    state.edits = (state.edits or 0) + 1
  end
end

local function reshape(self)
  self.state.shape = (self.state.shape or 0) + 1
end

local function add(self, observable, fn)
  observable:add_notifier(fn)
  self.entries[#self.entries + 1] = { observable = observable, fn = fn, a = self.owner_a, b = self.owner_b }
end

-- A group is the watchers of one object (a track, a pattern, a device...). The operation that deletes the object marks its
-- group dead: Renoise keeps a deleted object readable from Lua, so nothing else tells. `prune` then drops the entries of
-- dead groups (an entry belongs to up to two: an alias watcher to its pattern and its track) without calling Renoise.
function M.group()
  return { dead = false }
end

-- Runs fn with the watchers it adds belonging to the groups a and b.
function M:owned(a, b, fn)
  local before_a, before_b = self.owner_a, self.owner_b
  self.owner_a, self.owner_b = a, b
  fn()
  self.owner_a, self.owner_b = before_a, before_b
end

-- Drops the watchers of deleted objects from the list (the objects are gone; nothing is asked of Renoise).
function M:prune()
  local kept = {}
  for _, entry in ipairs(self.entries) do
    if not ((entry.a and entry.a.dead) or (entry.b and entry.b.dead)) then kept[#kept + 1] = entry end
  end
  self.entries = kept
  self.dead = 0
end

function M:watch(observable, key, unkept)
  add(self, observable, self.guard("notifier " .. key, function()
    self.dirty[key] = true
    count(self, true, unkept)
  end))
end

-- Not even counted: what the producer's undo may move and that a producer's hand or an automation moves all the time (a
-- fader). It only asks whether the change that fired it was an undo or redo.
function M:watch_undo(observable)
  add(self, observable, self.guard("undo notifier", function()
    local undoing = self.hooks.undoing
    if undoing and undoing() then self.state.undone = true end
  end))
end

-- The same with a function of the caller's (one function for many observables: has_notifier tells it is there already).
function M:watch_undo_with(observable, fn)
  add(self, observable, fn)
end

-- Only counts: a name, a colour, a mute (`unkept`: no step of Renoise's undo, as count says).
function M:watch_changes(observable, unkept)
  add(self, observable, self.guard("notifier", function() count(self, false, unkept) end))
end

-- A list operation: Renoise names the places of a swap `index1` and `index2` (measured, 3.5.4), those of an insert or remove `index`.
local function place_of(op)
  if not op then return nil, nil end
  return op.index or op.index1, op.index2
end

-- The track list: its operations ({type = "insert" | "remove" | "swap", index, index2}).
function M:watch_tracks(observable)
  add(self, observable, self.guard("tracks notifier", function(op)
    count(self)
    reshape(self)
    if self.hooks.tracks then
      local index, index2 = place_of(op)
      self.hooks.tracks({ type = op and op.type, index = index, index2 = index2 })
    end
  end))
end

-- Refs shift or vanish when this changes. `kind` names the list; `op` is the list operation, when the observable is a list;
-- `owner` is the object that holds the list (a track, an instrument), when it has one.
function M:watch_structural(observable, reason, kind, owner, group)
  add(self, observable, self.guard("notifier " .. reason, function(op)
    count(self)
    reshape(self)
    if self.hooks.structural then
      local index, index2 = place_of(op)
      self.hooks.structural(reason, kind, op and { type = op.type, index = index, index2 = index2 } or nil, owner, group)
    end
  end))
end

-- A pattern's lines: any change counts (the handler only counts; clearing a pattern fires hundreds). A phrase's lines name no
-- phrase: `token` (the phrase's group) goes with them.
function M:watch_lines(pattern, token)
  local fn = self.guard("line notifier", function(pos)
    count(self)
    if self.hooks.lines then self.hooks.lines(pos, token) end
  end)
  pattern:add_line_notifier(fn)
  self.entries[#self.entries + 1] = { remove = function() pattern:remove_line_notifier(fn) end, a = self.owner_a, b = self.owner_b }
end

-- A pattern track made an alias or not: its lines change with no line notifier, so `hooks.alias` counts it for the pattern
-- (`token`).
function M:watch_alias(observable, token)
  add(self, observable, self.guard("alias notifier", function()
    count(self)
    if self.hooks.alias then self.hooks.alias(token) end
  end))
end

-- A change that moves things inside what a read steps through (a pattern's number of lines).
function M:watch_layout(observable)
  add(self, observable, self.guard("notifier", function()
    count(self)
    reshape(self)
  end))
end

function M:detach()
  for _, entry in ipairs(self.entries) do
    pcall(function()
      if entry.remove then
        entry.remove()
      elseif entry.observable:has_notifier(entry.fn) then
        entry.observable:remove_notifier(entry.fn)
      end
    end)
  end
  self.entries = {}
  self.dirty = {}
  self.last = {}
  self.dead = 0
  self.owner_a, self.owner_b = nil, nil
end

function M:attached()
  return #self.entries > 0
end

-- The marks since the last call. (With none, the empty table itself, which the caller only reads: most ticks mark nothing.)
function M:take_dirty()
  local dirty = self.dirty
  if next(dirty) == nil then return dirty end
  self.dirty = {}
  return dirty
end

local function same(a, b)
  if type(a) ~= "table" or type(b) ~= "table" then return a == b end
  for k, v in pairs(a) do
    if b[k] ~= v then return false end
  end
  for k in pairs(b) do
    if a[k] == nil then return false end
  end
  return true
end

-- Whether value (a flat table) differs from the one last reported under key; remembers it.
function M:changed(key, value)
  if same(self.last[key], value) then return false end
  self.last[key] = value
  return true
end

return M
