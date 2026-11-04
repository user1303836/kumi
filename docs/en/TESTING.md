# Testing

English · [简体中文](../zh-CN/TESTING.md) · [日本語](../ja/TESTING.md)

How to run the tests of each part of the repository, what they need, and what
CI runs. None of the ordinary tests need Live or a sign-in.

## Quick start

From the repository root:

```sh
npm ci --prefix crates/kumi-agent/tests/support   # once: the official SDKs some Rust tests run
cargo build --workspace --all-targets --locked
sh scripts/test-isolated.sh                        # Windows: ./scripts/test-isolated.ps1
python3 -m unittest discover -s daws/ableton/remote-script -p 'test_*.py'
npm ci --prefix daws/ableton/live-extension
npm test --prefix daws/ableton/live-extension
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

On Windows, a few tests create symlinks, which needs Developer Mode or an
administrator account. Without it, some of them skip and a few fail with
`EPERM`; CI's Windows runner has the right.

## Kumi and the bridge

Run from the repository root.

| Command | What it does |
| --- | --- |
| `sh scripts/test-isolated.sh` (Windows: `./scripts/test-isolated.ps1`) | Every test with cargo-nextest, each in a process of its own, in a home of its own; arguments go to `cargo nextest run` (`KUMI_NEXTEST=0`: `cargo test`, a crate's tests in one process; `KUMI_NEXTEST_ARCHIVE`: tests built elsewhere, from a `cargo nextest archive`, unpacked into the checkout's `target/`) |
| `sh scripts/test-isolated.sh -p kumi-daw-ableton -E 'test(/^hands_transport::/)'` | One test file of one crate: a crate's tests are one binary, `tests/it/`, a module a file |
| `cargo fmt --all --check` | Formatting, as CI checks it |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | Lints; CI doesn't run them |
| `cargo clippy --workspace --lib --bins --examples --locked -- -A clippy::all -D clippy::await_holding_refcell_ref` | A `RefCell` borrow held across an await; fails `Required CI` |
| `sh scripts/test-isolated.sh --profile ci` | The tests the `Rust / Linux` parts run on `main`, with cargo-nextest (`-p <crate>` or `--exclude <crate>` for one part's; `--profile extended`: every test) |
| `python3 scripts/dev.py check` | Required CI's checks for what this branch changes since `origin/main` (`--base` for another): the change's whitespace, every file in a crate or an area, the crates' layers (`dev.py deps --check`: a dependency points down, and no DAW's crates use another's), the scripts' tests and the installers' syntax, then for the crates it affects `cargo fmt --check` and the `RefCell` lint. `--tests` then runs what `main` tests after the merge: those crates' `ci` tests (with cargo-nextest), and the Remote Script's, the Live extension's, the Renoise tool's and the Hermes plugin's tests when they changed. `--dry-run` says what it would run |
| `cargo run --locked --release -p ableton-mcp-server --bin ableton-mcp-benchmark` | The bridge's performance budgets; see [the developer guide](DEVELOPER_GUIDE.md#build-test-and-measure) |

The isolated runner gives the tests a home of their own: `HOME`, `USERPROFILE`,
`APPDATA`, `LOCALAPPDATA`, `XDG_CONFIG_HOME` and `KUMI_HOME` point into a fresh
temporary folder, and `KUMI_REMOTE_SCRIPTS_DIR` and `KUMI_LIVE_EXTENSIONS_DIR`
are dropped, so no test can reach your Live folders or `~/.kumi`.

The tests that run the official MCP and model SDKs beside Kumi's own client and
providers fail until `npm ci --prefix crates/kumi-agent/tests/support` has
run. The loudness and true-peak tests hold the analysis to FFmpeg's `ebur128`
results for generated audio.

The JSON oracle files in `testdata/oracles/<crate>/`, which that crate's tests
read with `kumi_testkit::oracle`, are golden files recorded from the
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
python3 -m unittest discover -s daws/ableton/remote-script -p 'test_*.py'
python3 -m compileall -q daws/ableton/remote-script/AbletonMcpBridge
```

The tests run the Remote Script against fake Live objects: authentication,
sequencing, the main-thread queue, the registry and its hash, discovery,
transactions, capture and realtime safety, and the optional Willington provider.

## Kumi's Live extension

`daws/ableton/live-extension` has its own `package.json`. After
`npm ci --prefix daws/ableton/live-extension`, `npm test --prefix daws/ableton/live-extension`
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
`daws/ableton/ableton-mcp-server/tests/it/lifecycle_migration.rs`. That test doesn't
skip without it: it downloads the published Kumi 1.7.5 bundle and checks its
SHA-256. With the variable set, it runs offline once the bundle is fetched.

## Checks with Live or a model

These are opt-in. They change real things or spend real tokens, so CI doesn't
run them.

| Command (from the root) | Needs | What it does |
| --- | --- | --- |
| `cargo run --release -p kumi --example accept_live -- --set "<Set>"` | Live with a disposable copy of a Set open; the bridge, built first with `cargo build --release -p ableton-mcp-server --bins` (for a debug run, the same without `--release`) | Makes every kind of change Kumi can, undoes each with Kumi's undo, plays, bounces, listens and watches, and times reads of a big Set. No model. |
| `cargo run --release -p kumi --example time_rebuild` | Live with a Set open; the bridge, built first as for `accept_live` | Times what a tutorial's rebuild costs in Live, without a model: a MIDI track with Drift and four effects in one plan, each device's parameters read, 30 of them set by name, and Kumi's looks at the Set, each with the bridge requests it took. It adds one track and deletes it at the end. |
| `cargo run --release -p kumi --example eval_changes [-- <part of a case name>]` | Your sign-in and model | How the model uses Kumi's tools, against a synthetic bridge with the real bridge's tool schemas, read from its native catalog. Never touches Live. Each case says its time, its tools' share of it, and how many model calls it took; `EVAL_EFFORT` sets the model's reasoning effort, and `EVAL_TRACE=1` prints each call. `EVAL_TUTORIAL=1` adds a real 16-minute tutorial from YouTube, which needs the network. `EVAL_MEASURE=1`, with no sign-in or model, prints what every request carries: the instructions and each tool's definition in bytes (`EVAL_MEASURE=tools` adds the definitions). |
| `cargo run --release -p kumi --example probe_inference` | Your sign-in | One authenticated request with a harmless tool. Never touches Live. |
| `cargo run --release -p kumi --example probe_cache` | Your sign-in and model | Whether a conversation keeps its provider's prompt cache when its kernel is rebuilt, as on a resume: short turns that print their input and cached tokens, rebuilt with the same conversation, another one and none. Never touches Live. |

The synthetic bridge's Operator, Saturator and EQ Eight have every parameter
Live 12.4 gives them, read from Live into
`crates/kumi/examples/fixtures/eval_changes/live-devices.json`, and it runs
Kumi's own scripts for setting parameters as Live does.

## Docs

After editing docs, run the packaging tests:

```sh
python3 -m unittest discover -s scripts/tests -p test_native_release.py
```

They check every link in the guides the bridge ships with: its README and the
fourteen `docs/en` pages listed in `DOCUMENTS` in
`scripts/build-native-release.py`. Nothing checks that the three languages
agree; keep them in step by hand. The release script's tests
(`test_release.py`) check that REAPER's extension and Renoise's tool each have
one ships-with line in every language's docs (in `KUMI_GUIDE.md`), naming the
versions this checkout carries.

## CI

CI is the bare minimum ([AGENTS.md](../../AGENTS.md), "Minimal CI"). A pull request runs one quick job on Linux,
`Required CI`, which runs no tests. The tests run on `main` after each merge, on Linux, and a release waits until they
pass. macOS and Windows build only in the Installer's nightly and release runs; Extended and Network run only by hand.

| Workflow | Jobs | What runs |
| --- | --- | --- |
| **CI** | `Required CI` (Ubuntu) | On every pull request, and on `main` after each merge. The change's whitespace (`git diff --check`); every tracked file in a crate or an area, the crates' layers and `dev.py`'s tests; the packaging tests, the release script's tests and the models catalog; `scripts/check-installers.sh` (install.sh is POSIX sh; install.ps1 parses and is ASCII). Then, for a change that builds Rust (not docs, the Renoise tool's Lua or the Hermes plugin alone, as `scripts/dev.py` reads it from the crates and `scripts/areas.toml`), `cargo fmt --check` and the build, with the one Clippy check that fails it: a `RefCell` borrow held across an await. No tests |
| | `Rust / Linux`, `Rust / Linux (Kumi's crates)`, `Rust / Linux (bridges)` | On `main` after each merge and by hand, never on a pull request: the `ci` set of `.config/nextest.toml` through the isolated runner, for every crate, in three parts at once by crate, of about the same test time (the app; Kumi's other crates; `ableton-mcp-server`, `daw-bridge`, `daw-mcp-server`, `reaper-kumi` and `soundcraft-kumi`), so no runner builds every test binary: every test but the long ones the Extended workflow runs (REAPER's and Renoise's whole-flow simulations among them), which the config lists with why. A test that runs past 30 s fails, by name. Each Rust job, Extended's too, keeps the list of the tests it selected as an artifact (`nextest-list-*`), to compare one run's tests with another's |
| | `Remote Script, Live extension, Renoise tool` (Ubuntu) | On `main` after each merge and by hand: the Remote Script's tests on Live's Python (3.11); the Live extension's tests against its committed build (Node 24); the Renoise tool's Lua tests on a pinned LuaJIT against a mock of Renoise's API, and `lua-language-server --check` against Renoise's API 6.2 definitions |
| | `Hermes plugin` (Ubuntu) | On `main` after each merge and by hand: the plugin's tests on Python 3.11 and on Hermes's own, the Hermes catalog's checks with Hermes v0.22.7 (`plugins validate --install-deps`, `plugins doctor --ci`, the payload scan, the self-updater check), and the Desktop and dashboard halves' tests |
| | `Main's CI failed` | On `main` only, when a job failed: opens an issue with that title (or comments on the open one), naming the commit and the run |
| **Extended** | `Extended / Linux / build`, `(1/2)`, `(2/2)`; `Extended / macOS / build`, `(1/2)`, `(2/2)`; `Extended / Windows / build`, `(1/3)` to `(3/3)`; `Extended / eval runner tests` | Each system's tests built once and archived (`cargo nextest archive`), then run in shares from the archive, which start when their own system's build is done (`extended-system.yml`). Every test (the `extended` set) with the official SDKs installed: what `ci` leaves out (the recorded-source oracles, the long listening, form and rendering simulations, video with ffmpeg, the interoperability tests beside the official MCP and model SDKs, the bridge's benchmark harness, the migration from Kumi 1.7.5, the long sweeps of the timing building blocks) and each platform's every test. Beside them, the eval runner's own tests (`cargo test -p kumi --example eval_daws`), which no test binary holds. It runs only by hand (`gh workflow run Extended --ref <branch>`), and blocks nothing |
| **Network** | `Network / Linux`, `macOS`, `Windows` | Kumi's HTTP clients with a certificate authority in each system's own trust store, and through the proxy in its own settings (`network.rs`'s ignored tests). Only by hand (`gh workflow run Network --ref <branch>`), for a change to how the clients are made |
| **Willington files** | `Willington files` (Ubuntu) | Only the repository owner's pull requests, from a `willington/` branch in this repository, change `vendor/willington/`, and such a pull request changes nothing else. `main`'s copy of the check runs, reading the pull request's file list, never its code |
| **CodeQL** | `Analyze (actions)`, `Analyze (c-cpp)`, `Analyze (javascript-typescript)`, `Analyze (python)` | Code scanning, with CodeQL's default queries, of the Live extension's TypeScript and JavaScript, the Remote Script's and the scripts' Python, REAPER's C++ shim and SDK headers, and the workflows. Only for a pull request, or a push to `main`, that changes one of those, and each Monday over all of them. It blocks nothing |
| **Installer** | `Native bundle / <target>`, `Aggregate native and existing-installer releases`, then `Install / <system>` and `Existing installer transition / <system>`; `Installer edges / <system>` (three) needs no bundle | Never on a pull request or a merge. Each night on `main`, it builds all six bundles below and, on one machine of each system (Linux and Windows on x64, macOS on Apple silicon), installs, checks and uninstalls as below. Tags and a run by hand do all of it: build a native bundle for Intel and ARM on macOS, Linux and Windows (each Mac bundle with the helper Kumi uses Live's menus with, universal and ad hoc signed; the Intel one cross-built on Apple Silicon), then the compatibility release that existing installations update from, and serves them locally with the installers, as GitHub serves a release's files. On each system: installs with the one-liner from that copy, as producers do (Windows PowerShell 5.1 on Windows, with `KUMI_HOME` in a folder named in non-ASCII letters), checks the version, `doctor`, the bridge and its analysis worker, installs again as a repair, runs `kumi bridge --yes` into a scratch Remote Scripts folder, puts REAPER's extension and Renoise's tool into scratch DAW folders that each hold a file of the producer's (`kumi --daw reaper\|renoise bridge --yes`), checks each is the bundle's copy with its reference, receipt and kept generation, loads the extension with REAPER's entry point and starts Kumi's host for them, then runs `kumi update` (both bridges as they were), `kumi update --rollback` twice (both to an earlier Kumi's copies and back) and `kumi uninstall` (both out, a producer's copy of the tool kept, the producer's own files left). One install on each system (macOS arm64, Linux x64, Windows x64) then, in a home with spaces and non-ASCII letters, installs GitHub's latest release with this build's installer and then, with `KUMI_VERSION`, the release before this build; puts that release's bridge in; updates to this build and rolls back twice, checking the Kumi and the bridge each time (with REAPER's and Renoise's bridges in, which that release doesn't know and leaves be); and uninstalls. The PATH line goes in zsh's `ZDOTDIR` on macOS and fish's `conf.d` on Linux, where that shell finds `kumi`, and the uninstall takes it out; Windows runs this in PowerShell 7: this build's install, the bridge, the rollbacks and the uninstall with `KUMI_HOME` in a folder named in non-ASCII letters, and the update from the release before in a home with spaces only (that release's own updater unpacks it). The transition jobs run the migration tests with Kumi 1.7.5 and the new bundles. The edges jobs run install.sh and install.ps1 against stand-in releases: a missing release, no answer, a sign-in page, a bad checksum, a Kumi that doesn't start, Ctrl-C. On a `v*` tag's push, once all these pass, `publish` attaches the bundle and the installers to the release, and publishes it when `scripts/release.py` wrote its notes. |

On a pull request:

- Only `Required CI` runs: no tests, and nothing on macOS or Windows. Run
  the tests your change touches yourself: `python3 scripts/dev.py check
  --tests` runs the ones `main` would run for it.
- Rust's build caches are saved from `main` only: a pull request restores
  `main`'s and saves none.
- `Required CI` skips Rust for a change that builds none (`python3
  scripts/dev.py affected --base origin/main` says whether a branch does), and
  always checks that every tracked file belongs to a crate or an area.

Each push to `main` runs `Required CI` and every Linux test, to the end (a
newer merge doesn't cancel it); when a job fails, it opens an issue ("Main's CI
failed"). `release.py --go` refuses until `main`'s CI on the commit it releases
is green. Each tag runs all of the Installer. Each night the Installer builds
all six bundles with the `release` profile, which keeps that profile's build
cache warm for the next tag, and installs and checks them on one machine of
each system; when that fails, it opens an issue ("The nightly Installer build
failed"). Each day the Caches workflow
deletes every Rust build cache but the newest of its kind, so older lockfile
generations don't push the caches pull requests restore out of the
repository's 10 GB.

To merge into `main`, `Required CI` and `Willington files` must pass. The
Installer isn't required.
[Releases and distribution](DISTRIBUTION_POLICY.md#merge-gate) has the rest of
the rules.

## What passing means

Passing shows the code behaves as its tests say, the packages install and run
on macOS, Linux and Windows, and the installer works on GitHub's runners. It
doesn't show that the Remote Script loads in your Live, that Live's API has the
shape the fakes have, how anything sounds, or that a terminal or screen reader
works with Kumi. The opt-in checks above, and the records in
[implementation status](IMPLEMENTATION_STATUS.md#evidence), cover real Live.

## Writing tests

For every new protocol method or change to Live, add a test that it works and
tests that it refuses what it should: stale references, revisions and epochs,
expired confirmations, a reused idempotency key, timeouts, cancellation before
and after it's sent, disconnects, a lost acknowledgement, partial changes,
failed compensation, a change made in Live meanwhile, and undo. Keep fake Live,
the simulator and real Live apart in what a test claims (`fake-live`,
`simulator` and `real-live` provenance). Keep fixtures small and free of
private data, and never let a test reach real Live folders or `~/.kumi`.

A new test waits on a state, never on a clock, and asserts no duration;
benchmarks stay out of the suite, and older tests that still assert a duration
are debt. A test that fails on a change it has nothing to do with is fixed if
the fix is small and certain, or gets an issue (`flaky test`); it's never
deleted to hide a real break, but one that costs more time than the breaks it
catches can go ([AGENTS.md](../../AGENTS.md), "Minimal CI").
