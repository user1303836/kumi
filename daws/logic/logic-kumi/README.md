# Logic Pro bridge: M2

- Experimental native-reader foundation for Logic Pro 12 on macOS. Authenticated loopback exposes exactly `ping`, `status`, `focus`, `read`, `snapshot`, and `focus_body`, with project, transport, track, and selection events. Cached status and focus keep answering while a native read waits.
- Reads cover project, track, region, note, plugin, send, marker, and tempo rows; bodies cover `tree`, `notes`, `timeline`, and `routing`. Musical edits, listening, and rendering are not exposed. The experimental app provider is available through `kumi --daw logic` and `/daw logic`; recorded tests and real-Logic acceptance remain separate.
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
