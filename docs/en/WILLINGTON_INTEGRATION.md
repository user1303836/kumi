# Willington

English · [简体中文](../zh-CN/WILLINGTON_INTEGRATION.md) · [日本語](../ja/WILLINGTON_INTEGRATION.md)

Willington is a set of native providers that reach parts of Live its Python API
doesn't: Session clip Follow Actions, rack macro mappings and names, and rack
chain zones. From the release that carries Willington's runtime files, Kumi's
bridge has them inside its Remote Script; until then, [install Willington
yourself](#installing-willington-yourself). Either way they're off until you
turn them on with `/willington`. Each provider selects
bindings for the exact Live build it's connected to; on any other build its
tools simply don't appear, and the rest of Kumi works as before.

## What it adds

| Kumi tool | Bridge tools | Edit kinds | Provider |
| --- | --- | --- | --- |
| `set_clip_follow_actions` | `live_follow_actions_preview/apply` | All ten Follow Action fields of a Session clip | WillingtonBindings |
| `edit_rack_mapping` | `live_willington_device_preview/apply` | `macro-name`, `variation-name`, `macro-mapping` | WillingtonDeviceTools |
| `map_modulator` | `live_run_python`, offered with `live_willington_device_preview/apply` | Live's LFO, Shaper and Envelope Follower (8 targets each) and Expression Control (5) mapped to parameters, with Willington's `map_modulation` | WillingtonDeviceTools |
| `edit_rack_mapping` | `live_willington_device_preview/apply` | `selector-zone`, `key-zone`, `velocity-zone` | WillingtonRackZones |

The bridge offers only the edit kinds whose provider is loaded with writes
enabled: `live_willington_device_preview` lists just those `kind` values. Every
edit needs the transport stopped.

Some rack improvements need no Willington: racks read with Live's own macro
layout, recalling or deleting a variation by index, and the fallback to the
stock modulator devices when Live's Browser has no Modulators category. They
work with any bridge.

## Turning it on and off

Type `/willington` in Kumi. It writes `willington.json` beside the bridge's
`__init__.py`, in `Remote Scripts/AbletonMcpBridge`, turning DeviceTools and
RackZones on with their edits, and the bridge loads them within a second, Live
running. Follow Actions come on too when a passing
[self-test](#follow-action-self-test) is there: their edits need one, and Live
can't unload their bindings. `/willington` again removes the file: the bridge
uninstalls DeviceTools and RackZones and turns Follow Action writes off; Follow
Actions' bindings stay until Live restarts, which Kumi says. While the bindings
are off, Kumi says so when it starts.

Kumi tells its model too. With the bindings off, a request that needs one of
their edits gets a sentence saying that `/willington` turns them on; with them
on, Kumi maps macros itself, and Live's modulators with `map_modulator`, in the
same plan that loads them, instead of asking you to map them in Live.

Which Live builds they cover depends on the Willington release you have; for
Kumi's copy, `willington/release.json` in the bridge's folder names it. Willington's
validated bindings: Follow Actions and DeviceTools for macOS ARM64 Live
12.4.15b4 and b5, Rack Zones for macOS ARM64 b5, and all three for Windows x64
Live 12.4.15b5. Intel macOS isn't covered. Each provider selects bindings using
the running Live process's OS, architecture, version and executable hash, and
the native libraries also check the running executable itself (its Mach-O UUID
on macOS, its CodeView GUID on Windows).

Follow Action edits also need a passing self-test for the selected library:
`self-test.json` in the WillingtonBindings folder with `"status": "passed"` and
`library_sha256` equal to that library's SHA-256. Without it, only Follow Action
edits stay off; macro, name and zone edits still work.

A missing validated profile skips only that component: one configuration can
use Follow Actions and DeviceTools on b4 and also Rack Zones on b5. These typed
no-profile refusals are cached and logged once per component per Live process.

Malformed configuration, missing artifacts, integrity errors, unexpected startup
failures or another active owner leave the native extensions unavailable; the
ordinary bridge stays active. Live's log (Log.txt) reports the cause and names
the providers actually active and enabled for writes. A missing or stale Follow
self-test disables only Follow writes.

When the Remote Script stops, it turns Follow Action writes off and uninstalls
DeviceTools and RackZones. Follow Action bindings can't be uninstalled: they
stay registered in that Live process, and the bridge reuses them, writes off,
when it starts again or loads the providers again after `/willington`.

### willington.json

`/willington` writes this file; you can also write it yourself. It must be a
regular file, owner-only and at most 4 KiB, with the keys below (`rackZones` and `editing` are
optional). `/willington` writes this, with `followActions` true when a passing
self-test is there:

```json
{"version": 1, "followActions": false, "deviceTools": true, "rackZones": true, "enableWrites": true}
```

| Key | Meaning |
| --- | --- |
| `version` | Always `1` |
| `followActions` | Load WillingtonBindings |
| `deviceTools` | Load WillingtonDeviceTools |
| `rackZones` | Optional; load WillingtonRackZones |
| `editing` | Optional: load WillingtonEditing. `/willington` adds `"editing": true` only with a passing exact-library receipt. |
| `enableWrites` | Allow edits; `false` loads the providers without offering edits |

The bridge reads it again within a second of its changing. Kumi updates keep it,
and it doesn't count against the bridge's installation check. Without it, the
bridge is the plain bridge.

## Installing Willington yourself

Before a release carries Willington, or for a build it doesn't carry yet (for a
new Live version, say),
install Willington's multi-version bundle into Live's Remote Scripts folder,
beside AbletonMcpBridge: the required `WillingtonRuntime` with the providers you
want, `WillingtonBindings` (Follow Actions), `WillingtonDeviceTools` (macros and
variations; it must provide `get_macro_mapping` and
`get_selected_variation_name`) and `WillingtonRackZones` (zones). Manual copies
must keep the runtime, `build/<profile-id>/` folders and manifests. Providers
installed there come before Kumi's own copy. Turn off any standalone Willington
control surface in Live and restart Live: the bridge won't share the providers
with another owner. Then turn them on with `/willington`.

## Follow Action self-test

Repeat after switching Live builds or replacing the selected Follow library. The
test runs WillingtonBindings as a Control Surface of its own, so it needs a copy
[installed beside the bridge](#installing-willington-yourself): Kumi's own copy,
inside the bridge, isn't in Live's list. This procedure works with the
distributed bundle; no source checkout or `manage.py` is needed. The standalone
test creates a fixture track and performs native writes and Live Undo in the
current Set, so use a disposable Set.

1. Disable `AbletonMcpBridge` and standalone Willington surfaces in Live’s Control
   Surface settings, then quit Live. Native Follow properties remain registered
   until the process exits.
2. Start the intended Live build, open a disposable Set with playback stopped,
   and select only `WillingtonBindings` as the Willington Control Surface, with
   MIDI input/output None. Its installed `status.json` should report
   `"status": "registered"`.
3. Queue the test with this command, replacing the folder argument with the
   installed Bindings directory. It refuses pending commands and removes the
   old receipt so it cannot be mistaken for this run.

   ```sh
   python3 - '/path/to/User Library/Remote Scripts/WillingtonBindings' <<'PYTEST'
   import json, os, sys
   from pathlib import Path
   folder = Path(sys.argv[1]).expanduser()
   assert (folder / '__init__.py').is_file(), 'Not an installed Bindings folder'
   command = folder / 'command.json'
   assert not command.exists(), 'A command is already pending'
   (folder / 'self-test.json').unlink(missing_ok=True)
   temporary = folder / 'command.json.tmp'
   temporary.write_text(json.dumps({'action': 'self_test'}) + '\n')
   os.replace(temporary, command)
   PYTEST
   ```

4. Wait for the new `self-test.json` to finish with `"status": "passed"` and a
   `library_sha256`. A running or failed report does not enable writes. Inspect
   `command-error.json` if the command fails. The hash must match the selected
   `build/<profile-id>/libwillington.dylib` (or root library for a legacy install);
   `shasum -a 256 '/full/path/to/libwillington.dylib'` prints that digest. Keep
   the receipt in the installed Bindings folder. For Kumi's own copy, with
   nothing installed beside the bridge, put a receipt for the same library in
   `AbletonMcpBridge/willington/WillingtonBindings/`: Kumi updates keep it, and
   it doesn't count against the bridge's installation check.
5. Set the standalone `WillingtonBindings` Control Surface to None, quit Live,
   and restart before enabling `AbletonMcpBridge` again. Discard the disposable
   Set. Do not select standalone Willington surfaces alongside Kumi: both would
   attempt to own native bindings. Kumi rechecks the receipt against its selected
   library before enabling Follow writes with `enableWrites: true`.

## Follow Actions

`live_follow_actions_preview/apply` sets all ten fields of one Session clip:
enabled, linked, actions A and B, chances A and B, loop count, time, and jump
targets A and B.

- Actions are numbers: 0 none, 1 stop, 2 again, 3 previous, 4 next, 5 first,
  6 last, 7 any, 8 other, 9 jump. Jump targets are 1-based scene numbers.
- The two chances add up to 100; give one and the other is set to the rest.
- Linked timing uses the loop count; unlinked timing uses `time` in beats.
- The transport must be stopped and the clip not recording.
- It doesn't change scene Follow Actions or Live's global Follow Action switch.
  For launch Legato, use the clip settings tool (`set_clip` in Kumi).

The preview captures all ten fields. If a write fails partway, the earlier
fields go back. Undo restores the captured fields, and is refused if the clip
changed since.

## Macros, variations and mappings

`live_willington_device_preview/apply` with `ref` naming a rack:

| Kind | Arguments | Notes |
| --- | --- | --- |
| `macro-name` | `macroIndex` (0–15), `name` | Renames a macro |
| `variation-name` | `name` | Renames the selected variation; a named variation must be selected |
| `macro-mapping` | `targetRef`, `mappingIndex` (0–15, or `null` to unmap), `minimum`, `maximum`, `mappingKind` | Maps a parameter inside this rack to a macro |

Mapping kinds:

- `continuous` and `enum`: `minimum` and `maximum` in the parameter's own units,
  within its range; the range may be inverted. `enum` endpoints are whole
  numbers.
- `boolean`: integer macro thresholds from 0 to 127, `minimum` ≤ `maximum`.

`targetRef` must come from fresh discovery and lie inside this rack, its nested
devices or its chains' mixers. A transaction captures the names, or the
mapping, the parameter's value and the macro values, with the identities
involved. Live sets a mapped parameter's value a tick later, so a mapping is
fenced on the mapping and the macro values, not that value. Every write is read
back and, if it doesn't match, put back exactly. Undo restores the captured
state and is refused if the rack changed since. It's the transaction's own
undo, not Live's.

## Rack chain zones

`live_willington_device_preview/apply` with `ref` naming a rack and `targetRef`
one of its regular chains:

| Rack | Zones |
| --- | --- |
| Audio Effect Rack | `selector-zone` |
| Instrument Rack, MIDI Effect Rack | `selector-zone`, `key-zone`, `velocity-zone` |

Drum Racks and return chains are refused. A zone has four integer endpoints,
`minimum`, `fadeMinimum`, `fadeMaximum` and `maximum`, which must stay in order
(`minimum` ≤ `fadeMinimum` ≤ `fadeMaximum` ≤ `maximum`) within 0–127 (1–127
for velocity). An endpoint you leave out keeps its current value, so moving a
range may mean giving both fade endpoints too.

The preview captures all four endpoints. Apply and undo fence the rack and the
chain by identity and the whole zone state; a write that doesn't read back as
asked is put back exactly.

## Evidence

| Provider | Run | Live | Bridge | Covers |
| --- | --- | --- | --- | --- |
| Follow Actions | [kumi-clip-follow-actions-b5.json](../evidence/kumi-clip-follow-actions-b5.json), 2026-09-30 | 12.4.15b5, macOS arm64 | 1.0.53 | Kumi changes and undo on a saved test Set, transport stopped |
| Follow Actions, macros, mapping | [willington-kumi-chat.json](../evidence/willington-kumi-chat.json), 2026-09-30 | 12.4.15b4 ARM64 | 1.0.52 | A real Kumi chat: Follow Actions, macro rename, mapping, each undone |
| Rack zones | [rack-zones-b5.json](../evidence/rack-zones-b5.json), 2026-10-01; completion 2026-10-02 | 12.4.15b5 (2026-09-24 build), arm64 | 1.0.66 (Kumi transactions) | Readback, write, undo and redo, save/reopen, Kumi undo; completion adds signal gating, fades and actual Max invocation |

Variation rename and the inverted continuous and enum mappings were tested
through the bridge directly. Follow Action scheduling and save/reopen persistence
of Follow Action and macro edits were not tested.

Rack Zones completion results and receipt digests are in the [public validation
summary](../evidence/rack-zones-b5.json): 42 signal-gating checks, 49 fade
measurements with 14 directional comparisons, and seven actual Max `live.object`
write/read/restore cycles. The promoted `live-12.4.15b5-arm64` library is
byte-identical to the tested candidate library. Measurements use normalized Live
meters; exact linear gain, silence at fade endpoints, held-note edits, overlapping
multi-chain crossfades and other builds/platforms are not claimed. Rack Zones
remains unsupported on b4. Raw completion receipts and the harness are retained
in the private Willington repository at the immutable commit recorded in the
summary; those raw files are not published here.

The bridge's automated tests cover the rest without Live: missing providers,
malformed config, stale and conflicting edits, partial writes, ownership and
reconnects, and lost replies.

## Deliberately unavailable

Willington has native methods for these, but Kumi doesn't offer them until
they can be undone safely:

- **Overwriting a variation**: needs reading and restoring the full stored macro
  values and enabled mask, not just the variation's name.
- **Replacing a Drum Sampler's sample directly**: needs the current sample's
  identity and path, and restoring what replacing it changes. Loading presets
  and samples through the Browser works.
- **Mapping a modulator as a bridge transaction**: the native change settles
  later, out of step with Live's thread. It needs settled-state checks, exact
  ownership of source and target, cancellation and restoration first. Until
  then `map_modulator` maps through Kumi's own Python, with a HISTORY entry:
  Kumi's undo empties a slot it filled (`map_modulation(slot, None)`), only
  while the slot still holds the parameter it mapped, and a mapping that
  replaced another is kept, since Kumi can't map the old one back. Live's undo
  for it is unverified.

A native method existing isn't enough for an undoable operation: don't offer
one by adding only a runtime descriptor or a protocol entry.

## Native editing and its self-test

Kumi offers native group creation, Arrangement automation, scene/global Follow Actions and per-note MPE when `WillingtonEditing` is installed and verified for **Live 12.4.15b5 macOS ARM64**. Other builds, Intel macOS and Windows do not support this component. `/willington` requests editing only with an owner-only `self-test.json` matching an installed library; the provider checks the library actually selected in Live again. Missing or stale evidence leaves editing off. Updates preserve the receipt, but a changed library needs a new test.

`group_tracks`, `set_scene_follow_actions`, `set_global_follow_actions`, `set_note_expression`, `edit_arrangement_automation` → `live_native_editing_preview/apply`.

Use a disposable stopped Set named `Willington Native Editing`, with two ungrouped first tracks, a MIDI clip in track 1 / slot 0 containing note ID 1, and scene 0. This test changes and restores the Set. Load the provider using the owner-only `willington.json` below, then run the Python block on Live’s main thread through an explicitly authorized `run_python` session. Replace the checkout path with your local Kumi source path. The runner tests all five kinds, writes the receipt beside the loaded package only after all checks pass, and leaves editing writes disabled. A failed run invalidates earlier evidence. This is an explicit operator procedure, not automatic startup behavior.

```json
{"version":1,"followActions":false,"deviceTools":false,"editing":true,"enableWrites":false}
```

The runner is not included in the installed bridge bundle. Clone [Kumi](https://github.com/user1303836/kumi), check out the tag matching your installed Kumi release, and use `remote-script/native_editing_self_test.py` from that checkout. Set `/absolute/kumi` below to the absolute checkout directory. During development, use the matching feature branch instead of a release tag.

```python
import importlib.util, sys, Live
spec = importlib.util.spec_from_file_location(
    "editing_fixture", "/absolute/kumi/remote-script/native_editing_self_test.py")
fixture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixture)
provider = Live._kumi_willington_owner
fixture.run(provider, sys.modules[type(provider.mapper).__module__], song)
```

Then use `/willington` to request writes. It verifies the receipt again. Four kinds support guarded HISTORY restoration within the current connection; grouping has no HISTORY inverse and retires moved references. Rediscover tracks after grouping. New MPE lanes and existing lanes to be edited are limited to 4096 events. Arrangement insertion uses linear handles only. See the [native API guide](WILLINGTON_NATIVE_EDITING.md) for the lower-level limits.
