# Kumi: guide for agents

## 1. What Kumi is

Kumi is a producer's agent that works inside their DAW from the terminal: it reads the project, makes changes with its own history and undo, listens to the audio and learns the producer's library. It supports Ableton Live 12 (macOS, Windows), REAPER 7+ and Renoise 3.5+ (macOS, Windows, Linux); Logic Pro is in progress (#374), and SoundCraft is experimental (read-only, reached only with `--daw soundcraft`). One Rust workspace, shipped as GitHub releases.

Kumi starts a bridge and talks MCP to it over stdio; the bridge reaches the DAW over authenticated TCP on 127.0.0.1:
- Live: `ableton-mcp-server`, to the Remote Script (`ableton-loopback/v1`) and Kumi's Live extension.
- REAPER, Renoise and SoundCraft (and Logic): `daw-mcp-server --daw <daw>`, to Kumi's extension or tool in the DAW (`kumi-daw/1`); SoundCraft's bridge is a program beside it, which the host starts.

Words: **own bridge** = Kumi's `kumi-daw/1` bridge, for every DAW but Live. **integration** = a DAW's host side in Kumi (`kumi-daw-<daw>`). **provider** = a DAW's install, update, doctor and setup steps (`DawProvider`, in `kumi-daw`). **place** = where a conversation, its notes and jobs are kept. **FOCUS** = what the producer is looking at. **catch-up** = what changed since Kumi last looked. **change family** = a kind of change with its own HISTORY row and undo. **Hands** = drives Live's menus. **Ears** = how Kumi hears a DAW. **slots** = which model does each listening job. **kit** = Max for Live building blocks. **goldens** = byte-exact expected outputs. **oracles** = recordings of the old TypeScript Kumi (tag `v1.7.6`). **Willington** = optional native Live bindings.

## 2. Layout and architecture

`Ln` is a crate's layer: a crate uses only its own layer and below.

```text
crates/                     Kumi itself; DAW-neutral
  kumi/                     L6 the app: CLI, TUI (src/tui/), install, update, doctor; src/daw/ is wiring only (the DAW table)
  kumi-daw/                 L4 the contract between the app and a DAW's crate (no terminal or UI types)
  kumi-agent/               L3 agent loop (src/kernel/), DAW-neutral core (src/core/), MCP client (src/mcp/),
                               what every DAW's integration shares (src/integrations/shared/)
  kumi-ai/                  L2 model providers and clients, sign-in, local models
  kumi-listening/           L2 listening: measures, judge, rounds, references, notation
  kumi-library/             L2 the producer's library, plug-in formats, samples on disk
  kumi-web/                 L2 web and video tools
  kumi-design/              L2 sound design on a group of samples (cross, fuse, stack), seeded; built optimized even in tests
  kumi-media/, kumi-net/    L1 audio, voice, ffmpeg/yt-dlp/whisper; HTTP clients, proxies
  kumi-store/, kumi-genexpr/ L1 SQLite; gen~ interpreter (tests, and the Hermes device build's limits and output-stage proof)
  kumi-layout/              L1 the map's layout (t-SNE), built optimized even in tests
  kumi-types/, kumi-common/ L0 what every layer names; utilities (Live's bridge links kumi-common)
  kumi-testkit/             test support: dev-dependency only, depends on nothing
daws/                       everything DAW-specific, one folder per DAW
  ableton/                  kumi-daw-ableton/ (L5: Live's integration, its provider in src/app/, the M4L device builder),
                            ableton-mcp-server/ (bridge), remote-script/ (Python, in Live),
                            live-extension/ (TypeScript), protocol/ (the only list of Live's operations), max-standard/
  own/                      Kumi's own bridge: daw-bridge/ (kumi-daw/1 wire), daw-mcp-server/ (host),
                            kumi-daw-own/ (L5: what the own-bridge integrations share, their provider among it)
  reaper/                   reaper-kumi/ (REAPER's extension; SDK in sdk/), kumi-daw-reaper/ (L5)
  renoise/                  com.kumi.Bridge.xrnx/ (Lua tool), tests/ (against a mock Renoise), kumi-daw-renoise/ (L5)
  soundcraft/               soundcraft-kumi/ (Kumi's bridge beside SoundCraft, a program its host starts: the client of
                            SoundCraft's control channel and its recorded vectors, starting SoundCraft, kumi-daw/1),
                            kumi-daw-soundcraft/ (L5: experimental and read-only; its provider, `Beside`, opens SoundCraft
                            through the bridge program beside Kumi)
hermes-plugin/              Kumi as a Hermes Agent plugin: Python (standard library only) over `kumi serve --stdio`,
                            tests/ against a fixture runtime; the Hermes catalog installs this folder alone
testdata/oracles/<crate>/   recorded oracles; .ignore hides them from search (rg -u finds them)
plugin-formats/             plug-in preset formats Kumi builds in
vendor/willington/          Willington's files: owner only
scripts/                    dev.py, areas.toml, moves.toml; release.py and packaging, daws.toml (their DAW list);
                            test-isolated.sh/.ps1
docs/{en,ja,zh-CN}/         user docs in three languages
install.sh, install.ps1     the installers
.config/nextest.toml        the test sets: ci (main's CI), extended (every test)
.github/workflows/          CI, Extended, Installer, Network, Willington files, Caches, Release notes to Discord
```

Why it's shaped this way (keep it so):
- **One DAW, one folder.** DAW-specific code lives in `daws/<daw>/` so a DAW's work doesn't spread across the repo. Nothing in `crates/` uses `daws/` but the app (it wires the DAWs in) and tests (`daws/own/` only). A DAW's crates use no other DAW's, only `daws/own/`.
- **Dependencies point down** (`LAYERS` in `scripts/dev.py`, checked by Required CI). Small acyclic crates mean a change rebuilds and tests only what depends on it; `dev.py affected` reads the graph.
- **The app reaches a DAW only through `kumi-daw`**, which each `kumi-daw-<daw>` implements; `crates/kumi/src/daw/registry.rs` is the table of DAWs. Outside its folder, a new DAW adds only its `DawId`, profile and place names in `kumi-types`, one table entry, and (own-bridge DAWs) its entry in `daw-bridge`'s DAW list, its module in `daw-mcp-server` and its entry in `scripts/daws.toml`, the list the release and packaging scripts read.
- **Bridges are separate programs.** They run in or beside a DAW, work without Kumi (other MCP clients use them), use nothing of Kumi's but `kumi-common`, and have their own versions. Updating one means restarting the DAW. The exception is a `program` in `scripts/daws.toml` (SoundCraft's `soundcraft-kumi`): it runs beside the DAW, which loads nothing of it, and takes Kumi's version (section 5).
- **The Hermes plugin is self-contained.** The Hermes catalog installs `hermes-plugin/` alone, from a pinned commit: nothing in it reaches outside the folder, and it carries no compiled code (setup downloads the Kumi release it pins, checked by SHA-256). It has no Rust and no layer.
- **Frozen:** what an installed app is read by (`kumi` at the app's root, `package.json`, `bridge/prepared.json` and the package it names) and the wire protocols. Installed copies update, roll back and talk across versions through them.

Where new code goes:
- A DAW feature: `daws/<daw>/kumi-daw-<daw>/`, plus its bridge if it needs new operations. Shared by own-bridge DAWs: `daws/own/kumi-daw-own/`.
- DAW-neutral behaviour (history, plans, sessions, workflows): `crates/kumi-agent/src/core/`. Model providers: `crates/kumi-ai/`.
- A Live operation: `daws/ableton/protocol/` first (never copy operation names or the registry hash out of it), then the bridge and the Remote Script, tested as TESTING.md's "Writing tests" says. A change family: `CHANGES` in `daws/ableton/kumi-daw-ableton/src/changes.rs`.
- Anything else: the lowest crate that can hold it. Don't add DAW-specific code to `crates/` (kumi-library's REAPER and Renoise preset readers and one search boost are due to move out).
- A new crate gets its layer in `LAYERS` and a line in the tree above. A new top-level file or folder gets an area in `scripts/areas.toml`: CI fails on a file that belongs to no crate or area.

## 3. Conventions

Working:
- One task, one branch, one PR, in its own worktree: `python3 scripts/dev.py worktree new <name>` makes `../kumi-<name>` from origin/main. Branches are `<topic>/<name>`; `willington/` is the owner's.
- On long tasks, open the PR early and push regularly. A branch begun before a move: `python3 scripts/dev.py rebase-moves`.
- Before pushing: `python3 scripts/dev.py check` runs what Required CI runs for your change (whitespace, layers, the scripts' tests, fmt, the gating lint; `--dry-run` lists them), and `--tests` adds the affected tests main runs after the merge. No PR runs tests: run the ones your change touches yourself. `python3 scripts/dev.py affected --base origin/main` says what a branch affects.
- Run tests only through the isolated runner (a home of its own; needs cargo-nextest): `sh scripts/test-isolated.sh --profile ci -p <crate>`, one file with `-E 'test(/^<module>::/)'` (Windows: `scripts/test-isolated.ps1`).

Tests:
- They never need a running DAW, a sign-in, the network or anything from this machine, and never touch `~/.kumi` or a DAW's folders.
- A crate's integration tests are one binary: `tests/it/<file>.rs`, listed in `tests/it/main.rs`. Shared support goes in `kumi-testkit`.
- `ci` is the set main runs after each merge; long simulations, oracle replays and SDK interop belong in Extended, which runs by hand (`.config/nextest.toml` says what `ci` leaves out).
- Pull requests run no tests, and CI stays minimal on the way to no PR checks at all: section 6 says what runs. Cutting a test that costs more time than the breaks it catches is welcome.
- Goldens (`tests/goldens/` in `kumi`, `kumi-agent`, `kumi-daw-ableton`; `daw-mcp-server/tests/golden`; `daw-bridge/tests/vectors`) are byte-exact. `KUMI_BLESS=1` rewrites them: bless only an intended change, and say why in the PR. Oracles are never regenerated or edited.
- A refactor's proof: goldens unchanged, the same test list, an unchanged release dry run.

Commits and PRs:
- Subjects are plain English and say what changed ("The Network workflow parses again"). No conventional-commit prefixes.
- The PR body: what changes, the proof (commands and results), then its `Changelog:` lines (section 5).
- Squash-merge. The subject must end in `(#N)`: `release.py` finds each PR by it, and refuses to release past a commit on main without it.
- A PR that moves files: commit 1 is `git mv` only, commit 2 the fix-ups; record the move in `scripts/moves.toml`; merge with a merge commit titled `<PR title> (#N)` so git follows the moves.
- Only `Required CI` and `Willington files` gate a merge. Wait on the CI run of your pushed commit (`gh pr checks --required --watch` can return early): `gh run watch --exit-status "$(gh run list -c "$(git rev-parse HEAD)" -w CI --json databaseId -q '.[0].databaseId')"`

Docs:
- A user-doc change goes into `docs/en`, `docs/ja` and `docs/zh-CN` (and the three READMEs) in the same PR. Short, factual, bulleted.
- Docs that aren't for users (design docs, plans, agent notes, prompts) stay out of git; `.hermes/` is ignored. AGENTS.md and CLAUDE.md are the exceptions: a PR that adds, moves or splits a crate updates section 2.
- `vendor/willington/` changes only in the owner's PRs, from a `willington/` branch, that change nothing else.

## 4. Labels

Label every issue and PR you open (`--label` on `gh issue create` / `gh pr create`); fix them when the scope changes.
- **Type, exactly one:** `bug`, `enhancement`, `performance`, `security`, `chore` (dependencies, cleanup, refactors), `ci`, `documentation`, `flaky test`.
- **Where, one or more** (none for repo-wide work: CI, tooling, this file):
  - `daw: ableton`, `daw: reaper`, `daw: renoise`, `daw: logic`. A change in `daws/own/` gets each DAW it changes.
  - `area: bridge` (what runs in or beside a DAW), `area: app`, `area: install` (installers, setup, update, rollback), `area: agent` (models, prompts, tools), `area: listening`, `area: library`, `area: memory`, `area: video`, `area: devices` (Max for Live devices, racks, plug-ins), `area: willington`.
  - `platform: macos`, `platform: windows`, `platform: linux`: only when it's specific to that system.
- **Flags, when true:** `critical` (crashes or hangs a DAW, loses work, blocks a release); `bridge bump` (changes what a DAW loads, section 5); `migration` (changes the form of saved settings or data; update and rollback must keep them).
- **Status, while true:** `status: stacked` (the body says "Stacked on #N"), `status: blocked`, `status: needs decision` (the body says what's to decide), `status: needs live test`, `status: needs repro` (issues), `status: do not merge`. Never merge a PR with `stacked`, `blocked`, `needs decision` or `do not merge`.
- **Not for agents:** `release`, `dependencies`, `javascript`, and GitHub's defaults (`duplicate`, `invalid`, `wontfix`, `question`, `good first issue`, `help wanted`).
- In the body: `Fixes #N` when the PR finishes an issue, `Refs #N` when it's part of one.

## 5. Releases

- Kumi ships as GitHub releases: `install.sh`, `install.ps1` and `kumi update` install the latest published one. Nothing else is deployed.
- Kumi has one version, `X.Y.Z`. Live's bridge, REAPER's extension and Renoise's tool have their own, bumped when what that DAW loads changes (tests and Markdown aside):
  - Live: `daws/ableton/` (bridge, Remote Script, protocol, Live extension), `crates/kumi-common`, `vendor/willington`.
  - REAPER and Renoise: what `scripts/daws.toml` says each loads: REAPER's extension (`daws/reaper/reaper-kumi`) and the parts of `daws/own/daw-bridge` it links; Renoise's tool (`daws/renoise/com.kumi.Bridge.xrnx`).
- SoundCraft's program (`soundcraft-kumi`, beside `kumi` in the bundle) has no version of its own: it takes Kumi's, like `daw-mcp-server`. Its lines are plain `Changelog:` lines; `release.py` refuses `Changelog (soundcraft):`.
- **In a PR, never bump a version or edit CHANGELOG.md.** Write `Changelog:` lines: they become the release notes and the app's What's new, so each is one thing a producer will notice, most important first. `Changelog: none` for internal work; `Changelog (bridge):`, `Changelog (reaper):` or `Changelog (renoise):` files a line under that heading.
- Releasing is for the owner, or an agent asked to (`gh` signed in as a repository admin), once main's CI on the commit to release is green (pull requests run no tests, so main's run is the first to test a change):
  - `python3 scripts/release.py`: the dry run (version, CHANGELOG entry, notes, checks, and how main's CI stands).
  - `python3 scripts/release.py --go` (`--minor`, `--version X.Y.Z`, `--summary "…"`): refuses until main's CI on that commit is green, then bumps the versions, writes CHANGELOG.md and the READMEs, merges "Kumi X.Y.Z (#N)", tags `vX.Y.Z` and creates a draft release.
- The tag's Installer run builds the six bundles (macOS, Linux, Windows; Intel and ARM) with the `release` profile, tests installing, updating and migrating, attaches them and publishes the release. Nobody gets it before then. Nothing waits for CI on the release's own commit (versions and notes only): this run, on all six systems, is its check, and with the nightly run the only build on macOS and Windows.
- If that run fails, the release stays a draft: fix main and release the next patch (1.21.1 did this for 1.21.0). Never move, delete or re-push a `v*` tag.
- More: DEVELOPER_GUIDE.md "Releasing", DISTRIBUTION_POLICY.md.

## 6. Minimal CI

Kumi is one contributor's repository, worked on by many agents at once, and traditional CI doesn't fit that pace: CI is the bare minimum, on the way to no pull-request checks at all. A pull request waits only for what keeps main building; the tests run after the merge, on Linux; macOS and Windows build in the Installer's nightly and release runs. The owner decides any addition, in the PR that makes it; cutting needs no one's yes.

What runs:
- **A pull request:** `Required CI`, one job on Linux: the change's whitespace, every file in a package or an area, the crates' layers, the scripts' tests, the installers' syntax and, for a change that builds Rust (`dev.py affected`'s `rust_lint`), `cargo fmt --check` and the build with the one gating Clippy lint. No tests. With `Willington files`, nothing else gates a merge.
- **Main, after each merge:** `Required CI` again (it keeps the build caches pull requests restore), and the tests on Linux: `Rust / Linux` in three parts (the `ci` set, every package), the Remote Script's, the Live extension's and Renoise's tool's tests, and the Hermes plugin's (with the Hermes catalog's own checks on Hermes v0.22.7). No newer merge cancels a main run. A red one opens an issue ("Main's CI failed") and is fixed before anything else, by whoever broke it; no release goes out until main's CI on the commit to release is green.
- **The Installer:** on a release tag, which it publishes (all six bundles, each installed, the update from the last release, the migrations), and each night on main (all six bundles with the `release` profile, which keeps the tag's build cache warm, installed on each system; a failure opens an issue). Nothing else builds on macOS or Windows.
- **By hand, on any branch, when a change wants it (nothing requires it):** `gh workflow run CI --ref <branch>` (main's set), `gh workflow run Installer --ref <branch>` (a tag's run; only a tag publishes), `gh workflow run Extended --ref <branch>` (every test on Linux, macOS and Windows: oracle replays, long simulations, SDK interop, the eval runner's own tests) and `gh workflow run Network --ref <branch>` (the HTTP clients against each system's own trust store and proxies).

Rules:
- No new workflow, job, matrix entry, schedule, required check or step without the owner's yes in the PR; cutting one needs none. A one-off measurement or probe runs locally or through `workflow_dispatch` of a workflow that exists; it's never committed as a workflow of its own.
- A test earns its place by catching a break a producer would notice, at the lowest level that shows it, for less time than it costs. Extend a test before adding one. Don't test wiring, forwarding or defaults; test wording only where the words are the product (the goldens, Live's word checks, the eval runner).
- A new test asserts no duration and isn't a benchmark: it waits on a state, never on a clock. Older tests that still assert one are debt: make them wait on a state when you next touch them.
- A `ci` test takes seconds. One that needs longer is made faster, cut, or moved to Extended (`.config/nextest.toml`); past 30 s it fails by name.
- A test that fails on a change it has nothing to do with is fixed in the PR that sees it, if the fix is small and certain; otherwise open an issue (`flaky test`) naming the run. Never delete a test to hide a real break, and add no retries or quarantine.
- Locally, run only what a change affects (`python3 scripts/dev.py check`, with `--tests` for its tests), one build at a time.
