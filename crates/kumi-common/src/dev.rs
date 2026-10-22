//! What a development build reaches for in the checkout it was built from: the Live extension, the own bridges'
//! builds, the release script. A release, built elsewhere, finds none.
use std::path::{Path, PathBuf};

/// The checkout this build was made from, while it's there: the nearest folder above this crate's whose Cargo.toml
/// holds the workspace.
pub fn checkout() -> Option<PathBuf> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .skip(1)
        .find(|folder| std::fs::read_to_string(folder.join("Cargo.toml")).is_ok_and(|manifest| manifest.contains("[workspace]")))
        .map(Path::to_path_buf)
}
