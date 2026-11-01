-- The payload's entry: setup, the server, requests, song observers and events.

local import = ...
local json = import("json")
local wire = import("wire")
local server = import("server")
local sync = import("sync")
local jobs_module = import("jobs")
local reads = import("reads")
local focus = import("focus")
local rows = import("rows")
local changes = import("changes")
local refs = import("refs")
local places = import("places")
local status = import("status")
local config = import("config")
local discovery = import("discovery")
local log = import("log")
local VERSION = import("version")
-- (M5 m5-renoise: listening)
local listening = import("listening")
local audition = import("audition")
local reads_listening = import("reads_listening")
local scratch = import("scratch")
local actions = import("actions")
local digest = import("digest")
local chores = import("chores")

local M = {}

M.RETRY_MS = 1000
-- A read in progress that the timers haven't run for this long is run from the next message.
M.STALE_JOBS_MS = 100
-- With the timers stopped nothing but Renoise's dialog is in use, so such a slice may be longer.
M.STALE_SLICE_MS = 25
M.SETUP_RETRY_MS = 5000


-- Renoise replaces os.clock with a high-precision timer in seconds.
function M.now_ms()
  return os.clock() * 1000
end

local host, st, b, conns, observers, jobs
local instrument_scope -- (n-instruments; defined with the structural notifier)
local instrument_list_count, recount, retire -- (n-instruments)

-- The epoch: refs are valid within one. It moves, at once and inside the notifier that saw it, when a song is switched and
-- when something happens to the song that the tool can't map into the refs it handed out (instruments, patterns or the
-- sequence restructured, devices, samples or phrases inserted or removed, tracks swapped). Within an epoch the track
-- layout counts the tracks inserted or removed: those move refs, and the tool carries them to where they went (refs.carry)
-- for a request that says which layout its refs were made in.
local function epoch()
  return wire.hex(b.epoch_n)
end

-- The epoch moves when refs stop meaning what they meant. `tracks_kept`: the journal's places (tracks) still hold. `broken`: the
-- lists whose numbers moved (a set of CATEGORIES); a ref of an earlier epoch is still the same thing when none of the categories
-- its kind is numbered by moved since (b.snap keeps the count of each, per epoch).
local CATEGORIES = { "document", "tracks", "devices", "samples", "phrases", "patterns", "instruments", "sequence" }
local DEPENDS = {
  song = { "document" }, track = { "document", "tracks" }, device = { "document", "tracks", "devices" },
  param = { "document", "tracks", "devices" }, ptrack = { "document", "tracks", "patterns" },
  slot = { "document", "tracks", "sequence" }, seq = { "document", "sequence" }, pattern = { "document", "patterns" },
  instrument = { "document", "instruments" }, sample = { "document", "instruments", "samples" },
  phrase = { "document", "instruments", "phrases" },
}
-- The lists a notifier's change moves the numbers of.
local BREAKS = {
  devices = { "devices" }, samples = { "samples" }, phrases = { "phrases" }, patterns = { "patterns" },
  instruments = { "instruments" }, sequence = { "sequence" },
}
local EVERYTHING = { "document", "tracks", "devices", "samples", "phrases", "patterns", "instruments", "sequence" }
-- n-instruments: an instrument's modulation sets and their devices, its sample FX chains and their devices (and their
-- parameters), and a plug-in instrument's parameters are lists of their own.
for _, category in ipairs({ "modsets", "moddevices", "chains", "chaindevices", "plugins" }) do
  CATEGORIES[#CATEGORIES + 1] = category
  EVERYTHING[#EVERYTHING + 1] = category
  BREAKS[category] = { category }
end
DEPENDS.modset = { "document", "instruments", "modsets" }
DEPENDS.moddevice = { "document", "instruments", "modsets", "moddevices" }
DEPENDS.chain = { "document", "instruments", "chains" }
DEPENDS.chaindevice = { "document", "instruments", "chains", "chaindevices" }
DEPENDS.chainparam = { "document", "instruments", "chains", "chaindevices" }
DEPENDS.pluginparam = { "document", "instruments", "plugins" }

local function bump_epoch(reason, tracks_kept, broken, keep_shifts)
  b.epoch_n = b.epoch_n + 1
  broken = broken or EVERYTHING
  -- The layout goes on counting across epochs (no event says when an epoch moves for a device or a pattern, so a layout
  -- that started over would look older than the one Kumi holds). The shifts start over when track numbers can't be followed.
  local cleared = false
  for _, category in ipairs(broken) do
    b.cat[category] = (b.cat[category] or 0) + 1
    if (category == "tracks" or category == "document") and not keep_shifts then cleared = true end
  end
  if cleared then
    -- (the layout moves past what the shifts cover: a track numbered in an earlier layout has no place that can be told now,
    -- where the layout kept would have said it is where it was. Only a layout something was told of needs it.)
    if b.layout_told or #b.shifts > 0 then b.layout = b.layout + 1 end
    b.shifts, b.layout_told = {}, false
  end
  -- Another song: the pool's and the instruments' places are its own.
  for _, category in ipairs(broken) do
    if category == "document" and b.plog then
      places.gap(b.plog)
      places.gap(b.ilog)
    end
  end
  local snapshot = {}
  for _, category in ipairs(CATEGORIES) do snapshot[category] = b.cat[category] or 0 end
  b.snap[b.epoch_n] = snapshot
  b.snap[b.epoch_n - 512] = nil
  -- (the journal's places are tracks: a change to devices, patterns or instruments leaves them where they were)
  if b.journal and not tracks_kept then changes.journal_reset(b.journal) end
  log.write("info", "epoch " .. epoch() .. ": " .. reason)
end

-- Whether a ref of `kind` made in epoch `from` (hex) names the same thing in epoch `to` (default: now).
local function survives(from, kind, to)
  local a, c = wire.from_hex(from), to and wire.from_hex(to) or b.epoch_n
  local before, after = b.snap[a], b.snap[c]
  local depends = DEPENDS[kind]
  if not before or not after or not depends then return false end
  for _, category in ipairs(depends) do
    if before[category] ~= after[category] then return false end
  end
  return true
end

-- The layout, as it is handed out (it then needs moving when the shifts are cleared).
local function layout_now()
  b.layout_told = true
  return b.layout
end

-- How often the journal reads again what no notifier tells (a container's contents), how many of those at most each time,
-- and how many devices of them a tick reads (a round goes on over the next ticks: #361 N-S16).
M.POLL_MS, M.POLL_MOST, M.POLL_EACH = 1000, 8, 2

-- How many shifts of the track list the tool keeps to carry refs through.
-- (a restore that puts a track back into a big group makes two swaps per member itself: 1024 covers groups of 500)
M.SHIFT_HISTORY = 1024
-- Watchers left behind by deleted objects, past which the song is watched anew.
M.MAX_DEAD = 256

function M.log_error(where, err)
  log.write("error", where .. ": " .. tostring(err))
end

-- How deep in the tool's notifiers the code running now is: a run-lua chunk's time limit (a count hook) stops the chunk only
-- outside them, so every notifier runs whole (setting the hook aside instead would restart its count at each notifier).
local guarded = 0
-- The guards themselves, by identity: a stop must not land in one either, before its count goes up or after it goes down
-- (raised there, it would reach Renoise's notifier dispatch, which disables the notifier behind an error dialog).
local guards = setmetatable({}, { __mode = "k" })

-- A notifier, run whole and never raising. One that failed may have left the song half watched: it is watched anew.
local function guard(where, fn)
  local function guarded_fn(...)
    guarded = guarded + 1
    local ok, result = xpcall(fn, debug.traceback, ...)
    guarded = guarded - 1
    if not ok then
      M.log_error(where, result)
      if b then b.reattach = true end
      return nil
    end
    return result
  end
  guards[guarded_fn] = true
  return guarded_fn
end

local function show_status(message)
  pcall(function() renoise.app():show_status(message) end)
end

-- Unix milliseconds: os.time's second, refined by the timer since that second was seen to start.
local function unix_ms()
  local now, seconds = M.now_ms(), os.time()
  if seconds ~= b.anchor_seconds then
    b.anchor_seconds, b.anchor_ms = seconds, now
  end
  return seconds * 1000 + math.floor(math.min(now - b.anchor_ms, 999))
end

local function identity()
  return status.identity(b.memory, host.bundle_path)
end

local function write_discovery()
  if not (b.cfg and b.port) then return end
  local pid = status.pid() or 0
  -- (the file's name is made from the resource as written in it, cut or not: that is what a host hashes)
  local resource = status.resource_text(host.bundle_path)
  local path = b.cfg.discovery_dir .. "/" .. discovery.file_name(pid, resource)
  local ok, err = discovery.write(path, {
    schema = discovery.SCHEMA,
    daw = "renoise",
    pid = pid,
    host = "127.0.0.1",
    port = b.port,
    protocol = wire.PROTOCOL,
    version = VERSION,
    dawVersion = renoise.RENOISE_VERSION,
    resource = resource,
    project = status.project(b.memory),
    startedMs = b.started_ms,
  })
  if ok then
    if b.discovery_path and b.discovery_path ~= path then discovery.remove(b.discovery_path) end
    b.discovery_path = path
  else
    log.write("error", "could not write the discovery file: " .. tostring(err))
  end
end

-- Each Renoise has a log of its own (named by its pid, or by when it started where the pid is unknown).
local function log_file(dir)
  local id = status.pid() and ("pid-" .. status.pid()) or ("started-" .. math.floor(b.started_ms / 1000))
  return dir .. "/renoise-bridge-" .. id .. ".log"
end

local function configure_log(dir)
  local path = log_file(dir)
  log.configure(path)
  log.prune(dir, path)
end

local function configure()
  local cfg, err = config.load(host.bundle_path)
  if not cfg then
    if b.setup_error ~= err then
      b.setup_error = err
      log.write("error", "not set up: " .. err)
      show_status("Kumi: the bridge isn't set up yet; run kumi setup")
    end
    b.retry_ms = M.now_ms() + M.SETUP_RETRY_MS
    return false
  end
  b.cfg, b.setup_error = cfg, nil
  if cfg.log_dir then configure_log(cfg.log_dir) end
  -- (#401: the scripts kept for a chore, in the Tools menu again, once a start)
  if not b.chores_registered then
    b.chores_registered = true
    local ok, err = pcall(chores.register_all, cfg)
    if not ok then log.write("error", "registering the kept scripts: " .. tostring(err)) end
  end
  return true
end

-- (asked every tick: the pcall takes a function of the module's, not a closure made for it)
local function server_is_running() return st.server ~= nil and st.server.is_running end
local function server_running()
  local ok, running = pcall(server_is_running)
  return ok and running
end

local function listen()
  local last_err
  for _, port in ipairs(config.candidate_ports(b.cfg.port)) do
    local socket, err = host.listen(port)
    if socket then
      b.port, b.listen_error = port, nil
      log.write("info", "listening on 127.0.0.1:" .. port)
      write_discovery()
      return true
    end
    last_err = err
  end
  if b.listen_error ~= last_err then
    b.listen_error = last_err
    log.write("error", "could not listen: " .. tostring(last_err))
    show_status("Kumi: the bridge could not open a port")
  end
  b.retry_ms = M.now_ms() + M.RETRY_MS
  return false
end

local function selection(song)
  return {
    track = song.selected_track_index,
    instrument = song.selected_instrument_index,
    pattern = song.selected_pattern_index,
    sequence = song.selected_sequence_index,
  }
end

local function track_total(song)
  return song.sequencer_track_count + song.send_track_count + 1
end

local function transport(song)
  local t = song.transport
  return { playing = t.playing, bpm = t.bpm, lpb = t.lpb }
end

local TRANSPORT_OBSERVABLES = {
  "playing_observable", "bpm_observable", "lpb_observable", "tpl_observable", "edit_mode_observable",
  "follow_player_observable", "loop_pattern_observable",
}
-- (what Renoise's undo doesn't keep, measured in 3.5.4: no step, and Renoise's redo stays as it was)
local UNKEPT = {}
for name in ([[playing_observable edit_mode_observable follow_player_observable loop_pattern_observable metronome_enabled_observable
  metronome_precount_enabled_observable metronome_precount_bars_observable edit_step_observable octave_observable
  record_quantize_enabled_observable record_quantize_lines_observable wrapped_pattern_edit_observable
  single_track_edit_mode_observable keyboard_velocity_enabled_observable keyboard_velocity_observable
  record_parameter_mode_observable mute_state_observable solo_state_observable active_tab_observable]]):gmatch("%S+") do UNKEPT[name] = true end
-- (the tempo, LPB and TPL also move as the player or a render meets a ZT, ZL or ZK command: no step either)
local function playing_or_rendering()
  local ok, moving = pcall(function()
    local song = renoise.song()
    return song ~= nil and (song.transport.playing or song.rendering)
  end)
  return ok and moving == true
end
for _, name in ipairs({ "bpm_observable", "lpb_observable", "tpl_observable" }) do UNKEPT[name] = playing_or_rendering end
-- M4 (n-song): the song's settings Kumi changes (the groove, the metronome, the editing settings, the headroom), counted, and
-- asked on the producer's undo.
local SETTING_OBSERVABLES = {
  "groove_enabled_observable", "groove_assignment_observable", "metronome_enabled_observable",
  "metronome_beats_per_bar_observable", "metronome_lines_per_beat_observable", "metronome_precount_enabled_observable",
  "metronome_precount_bars_observable", "metronome_volume_observable", "edit_step_observable", "octave_observable",
  "record_quantize_enabled_observable", "record_quantize_lines_observable", "wrapped_pattern_edit_observable",
  "single_track_edit_mode_observable", "keyboard_velocity_enabled_observable", "keyboard_velocity_observable",
  "record_parameter_mode_observable", "track_headroom_observable",
}
local SELECTION_OBSERVABLES = {
  "selected_track_index_observable", "selected_instrument_index_observable", "selected_pattern_index_observable",
  "selected_sequence_index_observable", "selected_sample_observable", "selected_phrase_observable",
  "selected_track_device_observable",
}

-- Each object is watched in a group of its own (see sync.lua), kept in a list in the order of the song's own, so the operation
-- that deletes or swaps an object (which names its index) can say whose watchers are dead or moved.
local group = sync.group

-- What each kind of object is watched for. A new object gets its watchers where it is made (inside the notifier that saw
-- it), so there is no turn on which it goes unseen.
local function watch_device(device, token, track_token)
  observers:owned(token, track_token, function()
    observers:watch_changes(device.is_active_observable)
    observers:watch_changes(device.display_name_observable)
    -- (M4 n-devices: device-set's maximized, for the producer's undo of it)
    observers:watch_changes(device.is_maximized_observable)
  end)
end

local function watch_track(song, i)
  local track = song:track(i)
  local token = b.groups.tracks[i]
  token.devices = {}
  -- (the track's devices' log: a new one for every watch, so a place from before the track was watched anew is unknown)
  b.ids = (b.ids or 0) + 1
  token.dlog = places.new(b.run .. "d" .. b.ids)
  observers:owned(token, nil, function()
    observers:watch_changes(track.name_observable)
    observers:watch_changes(track.color_observable)
    observers:watch_changes(track.mute_state_observable, UNKEPT.mute_state_observable)
    observers:watch_changes(track.solo_state_observable, UNKEPT.solo_state_observable)
    observers:watch_changes(track.visible_note_columns_observable)
    observers:watch_changes(track.visible_effect_columns_observable)
    for c = 1, track.max_note_columns do
      observers:watch_changes(track:column_is_muted_observable(c))
      observers:watch_changes(track:column_name_observable(c))
    end
    -- M4 (n-song): what the track kinds change (a group's group_collapsed has no observable)
    for _, name in ipairs({ "collapsed_observable", "color_blend_observable", "output_routing_observable", "output_delay_observable" }) do
      observers:watch_changes(track[name])
    end
    if track.max_note_columns > 0 then
      for _, name in ipairs({ "volume_column_visible_observable", "panning_column_visible_observable", "delay_column_visible_observable",
        "sample_effects_column_visible_observable" }) do
        observers:watch_changes(track[name])
      end
    end
    observers:watch_structural(track.devices_observable, "a track's devices changed", "devices", track, token)
  end)
  for d = 1, #track.devices do
    token.devices[d] = group()
    watch_device(track:device(d), token.devices[d], token)
  end
end

-- What a pattern's track shows is another pattern's when it is an alias. (Its watcher belongs to the pattern and the track.)
local function watch_pattern_track(pattern, t, pattern_token, track_token)
  observers:owned(pattern_token, track_token, function()
    observers:watch_alias(pattern:track(t).alias_pattern_index_observable, pattern_token)
  end)
end

local function watch_pattern(song, p, tracks)
  local pattern = song:pattern(p)
  local token = b.groups.patterns[p]
  -- (the count of its changed lines, under an id of this watch: a mark from before the pattern was watched anew is unknown)
  b.ids = (b.ids or 0) + 1
  token.pid, token.touches = b.run .. "p" .. b.ids, 0
  observers:owned(token, nil, function()
    observers:watch_lines(pattern)
    observers:watch_changes(pattern.name_observable)
    observers:watch_layout(pattern.number_of_lines_observable)
  end)
  for t = 1, tracks do watch_pattern_track(pattern, t, token, b.groups.tracks[t]) end
end

local function watch_sample(sample, token, instrument_token)
  observers:owned(token, instrument_token, function()
    observers:watch_changes(sample.name_observable)
    observers:watch_changes(sample.sample_buffer_observable) -- (n-instruments: a sample loaded)
    local mapping = sample.sample_mapping
    if not mapping then return end
    observers:watch_changes(mapping.note_range_observable)
    observers:watch_changes(mapping.velocity_range_observable)
    observers:watch_changes(mapping.base_note_observable)
    observers:watch_changes(mapping.layer_observable)
    observers:watch_changes(mapping.map_key_to_pitch_observable)
    observers:watch_changes(mapping.map_velocity_to_volume_observable)
  end)
end

local function watch_phrase(phrase, token, instrument_token)
  -- (n-instruments: the count of its changed lines, under an id of this watch, as a pattern's: a phrase change's undo goes
  -- through only if none came since its step)
  b.ids = (b.ids or 0) + 1
  token.pid, token.touches = b.run .. "ph" .. b.ids, 0
  observers:owned(token, instrument_token, function()
    observers:watch_lines(phrase, token)
    observers:watch_changes(phrase.name_observable)
    observers:watch_layout(phrase.number_of_lines_observable)
    observers:watch_changes(phrase.lpb_observable)
    observers:watch_changes(phrase.visible_note_columns_observable)
    observers:watch_changes(phrase.visible_effect_columns_observable)
    for _, name in ipairs({ "looping", "key_tracking", "base_note", "playback_mode" }) do
      observers:watch_changes(phrase[name .. "_observable"])
    end
  end)
end

-- n-instruments: a modulation set's devices and a sample FX chain's devices are lists of their own (with their logs: a new one
-- for every watch, as a track's devices have).
local function watch_modset(set, token, instrument_token)
  b.ids = (b.ids or 0) + 1
  token.log, token.parent, token.devices = places.new(b.run .. "m" .. b.ids), instrument_token, {}
  for d = 1, #set.devices do token.devices[d] = group() end
  observers:owned(token, instrument_token, function()
    observers:watch_changes(set.name_observable)
    observers:watch_structural(set.devices_observable, "a modulation set's devices changed", "moddevices", set, token)
  end)
end

local function watch_chain(chain, token, instrument_token)
  b.ids = (b.ids or 0) + 1
  token.log, token.parent, token.devices = places.new(b.run .. "c" .. b.ids), instrument_token, {}
  for d = 1, #chain.devices do token.devices[d] = group() end
  observers:owned(token, instrument_token, function()
    observers:watch_changes(chain.name_observable)
    observers:watch_changes(chain.output_routing_observable)
    observers:watch_structural(chain.devices_observable, "a sample FX chain's devices changed", "chaindevices", chain, token)
  end)
end

local function watch_instrument(song, i)
  local instrument = song:instrument(i)
  local token = b.groups.instruments[i]
  token.samples, token.phrases = {}, {}
  -- n-instruments: the logs of its lists, new for every watch (a place from before the instrument was watched anew is unknown)
  b.ids = (b.ids or 0) + 1
  token.slog, token.phlog = places.new(b.run .. "s" .. b.ids), places.new(b.run .. "ph" .. b.ids)
  token.mlog, token.clog = places.new(b.run .. "ms" .. b.ids), places.new(b.run .. "cs" .. b.ids)
  token.modsets, token.chains = {}, {}
  observers:owned(token, nil, function()
    observers:watch_changes(instrument.name_observable)
    for k = 1, #instrument.macros do
      local macro = instrument:macro(k)
      observers:watch_changes(macro.value_observable)
      observers:watch_changes(macro.name_observable)
    end
    observers:watch_structural(instrument.samples_observable, "an instrument's samples changed", "samples", instrument, token)
    observers:watch_structural(instrument.phrases_observable, "an instrument's phrases changed", "phrases", instrument, token)
    -- n-instruments
    for _, name in ipairs({ "volume", "transpose", "sample_mapping_overlap_mode", "phrase_playback_mode", "phrase_program", "active_tab" }) do
      observers:watch_changes(instrument[name .. "_observable"], UNKEPT[name .. "_observable"])
    end
    observers:watch_changes(instrument.phrase_mappings_observable)
    observers:watch_structural(instrument.sample_modulation_sets_observable, "an instrument's modulation sets changed", "modsets", instrument, token)
    observers:watch_structural(instrument.sample_device_chains_observable, "an instrument's sample FX chains changed", "chains", instrument, token)
    observers:watch_structural(instrument.plugin_properties.plugin_device_observable, "an instrument's plug-in changed", "plugins", instrument, token)
  end)
  for s = 1, #instrument.samples do
    token.samples[s] = group()
    watch_sample(instrument:sample(s), token.samples[s], token)
  end
  for ph = 1, #instrument.phrases do
    token.phrases[ph] = group()
    watch_phrase(instrument:phrase(ph), token.phrases[ph], token)
  end
  for m = 1, #instrument.sample_modulation_sets do
    token.modsets[m] = group()
    watch_modset(instrument:sample_modulation_set(m), token.modsets[m], token)
  end
  for c = 1, #instrument.sample_device_chains do
    token.chains[c] = group()
    watch_chain(instrument:sample_device_chain(c), token.chains[c], token)
  end
end

-- Everything the focus pane and the reads depend on is watched: events for what Kumi is told as it happens, a counter
-- for the rest (`changed`), and structural marks for what moves the refs.
local function attach(song)
  observers:detach()
  if not song then return end
  local tracks = track_total(song)
  b.groups = { tracks = {}, patterns = {}, instruments = {} }
  for t = 1, tracks do b.groups.tracks[t] = group() end
  for p = 1, #song.patterns do b.groups.patterns[p] = group() end
  for i = 1, #song.instruments do b.groups.instruments[i] = group() end
  for _, name in ipairs(SELECTION_OBSERVABLES) do observers:watch(song[name], "selection", true) end
  for _, name in ipairs(TRANSPORT_OBSERVABLES) do observers:watch(song.transport[name], "transport", UNKEPT[name]) end
  observers:watch(song.name_observable, "project")
  for _, name in ipairs(SETTING_OBSERVABLES) do observers:watch_changes(song.transport[name], UNKEPT[name]) end
  observers:watch_changes(song.artist_observable)
  observers:watch_changes(song.comments_observable)
  observers:watch_tracks(song.tracks_observable)
  observers:watch_structural(song.instruments_observable, "the instruments changed", "instruments")
  observers:watch_structural(song.patterns_observable, "the patterns changed", "patterns")
  local sequencer = song.sequencer
  observers:watch_structural(sequencer.pattern_sequence_observable, "the sequence changed", "sequence")
  observers:watch_changes(sequencer.pattern_assignments_observable)
  observers:watch_changes(sequencer.pattern_slot_mutes_observable)
  observers:watch_changes(sequencer:sequence_sections_changed_observable())
  for p = 1, #song.patterns do watch_pattern(song, p, tracks) end
  for t = 1, tracks do watch_track(song, t) end
  for i = 1, #song.instruments do watch_instrument(song, i) end
  b.track_count = tracks
  b.reattach = false
  observers:changed("selection", selection(song))
  observers:changed("transport", transport(song))
  observers:changed("project", status.project(b.memory))
end

local function send_project()
  local project = status.project(b.memory)
  observers:changed("project", project)
  conns:event("project", project)
  write_discovery()
end

-- The group of a deleted owner is marked dead (its watchers are dropped when enough have piled up); a swap moves the groups
-- with their objects. Returns false when the list doesn't match the operation, which asks for the song to be watched anew.
local function forget(list, index)
  if type(index) ~= "number" or not list[index] then return false end
  table.remove(list, index).dead = true
  observers.dead = observers.dead + 1
  return true
end

local function exchange(list, a, b_)
  if type(a) ~= "number" or type(b_) ~= "number" or not list[a] or not list[b_] then return false end
  list[a], list[b_] = list[b_], list[a]
  return true
end

-- A group for an object made at `index`. A list that doesn't reach that far (it should) is filled up so the object is watched
-- at once; false says the list was out of step, which asks for the song to be watched anew on the next tick.
local function remember(list, index)
  if type(index) ~= "number" or index < 1 then return false end
  local exact = index <= #list + 1
  while #list < index - 1 do list[#list + 1] = group() end
  table.insert(list, index, group())
  return exact
end

-- n-instruments: the logs of an instrument's lists (on its group), and of a set's or chain's devices (on theirs).
local INSTRUMENT_LOGS = { samples = "slog", phrases = "phlog", modsets = "mlog", chains = "clog", moddevices = "log", chaindevices = "log" }
-- The key of the journal's places each list moves.
local INSTRUMENT_JOURNAL = {
  instruments = "instrument", samples = "sample", phrases = "phrase", modsets = "modset", chains = "chain",
  moddevices = "moddevice", chaindevices = "chaindevice",
}

local function position(list, token)
  for k, candidate in ipairs(list or {}) do
    if candidate == token then return k end
  end
  return nil
end

-- Which entries a list's operation moves: those of the instrument (and the set or chain) whose list it is; nil when the owner
-- isn't found (the lists are out of step: the song is watched anew).
instrument_scope = function(kind, token)
  if kind == "instruments" then return {} end
  if not token or not b.groups then return nil end
  if kind == "moddevices" or kind == "chaindevices" then
    local parent = token.parent
    local i = position(b.groups.instruments, parent)
    local at = parent and position(kind == "moddevices" and parent.modsets or parent.chains, token)
    if not i or not at then return nil end
    return kind == "moddevices" and { instrument = i, modset = at } or { instrument = i, chain = at }
  end
  local i = position(b.groups.instruments, token)
  return i and { instrument = i } or nil
end

-- n-instruments: the logs of an object that went (an instrument's lists, its sets' and chains' devices, its phrases' counts of
-- changed lines; a set's or chain's devices; a phrase's count), kept a while by their ids: when Kumi puts the object back, the
-- one made in its place takes them on (`adopt`), so the places kept in them before the delete follow it.
local RETIRED = 512
retire = function(token)
  if type(token) ~= "table" then return end
  local kept = b.retired or { order = {}, logs = {} }
  b.retired = kept
  local function keep(log_)
    if type(log_) ~= "table" or not log_.id or kept.logs[log_.id] then return end
    kept.logs[log_.id] = log_
    kept.order[#kept.order + 1] = log_.id
    if #kept.order > RETIRED then kept.logs[table.remove(kept.order, 1)] = nil end
  end
  for _, key in ipairs({ "slog", "phlog", "mlog", "clog", "log" }) do keep(token[key]) end
  for _, owned in ipairs(token.modsets or {}) do keep(owned.log) end
  for _, owned in ipairs(token.chains or {}) do keep(owned.log) end
end

-- n-instruments: how many objects Renoise has in the list a structural notifier is about (nil when it can't be read).
instrument_list_count = function(kind, owner)
  local ok, count = pcall(function()
    if kind == "samples" then return #owner.samples end
    if kind == "phrases" then return #owner.phrases end
    if kind == "modsets" then return #owner.sample_modulation_sets end
    if kind == "chains" then return #owner.sample_device_chains end
    return #owner.devices
  end)
  return ok and count or nil
end

-- n-instruments: one list watched anew from what Renoise has: its groups (and what is in them) new, its log started again.
recount = function(kind, token, owner, list, count)
  places.gap(token[INSTRUMENT_LOGS[kind]])
  for _, old in ipairs(list) do
    if not old.dead then
      old.dead = true
      observers.dead = observers.dead + 1
    end
  end
  for k = #list, 1, -1 do list[k] = nil end
  for k = 1, count or 0 do
    list[k] = group()
    if kind == "samples" then
      watch_sample(owner:sample(k), list[k], token)
    elseif kind == "phrases" then
      watch_phrase(owner:phrase(k), list[k], token)
    elseif kind == "modsets" then
      watch_modset(owner:sample_modulation_set(k), list[k], token)
    elseif kind == "chains" then
      watch_chain(owner:sample_device_chain(k), list[k], token)
    end
  end
  log.write("info", "the " .. kind .. " list was out of step: watched anew on its own")
end

-- What the journal links Renoise's redo of a change that made something back to its undo with (changes_base `structural`): a
-- list's place log, or the track shifts.
local function linker(log)
  if not log then return nil end
  return {
    mark = function() return places.mark(log) end,
    back = function(mark, at) places.back(log, mark, at) end,
  }
end
local track_linker = {
  mark = function() return b.layout end,
  back = function(mark, at) refs.back(b.shifts, mark, at) end,
}

-- Renoise's undo or redo of a load that replaced an instrument in place (an .xrni load: Kumi's, the producer's) tells an
-- insert next to it, then the other one's remove: insert@i, remove@i+1 undone; insert@i+1, remove@i redone (lab 2026-10-20).
-- For what keeps an instrument's place (the places log, the journal) the slot keeps its instrument, as in Kumi's own load:
-- the pair's remove is taken as the inserted one's. (The list of watched instruments takes the operations as told.)
local function in_place(op)
  local held = b.held_insert
  b.held_insert = nil
  local song = renoise.song()
  if not (op and type(op.index) == "number" and song and song:is_undo_redoing()) then return op end
  if op.type == "insert" then
    b.held_insert = op.index
  elseif op.type == "remove" and held and math.abs(op.index - held) == 1 then
    return { type = "remove", index = held }
  end
  return op
end

-- A change to the lists refs index into: the epoch moves (so every answer after it is in the new epoch, and a ref from
-- before is refused), and what was made gets its watchers. What was deleted loses its group, and a swap moves the groups
-- with their objects (Renoise keeps an object's notifiers through a swap: measured). Only an operation the lists can't follow
-- asks for the whole song to be watched anew.
local function on_structural(reason, kind, op, owner, token)
  -- (n-instruments: a modulation set's devices notifier names each place one higher than the list has it, and tells of a
  -- device at 1 that the list doesn't show as a set is copied: lab 2026-10-19. That one is no change to the list.)
  if kind == "moddevices" and op then
    op = { type = op.type, index = type(op.index) == "number" and op.index - 1 or op.index, index2 = type(op.index2) == "number" and op.index2 - 1 or op.index2 }
    if type(op.index) == "number" and op.index < 1 and op.type ~= "swap" then
      bump_epoch(reason, true, BREAKS[kind])
      return
    end
  end
  bump_epoch(reason, true, BREAKS[kind])
  local placed = op
  -- (what moved, for the places a change keeps: a track's devices, the pool, the instruments; anything not told is a gap)
  if kind == "devices" and token and token.dlog then
    places.note(token.dlog, op)
  elseif kind == "patterns" then
    places.note(b.plog, op)
  elseif kind == "instruments" then
    -- (M4 n-devices: Kumi's own load of an instrument file into slot i is told as an insert after it and a remove of it, but
    -- the slot keeps its instrument (pattern notes keep playing it, measured in 3.5.4): no delete in the places log)
    local r, own = b.replacing, false
    if r and r.deadline and os.clock() > r.deadline then b.replacing, r = nil, nil end
    if r and op then
      if r.insert and op.type == "insert" and op.index == r.i + 1 then
        r.insert, own = false, true
      elseif r.remove and not r.insert and op.type == "remove" and op.index == r.i then
        r.remove, own = false, true
      end
      if not r.insert and not r.remove then b.replacing = nil end
    end
    placed = in_place(op)
    if not own then places.note(b.ilog, placed) end
  elseif token and INSTRUMENT_LOGS[kind] and token[INSTRUMENT_LOGS[kind]] then
    places.note(token[INSTRUMENT_LOGS[kind]], op) -- (n-instruments)
  end
  local song = renoise.song()
  if not song then return end
  local lists = { instruments = b.groups.instruments, patterns = b.groups.patterns }
  if token then
    lists.devices, lists.samples, lists.phrases = token.devices, token.samples, token.phrases
    lists.modsets, lists.chains, lists.moddevices, lists.chaindevices = token.modsets, token.chains, token.devices, token.devices
  end
  -- (n-instruments: the journal's places of instruments and what is in them move with the list's operations)
  if op and INSTRUMENT_JOURNAL[kind] then
    local scope = instrument_scope(kind, token)
    if scope then
      changes.journal_list_event(b.journal, INSTRUMENT_JOURNAL[kind], scope, placed.type, placed.index, placed.index2, song:is_undo_redoing(),
        linker(kind == "instruments" and b.ilog or (token and INSTRUMENT_LOGS[kind] and token[INSTRUMENT_LOGS[kind]])))
    end
  end
  if kind == "sequence" and op then changes.journal_sequence_event(b.journal, song:is_undo_redoing()) end
  local list = lists[kind]
  if not list then return end -- (the sequence: nothing is watched per slot)
  if kind == "devices" and op then
    -- (the journal's places are device places: they move, and a change that made or removed a device is told on the producer's undo)
    for track_index, candidate in ipairs(b.groups.tracks) do
      if candidate == token then
        changes.journal_device_event(b.journal, track_index, op.type, op.index, op.index2, song:is_undo_redoing(), linker(token.dlog))
      end
    end
  end
  if kind == "patterns" and op then changes.journal_pattern_event(b.journal, op.type, op.index) end
  local fine = false
  if op and op.type == "insert" then
    fine = remember(list, op.index)
    if type(op.index) == "number" and op.index >= 1 then
      if kind == "instruments" then
        watch_instrument(song, op.index)
      elseif kind == "patterns" then
        watch_pattern(song, op.index, track_total(song))
      elseif kind == "devices" then
        watch_device(owner:device(op.index), list[op.index], token)
      elseif kind == "samples" then
        watch_sample(owner:sample(op.index), list[op.index], token)
      elseif kind == "phrases" then
        watch_phrase(owner:phrase(op.index), list[op.index], token)
      elseif kind == "modsets" then
        watch_modset(owner:sample_modulation_set(op.index), list[op.index], token)
      elseif kind == "chains" then
        watch_chain(owner:sample_device_chain(op.index), list[op.index], token)
      end
    end
  elseif op and op.type == "remove" then
    if (INSTRUMENT_LOGS[kind] or kind == "instruments") and type(op.index) == "number" then retire(list[op.index]) end
    fine = forget(list, op.index)
  elseif op and op.type == "swap" then
    fine = exchange(list, op.index, op.index2)
  end
  -- n-instruments: an instrument's lists, a set's or a chain's devices, out of step with what Renoise has (an instrument copied
  -- tells its lists in its own order: lab), are counted again on their own: that list's log starts again (its places are
  -- unknown), and the rest of the song keeps its places.
  if INSTRUMENT_LOGS[kind] and token then
    local count = instrument_list_count(kind, owner)
    if not fine or (count and #list ~= count) then recount(kind, token, owner, list, count) end
    return
  end
  if not fine then b.reattach = true end
end

-- A change to the track list. An insert or removal keeps the epoch and moves the layout: the shift is kept, so a request
-- made in an earlier layout can be carried to the present, and it is told to Kumi at once, before any later answer. Anything
-- else (a swap, notifications that don't add up to the count) moves the epoch.
local function on_tracks(op)
  local song = renoise.song()
  if not song then return end
  local count = track_total(song)
  local expected = b.track_count
  if op.type == "insert" then expected = expected + 1 elseif op.type == "remove" then expected = expected - 1 end
  local swapped = op.type == "swap" and expected == count and exchange(b.groups.tracks, op.index, op.index2)
  if swapped then
    -- (the tracks are the same objects in another order: they keep their watchers and their groups move with them)
    -- (a swap moves the epoch and the layout, and is kept as a shift: a job can follow a track through the swaps of a producer, or its own)
    bump_epoch("tracks were swapped", true, { "tracks" }, true)
    b.layout = b.layout + 1
    b.shifts[#b.shifts + 1] = { layout = b.layout, type = "swap", index = op.index, index2 = op.index2 }
    if #b.shifts > M.SHIFT_HISTORY then table.remove(b.shifts, 1) end
    changes.journal_track_event(b.journal, "swap", op.index, song:is_undo_redoing(), op.index2)
    conns:event("tracks", { count = count, made = json.array(), gone = json.array(), layout = b.layout, epoch = epoch() })
    return
  end
  if (op.type ~= "insert" and op.type ~= "remove") or expected ~= count or type(op.index) ~= "number" then
    bump_epoch(op.type == "swap" and "tracks were swapped" or "the tracks changed in a way the tool can't follow")
    b.track_count = count
    b.reattach = true
    conns:event("tracks", { count = count, made = json.array(), gone = json.array(), layout = b.layout, epoch = epoch() })
    return
  end
  b.track_count = count
  b.layout = b.layout + 1
  b.shifts[#b.shifts + 1] = { layout = b.layout, type = op.type, index = op.index }
  if #b.shifts > M.SHIFT_HISTORY then table.remove(b.shifts, 1) end
  local listed = true
  changes.journal_track_event(b.journal, op.type, op.index, song:is_undo_redoing(), nil, track_linker)
  if op.type == "insert" then
    listed = remember(b.groups.tracks, op.index)
    if type(op.index) == "number" and op.index >= 1 then
      watch_track(song, op.index)
      for p = 1, #song.patterns do
        watch_pattern_track(song:pattern(p), op.index, b.groups.patterns[p], b.groups.tracks[op.index])
      end
    end
  else
    listed = forget(b.groups.tracks, op.index)
  end
  if not listed then b.reattach = true end
  conns:event("tracks", {
    count = count,
    made = json.array(op.type == "insert" and { op.index } or {}),
    gone = json.array(op.type == "remove" and { op.index } or {}),
    layout = b.layout,
    epoch = epoch(),
  })
end

local function flush_events()
  b.held_insert = nil -- (an undo or redo is over by the next callback: its pairs with it)
  if not observers:attached() then return end
  local song = renoise.song()
  if not song then return end
  local dirty = observers:take_dirty()
  if dirty.transport and listening.active() then
    -- (M5 m5-renoise: Renoise reads as playing while it renders; Kumi's render isn't the producer's transport, told once it ends)
    observers.dirty.transport = true
    dirty.transport = nil
  end
  if dirty.selection then
    local value = selection(song)
    if observers:changed("selection", value) then conns:event("selection", value) end
  end
  if dirty.transport then
    local value = transport(song)
    if observers:changed("transport", value) then conns:event("transport", value) end
  end
  if dirty.project and observers:changed("project", status.project(b.memory)) then send_project() end
  if b.undone then
    b.undone = false
    changes.journal_check(b.journal)
  end
  -- (what Renoise's undo changes inside a Doofer, a Splitter or a Notepad no notifier tells: read again about once a second,
  -- POLL_EACH devices a tick until the round is done)
  local now_ms = M.now_ms()
  if changes.journal_active(b.journal) and (b.polling or now_ms - (b.polled_ms or 0) >= M.POLL_MS) then
    if not b.polling then b.polled_ms = now_ms end
    b.polling = not changes.journal_poll(b.journal, M.POLL_MOST, M.POLL_EACH)
  end
  for _, event in ipairs(changes.journal_take(b.journal)) do
    conns:event("undone_in_daw", { descs = json.array(event.descs), how = event.how, epoch = epoch() })
  end
  if b.reattach then
    -- (the lists were out of step: what moved meanwhile can't be told)
    places.gap(b.plog)
    places.gap(b.ilog)
    attach(song)
  elseif observers.dead > M.MAX_DEAD then
    observers:prune() -- (no call to Renoise: the entries of deleted objects are dropped)
  end
end

-- Requests

local handlers = {}

function handlers.ping()
  return json.object()
end

function handlers.status()
  return status.status(b.memory, host.bundle_path)
end

-- The reads. A long one is a job, run a slice at a time from the timers (the fast one while any is pending). Every
-- answer names the epoch and the track layout its refs were made in.
local function as_job(work)
  return { kumi_job = work }
end

local function stamped(result, layout)
  if type(result) == "table" then result.layout = layout end
  b.layout_told = true
  return result
end

function handlers.read(params)
  return as_job(function(job) return stamped(reads.read(job, params, job.epoch), job.layout) end)
end

function handlers.snapshot()
  return as_job(function(job) return stamped(focus.snapshot(job, job.epoch, b.changed or 0), job.layout) end)
end

-- M6: the song as records for catch-up and watch_me (digest.lua), with the song's id and identity as the project row.
function handlers.digest(params)
  return as_job(function(job)
    local project = status.project(b.memory)
    if type(project) == "table" and renoise.song() then project.identity = rows.song_extra(renoise.song()).identity end
    return digest.digest(job, params, job.epoch, { project = project, changed = b.content or 0 })
  end)
end

function handlers.focus()
  local answer = focus.focus(epoch(), b.changed or 0)
  -- (while Kumi renders, bodies are refused: the focus says Renoise is busy, so Kumi keeps what it showed)
  if listening.active() then answer.busy = "rendering" end
  return stamped(answer, b.layout)
end

function handlers.focus_body(params)
  return as_job(function(job) return stamped(focus.focus_body(job, params, job.epoch), job.layout) end)
end

-- The faders and pans of a track Kumi changed are watched for the producer's undo (and for nothing else: an automation moves
-- them all the time, and `changed` is not for that). Once per track.
local function watch_params(index)
  local token = b.groups and b.groups.tracks[index]
  local song = renoise.song()
  if not token or token.params_watched or not song or index > track_total(song) then return end
  token.params_watched = true
  local track = song:track(index)
  observers:owned(token, nil, function()
    for _, parameter in ipairs({ track.prefx_volume, track.prefx_panning, track.postfx_volume, track.postfx_panning, track.prefx_width }) do
      observers:watch_undo(parameter.value_observable)
    end
  end)
end

-- A parameter's value is watched the same way, for the producer's undo only; once per parameter.
local function watch_param(track_index, device_index, k)
  local token = b.groups and b.groups.tracks[track_index]
  local device_token = token and token.devices and token.devices[device_index]
  local song = renoise.song()
  if not device_token or not song then return end
  device_token.params_watched = device_token.params_watched or {}
  if device_token.params_watched[k] then return end
  device_token.params_watched[k] = true
  local parameter = song:track(track_index):device(device_index):parameter(k)
  observers:owned(device_token, token, function() observers:watch_undo(parameter.value_observable) end)
end

-- n-instruments: what a change of Kumi's wrote is watched for the producer's undo (and nothing else), once per observable: an
-- instrument's, a sample's or a phrase's settings aren't all watched otherwise. The watcher lives as long as the song's.
local undo_watch_fn
local function watch_undo(observable)
  if not observers or not observable then return end
  undo_watch_fn = undo_watch_fn or guard("undo watch", function()
    -- (the journal first: it is empty until Kumi makes a change, and asking Renoise for the song costs more than looking)
    if not changes.journal_active(b.journal) then return end
    local song = renoise.song()
    if song and song:is_undo_redoing() then b.undone = true end
  end)
  local ok, has = pcall(function() return observable:has_notifier(undo_watch_fn) end)
  if not ok or has then return end
  observers:owned(nil, nil, function() observers:watch_undo_with(observable, undo_watch_fn) end)
end

-- Changes (M3): see changes.lua. Every request carries the epoch (and the layout) its refs were made in.
-- A dialog holds Renoise's timers while its sockets keep answering (measured, 3.5.4: no tool timer ran in 3 s behind the
-- "unsaved changes" prompt; with Renoise in the background they run as ever, 57-73 ms apart for 50). So a change that finds
-- the timers silent for this long refuses `busy`, and a sliced restore waits before each slice. A callback of our own that
-- runs long holds the timers too: `held` moves the beat by that time.
M.DIALOG_MS = 400

local function busy()
  local song = renoise.song()
  local listening_reason = listening.busy_reason()
  if listening_reason then return listening_reason end
  if song and song.rendering then return "Renoise is rendering the song" end
  -- (What a socket callback does itself before it asks, decoding a big request, is its own time, not a dialog's: from its start.)
  if b.alive_ms and (b.work_ms or M.now_ms()) - b.alive_ms >= M.DIALOG_MS then return "a Renoise dialog is open (its timers aren't running)" end
  return nil
end

-- Track numbers carried from layout `since` to now: the new number, or nil and why ("gone": that track was deleted; "unknown":
-- the shifts since aren't kept).
local function shifted(index, since)
  if type(since) ~= "number" or since > b.layout or b.layout - since > #b.shifts then return nil, "unknown" end
  if since == b.layout then return index end
  local moved = refs.shift(index, b.shifts, since)
  if not moved then return nil, "gone" end
  return moved
end

-- A place among the tracks carried the same way: a track removed there leaves the place (nil and "unknown" as above).
local function place_shifted(index, since)
  if type(since) ~= "number" or since > b.layout or b.layout - since > #b.shifts then return nil, "unknown" end
  return refs.shift_place(index, b.shifts, since)
end

-- The places a change keeps of devices (on the track now at `t`), patterns and instruments: a mark, and where the object is now.
local function device_log(t)
  local token = b.groups and b.groups.tracks[t]
  return token and token.dlog or nil
end

local function unknown_log() return nil, "unknown" end

-- M4 (n-song): a member of a pattern's track (its slot colour, an automation's points) watched for the producer's undo, once.
local function watch_ptrack_undo(p, t, name, observable)
  local pattern_token, track_token = b.groups and b.groups.patterns[p], b.groups and b.groups.tracks[t]
  local song = renoise.song()
  if not pattern_token or not track_token or not song or p > #song.patterns or t > track_total(song) then return end
  pattern_token.undo_watched = pattern_token.undo_watched or setmetatable({}, { __mode = "k" })
  local watched = pattern_token.undo_watched[track_token] or {}
  pattern_token.undo_watched[track_token] = watched
  if watched[name] then return end
  watched[name] = true
  observable = observable or song:pattern(p):track(t)[name]
  observers:owned(pattern_token, track_token, function() observers:watch_undo(observable) end)
end

-- n-instruments: the log of a list of the instrument now at `i`, or of the set or chain now at `at` in it.
local function instrument_log(kind, i, at)
  local token = b.groups and b.groups.instruments[i]
  if not token then return nil end
  if kind == "moddevices" or kind == "chaindevices" then
    local owner = (kind == "moddevices" and token.modsets or token.chains or {})[at]
    return owner and owner.log or nil
  end
  return token[INSTRUMENT_LOGS[kind]]
end

local function phrase_token(i, ph)
  local token = b.groups and b.groups.instruments[i]
  return token and token.phrases and token.phrases[ph] or nil
end

-- A pattern's or phrase's count of changed lines as a change kept it ("p12=40"), as the journal moved it on: Renoise's redo
-- wrote that change's lines again (changes_base `journal_check`), so its count starts again from after the redo.
local function counted_from(mark)
  return type(mark) == "string" and b.retouch and b.retouch[mark] or mark
end

-- The journal's: a pattern a change made, made again by Renoise's redo at `p`: the change's count for it (`mark`) goes on from
-- that pattern's own.
local function retouched_at(mark, p)
  local token = type(mark) == "string" and type(p) == "number" and b.groups and b.groups.patterns[p] or nil
  if not (token and token.pid) then return end
  b.retouch = b.retouch or {}
  b.retouch[mark] = token.pid .. "=" .. token.touches
end

-- The journal's: a change Renoise's redo put back holds what Kumi wrote again; its counts (`touch`, and a copy's source's) now
-- start from here.
local function retouched(applied)
  b.retouch = b.retouch or {}
  for _, mark in ipairs({ applied.touch, type(applied.source) == "table" and applied.source.touch or nil }) do
    local pid = type(mark) == "string" and mark:match("^(.*)=%d+$") or nil
    local token
    for _, candidate in ipairs(pid and b.groups and b.groups.patterns or {}) do
      if candidate.pid == pid then token = candidate break end
    end
    for _, instrument in ipairs(pid and not token and b.groups and b.groups.instruments or {}) do
      for _, phrase in ipairs(instrument.phrases or {}) do
        if phrase.pid == pid then token = phrase break end
      end
      if token then break end
    end
    if token then b.retouch[mark] = pid .. "=" .. token.touches end
  end
end

local place_env = {
  device_mark = function(t)
    local log_ = device_log(t)
    return log_ and places.mark(log_) or nil
  end,
  device_at = function(t, n, mark)
    local log_ = device_log(t)
    if not log_ then return unknown_log() end
    return places.item(log_, n, mark)
  end,
  -- (where a device deleted from place `n` goes back: after the one before it; the mixer device is always first)
  device_after = function(t, n, mark)
    local log_ = device_log(t)
    if not log_ then return unknown_log() end
    return places.after(log_, n, mark, 1)
  end,
  -- (M4 n-devices: the device Kumi put back at `now` on track `t` is the one it removed, `removed` the mark just after the
  -- remove: places kept from before it follow it)
  device_back = function(t, removed, now)
    local log_ = device_log(t)
    return log_ and places.back(log_, removed, now) or false
  end,
  -- (the count of pattern `p`'s changed lines so far, as a mark; and whether any changed since one: nil and "unknown" when the
  -- mark is of another pattern or watch)
  pattern_touch_mark = function(p)
    local token = b.groups and b.groups.patterns[p]
    return token and token.pid and (token.pid .. "=" .. token.touches) or nil
  end,
  pattern_touched_since = function(p, mark)
    local token = b.groups and b.groups.patterns[p]
    local pid, count
    mark = counted_from(mark)
    if type(mark) == "string" then pid, count = mark:match("^(.*)=(%d+)$") end
    if not token or not pid or pid ~= token.pid then return nil, "unknown" end
    return token.touches > tonumber(count)
  end,
  pattern_mark = function() return places.mark(b.plog) end,
  pattern_at = function(p, mark) return places.item(b.plog, p, mark) end,
  instrument_mark = function() return places.mark(b.ilog) end,
  instrument_at = function(i, mark) return places.item(b.ilog, i, mark) end,
  -- n-instruments: where an instrument deleted from place `i` goes back; the logs of an instrument's lists (`kind` samples,
  -- phrases, modsets or chains, of the instrument now at `i`) and of a set's or chain's devices (`kind` moddevices or
  -- chaindevices, `i` and `at` the instrument and the set or chain now); a phrase's count of changed lines.
  instrument_after = function(i, mark) return places.after(b.ilog, i, mark) end,
  list_mark = function(kind, i, at)
    local log_ = instrument_log(kind, i, at)
    return log_ and places.mark(log_) or nil
  end,
  list_at = function(kind, i, at, n, mark)
    local log_ = instrument_log(kind, i, at)
    if not log_ then return unknown_log() end
    return places.item(log_, n, mark)
  end,
  list_after = function(kind, i, at, n, mark, first)
    local log_ = instrument_log(kind, i, at)
    if not log_ then return unknown_log() end
    return places.after(log_, n, mark, first)
  end,
  phrase_touch_mark = function(i, ph)
    local token = phrase_token(i, ph)
    return token and token.pid and (token.pid .. "=" .. token.touches) or nil
  end,
  -- What an object's own logs are (taken before Kumi deletes it), for the one Kumi puts back to take on (`adopt`).
  logs_of = function(of, n)
    local token = b.groups and b.groups.instruments[n[1]]
    if not token then return nil end
    local function id(log_) return log_ and log_.id or nil end
    if of == "instrument" then
      local kept = { slog = id(token.slog), phlog = id(token.phlog), mlog = id(token.mlog), clog = id(token.clog), sets = {}, chains = {}, phrases = {} }
      for k, owned in ipairs(token.modsets or {}) do kept.sets[k] = id(owned.log) or false end
      for k, owned in ipairs(token.chains or {}) do kept.chains[k] = id(owned.log) or false end
      for k, owned in ipairs(token.phrases or {}) do kept.phrases[k] = owned.pid and { pid = owned.pid, touches = owned.touches } or false end
      return kept
    elseif of == "modset" or of == "chain" then
      local owned = (of == "modset" and token.modsets or token.chains or {})[n[2]]
      return owned and { log = id(owned.log) } or nil
    elseif of == "phrase" then
      local owned = (token.phrases or {})[n[2]]
      return owned and owned.pid and { pid = owned.pid, touches = owned.touches } or nil
    end
    return nil
  end,
  -- The object Kumi put back at `n` takes on the logs its deleted self had (`kept`, from logs_of): all or nothing.
  adopt = function(of, n, kept)
    local token = b.groups and b.groups.instruments[n[1]]
    local logs = b.retired and b.retired.logs or {}
    if not token or type(kept) ~= "table" then return false end
    local function find(id_) return type(id_) == "string" and logs[id_] or nil end
    if of == "instrument" then
      local sets, chains, phrases = token.modsets or {}, token.chains or {}, token.phrases or {}
      if #sets ~= #(kept.sets or {}) or #chains ~= #(kept.chains or {}) or #phrases ~= #(kept.phrases or {}) then return false end
      local own = { slog = find(kept.slog), phlog = find(kept.phlog), mlog = find(kept.mlog), clog = find(kept.clog) }
      if not (own.slog and own.phlog and own.mlog and own.clog) then return false end
      for k = 1, #sets do if not find(kept.sets[k]) then return false end end
      for k = 1, #chains do if not find(kept.chains[k]) then return false end end
      for key, log_ in pairs(own) do token[key] = log_ end
      for k = 1, #sets do sets[k].log = find(kept.sets[k]) end
      for k = 1, #chains do chains[k].log = find(kept.chains[k]) end
      for k = 1, #phrases do
        if type(kept.phrases[k]) == "table" then phrases[k].pid, phrases[k].touches = kept.phrases[k].pid, kept.phrases[k].touches end
      end
      return true
    elseif of == "modset" or of == "chain" then
      local owned = (of == "modset" and token.modsets or token.chains or {})[n[2]]
      local log_ = find(kept.log)
      if not owned or not log_ then return false end
      owned.log = log_
      return true
    elseif of == "phrase" then
      local owned = (token.phrases or {})[n[2]]
      if not owned or type(kept.pid) ~= "string" then return false end
      owned.pid, owned.touches = kept.pid, kept.touches
      return true
    end
    return false
  end,
  -- The object Kumi put back, now at `now` in its list, is the one it removed (`removed`: the mark just after the remove).
  back = function(kind, i, at, removed, now)
    local log_ = kind == "instruments" and b.ilog or instrument_log(kind, i, at)
    return log_ and places.back(log_, removed, now) or false
  end,
  phrase_touched_since = function(i, ph, mark)
    local token = phrase_token(i, ph)
    local pid, count
    mark = counted_from(mark)
    if type(mark) == "string" then pid, count = mark:match("^(.*)=(%d+)$") end
    if not token or not pid or pid ~= token.pid then return nil, "unknown" end
    return token.touches > tonumber(count)
  end,
}

-- The env a change runs in: built once (its functions reach the bridge's state through upvalues, and no change writes to
-- it), the journal taken afresh each time (it is made when the payload starts). The tick asked for one twenty times a
-- second, each a table of some thirty closures to collect.
local change_env_kept
local function change_env()
  local kept = change_env_kept
  if kept then
    kept.journal = b.journal
    return kept
  end
  local env = {
    epoch = epoch, layout = function() b.layout_told = true return b.layout end, journal = b.journal, watch_params = watch_params,
    watch_param = watch_param, busy = busy, now_ms = function() return M.now_ms() end, shifted = shifted, place_shifted = place_shifted,
    watch_pattern_track = watch_ptrack_undo,
    survives = function(from, kind) return survives(from, kind) end,
    watch_undo = watch_undo, -- (n-instruments)
    -- (how many changes the song's notifiers have counted: run-lua tells whether its chunk changed anything)
    changes = function() return b.changed or 0 end,
    in_notifier = function() return guarded > 0 end,
    is_guard = function(fn) return guards[fn] == true end,
    -- (M4 n-devices) Kumi's work folder beside its discovery folder (instrument files the host edits), and a load of an
    -- instrument file into slot i that the places log doesn't take for a delete.
    work_dir = function()
      local dir = b.cfg and b.cfg.discovery_dir
      return dir and (dir:gsub("[/\\][^/\\]*[/\\]?$", "") .. "/work") or nil
    end,
    replacing_instrument = function(i, fn)
      -- (the instrument loaded in its place takes on the logs it had, so the places Kumi's records keep inside it follow)
      local kept = place_env.logs_of("instrument", { i })
      b.replacing = { i = i, insert = true, remove = true }
      local ok, err = pcall(fn)
      -- (told now or on a later pass: kept for those two ops for a moment)
      if b.replacing then b.replacing.deadline = os.clock() + 2 end
      if not ok then
        b.replacing = nil
        error(err, 0)
      end
      if kept then place_env.adopt("instrument", { i }, kept) end
    end,
  }
  for name, fn in pairs(place_env) do env[name] = fn end
  change_env_kept = env
  return env
end

function handlers.change_preview(params)
  return changes.preview(params, change_env())
end

function handlers.change_apply(params)
  return changes.apply(params, change_env())
end

function handlers.change_restore(params)
  return changes.restore(params, change_env())
end

-- (M4 n-devices) A device's preset data and shape, for Kumi's host to edit Doofers, Splitters and Notepads.
function handlers.preset_read(params)
  return changes.preset_read(params, change_env())
end

-- (M4 n-devices) An instrument saved to Kumi's work folder, for its host to edit its macro mappings.
function handlers.instrument_export(params)
  return changes.instrument_export(params, change_env())
end

-- (M4 n-devices) An instrument's macros and their mappings, for a read.
function handlers.macro_read(params)
  return changes.macro_read(params, change_env())
end

-- (M4 n-devices) Actions on a device: its own editor window, its settings saved as a preset in the user library.
function handlers.device_editor(params)
  return changes.device_editor(params, change_env())
end

function handlers.preset_save(params)
  return changes.preset_save(params, change_env())
end

-- n-instruments: an instrument written to a file (an action: no undo step, nothing in the song changes).
function handlers.instrument_save(params)
  return changes.save_instrument(params, change_env())
end

-- A step of undo around a plan: Renoise has none that spans callbacks, so every change stays its own step (merged: false).
function handlers.undo_step_begin(params)
  local label = type(params) == "table" and params.label or nil
  if type(label) ~= "string" or label == "" then
    error({ code = "bad-params", message = "label must be a string" }, 0)
  end
  b.steps = (b.steps or 0) + 1
  b.open_step = { id = "s" .. b.steps, label = label }
  return { stepId = b.open_step.id, merged = false }
end

function handlers.undo_step_end(params)
  local id = type(params) == "table" and params.stepId or nil
  local open = b.open_step
  if type(id) ~= "string" or not open or open.id ~= id then
    error({ code = "failed", message = "that step isn't open" }, 0)
  end
  b.open_step = nil
  return { desc = open.label, entry = false, merged = false }
end

-- The emergency stop.
function handlers.transport_stop()
  local song = renoise.song()
  if not song then error({ code = "no-song", message = "Renoise has no song open" }, 0) end
  local transport = song.transport
  local running = transport.playing
  local recording = running and transport.edit_mode or false
  if running then transport:stop() end
  return { stopped = running, wasRecording = recording }
end

-- Play: from where the pattern is (no arguments), or from a line of a sequence slot, or of the pattern being edited when only
-- `line` is given (Renoise's number, from 0). Not an undo step, and nothing to put back. A slot is a place in the sequence, so a request that names one
-- may name the epoch it was read in.
function handlers.transport_play(params)
  local song = rows.the_song()
  if song.rendering then error({ code = "busy", message = "Renoise is rendering the song" }, 0) end
  params = type(params) == "table" and params or {}
  local sequence, line = params.sequence, params.line
  local function bad(message) error({ code = "bad-params", message = message }, 0) end
  local transport = song.transport
  local started = false
  if sequence ~= nil or line ~= nil then
    if params.epoch ~= nil and params.epoch ~= epoch() then
      error({ code = "stale", message = "those slots are from an earlier epoch (" .. tostring(params.epoch) .. "; now " .. epoch() .. "): read again" }, 0)
    end
    local slots = #song.sequencer.pattern_sequence
    if sequence ~= nil and (type(sequence) ~= "number" or sequence ~= math.floor(sequence) or sequence < 1 or sequence > slots) then
      bad(string.format("sequence is a sequence slot from 1 to %d", slots))
    end
    local pattern = sequence and song:pattern(song.sequencer:pattern(sequence)) or song:pattern(song.selected_pattern_index)
    if line == nil then line = 0 end
    if type(line) ~= "number" or line ~= math.floor(line) or line < 0 or line >= pattern.number_of_lines then
      bad(string.format("line is a line of that pattern, from 0 to %d", pattern.number_of_lines - 1))
    end
    if sequence ~= nil then transport:start_at(renoise.SongPos(sequence, line + 1)) else transport:start_at(line + 1) end
    started = true
  elseif not transport.playing then
    transport:start(renoise.Transport.PLAYMODE_CONTINUE_PATTERN)
    started = true
  end
  local position = transport.playback_pos
  return {
    epoch = epoch(), layout = layout_now(), started = started, playing = transport.playing,
    sequence = sequence or position.sequence, line = sequence and line or (line or position.line - 1),
  }
end

-- M4 (n-song): auditioning a note, selecting or showing, saving the song (actions.lua). None is an undo step or a change.
function handlers.audition_note(params)
  return actions.audition(params, change_env())
end

handlers.select = function(params)
  return actions.select(params, change_env())
end

function handlers.project_save(params)
  return actions.project_save(params, change_env())
end

-- A line in Renoise's status bar (never a dialog), for a producer working there while Kumi finishes or needs them: one
-- line of text, control characters as spaces. Not a change.
function handlers.show_status(params)
  local text = type(params) == "table" and params.text or nil
  if type(text) ~= "string" or text == "" or #text > 512 then error({ code = "bad-params", message = "text: one line to show, at most 512 bytes" }, 0) end
  renoise.app():show_status((text:gsub("%c", " ")))
  return { shown = true }
end

-- (#401) A chore kept as a menu entry and a shortcut: a file and Renoise's menus, not the song.
function handlers.script_install(params) return chores.install(params, b.cfg) end
function handlers.script_remove(params) return chores.remove(params, b.cfg) end

handlers["renoise.reload"] = function()
  host.request_reload()
  return { reloading = true }
end

-- (M5 m5-renoise) Listening: renders as jobs, the audio behind a ref, a render played out loud, scratch copies. See listening.lua,
-- audition.lua. Kumi's own solo, mute and loop writes for them are quiet: the song's notifiers don't count them as edits.
local function quiet(fn)
  b.quiet = (b.quiet or 0) + 1
  local ok, err = pcall(fn)
  b.quiet = b.quiet - 1
  if not ok then error(err, 0) end
end

-- (the change env with listening's additions, a table of its own built once, the journal taken afresh each time)
local listen_env_kept
local function listen_env()
  local env = listen_env_kept
  if env then
    env.journal = b.journal
    return env
  end
  env = {}
  for name, value in pairs(change_env()) do env[name] = value end
  env.document = function() return b.document end
  env.cfg = function() return b.cfg end
  env.quiet = quiet
  env.pattern_mark = place_env.pattern_touch_mark
  env.song_id = function() return status.song_id(b.memory, rows.the_song()) end
  env.unix_ms = unix_ms
  -- (a render stops playback: an audition playing ends first)
  env.before_render = function() audition.halt(env, "a render") end
  listen_env_kept = env
  return env
end

-- Answered at once while Kumi's render runs (contract §3); anything else waits, refused busy with nothing written.
local LISTEN_FREE = { ping = true, status = true, focus = true, listen_status = true, listen_cancel = true, audition_stop = true }

function handlers.render_start(params) return listening.render_start(params, listen_env()) end
function handlers.listen_status(params) return listening.status(params, listen_env()) end
function handlers.listen_cancel(params) return listening.cancel(params, listen_env()) end
function handlers.audio_file(params) return listening.audio_file(params, listen_env()) end
function handlers.audition(params) return audition.play(params, listen_env()) end
function handlers.audition_stop(params) return audition.stop(params, listen_env()) end
function handlers.scratch(params) return scratch.handle(params, listen_env()) end

-- Copies a crash left in this song are swept on the first tick after the bridge loads or a song opens (a song is changed in a
-- callback of the tool's own, not inside Renoise's document notifiers); Kumi is told, now or when it next connects.
local function sweep_scratch()
  if not b.scratch_sweep_due or not b.cfg then return end
  local song = renoise.song()
  if not song or listening.active() or busy() then return end
  b.scratch_sweep_due = false
  local swept = scratch.sweep(song, listen_env())
  if swept then
    if conns:authenticated_count() > 0 then conns:event("scratch_swept", swept) else b.scratch_swept = swept end
  end
end

-- (Exposed for the tests, which add methods of their own.)
M.handlers = handlers

-- A request may name the track layout its refs were made in (`layout`): the refs that name a track, or something on one,
-- are carried through the shifts since, to where the tracks are now. A track deleted meanwhile answers `gone`; a layout the
-- tool no longer remembers (or does not know) answers `stale`.
local function carried(params, carry)
  local since = type(params) == "table" and params.layout or nil
  if since == nil then return params end
  if type(since) ~= "number" or since ~= math.floor(since) or since < 0 then
    error({ code = "bad-params", message = "layout must be a whole number from 0" }, 0)
  end
  if since == b.layout then return params end
  -- Only refs on a track move with the layout: a request naming none is never refused for it.
  if not refs.names_tracks(params) then return params end
  if since > b.layout then
    error({ code = "stale", message = "that layout (" .. since .. ") is from another epoch (now " .. epoch() .. "): read again" }, 0)
  end
  if b.layout - since > #b.shifts then
    error({ code = "stale", message = "that layout is too old to carry its refs to the present: read again" }, 0)
  end
  if carry then return carry(params, b.shifts, since, epoch()) end
  local moved, err = refs.carry(params, epoch(), b.shifts, since)
  if not moved then error(err, 0) end
  return moved
end

local CARRIED = { read = true, focus_body = true, change_preview = true, change_apply = true, change_restore = true,
  -- (M4 n-song: the actions that name tracks, and the save, whose epoch says which song)
  audition_note = true, select = true, project_save = true,
  instrument_save = true,
  -- (M4 n-devices)
  preset_read = true, instrument_export = true, macro_read = true, device_editor = true, preset_save = true }
-- (M5 m5-renoise: the listening requests that name refs)
CARRIED.render_start, CARRIED.audio_file, CARRIED.scratch = true, true, true

-- An answer whose epoch is not the request's says what survives: the kinds of ref the request held that name the same things
-- in the answer's epoch (`tracks` too, for a reader that knows only that).
local ALL_KINDS = { "song", "track", "device", "param", "ptrack", "slot", "seq", "pattern", "instrument", "sample", "phrase",
  "modset", "moddevice", "chain", "chaindevice", "chainparam", "pluginparam" }
local function say_kept(result, from)
  if type(result) ~= "table" or type(from) ~= "string" or type(result.epoch) ~= "string" or result.epoch == from then return result end
  local kinds = {}
  for _, kind in ipairs(ALL_KINDS) do
    if survives(from, kind, result.epoch) then kinds[#kinds + 1] = kind end
  end
  if #kinds > 0 then
    result.kept = { from = from, tracks = survives(from, "track", result.epoch), kinds = kinds }
  end
  return result
end

function M.handle(_, request)
  local handler = handlers[request.method]
  if not handler then error({ code = "unknown-method", message = "no method " .. request.method }) end
  -- (M5 m5-renoise: while Kumi's render runs, only the requests that read its state are answered)
  if not LISTEN_FREE[request.method] then
    listening.check(listen_env())
    if listening.active() then error({ code = "busy", message = "Renoise is rendering for Kumi; nothing changed" }, 0) end
  end
  local params = request.params
  local from
  if CARRIED[request.method] then
    from = type(params) == "table" and params.epoch or nil
    -- (M4 n-song: a restore that finds what it changed by places leaves out the refs its record carries: see changes.lua)
    local loose = request.method == "change_restore" and type(params) == "table" and changes.loose_refs[params.kind] or nil
    if loose then
      local copy = {}
      for k, v in pairs(params) do copy[k] = v end
      for _, field in ipairs(loose) do
        if field == "target" and type(copy.target) == "table" then copy.target = { kind = copy.target.kind } else copy[field] = nil end
      end
      params = copy
    end
    -- A ref of an earlier epoch that names the same thing now is made in this one, whatever epoch the request names (Kumi keeps
    -- the names of what a device or a sequence change left alone); so is the request's own epoch, when every ref of it was.
    if type(params) == "table" then
      -- (M4 n-devices) A restore of a kind that finds things by places only: the refs in its target and `created` name
      -- nothing it uses, and one of an earlier epoch mustn't hold the request back.
      if request.method == "change_restore" and changes.follows_places(params.kind) and (params.created ~= nil or params.target ~= nil) then
        local copy = {}
        for k, v in pairs(params) do copy[k] = v end
        copy.created = nil
        if type(copy.target) == "table" then copy.target = { kind = copy.target.kind, track = copy.target.track, device = copy.target.device } end
        params = copy
      end
      local left
      params, left = refs.relabel_surviving(params, epoch(), survives)
      -- (n-instruments: a restore that finds what it changed by its places, not by its refs, goes ahead in the same song though
      -- its refs went stale; it reads none of them, and any ref read still goes through refs.expect)
      local by_place = request.method == "change_restore"
        and (changes.restores_by_place(params.kind) or changes.redone_entry(b.journal, params.of) ~= nil)
      if type(from) == "string" and from ~= epoch() and (by_place or not left[from]) and survives(from, "song") then
        local copy = {}
        for k, v in pairs(params) do copy[k] = v end
        copy.epoch = epoch()
        params = copy
      end
    end
    if request.method == "change_restore" and type(params) == "table" and changes.carry_for[params.kind] then
      -- (a deleted track's own ref is gone by design: only its place is carried; a deleted slot's mutes leave out a track gone)
      params = carried(params, changes.carry_for[params.kind])
    else
      params = carried(params)
    end
  end
  local started = M.now_ms()
  local result = handler(params)
  -- (A handler that ran long held the timers itself: that is not a dialog.)
  local held = M.now_ms() - started
  if held >= 50 and b.alive_ms then b.alive_ms = b.alive_ms + held end
  if type(result) == "table" and type(result.kumi_job) == "function" then
    local work = result.kumi_job
    result.kumi_job = function(job) return say_kept(work(job), from) end
    return result
  end
  return say_kept(result, from)
end

-- Lifecycle, called by main.lua

function M.start(h, reloading)
  host, st = h, h.state
  st.bridge = st.bridge or { memory = {}, retry_ms = 0, started_ms = 0 }
  b = st.bridge
  -- (#401: the kept scripts are registered again by this payload, its closures replacing a reload's)
  b.chores_registered = nil
  b.journal = b.journal or changes.journal_new()
  b.journal.edits = function() return b.edits or 0 end
  b.journal.retouched = retouched
  b.journal.retouched_at = retouched_at
  if b.started_ms == 0 then b.started_ms = unix_ms() end
  conns = server.new({
    connections = st.connections,
    now_ms = function() return M.now_ms() end,
    unix_ms = unix_ms,
    secret = function()
      local secret, why = config.secret(b.cfg)
      if not secret then log.write("error", "refusing a connection: " .. tostring(why)) end
      return secret, why
    end,
    identity = identity,
    on_request = M.handle,
    start_job = function(record, request, work, wrote) return jobs:add(record, request, work, wrote) end,
    cancel_jobs = function(record) jobs:cancel(record) end,
    on_authenticated = function(record)
      log.write("info", "Kumi connected from " .. record.key)
      show_status("Kumi connected")
      -- (M5 m5-renoise: copies swept while no Kumi was there)
      if b.scratch_swept then
        local swept = b.scratch_swept
        b.scratch_swept = nil
        conns:event("scratch_swept", swept)
      end
    end,
    log = log.write,
    on_too_many_open = function(open)
      b.remake = "too many connections held open after being refused (" .. open .. ")"
    end,
  })
  -- (a random start, 44 bits wide: two Renoise running at once, or one started again, never count through the same epochs, so
  -- a ref or a change from one is refused as stale by the other; and the place logs carry this run's own name, so a mark kept
  -- in one run never resolves in another's logs)
  b.epoch_n = b.epoch_n or wire.random28() * 65536
  b.run = b.run or string.sub(wire.nonce(), 1, 8)
  b.layout, b.shifts, b.shape = b.layout or 0, b.shifts or {}, b.shape or 0
  b.ids = b.ids or 0
  b.plog, b.ilog = b.plog or places.new(b.run .. "p"), b.ilog or places.new(b.run .. "i")
  b.cat, b.snap = b.cat or {}, b.snap or {}
  if not b.snap[b.epoch_n] then
    local snapshot = {}
    for _, category in ipairs(CATEGORIES) do snapshot[category] = b.cat[category] or 0 end
    b.snap[b.epoch_n] = snapshot
  end
  b.changed = b.changed or 0
  b.document = b.document or 1
  -- (M5 m5-renoise: the listening state, kept across reloads)
  listening.init(b)
  audition.init(b)
  status.listening = function() return listening.flags(b.cfg) end
  status.kumi_busy = function() return listening.busy_reason() end
  focus.rendering = function() return listening.active() ~= nil end
  reads_listening.pattern_mark = place_env.pattern_touch_mark
  -- (M7: where what Kumi keeps by place is now, for `read {kind: "where"}`)
  reads.places = place_env
  scratch.init(b)
  scratch.cfg = function() return b.cfg end
  if not reloading then b.scratch_sweep_due = true end
  -- A song's id (the one in the project row) and which opening of a song this is: the same while it stays open.
  rows.song_extra = function(song)
    -- (where the pid is unknown, when this Renoise was started stands in for it: `0:1` would be the same after every restart)
    return { id = status.song_id(b.memory, song), identity = (status.pid() or ("t" .. b.started_ms)) .. ":" .. b.document }
  end
  observers = sync.new(guard, b, {
    structural = on_structural,
    tracks = on_tracks,
    -- (every changed line counts for its pattern: the producer's edits, Renoise's undo and redo, Kumi's own writes. A notes
    -- change's undo goes through only if none came since its step; see changes_notes.lua)
    lines = function(pos, owner)
      -- (n-instruments: a phrase's lines count for the phrase)
      if owner then
        if owner.touches then owner.touches = owner.touches + 1 end
        return
      end
      local token = type(pos) == "table" and type(pos.pattern) == "number" and b.groups and b.groups.patterns[pos.pattern]
      if token and token.touches then token.touches = token.touches + 1 end
    end,
    -- (a track of the pattern made an alias or not: what its lines are changed)
    alias = function(token)
      if token.touches then token.touches = token.touches + 1 end
    end,
    -- (asked of every notifier once Kumi has made a change: was the producer's undo or redo what moved it? The journal
    -- first: it is empty until then, and asking Renoise for the song costs more than looking)
    undoing = function()
      if not changes.journal_active(b.journal) then return false end
      local song = renoise.song()
      return song ~= nil and song:is_undo_redoing()
    end,
  })
  jobs = jobs_module.new({
    now_ms = M.now_ms,
    unix_ms = unix_ms,
    epoch = epoch,
    layout = function() b.layout_told = true return b.layout end,
    shape = function() return b.shape end,
    finish = function(job, ok, value)
      conns:finish_job(job.record, job.request, ok, value)
    end,
    on_active = function(active) if host.set_fast then host.set_fast(active) end end,
  })
  status.cache = b
  if b.cfg and b.cfg.log_dir then configure_log(b.cfg.log_dir) end
  -- Two Renoises write to one log: each line says whose it is.
  log.set_tag(status.pid() and ("pid " .. status.pid()) or ("started " .. math.floor(b.started_ms / 1000)))
  -- The reference is read again on a reload (new log or discovery folders); the server is not remade for it.
  local configured = configure()
  if configured and (not reloading or not server_running()) then
    listen()
  elseif configured then
    -- Kept: the server. A changed discovery folder is followed: the file moves there.
    write_discovery()
  end
  attach(renoise.song())
  log.write("info", (reloading and "reloaded" or "started") .. " bridge " .. VERSION)
  log.flush()
end

function M.stop(_, reloading)
  -- (M5 m5-renoise: an audition ends; a render job puts back what it changed and lets go of its files)
  if b and host then
    pcall(function()
      local env = listen_env()
      audition.halt(env, "the tool stopped")
      listening.abort(env, "the tool was stopped or reloaded during the render: ask again")
    end)
  end
  if observers then observers:detach() end
  pcall(actions.tick, M.now_ms(), true)
  if jobs then jobs:abort_all("reloading", "the tool was stopped or reloaded: ask again") end
  if not reloading then
    -- Renoise releases the song just before it quits; a tool disabled or reloaded sees no release.
    if conns then conns:close_all(b.quitting and "quit" or "shutdown") end
    host.close_server()
    discovery.remove(b.discovery_path)
    b.discovery_path = nil
    log.write("info", "stopped")
  end
  log.flush()
end

-- (M5 m5-renoise: a render that ended with no callback, an audition's end; one function of the module's, not a closure a tick)
local function tick_listening()
  local env = listen_env()
  listening.check(env)
  audition.tick(env)
  sweep_scratch()
end

function M.tick()
  b.alive_ms = M.now_ms()
  unix_ms()
  if not b.cfg then
    if M.now_ms() >= b.retry_ms and configure() then listen() end
  elseif b.remake and M.now_ms() >= b.retry_ms then
    -- Descriptors held by refused connections can only be freed by closing the server (nothing else may close a
    -- client): Kumi gets a bye and reconnects.
    log.write("error", "making the server anew: " .. b.remake)
    b.remake = nil
    conns:close_all("overload")
    b.retry_ms = M.now_ms() + M.RETRY_MS
    listen()
  elseif not server_running() and M.now_ms() >= b.retry_ms then
    -- The only reason to remake the server: it says it isn't running. Remaking it drops every connection.
    log.write("error", "the server stopped running; listening again")
    b.retry_ms = M.now_ms() + M.RETRY_MS
    listen()
  end
  -- One failing step mustn't skip the others: connections still need their tick.
  local ok, err = xpcall(flush_events, debug.traceback)
  if not ok then M.log_error("events", err) end
  ok, err = xpcall(tick_listening, debug.traceback)
  if not ok then M.log_error("listening", err) end
  jobs:run()
  conns:tick()
  -- (the auditions whose time is up end: a note never hangs on)
  local ended, audition_err = pcall(actions.tick, M.now_ms())
  if not ended then M.log_error("auditions", audition_err) end
  b.ticks = (b.ticks or 0) + 1
  if b.ticks % 20 == 0 then log.flush() end
end

function M.on_accept(client)
  conns:accept(client)
end

-- (Every message also gives the reads in progress a slice if the timers haven't: see jobs.lua.)
function M.on_message(client, data)
  b.work_ms = M.now_ms()
  local ok, err = pcall(function()
    conns:receive(client, data)
    jobs:run_if_stale(M.STALE_JOBS_MS, M.STALE_SLICE_MS)
  end)
  b.work_ms = nil
  if not ok then error(err, 0) end
end

-- Only noted, whatever the error: it may be one client's (a reset, a timeout) and the server serves the others. Closing
-- the server inside Renoise's socket callback can upset it, and remaking it drops every connection, Kumi's included,
-- for anyone who can open a connection. The tick remakes it when it says it isn't running.
function M.on_socket_error(message)
  local text = tostring(message)
  if b.last_socket_error == text then
    b.socket_error_repeats = (b.socket_error_repeats or 0) + 1
    if b.socket_error_repeats % 100 ~= 0 then return end
    text = text .. " (" .. b.socket_error_repeats .. " times)"
  else
    b.last_socket_error, b.socket_error_repeats = text, 1
  end
  log.write("error", "socket error: " .. text .. " (server running: " .. tostring(server_running()) .. ")")
end

-- Called from the fast timer while jobs are pending.
function M.tick_jobs()
  b.alive_ms = M.now_ms()
  if jobs and jobs:pending() > 0 then jobs:run() end
end

-- M6: the digest's module, whose budgets the tests set small.
M.digest = digest

-- How many things the song is watched through (the tests count them to see that only what is new gets watched).
function M.watching()
  return observers and #observers.entries or 0
end

function M.on_new_document()
  b.quitting = false
  -- (M5 m5-renoise: what a job or an audition had of the song that went is nothing to put back)
  pcall(function()
    local env = listen_env()
    listening.song_closed(env)
    audition.halt(env, "another song")
  end)
  b.document = b.document + 1
  b.scratch_sweep_due = true
  -- Another song: its id is worked out afresh. (Starting with a file, Renoise opens its empty song and then the file
  -- without releasing the first, so the empty song's id would otherwise stay with the file.)
  b.memory.song_id, b.memory.born_unsaved = nil, nil
  if renoise.song() then status.note_document(b.memory, renoise.song()) end
  bump_epoch("a song was opened")
  attach(renoise.song())
  send_project()
end

function M.on_release_document()
  b.quitting = true
  pcall(function()
    local env = listen_env()
    listening.song_closed(env)
    audition.halt(env, "the song closed")
  end)
  observers:detach()
  b.memory.song_id, b.memory.born_unsaved = nil, nil
  conns:event("project", json.null)
end

function M.on_will_save()
  local song = renoise.song()
  if song then status.write_song_id(b.memory, song) end
end

function M.on_saved()
  send_project()
end

function M.on_reload_failed(err)
  log.write("error", "reload failed, keeping the running payload: " .. tostring(err))
end

return M
