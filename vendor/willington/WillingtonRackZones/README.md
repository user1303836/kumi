# Willington rack zones

Exact-build rack zone access for Live **12.4.15b5, macOS ARM64**.
This separate adapter leaves the existing follow-action and device adapters intact.
Supports regular Instrument, MIDI Effect and Audio Effect Rack chains.
Native reads/writes, undo/redo, Set persistence and Kumi transactions have retained
receipts. The [completion report](../../evidence/rack-zones/b5/completion-2026-10-02/validation.json)
and [validation notes](../../evidence/rack-zones/b5/completion-2026-10-02/README.md)
record the audio-engine and Max invocation checks used for b5 support.

Build the supported b5 profile:

```sh
bash integrations/WillingtonRackZones/build.sh \
  --profile profiles/live-12.4.15b5-arm64.json
```

Install the `WillingtonRackZones` Python package and the shared
`integrations/WillingtonRuntime` package beside each other in Live's User Library
`Remote Scripts` directory. Keep the generated library and `build.json` together
under `WillingtonRackZones/build/live-12.4.15b5-arm64/`, or extract both packages
from the matrix bundle. The runtime is required for every loading route:
it verifies the running executable and library hashes before native loading.

Default `install()` selects only validated profiles from the runtime matrix.
The normal b5 profile includes Rack Zones; b4 and other builds remain unavailable.
Call `install()` from a Live main-thread callback, retain its returned handle,
and call `enable(True)` explicitly. Writes start
disabled. `uninstall()` disables writes and removes only the Python methods and
Max entries this handle added.
Executable SHA-256, library SHA-256, running Mach-O UUID, chain handle type,
native branch type, settings type, range type, integer remoteable type, and
request dispatcher are checked before access.

```python
from WillingtonRackZones.api import install
native = install()
chain.get_zone('key')  # JSON with all endpoints and lowerBound/upperBound
native.enable(True)
chain.set_zone('key', 0, 63, 0, 63)
native.uninstall()
```

`get_zone(kind)` and `set_zone(kind, minimum, maximum, fade_minimum, fade_maximum)`
are added to `Live.Chain.Chain`. Kinds are `selector`, `key`, and `velocity`.
Endpoints are integers with
`lowerBound <= minimum <= fadeMinimum <= fadeMaximum <= maximum <= 127`.
Key and selector bounds start at 0; velocity starts at 1.
Audio Effect Racks support selector zones only. Instrument and MIDI Effect Racks
support all three. Drum Rack chains, return chains, detached chains, unknown
native types, and unsupported owners are rejected.

Writes use Live's normal integer request dispatcher and its registered zone
controllers. Moving a range can move its fades; the setter expands the range,
sets the requested boundaries, then sets both fades and verifies all four values.
It does not write value fields directly. Callers should retain complete prior
state and restore it on a transaction failure.

The Kumi integration adds `selector-zone`, `key-zone`, and `velocity-zone` to
`live_willington_device_preview`, with `ref` naming the rack and `targetRef`
naming one regular chain. Supply any changed endpoints; preview fills omitted
ones from current state and rejects invalid coupled ranges and no-ops. Apply
fences the complete state and rack/chain identities. `live_undo` restores all
four prior endpoints and refuses intervening changes. Playback must be stopped.

Example tool arguments:

```json
{
  "ref": "<rack ref>",
  "targetRef": "<chain ref>",
  "kind": "key-zone",
  "minimum": 0,
  "maximum": 63,
  "fadeMinimum": 0,
  "fadeMaximum": 63
}
```

Kumi accepts the optional `"rackZones": true` field in its owner-only
`willington.json` and calls the provider's default `install()`. Install the provider,
shared runtime and matching b5 artifacts alongside Kumi's bridge. The existing
`enableWrites` flag controls writes. Reload Live and Kumi after updating their
packages so both sides use the current operation registry and tool schema.
Leaving `rackZones` absent preserves the previous configuration contract.

## Windows x64

`native_windows.cpp` implements the same checks and write sequence for Live
12.4.15b5 on Windows x64 as the `willington_zones` extension module; `api.py`
selects it automatically and default `install()` resolves the
`live-12.4.15b5-windows-x86_64` row. Validation covered the same runtime checks
as macOS: seven zone read/write/undo/redo checks, 42 gating cases, 49 fade cases
with 14 attenuation trends and seven Max `live.object` cycles
([receipts](../../evidence/live-12.4.15b5-windows-x86_64/README.md)). Set
persistence and Kumi transactions were not exercised on Windows.

## Validation scope

The completion run measures static audio-engine gating in **42 cases**: full-range
reference, interior, both inclusive boundaries and both blocked sides for all seven
rack/zone combinations. Each case starts fresh clip playback after release settling
and measures the track's stereo output meters. All **42 cases passed**.
The separate fade run passed **49 measured cases and 14 directional attenuation
trends**, covering fade-in and fade-out at middle, near-edge and edge positions
against full-range references. Actual Max **`live.object` get/set/get/restore
cycles passed for all seven rack/zone combinations**. Zone endpoints, chain
selectors and launch quantization were restored; transport was stopped. Detailed
results are in the linked completion receipts.

Meter readings establish signal presence and attenuation trends, not an exact
linear gain law or guaranteed silence at fade edges. Coverage is static: dynamic
zone changes during held notes and overlapping-chain behavior are outside this
validation. Unsupported chain types remain rejected.

The older `profiles/live-12.4.15b5-rack-zones-arm64.json` remains a historical
candidate profile. To reproduce isolated research, build it with `--candidate`
and an explicit isolated `--output`, then call `api.install(library=...)` with
that library's absolute path and adjacent generated `build.json`. This route
still requires `WillingtonRuntime` and verifies executable and library hashes;
candidate artifacts are excluded from default selection and release bundles.

Earlier evidence is retained under [b5 rack zones](../../evidence/rack-zones/b5/):
seven zone readbacks, 28 write/undo/redo/no-op cases, packaged API validation,
save/reopen comparison, seven real-Live Kumi mapper transactions, and seven
complete Kumi host preview/apply/idempotency/history-undo transactions through
the installed authenticated bridge. The Python bridge regression suite passes
452 tests; host transaction/manifest tests pass 14 and Kumi runtime change tests
pass 40. Kumi's agent-facing `edit_rack_mapping` tool inherits the new zone schema
and describes the endpoints and supported rack types.
