# Logic Pro M5 gate (listening), 2026-10-21

Status: passed. Every automatic check against the real Logic Pro passed on the first run, 66 of 66: the M2, M3 and M4
checks again and listening, each change read back and put back, the project reading as before at the end.

Scope: `feat/logic` at 43f456b8: the helper's process tap, the bridge's `hear`, `audition` and `render` over the real
Logic Pro, Kumi's host offering them, and Kumi's listening engine running over Logic through Logic's `Renderer`. Paths
in this record show the Mac's home directory as `/Users/me`; the project ids, hashes of the paths, are over that.

| | |
| --- | --- |
| Logic Pro | 12.3.1, `com.apple.logic10`, `~/Music/Logic/Scratch.logicx` as recorded for M2 (the run saves it as a copy first and works on the copy) |
| macOS | 26.6.1, Apple Silicon; the process tap asked for no permission on this Mac |
| Bridge | `logic-kumi` 0.1.0 with the helper `kumi-logic-hands-cc8b21a51398` (the process tap added) |
| Run | `target/debug/examples/accept_logic --kumi-home <scratch home> --files <folder> --no-operator --report <file>`, the bridge set up into the scratch home with `kumi --daw logic bridge --yes` first |

## How Kumi hears Logic

- The helper taps Logic's process with a Core Audio process tap on a private aggregate device and writes the frames
  itself to a WAV file (32-bit float, the output device's rate), stamped with host-time anchors; it mutes Logic only
  when asked.
- `hear` puts the playhead at the part, starts the tap, plays, follows the playhead by the Mackie Control's timecode
  (Logic sends its changed digits about every 33 ms; the helper stamps each packet as it arrives), stops, and cuts the
  capture to the part. The tap's lag behind the timecode changes from one tap to the next (9 to 21.5 ms over 40 taps),
  so each run ends by playing two metronome clicks a bar past the song's end and measures the lag on them: over 8
  hears the Piano's notes start 0.4 to 1.8 ms after their beats in the file, where Logic's own bounce puts them 0.2 to
  0.9 ms after.
- `audition` captures each candidate soloed alone in one pass, the producer hearing the first; `render` bounces a part
  offline through the bounce of M4 (32-bit float, a track soloed alone for its own audio, the Stereo Out's plug-ins in
  it).
- The metronome is off for each pass and put back, so a listen marks the project edited (the metronome is a project
  setting); Logic is put back even when a pass fails (stopped, the playhead, solos, the metronome, the cycle).
- Kumi's `Renderer` for Logic hears a track alone by soloing it for its own pass, and a judged round turns the level
  at the run's track or the Stereo Out: its last Adaptive Limiter or Limiter that's on, else a last Gain, else a Gain
  Kumi adds at the end, set through the Controls View as a change in HISTORY that Kumi's undo takes back.
- Not done: the goal search (`goal`, a `GoalRig`), and a region's audio file (Logic shows it by name only).

## Numbers on the real Logic (Scratch, 160 BPM)

| Pass | Time |
| --- | --- |
| hear, a 1.5 s part | 4.3 to 4.9 s (2.8 to 3.4 s over the part, about 1 s of it the clicks) |
| hear, bars 1 to 5 (6 s), through the whole stack | 9.3 s |
| audition, 3 candidates of 1.5 s | 9.7 s |
| render, one track over 4 beats; 4 bars of the mix | 8.2 s; 6.4 s |

## The run

The run (the first) started from `Scratch.logicx` as recorded, saved it as a copy under the files folder and worked on
the copy: the M2, M3 and M4 checks as in the M4 gates, then the listening tools offered, bars 1 to 5 heard (6.000 s,
-24.1 LUFS), a judge run toward -21 LUFS whose round raised Keys 1 dB and brought the level back with a Gain Kumi put
on the Stereo Out (+2.1 dB), every change of it undone through Kumi, tracks 1 and 2 auditioned (Vocal holds no audio),
a render of Keys over bars 1 to 5 found on disk, and the project reading as before.

| Check | Result | Time |
| --- | --- | --- |
| status: Logic Pro  (pid 78014, com.apple.logic10), bridge 0.1.0, project “Scratch”, busy false, permission null | ok | 515 ms |
| discover: 1 Logic Pro instance(s) | ok | 12 ms |
| focus over 50 calls: median 0 ms, worst 1 ms (under 150 ms); selection 1:track:1, editor none, ageMs 67 | ok | 0 ms |
| first look: “Logic Pro — Scratch”, 2 tracks, 1 markers, selection track:1, place logic-e805ab87b3ca9d66f0f62d47b97caa29 | ok | 3.4 s |
| open_set: /Users/me/Music/Logic/Scratch.logicx | ok |  |
| recording: Some(false) (from the focus's transport) | ok |  |
| read project: 1 rows (total 1, epoch "1") | ok | 56 ms |
| read track: 2 rows (total 2, epoch "1") | ok | 27 ms |
| read region: 1 rows (total 1, epoch "1") | ok | 35 ms |
| read note: 4 rows (total 4, epoch "1") | ok | 729 ms |
| read plugin: 1 rows (total 1, epoch "1") | ok | 53 ms |
| read send: 0 rows (total 0, epoch "1") | ok | 32 ms |
| read marker: 1 rows (total 1, epoch "1") | ok | 34 ms |
| read tempo: 1 rows (total 1, epoch "1") | ok | 2.5 s |
| body tree: Tree(DeviceTree { track_ref: "1:track:1", devices: [DeviceNode { ref: "1:plugin:1.1", name: "Channel EQ", class_name: None, can_have_chains: None, can_have_drum… | ok | 55 ms |
| body notes: Notes(ClipView { slot_ref: "1:region:1", name: "Keys", length: 16.0, notes: [ClipViewNote { note: ClipNote { pitch: 60.0, start: 0.0, duration: 0.25, velocity: … | ok | 715 ms |
| body timeline: Timeline(ArrangementStrip { length: 16.0, position: 0.0, playing: false, loop: None, locators: [Locator { name: "Marker 1", position: 32.0 }], beats_per_bar: So… | ok | 64 ms |
| body routing: Routing(RoutingView { track_ref: "1:track:1", routes: [Route { kind: Receive, other: "Input 1", volume_db: None, muted: false, tap: None }, Route { kind: Hardwa… | ok | 706 ms |
| a plan of 4 changes: 4 in HISTORY (“Vocal volume −6 dB” applied; “Renamed track “Keys” → “Keys (Kumi)”” applied; “Added instrument track “Kumi Pad”” applied; “Kumi Pad muted” applied) | ok | 4.6 s |
| undo “Kumi Pad muted”: undone | ok | 693 ms |
| undo “Added instrument track “Kumi Pad””: undone | ok | 863 ms |
| undo “Renamed track “Keys” → “Keys (Kumi)””: undone | ok | 553 ms |
| undo “Vocal volume −6 dB”: undone | ok | 1.1 s |
| the project reads as before the plan: 2 tracks, “Vocal” at 0 dB, “Keys” | ok | 1.9 s |
| save as <files folder>/Scratch Kumi M4 20261021-163215.logicx: the project is the copy now (status and read) | ok | 3.8 s |
| key command Select All: no row (Logic Pro kept no step of it) | ok | 3.1 s |
| key command Create Marker at beat 24: 1 → 2 markers, row “Ran key command “Create Marker” (⌥')” applied | ok | 1.7 s |
| Kumi's undo of the key command is refused, the row kept: Kumi can't take back what a key command did: Logic Pro's own undo can (Cmd-Z in Logic Pro) | ok | 58 ms |
| the marker taken away through Kumi (marker_delete, not undone at the end): 1 markers again | ok | 4.7 s |
| region split “Keys” at beat 8: 1 → 2 regions, the right part region:4 | ok | 1.8 s |
| undo the split: 1 regions again (Put back with Logic Pro's own undo.) | ok | 1.1 s |
| MIDI import of 64 notes on “Keys” at beat 24: region region:6: the Event List listed 33 of 64 rows; the region spans 16 beats (read back in 97 ms), 1 → 2 regions | ok | 7.9 s |
| undo the import: 1 regions again (Put back with Logic Pro's own undo.) | ok | 1.1 s |
| cycle set, bar 1 to bar 5: the focus's transport says cycle true | ok | 7.9 s |
| cycle cleared: the focus's transport says cycle false | ok | 7.9 s |
| tempo 168 BPM at beat 24 in the Tempo List: set; the list shows 168 BPM there | ok | 5.5 s |
| the tempo change at beat 24 deleted: deleted; the list has no tempo event there | ok | 4.7 s |
| undo “Deleted tempo event at bar 7”: undone | ok | 2.3 s |
| undo “Tempo 168 BPM at bar 7”: undone | ok | 2.1 s |
| undo “Cycle off”: undone | ok | 5.5 s |
| undo “Cycle bar 1 to bar 5”: undone | ok | 5.5 s |
| the project reads as before M4: 2 tracks, 1 regions, 1 markers (before: 2 tracks, 1 regions, 1 markers) | ok |  |
| read smart of track 2: 4 rows | ok | 1.2 s |
| read automation of track 1: 0 rows | ok | 1.6 s |
| a Gain inserted on the Stereo Out (output:1): plugin:2 | ok | 7.2 s |
| its Gain set to -3 dB: the Controls View reads -3.0 dB (6 parameters read) | ok | 11.0 s |
| a send from track 1 to Bus 1 at -12 dB: 1 send(s) on track 1, this one at -12 dB (Logic Pro made an aux for the bus) | ok | 6.8 s |
| patch Piano/Steinway Grand Piano on “Keys”: the strip holds Sampler | ok | 8.9 s |
| bounce of bars 1 to 5: <files folder>/Scratch Kumi M4b bars 1 to 5 20261021-163419.wav found, 6.00 s long (6.00 s at 160 BPM), no row | ok | 7.3 s |
| undo “Loaded the patch “Steinway Grand Piano” on “Keys””: undone | ok | 2.1 s |
| undo “Added send from “Vocal” to Bus 1”: undone | ok | 2.5 s |
| undo ““Gain”'s Gain → -3.0 dB on “Stereo Out””: undone | ok | 3.3 s |
| undo “Added “Gain” to “Stereo Out””: undone | ok | 4.4 s |
| the project reads as before the second slice: 2 tracks, 0 plug-ins on the Stereo Out (before: 2 tracks, 0) | ok |  |
| the listening tools: audition, judge, tune, groove, form, sound, render (hear true, audition true) | ok |  |
| hear bars 1 to 5: 6.000 s of the mix (6.00 s at 160 BPM; the file 6.000 s, 0.000 s before the part), -24.1 LUFS | ok | 9.3 s |
| judge: a run toward -21 LUFS starts: Listened to the mix, bars 1–4 / Loudness -24.1 LUFS (wants -21 LUFS ±0.5) · Punch (crest) 12.3 dB (wants no lower) ✓ · Pumping 14.1 dB (wants no higher) ✓ · Distortion at peaks -10.8 dB (wants no higher) ✓ · Clipped samples 0 (wants ≤ 0) ✓ … | ok | 10.1 s |
| “Keys” up 1 dB: {"changed":"Keys volume +1 dB (Logic Pro's own undo (Cmd-Z in Logic Pro) can't take this back; Kumi can)","change":"c17"} | ok |  |
| the round: Round 1 · loudness / change: Keys up 1 dB / Loudness -24.1 → -21 LUFS ✓ / kept: Loudness improved and nothing else got audibly worse / rebalanced: the Gain on the Stereo Out +2.1 dB / every item is within tolerance / 3 listens · 0:54 | ok | 42.8 s |
| a Gain at the end of the Stereo Out: its Gain reads +2.1 dB | ok |  |
| undo ““Gain”'s Gain → +2.1 dB on “Stereo Out””: undone | ok | 4.0 s |
| undo “Added “Gain” to “Stereo Out””: undone | ok | 4.4 s |
| undo “Keys volume +1 dB”: undone | ok | 786 ms |
| audition of tracks 1 and 2: track 1: silent; track 2: unpitched · attack 1143 ms | ok | 20.7 s |
| render of bars 1 to 5 of “Keys”: kumi-render-1.wav found, 6.00 s long (6.00 s at 160 BPM) | ok | 9.7 s |
| the project reads as before M5: 2 tracks, faders Vocal +0.0 dB, Keys +0.0 dB, 0 soloed, 0 plug-ins on the Stereo Out (before: 0) | ok |  |

## What this gate rests on

The M5 bridge lane's work on the real Logic found and fixed, before the gate: the Mackie Control's timecode decoded as
Logic writes it (the digits are ASCII and the first two are the assignment display), a playhead placed after Logic
played landing exactly on its beat (it kept the old division and tick), and the metronome's click, which a bounce or a
capture holds while Metronome Click is on. Scratch.logicx on disk is as recorded: the run saved the project as a copy
under the files folder first and worked on the copy.
