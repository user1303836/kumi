# Logic Pro M3 gate, 2026-10-19

Status: passed. Every automatic check (24 of 24) on the real Logic, and with the orchestrator at Logic as
the operator, 34 of 37: the three misses are the operator's, listed under the run.

Scope: `feat/logic` at d1f99a13 (the bridge; the acceptance run's own fixes follow in 13ab8324 and 92b3d297), the bridge changing the real Logic Pro through Accessibility and the
control bar and naming Logic's own undo back, Kumi's host journalling the changes, and Kumi's runtime
putting a plan into HISTORY and taking it back out as new steps. Paths in this record and in the
recordings show the Mac's home directory as `/Users/me`; the project ids, hashes of the paths, are over that.

| | |
| --- | --- |
| Logic Pro | 12.3.1, `com.apple.logic10`, `~/Music/Logic/Scratch.logicx` as recorded for M2 |
| macOS | 26.6.1, Apple Silicon |
| Bridge | `logic-kumi` 0.1.0 with the helper `kumi-logic-hands-bf604b889e95` (unchanged since M2) |
| Run | `cargo run -p kumi --example accept_logic -- --kumi-home <scratch home> --no-operator --report <file>` |

## What Kumi can change in Logic, and through what

Sixteen kinds, each through one channel and read back before Kumi answers `applied`:

| Kind | Channel | Readback | Logic's own undo |
| --- | --- | --- | --- |
| track volume, pan | the header's slider, stepped by ten and by one | the slider's text | no step |
| track mute, solo, arm | a click on the header's checkbox (presses don't take) | the checkbox | no step |
| track rename | the Inspector's name field, confirmed | the header's label | "Renaming" |
| track add | the Track menu's New … Track, below the selected track | the new header | "Create Track" |
| track delete | Track > Delete Track, its sheet answered | the headers | "Delete Tracks" |
| marker add, move, delete | the Marker List: Create, the position segments, the row's Delete | the list | "Create Marker", "Value Change", "Delete" |
| tempo | the control bar's tempo slider | the LCD | no step |
| notes add, change, delete | the Event List: Create new Event, the row's cells, the row's Delete | the rows | "Create Event", "Value Change", "Delete" |
| plug-in bypass | the Mixer strip's bypass checkbox | the slot | no step |
| play, stop, cursor | the control bar | the LEDs and the LCD | none |

Restores go by value wherever a value can be set (a fresh step of Logic's undo for the kinds that have
one); a deleted track comes back only through Logic's own undo while that step is on top. A restore's
ref is carried from the change's epoch to where the track stands now, so a fader or a rename is undone
after a track was added. Logic's own undo is named back to Kumi (`undone_in_daw`) from probes of the
values and from the Edit menu's Undo and Redo titles.

Not possible from outside, and said so by the bridge: the cycle locators (no field Accessibility can
set), a marker's name (its cell isn't a text field), inserting, moving, deleting or tuning a plug-in and
adding or setting a send (their pop-up menus can't be driven), moving a track (no drag).

## The bridge's numbers on the real Logic (apply / first readback, ms)

| Kind | Apply | Readback |
| --- | --- | --- |
| volume | 993 | 704 |
| pan | 437 | |
| mute | 697 | |
| solo | 633 | 708 |
| arm | 628 | 708 |
| rename | 1168 | 705 |
| track add | 1537 | 685 |
| track delete | 810 | 666 |
| marker add | 2868 | 1232 |
| marker move | 1792 | |
| marker delete | 2834 | 1230 |
| tempo | 428 | |
| notes add | 3863 | 718 |
| notes change | 1485 | 721 |
| notes delete | 1516 | 689 |
| bypass | 1038 | 702 |
| cursor | 130 to 220 | |
| play, stop | about 190 | |

The list kinds take seconds because each write opens a pane and steps scrubbers. Reading the Edit
menu's two titles: median 0.64 ms, worst about 33 ms; a forced validation about 170 ms, at most every
2 s and only while Logic is behind.

In the bridge's recorded plan (a fader, a rename, a track added, a mute), two presses of Cmd-Z in Logic
took back the track and the rename, both named back to Kumi as undone; the mute and the fader, which
Logic keeps no step for, were put back by value.

## The run

The run without an operator, 24 of 24:

| Check | Result | Time |
| --- | --- | --- |
| status: Logic Pro  (pid 14133, com.apple.logic10), bridge 0.1.0, project “Scratch”, busy false, permission null | ok | 109 ms |
| discover: 1 Logic Pro instance(s) | ok | 12 ms |
| focus over 50 calls: median 0 ms, worst 1 ms (under 150 ms); selection 1:track:2, editor none, ageMs 63 | ok | 0 ms |
| first look: “Logic Pro — Scratch”, 2 tracks, 1 markers, selection track:2, place logic-e805ab87b3ca9d66f0f62d47b97caa29 | ok | 3.4 s |
| open_set: /Users/me/Music/Logic/Scratch.logicx | ok |  |
| recording: Some(false) (from the focus's transport) | ok |  |
| read project: 1 rows (total 1, epoch "1") | ok | 50 ms |
| read track: 2 rows (total 2, epoch "1") | ok | 48 ms |
| read region: 1 rows (total 1, epoch "1") | ok | 43 ms |
| read note: 4 rows (total 4, epoch "1") | ok | 717 ms |
| read plugin: 4 rows (total 4, epoch "1") | ok | 50 ms |
| read send: 0 rows (total 0, epoch "1") | ok | 46 ms |
| read marker: 1 rows (total 1, epoch "1") | ok | 47 ms |
| read tempo: 1 rows (total 1, epoch "1") | ok | 2.4 s |
| body tree: Tree(DeviceTree { track_ref: "1:track:2", devices: [DeviceNode { ref: "1:plugin:2.1", name: "Piano", class_name: None, can_have_chains: None, can_have_drum_pads… | ok | 54 ms |
| body notes: Notes(ClipView { slot_ref: "1:region:1", name: "Keys", length: 16.0, notes: [ClipViewNote { note: ClipNote { pitch: 60.0, start: 0.0, duration: 0.25, velocity: … | ok | 729 ms |
| body timeline: Timeline(ArrangementStrip { length: 16.0, position: 0.0, playing: false, loop: None, locators: [Locator { name: "Marker 1", position: 32.0 }], beats_per_bar: So… | ok | 50 ms |
| body routing: Routing(RoutingView { track_ref: "1:track:2", routes: [Route { kind: Hardware, other: "Stereo Output", volume_db: None, muted: false, tap: None }], parent_send:… | ok | 724 ms |
| a plan of 4 changes: 4 in HISTORY (“Vocal volume −6 dB” applied; “Renamed track “Keys” → “Keys (Kumi)”” applied; “Added instrument track “Kumi Pad”” applied; “Kumi Pad muted” applied) | ok | 3.5 s |
| undo “Kumi Pad muted”: undone | ok | 673 ms |
| undo “Added instrument track “Kumi Pad””: undone | ok | 850 ms |
| undo “Renamed track “Keys” → “Keys (Kumi)””: undone | ok | 539 ms |
| undo “Vocal volume −6 dB”: undone | ok | 1.0 s |
| the project reads as before the plan: 2 tracks, “Vocal” at 0 dB, “Keys” | ok | 1.9 s |

The run with the operator (the orchestrator, through Accessibility), 34 of 37:

| Check | Result | Time |
| --- | --- | --- |
| status: Logic Pro  (pid 14133, com.apple.logic10), bridge 0.1.0, project “Scratch”, busy false, permission null | ok | 109 ms |
| discover: 1 Logic Pro instance(s) | ok | 12 ms |
| focus over 50 calls: median 0 ms, worst 1 ms (under 150 ms); selection 1:track:2, editor none, ageMs 53 | ok | 0 ms |
| first look: “Logic Pro — Scratch”, 2 tracks, 1 markers, selection track:2, place logic-e805ab87b3ca9d66f0f62d47b97caa29 | ok | 3.4 s |
| open_set: /Users/me/Music/Logic/Scratch.logicx | ok |  |
| recording: Some(false) (from the focus's transport) | ok |  |
| read project: 1 rows (total 1, epoch "1") | ok | 48 ms |
| read track: 2 rows (total 2, epoch "1") | ok | 47 ms |
| read region: 1 rows (total 1, epoch "1") | ok | 47 ms |
| read note: 4 rows (total 4, epoch "1") | ok | 730 ms |
| read plugin: 4 rows (total 4, epoch "1") | ok | 48 ms |
| read send: 0 rows (total 0, epoch "1") | ok | 46 ms |
| read marker: 1 rows (total 1, epoch "1") | ok | 47 ms |
| read tempo: 1 rows (total 1, epoch "1") | ok | 2.4 s |
| body tree: Tree(DeviceTree { track_ref: "1:track:2", devices: [DeviceNode { ref: "1:plugin:2.1", name: "Piano", class_name: None, can_have_chains: None, can_have_drum_pads… | ok | 50 ms |
| body notes: Notes(ClipView { slot_ref: "1:region:1", name: "Keys", length: 16.0, notes: [ClipViewNote { note: ClipNote { pitch: 60.0, start: 0.0, duration: 0.25, velocity: … | ok | 700 ms |
| body timeline: Timeline(ArrangementStrip { length: 16.0, position: 0.0, playing: false, loop: None, locators: [Locator { name: "Marker 1", position: 32.0 }], beats_per_bar: So… | ok | 49 ms |
| body routing: Routing(RoutingView { track_ref: "1:track:2", routes: [Route { kind: Hardware, other: "Stereo Output", volume_db: None, muted: false, tap: None }], parent_send:… | ok | 711 ms |
| a plan of 4 changes: 4 in HISTORY (“Vocal volume −6 dB” applied; “Renamed track “Keys” → “Keys (Kumi)”” applied; “Added instrument track “Kumi Pad”” applied; “Kumi Pad muted” applied) | ok | 3.5 s |
| undo “Kumi Pad muted”: undone | ok | 669 ms |
| undo “Added instrument track “Kumi Pad””: undone | ok | 833 ms |
| undo “Renamed track “Keys” → “Keys (Kumi)””: undone | ok | 541 ms |
| undo “Vocal volume −6 dB”: undone | ok | 1.0 s |
| the project reads as before the plan: 2 tracks, “Vocal” at 0 dB, “Keys” | ok | 1.9 s |
| the focus follows: 1:track:2 → 5:track:2 | ok | 1 ms |
| selection events: 0 ([]) | failed |  |
| status says busy: modal while the dialog is up | ok | 14.5 s |
| a look is refused as busy: Logic Pro is showing a dialog; nothing changed | ok | 17 ms |
| status says free again once the dialog is closed | ok | 9.1 s |
| the next look succeeds: 2 tracks | ok | 1.9 s |
| a change to take back in Logic Pro: “Renamed track “Vocal” → “Vocal (Kumi)”” | ok |  |
| Cmd-Z named back: undone_in_daw {"descs":["Kumi: Renamed track “Vocal” → “Vocal (Kumi)” (kv7g8)"],"how":"undo","epoch":"7","transactionIds":["t9dhcm"]} | ok | 19.0 s |
| HISTORY: “Renamed track “Vocal” → “Vocal (Kumi)”” undone (Undone with Logic Pro's own undo (Cmd-Z in Logic Pro).) | ok |  |
| the epoch didn't move (no track added or moved, or not seen) | failed | 90.0 s |
| project event: {"id":"b433ed3df55bdfedb59257e8757931b8","name":"Untitled 1","path":"/Users/me/Music/Logic/Untitled 1.logicx"} | ok | 57.0 s |
| the look after is of another place: logic-e805ab87b3ca9d66f0f62d47b97caa29 → logic-e805ab87b3ca9d66f0f62d47b97caa29; “Logic Pro — couldn't look just now: Logic Pro is showing a dialog; nothing changed” | failed | 5.1 s |
| open_set now: /Users/me/Music/Logic/Untitled 1.logicx | ok |  |

The three failed rows:

- selection events: an operator-side miss: the check passed on a ref whose epoch the plan had moved before any track was selected, so no selection was made; the example now compares places.
- the epoch didn't move: an operator-side miss: the scripted Option-Cmd-N added no track, so there was nothing for the epoch to move for.
- the look after is of another place: the project event came (the row above); the look a second later met the new project's own dialog and was refused as busy, which is the right answer.


## The test suite

The whole isolated suite on b2ced52b (`scripts/test-isolated.sh --no-fail-fast`): 3145 passed, 2 failed, 6 ignored in 336 test
binaries; the two failures are the timing-bound `hands_transport` and `style_references` tests of the M1 and M2 gates, in code
this branch doesn't change. The bridge crate's 160 tests, with the twenty recorded change scenes, passed again on d1f99a13.

## Not covered

- The two operator checks the operator missed: a track added by hand moving the epoch (the bridge's own track add moved it in
  the plan, and the recorded `change-track-add` scene covers the observer), and a look landing after the switched project's
  first dialog is answered.
- During the gate, Logic had to be restarted: the earlier bridge's Edit-menu validation had left a menu bar item selected,
  after which Logic took no input at all, and the restarted Logic came back with a half-size window whose control bar hid
  the List Editors button until the window was resized. d1f99a13 removes the validation press; the window size is Logic's.
- Only an English Logic was run. A "Kumi Logic" Mackie Control device stays in Logic's Control Surfaces > Setup, offline
  while no bridge runs, and `~/Music/Logic/Untitled 1.logicx` was created by Logic for the project-switch check.
