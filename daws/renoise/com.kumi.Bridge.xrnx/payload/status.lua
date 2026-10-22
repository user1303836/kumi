-- Who this bridge is and what song is open: the welcome's identity, status, and the song's Kumi id.

local import = ...
local json = import("json")
local sha256 = import("sha256")
local wire = import("wire")
local VERSION = import("version")

local byte, sub = string.byte, string.sub

local M = {}

M.METHODS = { "ping", "status", "renoise.reload", "read", "snapshot", "focus", "focus_body", "change_preview", "change_apply", "change_restore", "undo_step_begin", "undo_step_end", "transport_stop", "transport_play", "audition_note", "select", "project_save", "instrument_save", "show_status", "digest" }
M.EVENTS = { "project", "tracks", "selection", "transport", "undone_in_daw" }
-- (M5 m5-renoise: listening)
for _, method in ipairs({ "render_start", "listen_status", "listen_cancel", "audition", "audition_stop", "audio_file", "scratch" }) do
  M.METHODS[#M.METHODS + 1] = method
end
M.EVENTS[#M.EVENTS + 1] = "scratch_swept"
-- (M4 n-devices: what Kumi's host asks of devices, presets and macros)
for _, method in ipairs({ "preset_read", "instrument_export", "macro_read", "device_editor", "preset_save" }) do
  M.METHODS[#M.METHODS + 1] = method
end
-- (#401: scripts kept for a chore)
for _, method in ipairs({ "script_install", "script_remove" }) do
  M.METHODS[#M.METHODS + 1] = method
end
-- Set by the bridge: the listening flags (contract §2.8), and why Kumi's own render keeps Renoise busy (nil when none runs).
M.listening = nil
M.kumi_busy = nil

-- Kept across payload reloads (the bridge sets it to its own state).
M.cache = {}

-- Renoise's pid where Lua can learn it without a console window: /proc on Linux, a shell's parent on
-- macOS. nil on Windows.
function M.pid()
  local cache = M.cache
  if cache.pid_looked_up then return cache.pid end
  cache.pid_looked_up = true
  local platform = os.platform and os.platform() or ""
  pcall(function()
    if platform == "LINUX" then
      local f = io.open("/proc/self/stat", "rb")
      if f then
        cache.pid = tonumber((f:read("*l") or ""):match("^(%d+)"))
        f:close()
      end
    elseif platform == "MACINTOSH" then
      local p = io.popen("echo $PPID")
      if p then
        cache.pid = tonumber((p:read("*l") or ""):match("^%s*(%d+)"))
        p:close()
      end
    end
  end)
  return cache.pid
end

function M.demo()
  return renoise.RENOISE_VERSION:find("[Dd]emo") ~= nil
end

-- The V<x.y.z> preferences folder the tool is installed in.
function M.resource(bundle_path)
  return bundle_path:match("^(.*)[/\\]Scripts[/\\]Tools[/\\][^/\\]+[/\\]?$") or bundle_path
end

-- The resource folder as the wire carries it: a path is long on Windows (extended paths run to 32,767 characters), so it is cut
-- like a title. (The discovery file's name is made from this text, since a host hashes the resource it reads in the file.)
function M.resource_text(bundle_path)
  return M.cap(M.resource(bundle_path), M.MAX_PATH_CHARS)
end

-- Song ids: 32 hex in song.tool_data, which Renoise saves with the song under this tool's Id.
local function stored_id(song)
  local data = song.tool_data
  if type(data) ~= "string" or data == "" then return nil end
  local value = json.decode(data)
  if type(value) == "table" and type(value.songId) == "string" and #value.songId == 32 and value.songId:match("^%x+$") then
    return value.songId:lower()
  end
  return nil
end

-- Whether the song was opened without a file (a new song, or one made from a template): remembered when it is opened, not
-- worked out at a save, since what the file name is while Renoise announces the save is Renoise's to decide.
function M.note_document(memory, song)
  memory.born_unsaved = (song.file_name or "") == ""
end

-- The current song's id: the stored one; for a saved song without one, one made from its path (so
-- reopening it gives the same id until a save stores it); for an unsaved song, a new one kept until
-- it's saved.
function M.song_id(memory, song)
  local path = song.file_name or ""
  if memory.born_unsaved == nil then M.note_document(memory, song) end
  -- A song never saved has no id of its own yet: one stored in it was inherited from the template it was made from,
  -- and every song made from that template would share it. (After its first save the id it got is its own.)
  local id = path ~= "" and not memory.born_unsaved and stored_id(song) or nil
  if not id then
    if memory.song_id then
      id = memory.song_id
    elseif path ~= "" then
      id = sha256.hex("renoise-song:" .. path):sub(1, 32)
    else
      id = wire.nonce()
    end
  end
  memory.song_id = id
  return id
end

function M.write_song_id(memory, song)
  local id = M.song_id(memory, song)
  local data = json.decode(song.tool_data or "")
  if type(data) ~= "table" or json.is_array(data) then data = {} end
  if data.songId ~= id then
    data.songId = id
    song.tool_data = json.encode(data)
  end
end

local function file_name(path)
  return path:match("([^/\\]+)$") or path
end

-- A song's title is whatever the user typed, and hosts skip a discovery file over 64 KiB: the title in every project row
-- (the discovery file, the welcome, status, events) is cut to this many characters, and ends in "…" when it was.
M.MAX_TITLE_CHARS = 256
-- The same for the song's path and the resource folder, which are longer for good reason.
M.MAX_PATH_CHARS = 1024

function M.cap(text, max)
  max = max or M.MAX_TITLE_CHARS
  if #text <= max then return text end -- (no more bytes than characters allowed)
  local count, position = 0, 1
  while position <= #text do
    local lead = byte(text, position)
    local width = lead >= 0xF0 and 4 or lead >= 0xE0 and 3 or lead >= 0xC0 and 2 or 1
    if count == max then return sub(text, 1, position - 1) .. "\226\128\166" end
    count, position = count + 1, position + width
  end
  return text
end

-- The open song as the wire's project, or json.null.
function M.project(memory)
  local song = renoise.song()
  if not song then return json.null end
  local path = song.file_name or ""
  local title = M.cap(song.name or "")
  local project = { id = M.song_id(memory, song) }
  if path ~= "" then
    project.path = M.cap(path, M.MAX_PATH_CHARS)
    project.name = M.cap(file_name(path))
  elseif title ~= "" then
    project.name = title
  end
  if title ~= "" then project.title = title end
  return project
end

-- The welcome's fields besides session and maxFrameBytes.
function M.identity(memory, bundle_path)
  return {
    daw = "renoise",
    bridge = VERSION,
    dawVersion = renoise.RENOISE_VERSION,
    pid = M.pid() or 0,
    resource = M.resource_text(bundle_path),
    project = M.project(memory),
    demo = M.demo(),
    apiVersion = renoise.API_VERSION,
    heartbeatMs = wire.HEARTBEAT_MS,
    methods = json.array({ unpack(M.METHODS) }),
    events = json.array({ unpack(M.EVENTS) }),
  }
end

-- status: the identity plus busy.
function M.status(memory, bundle_path)
  local status = M.identity(memory, bundle_path)
  status.heartbeatMs, status.methods, status.events = nil, nil, nil
  local song = renoise.song()
  local rendering = song ~= nil and song.rendering
  status.busy = rendering and "rendering" or false
  if rendering then status.busyReason = "Renoise is rendering the song" end
  -- (M5 m5-renoise: Kumi's render job keeps Renoise busy between its renders too)
  local kumi = M.kumi_busy and M.kumi_busy() or nil
  if kumi then status.busy, status.busyReason = "rendering", kumi end
  if M.listening then status.listening = M.listening() end
  return status
end

return M
