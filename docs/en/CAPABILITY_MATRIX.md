# Capability matrix

English · [简体中文](../zh-CN/CAPABILITY_MATRIX.md) · [日本語](../ja/CAPABILITY_MATRIX.md)

What the bridge covers in each area of Live, which channel does the work, how a
change is undone, and whether it has been tested on real Live. The tools
themselves are listed in the [user guide](USER_GUIDE.md); Kumi's own change
tools, built on these, are in [how Kumi changes your Set](KUMI_CHANGES.md).

## How to read the matrix

**Channel** is where the work happens:

| Channel | What it is |
| --- | --- |
| Remote Script | `AbletonMcpBridge`, inside Live, through Live's Python API |
| Extension | Kumi's Live extension, in Live's Extension Host (Live 12.4 or later) |
| Willington | Native bindings, in Kumi's bridge from the release that carries Willington's files, or installed yourself; off until `/willington`, for the Live builds they cover: macOS ARM64 12.4.15b4 and b5, Windows x64 12.4.15b5 ([Willington](WILLINGTON_INTEGRATION.md)) |
| Bridge | The bridge process itself, without Live |

A tool is offered only when the Live it's connected to has the operations it
needs, which the bridge learns when it connects, and when the deployment
policy allows it (see the [user guide](USER_GUIDE.md)).

**Undo:** *exact* means `live_undo` puts back the state recorded before the
change, as long as nothing else has changed it since. *Kept* means the bridge
can't bring it back but Live's own undo can (deleting a clip, track, scene or
locator, clearing a range). *Not undoable* means neither the bridge nor the
tool promises a way back (cropping a clip, clearing all envelopes, deleting a
device, rack actions such as randomizing macros). *None* means there's nothing
to undo: playback, momentary actions, reads. [Live safety](LIVE_SAFETY.md) has
the details.

**Real Live** says where an area was last exercised on real Live:
*acceptance* is Kumi's acceptance run (`accept_live`) on Live 12.4.15b5 with
bridge 1.0.63; *earlier runs* are other runs on Live 12.4.15 beta with bridges
1.0.0 to 1.0.65; *July runs* are the bridge's first runs, on Live 12.4.5b8 with
bridge 0.1.0. *Not yet* means only the tests, which run on macOS, Linux and
Windows against fake Live objects. The records are indexed in
[implementation status](IMPLEMENTATION_STATUS.md#evidence).

## Coverage by area

| Area | What's covered | Channel | Undo | Real Live |
| --- | --- | --- | --- | --- |
| Tracks and scenes | Create MIDI, audio and return tracks and scenes; duplicate tracks and scenes; rename; colour; fold and view settings; delete tracks, scenes and return tracks | Remote Script | Exact; deleting a track or scene: kept; deleting a return track: not undoable | Acceptance; earlier runs |
| Session clips and notes | MIDI clips with notes (velocity, probability, velocity deviation, release velocity, mute); note edits, quantize, duplicate; seeded MIDI transforms and generators; clip loop, launch mode and quantization, legato, colour, mute; crop, duplicate loop, scrub; delete clips | Remote Script | Exact; deletions: kept; crop: not undoable | Acceptance |
| Audio clips and files | Gain, pitch, warp mode, warp markers, fades, RAM mode; audio files into Session slots, the Arrangement or a take lane; Simpler sample replacement; samples onto Drum Rack pads | Remote Script; Extension for pad samples without the Browser | Exact | Earlier runs; warp markers not yet |
| Arrangement | Create, duplicate and move clips; locators and jumping to them; take lanes (read, rename, audio into a lane) and comps (read); MIDI clips with their notes; clearing a range | Remote Script; Extension for MIDI clips and clearing a range | Exact; clearing a range: kept | Acceptance |
| Automation | Session clip envelopes (create, insert points, delete a range, clear all); a parameter's automation value at a time; reading Arrangement automation | Remote Script | Exact; clearing all envelopes: not undoable | Earlier runs |
| Mixer and routing | Volume, pan, sends, mute, solo, cue, crossfader, split stereo; rack chain mixers; track input and output routing, arm and monitoring; device inputs and sidechains | Remote Script | Exact | Acceptance |
| Devices | Load from the Browser; parameters, on/off, move, duplicate, delete; racks, chains, drum pads, macros and variations; save, recall and morph device states; Drift, Drum Cell, EQ Eight, Hybrid Reverb, Meld, Looper, Simpler, Wavetable, Roar, Shifter, Spectral Resonator and CC Control settings; plug-in parameters, presets and editor window | Remote Script; Extension or Remote Script for duplicating | Exact; deleting a device and rack actions: not undoable | Acceptance; earlier runs (every device in the Browser) |
| Macro mappings, Follow Actions | Macro and variation names, mapping parameters to macros, rack chain zones, Session clip Follow Actions | Willington | Exact | Willington records |
| Browser and library | Search, roots, inspect, preview; Live's library database (tags, kinds, plug-in inventory; opt-in, read-only) | Remote Script; Bridge reads the database | None | Earlier runs |
| Transport and song | Play, stop, continue, position, loop, metronome, punch, tap tempo, nudge; tempo, time signature, swing, launch and record quantization; scale; the loaded tuning's name, range and reference pitch (Live loads a tuning only from its Browser, not from Python); groove pool; Link settings; Live's own undo and redo; one Live undo step for several changes | Remote Script | Exact for settings; actions: none | Acceptance |
| Playing and recording | Launch clips and scenes, hold launch buttons, guarded scene audition, emergency stop; Session and Arrangement recording; capturing MIDI and scenes | Remote Script | None (it plays); captured clips: exact | Acceptance; earlier runs |
| Offline render | An audio track's own clips, before its devices, many times faster than real time | Extension | None | Acceptance |
| Views and selection | Selected track, scene, clip, device, parameter and chain; Session or Arrangement, zoom, detail views; Live's dialogs; status-bar messages | Remote Script | Exact where Live lets it be put back; dialogs and messages: none | Earlier runs |
| Audio analysis | Loudness (BS.1770, EBU R128), true peak, spectrum and dynamics, reference comparison, diagnosis next to the Set's devices ([audio intelligence](AUDIO_INTELLIGENCE.md)) | Bridge | None | July runs; FFmpeg oracle |
| Audio capture | A track's output through Session Resampling, with consent, a watchdog and cleanup | Remote Script and Bridge | Cleaned up after | July runs |
| Projects and files | Project info and a verified backup copy; Set snapshots and diffs; reading, linting and diffing saved `.als` files without Live; importing a file into the project; Kumi's notes saved in the Set | Remote Script; Bridge for `.als` files; Extension for importing | Data notes: exact; the rest: none | July runs (info, backup) |
| Events | What changes in Live as it happens (transport, selection, names, mixer, parameters, structure, right-clicks), or observed and polled | Remote Script; Extension for right-clicks | None | Earlier runs; acceptance (right-click) |
| Realtime control | UDP JSON, OSC and XY packets (and Max-labelled ones) to armed parameters ([realtime control](REALTIME_CONTROL.md)) | Remote Script | Writes are checked; disarm stops it | July runs |
| Python inside Live | `live_run_python`, for what no other tool covers; only in the `full` policy | Remote Script | One step in Live's undo; no `live_undo` | Not yet |

## Reserved operations

The operation registry holds some operations the bridge never runs. They
refuse with the reason, and no tool offers them:

- `arrangement.automation.create`, `.delete`, `.point.insert`,
  `.point.delete`: editing Arrangement automation;
- `audio.comp.read`: comp regions as Live's comp editor shows them;
- `project.new`, `.open`, `.save`, `.save-as`, `.collect`, `.export`,
  `.bounce`: reported as limitations in the capability resource;
- `session.discover`: an alias, served by `discover`.

`browser.preview.start` and `browser.preview.stop` are also marked reserved in
[the capability manifest](../evidence/capability-manifest.json), but the
Remote Script runs them whenever Live's Browser can preview, and the bridge
offers `live_browser_preview`. Creating take lanes and MIDI clips in a take
lane are in the registry and the Remote Script, but no tool offers them.

## What Live's APIs don't offer

A census of Live 12.4.15b5's Python API ([LOM audit](../evidence/lom-audit.md))
checked every class and member against the Remote Script. What remains is out
of reach of both Live's Python API and the Extensions SDK, or out of scope:

| Not offered | What there is instead |
| --- | --- |
| Saving, opening or exporting the Set; Collect All and Save (through Live's scripting) | Through Live's own menus instead: save, new Set, Collect All and Save, export; save as and open a Set on Windows. A verified backup of the saved Set; importing a single file into the project. |
| Exporting the mix or stems; freezing and flattening | Offline render of an audio track's own clips; recording through Resampling |
| Creating group tracks | — |
| Editing Arrangement automation | Reading it; envelopes in Session clips |
| Mapping a macro or modulator to a parameter | Wavetable's and Drift's modulation matrices; macro and modulator mapping through Willington |
| Follow Actions | Through Willington |
| Comp editing, deleting or auditioning take lanes | Reading lanes and comps, renaming lanes, audio into a lane |
| Per-note MPE (pressure, slide, per-note tuning) | Probability, velocity deviation, release velocity, mute |
| A plug-in's own window or hidden state | Its parameters, presets, and opening or closing its window |
| A track's audio as it plays | Capture through Resampling; offline render |
| Browser similarity search, Packs, Cloud | Live's library database: tags, kinds, plug-in inventory |
| Preferences, audio and MIDI setup, licensing; stem separation; video tracks | — |
| Object identities that survive reopening a Set | References last one connection; discovery reads them again |

Out of scope by choice: Push and other hardware surfaces (the bridge is a
Control Surface but reads no raw MIDI), general OSC, web, serial or sensor
links (realtime control is loopback only), an external Link peer and Link
Audio, and Max for Live devices inside the bridge. Kumi makes Max for Live
devices itself; see [the Kumi guide](KUMI_GUIDE.md#making-max-for-live-devices).
