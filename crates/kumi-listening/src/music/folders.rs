//! Where the producer's music is: the usual places, found and counted quickly for the offer (nothing in them is read
//! until the producer agrees), and the walk over the folders they agreed to.

use kumi_library::sources::{below, expand_folder, homedir, normalize, on_disk};
use kumi_media::audio::decode::AUDIO_EXTENSIONS;
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

/// Songs counted at most when looking for them: past this, "about a million" says enough.
pub const MOST_COUNTED: usize = 1_000_000;

/// Apple's protected purchases: known by their tags, never heard.
pub const PROTECTED: [&str; 1] = [".m4p"];

/// The files of any DAW's projects: a folder that holds one is a project's (its stems, recordings and samples), not
/// the producer's songs.
const PROJECT_FILES: [&str; 17] = [
    ".als",
    ".rpp",
    ".xrns",
    ".flp",
    ".cpr",
    ".npr",
    ".song",
    ".ptx",
    ".ptf",
    ".bwproject",
    ".dawproject",
    ".aup3",
    ".aup",
    ".ardour",
    ".reason",
    ".mmpz",
    ".logic",
];
/// DAW projects kept as folders of their own (Logic's, GarageBand's).
const PROJECT_BUNDLES: [&str; 2] = [".logicx", ".band"];

/// A file's extension, lower case with its dot, as AUDIO_EXTENSIONS spells them.
fn extension(name: &str) -> String {
    name.rfind('.').map(|at| name[at..].to_lowercase()).unwrap_or_default()
}

/// Whether a file is a song Kumi can hear: an audio file it decodes, less .mp4 (more often a video).
pub fn is_song(name: &str) -> bool {
    let extension = extension(name);
    extension != ".mp4" && AUDIO_EXTENSIONS.contains(&extension.as_str())
}

fn is_protected(name: &str) -> bool {
    PROTECTED.contains(&extension(name).as_str())
}

/// Folders and files left out of every walk: hidden ones (and macOS's `._` copies on other disks) and Windows' own.
fn hidden(name: &str) -> bool {
    name.starts_with('.') || name.eq_ignore_ascii_case("$RECYCLE.BIN") || name.eq_ignore_ascii_case("System Volume Information")
}

/// The usual places for a producer's music, best first: their music app's own folder before the Music folder around it.
pub fn usual_places() -> Vec<PathBuf> {
    let home = PathBuf::from(homedir());
    if cfg!(target_os = "macos") {
        vec![
            home.join("Music").join("Music").join("Media.localized").join("Music"),
            home.join("Music").join("iTunes").join("iTunes Media").join("Music"),
            home.join("Music"),
        ]
    } else if cfg!(windows) {
        vec![windows_music().unwrap_or_else(|| home.join("Music"))]
    } else {
        vec![home.join("Music")]
    }
}

/// Windows' Music folder, wherever the producer moved it (OneDrive, another drive).
#[cfg(windows)]
fn windows_music() -> Option<PathBuf> {
    use std::{ffi::c_void, os::windows::ffi::OsStringExt};
    #[repr(C)]
    struct Guid(u32, u16, u16, [u8; 8]);
    // FOLDERID_Music: {4BD8D571-6D19-48D3-BE97-422220080E43}.
    const MUSIC: Guid = Guid(0x4BD8D571, 0x6D19, 0x48D3, [0xBE, 0x97, 0x42, 0x22, 0x20, 0x08, 0x0E, 0x43]);
    #[link(name = "shell32")]
    unsafe extern "system" {
        fn SHGetKnownFolderPath(id: *const Guid, flags: u32, token: *mut c_void, path: *mut *mut u16) -> i32;
    }
    #[link(name = "ole32")]
    unsafe extern "system" {
        fn CoTaskMemFree(memory: *mut c_void);
    }
    let mut path: *mut u16 = std::ptr::null_mut();
    let result = unsafe { SHGetKnownFolderPath(&MUSIC, 0, std::ptr::null_mut(), &mut path) };
    let found = (result == 0 && !path.is_null()).then(|| {
        let length = (0..).take_while(|&at| unsafe { *path.add(at) } != 0).count();
        PathBuf::from(std::ffi::OsString::from_wide(unsafe { std::slice::from_raw_parts(path, length) }))
    });
    if !path.is_null() {
        unsafe { CoTaskMemFree(path.cast()) };
    }
    found
}

#[cfg(not(windows))]
fn windows_music() -> Option<PathBuf> {
    None
}

/// A usual place that holds songs, and about how many (`capped` past `MOST_COUNTED`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub folder: String,
    pub songs: usize,
    pub capped: bool,
}

/// The first usual place that holds songs, counted with the walk's own rules (not `skip`'s folders).
pub fn found(skip: &[String]) -> Option<Found> {
    found_in(usual_places(), skip)
}

/// The first of `places` that holds songs.
pub fn found_in(places: Vec<PathBuf>, skip: &[String]) -> Option<Found> {
    places.into_iter().filter(|place| place.is_dir()).find_map(|place| {
        let folder = place.to_str()?.to_string();
        let mut songs = 0;
        walk(std::slice::from_ref(&folder), skip, &mut |song| {
            songs += usize::from(!song.protected);
            songs < MOST_COUNTED
        });
        (songs > 0).then_some(Found { folder, songs, capped: songs >= MOST_COUNTED })
    })
}

/// A file the walk found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Walked {
    pub path: String,
    /// The music folder it's under (one of the folders walked).
    pub folder: String,
    pub size: u64,
    /// When it last changed, ms.
    pub mtime: i64,
    /// One Kumi can name but never hear (`PROTECTED`).
    pub protected: bool,
}

/// The folders a walk reads: each as the disk spells it, with `~` read, and none inside another (its songs are the
/// outer one's).
pub fn folders_to_walk(folders: &[String]) -> Vec<String> {
    let home = homedir();
    let mut kept: Vec<String> = vec![];
    for folder in folders.iter().filter_map(|folder| expand_folder(folder, &home)).map(|folder| on_disk(&normalize(&folder))) {
        let folder = folder.trim_end_matches(['/', '\\']).to_string();
        let folder = if folder.is_empty() { normalize("/") } else { folder };
        if kept.iter().any(|outer| same_or_below(&folder, outer)) {
            continue;
        }
        kept.retain(|inner| !same_or_below(inner, &folder));
        kept.push(folder);
    }
    kept
}

/// A path as compared: in one case where the system doesn't tell cases apart (Windows, a Mac).
fn fold(text: &str) -> String {
    if cfg!(any(windows, target_os = "macos")) {
        text.to_lowercase()
    } else {
        text.to_string()
    }
}

/// Whether `path` is `folder` or inside it.
fn same_or_below(path: &str, folder: &str) -> bool {
    let (path, folder) = (fold(path), fold(folder));
    path == folder || below(&path, &folder).is_some()
}

/// Every song file under `folders`, each told to `visit` (false stops the walk), and whether each folder could be read
/// at all (one that couldn't, a drive that isn't there now, has gone nowhere: its songs aren't marked gone).
///
/// Left out: hidden folders and files, the folders in `skip` that lie inside a folder walked (the producer's sample
/// folders, the library's places: a skip around a music folder doesn't hide it), DAW projects' folders, and any folder
/// reached a second time (a link back up the tree).
pub fn walk(folders: &[String], skip: &[String], visit: &mut dyn FnMut(Walked) -> bool) -> Vec<bool> {
    let home = homedir();
    let skip: Vec<String> = skip
        .iter()
        .filter_map(|folder| expand_folder(folder, &home))
        .map(|folder| on_disk(&normalize(&folder)).trim_end_matches(['/', '\\']).to_string())
        .filter(|folder| !folder.is_empty())
        .collect();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut readable = vec![];
    for root in folders {
        // Only the skips inside this folder count here.
        let inside: Vec<&String> = skip.iter().filter(|folder| below(&fold(folder), &fold(root)).is_some()).collect();
        let Ok(canonical) = std::fs::canonicalize(root) else {
            readable.push(false);
            continue;
        };
        readable.push(std::fs::read_dir(root).is_ok());
        let mut stack = vec![(PathBuf::from(root), canonical)];
        while let Some((folder, canonical)) = stack.pop() {
            if !seen.insert(canonical) {
                continue;
            }
            let Ok(entries) = std::fs::read_dir(&folder) else { continue };
            let entries: Vec<_> = entries.flatten().collect();
            let names: Vec<String> = entries.iter().map(|entry| entry.file_name().to_string_lossy().into_owned()).collect();
            // A DAW project's folder: what's in it is the project's (but a folder the producer named is walked).
            if folder != Path::new(root) && names.iter().any(|name| PROJECT_FILES.contains(&extension(name).as_str())) {
                continue;
            }
            let mut folders_here = vec![];
            for (entry, name) in entries.iter().zip(&names) {
                if hidden(name) {
                    continue;
                }
                let path = entry.path();
                let Ok(kind) = entry.file_type() else { continue };
                // A link is read as what it points to.
                let meta = if kind.is_symlink() { std::fs::metadata(&path) } else { entry.metadata() };
                let Ok(meta) = meta else { continue };
                if meta.is_dir() {
                    let text = path.to_string_lossy();
                    if PROJECT_BUNDLES.contains(&extension(name).as_str()) || inside.iter().any(|skipped| same_or_below(&text, skipped)) {
                        continue;
                    }
                    if let Ok(canonical) = std::fs::canonicalize(&path) {
                        folders_here.push((path, canonical));
                    }
                } else if meta.is_file() && (is_song(name) || is_protected(name)) {
                    let Some(text) = path.to_str() else { continue };
                    let mtime = meta
                        .modified()
                        .ok()
                        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                        .map_or(0, |since| since.as_millis() as i64);
                    let song =
                        Walked { path: text.to_string(), folder: root.clone(), size: meta.len(), mtime, protected: is_protected(name) };
                    if !visit(song) {
                        return readable;
                    }
                }
            }
            // Depth first, in name order.
            folders_here.sort_by(|a, b| b.0.cmp(&a.0));
            stack.extend(folders_here);
        }
    }
    readable
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn songs_are_audio_files_but_videos() {
        assert!(is_song("Archangel.MP3"));
        assert!(is_song("Archangel.flac"));
        assert!(!is_song("Archangel.mp4"));
        assert!(!is_song("cover.jpg"));
        assert!(!is_song("Archangel.m4p"));
        assert!(is_protected("Archangel.m4p"));
    }

    #[test]
    fn nested_folders_are_walked_once() {
        let folders = folders_to_walk(&["/m/a/b".into(), "/m/a".into(), "/n".into(), "/m/a/".into()]);
        assert_eq!(folders, ["/m/a", "/n"]);
    }
}
