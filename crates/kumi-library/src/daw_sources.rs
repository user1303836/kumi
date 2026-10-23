//! The libraries of the other DAWs Kumi works in, beside Live's: REAPER's resource folder (its FX chains, track and
//! project templates, JSFX and FX preset banks) and Renoise's own Library and User Library (instruments, multi-samples,
//! effect chains and presets, phrases, phrase scripts, modulation sets and samples). Each DAW's integration says where
//! its library is ([`LibrarySources::own_library`](crate::daws::LibrarySources::own_library)).
//!
//! What a library holds is read by the DAW's own layout: its top folders say what the files in them are.

use super::sources::{below, join, list, Source, SourceKind};
use crate::samples::SAMPLE_EXTENSIONS;

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
