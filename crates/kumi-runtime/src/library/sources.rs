//! Where the producer's sounds, presets and Sets are: Live's User Library and Places (as Live's own
//! preferences name them), the packs Live installed and its Core Library, Splice's folder, and folders
//! the producer named. Only folders that exist; a folder inside another counts once.

use std::collections::HashMap;
use std::path::Path;
use std::sync::LazyLock;
use std::time::UNIX_EPOCH;

use regex::Regex;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SourceKind {
    UserLibrary,
    Place,
    Pack,
    Core,
    Splice,
    Folder,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    pub path: String,
    /// What the producer calls it: "User Library", a Place's name, a pack's.
    pub label: String,
    pub kind: SourceKind,
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

/// What of `path` is inside `folder` ("Kicks/808.wav"), when it's there. A root (`/`, `Z:\`, `\\NAS\Samples\`)
/// already ends with its separator, so it isn't added twice.
pub fn below<'a>(path: &'a str, folder: &str) -> Option<&'a str> {
    let rest = path.strip_prefix(folder)?;
    if folder.ends_with(is_separator) {
        Some(rest)
    } else {
        rest.strip_prefix(SEP)
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
fn list(folder: &str) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(folder) else { return Vec::new() };
    let mut names: Vec<String> = entries.flatten().map(|entry| entry.file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    names
}

fn is_folder(path: &str) -> bool {
    std::fs::metadata(path).map(|info| info.is_dir()).unwrap_or(false)
}

fn exists(path: &str) -> bool {
    Path::new(path).exists()
}

/// `statSync(path).mtimeMs`.
fn mtime_ms(path: &str) -> f64 {
    std::fs::metadata(path)
        .ok()
        .and_then(|info| info.modified().ok())
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|since| since.as_secs_f64() * 1000.0)
        .unwrap_or(0.0)
}

fn unescape(value: &str) -> String {
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

fn platform_of(options: &SourceOptions) -> String {
    options.platform.clone().unwrap_or_else(|| current_platform().to_string())
}

fn home_of(options: &SourceOptions) -> String {
    options.home.clone().unwrap_or_else(homedir)
}

/// `env.NAME`, from the injected environment or the process's.
fn env_of(options: &SourceOptions, name: &str) -> Option<String> {
    match &options.env {
        Some(env) => env.get(name).cloned(),
        None => std::env::var(name).ok(),
    }
}

static LIVE_FOLDER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^Live [0-9]").unwrap());

/// Live's preference folders, newest first ("Live 12.1.5", "Live 12.0.20"…).
pub fn live_preference_folders(options: &SourceOptions) -> Vec<String> {
    let home = home_of(options);
    let platform = platform_of(options);
    let root = if platform == "win32" {
        join(&env_of(options, "APPDATA").unwrap_or_else(|| join(&join(&home, "AppData"), "Roaming")), "Ableton")
    } else {
        join(&join(&join(&home, "Library"), "Preferences"), "Ableton")
    };
    let mut folders: Vec<(String, f64)> = list(&root)
        .into_iter()
        .filter(|name| LIVE_FOLDER.is_match(name))
        .map(|name| {
            let folder = join(&root, &name);
            if platform == "win32" {
                join(&folder, "Preferences")
            } else {
                folder
            }
        })
        .filter(|folder| exists(&join(folder, "Library.cfg")))
        .map(|folder| {
            let mtime = mtime_ms(&join(&folder, "Library.cfg"));
            (folder, mtime)
        })
        .collect();
    folders.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    folders.into_iter().map(|(folder, _)| folder).collect()
}

/// What Live's newest Library.cfg says.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LibraryConfig {
    pub user_library: Option<String>,
    pub places: Vec<String>,
    pub packs: Option<String>,
    pub splice: Option<String>,
}

static USER_LIBRARY_BLOCK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<UserLibrary>(.*?)</UserLibrary>").unwrap());
static PLACES_BLOCK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<UserFolderInfoList>(.*?)</UserFolderInfoList>").unwrap());
static ANY_VALUE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"Value="([^"]+)""#).unwrap());
static USER_FOLDER_ADDRESS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^userfolder:(.+?)(?:#.*)?$").unwrap());
static FILE_EXTENSION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\.[A-Za-z0-9_]{2,4}$").unwrap());

/// What Live's newest Library.cfg says: the User Library, Places, where packs and Splice's downloads go.
pub fn read_library_config(text: &str) -> LibraryConfig {
    let value = |block: &str, field: &str| -> Option<String> {
        let pattern = Regex::new(&format!(r#"<{field} Value="([^"]*)""#)).expect("field pattern");
        pattern.captures(block).map(|found| unescape(&found[1]))
    };
    let user = USER_LIBRARY_BLOCK.captures(text).map(|found| found[1].to_string()).unwrap_or_default();
    let folder = value(&user, "ProjectPath");
    let name = value(&user, "ProjectName").filter(|name| !name.is_empty()).unwrap_or_else(|| "User Library".to_string());
    // Places are listed with their paths (or "userfolder:" addresses that hold them), whatever Live's version calls the fields.
    let places_block = PLACES_BLOCK.captures(text).map(|found| found[1].to_string()).unwrap_or_default();
    let mut places: Vec<String> = Vec::new();
    for found in ANY_VALUE.captures_iter(&places_block) {
        let raw = unescape(&found[1]);
        let path = match USER_FOLDER_ADDRESS.captures(&raw) {
            Some(address) => {
                let address = &address[1];
                percent_encoding::percent_decode_str(address)
                    .decode_utf8()
                    .map(|decoded| decoded.into_owned())
                    .unwrap_or_else(|_| address.to_string())
            }
            None => raw,
        };
        if is_absolute(&path) && !FILE_EXTENSION.is_match(&path) && !places.contains(&path) {
            places.push(path);
        }
    }
    let packs = value(text, "PreferredFactoryPacksInstallationPath");
    let splice = value(text, "CustomSpliceDownloadPathMember");
    LibraryConfig {
        user_library: folder.filter(|folder| !folder.is_empty() && is_absolute(folder)).map(|folder| join(&folder, &name)),
        places,
        packs: packs.filter(|packs| !packs.is_empty() && is_absolute(packs)),
        splice: splice.filter(|splice| !splice.is_empty() && is_absolute(splice)),
    }
}

/// The folders Live's indexer last said it looks after.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IndexerLog {
    pub places: Vec<Source>,
    pub packs: Vec<Source>,
}

static INDEXED_FOLDER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"'([^']+)'\s*\[([^\]]*)\]").unwrap());
static CORE_LIBRARY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)Core Library$").unwrap());

/// The folders Live's indexer last said it looks after, from its log ("Configure: UserFolders:
/// '/Users/me/Samples' [Samples]"): Places by `UserFolders`, packs by `FactoryPacks`.
pub fn read_indexer_log(text: &str) -> IndexerLog {
    let last = |kind: &str| -> Vec<(String, String)> {
        let marker = format!("Configure: {kind}:");
        let line =
            text.split('\n').map(|line| line.strip_suffix('\r').unwrap_or(line)).filter(|line| line.contains(&marker)).last().unwrap_or("");
        let key = format!("{kind}:");
        let rest = match line.find(&key) {
            Some(at) => &line[at + key.len()..],
            None => "",
        };
        INDEXED_FOLDER
            .captures_iter(rest)
            .map(|found| {
                let path = found[1].to_string();
                let label = kumi_common::js::string::trim(&found[2]);
                let label = if label.is_empty() { basename(&path) } else { label.to_string() };
                (path, label)
            })
            .collect()
    };
    IndexerLog {
        places: last("UserFolders").into_iter().map(|(path, label)| Source { path, label, kind: SourceKind::Place }).collect(),
        packs: last("FactoryPacks")
            .into_iter()
            .chain(last("LegacyFactoryPacks"))
            .map(|(path, label)| {
                let kind = if CORE_LIBRARY.is_match(&path) { SourceKind::Core } else { SourceKind::Pack };
                Source { path, label, kind }
            })
            .collect(),
    }
}

static LIVE_PROGRAM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^Live ").unwrap());
static LIVE_APP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^Ableton Live.*\.app$").unwrap());

/// The Core Library inside Live (the newest found): the app's on macOS, ProgramData's on Windows.
fn core_libraries(options: &SourceOptions) -> Vec<String> {
    let platform = platform_of(options);
    if platform == "win32" {
        let program_data = options
            .program_data
            .clone()
            .or_else(|| options.env.as_ref().and_then(|env| env.get("ProgramData").cloned()))
            .or_else(|| std::env::var("ProgramData").ok())
            .unwrap_or_else(|| "C:\\ProgramData".to_string());
        let ableton = join(&program_data, "Ableton");
        let mut names: Vec<String> = list(&ableton).into_iter().filter(|name| LIVE_PROGRAM.is_match(name)).collect();
        names.sort();
        names.reverse();
        return names.into_iter().map(|name| join(&join(&join(&ableton, &name), "Resources"), "Core Library")).collect();
    }
    let applications = options.applications.clone().unwrap_or_else(|| "/Applications".to_string());
    let mut names: Vec<String> = list(&applications).into_iter().filter(|name| LIVE_APP.is_match(name)).collect();
    names.sort();
    names.reverse();
    names.into_iter().map(|name| join(&join(&join(&join(&applications, &name), "Contents"), "App-Resources"), "Core Library")).collect()
}

/// Where everything is, User Library first.
pub fn library_sources(options: &SourceOptions) -> Vec<Source> {
    let home = home_of(options);
    let platform = platform_of(options);
    let documents = if platform == "win32" { join(&home, "Documents") } else { join(&home, "Music") };
    let mut found: Vec<Source> = Vec::new();
    let add = |found: &mut Vec<Source>, path: Option<&str>, label: &str, kind: SourceKind| {
        if let Some(path) = path.filter(|path| !path.is_empty() && is_absolute(path)) {
            found.push(Source { path: resolve(path), label: label.to_string(), kind });
        }
    };
    let preferences = live_preference_folders(options).into_iter().next();
    let mut config = LibraryConfig::default();
    let mut indexer = IndexerLog::default();
    if let Some(preferences) = preferences {
        if let Ok(text) = std::fs::read(join(&preferences, "Library.cfg")) {
            config = read_library_config(&String::from_utf8_lossy(&text));
        }
        if let Ok(text) = std::fs::read(join(&preferences, "Indexer.txt")) {
            indexer = read_indexer_log(&String::from_utf8_lossy(&text));
        }
    }
    let default_user_library = join(&join(&documents, "Ableton"), "User Library");
    add(&mut found, Some(config.user_library.as_deref().unwrap_or(&default_user_library)), "User Library", SourceKind::UserLibrary);
    for place in &indexer.places {
        add(&mut found, Some(&place.path), &place.label, SourceKind::Place);
    }
    for place in &config.places {
        add(&mut found, Some(place), &basename(place), SourceKind::Place);
    }
    for folder in options.folders.as_deref().unwrap_or_default() {
        let path = expand_folder(folder, &home);
        let label = match &path {
            Some(path) => basename(path),
            None => folder.clone(),
        };
        add(&mut found, path.as_deref(), &label, SourceKind::Folder);
    }
    // Splice's own app keeps its downloads in ~/Splice (Documents on Windows); Live's Splice browser may keep them elsewhere.
    add(&mut found, config.splice.as_deref(), "Splice", SourceKind::Splice);
    let splice_folders = [join(&home, "Splice"), join(&join(&home, "Documents"), "Splice")];
    add(&mut found, splice_folders.iter().find(|folder| is_folder(folder)).map(String::as_str), "Splice", SourceKind::Splice);
    for pack in &indexer.packs {
        add(&mut found, Some(&pack.path), &pack.label, pack.kind);
    }
    for folder in [config.packs.clone(), Some(join(&join(&documents, "Ableton"), "Factory Packs"))].into_iter().flatten() {
        let mut names = list(&folder);
        names.sort();
        for name in names {
            if !name.starts_with('.') && is_folder(&join(&folder, &name)) {
                add(&mut found, Some(&join(&folder, &name)), &name, SourceKind::Pack);
            }
        }
    }
    if !found.iter().any(|source| source.kind == SourceKind::Core) {
        let core = core_libraries(options).into_iter().find(|folder| is_folder(folder));
        add(&mut found, core.as_deref(), "Core Library", SourceKind::Core);
    }
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
    unique
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

static LOADED_DOCUMENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"Loading document "([^"\r\n]+\.als)""#).unwrap());
static LIVES_OWN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[\\/](App-Resources|Resources)[\\/](Builtin|Core Library)[\\/]").unwrap());
static INSIDE_APP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\.app[\\/]").unwrap());

/// Sets Live opened lately, from its logs ("Loading document "/Users/me/Song Project/Song.als""): the
/// producer's own, wherever they keep them. Live's own templates and lessons aren't the producer's.
pub fn recent_sets(options: &SourceOptions) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for folder in live_preference_folders(options) {
        let Ok(text) = std::fs::read(join(&folder, "Log.txt")) else { continue };
        let text = String::from_utf8_lossy(&text);
        for document in LOADED_DOCUMENT.captures_iter(&text) {
            let path = &document[1];
            if !LIVES_OWN.is_match(path) && !INSIDE_APP.is_match(path) && !found.iter().any(|known| known == path) {
                found.push(path.to_string());
            }
        }
    }
    found.into_iter().filter(|path| exists(path)).collect()
}
