# Logic Pro M4 gate (first slice), 2026-10-21

Status: passed. Every automatic check against the real Logic Pro passed, 42 of 42: the M2 and M3 checks again and
the first slice of M4, each change read back and put back, the project reading as before at the end.

Scope: `feat/logic` at f5854326: the bridge's first slice of M4 over the real Logic Pro (fifteen kinds beyond M3's
sixteen), Kumi's host journalling them, and Kumi's runtime and app putting them into HISTORY and back out; the base
branch (`origin/feat/reaper-renoise`, REAPER's and Renoise's M4 and the M8 release pipeline) merged first (f0dc702e).
Paths in this record show the Mac's home directory as `/Users/me`; the project ids, hashes of the paths, are over that.

| | |
| --- | --- |
| Logic Pro | 12.3.1, `com.apple.logic10`, `~/Music/Logic/Scratch.logicx` as recorded for M2 (each run saves it as a copy first and works on the copy) |
| macOS | 26.6.1, Apple Silicon |
| Bridge | `logic-kumi` 0.1.0 with the helper `kumi-logic-hands-bf604b889e95` (unchanged since M2) |
| Run | `target/debug/examples/accept_logic --kumi-home <scratch home> --files <folder> --no-operator --report <file>`, the bridge set up into the scratch home with `kumi --daw logic bridge --yes` first |

## What Kumi can change in Logic now, and through what

Fifteen kinds more, each through one channel and read back before Kumi answers `applied`:

| Kind | Channel | Readback | Logic's own undo |
| --- | --- | --- | --- |
| save, save as (`save_project`) | Logic's Save key command; File > Save As… through macOS's panel service (the go-to-folder sheet, the name, Save) | the window's document and its edited flag | none |
| a key command by name or id (`key_command`) | the chord from the producer's key set (`com.apple.logic10.plist`, their preset) or the factory U.S. set, posted with Logic in front | the Edit menu's Undo title, moved or not | the command's own, or none |
| region rename | the Region inspector's name field, confirmed | the Tracks area | "Renaming" |
| region move | the Event List at its region level, the Position scrubber | the Tracks area | "Value Change" |
| region resize | the playhead at the new edge, Set Region Start/End to Playhead | the Tracks area (whole beats) | "Set Start to Playhead Position", "Set End to Playhead Position" |
| region split | the region clicked, the playhead at the split, Split at Playhead | the Tracks area: the right part comes back as `created` | "Split Regions" |
| region loop | the Region inspector's Loop checkbox | the checkbox | "Loop Parameter Change" |
| region delete | the region clicked, the Delete key | the Tracks area | "Delete" |
| the cycle (`loop_range`) | the LCD on its Custom display, the two locator groups' sliders, the Cycle button | the locators and the button | no step |
| quantize (`notes_quantize`) | the Event List: Select > All, the Quantize pop-up (Logic's quantize parameter, non-destructive) | the rows, on the grid | "Quantize" |
| MIDI import (`midi_import`) | a format 0 MIDI file Kumi writes, File > Import > MIDI File… through the panel service | the new region and the track Logic adds for it | "Import" |
| tempo event set, deleted | the Tempo List: Create new Tempo Event at the playhead, the Tempo cell's segments; the row's Delete | the list | "Insert Tempo" + "Value Change"; "Delete" |
| time signature set | the Signature List: Create new Time Signature Change, the numerator slider, the denominator pop-up | the list | "Value Change" |
| Track Stack made, flattened | the headers clicked, Create Summing/Folder Stack; Flatten Stack | the headers (the stack's disclosure triangle) | "Create Track Stack"; "Flatten Stack" |

Not possible from outside, and said so by the bridge: a track's colour (the Color palette's swatches are bare positions
to Accessibility and a header shows no colour: nothing to read back) and a track moved (no key command reorders tracks,
a header offers no action but its menu); as before, a marker's name, plug-ins inserted, moved, deleted or tuned, and
sends.

## The bridge's numbers on the real Logic (apply / restore, ms, from the recorded scenes)

| Kind | Apply | Restore |
| --- | --- | --- |
| key command | 1026, 984 | Logic's own |
| region rename | 885 | 855 |
| region move | 1837 | 1817 |
| region resize | 1488 (end), 1371 (start) | 1642, 1274 |
| region split | 1494 | 958 (Logic's undo) |
| region loop | 1033 | 1001 |
| region delete | 1192 | 966 (Logic's undo) |
| cycle | 5874 (preview 2371) | 5827 |
| quantize | 3562 | 3562 |
| MIDI import, 4 notes | 10018 | 973 (Logic's undo) |
| tempo event made, changed | 2284, 1255 | 1183, 1238 |
| tempo event deleted | 1180 | 1398 (Logic's undo) |
| time signature | 2270 | 1723 |
| Track Stack, summing; folder with a name | 2276; 3178 | 1868 (Logic's undo); 1530 (flattened) |
| Track Stack flattened | 1793 | 1884 (Logic's undo) |
| save as; save in place; nothing to save | 3239; 308; 15 | |

A plan of a split, the cycle, a tempo event, an import of 64 notes and a save as ran as one step each of Logic's own
undo (the cycle none); four presses of Cmd-Z in Logic afterwards took back the import, the tempo event's two steps
and the split, each named back to Kumi.

## The runs

The passing run (the seventh) started from `Scratch.logicx` as recorded, saved it as a copy under the files folder,
and worked on the copy: a save as, a key command Logic keeps no step of (no row) and one it does (a row only Logic's
own undo takes back, Kumi's undo refused and the row kept, the marker it made taken away through Kumi), a region split
and its undo through Logic's own, a MIDI import of 64 notes (the Event List listed the 33 rows on screen; the region
spans the 16 beats) and its undo, the cycle set and cleared, a tempo event in the Tempo List set and deleted (read back
from the list), then Kumi's undo of the cycle and the Tempo List, last first; the project read as before.

| Check | Result | Time |
| --- | --- | --- |
| status: Logic Pro  (pid 78014, com.apple.logic10), bridge 0.1.0, project “Scratch”, busy false, permission null | ok | 115 ms |
| discover: 1 Logic Pro instance(s) | ok | 12 ms |
| focus over 50 calls: median 1 ms, worst 1 ms (under 150 ms); selection 1:track:1, editor none, ageMs 46 | ok | 1 ms |
| first look: “Logic Pro — Scratch”, 2 tracks, 1 markers, selection track:1, place logic-e805ab87b3ca9d66f0f62d47b97caa29 | ok | 3.5 s |
| open_set: /Users/me/Music/Logic/Scratch.logicx | ok |  |
| recording: Some(false) (from the focus's transport) | ok |  |
| read project: 1 rows (total 1, epoch "1") | ok | 61 ms |
| read track: 2 rows (total 2, epoch "1") | ok | 33 ms |
| read region: 1 rows (total 1, epoch "1") | ok | 34 ms |
| read note: 4 rows (total 4, epoch "1") | ok | 724 ms |
| read plugin: 1 rows (total 1, epoch "1") | ok | 60 ms |
| read send: 0 rows (total 0, epoch "1") | ok | 42 ms |
| read marker: 1 rows (total 1, epoch "1") | ok | 35 ms |
| read tempo: 1 rows (total 1, epoch "1") | ok | 2.5 s |
| body tree: Tree(DeviceTree { track_ref: "1:track:1", devices: [DeviceNode { ref: "1:plugin:1.1", name: "Channel EQ", class_name: None, can_have_chains: None, can_have_drum… | ok | 63 ms |
| body notes: Notes(ClipView { slot_ref: "1:region:1", name: "Keys", length: 16.0, notes: [ClipViewNote { note: ClipNote { pitch: 60.0, start: 0.0, duration: 0.25, velocity: … | ok | 760 ms |
| body timeline: Timeline(ArrangementStrip { length: 16.0, position: 0.0, playing: false, loop: None, locators: [Locator { name: "Marker 1", position: 32.0 }], beats_per_bar: So… | ok | 62 ms |
| body routing: Routing(RoutingView { track_ref: "1:track:1", routes: [Route { kind: Receive, other: "Input 1", volume_db: None, muted: false, tap: None }, Route { kind: Hardwa… | ok | 735 ms |
| a plan of 4 changes: 4 in HISTORY (“Vocal volume −6 dB” applied; “Renamed track “Keys” → “Keys (Kumi)”” applied; “Added instrument track “Kumi Pad”” applied; “Kumi Pad muted” applied) | ok | 4.7 s |
| undo “Kumi Pad muted”: undone | ok | 695 ms |
| undo “Added instrument track “Kumi Pad””: undone | ok | 857 ms |
| undo “Renamed track “Keys” → “Keys (Kumi)””: undone | ok | 559 ms |
| undo “Vocal volume −6 dB”: undone | ok | 1.1 s |
| the project reads as before the plan: 2 tracks, “Vocal” at 0 dB, “Keys” | ok | 2.0 s |
| save as <files folder>/Scratch Kumi M4 20261021-124111.logicx: the project is the copy now (status and read) | ok | 3.9 s |
| key command Select All: no row (Logic Pro kept no step of it) | ok | 3.1 s |
| key command Create Marker at beat 24: 1 → 2 markers, row “Ran key command “Create Marker” (⌥')” applied | ok | 1.8 s |
| Kumi's undo of the key command is refused, the row kept: Kumi can't take back what a key command did: Logic Pro's own undo can (Cmd-Z in Logic Pro) | ok | 65 ms |
| the marker taken away through Kumi (marker_delete, not undone at the end): 1 markers again | ok | 4.6 s |
| region split “Keys” at beat 8: 1 → 2 regions, the right part region:4 | ok | 1.8 s |
| undo the split: 1 regions again (Put back with Logic Pro's own undo.) | ok | 1.1 s |
| MIDI import of 64 notes on “Keys” at beat 24: region region:6: the Event List listed 33 of 64 rows; the region spans 16 beats (read back in 91 ms), 1 → 2 regions | ok | 7.6 s |
| undo the import: 1 regions again (Put back with Logic Pro's own undo.) | ok | 1.1 s |
| cycle set, bar 1 to bar 5: the focus's transport says cycle true | ok | 7.9 s |
| cycle cleared: the focus's transport says cycle false | ok | 7.9 s |
| tempo 168 BPM at beat 24 in the Tempo List: set; the list shows 168 BPM there | ok | 5.4 s |
| the tempo change at beat 24 deleted: deleted; the list has no tempo event there | ok | 4.6 s |
| undo “Deleted tempo event at bar 7”: undone | ok | 2.2 s |
| undo “Tempo 168 BPM at bar 7”: undone | ok | 2.0 s |
| undo “Cycle off”: undone | ok | 5.5 s |
| undo “Cycle bar 1 to bar 5”: undone | ok | 5.5 s |
| the project reads as before M4: 2 tracks, 1 regions, 1 markers (before: 2 tracks, 1 regions, 1 markers) | ok |  |

## What the gate found, and what changed for it

The gate ran on the real Logic six times before the passing run (the first stopped at its own setup, before the
bridge was set up in its home); each miss was fixed in the code, never by easing a check.

- Run 2 (35 of 40). The key command Create Marker was taken for no step: the bridge read the Edit menu's title once,
  500 ms after the chord, before Logic had named the step. The bridge reads the title every 250 ms now, six times at
  most, until it moves (aa602e14). The cycle read back one write behind through the host's `focus`, which is answered
  from a cache the bridge's run loop refreshes on its tick, held up by a write of several seconds: a write refreshes the
  focus before its answer leaves now (aa602e14). The MIDI import of 64 notes read back 33: Logic's Event List lists
  only the rows on screen. The bridge says so (`applied.rows` beside `applied.notes`, with a note), and the acceptance
  run judges an import by the region's span and that count (6e58a8cd); reading a region's notes past the screen is open
  work for the bridge's reads.
- Between runs, a replay of every recorded scene through the simulated Logic found the bridge's resize checking each
  edge for the beat only when it reached it, so a whole-beat start with an end off the beat moved the start and then
  refused: both edges are checked before either moves (b84fa4b2). The acceptance run's two Tempo List checks read
  tempo events from the tempo row, which holds the project's tempo and signatures only, so they passed without reading
  anything; they read the list through a delete previewed at the beat now (b84fa4b2).
- Run 3 (39 of 43). After its save as the acceptance run ran key commands as `make_changes` steps from the observation
  taken before it, which the runtime refuses as made for the project before it changed: it observes again once the copy
  is the project (a7ef7279). Create Marker made its marker, but the marker read right after was answered from the
  bridge's two-second list cache, which the marker kinds clear after a write and a key command didn't: a key command
  forgets every read of the project once its chord is posted (a7ef7279).
- Run 4 (15 of 32) met Logic's window narrowed to 865 points while the producer used the Mac, with the List Editors
  and the control bar's Cycle button off screen, so the bridge said so for every read and change that needs them. Not a
  code change: the runs check the window first now.
- Run 5 (25 of 35, every M4 miss after the first). The save as lost one read the helper didn't answer within two
  seconds while the save panel came up (two lanes were building on the Mac, load average over 100), and gave up with
  the panel still in front of the project, so every change after it was refused as Logic being busy. The panel is
  looked for again until its four seconds are up, and a save as or an import that can't find its panel closes it with
  Escape before answering (6b97e064). The runs wait for a calm Mac now.
- Run 6 (41 of 43). The marker Create Marker made was found by its place in the read but not in the look, which the
  acceptance run searched by matching the look's short ref against the read's long one; a short ref is counted over
  the session, not taken from the marker's place, so the marker was never found and was left for Cmd-Z in Logic. The
  run finds it in the look by its place too (f5854326). The window had been minimized while the producer was away from
  the Mac: the runs show it for the run and minimize it again after.
- Scratch.logicx on disk is as recorded: every run saved the project as a copy under the files folder first and worked
  on the copy.
