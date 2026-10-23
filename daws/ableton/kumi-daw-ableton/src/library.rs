//! Live's library, as the library learns it: its User Library and Places (as its newest Library.cfg and its indexer's
//! log name them), Splice's downloads, the packs Live installed and its Core Library, the Sets it opened lately (from
//! its log), and its manual.

use std::rc::Rc;
use std::sync::LazyLock;

use kumi_library::daws::{Daws, LibrarySources, ManualOptions, Places};
use kumi_library::sources::{
    basename, env_of, exists, home_of, is_absolute, is_folder, join, list, mtime_ms, place, platform_of, unescape, Source, SourceKind,
    SourceOptions,
};
use regex::Regex;

use kumi_runtime::core::contracts::KernelTool;
use kumi_types::daw::DawProfile;

/// Live's library.
pub struct LiveLibrary;

/// Live's library, as the registry gives it to the library.
pub static LIBRARY: LiveLibrary = LiveLibrary;

/// Live's library alone, for what only Live reads of it (its Places, as Library.cfg and its indexer name them).
pub static ALONE: Daws = Daws(&[&LIBRARY]);

impl LibrarySources for LiveLibrary {
    fn profile(&self) -> &'static DawProfile {
        &DawProfile::ABLETON
    }

    /// Live's User Library and Places first; after the folders the producer names, Splice's downloads, the packs Live
    /// installed, and its Core Library.
    fn places(&self, options: &SourceOptions) -> Places {
        let home = home_of(options);
        let platform = platform_of(options);
        let documents = if platform == "win32" { join(&home, "Documents") } else { join(&home, "Music") };
        let mut found = Places::default();
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
        found.before.extend(place(
            Some(config.user_library.as_deref().unwrap_or(&default_user_library)),
            "User Library",
            SourceKind::UserLibrary,
        ));
        for indexed in &indexer.places {
            found.before.extend(place(Some(&indexed.path), &indexed.label, SourceKind::Place));
        }
        for configured in &config.places {
            found.before.extend(place(Some(configured), &basename(configured), SourceKind::Place));
        }
        // Splice's own app keeps its downloads in ~/Splice (Documents on Windows); Live's Splice browser may keep them elsewhere.
        found.after.extend(place(config.splice.as_deref(), "Splice", SourceKind::Splice));
        let splice_folders = [join(&home, "Splice"), join(&join(&home, "Documents"), "Splice")];
        found.after.extend(place(splice_folders.iter().find(|folder| is_folder(folder)).map(String::as_str), "Splice", SourceKind::Splice));
        for pack in &indexer.packs {
            found.after.extend(place(Some(&pack.path), &pack.label, pack.kind));
        }
        for folder in [config.packs.clone(), Some(join(&join(&documents, "Ableton"), "Factory Packs"))].into_iter().flatten() {
            let mut names = list(&folder);
            names.sort();
            for name in names {
                if !name.starts_with('.') && is_folder(&join(&folder, &name)) {
                    found.after.extend(place(Some(&join(&folder, &name)), &name, SourceKind::Pack));
                }
            }
        }
        if !found.before.iter().chain(&found.after).any(|source| source.kind == SourceKind::Core) {
            let core = core_libraries(options).into_iter().find(|folder| is_folder(folder));
            found.after.extend(place(core.as_deref(), "Core Library", SourceKind::Core));
        }
        found
    }

    fn recent_projects(&self, options: &SourceOptions) -> Vec<String> {
        recent_sets(options)
    }

    fn manual_tool(&self, options: ManualOptions) -> Option<Rc<dyn KernelTool>> {
        Some(super::manual::manual_tool(options))
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
