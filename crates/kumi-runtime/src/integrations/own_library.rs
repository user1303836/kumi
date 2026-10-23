//! Where Renoise keeps its library, for the library to learn, and what REAPER's (kumi-daw-reaper's) shares with it:
//! found where daw-bridge's detection finds each DAW, and only for a DAW whose Kumi bridge is installed there. A producer
//! who has Renoise but never works in it with Kumi doesn't wait while its factory sounds are measured. How each library
//! is read, by its layout, is the library's own (`kumi_library::daw_sources`).

use std::path::PathBuf;

use daw_bridge::detect::{detect_renoise, Env, Os};
use daw_bridge::install::RENOISE_TOOL_FOLDER;
use kumi_library::daws::LibrarySources;
use kumi_library::sources::{env_of, home_of, is_folder, join, on_disk, platform_of, resolve, Source, SourceKind, SourceOptions};

use crate::core::daw::DawProfile;

/// Renoise's library: its own Library and User Library, for the newest version Kumi's tool is in.
pub struct RenoiseLibrary;

pub static RENOISE: RenoiseLibrary = RenoiseLibrary;

impl LibrarySources for RenoiseLibrary {
    fn profile(&self) -> &'static DawProfile {
        &DawProfile::RENOISE
    }

    /// Renoise's libraries for the newest version Kumi's tool is in.
    fn own_library(&self, options: &SourceOptions) -> Vec<Source> {
        let os = os_of(options);
        let home = PathBuf::from(home_of(options));
        let var = |name: &str| env_of(options, name);
        let env = Env { os, home: home.clone(), var: &var, root: None };
        let mut found = Vec::new();
        let renoise = detect_renoise(&env);
        if let Some(target) = renoise.target.filter(|target| target.tools().join(RENOISE_TOOL_FOLDER).is_dir()) {
            // Renoise copies its own library out of the app on its first run: into the version folder, or (macOS) the
            // version's folder in Application Support (3.5.4, seen on this Mac).
            let version = target.dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let mut own = vec![target.dir.join("Library")];
            if os == Os::Mac {
                own.push(home.join("Library").join("Application Support").join("Renoise").join(&version).join("Library"));
            }
            if let Some(library) = own.into_iter().find(|path| path.is_dir()) {
                found.push(source(library, "Renoise Library".into(), SourceKind::Renoise));
            }
            let user = PathBuf::from(join(&join(&join(&home.to_string_lossy(), "Documents"), "Renoise"), "User Library"));
            if is_folder(&user.to_string_lossy()) {
                found.push(source(user, "Renoise User Library".into(), SourceKind::Renoise));
            }
        }
        found
    }
}

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
