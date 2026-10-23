# Kumi: guide for agents

Kumi is a terminal agent that drives DAWs: Ableton Live, REAPER and Renoise (Logic Pro in progress, #374).
It's one Cargo workspace. Each DAW is reached through a bridge that runs inside or beside it.

## Where things live
- `crates/kumi`: the app. TUI (`src/tui/`), CLI, install/update/doctor; `src/daw/` wires each DAW in.
- `crates/kumi-runtime`: the agent, providers, listening, library, media and the DAW-neutral core.
  - `src/kernel/`: the agent loop. `src/core/`: the DAW-neutral model (HISTORY, plans, sessions, places, FOCUS
    views, and workflows such as jam, layers and stems).
  - `src/integrations/ableton/`: Live, host side. `own_*` and `shared/`: what REAPER and Renoise share.
  - `src/integrations/reaper*.rs`, `renoise*.rs`: REAPER and Renoise, host side.
- `daws/ableton/`: Live. `ableton-mcp-server/` is its bridge, an MCP server (`src/`: a module each for `protocol`,
  `host`, `live` with its `simulator/`, `lifecycle`, `analysis` and `project`); `remote-script/` runs inside Live
  (Python); `live-extension/` is Live's extension (TypeScript); `protocol/` is the only list of Live operations;
  `max-standard/` is the standard Kumi holds Max patches to (data Kumi builds in).
- `daws/own/`: Kumi's own bridges, for REAPER and Renoise: `daw-bridge/` (the `kumi-daw/1` wire) and
  `daw-mcp-server/` (the host; what's one DAW's alone is in its `src/reaper/` and `src/renoise/`).
- `daws/reaper/reaper-kumi`: REAPER's extension (Rust, built against REAPER's SDK headers in its `sdk/`).
- `daws/renoise/`: Renoise's tool, `com.kumi.Bridge.xrnx` (Lua), and its tests against a mock of Renoise's API.
- `crates/kumi-types`: what every layer names: DAW profiles and words, the contracts with the tools, errors, places,
  the system and the version. `kumi-runtime` re-exports it where its code always named it.
- `crates/kumi-store` (SQLite), `kumi-common` (shared utilities), `kumi-genexpr` (gen~ test interpreter).
- `plugin-formats/`: the plug-in preset formats Kumi reads (data Kumi builds in).
- `testdata/oracles/<crate>/`: the recorded oracles that crate's tests read, through `kumi_testkit::oracle`.
- `crates/kumi-testkit`: what tests share (a dev-dependency only): `workspace_path`, `chunked!`, `fixture_paths!`,
  `in_process!`. It depends on nothing: support that needs Kumi's crates is a macro the test's crate expands.
- `scripts/`: release and packaging (Python), the isolated test runners, and `dev.py` with `areas.toml` (what a change
  affects, which CI picks its jobs by). `install.sh` and `install.ps1`: the installers.
- `.config/nextest.toml`: CI's test sets (`ci`, `ci-os`, `extended`). `.github/workflows/`: CI.
- `docs/en`, `docs/ja`, `docs/zh-CN`: user docs. DEVELOPER_GUIDE.md and TESTING.md have the details.

## Commands
- Tests, in a home of their own (never your real ~/.kumi or Live folders):
  `sh scripts/test-isolated.sh --profile ci -p <crate>` (Windows: `scripts/test-isolated.ps1`)
  - Needs cargo-nextest: each test runs in a process of its own. `KUMI_NEXTEST=0` runs `cargo test` instead.
- One test file: `sh scripts/test-isolated.sh -p kumi-runtime -E 'test(/^<name>::/)'`. A crate's integration tests
  are one binary, `tests/it/`, with a module for each file (`tests/it/main.rs` lists them).
- Format and the gating lint:
  `cargo fmt --all --check`
  `cargo clippy --workspace --lib --bins --examples --locked -- -A clippy::all -D clippy::await_holding_refcell_ref`
- Before a push: `python3 scripts/dev.py check` runs Required CI's checks for what the branch changes (whitespace, fmt,
  the gating lint, the affected crates' `ci` tests, the changed areas' tests; `--dry-run` lists them).
- Release scripts' tests: `python3 -m unittest discover -s scripts/tests -p 'test_*release.py'`
- What a branch affects, and the CI jobs that test it: `python3 scripts/dev.py affected --base origin/main`
- A branch begun before a move: `python3 scripts/dev.py rebase-moves` (`scripts/moves.toml` lists the moves).
- Wait for CI: watch the CI run of the commit you pushed. Only "Required CI" (its last job) and "Willington files" are
  required. `gh pr checks --required --watch` can return early: "Required CI" is listed only once the jobs it needs end.
  `gh run watch --exit-status "$(gh run list -c "$(git rev-parse HEAD)" -w CI --json databaseId -q '.[0].databaseId')"`

## Searching
- `.ignore` hides the recorded oracles (JSON files of up to 30 MB, some on one line) and built bundles from search.
- Search them on purpose with `rg -u`, or `rg --no-ignore-dot`, which still skips what git ignores (`target/`).

## Rules
- Use worktrees: `python3 scripts/dev.py worktree new <name> --lean` makes `../kumi-<name>` from origin/main without
  the 98 MB of oracles only Extended reads. On long tasks, open a PR early and keep pushing at a regular cadence.
- Commit subjects are plain English and say what changed. No conventional-commit prefixes.
- Every PR body has `Changelog:` lines (`Changelog: none` for internal work); `scripts/release.py` reads them.
- A bridge or Remote Script change bumps `daws/ableton/ableton-mcp-server`'s version (DEVELOPER_GUIDE, "Releasing").
- A docs change goes into `docs/en`, `docs/ja` and `docs/zh-CN` in the same PR.
- Docs are concise and factual, bulleted where possible.
- Docs that aren't for users (design docs, agent notes, prompts, PRDs) stay out of git; `.hermes/` is ignored.
  AGENTS.md and CLAUDE.md are the exceptions.
- Never change binary names, the installed bundle layout or the wire protocols.
- Every tracked file belongs to a crate or an area in `scripts/areas.toml`; CI fails on one that doesn't.
- Never copy Live operation names or the registry hash out of `daws/ableton/protocol/`.
- `vendor/willington/` changes only in the owner's PRs from a `willington/` branch that change nothing else.
- Tests never need a running DAW, a sign-in or anything from this machine.

## Pins and oracles
- Goldens are byte-exact, and `KUMI_BLESS=1` rewrites them: the pins in `crates/kumi/tests/goldens` and
  `crates/kumi-runtime/tests/goldens`, the host's `daws/own/daw-mcp-server/tests/golden` and the wire vectors in
  `daws/own/daw-bridge/tests/vectors`. A refactor's proof is "goldens unchanged".
- Oracles (`testdata/oracles/<crate>/`) were recorded from the TypeScript version (tag `v1.7.6`). Nothing regenerates
  them. A lean worktree leaves out the ones only Extended's tests read (`testdata/oracles/extended-only.txt`).

## Glossary
- own bridge: Kumi's own bridge for REAPER, Renoise and Logic over `kumi-daw/1` (Live uses its MCP bridge).
- Hands: the helper that drives Live's menus and windows. Ears: how Kumi hears a DAW's audio.
- place: where a conversation, its notes and its jobs are kept (a project id, "unsaved", "reaper-<id>").
- FOCUS: what the producer is looking at (view, track, item, device), plus the transport.
- catch-up: what changed in the project since Kumi last looked (snapshots).
- kit: building blocks for Max for Live devices (`[kumi.transport]`, `[kumi.clock]`, `[kumi.notes]`).
- slots: which model does each listening job. Willington: optional native Live providers (`vendor/willington/`).
