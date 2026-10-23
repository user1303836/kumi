# Logic Pro bridge contract (M2 to M6)

English · [简体中文](../zh-CN/LOGIC_CONTRACT.md) · [日本語](../ja/LOGIC_CONTRACT.md)

What `logic-kumi` answers over `kumi-daw/1`, and which macOS channel each answer comes from. The
wire, handshake, discovery files and lifecycle are daw-bridge's; this page is only Logic's shapes.
Everything here was measured in the lab of 2026-10-18 (`docs/evidence/logic-lab-2026-10-18.md`)
and, for the rows, on the Scratch project in Logic Pro 12.3.1 during M2. The exact JSON is the
host's simulated Logic (`crates/daw-mcp-server/src/simulator/logic.rs`) and the vectors the bridge
records from the real Logic (`crates/daw-mcp-server/tests/vectors/logic-reads.json`); where the two
differ, the vectors are what Logic shows and this page says why. A recording names no home of the Mac
it was made on: the bridge's recorder saves `/Users/<name>` as `/Users/me`, and a project's id, the
hash of its path, is the hash of the redacted path (`logic_kumi::fixtures::redacted`).

## Channels

| Channel | Used for | Rule |
| --- | --- | --- |
| Accessibility reads | every row, focus and body | poll; never read a pop-up button's children; prefer `AXMainWindow` and the Window menu over `AXWindows`; address the project's window by title when one of Logic's own windows (the Control Surface Setup) is main; a pane a read needs (the Mixer, the List Editors) is opened for it and closed after |
| Accessibility writes | values with a text field or slider, buttons, rows | set, then `AXConfirm`; read back before answering |
| Key commands | actions with a binding | activate Logic first; post modifiers as their own events |
| Menu items | actions without a binding | activate Logic first; press by index after matching the title |
| Mackie Control | selection, transport and level events; the LCD names; selecting a track; the timecode display followed while Logic plays (listening); the strips' solo buttons (a listening pass's solos) | answer only as a Mackie Control (device 0x14), and the version request; select from the Pan view, where the LCD names the strips; the timecode's digits are ASCII (0x31 is "1"), the first two controllers the assignment display ("PN") |
| Panel service | open and save panels | post keys to `com.apple.appkit.xpc.openAndSavePanelService`; set the go-to-folder field by AX |
| Spotlight | where a recent project is (`read {kind: "recent"}`) | `mdfind` by the project's file name, one query for every name; it finds only what macOS lets the bridge see |
| Preferences | the octave of middle C (the project row's `middleC`) | `defaults read com.apple.logic10 <key>`, through the preferences daemon, so a setting just changed in Logic's Settings is seen at once; never written |
| Process tap | hearing Logic (`hear`, `audition`) | the helper's `tap.*` on Logic's pid, into a 32-bit float WAV file under the request's folder; started and stopped around a run of passes, never left running; `mute` keeps Logic from the speakers meanwhile |

## Identity

- `pid`: Logic's; `resource`: Logic's bundle identifier (`com.apple.logic10`, or `com.apple.mobilelogic` for the Creator Studio build).
- `project.id`: no UUID exists in every project, so the id is the 32-hex SHA-256 prefix of the saved package path; unsaved projects carry an `identity` from the window title.
- `status` is flat, as REAPER's: `{project, busy, permission}`. `busy` is `false` when free, else `modal` (the main or focused window is a dialog or a sheet is open), `away` (Logic reports no window, its Space not being the current one) or `loading` (the project window has no title and no document yet); the last project is kept through those.
- `status.permission`: `null`, or `"accessibility"` while the helper is not trusted; reads then answer `failed`, never `busy`. The identity (daw, bridge, dawVersion, pid, resource, project) is in the welcome, as for every own bridge; the discovery file is `logic-<pid>-<key>.json` with the same fields.

## Rows (`read`)

| Kind | Fields | Source |
| --- | --- | --- |
| `project` | id, identity, path, tempo, timeSignature, keySignature, sampleRate, trackCount, middleC (3 or 4) | window document and the control bar's LCD; `MetaData.plist` for the sample rate when the package can be read (macOS keeps `~/Music/Logic` from a process without the Files and Folders permission: then `null`); Logic's preferences for `middleC`, the octave its editors name note 60 in ("Display Middle C As" in Settings > View > General: the key `MiddleC` on is C4, Roland; off, or never written, C3, Yamaha, Logic's default), read again at most every five seconds |
| `track` | ref, index, name, kind (audio, instrument, external, aux, output), mute, solo, armed, monitoring, volumeDb, pan, color (always `null`: the header shows no colour), output, inputs, selected, stack (only on a Track Stack's header: `true`), stripUnread (only when the Mixer hadn't drawn the track's strip: `true`; see "Catch-up's questions") | track headers (name, mute, solo, armed, monitoring, volume, pan, selected; a stack's disclosure triangle); the Mixer's strips for the kind, output and inputs, read when needed and kept for five seconds |
| `region` | ref, trackRef, name, startBeats, endBeats, kind (audio, midi), selected, looped | the Tracks area's layout items and their help text ("Region starts at 1 bar  and ends at 5 bars , MIDI region…") |
| `note` | ref, regionRef, position (bar, beat, division, tick), pitch, velocity, lengthTicks, channel, selected? | the Event List of the region, which shows the selected region's events: a region not selected is selected first, through its item |
| `plugin` | ref, trackRef, slot, name, bypassed, kind (instrument, effect) | the strip's slots, the instrument's first; the strip lists its slots bottom-up, the rows count them from the top; an output's plug-ins (the Stereo Out's) under `trackRef` `<epoch>:output:<n>` |
| `send` | ref, trackRef, slot, destination, level | the strip's send slots: a send's group names its bus, the knob listed right after it says its level ("-6.0", "-∞" as −96) |
| `param` | ref, pluginRef, index, name, value, text, min, max, normalized | the plug-in window's Controls View (opened from the slot's open button for the read and closed after, its view put back): a row per parameter, its name without the colon, the slider's value and range (`normalized` is the value's share of it), the text beside it; a choice's value is its option's place, and a menu's value `null`, as is a value the view shows with no control to set it (the Compressor's Knee: its text alone, and a change of it refused `bad-params`); needs `parent` (the plug-in) |
| `smart` | ref, trackRef, index, name, value (0 to 1), text | the Smart Controls pane (opened for the read and closed after) for the track, selected for the read and the selection put back; needs `parent` (the track); a track whose strip has nothing the pane maps (an audio track with only a Channel EQ on it) shows the pane's header over no controls and reads no rows, while a pane that hasn't drawn its header is `failed` |
| `automation` | ref, trackRef, index, parameter, number, events (positionBeats, value) | the Track Automation Event List of the track (its key command, command 1032, Control-Command-E in the factory set, opens it as a window of its own for the selected track; closed after): its "Fader" rows grouped by parameter, the parameter's name from the Length/Info cell, its number from Num, each event's value (0 to 127) from Val; needs `parent` (the track) |
| `marker` | ref, position, name, length (beats), text | the Marker List |
| `recent` | index (from 1, most recent first), name, path (`null` when not known), open | File > Open Recent's items (read without opening the menu: each is `_openRecentDocument:`, titled with the project's file name); `open` on the first item of the open project's file name alone, its `path` from the project's window; another item's from Spotlight when it finds one project of that name besides the open one (see "Recent projects") |
| `tempo` | bpm, tempos (position, bpm), signatures (position, time, key) | the Tempo List (`tempos` is every event of it, the first the project's tempo at its start, then each change of tempo by position) and the Signature List |

What the bridge reads outside Logic's windows (the preferences, Spotlight, a file on disk) goes through one seam
(`src/disk.rs`), so a recording keeps it beside the helper's replies and a replay answers the same: the M6 recording
of Scratch's reads kept `MiddleC` as "0" (written `false` since the lab's toggle of the setting, which found the key:
it appears as `true` once C4 is chosen and stays, as `false`, when C3 is chosen back).

Logic's own words are kept: the key signature as the LCD shows it ("C Major"), an output as the
slot names it ("Stereo Output"), a plug-in as its slot names it. Refs are positional with an epoch
(`<epoch>:<kind>:<n>`), as Renoise's: tracks by their place in the Tracks area, regions by their
place through the project (track by track), markers by their place in the list, notes under their
region (`1:note:1.3`), plug-ins and sends under their track (`1:plugin:1.1`), a parameter under its plug-in
(`1:param:1.1.7`), a Smart Control and a parameter's automation under their track (`1:smart:2.1`,
`1:automation:1.1`). The Mixer's output strips, which no track has, are `<epoch>:output:<n>` in the Mixer's
order (the Stereo Out is `1:output:1`), their plug-ins `<epoch>:plugin:0.<n>.<slot>` and those plug-ins'
parameters `<epoch>:param:0.<n>.<slot>.<i>`. The epoch moves on
every track or region change the bridge observes between polls (a header appeared, vanished, was
renamed or moved; a region added, removed or moved); notes, plug-ins, sends, markers and tempo
don't move it. A ref, parent, around or cursor from an earlier epoch is refused as `stale`,
whatever it was for. Paging: `limit` (50, at most 200), `cursor` `<epoch>.<n>`, `nextCursor`,
`total` and `epoch` in every answer; `ref` alone reads one row; `parent` filters; `note` needs its
region as `parent`.

### The bulk read (`snapshot {rows}`)

`snapshot {rows: [kinds], limit?}` reads every row of the kinds named (of `project`, `track`, `region`, `plugin`,
`send`, `marker`, `tempo`) in one request, from one look at the project in one epoch: `{epoch, rows: {<kind>: [rows]},
totals: {<kind>: n}, truncated?: [kinds]}`, each kind's rows exactly what `read` pages for it (the same rows in the same
order), cut at `limit` (10000, the most) with the kind named under `truncated` to page with `read`. Without `rows`,
`snapshot` answers as before. It shows the Mixer once when tracks, plug-ins or sends are asked (their kinds, routing
and slots are the Mixer's) and the List Editors once for the markers, the tempo and the signatures (each tab in turn,
then the tab and the pane put back), where the paged reads show the List Editors once per list: on Scratch the six
reads a catch-up makes (project, track, region, plugin, marker, tempo; 200 rows a page) took 4.4 to 4.5 s, the
Mixer 0.7 s of it, the Marker List 1.2 s and the Tempo and Signature lists 2.4 s, and the bulk read of the same rows
2.7 s. Both open what Logic doesn't show already: no read gives the strips without the Mixer, nor the lists without
the List Editors; a pane the producer has open stays open. The answer is one frame (the wire takes 64 MB).

### Recent projects

Logic keeps its recent projects in macOS's shared file list, which macOS keeps from other processes ("Operation not
permitted"), and its preferences hold no project path (518 keys on the lab's Mac; `DefaultDir1` to `4` are the open
and save panels' last folders). File > Open Recent shows them, and Accessibility reads its items without opening the
menu, but an item carries the project's file name alone (`AXTitle`; no URL, document or help: its title, identifier
and frame are all Accessibility shows of it on Logic Pro 12.3.1). So the open project can't be matched by its whole
path: it is the first item of its file name (the menu starts with the latest, and opening a project puts it there),
and only that item is `open`; another of the same name (a project of that name in another folder) isn't. `read {kind:
"recent"}` answers the items in order with `path` where the bridge can know it: the project open now (its window's
document), and a project Spotlight finds by that file name, once, outside any other package and the Trash and besides
the open one; two items of one name besides the open one can't be told apart, so neither has a path. Spotlight
answers only for folders the bridge's process may read, so a project under `~/Music` (Logic's own folder) is never
found unless it is the one open: on the lab's Mac the eight recent projects read with Scratch's path from its window
and the seven gate copies' from Spotlight (they sit in a shared folder outside the home), in 101 ms (66 ms for a
second read).

### Catch-up's questions (M6)

Measured on a copy of Scratch with 38 audio tracks added below Vocal (40 tracks, Keys and its region last):

- **Tracks and regions out of view are read**: the headers' group lists all 40 headers with their frames, in view or
  not (header 1 at y -1420 while the Tracks area shows y 194 to 1117), and the contents list every region (Keys' on
  track 40, below the view). **The Mixer draws only what it has shown**: it lists every strip, but the strips right of
  its view (the 40th at x 1728, the pane's right edge, then the Stereo Out and the Master) have numbers for a
  description ("224 40 4") and generic controls and slots ("output", "input", "audio plug-in", each bypassed), where
  Keys holds Piano, Channel EQ, Compressor and ChromaVerb; a strip scrolled past on the left (Vocal at x -885) reads
  right. `AXScrollToVisible` on such a strip is answered and moves nothing, and the Mixer shows Accessibility no scroll
  bar. So the bridge takes no routing, plug-ins or sends from a strip it recognises as undrawn (its description bare
  numbers, not its name), and the track row says `stripUnread: true`: a snapshot can tell an unread strip from an
  empty one. The track's kind still follows its slots' shape (Keys read `instrument`).
- **Reading 40 tracks**: the six paged reads took 6.5 s (project 0.3 s, track 1.4 s, region 0.3 s, plug-in 0.3 s,
  marker 1.5 s, tempo 2.7 s) and the bulk read 3.3 s.
- **A region's notes aren't shown**: a region's item in the Tracks area holds its name field and three handles (move
  left border, move right border, loop) beside its help text and its size ("4 bars"); no count or fingerprint of its
  notes, which only the Event List gives (about 0.7 s a region, the region selected).
- **`looped` shows only in a region's width**: looped, Keys kept its help text ("starts at 1 bar and ends at 5 bars")
  and its size ("4 bars") while its item's width ran with the loop to the project's end (13,421,293 px, against 192 px
  unlooped). The region reads ask for the cheap attributes, which have no frame, so `looped` stays `false` from the
  Tracks area; the Region inspector's Loop checkbox (`region-loop`) says it for the selected region.
- **Project Alternatives** weren't switched: File > Project Alternatives fills its submenu only once the menu opens
  (Accessibility lists one untitled item before) and no key command reaches it, and the bridge presses no menu bar item
  (the M3 gate's wedge). The window's document is the package (`…/Scratch M6.logicx/`) and the alternatives are folders
  inside it (`Alternatives/000`, …), so the id, the hash of the document's path, is the package's as long as the
  document stays the package; a switch wasn't watched.

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
- A pop-up menu Logic opens from a button (a send slot, an insert slot, an output slot) holds the
  Accessibility call while it is up and shows no menu element. M4's second slice found that the
  menu takes keys posted to Logic all the same (see "Where Logic forces a difference").
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
(`crates/logic-kumi/helper/README.md`) is the exact reference; this table is the summary.

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
| `tap.start` | `pid`, `path` (a new WAV file's whole path), `mute` (false), `maxSeconds` (600, up to 3600) | `sampleRate`, `channels`, `hostTime`; `busy` while a tap runs, `denied` (no System Audio Recording permission), `unsupported` (before macOS 14.2), `failed` with `step` and `detail` (`step: "process"` while the process makes no audio yet) |
| `tap.status` | | `running`, `frames`, `seconds`, `truncated` |
| `tap.stop` | | `file`, `frames`, `seconds`, `sampleRate`, `channels`, `peak`, `rms` (linear), `anchors` (`[frame, hostTime]`), `timebase` (`{numer, denom}`), `dropped`, `truncated`; `failed` when no tap runs |
| `quit` | | `ok` then exit |

Error codes: `malformed`, `unknown-op`, `bad-request`, `untrusted`, `no-process`, `no-app`,
`timeout`, `busy`, `popup`, `unknown-attr`, `no-menubar`, `no-menu-item`, `disabled`,
`unknown-key`, `unknown-mod`, `too-long`, `bad-hex`, `midi-closed`, `midi-open`, `denied`,
`unsupported`, `failed`, and an AXError's name for a failed read, write or action.

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

Events: `{"event": "midi", "hex": "…", "ms": n, "hostTime": n, "timeStamp": n}` for bytes from the
MIDI destination (a long SysEx may come in several; `hostTime` is when the helper received them,
`timeStamp` the packet's own host time stamp, 0 when the sender gave none); `{"event": "app", "state": "launched" | "terminated" | "activated" |
"deactivated", "pid": n, "bundle": "…"}` for the apps named in the last `apps` request.

The process tap (`tap.*`) records a process's output mixed down to stereo into a WAV file the helper
writes itself: format 3 (IEEE float), 32-bit, at the rate of the tap's private aggregate device
(clocked by the default output device; a process playing at another rate is resampled to it), the
header's sizes patched every second and at the end. `mute` keeps the process's own output from the
speakers while the tap is read. `anchors` pair a frame of the file with the host time
(`mach_absolute_time`, the clock CoreMIDI stamps packets with) Core Audio gave the buffer starting
there: the first buffer, one at least every quarter second, one after any loss, and the last. A host
time `t` falls on frame `f + (t - h) × numer / denom / 10⁹ × sampleRate` from the anchor `(f, h)` at
or before it. `dropped` counts frames lost; a capture that reaches `maxSeconds` stops writing and says
`truncated`. EOF, `quit` and SIGTERM finish a running tap's file; the tap and its aggregate device
are private to the helper and go with it.

## Changes (`change_preview`, `change_apply`, `change_restore`)

M3 lets Kumi change the project and undo, and M4 adds the kinds below it from `key-command` down,
with `project_save` and `key_command`, REAPER's `run_action`'s shape. The wire is the REAPER extension's: the host's journal
(`crates/daw-mcp-server/src/changes.rs`) previews a change, keeps it, applies it in one named step
(`desc`, `Kumi: … (k<nonce>)`) and undoes it as another; the bridge answers `change_preview`,
`change_apply` and `change_restore` with the same fields REAPER's does. Each kind writes through
one macOS channel and reads the value back: `applied` only when Logic shows the new value, else
`uncertain`. A preview writes nothing and fences the ref's epoch and the target's value as read
now; an apply re-checks the fence and refuses `changed` before it writes; a restore puts the value
back when the value can be set, else turns to Logic's own undo. A dialog up is `busy`, a missing
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
| `key-command` | `key_command` | the key command's chord, by name or id from `com.apple.logic10.plist` (else the factory U.S. set), posted with Logic in front | the Edit menu's Undo title, moved or not | the command's own, or none |
| `region-move` | `region_move` | the Event List at its region level (Leave Folder), the region's Position scrubber | the Tracks area's items | "Value Change" |
| `region-resize` | `region_resize` | the playhead at the new edge, Set Region Start/End to Playhead (Cmd-[ / Cmd-]) | the Tracks area | "Set Start to Playhead Position", "Set End to Playhead Position" (one each edge) |
| `region-split` | `region_split` | the region clicked, the playhead at the split, Split at Playhead (Cmd-T) | the Tracks area: the right part is `created` | "Split Regions" |
| `region-loop` | `region_loop` | the Region inspector's Loop checkbox | the checkbox (the Tracks area never shows `looped`) | "Loop Parameter Change" |
| `region-rename` | `region_rename` | the Region inspector's name field, `AXConfirm` | the Tracks area | "Renaming" |
| `region-delete` | `region_delete` | the region clicked, the Delete key | the Tracks area | "Delete" |
| `loop-range` | `loop_range` | the LCD on Custom, its two locator groups' sliders, the Cycle button | the locators and the button | none |
| `notes-quantize` | `notes_quantize` | the Event List: Edit > Select > All, the header's Quantize pop-up | the rows, on the grid | "Quantize" |
| `midi-import` | `midi_import` | a format 0 file under `/tmp/kumi-logic`, File > Import > MIDI File…, the panel's go-to | the new region and track | "Import" |
| `tempo-marker-set` | `tempo_marker_set` | the Tempo List: Create new Tempo Event at the playhead, the Tempo cell's segments | the list | "Insert Tempo" + "Value Change" |
| `tempo-marker-delete` | `tempo_marker_delete` | the row's Delete action | the list | "Delete" |
| `signature-set` | `signature_set` | the Signature List: Create new Time/Key Signature Change, the numerator slider, the denominator pop-up | the list | "Value Change" |
| `track-stack` | `track_stack` | the headers clicked (Shift for the others), Create Summing/Folder Stack (Cmd-Shift-G/H) | the headers: the stack's disclosure triangle | "Create Track Stack" |
| `track-stack-flatten` | `track_stack_flatten` | the stack's header clicked, Flatten Stack (Cmd-Shift-U) | the headers | "Flatten Stack" |
| `fx-add` | `fx_add` | the Mixer: the empty slot under the strip's last effect (or the insert bar along the top of the effect it goes before) pressed, its menu's search typed key by key, Down, Return; the plug-in window Logic opens closed | the strip's slots: the name asked at its place | "Insert Plug-in in Channel Strip" (an empty slot), "Change Plug-in in Channel Strip" (an insert bar) |
| `fx-delete` | `fx_delete` | the Mixer: the plug-in's list button pressed, No Plug-in (Down, Down, Return) | the strip's slots | "Remove Plug-in in Channel Strip" |
| `fx-param` | `fx_param` | the plug-in window (its slot's open button), View > Controls, the parameter's slider set (exact), its choice pressed or its switch toggled; the view put back and the window closed | the Controls View's value and text | none (the plug-in window's own undo has it) |
| `fx-params` | `fx_params` | as `fx-param`, several in one window | the Controls View | none |
| `fx-preset` | `fx_preset` | the plug-in window in its editor (made larger until its header shows the buttons): the next or previous setting button, `step` times, or next until the setting menu names `preset` | the setting pop-up's name | none (the plug-in window's own undo, which the restore presses) |
| `patch-load` | `patch_load` | the track selected, the Library (Y): its search field set to the patch's name, the result's row selected; the search cleared, the Library closed | the Inspector strip's setting button (the patch's name) and its plug-ins | "Load Patch" |
| `send-add` | `send_add` | the Mixer: the strip's empty send slot pressed, Down, Down (Bus), Right, Down to the bus, Return; the level then stepped as `send-volume` | the strip's sends: a group named "Bus n" (and the aux Logic made, under `created.aux`) | "Change Send in Channel Strip" |
| `send-remove` | `send_remove` | the Mixer: the send's list button pressed, No Send (the sixth item) | the strip's sends | "Change Send in Channel Strip" |
| `send-volume` | `send_volume` | the Mixer: the send's knob, coarse by increments, fine by sets (0.1 dB near 0 dB) | the knob's text | none |
| `smart-control-set` | `smart_control_set` | the track selected, the Smart Controls pane (B): the control's slider, tenths by increments, hundredths by sets | the slider's value and text | none |
| `automation-add` | `automation_add` | the track selected, its Track Automation Event List (command 1032, Control-Command-E): the event type set to Fader, the playhead at each event's beat, Create new Event, the new row's Num stepped until its Length/Info cell names the parameter (or to the number asked), its Val stepped; the type, the playhead and the selection put back, the list closed | the list's rows | "Create Event"/"Value Change" |
| `automation-change` | `automation_change` | the list: the event's row deleted and made again where and as asked | the list | "Delete", then as `automation-add` |
| `automation-delete` | `automation_delete` | the list: the row's Delete action | the list | "Delete" |
| `metronome-set` | `metronome_set` | the control bar's Metronome Click checkbox, pressed only when it differs from what is asked | the checkbox | none |
| `sample-load` | `sample_load` | File > Import > Audio File… through the panel service onto an audio track at the playhead, then Track > Convert Regions to New Sampler Track… (its sheet: zones from Regions, Sampler, the name, OK) | the new header and its strip in the Inspector (the instrument slot's plug-in) | "Insert", then "Convert Regions to New Sampler Track" |

A header's control (volume, pan, mute, solo, arm) and the Mixer slot's bypass take a write without
the track selected or Logic in front; the mute, solo and arm checkboxes take no `AXPress` or set,
only a click at their frame, posted at the system's event tap with Logic brought forward first.
The name field, the Track menu and the lists' presses act only while Logic is the app in front, so
those kinds activate Logic and refuse `busy` if another app keeps the front. A list read opens its
pane and tab and puts them back; a marker is made at the playhead, which is moved there and back.

`transport_play`, `transport_stop` and `cursor_set` (the tools `play`, `stop`, `set_cursor`) go
through the control bar's Play and Stop buttons and the LCD's bar and beat sliders; none is a step
of Logic's undo, and `cursor_set` takes `positionBeats` or `position` (seconds at the project's
constant tempo). The playhead is placed exactly on the beat asked, wherever it is placed (`cursor_set`,
and where a marker, a split, a resize edge, a tempo or signature event, an import or an automation event
is made): the LCD's bar and beat sliders keep the division and tick the playhead had, and a stop leaves
those anywhere (after one, Logic played from 2.1.3.162 and a split asked at bar 3 landed past it), so
when Logic has played or stopped since the bridge last placed it, or the Mackie Control's timecode shows
a division or tick past 1, the playhead goes to the project's start first with the Stop button, which
reads "Go to Beginning" while Logic is stopped away from it (at the start it reads "Stop", and the
playhead is on the beat already), then the bar and beat are stepped. `undo_step_begin`/`undo_step_end` are counted (Logic has no undo block):
`undo_step_begin` answers `{stepId: "sN"}`, `undo_step_end` `{desc, entry: false, changes: n}`, and
each change in the plan stays its own step of Logic's undo.

`bounce {epoch, startBeats, endBeats, path, format?}` and `export_tracks {epoch, path}` are actions, methods of the
bridge as `project_save` is (no change, no row, no undo step). `bounce` goes through File > Bounce > Project or
Section…: Logic's Bounce dialog with Uncompressed alone checked (its row selected, so the File Type and Bit Depth
pop-ups are its own), the format's file type and bit depth (`wav24` by default, `wav16`, `wav32f`, `aiff24`),
Normalize Off, Add to Project and Add to Apple Music Library unchecked, the Start and End positions' segments
stepped, OK; then the save panel through its service (the folder by the go-to sheet, the name), and the file is
waited for on disk and its header read (`{epoch, bounced, format, bytes, seconds, rate, channels, bits, startBeats,
endBeats}`; `uncertain` when its length isn't the range's at the project's tempo). `path` is a whole path to a `.wav`
(or `.aif`) that isn't there yet, in a folder that is. Add to Project, which the dialog had checked, puts the file in
the project's audio, and that is a step of Logic's undo ("Bounce Project") on top of whatever Kumi did before: a
Kumi step taken back through Logic's own undo (a split, a patch) was then refused, its Edit menu saying "Undo Bounce
Project". With it unchecked a bounce changes nothing in the project and leaves Logic's undo as it was (a split, a
bounce, then the split undone through Logic's undo: recorded). The dialog keeps these choices for the next bounce, as
it keeps the format. A bounce holds whatever Logic plays, the metronome's click included while the control bar's
Metronome Click is on (bars 6 and 7 of Scratch, where no region plays, bounced at a peak of 0.42 with the click on):
`bounce` leaves the click as the producer set it, and `render` (M5, "Listening" below) turns it off for its bounce.
`export_tracks` goes through File > Export > All Tracks as Audio Files…, an open panel that takes a
folder (its options are left as the producer set them): the folder (which must be there) is reached by the go-to
sheet, Export pressed, and the answer lists the files Logic wrote there (`{epoch, folder, files: [{name, bytes,
seconds}]}`). What the bridge looks at on disk goes through one seam (`src/disk.rs`), so a recorded scene keeps
it and a replay answers from it.

`project_save {epoch, path?, version?}` (the tool `save_project`) is REAPER's shape: no `path`
saves in place with Logic's Save key command (Cmd-S), answered at once with `dirty: false` when the
window's close button shows nothing to save; a `path` (a whole path to a `.logicx` that isn't
there yet) is File > Save As… through macOS's panel service (the go-to-folder sheet, the name
field, Save), and the project is that file from then on. Read back from the window's document and
its edited flag; never over a file Kumi can see, never the project's own file as a path
(`bad-params`), never into a folder that isn't there (the panel's "where" pop-up must name it, else
`bad-params` and the panel is cancelled; Logic itself leaves an empty `<name>.logicx/Alternatives`
package behind for the name its panel showed). `version` is REAPER's and refused. Saving is no
step of Logic's undo. `{saved, dirty}` answers.

### The fences and the outcomes

- A header prop fences the value now; a rename the name; a track delete `{name, index, regions}`; a
  track add `{tracks, names}` (the count and every name); a marker its whole state or the count; the
  tempo the BPM; notes the region's note count and a hash of their values. A region kind fences the
  region's `{startBeats, endBeats, name, track, looped}` and the region count; `loop-range` the cycle
  `{on, startBeats, endBeats}`; `notes-quantize` the notes' count and hash; `midi-import` and
  `track-stack` `{tracks, names, regions}`; a tempo event or a signature its row (`exists`, `bpm` or
  `timeSignature`) and the list's count; `key-command` the command's id and name; `metronome-set` the click
  `{on}`.
- `key-command` answers like REAPER's `run_action` (the tool is `key_command`): `applied: {command, name, changed}`, with
  `display.unchanged` and `undo.entry: false` when the Undo title didn't move (Logic made no step
  of it); when it did, the step is kept under Logic's own name and its restore is `daw-only`
  (Cmd-Z in Logic). Save, Save As, Undo, Redo, Quit, the project commands and every command whose
  name ends in "…" (a dialog) are refused.
- `region-split` answers `applied: {left, right}` and `created: {ref}` (the right part, in the new
  epoch); `midi-import` answers `applied: {notes, startBeats, endBeats, track, trackName, file}`
  and `created: {ref, track, trackAdded}` (Logic imports a MIDI file onto a new track below the
  one named, never into it), with `rows` and a note when the Event List lists fewer rows than the
  file's notes (it lists the rows on its screen; the region's length says the rest came in);
  `track-stack` answers `applied: {index, name, kind, tracks}` (the subtracks' refs) and `created:
  {ref, stack}`; `tempo-marker-set` and `signature-set` answer `made: true` when they made the row.
- `fx-add` answers `applied: {index, name, slot, plugins}` (the strip's effects after, from the top) and
  `created: {ref, kind: "plugin"}`; its fence is the strip's `{plugins, instrument}`. `fx-delete` answers
  `applied: {deleted, plugins}` and `material: {name, index, strip, step}`; it refuses an instrument. `fx-param`
  and `fx-params` fence and answer `{plugin, params: [{index, name, value, text}]}`; `fx-preset` answers
  `{plugin, preset, steps}` with `material: {steps}`. `send-add` answers `applied: {destination, slot, level}`
  and `created: {ref, kind: "send", bus, aux?}`; `send-remove` `material: {destination, level, slot, step}`;
  `send-volume` `applied: {level}`. `patch-load` fences and answers `{patch, plugins}` (the strip's setting
  name and plug-ins). `smart-control-set` fences and answers `{name, value, text}`. The automation kinds take
  `{ref: track, parameter: <name as the list's Length/Info cell writes it, "Volume"> | <its number>}` with
  `events: [{positionBeats, value}]` (`automation_add`; whole beats, values 0 to 127), `changes: [{index,
  positionBeats?, value?}]` (`automation_change`) or `indexes: [...]` (`automation_delete`), an event named by its
  place among that parameter's events from 0 as the read lists them; they fence the track's whole automation
  `{events: [{positionBeats, value, number, parameter}]}` and answer `applied: {made, removed, events}`.
- `sample-load` (`sample_load {kind: "sampler", file, ref?, positionBeats?, name?}`) takes the whole path of an
  audio file (wav, aif, aiff, aifc, caf, mp3, m4a) Kumi can see, an audio track for it to come in on (`ref`; else the
  selected track when it is an audio track, else the first audio track; `bad-params` for a track of another kind),
  where (`positionBeats`, a whole beat; else the playhead) and the instrument's name (else the file's name without its
  extension). It fences `{tracks, names, regions}` and answers `applied: {file, kind, track, name, instrument,
  startBeats, endBeats, sourceTrack, answered}` (the new track's place, name and instrument as read back, its trigger
  region's bounds, the audio track's place) and `created: {ref, kind: "track", region, source}` (the new track, its
  trigger region and the muted audio region, in the new epoch). `kind: "quick-sampler"` is refused: Logic converts
  audio regions into a Sampler, Alchemy or Drum Machine Designer track only (the sheet's three instruments), and Quick
  Sampler takes a sample by a drag onto its window.
- `metronome-set` (`metronome_set {on}`) previews, fences and answers the click as `{on}` with the target `{ref: null,
  kind: "project"}`; a click already as asked is `display.unchanged` (no step, no row), and one the control bar
  doesn't show (taken off it with Customize Control Bar and Display) is `failed`. Its restore presses it back by value.
- `applied` carries the value Logic shows after the write. A write Logic doesn't show back is
  `uncertain` ("Logic Pro shows … after Kumi asked for …"), and leaves the step unnamed. The apply's
  `undo` is `{desc, entry}`, with `title` (the Edit menu's Undo title at apply time) and `entry: true`
  when Logic's history took a step, `entry: false` when it didn't. `track-add` answers the new epoch
  and `created: {ref: "<new epoch>:track:<n>"}`; `track-delete` answers `material` holding
  `{name, kind, index, volumeDb, pan, mute, solo, arm, step}` (`step` is the apply's `desc`).
- Restore is by value where the value can be set (every header prop, the tempo, markers, notes,
  bypass), which is a fresh step of Logic's undo, answered `undo: {desc, entry: false}`. A deleted
  track can't be made again by value, so its restore is Logic's own undo (Cmd-Z through the Edit
  menu), sent only while that delete is still Logic's top Undo step (the watch holds it by the
  change's kind-and-target and by the step's `desc`); it answers `restored`, `undo: {desc, entry:
  false, how: "undo"}` and a note that Logic's own undo did it, else `failed` ("Logic Pro's undo has
  other steps on top of this one …; Cmd-Z in Logic Pro can, step by step"). `marker-delete`'s restore
  takes Logic's own undo first while its step is on top, else makes the marker again by value (Logic
  numbers it itself). Of the M4 kinds, a move, resize, loop, rename, cycle, quantize, tempo event
  and signature go back by value; a split, a region delete, an import, a flattened stack and a
  deleted tempo event only through Logic's own undo while on top (`undo: {how: "undo"}`); a stack
  Kumi made is flattened again by value when its step isn't on top. Of the second slice's, a plug-in
  added is taken off again by value, a deleted one comes back through Logic's own undo while on top
  (with its settings) and else by name (with Logic's defaults; an instrument isn't deleted); a send
  added is taken back through Logic's own undo while on top (the aux Logic made goes with it) and
  else by value (No Send, the aux left in the Mixer), a removed one likewise; a patch only through
  Logic's own undo while on top; parameters, a send's level and Smart Controls by value; a preset
  through the plug-in window's own Undo, pressed as many times as Kumi stepped; automation events by value (the
  events Kumi made deleted, the ones it deleted made again).
- A restore's refs are from the change's own epoch, carried to where the track or region stands now:
  the bridge watches each epoch move (a header added, deleted, renamed or moved; the M2 observers'
  rule) and keeps, per move, where each place went, so undoing a fader or a rename after a track was
  added or deleted does **not** answer `stale`. A preview or an apply still refuses a ref from an
  earlier epoch as `stale`.

### The undo stack and `undone_in_daw`

Logic tells an extension nothing of its own undo, its Undo History isn't readable from outside, and
the Edit menu's Undo and Redo titles name the top steps only once Logic has validated that menu —
which a key command of its own (the producer's Cmd-Z among them) does not do. So the bridge keeps a
stack of Kumi's steps in the order Logic's history holds them, each with the title Logic gave it at
apply time, the `desc` the host sent, and (for what the focus tick reads anyway) a probe: a header's
mute, solo, arm, volume, pan or name, the tracks' names, or the tempo. Each tick (the focus tick,
the menu bar reads without activating Logic) it looks two ways:

- the probes: a value back at what it was before Kumi's change is that change undone by hand (`how:
  "undo"`), back at what Kumi set is a redo; these fire at once, whatever the titles say;
- the titles, read as they stand (~0.6 ms): the Redo title taking a step's name is that step and
  every one above it undone, the Undo title taking it back a redo. A step whose probe still shows it
  applied is not taken as undone by a title alone, since Logic's titles carry no nonce and the
  producer's own step may share the name. The tick never validates the titles itself: they are
  refreshed only after Kumi's own writes while Logic is in front, by a key equivalent Logic doesn't
  own (which opens nothing); behind, they are read unverified, and an apply then names its step by
  the kind's own Logic name and carries no `title`.

The bridge presses no menu bar item. It used to validate the Edit menu while Logic was behind with a
press and a cancel of the item (invisible, the menu bar being off screen); at the M3 gate one press
left the item `selected` with no menu to cancel, and Logic then took no click, key, selection or
activation and answered no Apple Event until a real click, so every change after it failed. A read
can't wedge anything and the probes name most undos at once, so the loss — a producer's Cmd-Z on a
marker or note step while Logic is behind is named when Logic next validates its Edit menu (Kumi's
next write in front, or the producer opening the menu) — is taken over the risk; the log says so once
when the titles are first read unverified. What is kept is the cure: before a write gives up on the
front as `busy`, a menu bar item found left `selected` is closed with a cancel on its menu (twice at
most) and Logic asked forward once more; a menu that stays open is logged, and the change is `busy`.
A restore by Logic's own undo brings Logic in front first and checks the validated title only then.

The bridge sends `undone_in_daw {descs, how, epoch}` with the `desc`s of the changes the titles or
probes moved, so the host's `by_descs` matching names Kumi's changes although Logic's titles carry no
nonce. `plan_steps` is false for Logic: a plan is several steps of Logic's undo, named by their
titles.

### Where Logic forces a difference from REAPER

- Logic keeps **no undo step** for a mixer-ish move (volume, pan, mute, solo, arm, tempo, bypass):
  those answer `undo.entry: false` and are undone by value, never by Logic's own undo. Nor for the
  **metronome** (`metronome-set`): with Logic's history empty, its Edit menu still said "Undo", greyed, after the
  Metronome Click was pressed off and on again (each press validated by the refresh key). The click is a setting
  Logic saves with the project, so turning it changes the project as the cycle does: Logic asks to save it on
  close (as after a listening pass, which turns it off and on).
- The tempo takes **whole BPM** from the slider; a fractional `bpm` is rounded, with a note.
- A **marker can't be named** from outside (its name cell isn't a text field): `marker-add` refuses a
  `name`, and Logic numbers markers itself. `marker-rename` is not made.
- `fx-move`, `send-mode`, `track-move`, `track-color` and `marker-rename` are **not made**: Logic
  moves a plug-in only by dragging it; a send's mode is a mark in the send's menu, which Accessibility
  can't read, so it could never be read back; the headers can't be dragged (no key command reorders
  tracks and a header offers no action but its menu); the Color palette's swatches are bare positions
  to Accessibility while a header shows no colour. Each is refused `bad-params` saying why.
- The **Mixer's slot menus are Logic's own** (an insert slot's, an insert bar's, a send slot's, a
  slot's list button's): Accessibility shows no element for them and a press is held while the menu is
  up, so the helper answers it `cannotComplete` after its 1.5 s messaging timeout (`timeout` on a
  slower Mac) with the menu still up (the window server shows it at the pop-up menu level). Keys posted
  to Logic then go to the menu: the plug-in menu's search takes a name typed key by key (key codes;
  text posted as characters reaches it too, but the bridge types keys), Down highlights its first
  result and Return takes it, in the format Logic picks for the strip; an occupied slot's menu has No
  Plug-in second; the send menu is walked by arrows (Down twice to Bus, Right into it, Down to the
  bus), and an occupied send's menu has No Send sixth, after its routing modes, whose mark Accessibility
  can't read. Escape closes any of them. A plug-in or a send Logic took is read back by name, and the
  wrong one (another search result) is taken off again.
- Logic **opens a plug-in's window when it inserts one** (a dialog titled after the strip); the bridge
  closes it again and waits for it to go, since while it is up the Edit menu speaks for the window
  ("Undo", "Redo") and not for the project.
- A **plug-in's parameters are reached in its window's Controls View** (the view menu, a standard menu
  read through the pop-up), whose sliders take an exact value; Logic's own plug-ins expose their
  sliders in the editor view too, but the Controls View lists every plug-in's. A number asked as `value`
  (what the view shows: -3 for "-3.0 dB") is found by a bisection over the slider's raw values, each set
  and its text read: it goes on until a value reads the number exactly (the Stereo Out's Gain reads
  -3.0 dB at raw 930, where the first reading within a quarter dB, -3.1 dB, used to end it), and when no
  value does, the nearest within the number's own precision is set and `display.note` says the slider's
  step there. A parameter moved there
  and a preset loaded are steps of the plug-in window's own undo (its Undo and Redo buttons), never of
  the project's, so `outside_undo` holds them and the restore is by value (a preset's by the window's
  Undo). The setting (preset) menu is Logic's own too: a preset is reached through the next and
  previous buttons. A **narrow header hides what doesn't fit**: the Controls View's header has no Undo
  and Redo, and a small editor (a Gain at 75%) not even the setting buttons, so `fx-preset` works in the
  editor, made a size larger at a time (the view menu's sizes) until the header shows the button, the
  size put back after. "Default Preset" is on no setting button's way (next from it is the first
  preset, previous from there stays put): only the window's Undo brings it back.
- A **plug-in with an editor of its own puts its controls beside the header's**: the Compressor's
  window (Logic Pro 12.3.1) lists 44 elements at its top level, its knobs, circuit type buttons and
  fields among the header's, with the texts naming the plug-in and the strip last; the bridge reads a
  window's top level 40 at a time and again whole when that was cut, so the window is found. Its
  header lists the Side Chain pop-up ("Internal") before the setting pop-up ("Custom"), so the setting
  is the header's last pop-up; its editor keeps a size of its own (71%) beside the view menu's 50% to
  200%, and a size larger is the next of the menu's above it. Its Controls View lists 25 parameters:
  Threshold ("-17.5 dB"), Ratio ("3.9:1"), Attack, Release, Make Up ("0.0 dB"), Knee ("0.7"),
  Peak/RMS, Auto Gain, Distortion, Circuit Type, Side Chain Detection, Limiter Threshold, Limiter
  On, Auto Release, the side chain filter's State, Mode, Frequency, Q and Gain, Mix, Gain Reduction,
  Input Gain, Output Gain, Display Select and View Select (Scratch's Keys). Knee, Peak/RMS, the filter's
  Gain and Gain Reduction are shown with no control (the text's group holds a disabled slider with no
  range): they read with their text and a `null` value, and aren't set.
- A **send Logic makes starts at −∞** on this Mac: `send-add` sets `volumeDb` when it is given, and
  says so when it isn't. A new bus's aux is Logic's own doing, named under `created.aux`.
- A **patch replaces the whole strip** (instrument, effects, sends): it is loaded through the Library's
  search, whose result rows take a selection by Accessibility (its browser's items take only a click),
  and only Logic's own undo takes it back.
- **Smart Controls and a send's level move by steps**: a set moves a Smart Control a hundredth and a
  send's knob one fine step toward the value asked, an increment a tenth or a coarse step. A send's
  fine step is 0.1 dB above -6 dB and a whole dB below, its increments 1 dB near 0 dB and 10 dB lower
  down (so they pass the level and come back), so a level is reached within half a fine step (−6.5
  asked is −6.1, −20 is −20.1) and answered as Logic shows it. Several sends' knobs are listed after all
  the send groups, in their order (a lone send's right after its group), and are paired by order.
- **Smart Controls are listed by where the pane shows them** (by rows, then from the left, from their
  frames): Accessibility lists the pane's sliders in an order of its own, which changes from one showing
  of the pane to the next.
- **Automation is read and written through the Track Automation Event List**, a window of its own
  (each press of its key command opens another one, so the bridge closes the one it opened), whose rows
  are Logic's "Fader" events numbered by Logic's own parameter numbers (Num: Volume is 7), with the
  parameter's name in the Length/Info cell and the value from 0 to 127 (no dB text). An event is made
  at the playhead, which the LCD places on a beat, so events take whole beats; a parameter asked by
  name is found by stepping the new row's Num up from 0 until the cell names it (at most 127). A
  change of an event's place or value is the event taken out and made again. None of it moves the
  epoch; each event is a step of Logic's undo ("Create Event", "Value Change", "Delete").
- A **bounce** goes through Logic's own Bounce dialog: its pop-up menus are standard ones read through
  the pop-up, but a menu opened just after another closed may not be up yet, so it is looked for again
  and pressed once more. An **export holds Logic's main thread** while it writes: the helper's reads
  time out, which the bridge takes as busy while it waits for the files. The export's options (file
  names, format) are the producer's: on this Mac the pattern names the files by number ("01.wav"), and
  a track with no sound writes none.
- The **Library's search results** sit six levels down its pane and their names three more, deeper
  than one read of the helper goes (eight): the table is found first, then read.
- The **cycle** (`loop-range`) is written through the LCD's Custom display mode, whose locator
  sliders sit on Logic's ticks; it is no step of Logic's undo, and an off cycle keeps its locators,
  which a restore puts back too.
- Logic's **quantize is a parameter** of the events, not a move: `notes-quantize` sets the Event
  List's Quantize pop-up for every event of the region, the rows then read (and Logic plays) the
  notes on the grid while their recorded positions stay underneath, and the restore sets the
  pop-up back so they read as recorded again, exactly. The grid is one of the pop-up's note values
  or triplets; there is no strength (100 only).
- The Event List applies a **value change to every selected event** when the edited one is among
  them: a note row selected with others is selected alone before a write (`notes-change`,
  `notes-delete`; the quantize's selection is put back after).
- A region's **edges move on whole beats** (`region-resize`: the playhead placed by the LCD's bar
  and beat, then Logic's Set Region Start/End to Playhead); a split and a delete need the region
  clicked (a key command acts on the keyboard focus, which a set of `AXSelected` doesn't give).
- A **sample becomes a Sampler track by conversion**: Logic loads a sample into its Sampler from an audio region
  (Track > Convert Regions to New Sampler Track…), so `sample-load` imports the file onto an audio track first and
  converts the region Logic selected; Logic makes the software instrument track below that audio track, named after
  the instrument, with one zone and a MIDI region of its trigger note, and leaves the audio region muted where it
  was. Logic **copies the file into the project's package at once**, saved or not (`Media/Audio Files/<file>` for the
  import, `Media/Samples/<instrument>/<file>` for the zone), and its undo leaves the copies: the kind was measured on a
  copy of Scratch, never on Scratch, whose package would keep them. A restore is Logic's own undo of the conversion
  while it is on top, then of the import right under it ("Insert"), which takes the new track and the audio region away;
  both are read back (the Sampler track's header gone, and the audio region the import made, by its track and bounds),
  and a step that didn't land is `uncertain`, named ("its “Undo Insert” didn't land: the audio region of “kumi-tone” is
  still on “Vocal”").
- A **MIDI import makes a track**: File > Import > MIDI File… lands the file on a new track below
  the one named ("Studio Grand"), so `created.trackAdded` is `true` and the import's undo takes the
  track away with the region.
- A **tempo event** made from outside is two steps of Logic's undo ("Insert Tempo", "Value
  Change"); the Tempo and Signature lists rebuild a cell with every step, so the bridge reads a step
  back at the cell's path. A signature row has no Delete action: a row Kumi made goes out through
  the list's Edit > Delete, selected. Both lists take whole beats for a position.
- A **Track Stack** is made by Logic's own key commands (Create Summing/Folder Stack) on the
  selected headers, never through the dialog; Logic names it (Sum 1, Sub 1) and a `name` renames it
  after, which puts "Renaming" on top of the creation. A new folder stack shows closed and is opened
  (Open Track Stack) to be read; flattening a summing stack, and undoing its creation, leaves its
  subtracks' output on the stack's bus (Logic's own doing). Which tracks are under a stack isn't
  readable, so a flattened stack comes back only through Logic's undo.
- A **key command** runs only with Logic in front; `key_command` takes a name or an id, resolved from
  the producer's key set (`com.apple.logic10.plist`, their chosen preset) with the factory U.S. set
  behind it, and refuses an unbound command.
- `track-add` places a track **below the selected one** (Logic's New Tracks behaviour), so `index`
  counts from 1 (0, above every track, is unreachable); a kind (`audio`, `instrument`, `external`)
  may be given.

## Listening (`hear`, `audition`, `render`)

M5 lets Kumi hear Logic. The three are methods of the bridge, queued on the run loop as the changes are. None is a
change, a row or a step of Logic's undo, and each leaves Logic as it found it: stopped, its playhead on the bar and
beat it was on (on the beat), every track's solo, the metronome and the cycle as they were. They are put back whatever
happens, a pass that fails included, and what couldn't be is listed under `notPutBack`. Logic playing or recording is
`busy` ("Kumi listens only while it is stopped"), a dialog up `busy`, a missing Accessibility grant `failed` in words,
as for every method. Audio never travels on the wire: each answer names a file in the request's `folder`, a whole path
to a folder that is there (Kumi makes no folders and never writes over a file: `kumi-hear-<n>.wav`,
`kumi-audition-<n>-<k>.wav`, `kumi-render-<n>.wav`, numbered past those there). Kumi never hears the metronome: the
control bar's Metronome Click is turned off for the pass when it is on, read back, and turned on again after
(`metronome: "off for the pass"`; `"off"` when it was off). The metronome's state is a setting Logic saves with the
project, so turning it off and on again, which every run of passes does (its clicks turn it on), marks the project
edited though nothing in it changed: the producer's next close of it asks to save (moving the playhead leaves it as it
was; measured on a copy of Scratch).

| Method | Params | Answer |
| --- | --- | --- |
| `hear` | `fromBeats`, `beats` (at least 0.25, at most 600 s at the project's tempo), `folder`, `mute?` (false), `solo?` (track refs soloed alone for the pass; none: the whole mix, no track soloed), `epoch?` | `{file, seconds, sampleRate, channels, lead: 0, peak, rms, aligned: {by, latencyMs, errorMs}, fromBeats, beats, mute, solo, metronome, epoch, note?, notPutBack?}` |
| `audition` | `candidates` (1 to 16 of `{ref}`, a track soloed alone, or `{mix: true}`, the whole mix), `fromBeats`, `beats`, `folder`, `mute?`, `epoch?` | `{candidates: [{ref` or `mix: true, name, file, seconds, sampleRate, channels, lead: 0, peak, rms, note?}], aligned, fromBeats, beats, mute, metronome, epoch, notPutBack?}`: the candidates in their order, heard one pass each in one capture |
| `render` | `ref?` (a track soloed alone for the bounce; none: the whole mix), `fromBeats`, `beats`, `folder`, `epoch?` | `{file, seconds, sampleRate, channels, bits: 32, lead: 0, fromBeats, beats, metronome, epoch, ref?, note?, notPutBack?}` |

`peak` and `rms` are linear over every sample of the file (1.0 is full scale); `seconds`, `sampleRate` and `channels`
are the file's. A pass is captured as 32-bit float at the default output device's rate and in stereo (44.1 kHz on the
lab's Mac), and a render is Logic's bounce at the project's rate, 32-bit float. `lead` is 0: the file starts on the
part's first beat. A track soloed that the producer muted stays silent, and its take's `note` says so; a play that
stopped short of the part's end (the producer pressed Stop) keeps what was played, and `note` says where Logic
stopped. Refs are this epoch's (`stale` otherwise), and a request's `epoch`, when it gives one, must be the project's.

### A run of passes

1. Logic read as it stands: the transport (`busy` while it plays), every header's solo and mute, the Metronome Click,
   the cycle, the playhead, the regions (where the song ends).
2. The cycle off, the metronome off, the helper's tap started on Logic's pid into `kumi-tap-<n>.wav` under `folder`
   (muted for the producer when `mute`), for at most the passes' length and a margin.
3. For each pass: exactly the tracks it asks for soloed, through the Mackie Control's solo buttons when its strips
   line up with the tracks (the LCD naming the first eight in order, as Logic last wrote the names), else each
   header's Solo checkbox (a click with Logic in front), read back from the headers; the playhead placed exactly on the
   part's first whole beat (a part from a fraction of a beat plays from the beat before and is cut where it starts);
   Play; the playhead followed until it is 150 ms past the part's end, eight or more of the display's updates seen;
   Stop.
4. The clicks: the playhead a bar past the song's end, where nothing plays, the metronome on, Play for a beat and a
   little more (two clicks), Stop.
5. The tap stopped, Logic put back (stopped, the solos, the metronome, the playhead on its bar and beat, then the
   cycle, whose left locator the Stop button would go to), each pass's part cut out of the capture into a file of its
   own, and the capture deleted.

### Following the playhead, and how a part is cut

The playhead is followed by the Mackie Control's timecode display. While Logic plays, it sends the digits that
changed, about every 33 ms, a CoreMIDI packet each, the most significant first; the helper stamps each packet with the
host time it came (Logic's own packets carry a time stamp of 0). A run of descending digits is one update, dated by its
first packet, and while the surface's PLAY and BEATS LEDs are lit, each update is a position (bar, beat, division,
tick). A play's positions lie on a line of host time against ticks (960 a quarter note; the division's ticks found as
the divisor that makes them a line) to within 0.4 ms, with an update now and then a few milliseconds late, which the
fit leaves out. When the surface sends nothing within 1.5 s of Play (no surface, or its display on SMPTE time), the
control bar's bar and beat are polled instead (`by: "transport"`), each change of beat dated at the middle of the two
polls around it, good to half their spacing.

The tap stamps its frames with the same clock (its anchors), so a beat's host time is a frame of the capture. What the
timecode can't say is how late the tap hears what Logic's display shows: on the lab's Mac that latency changed from one
tap to the next, between 9 and 21.5 ms over 40 taps (within one tap, about a millisecond from one play to the next),
which no fixed number covers within 10 ms. So each run measures it with step 4's clicks, which land the latency after
their beats where nothing else plays, and every part of the capture is cut by it (`by: "timecode+click"`,
`latencyMs`). When the clicks can't be heard (a metronome set silent), 15 ms is taken and `errorMs` is 10 (`by:
"timecode"`). `errorMs` is the cut's uncertainty: the two clicks' agreement, the lines' spread, and what a play varies
by within a tap (1.5 ms).

Measured on Scratch at 160 BPM, each file read by the recorder before it was deleted: over the six hears of bar 1 in
`listen-align` and the two of `listen-hear`, the Piano region's four notes began 0.4 to 1.8 ms after where the beats
say in the file (32 notes; the latency the clicks measured was 20.1 to 21.2 ms, `errorMs` 1.7 to 1.8). In Logic's own
offline bounce of the part (`listen-render`, Keys alone) the same notes begin 0.2 to 0.9 ms after their beats (the
piano's attack), so a heard part is cut within about a millisecond of where the bounce puts its beats; in the
audition's takes the notes began 0.5 ms before to 0.7 ms after their beats. Before the clicks, the lab cut by the
timecode with a fixed latency: the tap's latency moved between 9 and 21.5 ms from tap to tap, so a fixed one missed by
up to 11 ms; measured by clicks in another play of the same tap, the notes landed within 2 ms.

### Render

`render` is M4b's `bounce` (above) over the part: 32-bit float WAV, Normalize off, Add to Project off, the metronome
off (a bounce holds the click otherwise) and the solos the request asks for. A track's own audio is that track soloed
alone, which then passes the Stereo Out's plug-ins (its `note` says so). The solos and the metronome are put back.

### Permission

The tap needed no prompt on the lab's Mac (macOS 26.6.1): the helper runs under the terminal the bridge runs in, whose
Accessibility grant it uses, and System Audio Recording was never asked for. Where macOS refuses the tap, the helper's
`tap.start` answers `denied`, and `hear` and `audition` answer `failed`, saying to allow System Audio Recording for the
app Kumi runs in (System Settings > Privacy & Security > Screen & System Audio Recording); before macOS 14.2, `failed`
says listening needs 14.2.

### How long it takes, and what the host's call must allow

Measured on Scratch (160 BPM, the parts 1.5 s long): a `hear` took 4.3 to 4.9 s, so a run's overhead is 2.8 to 3.4 s
(about a second of it the clicks); an `audition` of three candidates took 9.7 s, each pass after the first adding its
part and about 1.2 s; a `render` of Keys alone took 8.2 s and one of four bars of the mix (6 s of audio) 6.4 s, most of
it the Bounce dialog and the save panel.

The bridge waits for a listening request as long as its part plays and a margin, at the project's tempo as the focus
last read it (60 BPM when it has none): `hear` the part's seconds and 28 s, `audition` each candidate's part and 8 s,
and 20 s more, `render` the part and 45 s (`listen::bound_seconds`), at most two hours whatever a deadline says; a
request that gives its own deadline is waited for until then. The host's call must allow the part's length and a
margin too: a 1.5 s part is answered in about 5 s, so the host's 30 s change timeout covers parts up to about 20 s, and
a longer part (or an audition of several) needs the bound's length.

### A region's audio file

Logic shows an audio region's file by name only, never its path. Imported into a copy of Scratch (a tone onto Vocal),
the region's item names the region ("import-tone") and its help text says where it starts and ends and that it is an
audio region; the Region inspector names the region; the Browsers pane's Project tab lists the file by name
("import-tone.wav"), its regions under it, in text fields that carry no path, URL or document. So a region row has no
`file` field, and the runtime's `audio_file` has nothing to read for Logic: Kumi hears a region by `hear` or `render` of
its track over the region's bars instead.
