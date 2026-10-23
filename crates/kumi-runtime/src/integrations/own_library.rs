//! What REAPER's and Renoise's libraries (kumi-daw-reaper's and kumi-daw-renoise's) share: each is found where
//! daw-bridge's detection finds its DAW, and only for a DAW whose Kumi bridge is installed there. How each library is
//! read, by its layout, is the library's own (`kumi_library::daw_sources`).

use std::path::PathBuf;

use daw_bridge::detect::Os;
use kumi_library::sources::{on_disk, platform_of, resolve, Source, SourceKind, SourceOptions};

/// daw-bridge's name for the options' system.
pub fn os_of(options: &SourceOptions) -> Os {
    match platform_of(options).as_str() {
        "darwin" => Os::Mac,
        "win32" => Os::Windows,
        _ => Os::Linux,
    }
}

/// A DAW's library at `path`, as the library keeps it (resolved, spelled as the disk spells it).
pub fn source(path: PathBuf, label: String, kind: SourceKind) -> Source {
    Source { path: on_disk(&resolve(&path.to_string_lossy())), label, kind }
}
