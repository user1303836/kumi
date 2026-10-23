# Logic Pro M1 gate, 2026-10-19

Status: passed. First for the bridge and its helper (fbcddc5b on `feat/logic`), attached to the
real Logic Pro through the Swift helper and answering a client over `kumi-daw/1`; then, with the
wire and app lanes merged (6db5dc05), for the install lifecycle, the doctor and the host's own
start of the bridge from a session. Kumi's interactive session was not run (see the end).

| | |
| --- | --- |
| Logic Pro | 12.3.1 (6682), `com.apple.logic10`, pid 48552, the lab's scratch project open |
| macOS | 26.6.1, Apple Silicon |
| Bridge | `target/debug/logic-kumi --kumi-home <scratch home> --helper target/logic-hands/kumi-logic-hands-eaf2506e88a9` |
| Helper | built by `scripts/build-logic-hands.py` in this checkout, universal, ad-hoc signed |
| Client | `cargo run -p logic-kumi --example logic_lab_client -- --kumi-home <scratch home> --watch 8` (the example was named `lab_client` then) |
| Kumi home | a scratch folder under the session's scratchpad, never `~/.kumi` |

## What happened

1. First run, with Logic showing its audio-interface notice ("The last selected audio interface
   is not available"): the bridge attached 255 ms after start, wrote
   `bridges/logic/discovery/logic-48552-9a692d42ae0aa40f.json` with `project: null`, and logged
   "busy: Logic Pro is showing a dialog". On SIGTERM it logged "stopped" and removed the file.
2. The notice was dismissed with its OK button through Accessibility.
3. Second run: attached in 23 ms, "Logic Pro can take work", the project read as `KumiLab` with
   the derived id `31a3626e1094a7ebac8e85f7f68c8ffc` and its `.logicx` path; the discovery file
   carried the project. The client proved the secret, got the welcome in 7 ms (`daw logic`,
   `dawVersion 12.3.1`, `pid 48552`, `resource com.apple.logic10`, `methods [ping, status]`,
   `events [project]`), `ping` and `status` in under 1 ms:
   `{"project": {"id": "31a3…8ffc", "name": "KumiLab", "path": "…/KumiLab.logicx"}, "busy": false,
   "permission": null}`. After SIGTERM: "stopped", no `logic-kumi` or helper process left, the
   discovery folder empty.

## The host and the app, later the same day

With the wire lane (e8ff9d86) and the app lane (6db5dc05) merged: the same Logic Pro (pid 43575)
and a fresh scratch home under the session's temporary folder.

| | |
| --- | --- |
| Kumi | `target/debug/kumi` of this checkout (Kumi 1.19.2), `KUMI_HOME` set to the scratch home |
| Host | `target/debug/daw-mcp-server --daw logic --kumi-home <scratch home>`, fed JSON-RPC on stdin by a short shell script |
| Bridge | the one `kumi --daw logic bridge` put in the scratch home: `logic-kumi` 0.1.0 staged from this checkout's build, with its helper |
| Project | `Untitled 1`, made with File > New in Logic (the lab's `KumiLab` went with the earlier scratch folder) |

1. `kumi --daw logic bridge --yes` answered in one line, "Kumi's Logic Pro bridge 0.1.0 is in
   Kumi's own folder. Kumi starts it beside Logic Pro when a session begins: open Logic Pro, then
   run … --daw logic. The first time, macOS asks to allow Kumi's helper under Accessibility.",
   exit 0. It staged the bridge as a release lays it out (`target/logic-bridge/logic-kumi/`: the
   binary, the helper `kumi-logic-hands-eaf2506e88a9`, `version`) and installed it through the
   shared lifecycle: `bridges/logic/bridge/logic-kumi/` with `bridge-reference.json`, beside
   `bridge.secret`, `discovery/`, `logs/`, `generations/0.1.0-…`, `install-receipt.json` and
   `lifecycle.lock`. Nothing went into Logic's folders, and Logic stayed open throughout.
2. `kumi --daw logic doctor`: "Logic Pro is installed here", "Logic Pro 12.3.1 at
   /Applications/Logic Pro.app", "bridge 0.1.0 installed (88ba612686cc)", "Logic Pro is running
   (pid 43575)" and the note "Logic Pro is running without Kumi's bridge: Kumi starts its bridge
   for Logic Pro when a session begins". Its exit code was 1 for three things outside Logic: no
   provider sign-in in the scratch home, and Live's bridge on this Mac (older than this
   checkout's, and not started).
3. The host, on its first `status`, started the installed bridge. The bridge attached to Logic
   278 ms after it was started ("helper started", "attached to Logic Pro 12.3.1 (pid 43575,
   com.apple.logic10); serving on 127.0.0.1:9790", "Logic Pro can take work", "project
   Untitled 1"), and the host answered `{"connected": true, "daw": "logic", "bridge": "0.1.0",
   "dawVersion": "12.3.1", "pid": 43575, "port": 9790, "resource": "com.apple.logic10",
   "project": {"id": "6525cc…e9f7", "name": "Untitled 1", "path": "…/Music/Logic/Untitled
   1.logicx"}, "busy": false, "instances": 1, "permission": null}`. `discover` listed that one
   instance with the same project. Logic's project chooser, left open as a second window behind
   the project, did not count as busy.
4. `focus`, `read` (project, then track) and `focus_body` (tree) came back as tool errors, each
   `{"error": "the Logic Pro bridge has no method \"…\"", "code": "unknown-method",
   "connected": true}`: the M1 bridge serves `ping` and `status` only, and the reads are M2's.
5. A last `status` answered as the first. When stdin closed, the host stopped the bridge: its log
   ends in "stopped", 17 s after it started; no `logic-kumi` or helper process was left, and the
   discovery folder was empty.

## The test suite

The whole isolated suite (`scripts/test-isolated.sh --no-fail-fast`) on the merge, 6db5dc05:
2965 passed, 5 failed, 6 ignored, in 332 test binaries; every Logic test passed. The five
failures are in three `kumi-runtime` files this branch doesn't change (neither the tests nor the
code under them):

- `tests/hands_transport.rs`, three tests (which ones varies): "Live didn't answer in time" from
  the fake shell helper under its 1 s limit. The file passes 8 of 8 with `--test-threads=1`. On
  this Mac (macOS 26.6.1) the first run of a freshly written script costs 250 to 400 ms, and the
  tests each write and start a helper of their own at the same time.
- `tests/library_stop.rs`: the fake measuring worker, a freshly written script, is stopped 300 ms
  after it starts and hasn't written its pid file by then: the same first-run cost.
- `tests/style_references.rs`, the MusicBrainz pacing test: the first two requests reach the fake
  server 0.6 ms apart where the test expects 100 ms or more; the later ones are 150 ms apart, as
  paced. The same on a quiet machine, run alone.

## Not covered

- Kumi's own interactive session, `kumi --daw logic`: the scratch home has no provider sign-in,
  and no credentials were copied into it. Its setup path (the bridge put in with Logic open, the
  session reconnecting without a restart, the Accessibility step read from the bridge's status)
  is covered by the app's tests over a fake setup and the simulated bridge.
- The Accessibility prompt: the grant was already held by the app the session runs in, so the
  bridge answered `permission: null` from the start and no prompt was seen.
- Reads over the real Logic: M2.
