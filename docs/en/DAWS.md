# What Kumi does in Live, REAPER and Renoise

English · [简体中文](../zh-CN/DAWS.md) · [日本語](../ja/DAWS.md)

Kumi works in Ableton Live, REAPER and Renoise, in each DAW's own words. This
page says what works where. Connecting is in the
[guide](KUMI_GUIDE.md#connect-to-reaper-or-renoise), every change tool in
[how Kumi changes your Set](KUMI_CHANGES.md), and the Live bridge's detail in
the [capability matrix](CAPABILITY_MATRIX.md).

## At a glance

✓ works. "Live only", "REAPER only" or "Renoise only" is something only that
DAW has. "not yet" names what's missing. "not needed" means the DAW does it
another way. "no" means the DAW's API doesn't allow it. "instead" names what
does the job there.

| Ability | Live | REAPER | Renoise |
| --- | --- | --- | --- |
| **Changing the project** | | | |
| Changes in HISTORY with Kumi's own undo | ✓ | ✓ | ✓ |
| Instant phrases, made with no model call | ✓ | ✓ | ✓ |
| Code run inside the DAW | ✓ Python | ✓ Lua (ReaScript), REAPER's API and actions | ✓ Lua |
| Stop everything at once (`/stop`) | ✓ | ✓ | ✓ |
| Variations as numbered takes you keep, change or drop | ✓ Session clips | ✓ items | ✓ |
| Sample maps: sounds laid across the keys by name, pitch and loudness, and an instrument tidy | not yet | not yet: drum kits only (`drum_kit`) | ✓ |
| Chops a recording at its real hits and lays the slices out to play it | not yet | not yet | ✓ |
| Delivers files: a region per asset, named in the project's convention, a dry run first | not yet | ✓ | not yet |
| Finds why a track plays twice or nowhere, and builds sidechains, parallel buses and stems as one undo step | not yet | ✓ | not yet |
| Keeps a repeated chore as a script: run once as one undo step, then kept as the DAW's own action once you say yes | not yet | ✓ | ✓ |
| Meets a loudness spec: ACX, streaming or broadcast, measured, fixed at the master fader, measured again | not yet | ✓ | ✓ |
| Cleans up a jam: empties, copies, repeats and mistakes told from feel, quantized by a strength | not yet | not yet | ✓ |
| Lines up layered breaks: each layer's offset, polarity and level against the main break, and the glue measured | not yet | not yet | ✓ |
| Knows whether the DAW is recording | ✓ | ✓ | ✓ |
| A line in the DAW's status bar | ✓ | no: REAPER's API has no status bar (a notification instead) | ✓ |
| Opens a saved project | ✓ | not yet: you open it yourself (`kumi -r` and setup's restart don't) | not yet: you open it yourself (`kumi -r` and setup's restart don't) |
| Catches up on what changed while Kumi was closed | ✓ | ✓ | ✓ |
| Conversations follow the saved project | ✓ | ✓ | ✓ |
| **Listening** | | | |
| Hears tracks or the mix | ✓ | ✓ | ✓ |
| Renders offline to listen, then plays it out loud | instead: hears as Live plays | ✓ | ✓ |
| Auditions candidates against a reference, and goals (`/goal`) on a rig kept open | ✓ | ✓ | ✓ |
| Judges its changes, `/loop`, matches references | ✓ | ✓ | ✓ |
| The audio file behind a clip, item or sample | ✓ | ✓ | ✓ |
| Splits a track into stems | ✓ Live's own splitter | ✓ Kumi's own splitter: drums, bass and other | ✓ Kumi's own splitter: drums, bass and other |
| Turns audio into MIDI | ✓ drums, melody and harmony | ✓ drums, melody and harmony, by Kumi's own ears | ✓ drums, melody and harmony, by Kumi's own ears |
| **FOCUS** | | | |
| FOCUS and the beat light follow the DAW as it changes | ✓ | ✓ | ✓ |
| A track's devices as a tree | ✓ | ✓ FX chains and containers | ✓ Track DSPs, Doofers and Splitters |
| The notes in the note editor | ✓ | ✓ the MIDI editor | instead: the Pattern and Phrase Editors' lines |
| The Pattern and Phrase Editors' lines | Renoise only | Renoise only | ✓ |
| The timeline at a glance | ✓ Arrangement | ✓ arrange view | instead: the Pattern Matrix |
| The Pattern Matrix | Renoise only | Renoise only | ✓ |
| A launcher grid | ✓ Session View | Live only | Live only |
| An instrument's keyzones as a map of keys | not yet: Sampler's zones aren't drawn | Renoise only | ✓ |
| A track's routing: sends, receives and hardware outputs | not yet | ✓ | not yet |
| The mixer: levels, pan, mute and solo | not yet | ✓ with peak meters | ✓ without meters (Renoise's API has none) |
| Pointing at something in the DAW (**Ask Kumi about this**) | ✓ | not yet: no REAPER action for it | not yet: no Renoise menu entry for it |
| **Library and tutorials** | | | |
| Watches video tutorials | ✓ | ✓ | ✓ |
| Finds and loads the DAW's presets | ✓ | ✓ | ✓ |
| Lists your own projects (`my_sets`) | ✓ | not yet: `.rpp` files aren't read | ✓ |
| Searches the DAW's manual | ✓ | not yet: REAPER's manual isn't searched | not yet: Renoise's manual isn't searched |
| Loads Browser items by id | ✓ | Live only | Live only |
| Plug-in parameters shown once configured | ✓ | not needed: every parameter is there | not needed: every parameter is there |
| Kumi's hands on the DAW's window, where its scripting can't reach | ✓ | not yet | not yet |
| **Live's own** | | | |
| Max for Live devices, Kumi Ears | ✓ | Live only | Live only |
| Live's Extensions SDK, Willington | ✓ | Live only | Live only |

## Live

- Kumi's bridge is a Remote Script, with Kumi's Live extension and Willington
  beside it ([capability matrix](CAPABILITY_MATRIX.md)).
- A whole plan is one Live undo (Cmd-Z, or Ctrl-Z on Windows).
- Live's own menus do what its scripting can't: freezing, bouncing, stems,
  audio to MIDI ([Live's own commands](KUMI_GUIDE.md#lives-own-commands)).
- **Variations** (`variations`): numbered takes V1, V2… of a Session MIDI
  clip, each a new clip below it on its track in a scene with no other clips
  (a scene added at the end when none is free). Their notes move in pitch, velocity or
  timing, or thin out, or Kumi writes them; the notes you name stay as they
  are. Kumi checks each differs from the source and the others yet keeps its
  key. Live plays a MIDI clip only live: launch the takes to hear them. Kept
  takes are renamed "V3 · … · kept".

## REAPER

Through Kumi's extension (`reaper_kumi`). Needs REAPER 7 or later on macOS,
Windows or Linux ([what it runs on](SUPPORT_MATRIX.md#reaper-and-renoise));
tested on 7.82 on macOS, not yet on Windows or Linux.

### Your setup

Each time Kumi looks, it reads how you set REAPER up and works with it, not
with REAPER's factory defaults:

- new items' fades (or none), loop source for new items, a new track's
  volume, the project's timebase;
- which of SWS, ReaPack, ReaImGui and js_ReaScriptAPI are installed (it uses
  one only when it's there);
- the render settings in use (source, bounds, format, sample rate, file
  pattern), which it tells you about before changing.

A file Kumi imports loops its source only when your imported media does, and
gets no fades, as your own import gets none.

### What Kumi can change

Each change is one step of REAPER's undo, and a whole plan is one step. Every
tool is in [KUMI_CHANGES](KUMI_CHANGES.md#what-kumi-can-change-in-reaper).

- **Tracks** (`track_add`, `track_move`, `track_delete` and the other
  `track_…`): added (also as a folder), moved, deleted, renamed, coloured,
  shown or hidden, folders opened or collapsed, locked; groups, templates
  (`track_template_insert`), fixed lanes and comping (`lane_plays`,
  `lane_name`, `lane_comp`), razor areas (`razor_set`).
- **Mixer** (`track_volume`, `track_pan` and the like): volume, pan and pan
  mode, width, mute, solo, polarity, channels, the send to the parent or
  master, the FX chain on or off; arming, record input, monitoring.
- **Routing:** sends and receives (`send_…`, `receive_add`), sidechains on
  3/4, hardware outputs (`hw_out_…`), MIDI to a hardware output.
- **FX** (`fx_…`): added by the name REAPER's FX browser shows, deleted,
  moved, copied, bypassed, taken offline; wet, pins, presets, whole states, FX
  chain files; containers and parallel FX; parameters (up to 256 in one step),
  ReaEQ bands; parameter modulation (LFO, audio control signal, links, learn).
- **Envelopes and automation** (`envelope_…`, `automation_item_…`,
  `automation_override`): shown and armed, points, automation items,
  automation modes.
- **Items and takes** (`item_…`, `items_glue`, `items_render`, `take_…`,
  `stretch_markers_…`): added, a file imported, moved, split, trimmed, faded,
  grouped, deleted, duplicated; glued or rendered only when you ask; takes,
  stretch markers, take markers.
- **MIDI** (`notes_…`, `midi_events_…`): notes added, changed, deleted,
  quantized, humanized; CC, pitch bend, program, pressure, text, SysEx.
- **Markers, regions and the loop** (`marker_…`, `region_add`,
  `regions_add`, `region_render`, `ruler_lane`, `time_selection`,
  `loop_range`): ruler lanes, the region render matrix, many regions in one
  step.
- **Tempo and project** (`tempo_set`, `tempo_marker_set`, `metronome_switch`,
  `project_…`): tempo markers, time signatures and ramps, the metronome, grid,
  timebase, notes, title, author.
- **Kits and goals** (`drum_kit`, `scratch_keep`).
- **Scripts** (`run_lua`, `api_call`, `run_action`).

It also plays, stops, moves the edit cursor, and saves projects (or REAPER's
next version), track templates and FX chains.

### How it listens

- Renders the part offline (faster than it plays), hears that, then plays it
  out loud for you, up to a minute. A track alone is its pre-fader stem.
- While REAPER plays, it can hear the master output as it plays (8 seconds by
  default). A render stops REAPER's playback.
- Judged changes keep their level with the last ReaLimit, else a JS: Volume
  Adjustment it adds. Goals try candidates on scratch copies ("Kumi · try …");
  the best stays on "Kumi · Goal best".

### Workflows

- **Delivery** (`render_files`): files for a game or a client, one per sound.
  Kumi groups the items into assets (stacked layers make one footstep), names
  the new ones in the convention the project's regions follow (or as you say),
  adds tails that stop before the next sound, and makes a region for each in
  one step, rendering its asset's own tracks. It then checks the files REAPER
  would write against those names, rendering nothing, and renders exactly
  those files: only the delivery's regions, each its tracks through the
  master, without the render dialog's normalizing or second format unless you
  say, never over a file that's there unless you say, with REAPER's render
  settings put back after.
- **Routing** (`routing`): why a track is too loud, silent or cancels. Kumi
  reads every way each track reaches the master (folders, sends, hardware
  outputs) and names what's likely wrong: a track heard twice through buses
  that only sum, a loop, a track heard nowhere, a sidechain on channels its
  track doesn't have, a duplicate with its polarity inverted. It builds a
  sidechain (the kick ducking the bass through ReaComp), a parallel bus or
  stems as one step of REAPER's undo, and reads the routing again to check it.
- **SFX variations** (`variations`): numbered takes V1, V2… of an item, each a
  copy after it on its track with its own pitch, rate, start, length, fades
  and volume (a MIDI item's notes, velocities and timing vary instead). Kumi
  checks each differs from the source and the others yet stays the same sound
  (its attack, and any notes you name, kept), renders them to hear them (one
  that clips where the source doesn't is turned down to the source's peak),
  and keeps, changes or drops them as you say; `render_files` delivers a file
  each.
- **Stems** (`stems`): a track's stretch split by Kumi's own splitter into
  drums (what hits), bass (what sustains under 150 Hz) and other (the rest,
  voices too), which add up to the track. Each goes on a new track under it,
  an item over the stretch.
- **Audio to MIDI** (`audio_to_midi`): a track's audio over a stretch heard by
  Kumi's own ears: drums (each hit a kick, snare or hat on General MIDI's drum
  keys, as loud as it hits, a ghost softer), a melody or the harmony (Basic
  Pitch). The notes go in one MIDI item on a new track under it; add an
  instrument to hear them.
- **Delivery to a spec** (`delivery_spec`): ACX, streaming (Spotify, Apple
  Music, YouTube…), podcasts or broadcast (EBU R 128, ATSC A/85, Netflix).
  Kumi renders the mix offline and measures it: integrated loudness, true
  peak, and for ACX the RMS and noise floor. It moves the master fader by the
  gain the spec needs when the peaks allow, measures again, and says what a
  fader can't fix (limiting, noise).
- **Scripts for a chore** (`scripts`): a chore you repeat, written in Lua
  against your project and run once as one step of REAPER's undo. Once you say
  yes, Kumi keeps it to run again by name, and as an action in the Action List
  (Scripts/Kumi) for a shortcut or a toolbar button. It won't keep one that
  names this project's ids or Kumi's refs, and says what it takes by its place.
- **Drum kits** from sound files: a ReaSamplOmatic5000 per pad, as a folder of
  pad tracks or one container, hi-hats choking.
- **Sounds from tutorials and references:** `watch_video`, then ReaEQ,
  ReaComp, ReaSynth or your plug-ins, judged by ear.
- **Presets:** FX chains, track and project templates, JSFX and FX presets
  found in REAPER's resource folder and loaded.

### Limits

- **Only REAPER's undo** takes back `run_lua`, `api_call` and `run_action`.
  `run_action` never runs an action whose name ends in “…” (a dialog), that
  quits, opens, closes or saves projects, renders, or undoes or redoes in
  REAPER.
- **Only Kumi's undo** takes back what REAPER's undo skips: repeat
  (`loop_repeat`), the metronome switch (`metronome_switch`), a ReaEQ band's
  type or switch (`fx_eq_band`), the automation override
  (`automation_override`), the project's notes, title, author, grid and
  timebase (`project_notes`, `project_info`, `project_grid`,
  `project_timebase`), and parameter modulation with nothing on.
- Kumi's undo of parameter modulation turns its switches off but leaves the
  settings REAPER keeps once made; the HISTORY row says so.
- A project whose tracks are all replaced at once reads as another project:
  Kumi's references go stale and it reads again.
- `track_midi_out` needs a MIDI output device in REAPER's preferences.
- While a dialog or a render holds REAPER, changes are refused at once.

## Renoise

Through Kumi's tool (`com.kumi.Bridge.xrnx`). Needs Renoise 3.5 or later on
macOS, Windows or Linux ([what it runs on](SUPPORT_MATRIX.md#reaper-and-renoise));
tested on 3.5.4 on macOS, not yet on Windows or Linux.

### Your setup

Each time Kumi looks, it reads how the song is set up and follows it: BPM
(a 2× or 4× convention stays), LPB, TPL, edit step, the length most patterns
run, the groups and their colours, and the tools installed (Paketti among
them).

### What Kumi can change

Each change is its own step of Renoise's undo. Every tool is in
[KUMI_CHANGES](KUMI_CHANGES.md#what-kumi-can-change-in-renoise).

- **Song** (`song_bpm`, `song_lpb`, `song_tpl`, `song_groove` and the other
  `song_…`, `metronome_switch`, `edit_settings`): BPM, LPB, TPL, groove, the
  metronome, editing settings (follow, edit step, octave, record quantize),
  name, artist, comments, headroom.
- **Tracks** (`track_…`): sequencer, send and group tracks added, moved,
  grouped, deleted, renamed, coloured, collapsed; note and effect columns;
  volume, panning, width, mute, solo, output routing and delay.
- **Pattern sequence and matrix** (`slot_…`, `slots_drop`, `loop_range`,
  `loop_pattern`): slots inserted, deleted (several in one step), moved,
  cloned, made unique, given another pattern or a variation of one; sections,
  matrix mutes and colours; loops.
- **Patterns and lines** (`notes_write`, `notes_clear`, `notes_transpose`,
  `notes_humanize`, `lines_copy`, `pattern_…`, `automation_write`): notes,
  instrument, volume, panning, delay and effect columns in Renoise's notation;
  pattern names, lengths and aliases; automation.
- **Track DSPs** (`device_…`, `doofer_…`, `splitter_set`, `notepad_write`,
  `meta_destination`): native and plug-in devices, parameters, presets, chain
  files (.xrnt); Doofers and their macros, Splitters, Notepads, where a meta
  device points.
- **Instruments and samples** (`instrument_…`, `sample_…`, `keyzone_…`):
  added, duplicated, moved, loaded (.xrni, .sfz, a sample), made from sample
  files; macros; sample settings, slices, keyzones, drum-kit layouts; a
  sample's polarity flipped.
- **Modulation and sample FX** (`modset_…`, `moddevice_…`, `chain_…`,
  `chaindevice_…`).
- **Phrases** (`phrase_…`, `phrases_drop`): added (with their lines),
  deleted (several in one step), mapped to keys, their playback and lines;
  phrase scripts written and rendered; .xrnz files.
- **Plug-ins and MIDI** (`plugin_…`, `midi_set`): plug-in instruments, their
  parameters and programs; a MIDI instrument's output.
- **Goals and scripts** (`scratch_keep`, `run_lua`).

It also plays, stops, plays a note out loud, shows something in Renoise, and
saves the song, instruments and device presets.

### How it listens

- Renders the part offline, hears that, then plays it out loud for you, up to
  a minute. A track alone is Renoise's track solo.
- Renoise's scripts can't hear what it plays, and a render stops playback:
  while it plays, Kumi renders only a part you name.
- Judged changes keep their level with the last Maximizer, else a Gainer it
  adds.
- Renoise's demo can't render, so Kumi can't listen there; `kumi doctor` says
  so.

### Workflows

- **Variations** (`variations`): numbered takes V1, V2… of a pattern track's
  lines (a chop you wrote with `0S`, `0R` and `0B` commands), of a phrase, or
  new phrases for an instrument ("make me 9 phrases"). Kumi keeps the lines
  you name as what defines it, varies the rest (slices, rolls, reverses, ghost
  notes, accents, register), checks each take differs from the source and the
  others yet keeps the idea, the key and the grid, and renders the takes to
  hear them first (never while Renoise plays; one that clips where the source
  doesn't is turned down to the source's peak by its volume column). Each take is a phrase, or a slot of its own in a "Variations of …"
  section at the sequence's end that plays the rest of the pattern with it.
  Then "keep 1, 3 and 5; change the rest" changes only those, each in its own
  direction, and the rest go in one step; a take you edited is left alone.
  Kept takes are renamed "V3 · … · kept", and Kumi remembers the takes after a
  restart.
- **Phrases:** written in Renoise's notation, or as phrase scripts rendered to
  lines; a pattern's lines become a phrase you play from any key (`variations`
  extract), and a faster copy is switched to with `0Zxx`, since a phrase's LPB
  is fixed.
- **Sample maps** (`map_samples`): a folder of sounds, or an instrument's own
  samples, laid across the keys in one step. Kumi reads each file name's note
  ("C3", "A#2", "C-4"), velocity ("v3", "vel64", "ff") and round robin ("rr2"),
  hears each sample's pitch and loudness, and works out whether the names put
  middle C at C3, C4 or C5. It plans roots and zones, velocity layers in
  loudness order, round robins that take turns, and an instrument per
  articulation. It says what it decided and asks only what it can't tell. The
  tidy finds instruments no note plays, spare copies, and samples two
  instruments hold, and changes only what you accept.
- **Chops** (`chop`): a break cut at its real hits, each found to the frame
  and named as a kick, snare, hat, ghost or roll, with the break's own tempo
  worked out from its length. Kumi slices it in one step (on the song's grid
  when the tempos match) and lays the slices out to play it as recorded, using
  the delay column for hits between lines. It renders the result and says how
  close the hits land and how deeply it nulls against the recording. The
  lighter densities (no ghosts or rolls; kick and snare) come as variations
  takes to keep or drop.
- **Cleaning up a jam** (`jam`): after a catch-up, what you recorded is set
  against what came before. Kumi says what's new and proposes the clean-up:
  slots playing empty patterns to drop, a pattern the same as an earlier one
  to play that one instead, tracks repeating an earlier pattern's as aliases,
  and notes that read as mistakes rather than feel (a double trigger, a note
  far off the grid where its neighbours aren't, a stray; a swing every beat
  shares is kept). It applies what you accept, quantizes by a strength if you
  ask, and never touches what came before.
- **Layering breaks** (`layers`): the main break and its layers rendered alone
  over a stretch, hit by hit: how early or late each layer lands, whether its
  polarity fights the main's, and how loud it sits. Kumi nudges a layer by its
  track delay and flips one in its samples (Renoise has no polarity switch;
  only when no other track plays them). For the glue it hears the group
  without its compressor, then sets a Bus Compressor's threshold until the
  glue measures as asked. The balance is yours, by ear.
- **Stems** (`stems`): a track's stretch split by Kumi's own splitter into
  drums, bass (under 150 Hz) and other (voices too), which add up to the
  track. Each is loaded as a new instrument and played from the stretch's
  start on a new track after it.
- **Audio to MIDI** (`audio_to_midi`): a track's audio over a stretch heard by
  Kumi's own ears: drums (kick, snare and hat, as loud as they hit), a melody
  or the harmony (Basic Pitch). The notes go on a new track, in the patterns
  its slots play, each on its line with a delay, played by the instrument you
  name.
- **Delivery to a spec** (`delivery_spec`): ACX, streaming, podcasts or
  broadcast loudness. Kumi renders the song offline, measures it, moves the
  master's post-FX fader by the gain the spec needs when the peaks allow, and
  measures again; past the fader's +3 dB, it says a Gainer or the Maximizer
  comes first.
- **Scripts for a chore** (`scripts`): a chore you repeat, written in Lua
  against your song and run once as one step of Renoise's undo. Once you say
  yes, Kumi keeps it to run again by name, and as Tools > Kumi > its name, with
  a shortcut to set in Renoise's Keys preferences. It won't keep one that names
  Kumi's refs, and says what it takes by its place.
- **Instruments from sample files:** a drum kit with mute groups, or keys with
  velocity layers, in one instrument.
- **Doofers** from devices, with named macros.
- **Sounds from tutorials and references:** `watch_video`, then native devices
  or your plug-ins, judged by ear.

### Limits

- **Only Renoise's undo** takes back `run_lua`.
- **Only Kumi's undo** takes back what Renoise's undo skips: mute and solo
  (`track_mute`, `track_solo`), the loop and the pattern loop (`loop_range`,
  `loop_pattern`), the metronome switch (`metronome_switch`), editing settings
  (`edit_settings`) and a plug-in's auto-suspend (`plugin_suspend`).
- After Renoise's own undo and redo of line edits (`notes_write`, `lines_copy`,
  `phrase_write` and the like), Kumi's undo of them works for the newest change
  only: nothing edited after it, and nothing left to redo. In any other case
  it's refused: Kumi can't tell Renoise's redo from your own identical lines.
  Renoise's undo still can.
- In long sessions Renoise's undo history has sometimes emptied after AU
  instruments were loaded and unloaded (cause unknown).
- FOCUS's Mixer has no meters: Renoise's API has none.
- A render of a dense song can be refused as busy for a moment; ask again.
