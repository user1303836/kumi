//! Where Renoise keeps its library, for the library to learn: its own Library and User Library, found where daw-bridge's
//! detection finds Renoise, and only for a version Kumi's tool is in (a producer who has Renoise but never works in it with
//! Kumi doesn't wait while its factory sounds are measured), and how it's laid out: instruments, multi-samples, effect
//! chains and presets, phrases, phrase scripts, modulation sets and samples.

use std::path::PathBuf;

use daw_bridge::detect::{detect_renoise, Env, Os};
use daw_bridge::install::RENOISE_TOOL_FOLDER;
use kumi_daw_own::library::{os_of, source};
use kumi_library::daws::{Layout, LibrarySources};
use kumi_library::sources::{env_of, home_of, is_folder, join, Source, SourceKind, SourceOptions};
use kumi_types::daw::{DawId, DawProfile};

/// Renoise's library: its own Library and User Library, for the newest version Kumi's tool is in.
pub struct RenoiseLibrary;

/// Renoise's library, as the registry gives it to the library.
pub static LIBRARY: RenoiseLibrary = RenoiseLibrary;

/// Renoise's library folders Kumi reads (its Songs are the producer's songs, not presets), and the folders whose audio
/// files are sounds.
static LAYOUT: Layout = Layout {
    folders: &[
        ("instruments", &[".xrni"]),
        ("multi-samples", &[".sfz"]),
        ("effect chains", &[".xrnt"]),
        ("effect presets", &[".xrdp"]),
        ("phrases", &[".xrnz"]),
        ("phrase scripts", &[".lua"]),
        ("modulation sets", &[".xrno"]),
    ],
    sounds: &["samples", "multi-samples"],
    formats: &[],
};

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
                found.push(source(library, "Renoise Library".into(), SourceKind::Daw(DawId::Renoise)));
            }
            let user = PathBuf::from(join(&join(&join(&home.to_string_lossy(), "Documents"), "Renoise"), "User Library"));
            if is_folder(&user.to_string_lossy()) {
                found.push(source(user, "Renoise User Library".into(), SourceKind::Daw(DawId::Renoise)));
            }
        }
        found
    }

    fn layout(&self) -> &'static Layout {
        &LAYOUT
    }
}
