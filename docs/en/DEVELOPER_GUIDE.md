# Developer guide

English · [简体中文](../zh-CN/DEVELOPER_GUIDE.md) · [日本語](../ja/DEVELOPER_GUIDE.md)

How the repository fits together, how to work on each part, and how to release.
[Testing](TESTING.md) has every test command and what CI runs.

## Layout

| Folder | What's in it |
| --- | --- |
| `crates/kumi` | The native `kumi` command and terminal app (`src/tui/`), and opt-in checks against models or real Live (`examples/`) |
| `crates/kumi-runtime` | Agent loop, providers, sign-in, sessions, library, Live integration, media tools and MCP client |
| `crates/kumi-common` | Shared runtime utilities and source-compatible value handling |
| `crates/ableton-mcp-server` | Native MCP bridge, analysis worker, lifecycle, setup, migration and diagnostics |
| `remote-script` | The bridge's Remote Script, which runs inside Live (`ableton_mcp_remote_script.py`, the `AbletonMcpBridge/` entry point, Python tests) |
| `apps/live-extension` | Kumi's Live extension for Live 12.4 and later, on Live's Extensions SDK (TypeScript, with its own `package.json`) |
| `protocol` | `ableton-live-v1.operations.json`, the operation registry the bridge and the Remote Script share |
| `scripts` | Release builders, migration packaging, the Mac helper's build, the isolated test runners, and `native-kumi.mjs` behind `npm run kumi` and `npm run setup` |
| `install.sh`, `install.ps1` | The installers |

The four crates share the root Cargo workspace; there's no npm workspace. The
bridge also works without Kumi. The TypeScript implementation the crates were
ported from stays at the git tag `v1.7.6`.

## How the parts talk

```text
kumi (crates/kumi, crates/kumi-runtime)
  │  MCP over stdio: Kumi starts the bridge as a child process
  ▼
bridge (crates/ableton-mcp-server)
  │  ableton-loopback/v1: authenticated TCP on 127.0.0.1
  ├──► Remote Script inside Live (remote-script/)
  │  local channel to the Extension Host
  └──► Kumi's Live extension (apps/live-extension), Live 12.4+
```

- Kumi starts the bridge with the `full` deployment policy and an allow list of
  exactly the tools it uses (`ABLETON_MCP_TOOL_ALLOW`). The model calls a few
  bridge reads directly (`MODEL_TOOLS` in `crates/kumi-runtime/src/mcp/allowed_tools.rs`);
  Kumi's own tools call the rest.
- The bridge's router (`crates/ableton-mcp-server/src/bridge/router.rs`) sends each
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

Run native checks from the repository root. The isolated runner invokes
`cargo test --workspace --locked` with a temporary home:

```sh
npm ci --prefix crates/kumi-runtime/tests/support   # once: the official SDKs some tests run
cargo build --locked --workspace --all-targets
sh scripts/test-isolated.sh                 # PowerShell: ./scripts/test-isolated.ps1
sh scripts/test-isolated.sh -p kumi-runtime --test hands_transport
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
entry in `CHANGES` (`crates/kumi-runtime/src/integrations/ableton/changes.rs`).
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

Paths below are relative to `crates/ableton-mcp-server`.

| Path | What it is |
| --- | --- |
| `src/host.rs`, `src/host/` | MCP dispatch, strict tool schemas, transactions, undo and recovery |
| `src/tool_catalog.rs` | The single tool catalog: schemas, annotations, the capabilities each tool needs, and its deployment policy class |
| `src/live.rs`, `src/registry.rs` | Live types and adapters; loading and validating the registry and its hash |
| `src/bridge/` | The authenticated loopback client (`remote_adapter.rs`), the router, and the extension channel, launcher and folders |
| `src/transactions/` | Batches, device state, Session MIDI and discovery helpers |
| `src/mcp_protocol.rs`, `src/stdio.rs` | MCP wire handling for both protocol versions |
| `src/analysis*.rs`, `src/audio_*.rs`, `src/reference_analysis.rs` | Audio analysis in isolated workers |
| `src/delivery*.rs`, `src/lifecycle*.rs`, `src/setup.rs`, `src/migrate.rs`, `src/diagnostics.rs` | Configuration, secrets, install, upgrade, rollback and diagnostics |
| `src/als.rs`, `src/project*.rs`, `src/library_search.rs` | Saved Sets, Set snapshots and diffs, Live's library database |
| `src/follow_actions.rs` | The optional [Willington](WILLINGTON_INTEGRATION.md) Follow Actions |

**Contract rules.**

- The wire protocol is `ableton-loopback/v1`: canonical JSON (sorted keys,
  negative zero normalized), HMAC-SHA256 on requests and responses, bounded
  frames and collections, sequence numbers, and an epoch that changes each time
  the Remote Script starts. The details are in `remote-script/README.md`.
- The registry in `protocol/` is the only list of operations. The host and the
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

`apps/live-extension` is TypeScript, bundled with esbuild, with its own
`package.json`. It builds against Live's Extensions SDK, whose licence forbids
redistributing it, so it isn't in the repository: put a copy at
`vendor/ableton-extensions-sdk-1.0.0-beta.1/` in the repository root (the build
reads `package 3/dist/index.cjs` inside it). Run `npm ci` in
`apps/live-extension`, then `npm run build`, which writes `dist/extension.js`
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
the version in `crates/ableton-mcp-server/Cargo.toml` and `Cargo.lock`, starts
its subject with the new version ("Bridge 1.0.71: …"), and adds a
`### Bridge x.y.z` block under `## Unreleased` in `CHANGELOG.md`. If the
extension changed, rebuild and commit its bundle. Work goes on a branch and
reaches `main` by pull request.

**A Kumi release:**

1. On the branch, one commit titled "Kumi X.Y.Z: the changelog, READMEs and
   versions" sets the version in the root `package.json`,
   `crates/kumi-runtime/src/version.rs`, the `kumi`, `kumi-common` and
   `kumi-runtime` Cargo manifests and `Cargo.lock` (the packaging tests hold
   these equal); the Status line of the three READMEs; the ships-with
   line under "Bridge versions" in the three `KUMI_CHANGES.md`; and the
   `CHANGELOG.md`'s `## Unreleased` becomes `## X.Y.Z — date`, with a line
   that says which bridge it ships with.
2. Merge the pull request with a merge commit titled "Kumi X.Y.Z (#PR)".
3. Tag the merge commit `vX.Y.Z` and push the tag. The Installer workflow builds
   native bundles for Intel and ARM on macOS, Linux and Windows, tests installs
   and migration, and attaches the per-target bundles/manifests plus the
   compatibility `kumi.tar.gz`, `kumi-release.json` and `SHA256SUMS` to a draft
   release "Kumi X.Y.Z".
4. Write the release notes and publish the release. Only then do the installers,
   `kumi update` and the update check see it.

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
