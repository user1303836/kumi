# Kumi: guide for agents

## 1. What Kumi is

Kumi is a producer's agent that works inside their DAW from the terminal: it reads the project, makes changes with its own history and undo, listens to the audio and learns the producer's library. It supports Ableton Live 12 (macOS, Windows), REAPER 7+ and Renoise 3.5+ (macOS, Windows, Linux); Logic Pro is in progress (#374). One Rust workspace, shipped as GitHub releases.

Kumi starts a bridge and talks MCP to it over stdio; the bridge reaches the DAW over authenticated TCP on 127.0.0.1:
- Live: `ableton-mcp-server`, to the Remote Script (`ableton-loopback/v1`) and Kumi's Live extension.
- REAPER and Renoise (and Logic): `daw-mcp-server --daw <daw>`, to Kumi's extension or tool in the DAW (`kumi-daw/1`).

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
  kumi-media/, kumi-net/    L1 audio, voice, ffmpeg/yt-dlp/whisper; HTTP clients, proxies
  kumi-store/, kumi-genexpr/ L1 SQLite; gen~ test interpreter
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
testdata/oracles/<crate>/   recorded oracles; .ignore hides them from search (rg -u finds them)
plugin-formats/             plug-in preset formats Kumi builds in
vendor/willington/          Willington's files: owner only
scripts/                    dev.py, areas.toml, moves.toml; release.py and packaging, daws.toml (their DAW list);
                            test-isolated.sh/.ps1
docs/{en,ja,zh-CN}/         user docs in three languages
install.sh, install.ps1     the installers
.config/nextest.toml        CI's test sets: ci, ci-os, extended
.github/workflows/          CI, Extended, Installer, Network, Willington files, Caches
```

Why it's shaped this way (keep it so):
- **One DAW, one folder.** DAW-specific code lives in `daws/<daw>/` so a DAW's work doesn't spread across the repo. Nothing in `crates/` uses `daws/` but the app (it wires the DAWs in) and tests (`daws/own/` only). A DAW's crates use no other DAW's, only `daws/own/`.
- **Dependencies point down** (`LAYERS` in `scripts/dev.py`, checked by Required CI). Small acyclic crates mean a change rebuilds and tests only what depends on it; `dev.py affected` reads the graph and CI picks its jobs from it.
- **The app reaches a DAW only through `kumi-daw`**, which each `kumi-daw-<daw>` implements; `crates/kumi/src/daw/registry.rs` is the table of DAWs. Outside its folder, a new DAW adds only its `DawId`, profile and place names in `kumi-types`, one table entry, and (own-bridge DAWs) its entry in `daw-bridge`'s DAW list, its module in `daw-mcp-server` and its entry in `scripts/daws.toml`, the list the release and packaging scripts read.
- **Bridges are separate programs.** They run in or beside a DAW, work without Kumi (other MCP clients use them), use nothing of Kumi's but `kumi-common`, and have their own versions. Updating one means restarting the DAW.
- **Frozen:** binary names, the installed bundle layout and the wire protocols. Installed copies update, roll back and talk across versions through them.

Where new code goes:
- A DAW feature: `daws/<daw>/kumi-daw-<daw>/`, plus its bridge if it needs new operations. Shared by own-bridge DAWs: `daws/own/kumi-daw-own/`.
- DAW-neutral behaviour (history, plans, sessions, workflows): `crates/kumi-agent/src/core/`. Model providers: `crates/kumi-ai/`.
- A Live operation: `daws/ableton/protocol/` first (never copy operation names or the registry hash out of it), then the bridge and the Remote Script, tested as TESTING.md's "Writing tests" says. A change family: `CHANGES` in `daws/ableton/kumi-daw-ableton/src/changes.rs`.
- Anything else: the lowest crate that can hold it. Don't add DAW-specific code to `crates/` (kumi-library's REAPER and Renoise preset readers and one search boost are due to move out).
- A new crate gets its layer in `LAYERS` and a line in the tree above. A new top-level file or folder gets an area in `scripts/areas.toml`: CI fails on a file that belongs to no crate or area.

## 3. Conventions

Working:
- One task, one branch, one PR, in its own worktree: `python3 scripts/dev.py worktree new <name> --lean` makes `../kumi-<name>` from origin/main, without the oracles only Extended reads. Branches are `<topic>/<name>`; `willington/` is the owner's.
- On long tasks, open the PR early and push regularly. A branch begun before a move: `python3 scripts/dev.py rebase-moves`.
- Before pushing: `python3 scripts/dev.py check` runs what Required CI runs for your change (whitespace, fmt, the gating lint, layers, the affected tests; `--dry-run` lists them). `python3 scripts/dev.py affected --base origin/main` says what a branch affects.
- Run tests only through the isolated runner (a home of its own; needs cargo-nextest): `sh scripts/test-isolated.sh --profile ci -p <crate>`, one file with `-E 'test(/^<module>::/)'` (Windows: `scripts/test-isolated.ps1`).

Tests:
- They never need a running DAW, a sign-in, the network or anything from this machine, and never touch `~/.kumi` or a DAW's folders.
- A crate's integration tests are one binary: `tests/it/<file>.rs`, listed in `tests/it/main.rs`. Shared support goes in `kumi-testkit`.
- `ci` is the fast set; long simulations, oracle replays and SDK interop belong in Extended (`.config/nextest.toml` says what `ci` leaves out).
- CI tests what a change affects (`dev.py affected`): Rust / Linux in three parts and Rust / macOS gate a merge; Rust / Windows runs on main's pushes; `full-tests` runs every test on every system.
- Goldens (`tests/goldens/` in `kumi`, `kumi-agent`, `kumi-daw-ableton`; `daw-mcp-server/tests/golden`; `daw-bridge/tests/vectors`) are byte-exact. `KUMI_BLESS=1` rewrites them: bless only an intended change, and say why in the PR. Oracles are never regenerated or edited.
- A refactor's proof: goldens unchanged, the same test list, an unchanged release dry run.

Commits and PRs:
- Subjects are plain English and say what changed ("The Network workflow parses again"). No conventional-commit prefixes.
- The PR body: what changes, the proof (commands and results), then its `Changelog:` lines (section 5).
- Squash-merge. The subject must end in `(#N)`: `release.py` finds each PR by it, and refuses to release past a commit on main without it.
- A PR that moves files: commit 1 is `git mv` only, commit 2 the fix-ups; record the move in `scripts/moves.toml`; add `full-tests`; merge with a merge commit titled `<PR title> (#N)` so git follows the moves.
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
- **Flags, when true:** `critical` (crashes or hangs a DAW, loses work, blocks a release); `bridge bump` (changes what a DAW loads, section 5); `migration` (changes the form of saved settings or data; update and rollback must keep them); `full-tests` (every test, and the Installer on all six platforms: add it to a PR that moves files, has `bridge bump` or `migration`, or changes platform-specific code, and wait for it).
- **Status, while true:** `status: stacked` (the body says "Stacked on #N"), `status: blocked`, `status: needs decision` (the body says what's to decide), `status: needs live test`, `status: needs repro` (issues), `status: do not merge`. Never merge a PR with `stacked`, `blocked`, `needs decision` or `do not merge`.
- **Not for agents:** `release`, `dependencies`, `javascript`, and GitHub's defaults (`duplicate`, `invalid`, `wontfix`, `question`, `good first issue`, `help wanted`).
- In the body: `Fixes #N` when the PR finishes an issue, `Refs #N` when it's part of one.

## 5. Releases

- Kumi ships as GitHub releases: `install.sh`, `install.ps1` and `kumi update` install the latest published one. Nothing else is deployed.
- Kumi has one version, `X.Y.Z`. Live's bridge, REAPER's extension and Renoise's tool have their own, bumped when what that DAW loads changes (tests and Markdown aside):
  - Live: `daws/ableton/` (bridge, Remote Script, protocol, Live extension), `crates/kumi-common`, `vendor/willington`.
  - REAPER and Renoise: what `scripts/daws.toml` says each loads: REAPER's extension (`daws/reaper/reaper-kumi`) and the parts of `daws/own/daw-bridge` it links; Renoise's tool (`daws/renoise/com.kumi.Bridge.xrnx`).
- **In a PR, never bump a version or edit CHANGELOG.md.** Write `Changelog:` lines: they become the release notes and the app's What's new, so each is one thing a producer will notice, most important first. `Changelog: none` for internal work; `Changelog (bridge):`, `Changelog (reaper):` or `Changelog (renoise):` files a line under that heading.
- Releasing is for the owner, or an agent asked to (`gh` signed in as a repository admin), right after a merge:
  - `python3 scripts/release.py`: the dry run (version, CHANGELOG entry, notes, checks).
  - `python3 scripts/release.py --go` (`--minor`, `--version X.Y.Z`, `--summary "…"`): bumps the versions, writes CHANGELOG.md and the READMEs, merges "Kumi X.Y.Z (#N)", tags `vX.Y.Z` and creates a draft release.
- The tag's Installer run builds the six bundles (macOS, Linux, Windows; Intel and ARM) with the `release` profile, tests installing, updating and migrating, attaches them and publishes the release. Nobody gets it before then.
- If that run fails, the release stays a draft: fix main and release the next patch (1.21.1 did this for 1.21.0). Never move, delete or re-push a `v*` tag.
- More: DEVELOPER_GUIDE.md "Releasing", DISTRIBUTION_POLICY.md.
