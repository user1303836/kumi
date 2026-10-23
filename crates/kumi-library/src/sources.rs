//! Where the producer's sounds, presets and Sets are: each DAW's places, as its integration says (Live's User Library
//! and Places, its packs and Core Library, Splice's folder; REAPER's and Renoise's own libraries; see
//! [`LibrarySources`](crate::daws::LibrarySources)), and folders the producer named. Only folders that exist; a folder
//! inside another counts once, but a DAW's library stays a source of its own inside another folder.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf, Prefix};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

use crate::daws::{Daws, Places};
use kumi_types::daw::DawId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SourceKind {
    UserLibrary,
    Place,
    Pack,
    Core,
    Splice,
    Folder,
    /// A DAW's own library (REAPER's resource folder, one of Renoise's libraries), read by its DAW's layout
    /// ([`LibrarySources::layout`](crate::daws::LibrarySources::layout)). Written as the DAW's id ("reaper"), as each was
    /// before.
    #[serde(untagged)]
    Daw(DawId),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    pub path: String,
    /// What the producer calls it: "User Library", a Place's name, a pack's.
    pub label: String,
    pub kind: SourceKind,
}

impl Source {
    /// The DAW whose own library this is: what's in it is offered only in that DAW. None for the producer's own folders
    /// (Live's User Library and Places, Splice's, folders they named), whose sounds every DAW can use.
    pub fn daw(&self) -> Option<DawId> {
        match self.kind {
            SourceKind::Pack | SourceKind::Core => Some(DawId::Ableton),
            SourceKind::Daw(daw) => Some(daw),
            SourceKind::UserLibrary | SourceKind::Place | SourceKind::Splice | SourceKind::Folder => None,
        }
    }

    /// Whether its sounds are offered in a session with this DAW (Live's when none is known, as before other DAWs).
    pub fn offered_in(&self, daw: Option<DawId>) -> bool {
        self.daw().is_none_or(|own| own == daw.unwrap_or(DawId::Ableton))
    }

    /// A DAW's own library, read by that DAW's layout rather than by file type alone.
    pub fn is_daw_library(&self) -> bool {
        matches!(self.kind, SourceKind::Daw(_))
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceOptions {
    pub home: Option<String>,
    /// Node's names: "darwin", "win32", "linux"…
    pub platform: Option<String>,
    pub env: Option<HashMap<String, String>>,
    /// Folders the producer named (settings.json's libraryFolders, folders Kumi was asked to search): absolute or ~/….
    pub folders: Option<Vec<String>>,
    /// Where Live's apps are (macOS) and its shared files (Windows), for tests.
    pub applications: Option<String>,
    pub program_data: Option<String>,
    /// The DAW whose places these are (Live's when None).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daw: Option<DawId>,
}

// ---------------------------------------------------------------------------------------------------
// Node's `os` and `path` for this computer, as the TypeScript used them (shared with sets and plugins).

/// `process.platform`.
pub fn current_platform() -> &'static str {
    match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    }
}

/// `os.homedir()`.
pub fn homedir() -> String {
    home::home_dir().map(|path| path.to_string_lossy().into_owned()).unwrap_or_default()
}

/// `path.sep`.
pub const SEP: char = if cfg!(windows) { '\\' } else { '/' };

fn is_separator(c: char) -> bool {
    c == '/' || (cfg!(windows) && c == '\\')
}

/// `path` as the disk spells it, when it's there in another case (Windows and a Mac's usual volumes don't tell case
/// apart): one folder typed two ways is one place, and its sounds come once. Links aren't followed, and a path that
/// isn't there is left as it is: on a volume that tells case apart, another case is another folder.
pub fn on_disk(path: &str) -> String {
    let given = Path::new(path);
    if !given.is_absolute() || !given.exists() {
        return path.to_string();
    }
    let mut spelled = PathBuf::new();
    for part in given.components() {
        match part {
            Component::Prefix(prefix) => match prefix.kind() {
                Prefix::Disk(letter) => spelled.push(format!("{}:", letter.to_ascii_uppercase() as char)),
                _ => spelled.push(part.as_os_str()),
            },
            Component::Normal(name) => {
                let names: Vec<OsString> = std::fs::read_dir(&spelled)
                    .map(|entries| entries.flatten().map(|entry| entry.file_name()).collect())
                    .unwrap_or_default();
                let lower = name.to_string_lossy().to_lowercase();
                let found = if names.iter().any(|other| other == name) {
                    name.to_os_string()
                } else {
                    names.into_iter().find(|other| other.to_string_lossy().to_lowercase() == lower).unwrap_or_else(|| name.to_os_string())
                };
                spelled.push(found);
            }
            other => spelled.push(other.as_os_str()),
        }
    }
    spelled.to_string_lossy().into_owned()
}

/// What of `path` is inside `folder` ("Kicks/808.wav"), when it's there. A root (`/`, `Z:\`, `\\NAS\Samples\`)
/// already ends with its separator, so it isn't added twice. On Windows either separator follows a folder.
pub fn below<'a>(path: &'a str, folder: &str) -> Option<&'a str> {
    let rest = path.strip_prefix(folder)?;
    if folder.ends_with(is_separator) {
        Some(rest)
    } else {
        rest.strip_prefix(is_separator)
    }
}

fn is_device_root(c: char) -> bool {
    c.is_ascii_alphabetic()
}

/// `path.isAbsolute(path)`.
pub fn is_absolute(path: &str) -> bool {
    let chars: Vec<char> = path.chars().collect();
    if chars.is_empty() {
        return false;
    }
    if cfg!(windows) {
        is_separator(chars[0]) || (chars.len() > 2 && is_device_root(chars[0]) && chars[1] == ':' && is_separator(chars[2]))
    } else {
        chars[0] == '/'
    }
}

/// Node's `normalizeString`: segments with "." and ".." resolved, joined by the separator.
fn normalize_string(path: &str, allow_above_root: bool, separator: char) -> String {
    let mut segments: Vec<&str> = Vec::new();
    for segment in path.split(is_separator) {
        match segment {
            "" | "." => {}
            ".." => {
                if segments.last().is_some_and(|last| *last != "..") {
                    segments.pop();
                } else if allow_above_root {
                    segments.push("..");
                }
            }
            other => segments.push(other),
        }
    }
    segments.join(&separator.to_string())
}

/// Windows: a path's device ("C:" or "\\server\share"), where its root ends, and whether it's absolute.
fn win32_root(chars: &[char]) -> (Option<String>, usize, bool) {
    let len = chars.len();
    let text = |from: usize, to: usize| chars[from..to].iter().collect::<String>();
    if len == 0 {
        return (None, 0, false);
    }
    if is_separator(chars[0]) {
        if len > 1 && is_separator(chars[1]) {
            let mut j = 2;
            let mut last = j;
            while j < len && !is_separator(chars[j]) {
                j += 1;
            }
            if j < len && j != last {
                let first = text(last, j);
                last = j;
                while j < len && is_separator(chars[j]) {
                    j += 1;
                }
                if j < len && j != last {
                    last = j;
                    while j < len && !is_separator(chars[j]) {
                        j += 1;
                    }
                    if j == len {
                        return (Some(format!("\\\\{first}\\{}", text(last, j))), j, true);
                    }
                    if j != last {
                        return (Some(format!("\\\\{first}\\{}", text(last, j))), j, true);
                    }
                }
            }
        }
        return (None, 1, true);
    }
    if len > 1 && is_device_root(chars[0]) && chars[1] == ':' {
        let absolute = len > 2 && is_separator(chars[2]);
        return (Some(text(0, 2)), if absolute { 3 } else { 2 }, absolute);
    }
    (None, 0, false)
}

/// `path.normalize(path)`.
pub fn normalize(path: &str) -> String {
    if path.is_empty() {
        return ".".into();
    }
    let chars: Vec<char> = path.chars().collect();
    let trailing = is_separator(chars[chars.len() - 1]);
    if cfg!(windows) {
        if chars.len() == 1 {
            return if chars[0] == '/' { "\\".into() } else { path.to_string() };
        }
        let (device, root_end, absolute) = win32_root(&chars);
        let rest: String = chars[root_end.min(chars.len())..].iter().collect();
        let mut tail = normalize_string(&rest, !absolute, '\\');
        if tail.is_empty() && !absolute {
            tail = ".".into();
        }
        if !tail.is_empty() && trailing {
            tail.push('\\');
        }
        return match device {
            None => {
                if absolute {
                    format!("\\{tail}")
                } else {
                    tail
                }
            }
            Some(device) => {
                if absolute {
                    format!("{device}\\{tail}")
                } else {
                    format!("{device}{tail}")
                }
            }
        };
    }
    let absolute = chars[0] == '/';
    let mut tail = normalize_string(path, !absolute, '/');
    if tail.is_empty() {
        if absolute {
            return "/".into();
        }
        return if trailing { "./".into() } else { ".".into() };
    }
    if trailing {
        tail.push('/');
    }
    if absolute {
        format!("/{tail}")
    } else {
        tail
    }
}

/// `path.resolve(path)`: absolute, normalized, without a trailing separator.
pub fn resolve(path: &str) -> String {
    let cwd = || std::env::current_dir().map(|dir| dir.to_string_lossy().into_owned()).unwrap_or_default();
    if cfg!(windows) {
        let chars: Vec<char> = path.chars().collect();
        let (mut device, root_end, absolute) = win32_root(&chars);
        let mut tail: String = chars[root_end.min(chars.len())..].iter().collect();
        if !absolute {
            // Relative to the working directory, as Node resolves it.
            let base: Vec<char> = cwd().chars().collect();
            let (base_device, base_root, _) = win32_root(&base);
            if device.is_none() {
                device = base_device;
            }
            tail = format!("{}\\{tail}", base[base_root.min(base.len())..].iter().collect::<String>());
        } else if device.is_none() {
            device = win32_root(&cwd().chars().collect::<Vec<_>>()).0;
        }
        let tail = normalize_string(&tail, false, '\\');
        return format!("{}\\{tail}", device.unwrap_or_default());
    }
    let full = if path.starts_with('/') { path.to_string() } else { format!("{}/{path}", cwd()) };
    format!("/{}", normalize_string(&full, false, '/'))
}

/// `path.join(base, more)`.
pub fn join(base: &str, more: &str) -> String {
    let parts: Vec<&str> = [base, more].into_iter().filter(|part| !part.is_empty()).collect();
    if parts.is_empty() {
        return ".".into();
    }
    let mut joined = parts.join(&SEP.to_string());
    if cfg!(windows) {
        // A UNC path keeps its two leading slashes; other runs of them collapse to one.
        let first: Vec<char> = parts[0].chars().collect();
        let mut needs_replace = true;
        let mut slashes = 0;
        if is_separator(first[0]) {
            slashes += 1;
            if first.len() > 1 && is_separator(first[1]) {
                slashes += 1;
                if first.len() > 2 {
                    if is_separator(first[2]) {
                        slashes += 1;
                    } else {
                        needs_replace = false;
                    }
                }
            }
        }
        if needs_replace {
            let chars: Vec<char> = joined.chars().collect();
            while slashes < chars.len() && is_separator(chars[slashes]) {
                slashes += 1;
            }
            if slashes >= 2 {
                joined = format!("\\{}", chars[slashes..].iter().collect::<String>());
            }
        }
    }
    normalize(&joined)
}

/// `path.basename(path)`.
pub fn basename(path: &str) -> String {
    let chars: Vec<char> = path.chars().collect();
    let start = if cfg!(windows) && chars.len() >= 2 && is_device_root(chars[0]) && chars[1] == ':' { 2 } else { 0 };
    let mut from = start;
    let mut end: Option<usize> = None;
    let mut matched_slash = true;
    let mut i = chars.len();
    while i > start {
        i -= 1;
        if is_separator(chars[i]) {
            if !matched_slash {
                from = i + 1;
                break;
            }
        } else if end.is_none() {
            matched_slash = false;
            end = Some(i + 1);
        }
    }
    match end {
        None => String::new(),
        Some(end) => chars[from..end].iter().collect(),
    }
}

/// `path.dirname(path)`.
pub fn dirname(path: &str) -> String {
    let chars: Vec<char> = path.chars().collect();
    let len = chars.len();
    if len == 0 {
        return ".".into();
    }
    let text = |to: usize| chars[..to].iter().collect::<String>();
    if cfg!(windows) {
        if len == 1 {
            return if is_separator(chars[0]) { path.to_string() } else { ".".into() };
        }
        let (device, root_end, _) = win32_root(&chars);
        let (root_end, offset) = match (&device, root_end) {
            (Some(device), _) if device.starts_with("\\\\") => {
                if root_end == len {
                    return path.to_string();
                }
                (root_end + 1, root_end + 1)
            }
            (None, 1) => (1, 1),
            (Some(_), end) => (end, end),
            (None, _) => (usize::MAX, 0),
        };
        let mut end: Option<usize> = None;
        let mut matched_slash = true;
        let mut i = len;
        while i > offset {
            i -= 1;
            if is_separator(chars[i]) {
                if !matched_slash {
                    end = Some(i);
                    break;
                }
            } else {
                matched_slash = false;
            }
        }
        return match end {
            Some(end) => text(end),
            None if root_end == usize::MAX => ".".into(),
            None => text(root_end.min(len)),
        };
    }
    let has_root = chars[0] == '/';
    let mut end: Option<usize> = None;
    let mut matched_slash = true;
    let mut i = len;
    while i > 1 {
        i -= 1;
        if chars[i] == '/' {
            if !matched_slash {
                end = Some(i);
                break;
            }
        } else {
            matched_slash = false;
        }
    }
    match end {
        None => {
            if has_root {
                "/".into()
            } else {
                ".".into()
            }
        }
        Some(1) if has_root => "//".into(),
        Some(end) => text(end),
    }
}

// ---------------------------------------------------------------------------------------------------

/// `readdirSync`: a folder's names (sorted, as Node lists them), or none.
pub fn list(folder: &str) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(folder) else { return Vec::new() };
    let mut names: Vec<String> = entries.flatten().map(|entry| entry.file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    names
}

pub fn is_folder(path: &str) -> bool {
    std::fs::metadata(path).map(|info| info.is_dir()).unwrap_or(false)
}

pub fn exists(path: &str) -> bool {
    Path::new(path).exists()
}

/// `statSync(path).mtimeMs`.
pub fn mtime_ms(path: &str) -> f64 {
    std::fs::metadata(path)
        .ok()
        .and_then(|info| info.modified().ok())
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|since| since.as_secs_f64() * 1000.0)
        .unwrap_or(0.0)
}

/// XML's five entities, as text.
pub fn unescape(value: &str) -> String {
    value.replace("&quot;", "\"").replace("&lt;", "<").replace("&gt;", ">").replace("&apos;", "'").replace("&amp;", "&")
}

/// "~/Samples" and absolute paths; anything else isn't a folder Kumi can find.
pub fn expand_folder(value: &str, home: &str) -> Option<String> {
    let trimmed = kumi_common::js::string::trim(value);
    let expanded = if trimmed == "~" {
        home.to_string()
    } else if trimmed.starts_with("~/") || trimmed.starts_with("~\\") {
        join(home, &trimmed[2..])
    } else {
        trimmed.to_string()
    };
    if !expanded.is_empty() && is_absolute(&expanded) {
        Some(resolve(&expanded))
    } else {
        None
    }
}

pub fn platform_of(options: &SourceOptions) -> String {
    options.platform.clone().unwrap_or_else(|| current_platform().to_string())
}

pub fn home_of(options: &SourceOptions) -> String {
    options.home.clone().unwrap_or_else(homedir)
}

/// `env.NAME`, from the injected environment or the process's.
pub fn env_of(options: &SourceOptions, name: &str) -> Option<String> {
    match &options.env {
        Some(env) => env.get(name).cloned(),
        None => std::env::var(name).ok(),
    }
}

/// A place where the library looks, as it keeps one: resolved, and spelled as the disk spells it. None for a path that
/// isn't absolute.
pub fn place(path: Option<&str>, label: &str, kind: SourceKind) -> Option<Source> {
    let path = path.filter(|path| !path.is_empty() && is_absolute(path))?;
    Some(Source { path: on_disk(&resolve(path)), label: label.to_string(), kind })
}

/// Where everything is: each DAW's places before the folders the producer named (Live's User Library first), those
/// folders, and each DAW's places after them.
pub fn library_sources(daws: Daws, options: &SourceOptions) -> Vec<Source> {
    debug_assert!(
        daws.0.is_empty() || daws.iter().any(|daw| daw.profile().id == Some(options.daw.unwrap_or(DawId::Ableton))),
        "a registered DAW's places"
    );
    let home = home_of(options);
    let places: Vec<Places> = daws.iter().map(|daw| daw.places(options)).collect();
    let mut found: Vec<Source> = places.iter().flat_map(|places| places.before.iter().cloned()).collect();
    for folder in options.folders.as_deref().unwrap_or_default() {
        let path = expand_folder(folder, &home);
        let label = match &path {
            Some(path) => basename(path),
            None => folder.clone(),
        };
        found.extend(place(path.as_deref(), &label, SourceKind::Folder));
    }
    found.extend(places.into_iter().flat_map(|places| places.after));
    // Each folder once, and not again inside another: the outer one is searched whole.
    let mut unique: Vec<Source> = Vec::new();
    for source in found {
        if !is_folder(&source.path) {
            continue;
        }
        if unique.iter().any(|kept| kept.path == source.path || below(&source.path, &kept.path).is_some()) {
            continue;
        }
        unique.retain(|kept| below(&kept.path, &source.path).is_none());
        unique.push(source);
    }
    // A DAW's own library is read by its layout, even inside a folder the producer named (that folder's walk leaves it
    // out); each once.
    for source in daws.iter().flat_map(|daw| daw.own_library(options)) {
        if is_folder(&source.path) && !unique.iter().any(|kept| kept.path == source.path) {
            unique.push(source);
        }
    }
    unique
}

/// The projects the DAWs opened lately, the producer's own (Live's Sets, from its log): each DAW's, in turn.
pub fn recent_projects(daws: Daws, options: &SourceOptions) -> Vec<String> {
    debug_assert!(
        daws.0.is_empty() || daws.iter().any(|daw| daw.profile().id == Some(options.daw.unwrap_or(DawId::Ableton))),
        "a registered DAW's projects"
    );
    daws.iter().flat_map(|daw| daw.recent_projects(options)).collect()
}

/// A sound's or preset's place in Live's Browser ("user_library/Samples/Kick.wav"), for the sources the Browser lists by path.
pub fn browser_path(source: &Source, relative_path: &str) -> Option<String> {
    let parts: Vec<&str> = relative_path.split(['\\', '/']).filter(|part| !part.is_empty()).collect();
    if parts.is_empty() {
        return None;
    }
    let root: Vec<&str> = match source.kind {
        SourceKind::UserLibrary => vec!["user_library"],
        SourceKind::Place => vec!["user_folders", &source.label],
        SourceKind::Pack => vec!["packs", &source.label],
        _ => return None,
    };
    Some(root.into_iter().chain(parts).collect::<Vec<_>>().join("/"))
}
