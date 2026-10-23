# Logic Pro M6 gate (catch-up, watch_me, instant phrases, curves, the library), 2026-10-21

Status: passed. Every automatic check against the real Logic Pro passed on the third run, 95 of 95: the M2 to M5 checks
again and M6, each change read back and put back, the project reading as before at the end.

Scope: `feat/logic` at c6a3d631: the bridge's M6 kinds and reads over the real Logic Pro (the metronome, a sample into
a Sampler track, the recent projects, middle C, the Tempo List on the tempo row, the bulk snapshot), Kumi's catch-up and
watch_me, instant phrases, the curves of Channel EQ and the Compressor in HISTORY, and Logic's library. Paths in this
record show the Mac's home directory as `/Users/me`; the project ids, hashes of the paths, are over that.

| | |
| --- | --- |
| Logic Pro | 12.3.1, `com.apple.logic10`, `~/Music/Logic/Scratch.logicx` as recorded for M2 (each run saves it as a copy first and works on the copy) |
| macOS | 26.6.1, Apple Silicon |
| Bridge | `logic-kumi` 0.1.0 with the helper `kumi-logic-hands-cc8b21a51398` |
| Run | `target/debug/examples/accept_logic --kumi-home <scratch home> --files <folder> --no-operator --report <file>`, the bridge set up into the scratch home with `kumi --daw logic bridge --yes` first |

## What M6 brings, as the run checked it

- Fixed phrases ("tempo 164", "metronome off" and "on", "mute Vocal", "loop 1-4", "loop off") run through Kumi's instant
  path with no model, each one row in HISTORY that Kumi's undo puts back (about half a second each; a loop eight
  seconds, the control bar's locators stepped).
- `metronome_set` through the control bar's Metronome Click: no step of Logic's undo, read back, put back by value.
- `sample_load`: a 1 s sine Kumi wrote, imported on Vocal and converted into a new Sampler track named after the file
  (7.8 s); Kumi's undo pressed both of Logic's undo steps (the conversion, then the import) and the track and region
  counts came back. Logic copies the file into the project package at once, saved or not, and its undo leaves the
  copies: the run worked on a copy. Quick Sampler is refused, saying it loads a sample only by a drag.
- The recent projects from File > Open Recent, read without opening it; the open copy among them with its path.
- `middleC` on the project row (3 on this Mac, Logic's Display Middle C As read and never changed), and the session
  naming MIDI 60 as C3 from it.
- The bulk snapshot: the six kinds catch-up keeps in one answer in 2.8 s, against 4.9 s for the six paged reads, the
  tracks and markers the same.
- `watch_me` around a marker Kumi added at bar 7: the marker told as added and the one after it as renamed, in Logic's
  words.
- Curves: Channel EQ's Low Shelf Gain +3 dB on Vocal drew an EQ curve of 8 bands in HISTORY, and the Compressor's
  Threshold -17.5 → -24.0 dB on Keys drew a compressor's curve at 3.9:1, each from one read of the plug-in's Controls
  View after the change settled, each undone.
- Logic's library: 14 content folders, each there on this Mac, none of Live's; the recent projects the bridge named.
- Not checked by this run: catch-up as a project opens (the run's context keeps no project store; the snapshot it takes
  is the bulk read above, and the simulated Logic's tests cover the catch-up itself).

## The runs

| Check | Result | Time |
| --- | --- | --- |
| status: Logic Pro  (pid 78014, com.apple.logic10), bridge 0.1.0, project “Scratch”, busy false, permission null | ok | 825 ms |
| discover: 1 Logic Pro instance(s) | ok | 9 ms |
| focus over 50 calls: median 1 ms, worst 1 ms (under 150 ms); selection 1:track:1, editor none, ageMs 52 | ok | 1 ms |
| first look: “Logic Pro — Scratch”, 2 tracks, 1 markers, selection track:1, place logic-e805ab87b3ca9d66f0f62d47b97caa29 | ok | 3.6 s |
| open_set: /Users/me/Music/Logic/Scratch.logicx | ok |  |
| recording: Some(false) (from the focus's transport) | ok |  |
| read project: 1 rows (total 1, epoch "1") | ok | 60 ms |
| read track: 2 rows (total 2, epoch "1") | ok | 36 ms |
| read region: 1 rows (total 1, epoch "1") | ok | 35 ms |
| read note: 4 rows (total 4, epoch "1") | ok | 790 ms |
| read plugin: 1 rows (total 1, epoch "1") | ok | 72 ms |
| read send: 0 rows (total 0, epoch "1") | ok | 41 ms |
| read marker: 1 rows (total 1, epoch "1") | ok | 29 ms |
| read tempo: 1 rows (total 1, epoch "1") | ok | 2.7 s |
| body tree: Tree(DeviceTree { track_ref: "1:track:1", devices: [DeviceNode { ref: "1:plugin:1.1", name: "Channel EQ", class_name: None, can_have_chains: None, can_have_drum… | ok | 839 ms |
| body notes: Notes(ClipView { slot_ref: "1:region:1", name: "Keys", length: 16.0, notes: [ClipViewNote { note: ClipNote { pitch: 60.0, start: 0.0, duration: 0.25, velocity: … | ok | 843 ms |
| body timeline: Timeline(ArrangementStrip { length: 16.0, position: 0.0, playing: false, loop: None, locators: [Locator { name: "Marker 1", position: 32.0 }], beats_per_bar: So… | ok | 79 ms |
| body routing: Routing(RoutingView { track_ref: "1:track:1", routes: [Route { kind: Receive, other: "Input 1", volume_db: None, muted: false, tap: None }, Route { kind: Hardwa… | ok | 53 ms |
| a plan of 4 changes: 4 in HISTORY (“Vocal volume −6 dB” applied; “Renamed track “Keys” → “Keys (Kumi)”” applied; “Added instrument track “Kumi Pad”” applied; “Kumi Pad muted” applied) | ok | 4.8 s |
| undo “Kumi Pad muted”: undone | ok | 730 ms |
| undo “Added instrument track “Kumi Pad””: undone | ok | 874 ms |
| undo “Renamed track “Keys” → “Keys (Kumi)””: undone | ok | 569 ms |
| undo “Vocal volume −6 dB”: undone | ok | 1.0 s |
| the project reads as before the plan: 2 tracks, “Vocal” at 0 dB, “Keys” | ok | 2.1 s |
| save as <files folder>/Scratch Kumi M4 20261021-211639.logicx: the project is the copy now (status and read) | ok | 4.1 s |
| key command Select All: no row (Logic Pro kept no step of it) | ok | 3.1 s |
| key command Create Marker at beat 24: 1 → 2 markers, row “Ran key command “Create Marker” (⌥')” applied | ok | 1.8 s |
| Kumi's undo of the key command is refused, the row kept: Kumi can't take back what a key command did: Logic Pro's own undo can (Cmd-Z in Logic Pro) | ok | 74 ms |
| the marker taken away through Kumi (marker_delete, not undone at the end): 1 markers again | ok | 4.8 s |
| region split “Keys” at beat 8: 1 → 2 regions, the right part region:4 | ok | 1.8 s |
| undo the split: 1 regions again (Put back with Logic Pro's own undo.) | ok | 1.1 s |
| MIDI import of 64 notes on “Keys” at beat 24: region region:6: the Event List listed 33 of 64 rows; the region spans 16 beats (read back in 114 ms), 1 → 2 regions | ok | 12.5 s |
| undo the import: 1 regions again (Put back with Logic Pro's own undo.) | ok | 1.1 s |
| cycle set, bar 1 to bar 5: the focus's transport says cycle true | ok | 8.0 s |
| cycle cleared: the focus's transport says cycle false | ok | 7.9 s |
| tempo 168 BPM at beat 24 in the Tempo List: set; the list shows 168 BPM there | ok | 5.7 s |
| the tempo change at beat 24 deleted: deleted; the list has no tempo event there | ok | 4.8 s |
| undo “Deleted tempo event at bar 7”: undone | ok | 2.3 s |
| undo “Tempo 168 BPM at bar 7”: undone | ok | 2.1 s |
| undo “Cycle off”: undone | ok | 5.5 s |
| undo “Cycle bar 1 to bar 5”: undone | ok | 5.5 s |
| the project reads as before M4: 2 tracks, 1 regions, 1 markers (before: 2 tracks, 1 regions, 1 markers) | ok |  |
| read smart of track 2: 4 rows | ok | 1.3 s |
| read automation of track 1: 0 rows | ok | 1.7 s |
| a Gain inserted on the Stereo Out (output:1): plugin:2 | ok | 7.5 s |
| its Gain set to -3 dB: the Controls View reads -3.0 dB (6 parameters read) | ok | 11.2 s |
| a send from track 1 to Bus 1 at -12 dB: 1 send(s) on track 1, this one at -12 dB (Logic Pro made an aux for the bus) | ok | 6.8 s |
| patch Piano/Steinway Grand Piano on “Keys”: the strip holds Sampler | ok | 8.9 s |
| bounce of bars 1 to 5: <files folder>/Scratch Kumi M4b bars 1 to 5 20261021-211849.wav found, 6.00 s long (6.00 s at 160 BPM), no row | ok | 8.5 s |
| undo “Loaded the patch “Steinway Grand Piano” on “Keys””: undone | ok | 2.1 s |
| undo “Added send from “Vocal” to Bus 1”: undone | ok | 2.5 s |
| undo ““Gain”'s Gain → -3.0 dB on “Stereo Out””: undone | ok | 3.4 s |
| undo “Added “Gain” to “Stereo Out””: undone | ok | 4.5 s |
| the project reads as before the second slice: 2 tracks, 0 plug-ins on the Stereo Out (before: 2 tracks, 0) | ok |  |
| the listening tools: audition, judge, tune, groove, form, sound, render (hear true, audition true) | ok |  |
| hear bars 1 to 5: 6.000 s of the mix (6.00 s at 160 BPM; the file 6.000 s, 0.000 s before the part), -24.1 LUFS | ok | 9.3 s |
| judge: a run toward -21 LUFS starts: Listened to the mix, bars 1–4 / Loudness -24.1 LUFS (wants -21 LUFS ±0.5) · Punch (crest) 12.5 dB (wants no lower) ✓ · Pumping 14.1 dB (wants no higher) ✓ · Distortion at peaks -10.7 dB (wants no higher) ✓ · Clipped samples 0 (wants ≤ 0) ✓ … | ok | 10.1 s |
| “Keys” up 1 dB: {"changed":"Keys volume +1 dB (Logic Pro's own undo (Cmd-Z in Logic Pro) can't take this back; Kumi can)","change":"c17"} | ok |  |
| the round: Round 1 · loudness / change: Keys up 1 dB / Loudness -24.1 → -21 LUFS ✓ · Distortion at peaks -10.7 → -11.8 dB ✓ / kept: Loudness improved and nothing else got audibly worse / rebalanced: the Gain on the Stereo Out +2.1 dB / every item is within tolerance / 3 listens · 0:54 | ok | 43.4 s |
| a Gain at the end of the Stereo Out: its Gain reads +2.1 dB | ok |  |
| undo ““Gain”'s Gain → +2.1 dB on “Stereo Out””: undone | ok | 3.8 s |
| undo “Added “Gain” to “Stereo Out””: undone | ok | 4.5 s |
| undo “Keys volume +1 dB”: undone | ok | 824 ms |
| audition of tracks 1 and 2: track 1: silent; track 2: unpitched · attack 1143 ms | ok | 20.8 s |
| render of bars 1 to 5 of “Keys”: kumi-render-1.wav found, 6.00 s long (6.00 s at 160 BPM) | ok | 9.4 s |
| the project reads as before M5: 2 tracks, faders Vocal +0.0 dB, Keys +0.0 dB, 0 soloed, 0 plug-ins on the Stereo Out (before: 0) | ok |  |
| “tempo 164” through the instant path, no model: 1 HISTORY row(s) (“Tempo 164 BPM”); reads 164.0 BPM; Kumi said: Done: Tempo 164 BPM (Logic Pro's own undo (Cmd-Z in Logic Pro) can't take this back; Kumi can). | ok | 517 ms |
| undo “Tempo 164 BPM” through Kumi: undone; reads 160.0 BPM | ok | 473 ms |
| “metronome off” through the instant path, no model: 1 HISTORY row(s) (“Metronome off”); reads the click off; Kumi said: Done: Metronome off (Logic Pro's own undo (Cmd-Z in Logic Pro) can't take this back; Kumi can). | ok | 533 ms |
| “metronome on” through the instant path, no model: 1 HISTORY row(s) (“Metronome on”); reads the click on; Kumi said: Done: Metronome on (Logic Pro's own undo (Cmd-Z in Logic Pro) can't take this back; Kumi can). | ok | 528 ms |
| undo “Metronome on” through Kumi: undone; reads the click off | ok | 471 ms |
| undo “Metronome off” through Kumi: undone; reads the click on | ok | 479 ms |
| “mute Vocal” through the instant path, no model: 1 HISTORY row(s) (“Vocal muted”); reads “Vocal” muted; Kumi said: Done: Vocal muted (Logic Pro's own undo (Cmd-Z in Logic Pro) can't take this back; Kumi can). | ok | 705 ms |
| undo “Vocal muted” through Kumi: undone; reads “Vocal” not muted | ok | 690 ms |
| “loop 1-4” through the instant path, no model: 1 HISTORY row(s) (“Cycle bar 1 to bar 5”); reads the cycle on, beats 0.0 to 16.0 (the focus: cycle true); Kumi said: Done: Cycle bar 1 to bar 5 (Logic Pro's own undo (Cmd-Z in Logic Pro) can't take this back; Kumi can). | ok | 8.0 s |
| “loop off” through the instant path, no model: 1 HISTORY row(s) (“Cycle off”); reads the cycle off, beats 0.0 to 16.0 (the focus: cycle false); Kumi said: Done: Cycle off (Logic Pro's own undo (Cmd-Z in Logic Pro) can't take this back; Kumi can). | ok | 8.0 s |
| undo “Cycle off” through Kumi: undone; reads the cycle on, beats 0.0 to 16.0 (the focus: cycle true) | ok | 5.6 s |
| undo “Cycle bar 1 to bar 5” through Kumi: undone; reads the cycle off, beats 0.0 to 16.0 (the focus: cycle false) | ok | 5.7 s |
| metronome_set {on: false} as a make_changes step: 1 HISTORY row(s) (“Metronome off”); reads the click off | ok | 528 ms |
| undo “Metronome off” through Kumi: undone; reads the click on | ok | 476 ms |
| sample_load kind quick-sampler refused, nothing made: Logic Pro's bridge can't load a sample into Quick Sampler: Logic converts audio regions into a Sampler, Alchemy or Drum Machine Designer track (whose pad holds the sample in a Quick Sampler, inside a Track Stack), and Quick Sampler takes a sample by a drag onto its window; ask for kind sampler | ok | 38 ms |
| sample_load of a 1 s sine (Kumi M6 Sine.wav: 44.1 kHz, 16-bit, 1.00 s) on “Vocal” at beat 24: a new instrument track “Kumi M6 Sine”, its instrument Sampler; 2 → 3 tracks, 1 → 3 regions; 1 HISTORY row(s) (“Loaded “Kumi M6 Sine” into a new Sampler track”) | ok | 7.8 s |
| undo “Loaded “Kumi M6 Sine” into a new Sampler track” through Kumi, both of Logic Pro's undo steps (the conversion, then the import): undone; 2 tracks, 1 regions (before the load: 2, 1) (Put back with Logic Pro's own undo; the copies of the file stay in the project's folder.) | ok | 2.2 s |
| read recent: 7 projects Logic Pro opened lately, 1 with a path; the open one “Scratch Kumi M4 20261021-211639” (item 1) at <files folder>/Scratch Kumi M4 20261021-211639.logicx, the copy the save as made | ok | 29 ms |
| middle C: the project row says middleC 3 (Logic Pro's Display Middle C As, read and never changed); the integration takes octave 3, so the session names MIDI 60 C3 | ok |  |
| the bulk read, snapshot {rows: [project, track, region, plugin, marker, tempo]}: 11 rows in one answer in 2.8 s (epoch 13), the six paged reads 4.9 s (project 36 ms (1 rows), track 796 ms (2 rows), region 89 ms (1 rows), plugin 61 ms (5 rows), marker 1.3 s (1 rows), tempo 2.6 s (1 rows)); the tracks (2) are and the markers (1) are the paged reads', as are the other kinds' | ok | 2.8 s |
| watch_me: started, a marker added through Kumi at bar 7, stopped: {"watched":"6 s","changes":[{"changed":"marker","name":"Marker 2","renamedFrom":"Marker 1"},{"added":"marker","name":"Marker 1","position":"bar 7","length":0}]} | ok | 8.1 s |
| undo “Marker “Marker 1” at bar 7” through Kumi: undone; 1 markers (before: 1) | ok | 2.1 s |
| Channel EQ's Low Shelf Gain on “Vocal” 0.0 dB → +3.0 dB through fx_param, 1 HISTORY row(s): its record draws an EQ curve of 8 bands, band 2 (LowShelf) +0.0 → +3.0 dB (5.5 s after the change) | ok | 13.2 s |
| undo ““Channel EQ”'s Low Shelf Gain → +3.0 dB on “Vocal”” through Kumi: undone; Low Shelf Gain reads 0.0 dB (before: 0.0 dB) | ok | 5.7 s |
| Compressor's Threshold on “Keys” -17.5 dB → -24.0 dB through fx_param, 1 HISTORY row(s): its record draws a compressor's curve, its threshold -17.5 → -24.0 dB at 3.9:1 (4.6 s after the change) | ok | 12.4 s |
| undo ““Compressor”'s Threshold → -24.0 dB on “Keys”” through Kumi: undone; Threshold reads -17.5 dB (before: -17.5 dB) | ok | 5.1 s |
| Logic Pro's library sources: 14 folder(s), each there on this Mac, none of Live's: Samples (~/Music/Audio Music Apps/Samples); Untagged Loops (~/Music/Audio Music Apps/Untagged Loops); Sampler Instruments (~/Music/Audio Music Apps/Sampler Instruments); Patches (~/Music/Audio Music Apps/Patches); Channel Strip Settings (~/Music/Audio Music Apps/Channel Strip Settings); Plug-In Settings (~/Music/Audio Music Apps/Plug-In Settings); Apple Loops (/Library/Audio/Apple Loops/Apple); Logic Pro Samples (/Library/Application Support/Logic/Samples); Logic Pro EXS Factory Samples (/Library/Application Support/Logic/EXS Factory Samples); Logic Pro Alchemy Samples (/Library/Application Support/Logic/…
| the library's recent Logic Pro projects: 1 of the 1 the bridge named with a path (those still there), the open one among them | ok |  |
| the project reads as before M6: 2 tracks (Vocal, Keys), 1 markers, 160.0 BPM, the click on, the cycle off, beats 0.0 to 16.0 (the focus: cycle false) | ok |  |

## What the gate found, and what changed for it

- Run 1 (93 of 94). The Compressor's Controls View read nothing: its editor puts 44 controls at the plug-in window's top
  level, past the 40 the bridge read, so the window wasn't recognised as the plug-in's. The bridge reads a cut top level
  again whole, reads values shown with no control as their text, and the curve takes the names and texts Logic shows
  (7dee224a, 2e88cc5d). The acceptance lane's review also brought: one recent project marked open for the open
  project's file name (7790fd0c), a sample load's restore reading back that the import's audio region went too
  (e59b0baa), and a curve's read carried to where the plug-in is after a plan moved Logic's refs (92ab1a71).
- Run 2 (93 of 94). The M4 checks' MIDI import pressed the panel's Open while the panel was still reading the file, and
  the panel stayed up: the import presses again once Open is enabled, as the sample load does (c6a3d631).
- Scratch.logicx on disk is as recorded: each run saved the project as a copy under the files folder first and worked
  on the copy.
