# Testing

English · [简体中文](../zh-CN/TESTING.md) · [日本語](../ja/TESTING.md)

How to run the tests of each part of the repository, what they need, and what
CI runs. None of the ordinary tests need Live, Logic Pro or a sign-in.

## Quick start

From the repository root:

```sh
npm ci --prefix crates/kumi-runtime/tests/support   # once: the official SDKs some Rust tests run
cargo build --workspace --all-targets --locked
sh scripts/test-isolated.sh                        # Windows: ./scripts/test-isolated.ps1
python3 -m unittest discover -s remote-script -p 'test_*.py'
npm ci --prefix apps/live-extension
npm test --prefix apps/live-extension
python3 -m unittest discover -s scripts/tests -p 'test_*release.py'
```

What they need:

| Needs | For |
| --- | --- |
| Rust and Cargo | The crates and the migration tests |
| cargo-nextest (`cargo install cargo-nextest --locked`) | The Rust tests, each in a process of its own |
| Node.js on PATH (CI uses 24) | Some Rust tests, the Live extension's tests and the migration tests |
| Python 3.11 or later on PATH (`python3`, or `python.exe` on Windows) | The Remote Script tests, the release scripts' tests, and Rust tests that run Python |
| A locally supplied Extensions SDK in `vendor/` | Building or type-checking the Live extension (its tests don't need it) |
| macOS with Swift (Xcode's command line tools) | Building Logic Pro's helper with `scripts/build-logic-hands.py`, which the Logic bridge's tests use |

On Windows, a few tests create symlinks, which needs Developer Mode or an
administrator account. Without it, some of them skip and a few fail with
`EPERM`; CI's Windows runner has the right.

## Kumi and the bridge

Run from the repository root.

| Command | What it does |
| --- | --- |
| `sh scripts/test-isolated.sh` (Windows: `./scripts/test-isolated.ps1`) | Every test with cargo-nextest, each in a process of its own, in a home of its own; arguments go to `cargo nextest run` (`KUMI_NEXTEST=0`: `cargo test`, a crate's tests in one process) |
| `sh scripts/test-isolated.sh -p kumi-runtime -E 'test(/^hands_transport::/)'` | One test file of one crate: a crate's tests are one binary, `tests/it/`, a module a file |
| `KUMI_PACKAGES="daw-bridge daw-mcp-server logic-kumi" sh scripts/test-isolated.sh` | Only those packages, every test of theirs: here the Logic Pro bridge's crates, as CI's `Logic bridge` job runs them on macOS after `python3 scripts/build-logic-hands.py` |
| `cargo fmt --all --check` | Formatting, as CI checks it |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | Lints; advisory in CI for now |
| `cargo clippy --workspace --lib --bins --examples --locked -- -A clippy::all -D clippy::await_holding_refcell_ref` | A `RefCell` borrow held across an await; fails the Lint CI job |
| `sh scripts/test-isolated.sh --profile ci` | The tests the `Rust / Linux` parts run, with cargo-nextest (`-p <crate>` or `--exclude <crate>` for one part's; `--profile ci-os`: what macOS and Windows run; `--profile extended`: every test) |
| `python3 scripts/dev.py check` | Required CI's checks for what this branch changes since `origin/main` (`--base` for another): the change's whitespace, every file in a crate or an area, then for the crates it affects `cargo fmt --check`, the `RefCell` lint and their `ci` tests (with cargo-nextest), then the Remote Script's, the Live extension's and the Renoise tool's tests when they changed, and the release scripts' tests. `--dry-run` says what it would run |
| `cargo run --locked --release -p ableton-mcp-server --bin ableton-mcp-benchmark` | The bridge's performance budgets; see [the developer guide](DEVELOPER_GUIDE.md#build-test-and-measure) |

The isolated runner gives the tests a home of their own: `HOME`, `USERPROFILE`,
`APPDATA`, `LOCALAPPDATA`, `XDG_CONFIG_HOME` and `KUMI_HOME` point into a fresh
temporary folder, and `KUMI_REMOTE_SCRIPTS_DIR` and `KUMI_LIVE_EXTENSIONS_DIR`
are dropped, so no test can reach your Live folders or `~/.kumi`.

The tests that run the official MCP and model SDKs beside Kumi's own client and
providers fail until `npm ci --prefix crates/kumi-runtime/tests/support` has
run. The loudness and true-peak tests hold the analysis to FFmpeg's `ebur128`
results for generated audio.

The JSON oracle files in the crates' tests are golden files recorded from the
TypeScript implementation; it and the scripts that generated them stay at the
git tag `v1.7.6`, and nothing in this tree regenerates them. After an
intentional change in behavior, rewrite the affected oracle entries from the
native output: the failing assertion prints what it got, and for a SHA-256
entry, hash that output. Review the diff, and say in the commit message that
the golden files changed and why. The host tests pin the bridge version the
golden files were recorded with (`ORACLE_VERSION`), so a bridge version bump
changes no golden file.

## The Remote Script

From the repository root:

```sh
python3 -m unittest discover -s remote-script -p 'test_*.py'
python3 -m compileall -q remote-script/AbletonMcpBridge
```

The tests run the Remote Script against fake Live objects: authentication,
sequencing, the main-thread queue, the registry and its hash, discovery,
transactions, capture and realtime safety, and the optional Willington provider.

## Kumi's Live extension

`apps/live-extension` has its own `package.json`. After
`npm ci --prefix apps/live-extension`, `npm test --prefix apps/live-extension`
loads the committed `dist/extension.js` against a fake Live and checks it
against its recorded sha256. Building it (`npm run build`) and type-checking it
(`npm run typecheck`) there need the Extensions SDK in `vendor/`; without it,
the committed build stays as it is. After a rebuild, commit `dist/extension.js`
with its `.sha256`.

## The release scripts

`python3 -m unittest discover -s scripts/tests -p 'test_*release.py'` runs
the packaging and migration tests. The packaging tests check the bundle's
contents and versions, and that every link in the packaged guides resolves.
The migration tests run the last JavaScript release's updater against native
bundles. Without these variables, the tests that need them skip:

| Variable | What it names |
| --- | --- |
| `KUMI_LEGACY_APP` | An unpacked Kumi 1.7.5: `python3 scripts/fetch-legacy-release.py [folder]` downloads it, checks its SHA-256, unpacks it and prints the folder |
| `KUMI_NATIVE_RELEASES` | A folder of built release artifacts (see [releasing](DEVELOPER_GUIDE.md#releasing)) |

`KUMI_LEGACY_APP` serves the bridge's own migration test too,
`crates/ableton-mcp-server/tests/it/lifecycle_migration.rs`. That test doesn't
skip without it: it downloads the published Kumi 1.7.5 bundle and checks its
SHA-256. With the variable set, it runs offline once the bundle is fetched.

## Checks with Live or a model

These are opt-in. They change real things or spend real tokens, so CI doesn't
run them.

| Command (from the root) | Needs | What it does |
| --- | --- | --- |
| `cargo run --release -p kumi --example accept_live -- --set "<Set>"` | Live with a disposable copy of a Set open; the bridge, built first with `cargo build --release -p ableton-mcp-server --bins` (for a debug run, the same without `--release`) | Makes every kind of change Kumi can, undoes each with Kumi's undo, plays, bounces, listens and watches, and times reads of a big Set. No model. |
| `cargo run --release -p kumi --example accept_logic -- [--files <folder>] [--report <file>] [--no-operator]` | Logic Pro 12 open on a project you can spare; Kumi's Logic Pro bridge set up (`kumi --daw logic bridge`; `--kumi-home <folder>` names a home of its own) with Accessibility allowed; Kumi's host, built first with `cargo build --release -p daw-mcp-server --bins` | The run each Logic Pro gate made, on the project Logic Pro has open: the status and discovery, FOCUS's speed, every read and body; a plan of changes, each undone through Kumi; a save as to a copy under `--files` (from then on the run works on the copy, so the project you opened stays as it was), key commands, regions, an import, the cycle and the Tempo List; a plug-in, a send, a patch (`--patch` names it) and a bounce; listening (a hear, a judged round, an audition, a render); the fixed phrases, the metronome, a sample into a Sampler track, the recent projects, middle C, the bulk read, `watch_me` and Channel EQ's and the Compressor's curves; and the project read as before at the end. Without `--files`, what writes a file is skipped. With you at Logic Pro, it also checks the selection followed, a dialog counted as busy, a project switched and a track moved, which `--no-operator` leaves out. `--report` writes its Markdown summary for `docs/evidence/`. No model. |
| `cargo run --release -p kumi --example time_rebuild` | Live with a Set open; the bridge, built first as for `accept_live` | Times what a tutorial's rebuild costs in Live, without a model: a MIDI track with Drift and four effects in one plan, each device's parameters read, 30 of them set by name, and Kumi's looks at the Set, each with the bridge requests it took. It adds one track and deletes it at the end. |
| `cargo run --release -p kumi --example eval_changes [-- <part of a case name>]` | Your sign-in and model | How the model uses Kumi's tools, against a synthetic bridge with the real bridge's tool schemas, read from its native catalog. Never touches Live. Each case says its time, its tools' share of it, and how many model calls it took; `EVAL_EFFORT` sets the model's reasoning effort, and `EVAL_TRACE=1` prints each call. `EVAL_TUTORIAL=1` adds a real 16-minute tutorial from YouTube, which needs the network. `EVAL_MEASURE=1`, with no sign-in or model, prints what every request carries: the instructions and each tool's definition in bytes (`EVAL_MEASURE=tools` adds the definitions). |
| `cargo run --release -p kumi --example eval_logic [-- <part of a case name>]` | Your sign-in and model | How the model knows Logic Pro and works in it, over the simulated Logic: Kumi's host in process, in front of the Scratch project the Logic bridge's tests recorded, with Logic's instruction pack, changes and listening. Never touches Logic Pro. Its 23 cases are knowledge probes of Logic's terms and ways (Apple's guide asked and cited where a question is about one of Logic's features), change requests over Scratch, the limits Kumi's bridge can't pass (refused saying why) and a listen; each case's check is code. `--list` prints each case and what its check looks for, and `--dry` runs every check on a canned answer per case, with no sign-in or model. `EVAL_EFFORT`, `EVAL_TRACE=1` (which also prints each answer whole) and `EVAL_TIMINGS` work as for `eval_changes`. |
| `cargo run --release -p kumi --example probe_inference` | Your sign-in | One authenticated request with a harmless tool. Never touches Live. |
| `cargo run --release -p kumi --example probe_cache` | Your sign-in and model | Whether a conversation keeps its provider's prompt cache when its kernel is rebuilt, as on a resume: short turns that print their input and cached tokens, rebuilt with the same conversation, another one and none. Never touches Live. |

The synthetic bridge's Operator, Saturator and EQ Eight have every parameter
Live 12.4 gives them, read from Live into
`crates/kumi/examples/fixtures/eval_changes/live-devices.json`, and it runs
Kumi's own scripts for setting parameters as Live does.

The Logic eval reads Apple's Logic Pro User Guide from a fixture of Logic's
topics, written for the eval in the shape of Apple's guide
(`crates/kumi/examples/fixtures/eval_logic/logic-guide.json`), which it lays
out as Apple's site lays the guide out, so Kumi's own parser reads it: nothing
comes from the network. Each case is a session of its own, with a throwaway
folder for Kumi's home, memory, recipes and library, and opens no project in
Logic Pro. CI builds it and runs its dry run
(`crates/kumi/tests/eval_logic_dry.rs`), which also checks that each kind of
check fails an answer wrong in its way and takes other good answers. A run
with the model is what costs: the canned answers take 35 model requests, and
a model takes more (it reads before it changes and asks `find_tools` for the
guide), about 50 to 70 for the 23 cases, each carrying about 47 KB of
instructions and tool definitions besides the conversation (cache reads after
the first, with a provider that caches). A case's name, or part of one,
narrows a run to those cases. Its results are recorded by hand under
`docs/evidence/`, as the Live benchmark's were: the date, the model and its
effort, each case's pass or FAIL with what it found wanting, and the totals the
eval prints.

## Docs

After editing docs, run the packaging tests and the release script's tests:

```sh
python3 -m unittest discover -s scripts/tests -p test_native_release.py
python3 -m unittest discover -s scripts/tests -p test_release.py
```

The first check every link in the guides the bridge ships with: its README and
the fourteen `docs/en` pages listed in `DOCUMENTS` in
`scripts/build-native-release.py`. The second check that the lines a release
edits are there once each: the READMEs' status lines, the ships-with lines
under "Bridge versions" (Logic Pro's in every language or in none, at its
bridge's version), and REAPER's extension's and Renoise's tool's, one in every
language's docs (in `KUMI_GUIDE.md`), naming the versions this checkout
carries. Nothing else checks that the three languages agree; keep them in step
by hand.

## CI

These workflows run on every pull request, and the CI, Willington files, Installer and Network workflows on every push to `main` too:

| Workflow | Jobs | What runs |
| --- | --- | --- |
| **CI** | `Rust / Linux`, `Rust / Linux (Live's bridge)`, `Rust / Linux (DAW bridges)` | The `ci` set of `.config/nextest.toml` through the isolated runner, in three parts at once by crate (Kumi and its runtime; `ableton-mcp-server`; `daw-bridge`, `daw-mcp-server` and `reaper-kumi`), so no runner builds every test binary: every test but the long ones the Extended workflow runs (REAPER's and Renoise's whole-flow simulations among them), which the config lists with why. On a pull request, each part tests only its crates the change affects (`scripts/dev.py`, as `dev.py check` picks them), and a part with none skips. A test that runs past 30 s fails, by name. Then `git diff --check`. Each Rust job, Extended's too, keeps the list of the tests it selected as an artifact (`nextest-list-*`), to compare one run's tests with another's |
| | `Rust / macOS`, `Rust / Windows` | The `ci-os` set: the tests that differ by platform (installing, launching and updating; the terminal, stdin and the Windows console; Hands; paths, files and child processes; the store), and only those built |
| | `Rust / Lint` (Ubuntu) | `cargo fmt --check`, the examples' build and Clippy (advisory, except a `RefCell` borrow held across an await, which fails the job) |
| | `Python Remote Script / ubuntu-24.04`, `macos-15`, `windows-2025` (Python 3.11) | The Remote Script's tests; compiles the package |
| | `Live extension` (Ubuntu, Node 24) | The extension's tests, against its committed build |
| | `Release scripts` (Ubuntu) | The whitespace check of the change, then the packaging tests, the release script's tests and `scripts/check-installers.sh` (install.sh is POSIX sh; install.ps1 parses and is ASCII). Pushes to `main` also check `main`'s raw installers and the latest release's |
| | `REAPER extension loads / macos-15`, `ubuntu-22.04`, `windows-2025` | REAPER's extension as it ships: the release build and the platform's own strip, the release script's check of what a bundle may carry (REAPER's entry point; signed and named for itself on macOS; the C runtime linked in on Windows; no newer glibc than Kumi's on Linux), loaded by the system's loader, and REAPER's calls through the C++ shim |
| | `Renoise tool` (Ubuntu) | The tool's Lua tests on a pinned LuaJIT, against a mock of Renoise's API, and `lua-language-server --check` against Renoise's API 6.2 definitions |
| | `Logic bridge` (macOS 15) | Logic Pro's helper built by its script, then the tests of `daw-bridge`, `daw-mcp-server` and `logic-kumi` through the isolated runner: the bridge over scenes recorded from the real Logic Pro, the host over the simulated Logic Pro. No Logic Pro |
| | `Required CI` | Passes only when all of the above passed |
| **Extended** | `Extended / Linux (1/3)` to `(3/3)`, `Extended / macOS (1/2)`, `(2/2)`, `Extended / Windows (1/3)` to `(3/3)`, `Extended / documentation tests` | Every test (the `extended` set) with the official SDKs installed: what `ci` leaves out (the recorded-source oracles, the long listening, form and rendering simulations, video with ffmpeg, the benchmarks, the interoperability tests beside the official MCP and model SDKs, the migration from Kumi 1.7.5, the long sweeps of the timing building blocks) and each platform's every test; then the documentation tests. It runs each week, by hand, and on a pull request labelled `full-tests`, and blocks nothing |
| | `Extended / REAPER SDK, newest header` | REAPER's extension built against the newest SDK headers Cockos has published, its generated function table checked, and the functions a move to them would bring listed |
| **Network** | `Network / What changed`, then `Network / Linux`, `macOS`, `Windows` | Kumi's HTTP clients with a certificate authority in each system's own trust store, and through the proxy in its own settings (`network.rs`'s ignored tests). The systems' jobs run only for a change to how the clients are made (the `network` area in `scripts/areas.toml`) |
| **Willington files** | `Willington files` (Ubuntu) | Only the repository owner's pull requests, from a `willington/` branch in this repository, change `vendor/willington/`, and such a pull request changes nothing else. `main`'s copy of the check runs, reading the pull request's file list, never its code |
| **Installer** | `Native bundle / <target>`, `Aggregate native and existing-installer releases`, then `Install / <system>` and `Existing installer transition / <system>`; `Installer edges / <system>` (three) needs no bundle | Each push to `main`, and a pull request that changes installing, builds a bundle for Linux and Windows on x64 and macOS on Apple silicon and, on each, installs, checks and uninstalls as below. Tags, a run by hand and a pull request labelled `full-tests` do all of it: build a native bundle for Intel and ARM on macOS, Linux and Windows (each Mac bundle with the helper Kumi uses Live's menus with, universal and ad hoc signed; the Intel one cross-built on Apple Silicon), then the compatibility release that existing installations update from, and serves them locally with the installers, as GitHub serves a release's files. The Apple Silicon Mac's bundle carries Logic Pro's bridge and its helper (built by `scripts/build-logic-hands.py`), each checked as Apple's own tools see them; no other bundle carries it. On each system: installs with the one-liner from that copy, as producers do (Windows PowerShell 5.1 on Windows, with `KUMI_HOME` in a folder named in non-ASCII letters), checks the version, `doctor`, the bridge and its analysis worker, installs again as a repair, runs `kumi bridge --yes` into a scratch Remote Scripts folder, puts REAPER's extension and Renoise's tool into scratch DAW folders that each hold a file of the producer's (`kumi --daw reaper|renoise bridge --yes`) and, on the Apple Silicon Mac, Logic Pro's bridge into Kumi's own folder (elsewhere, checks that `kumi --daw logic bridge` sets nothing up and says Logic Pro is supported on Apple Silicon Macs only), checks each is the bundle's copy with its reference, receipt and kept generation, loads the extension with REAPER's entry point and starts Kumi's host for them, then runs `kumi update` (each bridge as it was), `kumi update --rollback` twice (each to an earlier Kumi's copy and back) and `kumi uninstall` (each out, a producer's copy of the tool kept, the producer's own files left). One install on each system (macOS arm64, Linux x64, Windows x64) then, in a home with spaces and non-ASCII letters, installs GitHub's latest release with this build's installer and then, with `KUMI_VERSION`, the release before this build; puts that release's bridge in; updates to this build and rolls back twice, checking the Kumi and the bridge each time (with REAPER's and Renoise's bridges in, and on the Apple Silicon Mac Logic Pro's, which that release doesn't know and leaves be); and uninstalls. The PATH line goes in zsh's `ZDOTDIR` on macOS and fish's `conf.d` on Linux, where that shell finds `kumi`, and the uninstall takes it out; Windows runs this in PowerShell 7: this build's install, the bridge, the rollbacks and the uninstall with `KUMI_HOME` in a folder named in non-ASCII letters, and the update from the release before in a home with spaces only (that release's own updater unpacks it). The transition jobs run the migration tests with Kumi 1.7.5 and the new bundles. The edges jobs run install.sh and install.ps1 against stand-in releases: a missing release, no answer, a sign-in page, a bad checksum, a Kumi that doesn't start, Ctrl-C. On a `v*` tag's push, once all these pass, `publish` attaches the bundle and the installers to the release, and publishes it when `scripts/release.py` wrote its notes. |

On a pull request, less runs:

- `Python Remote Script` skips macOS.
- Rust's build caches are saved from `main` only: a pull request restores
  `main`'s and saves none.
- Each world's jobs run for what they test, as `scripts/dev.py` reads it from
  the crates and `scripts/areas.toml` (`python3 scripts/dev.py affected --base
  origin/main` says what a branch runs): Rust's skip a change to the Renoise
  tool's Lua alone, `REAPER extension loads` runs for the extension's crate,
  what it builds with (`daw-bridge`, the lockfile) and the release script, and
  `Renoise tool` for `daws/renoise`. `Release scripts` checks that every tracked
  file belongs to a crate or an area.
- The Installer builds and installs on one machine of each system only for a
  change to the install surface: the installers, installing, updating and
  releasing, Willington's files, the Mac helper, the `tar` and PowerShell Kumi
  runs, how it downloads (its HTTP client and network settings) and builds
  paths, how Kumi's extension goes into Live, how REAPER's, Renoise's and Logic
  Pro's bridges are built and go in, and the Installer's workflow; not their
  tests. Any other pull request builds nothing, and `main`'s run checks it once
  merged. All six, the updates from GitHub's releases, the edges and the
  transition run with the `full-tests` label.
- The Installer's bundles, on pull requests and pushes to `main`, build with
  the `ci-release` profile, which skips the release profile's whole-program
  optimization and optimizes less. Tags build with `release`.

Each push to `main` runs what a pull request that changes installing runs, and
each tag all of the Installer. The Extended workflow runs every test each week.
Each night the Installer builds all six bundles with the `release` profile,
which keeps that profile's build cache warm for the next tag; when that fails,
it opens an issue ("The nightly Installer build failed"). Each day the Caches
workflow deletes every Rust build cache but the newest of its kind, so older
lockfile generations don't push the caches pull requests restore out of the
repository's 10 GB.

To merge into `main`, `Required CI` and `Willington files` must pass. The Installer isn't required.
[Releases and distribution](DISTRIBUTION_POLICY.md#merge-gate) has the rest of
the rules.

## What passing means

Passing shows the code behaves as its tests say, the packages install and run
on macOS, Linux and Windows, and the installer works on GitHub's runners. It
doesn't show that the Remote Script loads in your Live, that Live's API has the
shape the fakes have, that your Logic Pro shows what the recorded scenes show,
how anything sounds, or that a terminal or screen reader works with Kumi. The
opt-in checks above, and the records in
[implementation status](IMPLEMENTATION_STATUS.md#evidence), cover real Live and
the real Logic Pro.

## Writing tests

For every new protocol method or change to Live, add a test that it works and
tests that it refuses what it should: stale references, revisions and epochs,
expired confirmations, a reused idempotency key, timeouts, cancellation before
and after it's sent, disconnects, a lost acknowledgement, partial changes,
failed compensation, a change made in Live meanwhile, and undo. Keep fake Live,
the simulator and real Live apart in what a test claims (`fake-live`,
`simulator` and `real-live` provenance). Keep fixtures small and free of
private data, and never let a test reach real Live folders or `~/.kumi`.
