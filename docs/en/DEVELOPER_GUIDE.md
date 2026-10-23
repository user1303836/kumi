# Developer guide

English · [简体中文](../zh-CN/DEVELOPER_GUIDE.md) · [日本語](../ja/DEVELOPER_GUIDE.md)

How the repository fits together, how to work on each part, and how to release.
[Testing](TESTING.md) has every test command and what CI runs.

## Layout

| Folder | What's in it |
| --- | --- |
| `crates/kumi` | The native `kumi` command and terminal app (`src/tui/`), and opt-in checks against models or real Live (`examples/`) |
| `crates/kumi-agent` | The agent: its loop, sessions, the DAW-neutral core (HISTORY, plans, places, FOCUS views, workflows), the MCP client, and what every DAW's integration shares |
| `crates/kumi-types` | The types and settings every layer names: DAW profiles and words, the agent's contracts with its tools, errors, places, the system, the version |
| `crates/kumi-net` | How Kumi reaches the network: its HTTP clients with the system's proxies and trust, why a request got no answer, the public-only web client and a model call's error |
| `crates/kumi-media` | Audio (decoding, analysis, matching a sound, DSP), voice input, and the programs that decode media (ffmpeg, yt-dlp, whisper.cpp) with a video's frames, captions and speech |
| `crates/kumi-ai` | The model clients: each provider's wire binding, the providers and their models, signing in, the models Kumi runs itself (ONNX Runtime), a turn's timing and how a failed call is told |
| `crates/kumi-web` | The web and video tools: searching and reading the web (pages, PDFs, GitHub, Max patches), and watching a tutorial for its words, frames and sound |
| `crates/kumi-library` | The producer's library (learning, search, presets, Sets, taste), the plug-ins' formats and roles, and finding samples on disk; each DAW says where its own library is |
| `crates/kumi-listening` | Listening: what a listen measures, the judge and its checklist, the engine's rounds, the listening models and their slots, style references and notation; each DAW plays and renders for it |
| `crates/kumi-daw` | What a DAW's crate gives the app and gets from it: the DAW's provider (what it is, and its bridge found, installed, checked and taken out), the doctor's checks, setup's DAW side, and the small ports its steps use (what they say, ask and run) |
| `crates/kumi-common` | Shared runtime utilities and source-compatible value handling |
| `daws/ableton/kumi-daw-ableton` | Live, host side: its integration (the Set's observation, changes and history, rendering and Kumi Ears, Hands, its library and manual), the Max for Live device builder, and its provider (`src/app/`: finding, opening and checking Live, its bridge put in, updated, rolled back and taken out, its part of the doctor, the report and first-run setup) |
| `daws/ableton/ableton-mcp-server` | Native MCP bridge, analysis worker, lifecycle, setup, migration and diagnostics |
| `daws/ableton/remote-script` | The bridge's Remote Script, which runs inside Live (`ableton_mcp_remote_script.py`, the `AbletonMcpBridge/` entry point, Python tests) |
| `daws/ableton/live-extension` | Kumi's Live extension for Live 12.4 and later, on Live's Extensions SDK (TypeScript, with its own `package.json`) |
| `daws/ableton/protocol` | `ableton-live-v1.operations.json`, the operation registry the bridge and the Remote Script share |
| `scripts` | Release builders, migration packaging, the Mac helper's build, the isolated test runners, and `native-kumi.mjs` behind `npm run kumi` and `npm run setup` |
| `daws/own/daw-mcp-server` | Kumi's host for REAPER and Renoise (`daw-mcp-server --daw reaper\|renoise`): MCP over stdio, the changes' journal and undo, listening jobs, catch-up; also usable by other MCP clients |
| `daws/own/kumi-daw-own` | What REAPER's and Renoise's integrations share, host side: the integration through Kumi's own bridges, the change engine and its undo, catching up, listening, audio to MIDI and stems, instant phrases, the pictures and FOCUS views, kept scripts and delivery to a spec, and the provider both DAWs' bridges are found, installed, checked and taken out through (`provider`) |
| `daws/reaper/kumi-daw-reaper` | REAPER, host side: its integration through Kumi's own bridge, the tools only REAPER has (variations of its items, delivery, routing), where its library is, and its provider's value (`PROVIDER`) |
| `daws/reaper/reaper-kumi` | Kumi's REAPER extension (`reaper_kumi`), built against REAPER's SDK headers vendored in `sdk/` |
| `daws/own/daw-bridge` | What the REAPER and Renoise bridges share: the loopback wire (`kumi-daw/1`), its handshake and auth, discovery, finding the DAWs, the doctor's checks and the bridge files' lifecycle |
| `daws/renoise` | Kumi's Renoise tool (`com.kumi.Bridge.xrnx`, Lua) and its tests against a mock of Renoise's API |
| `daws/renoise/kumi-daw-renoise` | Renoise, host side: its integration through Kumi's own bridge, the tools only Renoise has (variations of its lines and phrases, the sample map, chopping, cleaning up jams, layered breaks), where its library is, and its provider's value (`PROVIDER`) |
| `install.sh`, `install.ps1` | The installers |

The crates share the root Cargo workspace; there's no npm workspace. The
bridge also works without Kumi. The TypeScript implementation the crates were
ported from stays at the git tag `v1.7.6`.

When a folder moves, `scripts/moves.toml` records where it went. A branch begun
before a move catches up with `python3 scripts/dev.py rebase-moves` (`--merge`
merges main instead of rebasing): git follows the moved folders, the old paths
in the branch's own lines are rewritten, and what's left (conflicts, files at an
old path, build errors) is listed.

## How the parts talk

```text
kumi (crates/kumi, crates/kumi-agent)
  │  MCP over stdio: Kumi starts the bridge as a child process
  ▼
bridge (daws/ableton/ableton-mcp-server)
  │  ableton-loopback/v1: authenticated TCP on 127.0.0.1
  ├──► Remote Script inside Live (daws/ableton/remote-script/)
  │  local channel to the Extension Host
  └──► Kumi's Live extension (daws/ableton/live-extension), Live 12.4+

kumi (crates/kumi, crates/kumi-agent)
  │  MCP over stdio: Kumi starts its host for REAPER or Renoise
  ▼
host (daws/own/daw-mcp-server --daw reaper|renoise)
  │  kumi-daw/1: authenticated TCP on 127.0.0.1 (daws/own/daw-bridge)
  ├──► Kumi's REAPER extension (daws/reaper/reaper-kumi), in REAPER's UserPlugins
  └──► Kumi's Renoise tool (daws/renoise), in Renoise's Scripts/Tools
```

- Kumi starts the bridge with the `full` deployment policy and an allow list of
  exactly the tools it uses (`ABLETON_MCP_TOOL_ALLOW`). The model calls a few
  bridge reads directly (`MODEL_TOOLS` in `crates/kumi-agent/src/mcp/allowed_tools.rs`);
  Kumi's own tools call the rest.
- The bridge's router (`daws/ableton/ableton-mcp-server/src/bridge/router.rs`) sends each
  operation to the Remote Script, or to the extension when only the extension
  has it. The bridge can start Live's Extension Host itself when Live's
  Developer Mode keeps it from starting extensions.
- `kumi bridge` installs the bridge into Live through the bridge's lifecycle
  command, and finds it later through
  `Remote Scripts/AbletonMcpBridge/bridge-reference.json`.

## Setup

With Rust, Cargo, Python 3.11 or later, and git:

```sh
cargo build --release --locked --workspace --bins
cargo run --release -p kumi --
cargo run --release -p kumi -- bridge --allow-dirty   # Live must be closed
```

`npm run setup` and `npm run kumi -- ...` still work: they build this checkout
with Cargo when it's installed, and otherwise hand off to the matching published
native release. Node.js is needed for those, the Live extension and some tests,
not for the native application.

A checkout shares `~/.kumi` (settings, sign-ins, conversations, the bridge's
state) with an installed Kumi. `--allow-dirty` lets `kumi bridge` install the
bridge from a checkout with uncommitted changes. On Windows without Developer Mode or an
elevated shell, tests that create symlinks skip or fail; CI's runners can.

## Build, test and measure

Run native checks from the repository root. The isolated runner runs every test
with cargo-nextest, each in a process of its own, with a temporary home:

```sh
npm ci --prefix crates/kumi-agent/tests/support   # once: the official SDKs some tests run
cargo build --locked --workspace --all-targets
sh scripts/test-isolated.sh                 # PowerShell: ./scripts/test-isolated.ps1
sh scripts/test-isolated.sh -p kumi-daw-ableton -E 'test(/^hands_transport::/)'
python3 -m unittest discover -s scripts/tests -p 'test_*release.py'
```

Run performance gates with optimized binaries, outside coverage or other heavy
builds. Build the sibling analysis worker before starting the benchmark:

```sh
cargo build --locked --release -p ableton-mcp-server --bins
cargo run --locked --release -p ableton-mcp-server --bin ableton-mcp-benchmark
```

The benchmark prints JSON measurements and exits nonzero when a budget fails.
Its memory columns report tracked Rust allocations; they are not separate V8
heap, external and ArrayBuffer measurements. Debug timings are not release
performance evidence.

See [Testing](TESTING.md) for the other tests and the opt-in checks against
models and real Live.

## Working on Kumi

**A change family** (a kind of change with its own HISTORY row and undo) is an
entry in `CHANGES` (`daws/ableton/kumi-daw-ableton/src/changes.rs`).
Metadata comes from `assets/changes.json`; summaries live in
`changes/summaries.rs` and `changes/more_summaries.rs`. Each entry names the Kumi
tool, the bridge's preview and apply, a family (one of the fixed set of pictures
HISTORY and NOW draw), a model description and a plain-language summary. Give it `since` (the
first bridge release it works with, in `bridge_version.rs`) when older bridges
refuse it, and `permanent` when Live gives no way back. Tests check every family
for a unique tool, a title from a bare preview, descriptions that never ask the
model to confirm, and host-only bridge tools. Run it on real Live with its undo
(the `accept_live` example) before offering it.

**An action** (something that isn't a change to the Set, with nothing to undo,
such as playing) goes in `ACTIONS` in `actions.rs`, with metadata in
`assets/actions.json`.

The change eval (the `eval_changes` example) reads the bridge's tool schemas
from its catalog, so there's nothing to regenerate when they change.

The terminal app's design and foundations are in
[commands, keys and screens](KUMI_TUI.md#design-notes).

## Working on the bridge

The bridge's modules, in `daws/ableton/ableton-mcp-server/src` (a module `x` is
`x.rs` and, for its parts, `x/`):

| Module | What it is |
| --- | --- |
| `host` | MCP dispatch, strict tool schemas, transactions, undo and recovery |
| `protocol` | MCP's wire for both protocol versions (`mcp`, `stdio`); the single tool catalog (`tool_catalog`: schemas, annotations, the capabilities each tool needs, and its deployment policy class); Live's operation registry (`registry`: loading and validating it and its hash) |
| `live` | Live types and adapters, and the simulator, whose parts are in `simulator/` |
| `bridge` | The authenticated loopback client (`remote_adapter.rs`), the router, and the extension channel, launcher and folders |
| `transactions` | Batches, device state, Session MIDI and discovery helpers |
| `analysis` | Audio analysis in isolated workers (`runner`, `job_worker`), loudness standards, diagnosis, key estimation and reference comparison |
| `lifecycle` | Configuration and secrets (`delivery`), install, upgrade and rollback, setup, migration and diagnostics |
| `project` | Saved Sets (`als`), Set snapshots and diffs (`semantic`, `semantic_diff`), Live's library database (`library_search`, `sqlite_reader`) |
| `follow_actions` | The optional [Willington](WILLINGTON_INTEGRATION.md) Follow Actions |

**Contract rules.**

- The wire protocol is `ableton-loopback/v1`: canonical JSON (sorted keys,
  negative zero normalized), HMAC-SHA256 on requests and responses, bounded
  frames and collections, sequence numbers, and an epoch that changes each time
  the Remote Script starts. The details are in `daws/ableton/remote-script/README.md`.
- The registry in `daws/ableton/protocol/` is the only list of operations. The host and the
  Remote Script each hash it and must agree, or Live never connects; a host
  test runs the Remote Script's hashing to hold them equal. Never copy
  operation names or the hash into another source file.
- Changes go through purpose-specific operations with a preview, an apply and
  an undo. The one exception is `python.run` (`live_run_python`), which runs
  Python on Live's main thread with Live's undo as the only way back; it has its
  own policy class, `python`, allowed only by the `full` profile.
- The bridge's internal `get(ref)` is a bounded serializer over fixed rows, not
  a general reader of Live's object model; MCP reads stay purpose-specific.
- The Remote Script does all its work with Live on Live's main thread: it
  serves the sockets itself inside Live's display tick (and, between ticks,
  Live's own timer), within a time budget.
  Its only other threads write the diagnostics file and receive realtime UDP. A
  new epoch invalidates every earlier reference and cursor. A Live shape
  the Remote Script doesn't recognize is reported unavailable, never faked.
- stdout carries only the MCP protocol. Diagnostics go to stderr, without
  request data.
- Process-backed operations use `AsyncLiveAdapter`; the simulator also retains
  synchronous methods. Test compatibility work against both paths.
- Tests never need a running Live, a device, a particular machine or local-only
  material. Every new operation gets tests, including bad input and recovery.

**MCP versions.** The bridge speaks both `2025-11-25` (initialize, then
requests) and `2026-07-28` (per-request `params._meta` with the protocol version
and client capabilities, plus `server/discover`); one process uses one or the
other. Unknown versions get `-32022`; bad metadata `-32602`. Modern results carry
`resultType: "complete"` and also return JSON in `structuredContent`. The modern
version has no push: `live_subscribe` works only in the older one, and modern
clients poll. Client metadata never grants access to Live. See the
[specification](https://modelcontextprotocol.io/specification/2026-07-28/basic/versioning).

## Working on the Live extension

`daws/ableton/live-extension` is TypeScript, bundled with esbuild, with its own
`package.json`. It builds against Live's Extensions SDK, whose licence forbids
redistributing it, so it isn't in the repository: put a copy at
`vendor/ableton-extensions-sdk-1.0.0-beta.1/` in the repository root (the build
reads `package 3/dist/index.cjs` inside it). Run `npm ci` in
`daws/ableton/live-extension`, then `npm run build`, which writes `dist/extension.js`
and its `.sha256`; commit both. `npm run typecheck` needs the SDK too. Without
the SDK, the build stops and the committed bundle stays; `npm test` checks the
bundle against its checksum. Measurements of how Live runs the extension are in
[the evidence](../evidence/live-extension.md).

## Willington's files

[Willington](WILLINGTON_INTEGRATION.md)'s repository is private: Kumi carries
only its runtime files, in `vendor/willington/`, which change only by a
Willington update (below). Beside them are Willington's license notice
(`LICENSE` or `LICENSE.md`; Kumi's MIT license doesn't cover these files) and
`release.json`:

```json
{"schema": "kumi-willington-vendor/v1", "version": "0.4.0", "commit": "<Willington's 40-character commit>",
 "files": {"WillingtonRuntime/__init__.py": "<SHA-256>", "LICENSE": "<SHA-256>"}}
```

`files` lists every file in the folder but itself, by names every platform's
checkout can hold: ASCII letters, digits, `.`, `_` and `-`, no Windows device
names or trailing dots, and no two names that differ only in case.
`scripts/build-native-release.py` refuses a release when the folder holds a file
`release.json` doesn't list, one whose SHA-256 differs, or one that isn't a
Willington runtime file: only `.py`, `.json`, `.md`, `.pyd` and `.dylib` files
inside `WillingtonRuntime`, `WillingtonBindings`, `WillingtonDeviceTools` and
`WillingtonRackZones` qualify, so sources, headers, debug files and bytecode
caches never ship. `test_native_release.py` runs the same check on the
repository's folder in CI, and `.gitattributes` keeps the folder byte for byte
and out of whitespace checks.

A release stages the files inside the bridge's Remote Script, at
`AbletonMcpBridge/willington/`, with only its own platform's native libraries
(`.pyd` on Windows, `.dylib` on macOS), at most 16 MiB; a Linux bundle carries
none. The bridge's install copies that folder with it, with a `__pycache__`
blocker beside the Python files so the installed tree stays as the install
receipt records it. Two files in the installed bridge are the producer's, not
the release's: `willington.json` and the Follow Action self-test receipt,
`willington/WillingtonBindings/self-test.json`. They don't count as drift, and
an install carries them over. The bridge puts the folder on Python's path only
once `willington.json` turns Willington on, after any copy installed beside the
bridge.

**Updating Willington.** Willington's Bundle workflow builds its native
libraries on each platform they're for and keeps the matrix bundle as each
run's artifact. An update takes a run from a push to Willington's `main`:

1. Find the run: `gh run list -R xonedsp/willington -w Bundle -b main -e push -s success`.
2. On a branch from `main` named `willington/<anything>`, run
   `python3 scripts/vendor-willington.py --run <run>`. It checks that the run
   is a successful Bundle run on a push to Willington's `main` and that its
   commit is on `main`, that the artifact matches the digest GitHub recorded and
   the bundle its SHA-256, and fetches Willington's license at that commit. It
   replaces the folder only once the new one passes the release check, then
   prints the commit, the run and the artifact's digest.
3. Open a pull request that changes nothing else, with what it printed.
   `Willington files` passes only the repository owner's pull requests from a
   `willington/` branch in this repository, and the Installer builds and
   installs all six platforms.

A reviewer checks an update with
`python3 scripts/vendor-willington.py --check <run>`, which rebuilds the folder
from the run and compares it file by file.

CI builds the libraries, so their hashes can differ from the ones validated in
Live. Check an update in Live before the release that ships it, and run the
[Follow Action self-test](WILLINGTON_INTEGRATION.md#follow-action-self-test)
for its library.

## Releasing

**Commits** have plain-English subjects that say what changed for the producer
("Kumi: talk to it while it works"). A bridge or Remote Script change bumps
the version in `daws/ableton/ableton-mcp-server/Cargo.toml` and `Cargo.lock`, starts
its subject with the new version ("Bridge 1.0.71: …"), and adds a
`### Bridge x.y.z` block under `## Unreleased` in `CHANGELOG.md`. If the
extension changed, rebuild and commit its bundle. Work goes on a branch and
reaches `main` by pull request.

**A Kumi release** is one command, from any checkout, with `gh` signed in as a
repository admin. Run it right after a pull request merges:

```sh
python3 scripts/release.py          # dry run: the version, the CHANGELOG entry, the notes and the checks
python3 scripts/release.py --go     # the release
```

- **What ships** comes from `main` since the last tag: each merged pull
  request's `Changelog:` lines, as written ("none" adds nothing). A line goes
  under the bridge's heading when its pull request changes the bridge and none
  of Kumi's crates or installers. In a pull request that changes both, write
  `Changelog (bridge):` for a line about the bridge. The dry run shows where
  each line goes.
- **What's new:** Kumi builds `CHANGELOG.md` in and shows a release's list
  items as **What's new** the first time it starts after an update (five at
  most, newest release first; `/changelog` has the rest). Paragraphs, such as
  the ships-with and tested-with lines, aren't shown, so each `Changelog:` line
  is one item a producer reads there: say what they'll notice, the ones that
  matter most first.
- **The bridge** gets a new version when a pull request changes what Live
  loads: `daws/ableton/ableton-mcp-server`, `crates/kumi-common` (the host links it),
  `daws/ableton/remote-script`, `daws/ableton/protocol`, `daws/ableton/live-extension` or `vendor/willington`
  (tests and Markdown aside). When the host's dependencies in `Cargo.lock`
  change, the dry run says so; `--bridge` gives the bridge a new version then.
- **REAPER's extension and Renoise's tool** get new versions the same way, each
  when a pull request changes what its DAW loads: `daws/reaper/reaper-kumi` and
  `daws/own/daw-bridge`, or `daws/renoise/com.kumi.Bridge.xrnx`. `--reaper` and
  `--renoise` give one a new version anyway, `--no-reaper` and `--no-renoise`
  keep it; `--no-bridge` is only Live's. A line written `Changelog (reaper):`
  or `Changelog (renoise):` goes under its heading. With `--go` they move in
  `daws/reaper/reaper-kumi/Cargo.toml` and `Cargo.lock`, and in the tool's
  `payload/version.lua` and `manifest.xml`, and each one's ships-with line moves
  on in the three `KUMI_GUIDE.md`. One the last release didn't have ships at its
  version, and its line names this release already.
- **The version** is the next patch; `--minor` or `--version X.Y.Z` choose
  another.
- **With `--go`** it commits "Kumi X.Y.Z: the changelog, READMEs and versions"
  on `release/vX.Y.Z`: the version in the root `package.json`,
  `crates/kumi-types/src/version.rs`, the `kumi`, `kumi-common` and
  `kumi-types` Cargo manifests and `Cargo.lock` (the packaging tests hold
  these equal), and the bridge's when it changed; the Status line of the three
  READMEs; the ships-with line under "Bridge versions" in the three
  `KUMI_CHANGES.md`; and a `## X.Y.Z — date` entry in `CHANGELOG.md`. It opens
  the pull request "Kumi X.Y.Z", merges it with the admin bypass as
  "Kumi X.Y.Z (#PR)" without waiting for its CI, tags the merge commit
  `vX.Y.Z`, pushes the tag and creates the draft release with its notes
  (`--summary` opens them with a sentence).
- **The tag's Installer run** builds native bundles for Intel and ARM on macOS,
  Linux and Windows, tests installs and migration, attaches the per-target
  bundles/manifests plus the compatibility `kumi.tar.gz`, `kumi-release.json`,
  `SHA256SUMS` and the installers, and publishes the release when `release.py`
  wrote its notes (they end with `<!-- kumi:release-notes -->`) and both
  installers are attached. Only then do the installers, `kumi update` and the
  update check see it. Any other draft, such as a tag pushed by hand with notes
  to come, stays a draft until someone publishes it.

**Local native release staging**, from a clean commit:

```sh
python3 scripts/build-hands.py              # macOS only; universal, ad hoc signed helper in target/hands/
MACOSX_DEPLOYMENT_TARGET=13.0 python3 scripts/build-native-release.py --target aarch64-apple-darwin --out release/native/aarch64-apple-darwin
python3 -m unittest discover -s scripts/tests -p test_native_release.py
```

Use the host's Rust target triple; on an Apple Silicon Mac,
`--target x86_64-apple-darwin` cross-builds the Intel bundle. The builder runs a
locked release build and binds the bridge artifact to the commit, Cargo
lockfile, build recipe and exact file hashes. `--profile ci-release`, which CI
uses on pull requests and `main`, skips the release profile's whole-program
optimization to build faster; a published bundle always uses the default
`release`. Mac bundles need the current helper in `target/hands/`; other
bundles don't carry it. The server and analysis worker must be shipped
together. The `--binaries-dir` override packages existing binaries and
does not establish that they were built with release optimizations.

The aggregation step retains the manifest consumed by existing Node 24
installations:

```sh
python3 scripts/build-migration-release.py release/native/*/kumi-release.json --node 24.21.0 --out release/installer
export KUMI_LEGACY_APP="$(python3 scripts/fetch-legacy-release.py)"
KUMI_NATIVE_RELEASES="$PWD/release/installer" python3 -m unittest discover -s scripts/tests -p test_migration_release.py
```

Use the actual Node 24 version selected by the release workflow. The migration
tests run the last JavaScript release, Kumi 1.7.5: `fetch-legacy-release.py`
downloads its published bundle, checks its SHA-256, unpacks it and prints the
folder. Run them with Node 24 and both variables set, so every test runs.

Fresh installs select a native target and do not download Node. Existing Node 24
installations get a small compatibility bootstrap during `kumi update`
(`scripts/migration/kumi.mjs`, which every bundle ships as
`apps/kumi/bin/kumi.mjs`), which downloads and unpacks only their platform's
native bundle. The managed Node remains for rollback and optional YouTube
challenges. On Windows, the first native start replaces the old launcher; cmd,
still running it, resumes in the new one's padding and exits. Older Node majors
may require rerunning the installer with the same `KUMI_HOME`.

**The bridge** has no release of its own: it ships inside each Kumi release.
[Distribution](DISTRIBUTION_POLICY.md) covers what a release contains and how
it's checked.

## Docs

Every doc in `docs/en` has a Japanese and a Chinese copy in `docs/ja` and
`docs/zh-CN`, and the READMEs have `README.ja.md` and `README.zh-CN.md`. A
change to one language goes into all three in the same pull request. The
bridge's docs (its README and the fourteen `docs/en` pages listed in `DOCUMENTS`
in `scripts/build-native-release.py`) are packed with it, so their names are
fixed and their links must resolve; the packaging tests check them (see
[Testing](TESTING.md#docs)).

## Prior art

The bridge started from other Ableton MCP servers:
[bschoepke/ableton-live-mcp](https://github.com/bschoepke/ableton-live-mcp),
[uisato/ableton-mcp-extended](https://github.com/uisato/ableton-mcp-extended),
[Simon-Kansara/ableton-live-mcp-server](https://github.com/Simon-Kansara/ableton-live-mcp-server),
[jasper-zheng/ableton-sdk-mcp](https://github.com/jasper-zheng/ableton-sdk-mcp)
and [ahujasid/ableton-mcp](https://github.com/ahujasid/ableton-mcp).
