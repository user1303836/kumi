# Ableton Live safety

English · [简体中文](../zh-CN/LIVE_SAFETY.md) · [日本語](../ja/LIVE_SAFETY.md)

What the bridge guarantees when it reads and changes your Live Set, and what it
doesn't. The bridge is the MCP server in `crates/ableton-mcp-server` together
with its Remote Script inside Live. Kumi drives it, and so can any MCP client
([user guide](USER_GUIDE.md)). Read this before you point a client at a Set you
care about.

## Trust boundary

- Without `--config`, the bridge uses its unavailable adapter: it never reads or
  changes Live. Nothing a client sends can choose another adapter.
- With a configuration, the bridge reaches Live only over loopback (`127.0.0.1`
  or `::1`) and signs every request with an owner-only secret of at least 32
  characters. It trusts your user account: a program running as you can read
  that secret and drive Live. Nothing else on the network can.
- The deployment policy decides which tools a client can see and call:
  `read-only`, `edit-no-audio`, `performance` or `full` (the default), narrowed
  by `ABLETON_MCP_TOOL_ALLOW` and `ABLETON_MCP_TOOL_DENY`. The bridge checks it
  on every call, including apply, undo and emergency stop. Only `full` includes
  `live_run_python`; `ABLETON_MCP_TOOL_DENY=live_run_python` turns it off. See
  the [user guide](USER_GUIDE.md) for the profiles.
- Kumi starts the bridge with `full` and an allow list of exactly the tools it
  uses. Kumi's own code runs the previews and applies and keeps their
  confirmations; the model asks for changes by name. The model can also run
  Python inside Live (see [Python in Live](#python-in-live)).

## How a change reaches Live

Most changes are a preview, then an apply:

1. The `*_preview` tool reads fresh state, checks the request and returns a
   transaction that says what will change. It changes nothing, and expires
   after 10 minutes.
2. The `*_apply` tool takes the transaction id, its confirmation (the literal
   `apply` for most tools; an unpredictable token for scene audition, clip
   launch and audio capture) and an idempotency key.
3. The bridge sends the change to the Remote Script as one request. On Live's
   main thread, the Remote Script checks the fences the change carries (the
   identities of what it names and where they sit: a device, its track and its
   sibling devices; a clip, its slot and its scene), applies it, and records the
   result under the idempotency key. The bridge then reads the result back.

`live_change` runs a preview and its apply in one call. It won't do that for
the changes you decide on after seeing their preview: auditions, clip launches,
captures, recording, realtime arming and Live's dialogs.

For mixer, chain, routing, clip, scene, track and song settings, tuning, views,
renames, parameter and device edits, Simpler samples, Browser loads, data and
deletions, the preview also fetches a digest of what the change depends on: the
rows it names, the song's transport state and, except for parameter, mixer and
rename changes, the Set's track and scene structure. The apply carries the
digest, and the Remote Script refuses with "Live state changed since the
preview" if any of it differs. The digest covers only the first step of a
transaction and lasts 10 minutes. Changes that depend on what's playing
(transport, launches, capture, recording) carry none, because playback moves
between a preview and its apply.

How a change ends:

- **Refused, nothing changed.** A refusal that ends "nothing changed" was made
  before anything ran in Live. Fix what the reason says and preview again.
- **Applied.** A retry with the same idempotency key returns the recorded result
  instead of applying again. The Remote Script keeps these results while it
  runs; an explicit reconnect clears them.
- **Uncertain.** A timeout, a lost reply or a failed readback leaves the change
  uncertain. Don't retry it with a new preview or a new key. Retry the same
  apply with the same key while the same Live session is connected, which
  returns what happened, or check Live and recover by hand
  ([recovery](RECOVERY.md)). If a same-key retry is refused, only the retry is
  known not to have run, so the change stays uncertain.

References belong to one Live session (an epoch). After Live restarts or the
bridge reconnects, earlier references, transactions and undo no longer apply.

Other rules every change follows:

- **Values fit the parameter.** A value past a parameter's range goes to the
  nearest end, and one between a stepped parameter's steps goes to the nearest
  step. A switched-off device's knobs and parameters Live doesn't automate can
  be set; only a parameter Live greys out is refused. The realtime plane is
  stricter (see [realtime control](REALTIME_CONTROL.md)).
- **Names can repeat.** Tracks and scenes may share a name; changes find objects
  by identity, not by name.
- **Reads stay short.** A discovery page or snapshot window stops after about
  30 ms of work on Live's thread and returns a cursor for the rest, so no read
  holds Live's interface however big the Set.

## Undo

- `live_undo` takes back one applied transaction, within the same Live session.
- Undo follows the object, not its contents. A track, scene, return, duplicate,
  device or clip a change made is removed even after it was renamed or got
  clips or devices; a parameter, fader or tempo goes back to its value before
  the change, however it moved since. Undo is refused when the reference now
  holds a different object, or the object is gone.
- Two exceptions follow the contents. A Session MIDI clip made by
  `live_midi_clip_apply` is removed only while its name, length and notes are
  as the change left them. An Arrangement MIDI clip written through Kumi's Live
  extension stays once its notes, name or extent changed. Delete such a clip
  yourself if it should go.
- An undo refused by its checks before it touched Live leaves the change
  applied, and the next undo checks again.
- Deletions and momentary actions (those in
  [outside the transaction model](#outside-the-transaction-model), and rack,
  Looper and some view actions) have no transaction undo, and `live_undo` says
  so. Live's own Cmd-Z still takes back a deletion. The delete tools
  (`live_clip_delete_*`, `live_scene_delete_*`, `live_track_delete_*`,
  `live_locator_delete_*`, `live_device_delete_*`) fence the object and where it
  lives by identity, so they never delete a stand-in.
- MIDI clip, batch and device-state transactions keep up to 512 records each.
  Past that, the oldest applied one gives up its undo.

A plan can be one step in Live's own undo: `live_undo_step_begin` opens a step
and `live_undo_step_end` closes it, and everything the Remote Script changes in
between is one Cmd-Z. The Remote Script owns the open step and closes it when
the connection that opened it closes, when its time runs out, on reconnect and
at shutdown. Your own edit in Live while it's open becomes a step of its own.
Changes through the Live extension aren't grouped into it.
`live_song_undo` and `live_song_redo` press Live's own undo and redo, for what
was done in Live; they are not a transaction's undo.

## Outside the transaction model

These tools have no transaction undo. Know what each does before you allow it.

| Tool | What it does | What's checked, and how it ends |
| --- | --- | --- |
| `live_run_python` | Runs Python inside Live (below) | Nothing beforehand; one Live undo step |
| `live_transport_action_preview/apply` | Start, continue, play selection, scrub, tap tempo, nudge, jump, trigger Session record and more | Fenced on the Set and its playback state. `trigger-session-record` starts Session recording without the recording checks below. |
| `live_scene_fire_preview/apply` | Fires one scene as Live's selected-scene launch does | Fenced on the scene, the scene list, and whether it's triggered or the transport plays; an empty scene is refused |
| `live_fire_button_preview/apply` | Presses or lets go of a clip, slot or scene launch button | Held until let go, until the connection closes, or for 30 s |
| `live_browser_preview` | Plays a Browser item's preview | `live_browser_preview_stop` with its `previewId` stops it |
| `live_message` | Shows a message in Live's status bar or a dialog | Changes nothing in the Set |
| `live_song_undo`, `live_song_redo` | Live's own undo and redo | Whatever Live's history holds |

### Python in Live

`live_run_python` runs Python on Live's main thread for what the typed tools
don't cover. Live waits while a script runs.

- Arguments: `code` (up to 64 KiB), `mode` (`exec`, the default, returns what
  the code assigns to `result`; `eval` returns an expression's value),
  `timeoutMs` (1–30,000, default 5,000) and an optional `ref`.
- Names available to the code: `Live`, `song`, `app`, `obj` (the object `ref`
  names) and `bridge` (the Remote Script's object mapper, with all its
  internals).
- It returns `{ok, result, stdout, error}`. Live objects in the result come back
  as `{ref, type, name}`, with refs other tools accept. An error carries its
  type, message and traceback.
- Each run is one Live undo step ("Kumi: Python"), or joins a step that's
  already open, so Cmd-Z in Live takes it back.
- There is no preview, digest, idempotency key, transaction undo or HISTORY
  entry. After a timeout or a lost reply, what the script did is unknown: read
  the Set again.
- The timeout interrupts Python code; a long call into Live is checked only when
  it returns.
- A script can do anything Live's Python API allows, including play, record and
  delete. Deny the tool through the deployment policy when a client shouldn't
  have it.

Kumi offers it to its model as `run_python` and forgets all its references to
Live after each run.

## Audible and recording actions

Transport actions, scene fire and fire buttons are in the table above.
Output-safety evidence (`outputSafety`) is optional for every tool except scene
audition: when a client gives none, the bridge supplies its own. The bridge
can't hear your speakers, so keep monitoring at a safe level.

| Action | Tools | Checked before it runs | Stopping it |
| --- | --- | --- | --- |
| Clip launch | `live_clip_launch_preview/apply` | The track, scene, slot and clip identities from the preview. It launches whatever is playing or recording, as pressing the slot in Live does. | `live_clip_launch_stop` stops the clip's track, only while that clip is the track's only playing target |
| Scene audition | `live_session_audition_preview/apply/stop` | The exact Set name; transport stopped, nothing recording or playing, no armed or input-monitored track, launch quantization not None; caller output-safety evidence. On Live's thread it rechecks the Set, the scene, the playback revision and every target. | `live_session_audition_stop`, which stops only its own targets |
| Recording | `live_recording_preview/apply` | An armed destination track (`destinationTrackRef`), plus up to 1,024 more armed tracks to record with it (`alsoTrackRefs`). Other tracks may be armed too; Live records onto every armed track. The Session and Arrangement record states and the destination's identity are rechecked on Live's thread. | Preview and apply with action `stop`, or emergency stop |
| Audio capture | `live_audio_capture_*` | See [audio intelligence](AUDIO_INTELLIGENCE.md#live-capture) | A 10-second watchdog in Live, and its own emergency stop |
| Realtime | `live_realtime_*` | See [realtime control](REALTIME_CONTROL.md) | `live_realtime_disarm` |

`live_session_emergency_stop` stops Session clips, the transport, Session
Record and Arrangement Record at once. It takes the active targets and the
recording mode (`stopped`, `session`, `arrangement` or `both`) from fresh
playback discovery and refuses if Live has moved on since. It belongs to no
transaction, so it works after the host restarts.

No read-only tool starts playback or recording.

## Files offered to Live

When a tool loads an audio file (audio import, project import, Simpler, drum
pads, sample loading), the bridge copies the verified file, read-only, into a
managed folder and offers Live only that copy: `~/.config/ableton-mcp/import-staging`
on macOS and Linux, `%APPDATA%\ableton-mcp\import-staging` on Windows, or an
absolute path in `ABLETON_MCP_IMPORT_STAGING_DIR`. The folder is owner-only.
The file can't be swapped between the check and the load.

Live plays imported audio from where it is, so after a successful apply the
copy is the clip's or the Simpler's sample until you collect the Set's files or
delete the clip. The bridge removes a copy only when nothing uses it (a failed
or refused apply, an expired or finalized transaction, an undo after the clip is
gone), never on success or at shutdown. Delete old copies by hand once the clips
that use them are gone.

## The bridge and the Remote Script

Every request to the Remote Script is HMAC-signed and tied to one Remote Script
session and one connection, with a strictly increasing sequence and a deadline.
Live's main thread reads the socket and runs every request itself, a little in
each display tick, so nothing touches Live from another thread. On Windows,
owner-only means an access list with a single entry for you. The Remote Script
can write a small diagnostics file when you ask for one at install.
`remote-script/README.md` has the details:
[the Remote Script README](../../remote-script/README.md).

## What's been checked on real Live

Tests, the simulator and the fake-Live mapper check the bridge's contracts, not
Live's behavior. Real-Live evidence lives in `docs/evidence`, each file with
its date, Live version and bridge version, indexed in the
[implementation status](IMPLEMENTATION_STATUS.md). The newest runs are on Live
12.4.15 betas on macOS. The phase files from July 2026 (Live 12.4.5b8, bridge
0.1.0) predate the current bridge. No Windows Live run is tracked.

If what Live shows disagrees with what the bridge reports, stop the client, keep
the evidence, use emergency stop if something is playing, and treat it as a
bug.
