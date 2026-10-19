# Testing

English · [简体中文](../zh-CN/TESTING.md) · [日本語](../ja/TESTING.md)

How to run the tests of each part of the repository, what they need, and what
CI runs. None of the ordinary tests need Live or a sign-in.

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
| `sh scripts/test-isolated.sh` (Windows: `./scripts/test-isolated.ps1`) | `cargo test --workspace --locked` in a home of its own; arguments go to `cargo test` |
| `sh scripts/test-isolated.sh -p kumi-runtime --test hands_transport` | One test file of one crate |
| `cargo fmt --all --check` | Formatting, as CI checks it |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | Lints, as CI checks them (a `RefCell` borrow held across an await among them) |
| `KUMI_NEXTEST=1 sh scripts/test-isolated.sh --profile ci` | The tests `Rust / Linux` runs, with cargo-nextest (`--profile ci-os`: what macOS and Windows run; `--profile extended`: every test) |
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
`crates/ableton-mcp-server/tests/lifecycle_migration.rs`. That test doesn't
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
agree; keep them in step by hand.

## CI

These workflows run on every pull request, and the CI, Willington files and Installer workflows on every push to `main` too:

| Workflow | Jobs | What runs |
| --- | --- | --- |
| **CI** | `Rust / Linux` | The `ci` set of `.config/nextest.toml` through the isolated runner, built once: every test but the long ones the Extended workflow runs, which the config lists with why. A test that runs past 30 s fails, by name. Then `git diff --check` |
| | `Rust / macOS`, `Rust / Windows` | The `ci-os` set: the tests that differ by platform (installing, launching and updating; the terminal, stdin and the Windows console; Hands; paths, files and child processes; the store), and only those built |
| | `Rust / Lint` (Ubuntu) | `cargo fmt --check` and Clippy over every target with `-D warnings`. The lints the code goes against on purpose are allowed once, in `Cargo.toml`, each with its reason |
| | `Python Remote Script / ubuntu-24.04`, `macos-15`, `windows-2025` (Python 3.11) | The Remote Script's tests; compiles the package |
| | `Live extension` (Ubuntu, Node 24) | The extension's tests, against its committed build |
| | `Release scripts` (Ubuntu) | The whitespace check of the change, then the packaging tests, the release script's tests and `scripts/check-installers.sh` (install.sh is POSIX sh; install.ps1 parses and is ASCII). Pushes to `main` also check `main`'s raw installers and the latest release's |
| | `Required CI` | Passes only when all of the above passed |
| **Extended** | `Extended / Linux (1/3)` to `(3/3)`, `Extended / macOS (1/2)`, `(2/2)`, `Extended / Windows (1/3)` to `(3/3)`, `Extended / documentation tests` | Every test (the `extended` set) with the official SDKs installed: what `ci` leaves out (the recorded-source oracles, the long listening, form and rendering simulations, video with ffmpeg, the benchmarks, the interoperability tests beside the official MCP and model SDKs, the migration from Kumi 1.7.5, the long sweeps of the timing building blocks) and each platform's every test; then the documentation tests. It runs each week, by hand, and on a pull request labelled `full-tests`, and blocks nothing |
| **Willington files** | `Willington files` (Ubuntu) | Only the repository owner's pull requests, from a `willington/` branch in this repository, change `vendor/willington/`, and such a pull request changes nothing else. `main`'s copy of the check runs, reading the pull request's file list, never its code |
| **Installer** | `Native bundle / <target>`, `Aggregate native and existing-installer releases`, then `Install / <system>` and `Existing installer transition / <system>`; `Installer edges / <system>` (three) needs no bundle | Each push to `main`, and a pull request that changes installing, builds a bundle for Linux and Windows on x64 and macOS on Apple silicon and, on each, installs, checks and uninstalls as below. Tags, a run by hand and a pull request labelled `full-tests` do all of it: build a native bundle for Intel and ARM on macOS, Linux and Windows (each Mac bundle with the helper Kumi uses Live's menus with, universal and ad hoc signed; the Intel one cross-built on Apple Silicon), then the compatibility release that existing installations update from, and serves them locally with the installers, as GitHub serves a release's files. On each system: installs with the one-liner from that copy, as producers do (Windows PowerShell 5.1 on Windows, with `KUMI_HOME` in a folder named in non-ASCII letters), checks the version, `doctor`, the bridge and its analysis worker, installs again as a repair, runs `kumi bridge --yes` into a scratch Remote Scripts folder, `kumi update`, `kumi update --rollback` and `kumi uninstall`. One install on each system (macOS arm64, Linux x64, Windows x64) then, in a home with spaces and non-ASCII letters, installs GitHub's latest release with this build's installer and then, with `KUMI_VERSION`, the release before this build; puts that release's bridge in; updates to this build and rolls back twice, checking the Kumi and the bridge each time; and uninstalls. The PATH line goes in zsh's `ZDOTDIR` on macOS and fish's `conf.d` on Linux, where that shell finds `kumi`, and the uninstall takes it out; Windows runs this in PowerShell 7: this build's install, the bridge, the rollbacks and the uninstall with `KUMI_HOME` in a folder named in non-ASCII letters, and the update from the release before in a home with spaces only (that release's own updater unpacks it). The transition jobs run the migration tests with Kumi 1.7.5 and the new bundles. The edges jobs run install.sh and install.ps1 against stand-in releases: a missing release, no answer, a sign-in page, a bad checksum, a Kumi that doesn't start, Ctrl-C. On a `v*` tag's push, once all these pass, `publish` attaches the bundle and the installers to the release, and publishes it when `scripts/release.py` wrote its notes. |

On a pull request, less runs:

- `Python Remote Script` skips macOS.
- Rust's build caches are saved from `main` only: a pull request restores
  `main`'s and saves none.
- The Installer builds and installs on one machine of each system only for a
  change to installing, updating, releasing, Willington's files, the Mac helper,
  the `tar` and PowerShell Kumi runs, how it downloads (its HTTP client and
  network settings) and builds paths, how Kumi's extension goes into Live, or a
  dependency; all six, the updates from GitHub's releases, the edges and the
  transition run with the `full-tests` label. A change to the bridge (the Remote
  Script, the protocol, the Live extension) or a version number builds, installs
  and checks Linux's; any other change builds Linux's bundle only.
- A change to how Kumi stores what it keeps (`crates/kumi-store`, settings and
  sign-ins, memory, techniques, playbook, gaps, an older Kumi's files) also gets
  Linux's install check, which updates and rolls back over existing data.
- The Installer's bundles, on pull requests and pushes to `main`, build with
  the `ci-release` profile, which skips the release profile's whole-program
  optimization and optimizes less. Tags build with `release`.

Each push to `main` runs what a pull request that changes installing runs, and
each tag all of the Installer. The Extended workflow runs every test each week.
Each night the Installer builds all six bundles with the `release` profile,
which keeps that profile's build cache warm for the next tag.

To merge into `main`, `Required CI` and `Willington files` must pass. The Installer isn't required.
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
