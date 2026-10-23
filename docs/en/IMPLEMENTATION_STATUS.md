# Implementation status

English · [简体中文](../zh-CN/IMPLEMENTATION_STATUS.md) · [日本語](../ja/IMPLEMENTATION_STATUS.md)

Where things stand: current versions, what has been tested on real Live and
where the records are, and the known limits. What each area of Live supports
is in the [capability matrix](CAPABILITY_MATRIX.md); systems and Live versions
are in [supported platforms](SUPPORT_MATRIX.md).

## Versions

Each release in the [changelog](../../CHANGELOG.md) names the bridge it ships
with; [bridge versions](KUMI_CHANGES.md#bridge-versions) lists them all. The
version numbers themselves live in the crates' `Cargo.toml` files (Kumi, the
bridge), the root `package.json` (Kumi) and `apps/live-extension/manifest.json`
(the extension); REAPER's extension's in `crates/reaper-kumi/Cargo.toml`, and
Renoise's tool's in `daws/renoise/com.kumi.Bridge.xrnx/payload/version.lua` and
its `manifest.xml`. [Connect to REAPER or Renoise](KUMI_GUIDE.md#connect-to-reaper-or-renoise)
says which Kumi ships with which.

| Part | Version |
| --- | --- |
| Live protocol | `ableton-live/v1`; each release's `release-manifest.json` records the registry's hash |
| REAPER and Renoise protocol | `kumi-daw/1`, between Kumi's host and the bridge inside the DAW, on 127.0.0.1 |
| MCP protocol eras | `2025-11-25` and `2026-07-28` |
| Runtime | Native Rust app and standalone bridge; fresh installs require no Node |
| Older installs (Kumi 1.7.5 and earlier) | Node 22/24; retained Node also supports rollback and optional YouTube challenges ([details](SUPPORT_MATRIX.md)) |
| Logic Pro bridge (`logic-kumi`, beside Logic) | 0.1.0, on the `kumi-daw/1` wire; its shapes are in [the Logic contract](LOGIC_CONTRACT.md) |

## What has been tested where

The real-Live records below are from earlier TypeScript releases. Native CI
and migration checks do not replace a new real-Live acceptance run.

- **Every pull request:** native Rust builds and tests, the Remote Script's and
  the Live extension's tests, six target bundles, and installation and
  migration tests; see [CI](TESTING.md#ci).
- **Real Live on macOS** (Apple silicon, Live 12.4.15 beta): every kind of change
  Kumi makes, each undone through Kumi (65 of 65 on bridges 1.0.62 and 1.0.63,
  in 19- and 200-track Sets), playing, bouncing, listening and watching, offline
  renders and the right-click menu through Kumi's extension, and the Willington
  edits.
- **Real Live on Windows** (Windows 10, Live 12.4.15 beta, Kumi 1.6.0 with
  bridge 1.0.71): installing the bridge into a moved User Library, the Remote
  Script loading, and Kumi connecting. There's no record file for this yet.
- **REAPER and Renoise on macOS** (REAPER 7.82 evaluation, portable; Renoise
  3.5.4; 20 October 2026): each change applied through Kumi, undone through
  Kumi, then undone and redone with the DAW's own undo, big projects (REAPER 200
  tracks and 2,000 items, Renoise 64 tracks and 100 patterns), and listening.
  - REAPER, at commit `c80eb1e7`: 140 of its 141 kinds then, one known limit.
    `drum_kit`, `scratch_keep` and `metronome_switch` came later and weren't run.
  - Renoise, at commit `78faa00f`: 121 of 132 cases, 11 known limits.
    `instrument_kit` and `scratch_keep` came later and weren't run.
  - The limits are in [known limits](KUMI_CHANGES.md#known-limits). There's no
    record file for these yet. REAPER and Renoise on Windows and Linux are
    untested.
- **Real Logic Pro on macOS** (Apple silicon, Logic Pro 12.3.1): the lab of what
  macOS gives Kumi to read and change Logic, a go or no-go per channel (M0), and
  the bridge beside Logic attaching through its Accessibility helper and
  answering `ping` and `status`, set up by `kumi --daw logic bridge`, found by
  the doctor and started by Kumi's host from a session (M1). M2, observation:
  the bridge's reads over the real Logic are recorded in
  `crates/daw-mcp-server/tests/vectors/logic-reads.json` (the rows, the refs,
  the focus and FOCUS's bodies), the host replays them, the runtime reads them,
  opens a kept conversation's project again and tells whether Logic records, and
  `crates/kumi/examples/accept_logic.rs` is the opt-in run against the real
  Logic for the gate. M3, changes and undo: the bridge makes sixteen kinds over
  the real Logic (the header's fader, pan, mute, solo and arm, the Inspector's
  name, tracks added below the selected one and deleted, markers made, moved
  and deleted, the tempo, a region's notes as Event List rows, a plug-in's
  bypass), each read back, and names the producer's own undo back; Kumi's
  change set declares exactly those (`logic_changes.rs`), the simulated Logic
  answers their shapes over the recorded project, the host's journal keeps an
  apply or an undo the bridge couldn't confirm uncertain and never sent twice,
  HISTORY over Logic follows a Cmd-Z through `undone_in_daw`, and the
  acceptance run's M3 checks are a plan of a fader, a rename, a track added and
  muted, each undone through Kumi, and a rename taken back with Cmd-Z. Logic
  keeps no undo step for a fader, pan, mute, solo, arm, tempo or bypass, so
  those are put back by value; a deleted track comes back only through Logic's
  own undo while that step is on top. The M3 gate on this Mac is recorded in
  `docs/evidence/logic-m3-gate-2026-10-19.md`. M4, the first slice: Kumi's
  change set declares the kinds the bridge makes over the real Logic beyond
  M3's, with REAPER's tool names where the concept is the same and Logic's words
  throughout: `key_command` (the escape hatch, by the command's English name or
  id; a command Logic keeps no undo step of makes no row, and only Logic's own
  undo takes one back), the regions of the Tracks area moved along their track,
  resized and split on whole beats (the right part's ref comes back), looped,
  renamed and deleted, `notes_quantize`, the cycle range (`loop_range`, no step
  of Logic's undo), the Tempo and Signature Lists (`tempo_marker_set`,
  `tempo_marker_delete`, `signature_set`), many notes at once as a MIDI file
  Kumi writes and Logic imports onto a new track below the one named
  (`midi_import`), and Track Stacks made and flattened; and the action
  `save_project` (the host's `project_save`: a save, or a save as to a `.logicx`
  that isn't there yet; no row). A track moved and a track coloured are refused,
  saying why: the headers can't be dragged from outside (no key command reorders
  tracks, and a header offers no action but its menu), and the Color palette's
  swatches are bare positions to Accessibility while a header shows no colour,
  so a colour could never be read back. The simulated Logic answers every one of
  them but `notes_quantize` statefully over the recorded project, as the
  bridge's recorded scenes answer (their shapes, the titles Logic's Edit menu
  gives each step, the epoch moving where the bridge's moves); the host's tests
  and the wire golden cover them, the runtime's tests take them into HISTORY and
  back out (a save with no row, a key command with and without a row, a split
  and an import undone through Logic's own undo, the cycle, a tempo change, an
  undo Logic answered uncertain), and the acceptance run gains the M4 checks
  (`--files <dir>` names the folder a save as goes under; a MIDI import counts
  when its region spans the notes, since Logic's Event List lists only the rows
  on its screen). The M4 gate on this Mac, every check passed after six runs
  that each found something to fix, is recorded in
  `docs/evidence/logic-m4-gate-2026-10-21.md`.
  M4, the second slice: Kumi's change set
  declares plug-ins put on a channel strip and taken off it (`fx_add`,
  `fx_delete`), their parameters set in the plug-in window's Controls View
  (`fx_param`, `fx_params`) and their settings stepped with its buttons
  (`fx_preset`), a patch from the Library (`patch_load`), sends (`send_add`,
  which has Logic make an aux for a bus nothing takes, `send_remove`,
  `send_volume`), Smart Controls (`smart_control_set`) and automation through
  the Track Automation Event List (`automation_add`, `automation_change`,
  `automation_delete`), with REAPER's tool names where the concept is the
  same, and a bounce and an export as actions (`bounce`, `export_tracks`: the
  host's tools of the same names, files written, no row, the host titling
  what they wrote); the reads gain a plug-in's parameters (`param`), a track's
  Smart Controls (`smart`) and automation (`automation`). The Stereo Out is
  the bridge's `<epoch>:output:1`, which no read lists: Kumi names it in every
  observation (`outputs`) and keeps its name when the epoch moves, so its
  plug-ins are read with it as parent and a Gain goes on it. Logic's tools on
  every request are REAPER's eight (`fx_add` and `fx_param` among them; a
  split and an import are `make_changes` steps). The kinds follow the
  bridge's second slice, whose shapes and recorded scenes they were checked
  against (every scene replayed through the simulated Logic answers as Logic
  did, but an export's length and what the producer's own Cmd-Z puts back).
  The simulated Logic holds each strip's plug-ins (with their
  Controls View rows and settings; Channel EQ, the Gain and Keys' Piano as
  the scenes read them), sends, patch and Smart Controls, the Stereo Out's
  plug-ins and the auxes Logic made, and each track's automation over the
  recorded project, makes every one of these kinds (and the bypass)
  statefully, refuses `fx-move` and `send-mode` as the bridge does, and
  writes a bounce's and an export's audio files where it is asked to; it also names tracks and
  markers as the recorded scenes read them (a named track is made, then
  renamed; a new track is named after its channel strip; a marker is named by
  its place in the Marker List and made 0 beats long). The plug-in configure
  texts are each DAW's: Live's say to configure a parameter in the device, as
  before, and Logic's say the Controls View of the plug-in's window, which
  Kumi opens itself. Logic's instructions carry its terms and ways (regions,
  the Stereo Out and the auxes Logic makes, Track Stacks, what its own undo
  keeps no step of, quantize as a parameter, an import's own track, the lists,
  key commands on the selection). The acceptance run gains the second slice's
  checks: a Gain on the Stereo Out with its gain set and read back, a send from
  track 1 to a new bus, a patch loaded, a bounce of bars 1 to 5 found on disk,
  each undone or taken away. The second slice's gate on this Mac, every check
  passed on its second run, is recorded in
  `docs/evidence/logic-m4b-gate-2026-10-21.md`.
  M5, listening: Kumi hears Logic Pro. The host offers Logic's `hear` (a part
  played while the bridge's helper taps what Logic outputs after the Stereo
  Out, cut to the part, into a WAV file in a folder Kumi names), `audition`
  (such a pass for each candidate, a track soloed alone or the whole mix) and
  `render` (a part bounced offline, a track soloed for it), each bounded by the
  part's length at the tempo Logic plays at, plus a margin; audio travels as a
  file, never as samples. Logic's integration gives the shared one a listening
  hook (`OwnDaw::listening`), whose `Renderer`
  (`integrations/logic_listening.rs`) answers the listening tools (judge, tune,
  sound, form, groove, audition, render) and the listen tool's `hear`: a track
  is heard alone by soloing it for a pass of its own, the producer hears an
  audition's first candidate while Logic stays silent for the others, and a
  judged round brings the level back at the run's track or the Stereo Out (its
  last Adaptive Limiter or Limiter that's on, else a last Gain, else a Gain
  Kumi adds at the end), each set through the Controls View as a change in
  HISTORY that Kumi's undo takes back; a render's note says the Stereo Out's
  plug-ins are in it. Logic's capabilities gain `hear` and `audition`, and its
  words say how it is heard and where a judged round turns the level. The
  listen tool and the fallback say what Kumi can't do in a DAW that is
  connected ("Kumi can't pursue a goal in Logic Pro yet."), no longer that it
  isn't connected. The simulated Logic synthesizes what it plays (each track a
  tone of its own through its fader, pan and level plug-ins, then the Stereo
  Out's, so a Gain turned there is heard) and answers the three methods in the
  shapes of the contract's Listening section (its file names, fields, notes and
  refusals).
  The acceptance run gains the M5 checks: bars 1 to 5 heard (the file's length
  and loudness), a judged round whose gain goes on the Stereo Out (a Gain
  added, set, read back, undone), tracks 1 and 2 auditioned, a render of bars
  1 to 5 found on disk, and the project as before at the end. The M5 gate on
  this Mac, every check passed on its first run, is recorded in
  `docs/evidence/logic-m5-gate-2026-10-21.md`.
  M6, the bridge's new kinds and reads in Kumi: the change set declares
  `metronome_set` (the control bar's Metronome Click, no step of Logic's undo,
  so its row says Kumi's undo takes it back) and `sample_load` (an audio file
  imported onto an audio track and converted into a new Sampler track below it;
  Logic copies the file into the project's package, and its own undo takes the
  load back, both steps, while it is on top). Kumi finds a sample's file on this
  computer before Logic is asked, as it finds Live's (a whole path, or one under
  `~`, to an audio file Logic imports), and refuses one that isn't there in its
  own words. The host forwards Logic's bulk read (`snapshot` with `rows` and
  `limit`) and its `recent` read; the model's read lists the projects Logic
  opened lately. The simulated Logic makes both kinds as the bridge's scenes
  do, keeps the click's state, and answers the project row's `middleC`, the tempo
  row's Tempo List, the recent projects and the bulk read.
  Logic's fixed phrases ("tempo 124", "mute drums", "loop 9-16", "metronome
  off") run with no model call, as Live's do: each is one of Logic's own changes
  (`tempo_set` in whole BPM, a header's mute, solo or arm, `loop_range` for the
  cycle, `metronome_set`), made through `make_changes` on the project as a fresh
  look reads it, with tracks named or counted by their place among the
  producer's own (never an aux, an output or the Stereo Out); each lands in
  HISTORY with Kumi's undo. A tempo past Logic's 990 BPM goes to the model.
  Middle C follows Logic's "Display Middle C As" setting, which the project row
  says (`middleC`, 3 or 4): the session names notes in that octave (the listen
  tool's pitch of a sound, `find_sounds`' note words, FOCUS's keyzones), taking
  it from the open project rather than Logic's default. A recipe's save text
  names `watch_me` only in a DAW that has it, and Logic's
  example of a recipe's step names its own tools (`fx_param` on the plug-in an
  earlier `fx_add` inserted). `kumi export` takes the DAW's words from the
  conversation's place (a Logic project's says Logic Pro and project, as the
  session's own export does), and the export, `kumi import` and the budget's list
  of what the producer asked recognise a loop's or a goal's request in every
  registered DAW's framing, so one of several lines framed in REAPER's,
  Renoise's or Logic Pro's words is the producer's, not left out as Kumi's.
  Logic's library is its own: the session's and `kumi library`'s library learn
  Logic's content folders (the producer's under `~/Music/Audio Music Apps`
  before Logic's own under `/Library/Application Support/Logic`), Apple Loops,
  Splice's folder and the folders named, never Live's; `.caf` is a sound there,
  a patch (a package counted whole), a channel strip or plug-in setting, a
  Sampler instrument and an Audio Unit's preset are presets (`find_presets` is
  offered for Logic, its kinds an instrument or a plug-in's setting), and a
  `.logicx` package is a project, counted by its name (Kumi doesn't read inside
  it: `my_sets` lists it by its file, with no tempo or key to filter by, and the
  producer's habits are still learned from Live's Sets alone); the library's
  tools say so in Logic's words (`find_sounds` likens a sound to an audio
  region's by its ref, and names `sample_load`). Logic's projects
  are looked for in `~/Music/Logic` and the folders holding the ones it opened
  lately, which the integration reads once connected from the bridge's File >
  Open Recent and the library keeps for its next run: the bridge knows a recent
  project's place only for the one open and for those Spotlight finds outside
  `~/Music` (macOS keeps that folder from the bridge), so others aren't learned
  until opened. REAPER and Renoise keep Live's layout, their own libraries
  added. Over Logic, judge and groove say a region and its ref where Live's say
  a clip, groove names Logic's note changes and says in Logic's words that it
  can't read a region's notes there yet, and the gap guidance and `note_gap`
  name Kumi's tools alone, as Logic has no scripting Kumi reaches.
  A plug-in's parameter change in Logic draws in HISTORY as Live's do: the
  values it moved at once (as Logic shows them, with what was asked when Logic
  set something else), then, for Channel EQ and the Compressor, their curves
  before and after (Channel EQ's eight bands; the Compressor's threshold,
  ratio, knee and make-up), from one read of the plug-in's Controls View once
  its changes settle. That read opens the plug-in's window in Logic for a
  moment, as a change of it does.
- **Logic Pro's manual (M7):** `logic_manual` answers from Apple's Logic Pro
  User Guide for Mac as `live_manual` does from Live's, the two sharing what
  every manual does (`integrations/shared/manual.rs`: the kept copy read again
  after 45 days, the stems and BM25, the passages). The guide's welcome page
  (`support.apple.com/guide/logicpro/welcome/mac`) lists its contents whole,
  some two thousand topics in parts and chapters; a topic is a page of its own
  (`…/guide/logicpro/<its title's slug>-<id>/mac`), read without the site
  around it with `?displayMode=headless`, and a task or subhead in it is
  addressed by the id of the `div` that holds its heading. Reading every topic
  would take two thousand requests, so Kumi reads the contents once and a topic
  the first time an answer needs it (the six whose titles and places best match
  the question), keeps both in the library folder (`logic-pro-guide.json`), and
  cites a topic by the guide's title for it ("Logic Pro User Guide, Quantize
  regions"). A question is matched against the topics' titles and places
  first, so words no title uses ("sidechain", which the guide writes "side
  chain") may miss a topic that only its text names; `topic` reads one whole
  by its title or address. The parser and the tool are tested over a synthetic
  site laid out as Apple's (`crates/kumi-runtime/tests/it/logic_manual.rs`); the
  guide's structure was read from a dozen of Apple's pages while developing.
- **Logic Pro's instruction pack (M7):** Logic's turns carry Kumi's persona and
  the general rules Live's pack carries, said for Logic in its words
  (`integrations/logic_instructions.txt`: doing what's asked without asking
  first, being resourceful and quick, listening before and after a change,
  matching a sound by search, learning a routine and following a video,
  looking things up and asking Apple's guide how Logic works, what is untrusted
  and what is current, refs and pages, the project as the target), then
  Logic's own words and ways (M4b) and its change set's instructions. Live's
  own ways (Python in Live, tunings, resampling, Max for Live devices, Live's
  menus, its device containers and launcher, its note notation) are left out.
  With every option on, the first request over the simulated Logic is 56,698
  bytes (16,398 of system text) against Live's 73,204 (25,945); before the
  pack it was 47,656 (7,356). With Logic Pro away, the conversation's one line
  says so and points at Apple's guide.
- **Logic Pro's eval (M7):** `crates/kumi/examples/eval_logic.rs` asks the
  configured model 23 things over the simulated Logic, each a session of its
  own with Logic's instruction pack, changes and listening, and Apple's guide
  read from a fixture of its topics (never the network): ten knowledge probes
  of Logic's terms and ways (a region, the Stereo Out, Track Stacks, the
  Marker, Tempo and Signature Lists, key commands on the selection, what
  Logic's undo keeps no step of, the Controls View, quantize as a region
  parameter, an import's own track, Smart Controls), seven change requests over
  Scratch (a fader, a rename, a marker, a tempo change, a Gain on the Stereo
  Out, a send, a plan of several), five limits (a track moved or coloured, a
  send's mode, a plug-in moved, Quick Sampler) and a listen. Each check is
  code: Logic's words and none of Live's, the guide asked and cited by a topic
  it gave where a question is about Logic's features, nothing changed for a
  question, the changes Logic applied and HISTORY's rows, a limit refused
  saying why, the mix heard. Its dry run, every check on a canned answer, runs
  in CI (`crates/kumi/tests/eval_logic_dry.rs`); it hasn't been run with a
  model yet.

## Evidence

The records in [`docs/evidence`](../evidence/). Where a record doesn't name its
bridge, the version given is the one in the commit that added it.

| Record | Date | DAW | Bridge | What it shows |
| --- | --- | --- | --- | --- |
| [logic-lab-2026-10-18.md](../evidence/logic-lab-2026-10-18.md) | 2026-10-18 | Logic Pro 12.3.1 | — | The Logic Pro lab (M0): a go or no-go per macOS channel (Accessibility reads and writes, key commands, the open and save panels, Mackie Control, OSC, the project file, the process tap), measured on this Mac |
| [logic-m1-gate-2026-10-19.md](../evidence/logic-m1-gate-2026-10-19.md) | 2026-10-19 | Logic Pro 12.3.1 | `logic-kumi` 0.1.0 | The M1 gate: the bridge attached to the real Logic with its helper, `ping` and `status` over the wire; the install lifecycle, the doctor, and Kumi's host starting the bridge from a session |
| [logic-m2-gate-2026-10-19.md](../evidence/logic-m2-gate-2026-10-19.md) | 2026-10-19 | Logic Pro 12.3.1 | `logic-kumi` 0.1.0 | The M2 gate: Logic's project read through the bridge as rows with refs, FOCUS's bodies, and Kumi's look over Logic |
| [logic-m3-gate-2026-10-19.md](../evidence/logic-m3-gate-2026-10-19.md) | 2026-10-19 | Logic Pro 12.3.1 | `logic-kumi` 0.1.0 | The M3 gate: changes made through one channel each and read back, a plan into HISTORY and out, Logic's own Cmd-Z named back to Kumi |
| [logic-m4-gate-2026-10-21.md](../evidence/logic-m4-gate-2026-10-21.md) | 2026-10-21 | Logic Pro 12.3.1 | `logic-kumi` 0.1.0 | The M4 gate, first slice: a save as, key commands, regions, a MIDI import, the cycle and the Tempo List, each read back and put back; 42 of 42 |
| [logic-m4b-gate-2026-10-21.md](../evidence/logic-m4b-gate-2026-10-21.md) | 2026-10-21 | Logic Pro 12.3.1 | `logic-kumi` 0.1.0 | The M4 gate, second slice: a Gain on the Stereo Out set in its Controls View, a send, a patch, a bounce, Smart Controls and automation; 54 of 54 |
| [logic-m5-gate-2026-10-21.md](../evidence/logic-m5-gate-2026-10-21.md) | 2026-10-21 | Logic Pro 12.3.1 | `logic-kumi` 0.1.0 | The M5 gate: Kumi hears Logic through the process tap, a judged round's gain on the Stereo Out, an audition and a render; 66 of 66 |
| [logic-m6-gate-2026-10-21.md](../evidence/logic-m6-gate-2026-10-21.md) | 2026-10-21 | Logic Pro 12.3.1 | `logic-kumi` 0.1.0 | The M6 gate: instant phrases, the metronome, a sample into a Sampler track, recent projects, middle C, the bulk snapshot, watch_me and Channel EQ's and the Compressor's curves; 95 of 95 |
| [logic-m7-evals-2026-10-21.md](../evidence/logic-m7-evals-2026-10-21.md) | 2026-10-21 | Logic Pro 12.3.1 | `logic-kumi` 0.1.0 | The M7 eval with the producer's model over the simulated Logic: 14 of 23 at first, 22 of 23 after its fixes, and what words only nudge |
| [kumi-poc.md](../evidence/kumi-poc.md) | 2026-09-28 to 09-30 | 12.4.15b4, b5 | 1.0.0 to 1.0.63 | Kumi on real Live: acceptance runs, speed and big Sets, `kumi bridge`, audition and goals, a Max for Live device, reconnecting |
| [live-extension.md](../evidence/live-extension.md) | 2026-09-30 | 12.4.15b5 | 1.0.57 to 1.0.65 | How Live runs Kumi's extension; offline renders, its costs, both channels agreeing, undo, right-click |
| [lom-audit.md](../evidence/lom-audit.md), [JSON](../evidence/lom-audit-12.4.15b5.json) | 2026-09-30 | 12.4.15b5 | 1.0.55 | A census of Live's Python API against what the Remote Script uses |
| [kumi-benchmark.md](../evidence/kumi-benchmark.md) | 2026-09-30 | 12.4 beta | 1.0.52 | Recreating a section of a track by ear: runs and scores |
| [kumi-clip-follow-actions-b5.json](../evidence/kumi-clip-follow-actions-b5.json) | 2026-09-30 | 12.4.15b5 | 1.0.53 | Follow Actions and Legato through Willington, read back and undone |
| [willington-kumi-chat.json](../evidence/willington-kumi-chat.json) | 2026-09-30 | 12.4.15b4 | 1.0.52 | A Kumi conversation using the Willington edits, each undone |
| [rack-zones-b5.json](../evidence/rack-zones-b5.json) | 2026-10-01, 2026-10-02 | 12.4.15b5 | 1.0.66 | Rack chain zones through Willington: read, written, undone, saved and reopened; then signal gating, fades and Max `live.object` writes, read back and restored |
| [capability-manifest.json](../evidence/capability-manifest.json) | as of Kumi 1.7.6 (2026-10-04) | — | 1.0.74 | Every registry operation, executable or reserved, and the registry's hash; no longer regenerated |
| `phase-3` to `phase-9` files | 2026-07-26 to 07-28 | 12.4.5b8 | 0.1.0 | The bridge's first real-Live runs, before Kumi: discovery, audition, transport, clips, Arrangement, mixer, automation, devices, Browser, routing, recording, project files, events, realtime and capture; plus the [FFmpeg loudness oracle](../evidence/phase-8-audio-oracle.json) and the packaged journeys against a fake Live. History: the bridge has changed a great deal since. |

## Known limits

- Live's APIs can't save or export the Set, freeze, create group tracks or edit
  Arrangement automation; see [what Live's APIs don't offer](CAPABILITY_MATRIX.md#what-lives-apis-dont-offer).
- Some changes can't be undone through Kumi, only with Live's own undo; see
  [Live safety](LIVE_SAFETY.md).
- On Windows, Kumi's extension is untested and `kumi update` from 1.6.0 or
  earlier can fail on a tar error; see [Windows](SUPPORT_MATRIX.md#windows).
- Real-Live testing so far is on Live 12.4.15 beta, mostly on Apple silicon
  Macs. Live 12.0 to 12.3, Intel Macs and screen readers are untested.
- The Willington edits are off until `/willington` turns them on, and need a
  Live build Willington has bindings for: macOS ARM64 Live 12.4.15b4 and b5
  (rack chain zones on b5 only) and Windows x64 Live 12.4.15b5. On Windows, the
  bindings were tested through Python in Live, not yet through Kumi's own edits;
  see [Willington](WILLINGTON_INTEGRATION.md).
- Kumi and the bridge are unsigned; see [releases and distribution](DISTRIBUTION_POLICY.md).
- Logic Pro is heard after its Stereo Out (a process tap hears Logic's whole
  output): a track is heard alone by soloing it, so it passes the Stereo Out's
  plug-ins, and masking isn't read through any of them but a neutral Gain; each
  candidate is a real-time pass of its own. Kumi can't yet keep a goal's render
  rig open in Logic, tune a plug-in's knobs by ear (tune reads no knob of
  Logic's), or follow a track renamed during a run; nor hear a region's audio
  file as a file, since Logic shows it by name only (its track is heard over
  the region's bars instead). A listen turns Logic's metronome off and on (its
  clicks time the capture), a setting Logic saves with the project, so the
  project shows as edited after one though nothing in it changed; Kumi says so
  when it has listened.
- Logic Pro support can't name or rename a marker, since a marker's name cell
  isn't a text field; nor move or colour a track, since the headers can't be
  dragged from outside and the Color palette's swatches are bare positions that
  no header reads back; nor move a plug-in (Logic drags it) or set a send's
  mode (a mark Logic draws itself, which nothing outside can read); the bridge
  refuses each saying why. Kumi declares the kinds the bridge makes over the
  real Logic, both M4 slices' and M6's.
  Logic has no undo block and keeps no undo step for a fader, pan, mute, solo,
  arm, tempo, bypass or the cycle, nor for a plug-in's parameter or setting
  (steps of the plug-in window's own undo), a Smart Control, a send's level or
  the metronome, so Kumi puts those back by value; a deleted track or region, a
  split, an import, a flattened Track Stack and a patch loaded come back
  through Logic's own undo only while that step is on top (a deleted plug-in
  with its settings, and a send made with the aux Logic made for it), and a
  sample loaded goes that way, its Sampler track and both regions, the copies
  of its file staying in the project's package. What a key command did only
  Logic's own undo takes back. A sample goes into Logic's Sampler only: Quick
  Sampler takes one by a drag onto its window. A region moves in time only,
  along its track; its edges, a split and an import's place go on whole beats,
  and an import makes a track of its own. The tempo takes whole BPM. Saving
  keeps no versions (that is REAPER's). Logic Pro 11 and Intel Macs are out of
  scope. See [the Logic contract](LOGIC_CONTRACT.md).
- Logic Pro keeps no id Kumi could put inside a project, so Kumi knows a Logic
  project by its path (the bridge's id is a hash of it): a project moved or
  saved under another name is a new one for catching up and for memory.
  Catching up and `watch_me` compare
  snapshots Kumi reads through the bridge: tracks are matched by name and order
  (a track renamed in its place and kind is told as renamed), regions by their
  track and start, plug-ins by their order on the strip and markers by
  position, and what can't be told apart is said to be unclear. They don't
  compare notes or plug-in settings yet. A track whose Mixer strip Logic hadn't
  drawn when it was read (in a project wider than the Mixer's view) has its
  plug-ins and routing left out of the comparison rather than told as removed.
  The Stereo Out's plug-ins are the project's own lines (one added, removed,
  bypassed or back on), compared once both snapshots have read them. Where the
  bridge reads every row of the kinds at once (`snapshot` with `rows`), the
  snapshot is that one look at the project, its Mixer and List Editors shown
  once (2.7 s on Scratch against 4.5 s for the pages, by the M6 bridge's
  measure); an older bridge's is read a page at a time, as before.
  A project in Logic's Project Templates folder is never caught up on.

## Open work

- REAPER and Renoise on Windows and Linux, and REAPER for Windows on Arm, with
  the real DAWs.
- Logic Pro: the first release that carries its bridge (M8); the goal search
  over Logic (a `GoalRig`); a region's notes past the rows the Event List shows
  on its screen; a guard in code against ending a way round with `final: true`,
  which the eval shows words only nudge
  (`docs/evidence/logic-m7-evals-2026-10-21.md`).
- Logic Pro: the last acceptance run on this Mac failed in a cascade the
  earlier gates passed, after a day of automation left Logic with its main
  window closed and dozens of leftover Window menu entries; a run after Logic
  is restarted, and the bridge clearing a dialog it didn't ask for, are open
  (`docs/evidence/logic-m8-release-2026-10-21.md`).
- Logic Pro, M6: catching up on a saved project as it opens or is switched to,
  and `watch_me`, run over the simulated Logic (the snapshot is kept in the
  project store under the project's place, `logic-<id>`, beside its
  conversations, saved again every five minutes and after Kumi's own changes);
  the M6 gate on this Mac, every check passed on its third run and recorded in
  `docs/evidence/logic-m6-gate-2026-10-21.md`, ran the bulk snapshot (2.8 s on
  Scratch, against 4.9 s for six paged reads, the Mixer and the List Editors
  opening once for a moment), `watch_me`, instant phrases, the metronome, a
  sample into a Sampler track, the recent projects, middle C and the curves of
  Channel EQ and the Compressor on the real Logic; catch-up as a project opens
  still waits for a run of its own there.
- Confirming where Live on Windows keeps its Extensions folder.
- The generated capability manifest still lists Browser preview as reserved,
  though the bridge offers it whenever Live's Browser can preview.
- The decisions listed under [open owner decisions](DISTRIBUTION_POLICY.md#open-owner-decisions).
