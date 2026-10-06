# Willington follow-action bindings

Experimental native Clip properties for **Live 12.4.15b4 (2026-09-17_a0ac16f342), Apple Silicon**. All ten properties below are registered on `Live.Clip.Clip` and exposed to Max for Live. Each supports get, set and observation, plus Python's usual `add_<property>_listener`, `remove_<property>_listener`, and `<property>_has_listener` methods.

| Property | Values / units |
| --- | --- |
| `follow_action_enabled` | Boolean |
| `follow_action_linked` | Boolean; use clip loop length when linked |
| `follow_action_a`, `follow_action_b` | Integer action IDs below |
| `follow_action_chance_a`, `follow_action_chance_b` | Whole percentages 0–100; returned as floats. Setting either updates the other through Live's controller. |
| `follow_action_loop_count` | Integer multiplier, 1–1,073,741,823 |
| `follow_action_time` | Unlinked duration in quarter-note beats, minimum 0.25; double precision |
| `follow_action_jump_a`, `follow_action_jump_b` | Whole scene numbers, **1-based**, returned as floats; 1–8,388,608. A number does not guarantee that scene exists. |

Action IDs: **0** No Action, **1** Stop, **2** Play Again, **3** Previous, **4** Next, **5** First, **6** Last, **7** Any, **8** Other, **9** Jump. The jump property selects the target when its action is Jump. Unlinked time and linked loop count are separate stored values.

These extend clips only. Scene follow actions and the global Follow Actions switch are outside this implementation.

## Windows x64

`native_windows.cpp` registers the same ten properties on Live 12.4.15b5 for
Windows x64 through Live's own registrars, as the `willington_bindings` extension
module selected by the `live-12.4.15b5-windows-x86_64` matrix row. MSVC inlines
Live's float registrar, so the chance and jump properties register through its
double registrar; Python and Max still receive floats and Live still stores floats.
Validation passed 26 write, notification and undo cases plus 19 invalid-value
rejections ([receipts](../../evidence/live-12.4.15b5-windows-x86_64/follow-actions/validation.json)).
The Max LiveAPI self-test has not been run on Windows. Install with
`manage.py install <destination> --build-dir integrations/WillingtonBindings/build/live-12.4.15b5-windows-x86_64`.

## Use from Max

After enabling writes in the Remote Script, a Max JavaScript client can use:

```javascript
// Initialize from live.thisdevice, on the low-priority queue.
var clip = new LiveAPI(null, "live_set tracks 0 clip_slots 0 clip");
clip.set("follow_action_a", 4);          // Next
clip.set("follow_action_chance_a", 75); // B becomes 25
clip.set("follow_action_linked", 1);
clip.set("follow_action_loop_count", 2);
clip.set("follow_action_enabled", 1);

var watcher = new LiveAPI(function (message) { post(message + "\n"); }, clip.path);
watcher.property = "follow_action_a";
```

Standard `live.object` property messages use these same names. Runtime validation used Max JavaScript's LiveAPI bridge; a separate `live.object` patch was not tested. Defer writes made in response to notifications, as with existing Live properties.

## Build and install

Requires Xcode command-line tools and the exact supported Live executable at the path in `build.json`.

```sh
bash integrations/WillingtonBindings/build.sh
python3 integrations/WillingtonBindings/build_test_device.py
```

Quit Live before installing or replacing the library. Supply your own User Library path:

```sh
python3 integrations/WillingtonBindings/manage.py install '/path/to/User Library/Remote Scripts/WillingtonBindings'
```

The installer also installs the required shared `WillingtonRuntime` package beside
`WillingtonBindings`. Keep both packages when copying an installation manually.
Default installation selects exactly one validated profile from the runtime matrix
using the running Live version, platform, architecture and executable hash, then
verifies the selected library against its manifest. Unsupported builds and missing
artifacts are refused before native loading.

Launch Live and select **WillingtonBindings** as a control surface, with Input and Output set to None. Reads and listeners are available after registration; writes start disabled. In a disposable test Set, run:

```sh
python3 integrations/WillingtonBindings/manage.py self_test '/path/to/User Library/Remote Scripts/WillingtonBindings'
python3 integrations/WillingtonBindings/manage.py status '/path/to/User Library/Remote Scripts/WillingtonBindings'
```

Wait for the self-test report to say `passed`, then:

```sh
python3 integrations/WillingtonBindings/manage.py enable_writes '/path/to/User Library/Remote Scripts/WillingtonBindings'
```

Enabling requires a passed self-test for the **current library hash**. Writes reset to disabled when the script starts again, including Set changes; repeat `enable_writes` as needed. `disable_writes` turns them off. Commands are asynchronous; inspect the reports for results.

The self-test creates or reuses only the uniquely named `Willington Follow Action Test` track and its `Willington native binding fixture` clip. It exercises writes and undo, so use a disposable Set without concurrent editing. The fixture remains afterward. Failed tests disable writes and remove listeners, but may leave the last test value for inspection or undo.

To test Max, put `build/Willington Follow Action Test.amxd` in the test Project and double-click it in **Live's browser** after enabling writes. Live's File → Open dialog opens Sets, not this device. The generated device references this checkout's absolute paths; rebuild it after moving the checkout. It changes only the named fixture and restores values after each case; its report is `.runtime/follow-actions/mfl-self-test.json`.

Unloading the script disables writes. The library stays mapped because registered callbacks remain attached to the Clip class until Live exits. Restart Live to remove the added properties completely. This is a development integration, not a packaged end-user device.

## Verified behavior

- [Native report](../../evidence/follow-actions/b4/native-extended-self-test.json): **26 cases passed**, covering all ten properties and all action choices, with write readback, notifications, write guards, undo restoration of every field, and undo notifications. Invalid inputs and listener removal passed.
- [Max report](../../evidence/follow-actions/b4/mfl-extended-self-test.json): **26 cases passed** through actual Max for Live LiveAPI, including property discovery, writes, notifications and restoration of every field.
- Live's UI showed Jump target **1** for the native default **1**, confirming the documented numbering. The temporary action selection was undone.
- Native build passed `-Wall -Wextra -Werror`; registration refused a non-Live host before calling native addresses.

This does not establish exhaustive coverage of audio clips, Arrangement clips, playback timing, saved-Set persistence of every new value, or future Live builds.

## Implementation and provenance

The extension calls Live's native property registrars and remoteable controllers. No writes directly modify stored values. Controllers preserve chance coupling, undo and notifications. Jump setters use raw-value controller dispatch: the float subclass's UI conversion otherwise introduces an off-by-one error.

The executable hash and running Mach-O UUID are checked before registering build-specific callbacks. No executable patch or application rebuild is involved. A different build requires renewed verification.

The b3 and b4 main ARM64 code sections and function starts are identical, as are both data segments. Existing b3 Ghidra addresses are reused based on [code-equivalence.json](../../evidence/follow-actions/b4/code-equivalence.json). This is verified analysis reuse, not a newly completed full b4 decompilation. The original b3 checkpoint remains intact.


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
