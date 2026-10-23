//! The libraries of the other DAWs Kumi works in, beside Live's: REAPER's resource folder (its FX chains, track and
//! project templates, JSFX and FX preset banks) and Renoise's own Library and User Library (instruments, multi-samples,
//! effect chains and presets, phrases, phrase scripts, modulation sets and samples). Found where daw-bridge's detection
//! finds each DAW, and only for a DAW whose Kumi bridge is installed there: a producer who has Renoise but never works in
//! it with Kumi doesn't wait while its factory sounds are measured.
//!
//! What a library holds is read by the DAW's own layout: its top folders say what the files in them are.

use std::path::PathBuf;

use daw_bridge::detect::{detect_reaper, detect_renoise, Env, Os};
use daw_bridge::install::{reaper_extension_name, RENOISE_TOOL_FOLDER};

use super::sources::{below, env_of, home_of, is_folder, join, list, on_disk, platform_of, resolve, Source, SourceKind, SourceOptions};
use crate::integrations::shared::samples::SAMPLE_EXTENSIONS;

/// What a file in a library is to Kumi.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Sound,
    Preset,
}

/// REAPER's resource folders Kumi reads, and the file type each holds ("" is a JSFX: its file has no extension).
const REAPER_FOLDERS: &[(&str, &[&str])] = &[
    ("fxchains", &[".rfxchain"]),
    ("tracktemplates", &[".rtracktemplate"]),
    ("projecttemplates", &[".rpp"]),
    ("effects", &["", ".jsfx"]),
    ("presets", &[".ini"]),
];

/// Renoise's library folders Kumi reads (its Songs are the producer's songs, not presets).
const RENOISE_FOLDERS: &[(&str, &[&str])] = &[
    ("instruments", &[".xrni"]),
    ("multi-samples", &[".sfz"]),
    ("effect chains", &[".xrnt"]),
    ("effect presets", &[".xrdp"]),
    ("phrases", &[".xrnz"]),
    ("phrase scripts", &[".lua"]),
    ("modulation sets", &[".xrno"]),
];

/// Renoise's folders whose audio files are sounds.
const RENOISE_SOUNDS: &[&str] = &["samples", "multi-samples"];

fn folders_of(source: &Source) -> &'static [(&'static str, &'static [&'static str])] {
    match source.kind {
        SourceKind::Reaper => REAPER_FOLDERS,
        SourceKind::Renoise => RENOISE_FOLDERS,
        _ => &[],
    }
}

/// Whether a walk of a DAW's library goes into this top folder (by name, as the DAW makes it, in any case).
pub fn walks_into(source: &Source, top: &str) -> bool {
    let top = top.to_lowercase();
    folders_of(source).iter().any(|(folder, _)| *folder == top)
        || (source.kind == SourceKind::Renoise && RENOISE_SOUNDS.contains(&top.as_str()))
}

/// Whether a file is one a DAW library's own walk reads: below the library, in a top folder its layout names. The other
/// files there (a User Library's own "Kits" folder) belong to the folder around it, a Live Place or one the producer named,
/// as they did before the DAW's library was a source of its own.
pub fn owns(source: &Source, path: &str) -> bool {
    let Some(relative) = below(path, &source.path) else { return false };
    let mut parts = relative.split(['\\', '/']).filter(|part| !part.is_empty());
    match (parts.next(), parts.next()) {
        (Some(top), Some(_)) => walks_into(source, top),
        _ => false,
    }
}

/// The folders of a DAW library its own walk reads, as the disk spells them: what a walk of a folder around it leaves out.
pub fn owned_folders(source: &Source) -> Vec<String> {
    list(&source.path).into_iter().filter(|name| walks_into(source, name)).map(|name| join(&source.path, &name)).collect()
}

/// What a file in a DAW's library is, by its top folder and its extension (lower case, with its dot); None when Kumi
/// doesn't learn it.
pub fn role_of(source: &Source, top: &str, extension: &str) -> Option<Role> {
    let top = top.to_lowercase();
    if source.kind == SourceKind::Renoise && RENOISE_SOUNDS.contains(&top.as_str()) && SAMPLE_EXTENSIONS.contains(&extension) {
        return Some(Role::Sound);
    }
    folders_of(source)
        .iter()
        .find(|(folder, _)| *folder == top)
        .filter(|(_, extensions)| extensions.contains(&extension))
        .map(|_| Role::Preset)
}

/// A preset's format as the library keeps it: its extension, but "jsfx" for any JSFX (most have none).
pub fn preset_format(source: &Source, top: &str, extension: &str) -> String {
    if source.kind == SourceKind::Reaper && top.eq_ignore_ascii_case("effects") {
        return "jsfx".into();
    }
    extension.trim_start_matches('.').to_lowercase()
}

fn os_of(options: &SourceOptions) -> Os {
    match platform_of(options).as_str() {
        "darwin" => Os::Mac,
        "win32" => Os::Windows,
        _ => Os::Linux,
    }
}

fn source(path: PathBuf, label: String, kind: SourceKind) -> Source {
    Source { path: on_disk(&resolve(&path.to_string_lossy())), label, kind }
}

/// REAPER's resource folders with Kumi's extension in them (the default one, portable installs, or the one
/// `KUMI_REAPER_RESOURCE_DIR` names), and Renoise's libraries for the newest version Kumi's tool is in.
pub fn daw_library_sources(options: &SourceOptions) -> Vec<Source> {
    let os = os_of(options);
    let home = PathBuf::from(home_of(options));
    let var = |name: &str| env_of(options, name);
    let env = Env { os, home: home.clone(), var: &var, root: None };
    let mut found = Vec::new();
    let reapers: Vec<PathBuf> = detect_reaper(&env)
        .into_iter()
        .map(|install| install.resource)
        .filter(|resource| resource.join("UserPlugins").join(reaper_extension_name(os)).is_file())
        .collect();
    for resource in &reapers {
        let label = if reapers.len() == 1 {
            "REAPER".to_string()
        } else {
            format!("REAPER ({})", resource.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
        };
        found.push(source(resource.clone(), label, SourceKind::Reaper));
    }
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
