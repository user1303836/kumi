# Logic Pro bridge contract (M2)

What `logic-kumi` answers over `kumi-daw/1`, and which macOS channel each answer comes from. The
wire, handshake, discovery files and lifecycle are daw-bridge's; this page is only Logic's shapes.
The original shapes were measured in the lab of 2026-10-18 (`docs/evidence/logic-lab-2026-10-18.md`)
and, for the rows, on the Scratch project in Logic Pro 12.3.1 during M2. The host replays the recorded JSON in `daws/own/daw-mcp-server/tests/vectors/logic-reads.json`;
the native fixture recordings live under `daws/logic/logic-kumi/tests/fixtures`; where the two
differ, the vectors are what Logic shows and this page says why.

This port exposes six wire methods: `ping`, `status`, `focus`, `read`, `snapshot`,
and `focus_body`. It offers reading only; changes, undo, listening, goals, scripts,
and catch-up are not enabled. The historical lab record is source evidence, not a
new acceptance result for this build. Run the opt-in `accept_logic` example with a
scratch project for current native acceptance.

Normal launch does not create MIDI ports. The explicit `logic-kumi --mcu` option
can install a Mackie Control surface in Logic; use it only for a deliberate lab
run and check Logic's Control Surfaces setup afterwards. The write, key, menu and
panel-service helper operations below are available to explicit lab programs,
not to the six reading methods.

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
