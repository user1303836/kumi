# Logic Pro bridge: M3

- Experimental bridge for Logic Pro 12 on macOS. The six observation methods remain; M3 adds `change_preview`, `change_apply`, `change_restore`, `transport_stop`, `transport_play`, `cursor_set`, `undo_step_begin`, and `undo_step_end`, plus value-proven `undone_in_daw` events. Cached status and focus keep answering while a native request waits.
- Reads cover project, track, region, note, plugin, send, marker, and tempo rows; bodies cover `tree`, `notes`, `timeline`, and `routing`. M3 changes cover five track controls, track rename/add/delete, marker add/move/delete, tempo, note add/change/delete, and plug-in bypass. Each apply checks its preview fence and reads back the result. Listening and rendering are not exposed. The experimental app provider is available through `kumi --daw logic` and `/daw logic`; recorded tests and real-Logic acceptance remain separate.
- Sample rate is optional: a background probe reads only bounded XML metadata at `Alternatives/000/MetaData.plist`. Binary, inaccessible, symlinked, or uncertain metadata stays unknown.
- `kumi-logic-hands` is a separate child process. Missing Accessibility permission is reported as `permission: "accessibility"`, separately from `busy`. macOS may ask for the helper's permission when the bridge first attaches. Nothing is installed into Logic, and no Logic restart is required.

Run these commands from the repository root, one build at a time:

```sh
python3 scripts/build-logic-hands.py
cargo build -p logic-kumi
sh scripts/test-isolated.sh --profile ci -p logic-kumi
```

The helper build needs macOS and Xcode command line tools. It creates a universal helper named by its source digest under `target/logic-hands/`. The isolated Rust tests use disposable helpers and need neither Logic nor an Accessibility grant.

For a direct development run, with Logic already open:

```sh
target/debug/logic-kumi --kumi-home /absolute/path/to/disposable-kumi
cargo run -p logic-kumi --example logic_lab_client -- --kumi-home /absolute/path/to/disposable-kumi
```

- The bridge finds the built helper automatically; `--helper <path>` overrides it. A direct run ends when Logic quits or the bridge receives SIGINT, SIGTERM, or SIGHUP. Stdin EOF does not end it.
- The host adds `--hosted`: closing its stdin pipe also stops the bridge. Both shutdown paths remove discovery and stop/reap the helper, including an unanswered helper request. The bridge does not launch or quit Logic.
- Observation opens no virtual MIDI ports. `--mcu` explicitly opts in to a virtual Mackie Control surface: Logic may save its control-surface configuration. Stopping the helper closes the ports; it does not remove that saved configuration. Automatic restoration has not been established.
- Optional native helper checks, without Logic: `python3 -m unittest discover -s daws/logic/logic-kumi/helper/tests -p test_protocol.py`. These compile and run the macOS helper and use disposable virtual MIDI ports.

The native replay uses the four original M2 scenes and the 26-request vector from `d821e669`. `tests/fixtures/sources.json` records their source paths and SHA-256 hashes; tests verify unchanged source bytes and compare the actual reader answers without blessing. Recorded replay is not native acceptance on the current Logic or app. Read operations may navigate Logic's panes through the helper, but do not select tracks or regions. Notes require the requested region to be the sole selected region; another selection is refused without changing the producer's selection.

- Restores set values back as new writes. Deleted tracks and deleted markers require manual Undo in Logic: its Edit-menu titles do not identify Kumi's specific native step, and a marker's original name cannot be set by these channels. The bridge refuses those restores without pressing Undo or recreating a differently named marker. Title-only marker, note, and structural undo recognition is unsupported; property/name and tempo reversions need observed value evidence.
- Twenty additional scenes are copied byte-for-byte from M3 commit `8d609d6a`; `tests/fixtures/change-sources.json` records provenance and hashes. Seventeen scenes replay fully (with unsafe structural title attribution explicitly removed); three replay only to the refused native-Undo boundary, with the dependent recorded suffix retained and counted as unconsumed. This is historical replay evidence, not current real-Logic acceptance.
- Editing notes refuses another region's selection and checks the requested sole selection while the Event List is open. Observation still makes no selection writes. Ambiguous reference mappings refuse restores; missing or empty screen frames never produce coordinate clicks.

- Plug-in bypass fences include the owning track, slot number, complete ordered plug-in chain, names and instrument/effect kinds, plus stable AX roles/subroles/identifiers when present. Apply and restore read them afresh; missing, ambiguous or incomplete identity refuses, including an already-reverted bypass or `discard: true`. The bypass value and volatile UI state are excluded from identity. The original bypass replay explicitly derives the added opaque identity fields from its recorded Mixer facts.
- Logic's slot does not expose a durable plug-in instance identity or component class. A plug-in removed and recreated with exactly the same observable identity (an ABA change), or a change between the last read and AX dispatch, cannot be distinguished by these checks. This remains a native acceptance limitation; no current real-Logic mutation run has been performed. Undo bookkeeping observes titles without posting synthetic shortcuts.
