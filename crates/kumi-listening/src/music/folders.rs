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

/// A usual place that holds songs, and about how many (`capped` past `MOST_COUNTED`), `cloud` of them only in the cloud.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Found {
    pub folder: String,
    pub songs: usize,
    pub capped: bool,
    pub cloud: usize,
}

/// The first usual place that holds songs, counted with the walk's own rules (not `skip`'s folders).
pub fn found(skip: &[String]) -> Option<Found> {
    found_in(usual_places(), skip)
}

/// The first of `places` that holds songs.
pub fn found_in(places: Vec<PathBuf>, skip: &[String]) -> Option<Found> {
    places.into_iter().filter(|place| place.is_dir()).find_map(|place| {
        let folder = place.to_str()?.to_string();
        let (mut songs, mut cloud) = (0, 0);
        walk(std::slice::from_ref(&folder), skip, &mut |song| {
            songs += usize::from(!song.protected);
            cloud += usize::from(!song.protected && song.cloud);
            songs < MOST_COUNTED
        });
        (songs > 0).then_some(Found { folder, songs, capped: songs >= MOST_COUNTED, cloud })
    })
}

/// A file the walk found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Walked {
    pub path: String,
    /// The music folder it's under (one of the folders walked).
    pub folder: String,
    pub size: u64,
    /// When it last changed, ms.
    pub mtime: i64,
    /// One Kumi can name but never hear (`PROTECTED`).
    pub protected: bool,
    /// Only in the cloud now (OneDrive's online-only files, iCloud's and others' on a Mac): reading any of it would
    /// download it all, so it's named by its path and not heard until it's on this computer.
    pub cloud: bool,
}

/// The folders a walk reads: each as the disk spells it, with `~` read, and none inside another (its songs are the
/// outer one's).
pub fn folders_to_walk(folders: &[String]) -> Vec<String> {
    let home = homedir();
    let mut kept: Vec<String> = vec![];
    for folder in folders.iter().filter_map(|folder| expand_folder(folder, &home)).map(|folder| on_disk(&normalize(&folder))) {
        let folder = trimmed(&folder, cfg!(windows));
        if kept.iter().any(|outer| same_or_below(&folder, outer)) {
            continue;
        }
        kept.retain(|inner| !same_or_below(inner, &folder));
        kept.push(folder);
    }
    kept
}

/// A folder without the separators at its end, but for a root, which keeps its own (`/`, or `M:\` on Windows: a bare
/// `M:` is the drive's current folder, and a path joined to it has no separator).
fn trimmed(folder: &str, windows: bool) -> String {
    let kept = folder.trim_end_matches(['/', '\\']);
    if kept.is_empty() {
        return normalize("/");
    }
    match kept.as_bytes() {
        [letter, b':'] if windows && letter.is_ascii_alphabetic() => format!("{kept}\\"),
        _ => kept.to_string(),
    }
}

/// A path as compared: in one case where the system doesn't tell cases apart (Windows, a Mac).
pub(crate) fn fold(text: &str) -> String {
    if cfg!(any(windows, target_os = "macos")) {
        text.to_lowercase()
    } else {
        text.to_string()
    }
}

/// Whether `path` is `folder` or inside it.
pub(crate) fn same_or_below(path: &str, folder: &str) -> bool {
    let (path, folder) = (fold(path), fold(folder));
    path == folder || below(&path, &folder).is_some()
}

/// Whether a file's data is only in the cloud: Windows' cloud files (OneDrive's online-only ones) by their attributes,
/// a Mac's dataless files (iCloud Drive, and the file providers of Dropbox, OneDrive and others) by their flags.
fn in_cloud(meta: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        cloud_attributes(meta.file_attributes())
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::macos::fs::MetadataExt;
        dataless(meta.st_flags())
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = meta;
        false
    }
}

/// Windows' attributes of a file that isn't all on the disk: offline, or recalled when it's opened or read.
pub fn cloud_attributes(attributes: u32) -> bool {
    const OFFLINE: u32 = 0x1000;
    const RECALL_ON_OPEN: u32 = 0x4_0000;
    const RECALL_ON_DATA_ACCESS: u32 = 0x40_0000;
    attributes & (OFFLINE | RECALL_ON_OPEN | RECALL_ON_DATA_ACCESS) != 0
}

/// A Mac's flag for a file whose data a file provider keeps elsewhere (`SF_DATALESS`).
pub fn dataless(flags: u32) -> bool {
    flags & 0x4000_0000 != 0
}

/// What a walk could read: whether each folder could be read at all, and the paths under them it couldn't (a folder it
/// couldn't list, a link or file it couldn't look at, a link to a drive that isn't there now). Songs under either have
/// gone nowhere: they aren't marked gone.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reach {
    pub readable: Vec<bool>,
    pub unread: Vec<String>,
}

/// Every song file under `folders`, each told to `visit` (false stops the walk), and what the walk could read (a
/// folder that couldn't be read at all, a drive that isn't there now, has gone nowhere: its songs aren't marked gone).
///
/// Left out: hidden folders and files, the folders in `skip` that lie inside a folder walked (the producer's sample
/// folders, the library's places: a skip around a music folder doesn't hide it), DAW projects' folders, and any folder
/// reached a second time (a link back up the tree).
pub fn walk(folders: &[String], skip: &[String], visit: &mut dyn FnMut(Walked) -> bool) -> Reach {
    let home = homedir();
    let skip: Vec<String> = skip
        .iter()
        .filter_map(|folder| expand_folder(folder, &home))
        .map(|folder| on_disk(&normalize(&folder)))
        .filter(|folder| !folder.trim_end_matches(['/', '\\']).is_empty())
        .map(|folder| trimmed(&folder, cfg!(windows)))
        .collect();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut reach = Reach::default();
    for root in folders {
        // Only the skips inside this folder count here.
        let inside: Vec<&String> = skip.iter().filter(|folder| below(&fold(folder), &fold(root)).is_some()).collect();
        if std::fs::read_dir(root).is_err() {
            reach.readable.push(false);
            continue;
        }
        reach.readable.push(true);
        // A volume whose paths can't be made canonical (some on Windows) is walked by the paths it lists.
        let canonical = std::fs::canonicalize(root).unwrap_or_else(|_| PathBuf::from(root));
        let mut stack = vec![(PathBuf::from(root), canonical)];
        while let Some((folder, canonical)) = stack.pop() {
            if !seen.insert(canonical) {
                continue;
            }
            let Ok(entries) = std::fs::read_dir(&folder) else {
                reach.unread.push(folder.to_string_lossy().into_owned());
                continue;
            };
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
                // A link is read as what it points to.
                let meta = entry.file_type().and_then(|kind| if kind.is_symlink() { std::fs::metadata(&path) } else { entry.metadata() });
                let Ok(meta) = meta else {
                    reach.unread.push(path.to_string_lossy().into_owned());
                    continue;
                };
                if meta.is_dir() {
                    let text = path.to_string_lossy();
                    if PROJECT_BUNDLES.contains(&extension(name).as_str()) || inside.iter().any(|skipped| same_or_below(&text, skipped)) {
                        continue;
                    }
                    let canonical = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
                    folders_here.push((path, canonical));
                } else if meta.is_file() && (is_song(name) || is_protected(name)) {
                    let Some(text) = path.to_str() else { continue };
                    let mtime = meta
                        .modified()
                        .ok()
                        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                        .map_or(0, |since| since.as_millis() as i64);
                    let song = Walked {
                        path: text.to_string(),
                        folder: root.clone(),
                        size: meta.len(),
                        mtime,
                        protected: is_protected(name),
                        cloud: in_cloud(&meta),
                    };
                    if !visit(song) {
                        return reach;
                    }
                }
            }
            // Depth first, in name order.
            folders_here.sort_by(|a, b| b.0.cmp(&a.0));
            stack.extend(folders_here);
        }
    }
    reach
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
    fn a_root_keeps_its_separator() {
        assert_eq!(trimmed("M:\\", true), "M:\\", "a whole drive, not its current folder");
        assert_eq!(trimmed("m:/", true), "m:\\");
        assert_eq!(trimmed("M:\\Music\\", true), "M:\\Music");
        assert_eq!(trimmed("/Users/someone/Music//", false), "/Users/someone/Music");
        assert_eq!(trimmed("M:/", false), "M:", "elsewhere, a folder of that name");
        assert_eq!(trimmed("/", false), normalize("/"));
    }

    #[test]
    fn files_only_in_the_cloud_are_known_by_their_marks() {
        // Windows: recalled when read (OneDrive's online-only files), when opened, or offline; archive and normal aren't.
        assert!(cloud_attributes(0x40_0000 | 0x20) && cloud_attributes(0x4_0000) && cloud_attributes(0x1000));
        assert!(!cloud_attributes(0x20) && !cloud_attributes(0x80));
        // A Mac: SF_DATALESS, beside the flags a file usually has.
        assert!(dataless(0x4000_0000) && !dataless(0x8000));
    }

    #[test]
    fn nested_folders_are_walked_once() {
        // Absolute on every system, and never made.
        let root = std::env::temp_dir().join("kumi-nowhere-at-all");
        let path = |rest: &str| rest.split('/').fold(root.clone(), |path, part| path.join(part)).to_string_lossy().into_owned();
        let trailing = format!("{}{}", path("m/a"), std::path::MAIN_SEPARATOR);
        let folders = folders_to_walk(&[path("m/a/b"), path("m/a"), path("n"), trailing]);
        assert_eq!(folders, [normalize(&path("m/a")), normalize(&path("n"))]);
    }
}
