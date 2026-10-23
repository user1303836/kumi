# Logic Pro M2 gate, 2026-10-19

Status: passed for everything that runs without an operator (18 of 18 checks). The four checks with
an operator at Logic (a selection followed, a dialog counted as busy, refs held across a track move,
a project switched) are still to run: the Mac was in use when the run reached them, and they need
Logic in front. Scope: `feat/logic` at 245e932a, the bridge reading the real Logic Pro through
Accessibility and the Mackie channel, Kumi's host starting it from a session, and Kumi's runtime
reading the rows, the focus and the four bodies through it. Paths in this record and in the recordings
show the Mac's home directory as `/Users/me`; the project ids, hashes of the paths, are over that.

| | |
| --- | --- |
| Logic Pro | 12.3.1, `com.apple.logic10`, pid 43575, `~/Music/Logic/Scratch.logicx`: two tracks, one region of four notes, a Channel EQ, a marker |
| macOS | 26.6.1, Apple Silicon |
| Bridge | `logic-kumi` 0.1.0 (451b35ab9d0d) with the helper `kumi-logic-hands-bf604b889e95`, put in a scratch home by `kumi --daw logic bridge --yes` |
| Host | `target/debug/daw-mcp-server --daw logic`, started by the acceptance run as Kumi starts it |
| Run | `cargo run -p kumi --example accept_logic -- --kumi-home <scratch home> --no-operator --report <file>` |

## The run

| Check | Result | Time |
| --- | --- | --- |
| status: Logic Pro  (pid 43575, com.apple.logic10), bridge 0.1.0, project “Scratch”, busy false, permission null | ok | 116 ms |
| discover: 1 Logic Pro instance(s) | ok | 12 ms |
| focus over 50 calls: median 1 ms, worst 1 ms (under 150 ms); selection 1:track:1, editor none, ageMs 38 | ok | 1 ms |
| first look: “Logic Pro — Scratch”, 2 tracks, 1 markers, selection track:1, place logic-e805ab87b3ca9d66f0f62d47b97caa29 | ok | 3.4 s |
| open_set: /Users/me/Music/Logic/Scratch.logicx | ok |  |
| recording: Some(false) (from the focus's transport) | ok |  |
| read project: 1 rows (total 1, epoch "1") | ok | 52 ms |
| read track: 2 rows (total 2, epoch "1") | ok | 48 ms |
| read region: 1 rows (total 1, epoch "1") | ok | 46 ms |
| read note: 4 rows (total 4, epoch "1") | ok | 714 ms |
| read plugin: 1 rows (total 1, epoch "1") | ok | 53 ms |
| read send: 0 rows (total 0, epoch "1") | ok | 33 ms |
| read marker: 1 rows (total 1, epoch "1") | ok | 34 ms |
| read tempo: 1 rows (total 1, epoch "1") | ok | 2.5 s |
| body tree: Tree(DeviceTree { track_ref: "1:track:1", devices: [DeviceNode { ref: "1:plugin:1.1", name: "Channel EQ", class_name: None, can_have_chains: None, can_have_drum… | ok | 59 ms |
| body notes: Notes(ClipView { slot_ref: "1:region:1", name: "Keys", length: 16.0, notes: [ClipViewNote { note: ClipNote { pitch: 60.0, start: 0.0, duration: 0.25, velocity: … | ok | 762 ms |
| body timeline: Timeline(ArrangementStrip { length: 16.0, position: 0.0, playing: false, loop: None, locators: [Locator { name: "Marker 1", position: 32.0 }], beats_per_bar: So… | ok | 55 ms |
| body routing: Routing(RoutingView { track_ref: "1:track:1", routes: [Route { kind: Receive, other: "Input 1", volume_db: None, muted: false, tap: None }, Route { kind: Hardwa… | ok | 705 ms |

## Numbers from the bridge's recording run, same Logic, same project

- `focus` from the cache: median 0.12 ms, worst 0.19 ms over 50 calls; the refresh takes three or four helper
  calls, 20 to 40 ms, every 150 ms.
- Attach to the first complete epoch: 104, 115 and 148 ms over three runs; the main window to depth 8 (228
  nodes) in 90 ms.
- Mackie Control: Logic's first device query 0.3 s after the ports open, the surface installed 7 to 11 ms after
  it.
- Reads that open a pane: notes 720 ms, tempo 1.1 to 2.5 s (two list tabs), snapshot 1.3 s; the others 20 to
  60 ms with the strips cached.

## Where the recording differs from the hand-written Scratch

The key is "C Major" and the output "Stereo Output", in Logic's words; there is no send (the send slot's
pop-up menu can't be driven from outside); the marker is "Marker 1" (its name cell isn't a text field) and
its length is 32 beats where the simulator counted 8 bars; the sample rate is unknown (macOS keeps `~/Music`
from the terminal, so `MetaData.plist` can't be read); no editor is open in the idle scene; Keys sits at 0 dB.
The host's and the runtime's tests read their expectations from the recorded vectors.

## The test suite

The whole isolated suite on 245e932a (`scripts/test-isolated.sh --no-fail-fast`): 3099 passed, 3 failed, 6 ignored in 334
test binaries. Every Logic test passed, among them the bridge's reads over the four recorded scenes, the host's replay
of the vectors, the runtime's end-to-end Logic tests and the app's Logic sessions. The three failures are the same
timing-bound tests as at the M1 gate, one each in `hands_transport.rs`, `library_stop.rs` and `style_references.rs`,
in code this branch doesn't change.

## Not covered

- The four operator checks: run `accept_logic` without `--no-operator` at the Mac and follow its prompts
  (select another track; Cmd-Shift-S, then Escape; add a track; open another project).
- A "Kumi Logic" Mackie Control device stays in Logic's Control Surfaces > Setup, offline while no bridge
  runs. Logic's windows vanish from Accessibility while its Space isn't current, so a run brings Logic to
  the front first.
- Only an English Logic was run; the localized tables were checked on German strings by hand, and the loader
  on a synthetic bundle in the tests.
