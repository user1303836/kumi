# Logic Pro M4 gate (second slice), 2026-10-21

Status: passed. Every automatic check against the real Logic Pro passed, 54 of 54: the M2, M3 and M4 checks again and
the second slice of M4, each change read back and put back, the project reading as before at the end.

Scope: `feat/logic` at c62fce35: the bridge's second slice of M4 over the real Logic Pro (thirteen kinds and two methods
beyond the first slice's), Kumi's host journalling them, and Kumi's runtime and app putting them into HISTORY and back
out. Paths in this record show the Mac's home directory as `/Users/me`; the project ids, hashes of the paths, are over
that.

| | |
| --- | --- |
| Logic Pro | 12.3.1, `com.apple.logic10`, `~/Music/Logic/Scratch.logicx` as recorded for M2 (each run saves it as a copy first and works on the copy) |
| macOS | 26.6.1, Apple Silicon |
| Bridge | `logic-kumi` 0.1.0 with the helper `kumi-logic-hands-bf604b889e95` (unchanged since M2) |
| Run | `target/debug/examples/accept_logic --kumi-home <scratch home> --files <folder> --no-operator --report <file>`, the bridge set up into the scratch home with `kumi --daw logic bridge --yes` first |

## What Kumi can change in Logic now, and through what

Logic's own slot menus (an insert slot's, a send slot's) open with a held press, after which keys drive them: the
helper's press answers once the menu is open, and the bridge types and walks it.

| Kind | Channel | Readback | Logic's own undo |
| --- | --- | --- | --- |
| a plug-in added (`fx_add`) | the insert slot's menu, its search typed, Down, Return | the strip's slots | "Insert Plug-in in Channel Strip" |
| a plug-in deleted (`fx_delete`) | the slot's menu, No Plug-in (an instrument refused) | the strip's slots | "Remove Plug-in in Channel Strip" |
| a parameter, several (`fx_param`, `fx_params`) | the plug-in window's Controls View, each slider set until its text reads the number asked | the slider's text | none |
| a setting stepped (`fx_preset`) | the plug-in window's next and previous setting buttons | the window's setting name | the window's own undo |
| a patch (`patch_load`) | the Library's search, the result row selected | the strip's plug-ins | "Load Patch" |
| a send made, removed (`send_add`, `send_remove`) | the send slot's menu walked by arrows (Logic makes the aux), No Send | the strip's send slots | "Change Send in Channel Strip" |
| a send's level (`send_volume`) | the send's knob, stepped | the knob | none |
| a Smart Control (`smart_control_set`) | the Smart Controls pane of the selected track | the knob | none |
| automation (`automation_add`, `automation_change`, `automation_delete`) | the Track Automation Event List: Create new Event, cells stepped, the row's Delete | the list | "Value Change", "Delete" |
| a bounce (`bounce`, an action) | File > Bounce, Logic's dialog (its range, format and Normalize off; Add to Project and Add to Apple Music Library off), the save panel | the file on disk, its length | none |
| an export (`export_tracks`, an action) | File > Export > All Tracks as Audio Files, the dialog and the panel | the files on disk | none |

New reads: a plug-in's parameters (`param`, from the Controls View the read opens and closes), a track's Smart
Controls (`smart`; an audio track whose strip maps none reads no rows) and its automation (`automation`, the Track
Automation Event List's rows per parameter). Not possible from outside, and said so by the bridge: a send's mode (a mark
Logic draws itself in its menu, which Accessibility can't read) and a plug-in moved (Logic moves one only by dragging).
A bounce holds whatever Logic plays, the metronome's click included while Metronome Click is on.

## The bridge's numbers on the real Logic (apply / restore, from the recorded scenes)

| Kind | Apply | Restore |
| --- | --- | --- |
| plug-in added | 6.6 s | 4.3 s (by value) |
| plug-in deleted | 4.4 s | 2.3 s (Logic's undo) |
| one parameter, two | 5.4 s, 8.5 s | 3.7 s, 6.9 s (by value) |
| setting stepped | 2.5 s | 2.1 s (the window's undo) |
| send made | 5.1 to 6.0 s | 2.2 s (Logic's undo) |
| send level | 2.3 s | 1.9 s (by value) |
| send removed | 5.6 s | 2.2 s (Logic's undo) |
| Smart Control | 3.3 s | 3.3 s (by value) |
| patch | 6 s | 1.0 s (Logic's undo) |
| automation added (2 events), changed, deleted | 4.5 s, 3.3 s, 1.6 s | 2.2 s, 3.7 s, 2.5 s (by value) |
| bounce of 8 beats | 8.2 s | |
| export of every track | 12.0 s | |

## The runs

The passing run (the second) started from `Scratch.logicx` as recorded, saved it as a copy under the files folder and
worked on the copy: the M2, M3 and M4 checks as in the first slice's gate, then a software instrument track's Smart
Controls and the first track's automation read, a Gain added on the Stereo Out and its gain set to -3 dB in the
Controls View and read back, a send from track 1 to a new bus at -12 dB (Logic made the aux), a patch from the Library
on the instrument track, a bounce of bars 1 to 5 found on disk and its length checked, then Kumi's undo of the patch,
the send, the gain and the Gain, last first; the project read as before.

| Check | Result | Time |
| --- | --- | --- |
| status: Logic Pro  (pid 78014, com.apple.logic10), bridge 0.1.0, project “Scratch”, busy false, permission null | ok | 628 ms |
| discover: 1 Logic Pro instance(s) | ok | 10 ms |
| focus over 50 calls: median 0 ms, worst 1 ms (under 150 ms); selection 1:track:1, editor none, ageMs 104 | ok | 0 ms |
| first look: “Logic Pro — Scratch”, 2 tracks, 1 markers, selection track:1, place logic-e805ab87b3ca9d66f0f62d47b97caa29 | ok | 3.3 s |
| open_set: /Users/me/Music/Logic/Scratch.logicx | ok |  |
| recording: Some(false) (from the focus's transport) | ok |  |
| read project: 1 rows (total 1, epoch "1") | ok | 63 ms |
| read track: 2 rows (total 2, epoch "1") | ok | 47 ms |
| read region: 1 rows (total 1, epoch "1") | ok | 33 ms |
| read note: 4 rows (total 4, epoch "1") | ok | 728 ms |
| read plugin: 1 rows (total 1, epoch "1") | ok | 65 ms |
| read send: 0 rows (total 0, epoch "1") | ok | 43 ms |
| read marker: 1 rows (total 1, epoch "1") | ok | 32 ms |
| read tempo: 1 rows (total 1, epoch "1") | ok | 2.4 s |
| body tree: Tree(DeviceTree { track_ref: "1:track:1", devices: [DeviceNode { ref: "1:plugin:1.1", name: "Channel EQ", class_name: None, can_have_chains: None, can_have_drum… | ok | 66 ms |
| body notes: Notes(ClipView { slot_ref: "1:region:1", name: "Keys", length: 16.0, notes: [ClipViewNote { note: ClipNote { pitch: 60.0, start: 0.0, duration: 0.25, velocity: … | ok | 731 ms |
| body timeline: Timeline(ArrangementStrip { length: 16.0, position: 0.0, playing: false, loop: None, locators: [Locator { name: "Marker 1", position: 32.0 }], beats_per_bar: So… | ok | 65 ms |
| body routing: Routing(RoutingView { track_ref: "1:track:1", routes: [Route { kind: Receive, other: "Input 1", volume_db: None, muted: false, tap: None }, Route { kind: Hardwa… | ok | 697 ms |
| a plan of 4 changes: 4 in HISTORY (“Vocal volume −6 dB” applied; “Renamed track “Keys” → “Keys (Kumi)”” applied; “Added instrument track “Kumi Pad”” applied; “Kumi Pad muted” applied) | ok | 4.6 s |
| undo “Kumi Pad muted”: undone | ok | 693 ms |
| undo “Added instrument track “Kumi Pad””: undone | ok | 839 ms |
| undo “Renamed track “Keys” → “Keys (Kumi)””: undone | ok | 569 ms |
| undo “Vocal volume −6 dB”: undone | ok | 1.0 s |
| the project reads as before the plan: 2 tracks, “Vocal” at 0 dB, “Keys” | ok | 1.9 s |
| save as <files folder>/Scratch Kumi M4 20261021-142756.logicx: the project is the copy now (status and read) | ok | 3.8 s |
| key command Select All: no row (Logic Pro kept no step of it) | ok | 3.1 s |
| key command Create Marker at beat 24: 1 → 2 markers, row “Ran key command “Create Marker” (⌥')” applied | ok | 1.7 s |
| Kumi's undo of the key command is refused, the row kept: Kumi can't take back what a key command did: Logic Pro's own undo can (Cmd-Z in Logic Pro) | ok | 61 ms |
| the marker taken away through Kumi (marker_delete, not undone at the end): 1 markers again | ok | 4.7 s |
| region split “Keys” at beat 8: 1 → 2 regions, the right part region:4 | ok | 1.7 s |
| undo the split: 1 regions again (Put back with Logic Pro's own undo.) | ok | 1.0 s |
| MIDI import of 64 notes on “Keys” at beat 24: region region:6: the Event List listed 33 of 64 rows; the region spans 16 beats (read back in 90 ms), 1 → 2 regions | ok | 7.5 s |
| undo the import: 1 regions again (Put back with Logic Pro's own undo.) | ok | 1.1 s |
| cycle set, bar 1 to bar 5: the focus's transport says cycle true | ok | 7.9 s |
| cycle cleared: the focus's transport says cycle false | ok | 7.9 s |
| tempo 168 BPM at beat 24 in the Tempo List: set; the list shows 168 BPM there | ok | 5.5 s |
| the tempo change at beat 24 deleted: deleted; the list has no tempo event there | ok | 4.6 s |
| undo “Deleted tempo event at bar 7”: undone | ok | 2.3 s |
| undo “Tempo 168 BPM at bar 7”: undone | ok | 2.0 s |
| undo “Cycle off”: undone | ok | 5.5 s |
| undo “Cycle bar 1 to bar 5”: undone | ok | 5.5 s |
| the project reads as before M4: 2 tracks, 1 regions, 1 markers (before: 2 tracks, 1 regions, 1 markers) | ok |  |
| read smart of track 2: 4 rows | ok | 1.2 s |
| read automation of track 1: 0 rows | ok | 1.6 s |
| a Gain inserted on the Stereo Out (output:1): plugin:2 | ok | 7.3 s |
| its Gain set to -3 dB: the Controls View reads -3.0 dB (6 parameters read) | ok | 11.0 s |
| a send from track 1 to Bus 1 at -12 dB: 1 send(s) on track 1, this one at -12 dB (Logic Pro made an aux for the bus) | ok | 6.6 s |
| patch Piano/Steinway Grand Piano on “Keys”: the strip holds Sampler | ok | 8.7 s |
| bounce of bars 1 to 5: <files folder>/Scratch Kumi M4b bars 1 to 5 20261021-142958.wav found, 6.00 s long (6.00 s at 160 BPM), no row | ok | 9.2 s |
| undo “Loaded the patch “Steinway Grand Piano” on “Keys””: undone | ok | 2.1 s |
| undo “Added send from “Vocal” to Bus 1”: undone | ok | 2.3 s |
| undo ““Gain”'s Gain → -3.0 dB on “Stereo Out””: undone | ok | 3.2 s |
| undo “Added “Gain” to “Stereo Out””: undone | ok | 4.3 s |
| the project reads as before the second slice: 2 tracks, 0 plug-ins on the Stereo Out (before: 2 tracks, 0) | ok |  |

## What the gate found, and what changed for it

- Run 1 (52 of 55). The bounce left "Bounce Project" on top of Logic's undo, so Kumi's undo of the patch loaded before
  it, which only Logic's own undo takes back, was refused: the Bounce dialog had "Add to Project" checked, which puts
  the bounced file in the project. The bridge turns Add to Project and Add to Apple Music Library off before it
  bounces and reads them back (c62fce35); the dialog keeps those choices, as it keeps the format, so the producer's own
  next bounce opens with them off. The Gain asked for -3 dB read back -3.1 dB: the bridge's search for the slider's raw
  value stopped at the first text within a quarter of a unit; it goes on until a raw value reads the number exactly,
  and says the slider's step there when none does (833e4169). The Smart Controls read of an audio track whose strip
  maps none was an error: it reads no rows now (9e3f55c2), and the acceptance run reads the software instrument
  track's (a9ea8cb3).
- Scratch.logicx on disk is as recorded: each run saved the project as a copy under the files folder first and worked
  on the copy.
