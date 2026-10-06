# Willington Device Tools

Experimental extensions for **Live 12.4.15b4 (2026-09-17_a0ac16f342), macOS ARM64**.
They add Python Remote Script methods and expose those methods to Max's LiveAPI.
The executable is not patched on disk. Registration lasts for the Live process;
disconnecting this Control Surface removes its methods and disables its writes.

**Status:** native operations have passed fixture tests in Live and a 27-check Max
LiveAPI suite. Mapping creation/removal passed undo and redo. Continuous and enum
ranges use parameter units; boolean controls have a separate macro-threshold method.
The adapter remains experimental and restricted to this exact build. See the
[investigation and evidence](https://github.com/cyclesonata/ableton-decomp/blob/main/research/api-gaps/README.md).

## Build and package

Requires Apple Silicon macOS and Xcode Command Line Tools:

```sh
bash integrations/WillingtonDeviceTools/build.sh
python3 integrations/WillingtonDeviceTools/test_offline.py
python3 integrations/WillingtonDeviceTools/package.py
```

The ZIP is `build/WillingtonDeviceTools.zip`. It contains the loader, adapter,
profiles, compiled library, host manifest, documentation, default configuration
and required shared `WillingtonRuntime` package.
The native library and its `build.json` manifest are stored together in `build/`.
No fixture runner is included. Building or packaging does not operate Live.

To install later, extract both the `WillingtonDeviceTools` and `WillingtonRuntime`
directories into the Live User Library's `Remote Scripts` directory. Restart Live and select **WillingtonDeviceTools**
as a Control Surface with no MIDI ports. The package has `enable_writes: false` in
`config.json`. Set it to `true` and reload the surface only when deliberately
testing the extension. Live's Log.txt reports registration or refusal details.

Default installation selects exactly one validated profile from the runtime matrix
using the running Live version, platform, architecture and executable hash, then
verifies the selected library against its manifest. The full executable SHA-256 and
running Mach-O UUID must match `build.json` and the native adapter. The recorded
build-machine executable path does not identify the running Live installation.
Unsupported builds and missing artifacts are refused before native loading.
A different Live build requires fresh investigation; changing the hash alone is
insufficient. The library is ad-hoc signed locally.
After rebuilding native code, restart Live; `dlopen` can reuse an already loaded image.

## Python API

An existing Remote Script may instead call `install()` in a main-thread callback.
Install `WillingtonRuntime` beside `WillingtonDeviceTools` for this route too:

```python
from WillingtonDeviceTools.api import install
from WillingtonDeviceTools.browser import get_modulators

adapter = install()                   # writes initially disabled
adapter.enable(True)

# rack and target are existing Live RackDevice and DeviceParameter objects.
rack.map_macro(0, target)             # Macro 1; indices are zero-based
rack.set_macro_mapping_range(target, target.min, target.max)
rack.rename_macro(0, 'Tone')
rack.unmap_macro(target)
# Boolean controls use macro-input thresholds, not parameter units:
# rack.set_macro_switch_range(boolean_target, 32, 96)

rack.store_variation()                # existing Live API
rack.rename_selected_variation('Verse')
rack.overwrite_selected_variation()   # current mapped macro values
rack.recall_selected_variation()      # existing Live API

modulator.map_modulation(0, target)
target = modulator.get_modulation_target(0)
modulator.map_modulation(0, None)

drum_sampler.replace_sample('/absolute/path/to/sample.wav')
items = get_modulators(application.browser)  # real, loadable BrowserItems

adapter.uninstall()                  # use on script disconnect
```

Do not install through both this entry point and the Control Surface simultaneously.
An installation replaces its own previous registrations; collisions with other APIs
are rejected. Uninstall preserves unrelated methods and Max whitelist entries.
Loaded native images are retained until process exit.

| Method | Contract |
| --- | --- |
| `RackDevice.map_macro(index, parameter)` | Index 0–15; target must belong to this rack's native mapping context. |
| `set_macro_mapping_range(parameter, minimum, maximum)` | Target must already be mapped; continuous or enumerated parameters. Enum endpoints must be whole numbers. Endpoints use `parameter.min`/`max` units, not displayed units. Inverted endpoints are allowed. |
| `unmap_macro(parameter)` | Target must have a macro mapping belonging to the rack. Uses its own native undo transaction. |
| `set_macro_switch_range(parameter, minimum, maximum)` | Boolean target must already be mapped. Inclusive on-interval in macro units, with integer thresholds `0 <= minimum <= maximum <= 127`. |
| `rename_macro(index, name)` | Unicode text; embedded NULs rejected. |
| `get_macro_mapping(parameter)` | Read mapping index, endpoints and kind as JSON, or `null`. Used by Kumi to capture prior state. |
| `get_selected_variation_name()` | Read the selected variation name, including Unicode. Does not expose stored macro contents. |
| `rename_selected_variation(name)` | Requires a selected variation. |
| `overwrite_selected_variation()` | Captures current mapped macro values into the selected variation. |
| `MaxDevice.map_modulation(slot, parameter_or_None)` | Fingerprinted bundled LFO, Shaper and Envelope Follower: slots 0–7. Expression Control: 0–4. Unsupported targets and targets occupied by another modulation source are rejected. |
| `get_modulation_target(slot)` | Returns `DeviceParameter` or `None`; patch processing is asynchronous. |
| `DrumCellDevice.replace_sample(path)` | Drum Sampler's internal class is DrumCellDevice. Existing file required; Live validates audio loading. |

The modulator adapter resolves persistent Max object paths and validates parameter
signatures. It uses private `_MxDCore` internals and does not support arbitrary Max
devices, modified stock patches, or unverified stock-device revisions. It routes
through the bundled patch and clears signal endpoints when unmapping. Modulator undo and exact UI state equivalence remain unverified. First and last
slots have signal-count coverage; this does not prove every stock-device UI behavior.

`get_modulators()` is a Python compatibility helper, not a new browser category in
Live or Max. It prefers a populated native category and otherwise resolves the four
known stock URIs through Audio/MIDI Effects. Unavailable devices are omitted.

Mapping methods own their native undo transactions. Do not wrap them in additional
`Song.begin_undo_step()` / `end_undo_step()` calls: testing that combination produced
an extra parameter-change undo entry. Standalone mapping creation and removal have
passed undo/redo. Range undo grouping across multiple operations is not guaranteed.

## Max LiveAPI

Once registered and explicitly enabled, use ordinary LiveAPI calls:

```javascript
rack.call('map_macro', 0, 'id', parameter.id);
rack.call('set_macro_mapping_range', 'id', parameter.id, 0.2, 0.8);
rack.call('rename_macro', 0, 'Tone');
rack.call('unmap_macro', 'id', parameter.id);
rack.call('set_macro_switch_range', 'id', booleanParameter.id, 32, 96);
rack.call('rename_selected_variation', 'Verse');
rack.call('overwrite_selected_variation');
modulator.call('map_modulation', 0, 'id', parameter.id);
modulator.call('get_modulation_target', 0); // id response
modulator.call('map_modulation', 0, 'id', 0);
drumSampler.call('replace_sample', '/absolute/path/to/sample.wav');
```

## Development tests

The fixture runner is separate: `WillingtonApiTests` imports
`WillingtonProbe.api_test_runner`. Its `workspace.json` must explicitly identify
the checkout, using `workspace.example.json` as a template. Without that file it
does not poll. It consumes one `.runtime/api-tests/command.json` at a
time and renames it before execution. Every command must include `expected_set`
matching the current Set name. Example after opening the saved fixture:

```json
{
  "action": "regression",
  "expected_set": "Willington API Reload",
  "library": "/absolute/path/to/a/fresh/libwillington_devices.dylib",
  "output": "/absolute/path/to/regression-result.json"
}
```

The asynchronous regression checks mapping events, inverted endpoints, removal
undo, all four modulators' last slots, and actual native modulation source counts.
It disables writes on completion/failure and stops if the Set changes. Use only the
saved disposable fixture with its named rack, devices and existing initial mappings;
this is a development harness, not a general fixture generator for arbitrary Sets.

Build the separate on-load Max test in a disposable project directory:

```sh
python3 integrations/WillingtonDeviceTools/build_test_device.py \
  --output-dir '/absolute/path/to/disposable/project'
```

It generates an AMXD, a silent 0.1-second WAV and uses an output report in that
directory. Loading the device runs the tests and changes the named fixture devices.
The current 27-check source covers continuous, enum and boolean ranges, variation
editing, Shaper mapping and sample replacement. Do not run it concurrently with the Python regression.

## Kumi transaction readback

The optional Kumi integration adds native readback for rack mapping index/range/type
and the selected variation name. Continuous, enum and boolean mapping edits, macro
rename and variation rename were verified through Kumi preview/apply/history undo
on the Live 12.4.15b4 ARM64 fixture. Reports are in
`evidence/device-tools/b4/kumi-*.json`. Mapped parameter values settle asynchronously;
Kumi fences the mapping and driving macro values, and captures the independent
parameter value for restoring an unmapped target. This does not establish full
variation-content or Drum Sampler sample-state restoration.


## Version profiles

Build-specific addresses, layouts and identities now live under `profiles/`.
The default remains b4. Add `--profile profiles/live-12.4.15b5-arm64.json` to
`build.sh` for b5; its output is isolated under `build/live-12.4.15b5-arm64/`.
Builds do not install or change Live. Generated `build.json` travels with its
library; do not substitute the source checkout's legacy b4 manifest.

The b5 native/Max/Kumi regression results and the isolated bundled-Max startup
limitation are documented in [b5 validation](../../evidence/live-12.4.15b5-arm64/README.md).
See the [release workflow](../../scripts/releases/README.md) for research builds,
validation gates, installation paths and rollback. Supporting a new build never
means just changing the accepted hash.

## Windows x64

`native_windows.cpp` ports the adapter to Live 12.4.15b5 on Windows x64 as the
`willington_devices` extension module (`live-12.4.15b5-windows-x86_64` profile),
loaded by `WindowsNative` in `api.py` with the same Python and Max contract.
Build it with `build_profile.py` (see the [release workflow](../../scripts/releases/README.md#windows-x64))
and package it with `package.py --build-dir build/live-12.4.15b5-windows-x86_64`.
On Windows, `replace_sample` takes an absolute Windows path; Live validates it.

[Windows validation](../../evidence/live-12.4.15b5-windows-x86_64/device-tools/validation.json)
passed 29 checks: macro mapping, continuous/enum/boolean ranges, removal and
their undo/redo; Unicode macro rename with undo/redo; variation rename,
overwrite and recall; Drum Sampler sample load and replacement with undo/redo;
and first/last-slot mapping of all four stock modulators with native source
counts. Max LiveAPI calls, variation rename undo, Set persistence and Kumi
transactions are not yet validated on Windows. The
[static mapping record](../../evidence/live-12.4.15b5-windows-x86_64/device-tools/static-mapping.json)
explains each address and layout.
