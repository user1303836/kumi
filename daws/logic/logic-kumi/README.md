# Logic Pro bridge: M1

- Experimental foundation for Logic Pro 12 on macOS. The bridge serves only `ping` and `status`, plus `project` events, over authenticated loopback. It follows the project shown by Logic's windows and publishes its discovery file.
- This stage does not expose project rows, FOCUS, edits, listening, or rendering. The app has no Logic provider or DAW selection entry yet; this is a standalone bridge and host foundation.
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
- Optional native helper checks, without Logic: `python3 -m unittest discover -s daws/logic/logic-kumi/helper/tests -p test_protocol.py`. These compile and run the macOS helper and use disposable virtual MIDI ports.
