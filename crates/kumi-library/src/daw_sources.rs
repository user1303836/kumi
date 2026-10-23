//! A DAW's own library, beside the folders Live's layout and the producer name: REAPER's resource folder, Renoise's own
//! Library and User Library. Each DAW's integration says where its library is
//! ([`LibrarySources::own_library`](crate::daws::LibrarySources::own_library)) and how it's laid out
//! ([`LibrarySources::layout`](crate::daws::LibrarySources::layout)).
//!
//! What a library holds is read by the DAW's own layout: its top folders say what the files in them are.

use super::sources::{below, join, list, Source};
use crate::daws::Layout;
use crate::samples::SAMPLE_EXTENSIONS;

/// What a file in a library is to Kumi.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Sound,
    Preset,
}

/// Whether a walk of a DAW's library goes into this top folder (by name, as the DAW makes it, in any case).
pub fn walks_into(layout: &Layout, top: &str) -> bool {
    let top = top.to_lowercase();
    layout.folders.iter().any(|(folder, _)| *folder == top) || layout.sounds.contains(&top.as_str())
}

/// Whether a file is one a DAW library's own walk reads: below the library, in a top folder its layout names. The other
/// files there (a User Library's own "Kits" folder) belong to the folder around it, a Live Place or one the producer named,
/// as they did before the DAW's library was a source of its own.
pub fn owns(source: &Source, layout: &Layout, path: &str) -> bool {
    let Some(relative) = below(path, &source.path) else { return false };
    let mut parts = relative.split(['\\', '/']).filter(|part| !part.is_empty());
    match (parts.next(), parts.next()) {
        (Some(top), Some(_)) => walks_into(layout, top),
        _ => false,
    }
}

/// The folders of a DAW library its own walk reads, as the disk spells them: what a walk of a folder around it leaves out.
pub fn owned_folders(source: &Source, layout: &Layout) -> Vec<String> {
    list(&source.path).into_iter().filter(|name| walks_into(layout, name)).map(|name| join(&source.path, &name)).collect()
}

/// What a file in a DAW's library is, by its top folder and its extension (lower case, with its dot); None when Kumi
/// doesn't learn it.
pub fn role_of(layout: &Layout, top: &str, extension: &str) -> Option<Role> {
    let top = top.to_lowercase();
    if layout.sounds.contains(&top.as_str()) && SAMPLE_EXTENSIONS.contains(&extension) {
        return Some(Role::Sound);
    }
    layout.folders.iter().find(|(folder, _)| *folder == top).filter(|(_, extensions)| extensions.contains(&extension)).map(|_| Role::Preset)
}

/// A preset's format as the library keeps it: its extension, but its top folder's own where the layout gives one ("jsfx"
/// for any JSFX, most of which have no extension).
pub fn preset_format(layout: &Layout, top: &str, extension: &str) -> String {
    if let Some((_, format)) = layout.formats.iter().find(|(folder, _)| top.eq_ignore_ascii_case(folder)) {
        return (*format).into();
    }
    extension.trim_start_matches('.').to_lowercase()
}
