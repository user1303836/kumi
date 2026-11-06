//! Where Renoise keeps its library, for the library to learn: its own Library and User Library, found where daw-bridge's
//! detection finds Renoise, and only for a version Kumi's tool is in (a producer who has Renoise but never works in it with
//! Kumi doesn't wait while its factory sounds are measured), and how it's laid out: instruments, multi-samples, effect
//! chains and presets, phrases, phrase scripts, modulation sets and samples.

use std::path::{Path, PathBuf};

use daw_bridge::detect::{detect_renoise, renoise_user_libraries, user_libraries_in, Env, Os};
use daw_bridge::install::RENOISE_TOOL_FOLDER;
use kumi_daw_own::library::{os_of, source};
use kumi_library::daws::{Layout, LibrarySources, ProjectFormat};
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
            // An injected home/platform never asks this machine for its Documents folder.
            let candidates = if options.home.is_some() || os != Os::current() {
                user_libraries_in(os, None, &home)
            } else {
                renoise_user_libraries(&home)
            };
            found.extend(user_sources(os, candidates));
        }
        found
    }

    fn layout(&self) -> &'static Layout {
        &LAYOUT
    }

    fn projects(&self) -> Option<&'static ProjectFormat> {
        Some(&crate::songs::FORMAT)
    }
}

/// Linux keeps both its documented library and the legacy Documents library available.
/// Other platforms retain their existing first-existing-library behavior.
fn user_sources(os: Os, candidates: Vec<PathBuf>) -> Vec<Source> {
    user_sources_with_identity(os, candidates, |path| std::fs::canonicalize(path).ok())
}

fn user_sources_with_identity(os: Os, candidates: Vec<PathBuf>, mut identity: impl FnMut(&Path) -> Option<PathBuf>) -> Vec<Source> {
    let mut found = Vec::new();
    let mut identities = Vec::new();
    for path in candidates.into_iter().filter(|candidate| is_folder(&candidate.to_string_lossy())) {
        if os == Os::Linux {
            if let Some(real) = identity(&path) {
                if identities.contains(&real) {
                    continue;
                }
                identities.push(real);
            }
            // A failed identity read proves no alias: keep this root rather than silently
            // hiding a distinct library. Canonical paths are never used as outward names.
        }
        found.push(source(path, "Renoise User Library".into(), SourceKind::Daw(DawId::Renoise)));
        if os != Os::Linux {
            break;
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_learning_keeps_both_existing_libraries_in_documented_order() {
        for (native_exists, legacy_exists) in [(false, false), (false, true), (true, false), (true, true)] {
            let home = tempfile::tempdir().unwrap();
            let root = home.path().canonicalize().unwrap();
            let candidates = user_libraries_in(Os::Linux, None, &root);
            let mut expected = vec![];
            for (exists, path) in [(native_exists, &candidates[0]), (legacy_exists, &candidates[1])] {
                if exists {
                    std::fs::create_dir_all(path).unwrap();
                    expected.push(path.to_string_lossy().into_owned());
                }
            }
            let found = user_sources(Os::Linux, candidates);
            assert_eq!(found.iter().map(|source| source.path.clone()).collect::<Vec<_>>(), expected);
            assert!(found.iter().all(|source| source.kind == SourceKind::Daw(DawId::Renoise)));
        }
    }

    #[cfg(unix)]
    #[test]
    fn linux_library_aliases_are_learned_once_under_the_documented_path() {
        for documented_is_link in [false, true] {
            let home = tempfile::tempdir().unwrap();
            let root = home.path().canonicalize().unwrap();
            let candidates = user_libraries_in(Os::Linux, None, &root);
            let (documented, legacy) = (&candidates[0], &candidates[1]);
            let (link, target) = if documented_is_link { (documented, legacy) } else { (legacy, documented) };
            std::fs::create_dir_all(target.join("Samples")).unwrap();
            std::fs::create_dir_all(link.parent().unwrap()).unwrap();
            std::os::unix::fs::symlink(target, link).unwrap();
            std::fs::write(target.join("Samples/Kept.wav"), b"producer sample").unwrap();
            assert_eq!(documented.canonicalize().unwrap(), legacy.canonicalize().unwrap());
            let found = user_sources(Os::Linux, candidates.clone());
            assert_eq!(found.len(), 1, "two names for one library must not scan its sounds twice");
            assert_eq!(found[0].path, documented.to_string_lossy(), "keep the preferred outward path even when it is the link");
            assert_eq!(std::fs::read_link(link).unwrap(), *target, "source discovery cannot move or rewrite either root");
            assert_eq!(std::fs::read(target.join("Samples/Kept.wav")).unwrap(), b"producer sample");
        }
    }

    #[test]
    fn unreadable_library_identities_do_not_hide_existing_roots() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().canonicalize().unwrap();
        let candidates = user_libraries_in(Os::Linux, None, &root);
        for path in &candidates {
            std::fs::create_dir_all(path).unwrap();
        }
        for unreadable in [0, 1, 2] {
            let mut next = 0;
            let found = user_sources_with_identity(Os::Linux, candidates.clone(), |path| {
                let index = next;
                next += 1;
                if unreadable == 2 || index == unreadable {
                    None
                } else {
                    path.canonicalize().ok()
                }
            });
            assert_eq!(
                found.iter().map(|source| source.path.clone()).collect::<Vec<_>>(),
                candidates.iter().map(|path| path.to_string_lossy().into_owned()).collect::<Vec<_>>(),
                "an unresolved root cannot be collapsed into another one"
            );
        }
    }

    #[test]
    fn mac_and_windows_learning_still_choose_the_first_existing_library() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().canonicalize().unwrap();
        let (moved, in_home) = (root.join("OneDrive"), root.join("Documents"));
        for os in [Os::Mac, Os::Windows] {
            assert!(user_sources(os, vec![moved.clone(), in_home.clone()]).is_empty());
        }
        std::fs::create_dir(&in_home).unwrap();
        for os in [Os::Mac, Os::Windows] {
            assert_eq!(user_sources(os, vec![moved.clone(), in_home.clone()])[0].path, in_home.to_string_lossy());
        }
        std::fs::create_dir(&moved).unwrap();
        for os in [Os::Mac, Os::Windows] {
            let found = user_sources(os, vec![moved.clone(), in_home.clone()]);
            assert_eq!(found.len(), 1);
            assert_eq!(found[0].path, moved.to_string_lossy());
        }
    }
}
