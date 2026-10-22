//! What Kumi's crates' tests share. A dev-dependency only: nothing Kumi ships links it, so it may name paths in the
//! checkout that built it.
use std::path::{Path, PathBuf};

/// This checkout's root, as `.cargo/config.toml` sets `KUMI_WORKSPACE`.
pub fn workspace() -> PathBuf {
    // Cargo gives `<checkout>/.`: its components drop the `.`.
    Path::new(env!("KUMI_WORKSPACE", "run cargo from inside the checkout: its .cargo/config.toml sets KUMI_WORKSPACE"))
        .components()
        .collect()
}

/// A path in this checkout, from its root: `workspace_path("protocol/ableton-live-v1.operations.json")`.
pub fn workspace_path(relative: impl AsRef<Path>) -> PathBuf {
    workspace().join(relative)
}
