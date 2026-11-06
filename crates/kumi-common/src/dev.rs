//! What a development build reaches for in the checkout it was built from: the Live extension, the own bridges'
//! builds, the release script. A release, built elsewhere, finds none.
use std::path::{Path, PathBuf};

/// The checkout this build was made from, while it's there: the workspace `.cargo/config.toml` names, whose
/// Cargo.toml still holds the workspace. This explicit environment input follows a build between worktrees even when
/// they share a target folder; Cargo can reuse a dependency built with another worktree's CARGO_MANIFEST_DIR.
pub fn checkout() -> Option<PathBuf> {
    let folder = Path::new(env!("KUMI_WORKSPACE", "run cargo from inside the checkout: its .cargo/config.toml sets KUMI_WORKSPACE"));
    std::fs::read_to_string(folder.join("Cargo.toml"))
        .is_ok_and(|manifest| manifest.contains("[workspace]"))
        .then(|| folder.components().collect())
}
