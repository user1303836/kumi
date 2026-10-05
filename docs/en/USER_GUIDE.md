# User guide

English · [简体中文](../zh-CN/USER_GUIDE.md) · [日本語](../ja/USER_GUIDE.md)

The bridge is Kumi's connection to Ableton Live, and any MCP client can use it
on its own. It has two parts:

- a local MCP server, `@ableton-mcp/mcp-server`;
- a Remote Script, `AbletonMcpBridge`, that runs inside Live 12.

The server talks to the Remote Script over an authenticated loopback
connection. A client can then read the open Set and change it: each change is
previewed, applied, and undone when needed. A client can also play, record and
analyze audio.

This guide covers setup, configuration, the deployment policy, how changes work,
and every tool. If you use Kumi, `kumi bridge` does all of the setup for you;
see the [Kumi guide](KUMI_GUIDE.md).

## Install

The native server runs as an executable on macOS, Windows or Linux. Connecting to Live requires
macOS or Windows. Use the archive for your operating system and architecture; the analysis worker
must remain beside the server. No separate Node runtime is needed.

- **With Kumi:** close Live and run `kumi bridge`.
- **Standalone:** install the native tarball through `ableton-mcp-server lifecycle`, as described
  in [delivery](DELIVERY.md).
- **From source:** run `cargo build --release --locked -p ableton-mcp-server --bins` at the repository
  root. `target/release/ableton-mcp-server` starts with offline tools until given a configuration.

## Connect to Live

The lifecycle installer creates the owner-only secret and configuration, installs the Remote
Script, and keeps the receipt for upgrades and rollback. Use it on Windows too, so it applies the
required file permissions. Follow [delivery](DELIVERY.md).

Then open Live and select **AbletonMcpBridge** in **Settings → Link, Tempo & MIDI**. Check the
connection with:

```sh
/absolute/path/ableton-mcp-server diagnostics --config /absolute/path/bridge-config.json
```

Look for `"provenance": "real-live"` and `"readiness": { … "realLiveOperational": true }`.
Diagnostics can exit successfully while disconnected; read the report's readiness fields.

### The configuration file

`ableton-mcp-server setup` writes a version 2 file. The server, the Remote Script and
the lifecycle all read it:

```json
{
  "version": 2,
  "server": {
    "command": "/absolute/path/ableton-mcp-server",
    "args": ["--config", "/absolute/path/bridge-config.json"]
  },
  "bridge": {
    "host": "127.0.0.1",
    "port": 9765,
    "secretFile": "/absolute/path/bridge.secret",
    "timeoutMs": 5000,
    "realtimePort": 9766
  }
}
```

| Field | Rule |
| --- | --- |
| `server.command` | The native server's absolute executable path |
| `server.args` | `--config` and this file's own absolute path |
| `bridge.host` | `127.0.0.1` or `::1` |
| `bridge.port` | 1–65535; the Remote Script listens here |
| `bridge.secretFile` | Absolute path; owner-only, at least 32 characters |
| `bridge.timeoutMs` | 100–60,000 ms per request to Live (default 5,000) |
| `bridge.realtimePort` | Optional; must differ from `port`; see [realtime control](REALTIME_CONTROL.md) |
| `bridge.diagnostics` | Optional; written only by `ableton-mcp-server lifecycle install --enable-bridge-diagnostics` (see [operations](OPERATIONS.md)) |

Legacy version 2 configurations naming Node and `cli.js` remain readable during migration.
Unknown fields are refused. The file must be readable only by you. Running
`ableton-mcp-server setup` without bridge options writes a version 1 file. That file
only says how to start the server; passed to `--config`, it is refused.
`ableton-mcp-server migrate` converts older files (see [delivery](DELIVERY.md)).

## Add the bridge to an MCP client

Use the configuration's `server.command` and `server.args`. In the common
`mcpServers` format:

```json
{
  "mcpServers": {
    "ableton": {
      "command": "/absolute/path/ableton-mcp-server",
      "args": ["--config", "/absolute/path/bridge-config.json"],
      "env": { "ABLETON_MCP_TOOL_POLICY": "edit-no-audio" }
    }
  }
}
```

The server speaks MCP as JSON lines on stdin and stdout. It writes its own log
lines, prefixed `mcp-host:`, to stderr.

## Kumi's Live extension

On Live 12.4 and later, the bridge also connects to Kumi's Live extension when
the extension is running. That adds:

- offline renders;
- MIDI clips written straight into the Arrangement;
- clearing a stretch of the Arrangement;
- device copies;
- importing files into the project;
- the "Ask Kumi about this" right-click events.

The extension runs one of two ways:

- **Installed in Live's Extensions folder.** `kumi bridge` puts it there, and
  Live starts it when it opens.
- **Started by the bridge** through Live's own Extension Host, when Live's
  Developer Mode is on (Settings → Extensions).

The bridge looks for the extension every 10 seconds while Live is connected.
Its tools appear in `tools/list` once it answers; see the
[tool reference](#live-extension-tools). Everything else works without it.

| Variable | Effect |
| --- | --- |
| `ABLETON_MCP_EXTENSION=off` | Don't connect to the extension |
| `ABLETON_MCP_EXTENSION=external` | Connect to a running extension, but never start one |
| `ABLETON_MCP_EXTENSION_DIR` | Where a bridge-started extension keeps its endpoint, secret and renders (default: `live-extension` beside the configuration) |
| `ABLETON_MCP_LIVE_EXTENSIONS_DIR` | Live's Extensions folder, if not the default (`~/Library/Application Support/Ableton/Extensions`, `%LOCALAPPDATA%\Ableton\Extensions`) |

## Commands

| Command | Options |
| --- | --- |
| `ableton-mcp-server` | None, or exactly `--config PATH` |
| `ableton-mcp-server setup` | `--output PATH`, plus for version 2: `--bridge-port N`, `--secret-file PATH`, and optionally `--bridge-host`, `--bridge-timeout MS`, `--realtime-port N`. `--force` overwrites. |
| `ableton-mcp-server install-remote-script` | `--destination DIR`, `--config PATH`, `--dry-run`, `--force` |
| `ableton-mcp-server diagnostics` | None, or exactly `--config PATH`; prints a JSON report |
| `ableton-mcp-server lifecycle`, `ableton-mcp-server migrate` | See [delivery](DELIVERY.md) |

The first four exit with 2 for a bad option and 1 when they fail.

## Protocol

The server supports two MCP protocol versions. Use one per server process (a
`server/discover` may come before a `2025-11-25` `initialize`).

- **`2025-11-25`:** send `initialize`, then `notifications/initialized`. The
  server sends `notifications/tools/list_changed` and Live events (see
  [Events](#events)).
- **`2026-07-28`:** no handshake. Every request carries
  `io.modelcontextprotocol/protocolVersion` and
  `io.modelcontextprotocol/clientCapabilities` in `params._meta`;
  `server/discover` is optional:

  ```json
  {"jsonrpc":"2.0","id":1,"method":"server/discover","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}}}
  ```

  Results carry `resultType: "complete"`, and tool results carry their JSON as
  `structuredContent` too. Lists and resources are marked `ttlMs: 0`: read them
  again rather than caching them. This version has no push notifications. Use
  `live_observe_poll` instead of `live_subscribe`.

`tools/list` shows only the tools that the connected Live supports and the
deployment policy allows, right now. The list changes as Live connects,
disconnects or reconnects, and as the Live extension comes and goes. Under
`2025-11-25` the server announces each change with
`notifications/tools/list_changed`. `capabilities` and the
`ableton://capabilities` resource also list what is hidden, and why.

## Deployment policy

The deployment policy decides which tools a client can see and call. Set it in
the server's environment:

| Variable | Value |
| --- | --- |
| `ABLETON_MCP_TOOL_POLICY` | A profile: `read-only`, `edit-no-audio`, `performance` or `full` (default) |
| `ABLETON_MCP_TOOL_ALLOW` | Comma-separated tool names or `prefix*` patterns; only these, within the profile |
| `ABLETON_MCP_TOOL_DENY` | Comma-separated names or patterns that are never allowed; deny always wins |

| Profile | Allows |
| --- | --- |
| `read-only` | Local tools and reads; no changes |
| `edit-no-audio` | Reads plus edits: structure, MIDI, devices, mixer, automation, routing. Nothing that plays, records, captures, writes files or runs Python. |
| `performance` | Reads plus playback, views and selection, mixers, tempo, `live_undo` and `live_recovery_finalize` |
| `full` | Every class, including `python` |

Each tool has one class, listed in the [tool reference](#tool-reference):
`local`, `read`, `edit`, `performance`, `audio`, `filesystem`, `recording`,
`realtime`, `capture` or `python`. Where a class doesn't match what a tool
does:

- `live_render_offline` is `read`, though it writes a render file.
- The `.als` tools are `filesystem`, though they only read.
- `live_change` is `edit`, so `performance` doesn't include it.

The policy is checked again at every call. `live_undo` is refused for a change
whose tool the policy no longer allows. A malformed value stops the server at
start. `ableton-mcp-diagnostics` reports the policy in effect.

For a client you don't fully trust, start with `read-only` or
`edit-no-audio`. In `full`, deny `live_run_python`, which runs any Python
inside Live.

## How changes work

### Preview, apply, undo

A change takes three steps:

1. **Read** what you'll change (`live_discover`, `live_snapshot`), to get its
   ref.
2. **Preview** it with a `*_preview` tool. The preview changes nothing. It
   answers with what would change, a `transactionId`, a `confirmation` and
   `expiresAt`.
3. **Apply** it with the matching `*_apply` tool: the `transactionId`, the
   `confirmation`, and an `idempotencyKey` of your own (8–128 characters). The
   bridge checks on Live's thread that nothing changed since the preview,
   applies the change, and reads it back.

Sending the same apply again with the same key answers again
(`"idempotent": true`) and doesn't apply twice. Keep the `transactionId` to undo
the change with `live_undo` (`confirmation: "undo"`).

`live_change` does the preview and the apply in one call:
`{"tool": "live_mixer_preview", "args": {…}}`. It answers with the apply's
result (the preview's is under `preview`), so `live_undo` works as usual. It
refuses the changes that a person should see before they happen: auditions,
clip launches, launch buttons, captures, recording, realtime arming and Live's
dialogs.

### Confirmations and expiry

- Most previews hand out the confirmation `"apply"`.
- A scene audition and a clip launch hand out an unpredictable token, plus a
  separate one for stopping. A capture hands out an unpredictable token.
- A few tools take their own word: `"undo"`, `"backup"`, `"disarm"`,
  `"undo-in-live"`, `"redo-in-live"`, `"emergency-stop"`,
  `"emergency-stop-and-clean"` and `"finalize-recovery-record"`.

A preview expires after 10 minutes. Batch, MIDI clip and device-state previews
expire after 30 seconds, and a capture preview after 60. Preview again after it
expires.

Tools that play or record have an `outputSafety` object (`{"safe": true,
"provenance": "…"}`) in their schema. Only scene audition needs one. For the
others the bridge uses its own when the client gives none, but a client that
enforces the schema must still send it where the schema requires it.

### What the bridge adjusts

- A parameter value past its range goes to the nearest end. One between a
  stepped parameter's steps goes to the nearest step. Only a parameter Live
  greys out is refused.
- Tracks and scenes may share names, except a track a batch creates.
- A random MIDI transform with no `seed` derives one from the request, so the
  preview and the apply agree.

### When a change is refused

A refused call returns `isError: true` with `{"reason": "...", "remediation":
"..."}`. The reason is the bridge's or Live's own.

- "Nothing changed in Live": fix what the reason says, then preview again.
- "Live state changed since the preview": read again, then preview again.
- A timeout, a lost reply or a failed read-back leaves the change uncertain.
  Retry only the same apply with the same key; see [recovery](RECOVERY.md).

### Undo

`live_undo` takes a change back however the object has changed since: a
renamed track, a fader moved again, a device with clips added. It refuses when
the ref now holds a different object.

Some changes have no `live_undo`:

- Deletions, clearing a stretch of the Arrangement, `live_run_python`, and
  other changes whose preview says they're kept (a crop, a stored rack
  variation, a cleared pad). Live's own undo (`live_song_undo`) takes these
  back.
- Playback: launching, firing scenes, transport actions, launch buttons. Stop
  it instead.

`live_undo_step_begin` and `live_undo_step_end` group changes into one Cmd-Z in
Live. The step closes by itself after `timeoutMs` (two minutes by default),
when the connection goes, or when another step opens. Changes made through
the Live extension aren't grouped.

The server keeps the undo of applied changes: up to 1 GiB of records in all,
and 512 each for batch, MIDI clip and device-state changes. Past that, the
oldest applied change gives up its undo. `live_transaction_release` gives up
undos you won't use. Undo records live in the server's memory, and a restart
loses them.

[Live safety](LIVE_SAFETY.md) explains what the bridge guarantees, and which
tools work outside previews and applies.

## Tool reference

Every tool the server can offer. `tools/list` shows only those that the
connected Live supports and the policy allows. `name_preview/apply` stands for
the `name_preview` and `name_apply` pair; `/stop` adds `name_stop`. Class is
the [deployment policy](#deployment-policy) class.

### Status and offline tools

These work without Live, except `live_status`, which reports whether Live is
there.

| Tool | Class | What it does |
| --- | --- | --- |
| `server_status` | local | The server's version and whether a Live adapter is connected. |
| `capabilities` | local | The negotiated capabilities and which tools are executable, visible or denied by policy. |
| `live_status` | read | Live's connection: protocol, adapter, provenance (`real-live`), epoch, registry hash, capabilities and operations. Reconnects first if the bridge dropped. Always listed. |
| `plan_user_journey` | local | A plan for one of five guided journeys; changes nothing. See [worked examples](USER_JOURNEYS.md). |
| `audio_analyze` | local | Loudness (BS.1770-5 / EBU R128), true peak, spectrum, dynamics and clipping of float32 PCM you send. |
| `audio_compare_reference` | local | Compares your PCM with a reference: alignment, level match and differences. |
| `als_read/lint/diff` | filesystem | Read, lint or diff saved `.als` files inside an `allowedRoot` you name, without Live. |
| `live_project_snapshot_diff` | read | Compares two exported Set snapshots, without Live. |
| `live_library_search` | read | Searches Live's own library database (files, tags, plug-ins) in a folder you allow; read-only. |

### Reading the Set

Refs from these reads (`<epoch>:track:4` and so on) are what the change tools
take. A ref is valid until Live's epoch changes.

| Tool | Class | What it does |
| --- | --- | --- |
| `live_snapshot` | read | One bounded snapshot of the whole Set. |
| `live_discover` | read | Pages of one kind of object: `set`, `track`, `return-track`, `main-track`, `scene`, `clip-slot`, `session-clip`, `arrangement-clip`, `note`, `locator`, `device`, `parameter`, `selection`, `routing-choice`, `session-playback`. Clip slots, clips, notes, parameters and routing choices need a `parent` ref. Up to 8 filters, chosen `fields`, `limit`, `cursor`. |
| `live_song_state` | read | Song-level state: signature, swing, record and overdub modes, arm and solo modes, Link. |
| `live_performance_read` | read | CPU load, track meters and device latency, sampled once. |
| `live_note_read` | read | Notes of a MIDI clip by id, or the selected ones. |
| `live_key_estimate` | read | Ranked key candidates for a MIDI clip or a list of notes. |
| `live_project_info` | read | The saved Set's file, its referenced media and what is missing. |
| `live_project_snapshot_export` | read | A page of a privacy-filtered Set snapshot (`strict`, `collaboration` or `local`) to save and diff later. |
| `live_automation_read` | read | A Session clip's envelope for one parameter, and its value at a beat. |
| `live_arrangement_automation_read` | read | An Arrangement clip's envelope points for one parameter. |
| `live_take_lane_read` | read | A track's take lanes and their clips. |
| `live_comp_read` | read | Which take-lane segments make up a comped clip. |
| `live_warp_marker_read` | read | An audio clip's warp markers. |
| `live_device_read` | read | Every parameter name of a plug-in, or a Max for Live device's banks. |
| `live_clip_time_convert` | read | Converts between beats, sample frames and seconds in an audio clip. |
| `live_data_read` | read | Text stored in the Set or on a track under a key. |
| `live_browser_roots` | read | Live's Browser roots. |
| `live_browser_search` | read | Ranked search of Live's Browser by category and words. |
| `live_browser_inspect` | read | One Browser item by id: what it is and whether it can be loaded. |

### Following changes

| Tool | Class | What it does |
| --- | --- | --- |
| `live_subscribe` | read | Sends `notifications/live_event` as Live changes (see [Events](#events)). Legacy protocol only. |
| `live_unsubscribe` | read | Stops those notifications. |
| `live_observe_subscribe/poll/unsubscribe` | read | An observer you poll for changed topics (transport, selection, track, clip, device, parameter, groove, tuning, scene, meters, rack). Works in both protocol eras. |

### Tracks, scenes and structure

| Tool | Class | What it does |
| --- | --- | --- |
| `live_session_structure_preview/apply` | edit | Creates MIDI and audio tracks and scenes at the positions you give. Names may repeat. |
| `live_track_structure_preview/apply` | edit | Creates or deletes a return track; duplicates a track or a scene. |
| `live_scene_capture_preview/apply` | edit | Captures what's playing into a new scene. |
| `live_object_rename_preview/apply` | edit | Renames a track, scene, clip, device, locator or take lane. |
| `live_track_properties_preview/apply` | edit | A track's color (palette index 0–69). |
| `live_scene_preview/apply` | edit | A scene's color, tempo and time signature. |

### Deleting

Deletions are kept: `live_undo` can't bring them back, Live's own undo
(`live_song_undo`) can.

| Tool | Class | What it does |
| --- | --- | --- |
| `live_track_delete_preview/apply` | edit | Deletes an audio, MIDI or group track with its clips and devices; a group takes the tracks inside it. |
| `live_scene_delete_preview/apply` | edit | Deletes a scene and its clips. A Set keeps at least one scene. |
| `live_clip_delete_preview/apply` | edit | Deletes a Session or Arrangement clip. |
| `live_locator_delete_preview/apply` | edit | Deletes a locator. |
| `live_device_delete_preview/apply` | edit | Deletes a device. |

### Session clips, notes and clip automation

| Tool | Class | What it does |
| --- | --- | --- |
| `live_midi_clip_preview/apply` | edit | Creates a MIDI clip with its notes in an empty Session slot. |
| `live_note_update_preview/apply` | edit | Changes notes by id: pitch, start, length, velocity, mute, probability, velocity deviation, release velocity. |
| `live_note_delete_preview/apply` | edit | Deletes notes by id. |
| `live_note_edit_preview/apply` | edit | Quantizes or duplicates notes, selects notes, or deletes the notes in a pitch and time range. |
| `live_midi_transform_preview/apply` | edit | Transforms and generators: transpose, scale, quantize, swing, humanize, arpeggiate, Euclidean rhythms, chord progressions, drum patterns, basslines, motif inversion and more. Random ones take a `seed`, or derive one from the request. Generators write to a copy in an empty slot by default. |
| `live_capture_midi_preview/apply` | edit | Live's Capture MIDI. |
| `live_clip_properties_preview/apply` | edit | Clip mute, color, MIDI loop, launch mode and quantization, legato, RAM mode, velocity amount, groove. |
| `live_clip_action_preview/apply` | edit | Crop, duplicate the loop or a region, scrub, move the playing position. |
| `live_clip_duplicate_preview/apply` | edit | Copies a Session clip to another slot or into the Arrangement. |
| `live_clip_move_preview/apply` | edit | Moves an Arrangement clip, or a Session clip to another slot. |
| `live_automation_preview/apply` | edit | Session clip envelopes: create or delete, insert or delete points, draw a step, clear all envelopes. |

### Arrangement

| Tool | Class | What it does |
| --- | --- | --- |
| `live_arrangement_section_preview/apply` | edit | Adds two named locators around a section. |
| `live_arrangement_clip_preview/apply` | edit | Creates an empty MIDI clip, or an audio clip from `filePath` (passed to Live as given), in the Arrangement. |
| `live_locator_jump_preview/apply` | performance | Moves the playhead to the next, previous or a given locator. |

### Audio clips and files

Audio import, Simpler and drum pad loads take a file path and an `allowedRoot`
folder the file must be inside. The bridge checks the file and gives Live a copy
kept in a managed folder (see [Live safety](LIVE_SAFETY.md)).

| Tool | Class | What it does |
| --- | --- | --- |
| `live_audio_clip_preview/apply` | audio | An audio clip's gain, pitch, loop, warp and fades, as far as the clip offers them. |
| `live_warp_marker_preview/apply` | audio | Adds, moves or deletes a warp marker, by beat time. |
| `live_audio_import_preview/apply` | filesystem | Puts an audio file into an empty Session slot or a take lane. MIDI files are refused. |
| `live_simpler_preview/apply` | filesystem | Replaces a Simpler's sample. |
| `live_project_backup_preview/apply` | filesystem | A verified copy of the saved Set beside it. The preview takes `confirmation: "backup"` and an `allowedRoot` holding the Set. |

### Devices, racks and the Browser

| Tool | Class | What it does |
| --- | --- | --- |
| `live_browser_load_preview/apply` | edit | Loads a Browser item after a track's devices, or into a rack's chain (`chainRef`). A second instrument on a track is refused. |
| `live_device_preview/apply` | edit | Inserts a native device by name (a Simpler can load a sample at once), turns a device on or off, or moves it. |
| `live_device_parameter_preview/apply` | edit | Sets one parameter, or up to 10,000 of one device at once with `values`. A value past the range is held at its end, and one between steps goes to the nearest step. |
| `live_device_state_save` | filesystem | Saves a device's or rack's parameter values to a JSON file in a folder you name. |
| `live_device_state_recall_preview/apply` | read, edit | Recalls a saved state onto a device, or morphs between two states. |
| `live_device_advanced_preview/apply` | edit | Parameter banks, re-enable automation, A/B save, insert into a chain, move to another track or chain. |
| `live_device_specialized_preview/apply` | edit | Drift (with its modulation matrix), Drum Cell, EQ Eight, Hybrid Reverb, Meld, plug-in presets, Simpler sample settings, Wavetable. |
| `live_device_edit_preview/apply` | edit | Settings that aren't parameters (Roar, Shifter, Spectral Resonator, Hybrid Reverb, CC Control, Simpler), Simpler slices and warping, Wavetable modulation amounts. |
| `live_device_io_preview/apply` | edit | A device's own input or output routing, or a compressor's sidechain source. |
| `live_chain_preview/apply` | edit | A rack chain's color, mute and solo. |
| `live_chain_mixer_preview/apply` | edit | A rack chain's volume, pan, sends and activator. |
| `live_rack_preview/apply` | edit | Macro count and variations; add, remove or randomize macros; insert a chain; copy a pad. |
| `live_rack_view_preview/apply` | edit | Which chain or pad a rack shows. |
| `live_drum_pad_preview/apply` | edit | Pad note and solo, clear a pad, or load samples onto pads (one or a whole rack) as Simpler or Drum Sampler. |
| `live_looper_preview/apply` | edit | Looper actions and settings. |

### Mixing, routing and batches

| Tool | Class | What it does |
| --- | --- | --- |
| `live_mixer_preview/apply` | edit | Volume, pan, mute, solo, cue and sends. |
| `live_mixer_extended_preview/apply` | edit | Track activator, crossfader and its assignment, panning mode, split stereo. |
| `live_routing_preview/apply` | edit | Input and output routing, arm and monitoring. Routes that would feed back are refused. |
| `live_batch_preview/apply` | edit | Up to 32 mixer, parameter, clip, rename, new-track and arm operations as one change with one undo. A new track's name must not be taken. |

### Tempo, song settings, tuning and groove

| Tool | Class | What it does |
| --- | --- | --- |
| `live_tempo_preview/apply` | edit | Tempo, 20–999 BPM. |
| `live_song_settings_preview/apply` | edit | Time signature, swing, launch and record quantization, select on launch. |
| `live_tuning_preview/apply` | edit | Tuning system and scale. |
| `live_groove_preview/apply` | edit | Global groove amount and the grooves in the pool. |

### Playback

These can be heard. Most can't be undone; stopping is the way back.

| Tool | Class | What it does |
| --- | --- | --- |
| `live_transport_preview/apply` | performance | Song position, loop, punch and metronome (undoable). |
| `live_transport_action_preview/apply` | performance | Start, continue, stop, play selection, stop all clips, back to Arrangement, scrub, tap tempo, nudge, jump, trigger Session record. |
| `live_clip_launch_preview/apply/stop` | performance | Launches one clip, while the Set plays or not, and stops that clip again. |
| `live_scene_fire_preview/apply` | performance | Fires a scene. |
| `live_fire_button_preview/apply` | performance | Presses or releases a clip's, slot's or scene's launch button, as a controller does. |
| `live_session_audition_preview/apply/stop` | performance | A guarded scene audition: needs the Set's name, output-safety evidence, and a stopped Set with nothing armed or monitoring its input. |
| `live_session_emergency_stop` | performance | Stops the Session clips, transport and recording you just observed; needs no transaction and works after a restart. |
| `live_browser_preview` | performance | Plays a Browser item's preview, as clicking it in Live does. |
| `live_browser_preview_stop` | performance | Stops that preview. |

### Recording and capture

| Tool | Class | What it does |
| --- | --- | --- |
| `live_recording_preview/apply` | recording | Starts or stops Session or Arrangement recording. The destination must be armed; Live also records onto other armed tracks. |
| `live_audio_capture_preview/apply` | capture | Records 1–9 seconds of a clip through Resampling, analyzes it and deletes the recording. Real Live only; see [audio intelligence](AUDIO_INTELLIGENCE.md). |
| `live_audio_capture_status` | read | Where a capture is in its lifecycle. |
| `live_audio_capture_emergency_stop` | capture | Stops and cleans up a capture after a failure or restart. |
| `audio_diagnose_live_context` | read | Links measurements of PCM you send to one track's current devices and mixer. |

### Views and Live's interface

| Tool | Class | What it does |
| --- | --- | --- |
| `live_view_preview/apply` | performance | Shows Session or Arrangement; Arrangement zoom, scroll and follow. |
| `live_track_view_preview/apply` | performance | Track fold, device insert mode, rack chains shown, select the instrument. |
| `live_selection_preview/apply` | performance | Selects a track, scene, slot, clip, device, parameter or chain; draw mode. |
| `live_clip_view_preview/apply` | performance | A clip's grid, envelopes and loop display. |
| `live_device_view_preview/apply` | performance | Folds or unfolds a device. |
| `live_application_dialog_preview/apply` | edit | Reads Live's open dialog and presses one of its buttons. |
| `live_message` | performance | Shows a message in Live's status bar, or in a dialog with `modal: true`. |

### Undo and bookkeeping

| Tool | Class | What it does |
| --- | --- | --- |
| `live_undo` | edit | Undoes one applied change (`confirmation: "undo"`). |
| `live_change` | edit | Runs a preview and its apply in one call: `tool` (a `*_preview`), `args`, optional `idempotencyKey`. Refuses auditions, clip launches, launch buttons, captures, recording, realtime arming and dialogs. |
| `live_undo_step_begin/end` | edit | Groups the changes between them into one step of Live's own undo. |
| `live_song_undo/redo` | edit | Live's own undo and redo, once (`undo-in-live`, `redo-in-live`). For deletions and for edits made in Live. |
| `live_transaction_release` | edit | Gives up the undo of up to 64 applied changes you won't undo. |
| `live_recovery_finalize` | edit | Closes an uncertain change's record after you've checked Live by hand. See [recovery](RECOVERY.md). |

### Realtime control

A short-lived UDP channel for fast parameter moves. See
[realtime control](REALTIME_CONTROL.md).

| Tool | Class | What it does |
| --- | --- | --- |
| `live_realtime_arm_preview/apply` | realtime | Opens the channel for given parameters and returns its token. |
| `live_realtime_disarm` | realtime | Closes it. |
| `live_realtime_stats` | realtime | Packets accepted, applied and dropped. |

### Python and stored text

| Tool | Class | What it does |
| --- | --- | --- |
| `live_run_python` | python | Runs Python on Live's main thread (`code`, `mode` `exec` or `eval`, optional `ref`, `timeoutMs` up to 30,000). No preview and no `live_undo`; one step in Live's undo. See [Live safety](LIVE_SAFETY.md). |
| `live_data_preview/apply` | edit | Stores text in the Set or on a track under a `kumi.` key. |

### Live extension tools

Listed while the bridge is connected to Kumi's Live extension (see
[Kumi's Live extension](#kumis-live-extension)).

| Tool | Class | What it does |
| --- | --- | --- |
| `live_render_offline` | read | Renders an audio track's clips between two beats to a file, before its devices, without playing. |
| `live_project_import` | filesystem | Copies an audio file into the Set's project folder. |
| `live_arrangement_midi_clip_preview/apply` | edit | Writes one or more MIDI clips with notes into the Arrangement. |
| `live_clip_clear_range_preview/apply` | edit | Clears a stretch of a track's Arrangement, cutting clips at its edges. Kept, like a deletion. |
| `live_device_duplicate_preview/apply` | edit | Copies a device, with its settings, right after itself. |

### Willington tools

Listed only with the separately installed Willington provider; see
[Willington](WILLINGTON_INTEGRATION.md).

| Tool | Class | What it does |
| --- | --- | --- |
| `live_willington_device_preview/apply` | edit | Rack macro and variation names, macro mappings and chain zones. |
| `live_follow_actions_preview/apply` | edit | A Session clip's Follow Actions, with playback stopped. |

## Events

Under `2025-11-25`, `live_subscribe` (optionally with a list of `types`) makes
the server send `notifications/live_event` as Live changes:

| Type | When |
| --- | --- |
| `transport` | Playing or recording starts or stops |
| `object` | The track or scene list changes |
| `selection` | The selection changes |
| `name` | A track, scene or clip is renamed or recolored |
| `mixer` | A track's mute, solo, arm, volume, pan or send changes |
| `parameter` | A parameter of the selected device changes |
| `structure` | Tracks, scenes, locators, or a track's devices or clips change |
| `reset` | Read Live again: what you hold may be stale |

Each event has `epoch`, `sequence`, `type`, `channel` (`remote-script` or
`extension`, each numbered on its own) and a `payload`. A `pointed` event comes
from the extension's "Ask Kumi about this" without a subscription. If events
pile up past 65,536, the server drops the rest and sends
`notifications/live_event_overflow` with `resnapshot: true`. After that, a
`reset` or a gap in `sequence`, read the Set again.

Under `2026-07-28`, use `live_observe_subscribe` and `live_observe_poll`.

## Resources and prompts

| Resource | Content |
| --- | --- |
| `ableton://capabilities` | The negotiated capabilities, and which tools are available, visible or denied by policy, with their classes (JSON) |
| `ableton://safety` | A short safety summary (Markdown) |
| `ableton://journeys` | The five guided journeys and what this Live supports of each (JSON) |
| `ableton://live-workflow` | A safe tempo change, step by step (Markdown) |
| `ableton://max-extension` | The packet format an operator-built Max patch could use for realtime control; no Max device ships (JSON) |

| Prompt | Arguments |
| --- | --- |
| `analyze_audio` | `sampleRate`, optional `channels` |
| `change_tempo_safely` | None |
| `create_beat_or_song`, `sequence_advanced_drums`, `design_owned_sound`, `compare_reference_mix`, `diagnose_performance_setup` | `traits`, optional `experienceLevel` (`beginner` or `advanced`) and `bars` (`"1"` to `"16"`, as a string) |

Prompts and resources only describe; they authorize nothing. The journey
prompts are explained in [worked examples](USER_JOURNEYS.md).

## Environment variables

| Variable | Effect |
| --- | --- |
| `ABLETON_MCP_TOOL_POLICY`, `ABLETON_MCP_TOOL_ALLOW`, `ABLETON_MCP_TOOL_DENY` | The [deployment policy](#deployment-policy) |
| `ABLETON_MCP_EXTENSION`, `ABLETON_MCP_EXTENSION_DIR`, `ABLETON_MCP_LIVE_EXTENSIONS_DIR` | [Kumi's Live extension](#kumis-live-extension) |
| `ABLETON_MCP_IMPORT_STAGING_DIR` | Where imported files are copied for Live (absolute path; default `~/.config/ableton-mcp/import-staging`, or `%APPDATA%\ableton-mcp\import-staging` on Windows) |
| `ABLETON_MCP_USER_LIBRARY` | Live's User Library, for samples loaded into Drum Sampler (the bridge writes a carrier preset into its `Kumi` folder) |
| `ABLETON_MCP_LIVE_RESOURCES` | Live's Resources folder, where the default Drum Sampler preset is |
