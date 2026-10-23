//! What Kumi's crates' tests share. A dev-dependency only: nothing Kumi ships links it, so it may name paths in the
//! checkout that built it.
//!
//! - `workspace_path`: a file in this checkout, named from its root.
//! - `oracle`, `oracle_bytes`, `oracle_path`: a recorded oracle in `testdata/oracles/<crate>/`.
//! - `chunked!`: a fixture's cases as several tests that nextest runs side by side.
//! - `fixture_paths!`: recorded paths made native (separators, roots, Windows' verbatim prefix).
//! - `in_process!`: Kumi's host as an MCP endpoint called in process, for the own DAWs' end-to-end tests.
//!
//! kumi-testkit depends on nothing. The bridge's and REAPER's extension's tests use it, and release.py reads their
//! dependency graphs, test-only dependencies included. Support that needs other crates is a macro that declares its
//! module in the test's crate, which supplies them.
use std::path::{Path, PathBuf};

mod chunks;
mod fixture_paths;
mod in_process;

/// This checkout's root, as `.cargo/config.toml` sets `KUMI_WORKSPACE`.
pub fn workspace() -> PathBuf {
    // Cargo gives `<checkout>/.`: its components drop the `.`.
    Path::new(env!("KUMI_WORKSPACE", "run cargo from inside the checkout: its .cargo/config.toml sets KUMI_WORKSPACE"))
        .components()
        .collect()
}

/// A path in this checkout, from its root: `workspace_path("daws/ableton/protocol/ableton-live-v1.operations.json")`.
pub fn workspace_path(relative: impl AsRef<Path>) -> PathBuf {
    workspace().join(relative)
}

/// A recorded oracle, by its crate's folder and its name: `oracle_path("kumi-daw-ableton/views-oracle.json")` is
/// `testdata/oracles/kumi-daw-ableton/views-oracle.json` in this checkout.
pub fn oracle_path(name: &str) -> PathBuf {
    workspace_path("testdata/oracles").join(name)
}

/// A recorded oracle's text (see [`oracle_path`]).
pub fn oracle(name: &str) -> String {
    String::from_utf8(oracle_bytes(name)).unwrap_or_else(|_| panic!("{} isn't UTF-8", oracle_path(name).display()))
}

/// A recorded oracle's bytes (see [`oracle_path`]), for the gzipped ones.
pub fn oracle_bytes(name: &str) -> Vec<u8> {
    let path = oracle_path(name);
    std::fs::read(&path).unwrap_or_else(|error| {
        panic!(
            "{}: {error}. A lean worktree (dev.py worktree new --lean) leaves out the oracles only Extended's tests read; \
             `git sparse-checkout disable` brings them back",
            path.display()
        )
    })
}
