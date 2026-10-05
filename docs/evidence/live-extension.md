# Kumi's Live extension on real Live

Measured on Live 12.4.15b5 (macOS, arm64) with the Extensions SDK 1.0.0-beta.1, 2026-09-30.
Kumi's extension is `apps/live-extension`; the bridge reaches it through `crates/ableton-mcp-server/src/bridge/`.

> A record of measurements on bridges 1.0.57 to 1.0.65, on macOS only; it isn't updated as the bridge
> changes. The Remote Script's read costs below come from before reads were paged: the later numbers
> are in "Full control and scale on real Live" in [kumi-poc.md](kumi-poc.md). The Windows folders in
> the code are unconfirmed.

## How Live runs an extension

- **Installed, Developer Mode off (a producer's Live).** Live starts every extension in
  `~/Library/Application Support/Ableton/Extensions/` when it opens, in an Extension Host of its
  own. An extension's folder is named `<author>.<name>` in lower case: Kumi's is `kumi.kumi`.
  Copying the folder there (manifest.json, package.json, dist/extension.js) is enough: no
  installer UI, no registry. Settings → Extensions → Choose file does the same copy and asks for
  a restart. Live doesn't claim `.ablx` files, so double-clicking one does nothing.
- **Live's host is Node with its permission model:** `node --permission --allow-addons
  --allow-child-process`, reading only the Extensions, Extensions Data and
  `$TMPDIR/Ableton Extensions` folders, and writing only the last two. Listening on and
  connecting to 127.0.0.1 both work. An extension's data folder is
  `~/Library/Application Support/Ableton/Extensions Data/<id>/`, its temp folder
  `$TMPDIR/Ableton Extensions/<id>/`. The host's parent is Live, and its command line carries
  `parentProcessId`.
- **Developer Mode on.** Live starts no extensions and lets one host in that someone else starts:
  `<Live.app>/Contents/Helpers/ExtensionHost/node` with `ExtensionHostNodeModule.node`'s
  `initialize({extensions: [{path, storageDirectory, tempDirectory}]})`. With Developer Mode off,
  such a host never reaches Live ("bring-up timed out (control channel handshake)") and exits.
- **One host per Live.** A second host started while one is connected never activates.

So `kumi bridge` copies Kumi's extension into Live's Extensions folder, and the bridge uses the
one Live runs (its endpoint in Extensions Data/kumi.kumi); only in Developer Mode does the bridge
start a host itself, and a start that can't reach Live isn't tried again until Live restarts.

## Offline renders (`render.offline`, the SDK's `renderPreFxAudio`)

This answers the plan's first question: **routed inputs don't render offline.**

| Track | Result |
| --- | --- |
| Empty audio track, 8 beats at 120 | 26 ms; 24-bit/44.1 kHz stereo WAV, silence |
| Audio track, input a MIDI track's Post FX (Operator playing a chord), monitoring In, 16 beats | 18 ms; silence |
| Audio track, input Resampling, monitoring In | 20 ms; silence |
| Audio clip (2 s sine, -6 dBFS, mono) on an audio track | the sine, mono (channels follow the source) |
| The same with a Utility muting the track first | the sine at -6 dBFS: pre-FX means the track's devices are left out |
| 64 beats (32 s) of audio | 88 ms, about 360× real time |
| A MIDI track | refused by Live, without a reason |
| A group track (the SDK lists it as an audio track) | refused by Live, without a reason |

Offline renders therefore cover what's recorded on audio tracks (stems of audio clips, before
their devices). Instruments, effects and the mix keep Kumi's real-time render. Live writes a render
to `$TMPDIR/Ableton Extensions/AudioRender-<livepid>-…/<clip name> [YYYY-MM-DD HHMMSS].wav`, and a
second render of the same clip within that second replaces it, so the extension moves each render
to a name of its own in its temp folder at once.

## Costs of the SDK

- Getters are synchronous in the extension: a walk of a 9-track Set's names, devices, slots, lanes,
  returns, scenes and cue points took 2.8 ms.
- `DeviceParameter.getValue` and `setValue` are asynchronous round trips of about 22 ms each; 12
  parameters read one after another took 266 ms.
- Operations through Kumi's extension, bridge to Live and back: an Arrangement MIDI clip with notes
  19–33 ms, clearing a range 16–32 ms, an 8-beat render 18–34 ms.

## The two channels agree

On the same Set (a group made with Cmd+G, a Drum Rack with chains on notes 36/38/42, an Instrument
Rack in the group, Arrangement clips made at 8, 0 and 16, locators made at 16, 4 and 8):
- the SDK's `song.tracks` includes the group track in its place (as an AudioTrack), in the Remote
  Script's order; returns and Main follow in both;
- rack chains and the devices in them are in the same positions on both;
- Arrangement clips are sorted by start on both; cue points are in creation order on both.

The extension resolves the Remote Script's positional references by walking the same collections.
Live renamed "1-MIDI" to "1-Drum Rack" when a Drum Rack went in: names used as fences are read fresh.

## Undo

- **The SDK's `withinTransaction`** groups only what starts inside its synchronous callback. A clip
  is made asynchronously and its notes need the clip, so a clip with notes is two Cmd-Z steps (the
  clip, then its notes and name). `transaction.group` makes all its clips in one transaction and gives
  them their notes in one more: two Cmd-Z for the whole group, checked in Live. On bridge 1.0.65, two
  clips with four notes each: the first Cmd-Z took the notes and names off both, the second both clips.
- **The Remote Script's undo step** (`undo.step.begin`/`end`, Live's `begin_undo_step`) groups
  Kumi's Remote Script changes across separate requests: two track renames inside one step were one
  Cmd-Z.
- It does **not** group the extension's edits: a clip with notes made inside a step stayed two steps
  of its own.
- **A producer's own edit while Kumi's step is open never joins it.** With [begin, rename A, a tempo
  drag by hand, rename B, end], Cmd-Z undid rename B, then the tempo, then rename A: the producer's
  edit is its own step and splits Kumi's step there.
- A change through the bridge's three-step authority path (preview, then preflight, prepare, invoke)
  took about 0.8 s on the small Set. (A change now reaches Live as one request; see
  [how a change reaches Live](../en/LIVE_SAFETY.md#how-a-change-reaches-live).)

## Reading a big Set: the Remote Script against the SDK

A 200-track Set: each track a copy of one template (Operator, EQ Eight, Compressor, Reverb, and an
Audio Effect Rack whose chains hold Saturator and Auto Filter, and a nested Audio Effect Rack with
Utility), 1773 devices and 591 chains in all, and a 1000-beat Arrangement MIDI clip of 20000 notes.
The Remote Script is bridge 1.0.57 through its adapter (one request each); the SDK side is a probe
extension timing the same reads with the SDK's getters.

| Read | Remote Script | SDK |
| --- | --- | --- |
| Every track (name, type, group) | 109 ms | 3.2 ms |
| Every device and chain in the Set | 4153 ms | 56 ms |
| One track's device tree | 349 ms (3 requests) | 0.2 ms |
| Operator's 195 parameters with their values | 124 ms | 6.4 ms for names and ranges, 58 ms for the values fetched together (21 ms each one by one) |
| The 20000 notes of the Arrangement clip | none (note discovery read Session clips only; it reads Arrangement clips now) | 26 ms |
| The whole Set in one snapshot | 7201 ms | — |
| Every device's parameter list (69541 parameters) | — | 23 s |
| A 20000-note Arrangement clip written | — | 277 ms (Kumi's extension) |

The SDK reads from a copy of the Set in the extension's own process: its getters are synchronous and
never wait on Live. The Remote Script's reads run on Live's UI thread, and a Set-wide device list built
the whole Set (every parameter, every note) in one request: about four seconds of Live's UI each time
Kumi looked at this Set. Reads stay on the Remote Script all the same: the SDK knows its own classes
only (Device, RackDevice, DrumRackDevice, Simpler), not a device's Live class (Operator,
InstrumentGroupDevice), and has no identity for Live's objects, which Kumi's changes check against.
The Remote Script's device, note and parameter discovery is being made to read only what's asked, and
to page by work so no request holds Live's UI for long. (It now does: a read stops after about 30 ms of
work on Live's thread and returns a cursor for the rest.)

Duplicating the template track through the bridge's three-step change path took 0.92 s at 20 tracks,
1.5 s at 80, 2.5 s at 140 and 3.8 s at 200: then, every change cost more the bigger the Set was.
Bridge 1.0.63 made a parameter, mixer or rename change cost the same on any Set. Making or duplicating
a track still costs more on a bigger Set, as it does when Live makes one itself; see "Full control and
scale on real Live" in `docs/evidence/kumi-poc.md`.

## Right-click "Ask Kumi about this"

Checked on real Live with Kumi's extension running in Live's own host. Live puts an extension's
actions in an **Extensions** submenu, prefixed with the extension's name ("kumi: Ask Kumi about this",
"kumi: Ask Kumi about this selection"). Each click reached the bridge's connection as a `pointed` event:

| Right-clicked | Event payload |
| --- | --- |
| A MIDI track's title | `{kind: "track", path: [0], name: "Template", trail: ["Template"]}` |
| A time selection on a track's Arrangement lane (bars 9–26) | `{kind: "arrangement_selection", lanes: [{kind: "track", path: [1], name: "Template"}], timeSelection: {fromBeat: 32, toBeat: 100}}` |
| An Arrangement clip | `{kind: "arrangement_clip", path: [0, 0], name: "20000 notes", trail: ["Template", "20000 notes"]}` |
| A scene | `{kind: "scene", path: [1], name: "", trail: [""]}` (an unnamed scene: Kumi says "Scene 2") |

The SDK's menu scopes are clips, tracks, clip slots, scenes, Simpler, samples and Drum Racks, and the
selections of clip slots and Arrangement lanes: there is no scope for other devices, so right-clicking
Operator shows only Live's own items.
