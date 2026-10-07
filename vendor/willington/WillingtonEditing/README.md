# Native editing

Exact-build adapter validated for **Live 12.4.15b5 macOS ARM64** within the
[documented fixture coverage](../../evidence/native-editing/b5/README.md). All four
feature families are included in normal matrix bundles and default profile
selection. Writes remain disabled until explicitly enabled. No Windows support
is claimed. Kumi’s separate integration status is described below.

```sh
python3 scripts/releases/build_profile.py WillingtonEditing \
  --profile profiles/live-12.4.15b5-editing-arm64.json
```

With this checkout's `integrations` on Live's Python path, import
`from WillingtonEditing import api` and call
`api.install()` on Live's main thread. Retain the returned adapter and call `enable(True)` only in
a stopped Set. Use a disposable fixture for validation. Writes default to disabled.
Uninstall disables writes and removes only this adapter's Python methods and Max whitelist entries.
Replacement preflights the new library and API collisions before retiring the
old adapter; patch-install failures restore its methods, whitelist and write
state. Retired handles cannot enable writes or invoke retained methods, and
repeated teardown cannot disable their replacement.
Calling `install()` without a path uses validated-only runtime resolution and
selects the validated exact-build library. Unsupported builds raise
`ComponentUnavailableError` without replacing an existing installation. A missing
or tampered artifact for a supported build remains a hard error. Explicit
`install(library=...)` is available for isolated builds and still verifies identity
and integrity.
Undoable writes use explicit Song undo boundaries around native request scopes;
consecutive Max calls therefore retain separate undo entries.

## Group tracks

`song.group_tracks(*tracks)` groups 1–128 explicit contiguous top-level audio/MIDI
tracks in Song order and returns the group. It operates independently of UI
selection. Duplicate, foreign, nested, return/master and existing group tracks
are rejected. `song.ungroup_track(group)` accepts top-level groups with audio/MIDI
members and no devices or nested groups. Both require stopped transport.

Native readback verifies topology. Python and actual Max tests cover mutation,
undo/redo, volume/pan/mute preservation, and Main routing restoration. Additional
Python tests preserve custom track/Sends Only outputs, inputs, monitor state and
arm state. Native track-list listeners fire. This is bounded fixture coverage,
not exhaustive coverage of every routing configuration.

## Scene and global Follow Actions

`scene.get_follow_actions()` returns JSON.
`scene.set_follow_action(field, value)` accepts `enabled`, `action_a`, `action_b`,
`chance_a`, `chance_b`, `jump_a`, `jump_b`, `time`, `linked`, `loop_count`.
Actions are integers 0–9, chances 0–100 with native A/B coupling, jumps 0–8388608,
loops 1–1073741823. Jump targets are 1-based scene numbers; 0 preserves the unset
UI target. Time is quarter-note beats, minimum 0.25. Scene setters are undoable.

`song.get_follow_actions_enabled()` reads the global boolean;
`song.set_follow_actions_enabled(value)` writes a boolean or integer 0/1.
The global performance switch does **not** enter Live undo history. Restore it
with an explicit inverse setter, never `song.undo()`.

UI-launched scene tests honor the global switch. Linked mode used the longest
clip's four-beat loop times a loop count of two: the measured jump occurred after
8.02 beats. Python `Scene.fire()` did not reproduce UI scheduling in the earlier
experiment. No scene/global property-listener API is exposed by this module;
clients should reread state after writes.

## Per-note MPE

`clip.get_note_expression(note_id, dimension)` returns JSON with `exists`,
`events`, `unit`, and `time_origin`. Dimensions are `pitch`, `slide`, `pressure`.
Events are `[time, value, x1, y1, x2, y2]`: beats from the note start, pitch in
cents, and slide/pressure in floating-point MIDI units (0–127).
`clip.replace_note_expression(note_id, dimension, state_json)` replaces one lane.
A state contains boolean `exists` and an event list. Absent and explicitly empty
lanes are distinct. API bounds: ±4800 cents, 65536 events, nonnegative times
through 1576800 beats, at most two coincident events, curve coefficients 0–1.
Stale IDs and audio clips are rejected.

The native MIDI-note edit scope updates playback snapshots and Clip notes
listeners. Fixture playback through an MPE-enabled Max recorder verified all
three dimensions, restoration, and independent-lane undo/redo. Readback-only
prototype tests missed stale playback; the failure and correction are recorded.
Save/reopen, actual Max mutation, curves, and absent/empty roundtrips also passed.

## Arrangement automation

Continuous float parameters expose track-owned methods:

- `track.get_arrangement_automation(parameter, start, end)` returns interval JSON.
- `track.insert_arrangement_event(parameter, event_json)` inserts an event and
  creates the envelope if absent.
- `track.delete_arrangement_events(parameter, start, end)` deletes an interval,
  retaining its envelope and hidden initial value.

Events use the same six-number curve representation. Times are absolute Song
beats in 0–1576800; values use public parameter units. Cross-track and quantized
parameters are rejected. Insertion accepts monotonic 0–1 curve coefficients.
Reads are bounded to 65536 events. Native float rounding applies to public values.
Insertion also uses Live's native curve normalization: a final point can read
back with all four handles reset to 0.5 even when different handles were supplied.
Always reread the effective event geometry after insertion. Acceptance of an
input curve does not guarantee that Live retains those coefficients.
Playback reached inserted panning values and emitted parameter value notifications.

Interval reads are **not complete snapshots**. For lossless event restoration:

- `track.get_arrangement_snapshot(parameter)` captures all raw stored events,
  including the hidden initial-value event and coincident/curve data.
- `track.restore_arrangement_snapshot(parameter, snapshot_json)` restores that
  snapshot through native list requests, with native undo/redo and exact readback.

Treat snapshot JSON as opaque: do not edit its raw values or signature. Snapshots
are authenticated with an adapter-local key and bound to their parameter handle.
An opaque owner token also retains and checks the original Song, track and
parameter objects, so a recycled pointer alone cannot authorize restoration.
The cache holds 128 parameter owners in insertion order; capturing a 129th owner
evicts the oldest and invalidates its snapshots. Snapshots also expire when their
objects are deleted, the adapter is reinstalled, or Live restarts; they are unsuitable
for durable history across sessions. Uninstall releases the retained owners.
Numeric signing normalizes integer/float JSON forms and both signs of zero.
Snapshots preserve event data and envelope
presence, not UI selection or event-object identity. Pending UI transformations
are rejected. An empty envelope retains its initial event; absence removes the
envelope. Snapshot bounds include the initial sentinel and up to 65536 events,
with ordinary event times limited to ±1576800 beats. Raw values avoid lossy
parameter-domain roundtrips. Restoration of initial values, curves, coincident
events, absence, empty envelopes, and undo/redo passed Python and Max tests.

## Fixture tools and evidence

Build `build_test_device.py --output-dir /path/to/fixture` and load the generated
device only in the stopped disposable Set `Willington Native Editing`, with the
MIDI clip at track 1 / slot 0 / note ID 1 and native writes explicitly enabled.
It runs edits on load and reports restoration to `editing-max-report.json`.
The final Max regression passed 16 checks, including snapshot JSON and undo/redo.
Use `--script integrations/WillingtonEditing/group_validation.js` for grouping;
that test additionally requires two ungrouped audio tracks at indices 7 and 8,
named `Willington Max Group A` and `Willington Max Group B`.

`build_mpe_capture.py --output-dir /path/to/fixture` builds a passive MPE MIDI
effect. Load it before the instrument and play the edited clip. It blocks outgoing
MIDI and writes bounded raw MIDI and parsed events to `mpe-playback-capture.json`.
Its `is_mpe` attribute requests per-note data. Pitch bytes describe Live's internal
Max encoding, not an external synthesizer's bend-range configuration. See the
[Cycling '74 patcher](https://docs.cycling74.com/reference/p/) and
[mpeparse](https://docs.cycling74.com/reference/mpeparse/) references.

Generated devices reference checkout JavaScript by absolute path. Failed mutation
tests require inspecting/restoring the fixture before retrying. Disable writes
when finished. See the [evidence index](../../evidence/native-editing/b5/README.md)
for measured coverage and corrected failures. Run the 19 offline adapter tests
with `python3 integrations/WillingtonEditing/test_offline.py`.

`consumer_validation.py` exercises the public Python methods through a bridge
`python.run` context (`Live`, `song`, `result`). It disables writes and verifies
that attempted edits refuse without changing the fixture. Python access does
not create Kumi history entries. Native methods keep their own undo boundaries;
several calls inside one script can produce several Live undo steps.

## Kumi integration

The native implementation was merged in
[Willington PR #11](https://github.com/xonedsp/willington/pull/11). The profile
is now validated for the exact macOS ARM64 b5 build and included in normal matrix
bundles. Kumi's [documentation PR #248](https://github.com/user1303836/kumi/pull/248)
explains standalone use in English, Japanese and Simplified Chinese.
[Runtime draft #249](https://github.com/user1303836/kumi/pull/249) adds separate
preview/apply transactions; it is not part of Kumi's currently bundled support.

The runtime draft requires both negotiated editing operations and writable kinds
before exposing its tools. Write enablement requires a self-test receipt for the
exact compiled library. It fences edits by object identity, connection epoch and
state revision, retains private prior state for explicit restoration, and handles
same-key retries after lost replies. Its contract is narrower than the native API:

| Operation | Kumi runtime draft |
| --- | --- |
| Group creation | Contiguous top-level tracks; no Kumi history inverse |
| Scene/global Follow Actions | Stopped edits with guarded explicit restoration, including the global switch |
| Per-note MPE | Stable note ID, one dimension, at most 4096 events per replacement |
| Arrangement automation | Continuous track-owned parameter; new points require four 0.5 curve handles; restoration uses the complete opaque snapshot |

Existing Arrangement curves are preserved by snapshot restoration. Snapshot
expiration can prevent later history restoration, even within one connection.
The draft does not use blind Live undo to compensate for an edit. Its
[candidate fixture evidence](https://github.com/cyclesonata/kumi/blob/497a0936/docs/evidence/willington-native-editing-candidate.json)
covers Kumi mapper operations through authenticated `python.run`, including all
four reversible kinds and group creation followed by explicit native cleanup.
This is not evidence of a released bundle or end-to-end chat transport.

Kumi's approved import of a carrying Willington bundle, production write
evidence and Kumi's protocol release remain prerequisites for shipping its
transaction tools. Durable cross-session snapshots and scene/global observers are not
provided by this component.
