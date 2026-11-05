# Logic Pro bridge contract (M2, M3 and Loop Range preview)

What `logic-kumi` answers over `kumi-daw/1`, and which macOS channel each answer comes from. The
wire, handshake, discovery files and lifecycle are daw-bridge's; this page is only Logic's shapes.
The original shapes were measured in the lab of 2026-10-18 (`docs/evidence/logic-lab-2026-10-18.md`)
and, for the rows, on the Scratch project in Logic Pro 12.3.1 during M2. The host replays the recorded JSON in `daws/own/daw-mcp-server/tests/vectors/logic-reads.json`;
the native fixture recordings live under `daws/logic/logic-kumi/tests/fixtures`; where the two
differ, the vectors are what Logic shows and this page says why.

M2 exposes `ping`, `status`, `focus`, `read`, `snapshot`, and `focus_body`.
M3 adds `change_preview`, `change_apply`, `change_restore`, `transport_play`,
`transport_stop`, `cursor_set`, `undo_step_begin`, and `undo_step_end`. Kumi offers
16 change kinds and by-value undo, with play, stop and cursor actions. Listening,
goals, scripts, general UI tools and catch-up are not enabled. The historical lab record is source evidence, not a
new acceptance result for this build. The `accept_logic` example reads by default.
Its automatic M3 checks require `--changes --scratch-confirmed` and a disposable
project; a failed undo can leave edits behind. This port still needs a current
real-Logic acceptance run.

Normal launch does not create MIDI ports. The explicit `logic-kumi --mcu` option
can install a Mackie Control surface in Logic; use it only for a deliberate lab
run and check Logic's Control Surfaces setup afterwards. The write, key, menu and
panel-service helper operations below are used only where declared: M3 changes
may activate Logic and drive its controls; the six reading methods preserve the
M2 observation restrictions.

## Channels

| Channel | Used for | Rule |
| --- | --- | --- |
| Accessibility reads | every row, focus and body | poll; never read a pop-up button's children; prefer `AXMainWindow` and the Window menu over `AXWindows`; address the project's window by title when one of Logic's own windows (the Control Surface Setup) is main; a pane a read needs (the Mixer, the List Editors) is opened for it and closed after |
| Accessibility writes | values with a text field or slider, buttons, rows | set, then `AXConfirm`; read back before answering |
| Key commands | actions with a binding | activate Logic first; post modifiers as their own events |
| Menu items | actions without a binding | activate Logic first; press by index after matching the title |
| Mackie Control | selection, transport and level events; the LCD names; selecting a track | answer only as a Mackie Control (device 0x14), and the version request; select from the Pan view, where the LCD names the strips |
| Panel service | open and save panels | post keys to `com.apple.appkit.xpc.openAndSavePanelService`; set the go-to-folder field by AX |
| Process tap | listening | M5 |

Reading may temporarily open and restore the Mixer or List Editors tabs. It does
not activate Logic, set project values, select regions, post keys or open MIDI
ports. The new-project chooser (`AXIdentifier=newProjectDialog`) is busy even
when macOS presents it as a titled main window.

## Identity

- `pid`: Logic's; `resource`: Logic's bundle identifier (`com.apple.logic10`, or `com.apple.mobilelogic` for the Creator Studio build).
- `project.id`: no UUID exists in every project, so the id is the 32-hex SHA-256 prefix of the saved package path; unsaved projects carry an `identity` from the window title.
- `status` is flat, as REAPER's: `{project, busy, permission}`. `busy` is `false` when free, else `modal` (the main or focused window is a dialog or a sheet is open), `away` (Logic reports no window, its Space not being the current one) or `loading` (the project window has no title and no document yet); the last project is kept through those.
- `status.permission`: `null`, or `"accessibility"` while the helper is not trusted; reads then answer `failed`, never `busy`. The identity (daw, bridge, dawVersion, pid, resource, project) is in the welcome, as for every own bridge; the discovery file is `logic-<pid>-<key>.json` with the same fields.

## Rows (`read`)

| Kind | Fields | Source |
| --- | --- | --- |
| `project` | id, identity, path, tempo, timeSignature, keySignature, sampleRate, trackCount | window document and the control bar's LCD; `Alternatives/000/MetaData.plist` for the sample rate when readable: one asynchronous probe, no symlinks, a regular XML plist at most 64 KiB; missing, blocked, binary or malformed metadata gives `null` |
| `track` | ref, index, name, kind (audio, instrument, external, aux, output), mute, solo, armed, monitoring, volumeDb, pan, color (always `null`: the header shows no colour), output, inputs, selected | track headers (name, mute, solo, armed, monitoring, volume, pan, selected); the Mixer's strips for the kind, output and inputs, read when needed and kept for five seconds |
| `region` | ref, trackRef, name, startBeats, endBeats, kind (audio, midi), selected, looped | the Tracks area's layout items and their help text ("Region starts at 1 bar  and ends at 5 bars , MIDI region…") |
| `note` | ref, regionRef, position (bar, beat, division, tick), pitch, velocity, lengthTicks, channel, selected? | the Event List of the region, which shows the selected region's events: the requested region must already be the sole selected region; otherwise the read is refused without changing selection |
| `plugin` | ref, trackRef, slot, name, bypassed, kind (instrument, effect) | the strip's slots, the instrument's first; the strip lists its slots bottom-up, the rows count them from the top |
| `send` | ref, trackRef, slot, destination, level | the strip's send slots |
| `marker` | ref, position, name, length (beats), text | the Marker List |
| `tempo` | bpm, signatures (position, time, key) | the Tempo List and the Signature List |

Logic's own words are kept: the key signature as the LCD shows it ("C Major"), an output as the
slot names it ("Stereo Output"), a plug-in as its slot names it. Refs are positional with an epoch
(`<epoch>:<kind>:<n>`), as Renoise's: tracks by their place in the Tracks area, regions by their
place through the project (track by track), markers by their place in the list, notes under their
region (`1:note:1.3`), plug-ins and sends under their track (`1:plugin:1.1`). The epoch moves on
every track or region change the bridge observes between polls (a header appeared, vanished, was
renamed or moved; a region added, removed or moved); notes, plug-ins, sends, markers and tempo
don't move it. A ref, parent, around or cursor from an earlier epoch is refused as `stale`,
whatever it was for. Paging: `limit` (50, at most 200), `cursor` `<epoch>.<n>`, `nextCursor`,
`total` and `epoch` in every answer; `ref` alone reads one row; `parent` filters; `note` needs its
region as `parent`.

## Focus and bodies

- `focus`: `{epoch, view, editor, selection, playhead, transport, changed, ageMs}`, answered from a cache the run loop refreshes every 150 ms with four small reads (the control bar, the track headers, the regions, the List Editors' tabs), never from a walk of the window. `view` is `tracks`; `editor` is `eventList` when the List Editors show the Event tab, `mixer` when the Mixer pane is open, `pianoRoll` or `stepSequencer` when the editors pane shows one, else `null`. `selection` carries `trackRef` and `regionRef` (`null` when none), and beside them the selected track's `track: {name, kind}` and the selected region's `regionName`, which the runtime reads. `transport` is `{playing, recording, cycle, tempo, beat, beatsPerBar}`; `busy` is added, as the status's code, while Logic can't take work.
- `focus_body`: `tree` (the strip: name, kind, volumeDb, pan, mute, solo, armed, inputs, output, inserts with `pluginRef`, sends with `sendRef`), `notes` (the region's notes, paged, with `startBeats`, `endBeats`, `name` and `timeSignature`), `timeline` (tracks as lanes with their regions in a window of beats, paged by lanes, with `beatsPerBar`, `playheadBeats`, `playing` and the markers read so far), `routing` (inputs, sends, output). A body names a track or region by `ref`, else takes the selected one.

## Events

`project` (path or identity changed), `transport` (the control bar's state on each change, and at
once from the Mackie LEDs: `{playing, recording, cycle, playhead, tempo, timeSignature}`), `tracks`
(`{epoch}` when the epoch moved: a header appeared, vanished, was renamed or moved, a region added,
removed or moved), `selection` (the focus's selection with its `epoch`, when it changes).

## What M2 leaves as it is

- Selecting a track from outside is possible only through the Mackie Control's select buttons:
  the headers' `AXSelected` isn't settable, their "Has Focus" button doesn't take a press, and
  the Tracks area doesn't take keyboard focus from Accessibility. The surface must be in its Pan
  view for select to mean the track; Logic opens its Control Surface Setup window when it first
  adds the surface, and the bridge reads the project's window by title while that window is main.
- A pop-up menu Logic opens from a button (a send slot, an output slot) holds the Accessibility
  call until the menu closes and shows no menu element: sends can't be set from outside yet.
- A list's text cells (a marker's name) aren't text fields: a marker can't be renamed from outside;
  a region's name in the Event List at the region level is one.
- Scrubbers and sliders move by one on a set and by ten on an increment (the tempo, the playhead's
  bar, a list's position segments), which M3's changes must count with.

## Helper protocol (`kumi-logic-hands`)

The bridge runs the helper as a child and speaks JSON lines on its stdin and stdout: one request
per line, one reply per line, events in between. A request is `{"id": n, "op": "…", …}`; the
reply is `{"id": n, "ok": true, "ms": n, …}` or `{"id": n, "ok": false, "error": "<code>", …}`;
an event is `{"event": "…", …}` with no id. One request at a time: the helper answers within 2 s
or with `timeout` (the work had started) or `busy` (the previous request still runs); it logs to
stderr only and never activates an app unless asked. The helper's README
(`daws/logic/logic-kumi/helper/README.md`) is the exact reference; this table is the summary.

| Op | Parameters | Reply |
| --- | --- | --- |
| `trusted` | `prompt` (bool, default false) | `trusted` |
| `apps` | `bundles` (ids) | `apps: [{bundle, pid, name, active, hidden}]`, then `app` events for those bundles |
| `processes` | `contains` (a substring of the executable path) | `pids` (ascending) |
| `activate` | `pid` | `active` |
| `windows` | `pid`, `menu` (the Window menu's title, default "Window") | `windows: [{title, role, subrole, main, focused, document, minimized, fullscreen, modal}]`, `listed` (titles after the Window menu's last separator; can include items that are not windows) |
| `tree` | `pid`, `root`, `path`, `depth` (1), `children` (100), `attrs` (all but `set` and `act`), `popups` (false) | `node`, `truncated` |
| `get` | `pid`, `root`, `path`, `attrs`, `popups` | `attrs: {"AXName": value}`, `truncated` |
| `set` | `pid`, `root`, `path`, `attr`, `value`, `settleMs` (50), `popups` | `result`, `before`, `after`; an AX failure is `ok: false` with the AXError name as `error` and those three fields kept |
| `action` | `pid`, `root`, `path`, `name`, `settleMs`, `popups` | as `set` |
| `menu` | `pid`, `titles` (from the menu bar down) | `result`, `title`, `active`; `no-menu-item` names the first title not found |
| `keys` | `pid`, `key`, `mods` (`cmd`, `shift`, `opt`, `ctrl`) | `posted`, `active` |
| `type` | `pid`, `text` (at most 200 characters) | `posted`, `active` |
| `click` | `pid`, `x`, `y`, `mods`, `count` (1) | `posted`, `active` |
| `midi.open` | `name`, `uid` (int, or a string hashed to one) | `opened`, `source`, `destination`; `midi-open` while open |
| `midi.send` | `hex` | `sent` |
| `midi.close` | | `closed` (idempotent) |
| `tap.start`, `tap.stop` | reserved for M5 | `reserved` |
| `quit` | | `ok` then exit |

Error codes: `malformed`, `unknown-op`, `bad-request`, `untrusted`, `no-process`, `no-app`,
`timeout`, `busy`, `popup`, `unknown-attr`, `no-menubar`, `no-menu-item`, `disabled`,
`unknown-key`, `unknown-mod`, `too-long`, `bad-hex`, `midi-closed`, `midi-open`, `reserved`, and
an AXError's name for a failed read, write or action.

`root` is `"app"`, `"main"` (`AXMainWindow`), `"focused"` (`AXFocusedWindow`), `"menubar"` or
`{"title": "substring"}`; `path` is child indexes joined by dots from that root. `depth` and
`children` bound a `tree`; `attrs` names the attributes wanted (`"all"` for every one, by node
field or AX name); `popups` false never reads a pop-up button's children, and a path through one
is refused with `popup`.

A node is `{i, role, subrole, title, desc, value, valueDesc, help, id, enabled, selected, pos:
{x, y}, size: {w, h}, set: [attributes], act: [actions], c: [nodes], more: n}`; fields that are
empty are left out; `more` counts children beyond `children`, or all of them at the depth limit.
Values are JSON (numbers, strings, booleans), points, sizes, rectangles and ranges are objects,
other elements are `{"ref": "role title"}`.

Events: `{"event": "midi", "hex": "…", "ms": n}` for bytes from the MIDI destination (a long
SysEx may come in several); `{"event": "app", "state": "launched" | "terminated" | "activated" |
"deactivated", "pid": n, "bundle": "…"}` for the apps named in the last `apps` request.

## Changes (`change_preview`, `change_apply`, `change_restore`)

M3 lets Kumi change the project and undo. The wire is the REAPER extension's: the host's journal
(`daws/own/daw-mcp-server/src/changes.rs`) previews a change, keeps it, applies it in one named step
(`desc`, `Kumi: … (k<nonce>)`) and undoes it as another; the bridge answers `change_preview`,
`change_apply` and `change_restore` with the same fields REAPER's does. Each kind writes through
one macOS channel and reads the value back: `applied` only when Logic shows the new value, else
`uncertain`. A preview writes nothing and fences the ref's epoch and the target's value as read
now; an apply re-checks the fence and refuses `changed` before it writes; a restore puts the value
back when the value can be set; added tracks, deleted tracks and deleted markers require manual Logic Undo History. Marker additions and moves require complete captured row/list identity, including visible Text, for value restore. A dialog up is `busy`, a missing
Accessibility grant `failed` in words (never busy). What was measured is the Scratch project in
Logic Pro 12.3.1; the host's simulated Logic (`simulator/logic.rs`) answers the same shapes.

### The kinds, their channel and readback

| Wire kind | Tool | Channel (the write) | Read back from | Logic undo step |
| --- | --- | --- | --- | --- |
| `track-volume` | `track_volume` | the header's Volume slider, stepped to the dB text | the slider's `AXValueDescription` | none |
| `track-pan` | `track_pan` | the header's pan slider (0–127, 64 centre) | the slider's value | none |
| `track-mute` | `track_mute` | a click at the header's Mute checkbox's frame | the checkbox | none |
| `track-solo` | `track_solo` | a click at the header's Solo checkbox | the checkbox | none |
| `track-arm` | `track_arm` | a click at the header's Record Enable checkbox | the checkbox | none |
| `track-rename` | `track_rename` | the Inspector's "Track:" field, `AXConfirm` | the header's label | "Renaming" |
| `track-add` | `track_add` | Track menu (New Audio/Software Instrument/External MIDI Track) | the new header | "Create Track" |
| `track-delete` | `track_delete` | Track > Delete Track, its sheet's Delete answered | the headers | "Delete Tracks" |
| `marker-add` | `marker_add` | the playhead moved there, the Marker List's Create new Marker | the list's row | "Create Marker" |
| `marker-move` | `marker_move` | the row's Position scrubber, a segment at a time | the list | "Value Change" |
| `marker-delete` | `marker_delete` | the row's Delete action | the list | "Delete" |
| `tempo-set` | `tempo_set` | the control bar's Tempo slider (whole BPM) | the LCD's tempo | none |
| `notes-add` | `notes_add` | the Event List's Create new Event, its cells stepped | the list's rows | "Create Event"/"Value Change" |
| `notes-change` | `notes_change` | the note's row cells (pitch, velocity, length, channel, position) | the list | "Value Change" |
| `notes-delete` | `notes_delete` | the row's Delete action | the list | "Delete" |
| `fx-bypass` | `fx_bypass` | the Mixer strip slot's bypass checkbox | the slot | none |

A header's control (volume, pan, mute, solo, arm) and the Mixer slot's bypass take a write without
the track selected or Logic in front; the mute, solo and arm checkboxes take no `AXPress` or set,
only a click at their frame, posted at the system's event tap with Logic brought forward first.
The name field, the Track menu and the lists' presses act only while Logic is the app in front, so
those kinds activate Logic and refuse `busy` if another app keeps the front. A list read opens its
pane and tab and puts them back; a marker is made at the playhead, which is moved there and back.

`transport_play`, `transport_stop` and `cursor_set` (the tools `play`, `stop`, `set_cursor`) go
through the control bar's Play and Stop buttons and the LCD's bar and beat sliders; none is a step
of Logic's undo, and `cursor_set` takes `positionBeats` or `position` (seconds at the project's
constant tempo). `undo_step_begin`/`undo_step_end` are counted (Logic has no undo block):
`undo_step_begin` answers `{stepId: "sN"}`, `undo_step_end` `{desc, entry: false, changes: n}`, and
each change in the plan stays its own step of Logic's undo.

### The fences and the outcomes

- A header prop fences the value now; a rename the name; a track delete `{name, index, regions}`; a
  track add `{tracks, names}` (the count and every name); a marker the complete raw row/list identity, including raw name, length (finite or known infinite), Text availability, column shape, AX identity and other visible cell state; the
  tempo the BPM; notes the region's note count and a hash of their values.
- `applied` carries the value Logic shows after the write. A write Logic doesn't show back is
  `uncertain` ("Logic Pro shows … after Kumi asked for …"), and leaves the step unnamed. The apply's
  `undo` is `{desc, entry}`, with `title` (the Edit menu's Undo title at apply time) and `entry: true`
  when Logic's history took a step, `entry: false` when it didn't. `track-add` answers the new epoch
  and `created: {ref: "<new epoch>:track:<n>"}`; `track-delete` answers `material` holding
  `{name, kind, index, volumeDb, pan, mute, solo, arm, step}` (`step` is the apply's `desc`).
- Restore is by value for header properties, tempo, notes and bypass, and for marker additions/moves only when every required original and applied identity field was exposed. Added tracks, deleted tracks and deleted markers are always manual-only: Logic cannot expose all automation, closed plug-in parameters or hidden track metadata, an Undo title is not a unique step identity, and recreating a marker cannot recover its original name faithfully. Native restore returns `failed` before any helper operation, including with `discard`.
- Marker material keeps full `prior` and `applied` lists, a unique full `marker`, and `priorMarker` for moves. `material.undo` is `{mode:"value"}` only for complete known snapshots; otherwise it is `{mode:"manual",reason,guidance}`. An unexposed Text field is `null`, never an invented empty string. Revealing Text later cannot recover an unknown original value. Identity/list mismatches and unknown original fields cannot be overridden by `discard`; an already-restored answer requires the exact complete prior list and unique target.
- The Logic host validates the private snapshots and exposes only `undo.restore` with the manual/value capability in its public apply reply. Other DAWs do not interpret this opaque material. Missing, legacy or malformed marker hints are manual-only.
- A confirmed manual-only change stays `Kept` in HISTORY with permanent guidance to review the appropriate step in Logic's Undo History manually. Both explicit Undo and default `/undo` send no RPC for it; default Undo does not skip it to an earlier change. Unconfirmed applies stay `Unsure`, never promoted to confirmed permanent changes. Direct host clients retain an applied transaction after explicit restore refusal.
- A restore's refs are from the change's own epoch, carried to where the track or region stands now:
  the bridge watches each epoch move (a header added, deleted, renamed or moved; the M2 observers'
  rule) and keeps, per move, where each place went, so undoing a fader or a rename after a track was
  added or deleted does **not** answer `stale`. A preview or an apply still refuses a ref from an
  earlier epoch as `stale`.

### First M4 slice: Loop Range only

`loop-range` (`loop_range`) is the 17th kind, ported only from the Loop Range slice
of historical commit `6f8f6e9bbc3666159ce68be1ce7466aa30e82670`. Other M4 kinds and
`project_save` remain disabled. The input requires nonnegative `startBeats` and
`endBeats`, in quarter notes; end must follow start, except `0,0` disables Cycle
without changing either locator. A preflight bound of 64 scrubber actions refuses distant ranges before writing; any transition that could cross locators when higher segments reset lower fields also requires manual editing. Locator values must align to 960 ticks per quarter note; other fractions are refused.

The cycle is a controlled value: on/off plus both locators. Fresh, complete known
single-meter N/4 identity (N from 1 through 16, /16 division) is required before preview, apply and restore. The Custom LCD and complete Signature List must already be visible, and Logic must be stopped and not recording; Kumi performs no setup or cleanup writes. The fence binds
that timebase and the original cycle. Apply refuses a producer edit or timebase
change before writing. Restore requires the complete original and applied values,
unchanged meter/timebase, and either the exact applied or exact prior cycle; an
unknown field or mismatch refuses even with `discard`. It never deletes an entity
or presses native Undo. Successful readback creates an Applied HISTORY row; a
no-op creates none. Missing legacy restore evidence is manual-only and cannot
make default Undo skip to an earlier change. An unanswered write stays uncertain.

The host simulator's cycle and single-meter timebase are explicit synthetic test
state, not inferred from M2 recordings. The preserved original cycle scene is
historical evidence; replay and fixture checks do not claim current native acceptance.

### The undo stack and `undone_in_daw`

Logic's Edit menu exposes Undo/Redo titles, without a unique step token. A producer's edit can
have the same title as Kumi's. The bridge therefore never presses native Undo automatically,
and it does not attribute marker, note or structural undos from titles alone.

Positive observations of the full tracked property or renamed value returning to its prior
value can still yield `undone_in_daw {descs, how, epoch}` (or a redo when it returns to the
applied value). These are value-backed observations, not proof of which keyboard action the
producer used. Marker, note and structural states must be read again; their title-only
attribution is deliberately absent. A selected menu may be cancelled before an authorized
write; reading never presses the Edit menu merely to validate its title.

`plan_steps` remains false: Logic provides no atomic undo block. A plan has separate native
steps where Logic records them, and the seven M3 value kinds plus Loop Range have no native undo step at all.

The historical M3 gate and original transcript are preserved with source hashes. Their old
automatic structural/unknown-Text marker restores are not current native parity. The current host fixture refuses the
same restore without changing the project or pressing any native Undo step.

### Where Logic forces a difference from REAPER

- Logic keeps **no undo step** for a mixer-ish move (volume, pan, mute, solo, arm, tempo, bypass):
  those answer `undo.entry: false` and are undone by value, never by Logic's own undo.
- The tempo takes **whole BPM** from the slider; a fractional `bpm` is rounded, with a note.
- A **marker can't be named** from outside (its name cell isn't a text field): `marker-add` refuses a
  `name`, and Logic numbers markers itself. `marker-rename` is not made.
- `loop-range`, `fx-add`, `fx-delete`, `fx-param`, `fx-move`, `send-add`, `send-volume` and
  `track-move` are **not made**: the cycle locators have no settable field, the slot pop-up menus
  can't be driven from outside, the plug-in window isn't reached, and the headers can't be dragged.
  Each is refused `bad-params` saying why.
- `track-add` places a track **below the selected one** (Logic's New Tracks behaviour), so `index`
  counts from 1 (0, above every track, is unreachable); a kind (`audio`, `instrument`, `external`)
  may be given.
