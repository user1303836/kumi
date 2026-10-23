//! (M6) Logic Pro's library and places: where its sounds, patches and settings are on this Mac (the producer's
//! content folders before Logic's own, Apple Loops, Splice's and the folders the producer named), what Logic's files
//! are (`.caf` among the samples; patches, channel strip and plug-in settings and Sampler instruments as presets; a
//! `.logicx` package as a project), where its projects usually are, and the projects it opened lately as its bridge read
//! them from File > Open Recent. Only folders that exist; a folder inside another counts once.
use crate::core::{contracts::JsonObject, daw::DawId};
use crate::library::presets::{PresetCategory, PresetFacts};
use crate::library::sources::{basename, dirname, expand_folder, join, Source, SourceKind, SourceOptions};
use crate::library::tools::{FIND_PRESETS_TOOL, MY_SETS_TOOL};
use serde_json::{json, Value};
use std::path::Path;

/// The audio files Logic's library takes as sounds: the library's own kinds and Core Audio's `.caf`, which Apple Loops
/// and Logic's own content are made of.
pub const SAMPLE_EXTENSIONS: &[&str] = &[".wav", ".wave", ".aif", ".aiff", ".flac", ".mp3", ".ogg", ".m4a", ".caf"];
/// Logic's presets: a patch (a package), a channel strip setting, a plug-in setting, a Sampler instrument, and an Audio
/// Unit's own preset.
pub const PRESET_EXTENSIONS: &[&str] = &[".patch", ".cst", ".pst", ".exs", ".aupreset"];
/// A preset Logic keeps as a package (a folder Finder shows as one file), counted whole.
pub const PRESET_PACKAGES: &[&str] = &[".patch"];
/// A Logic project: a package.
pub const PROJECT_EXTENSION: &str = ".logicx";

/// The producer's own Logic content, under `~/Music/Audio Music Apps`, in the order Kumi learns it.
const USER_FOLDERS: [&str; 6] =
    ["Samples", "Untagged Loops", "Sampler Instruments", "Patches", "Channel Strip Settings", "Plug-In Settings"];
/// Logic's own content, under `/Library/Application Support/Logic`.
const SYSTEM_FOLDERS: [&str; 7] = [
    "Samples",
    "EXS Factory Samples",
    "Alchemy Samples",
    "Ultrabeat Samples",
    "Sampler Instruments",
    "Channel Strip Settings",
    "Plug-In Settings",
];

fn home_of(options: &SourceOptions) -> String {
    options.home.clone().unwrap_or_else(crate::library::sources::homedir)
}

/// The system's Library folder: `/Library` on a Mac (a test names another).
fn system_of(options: &SourceOptions) -> String {
    options.system_library.clone().unwrap_or_else(|| "/Library".into())
}

fn is_folder(path: &str) -> bool {
    std::fs::metadata(path).is_ok_and(|info| info.is_dir())
}

/// Where Logic keeps the producer's sounds, presets and settings: the producer's own content folders, the folders they
/// named, Splice's, Apple Loops and Logic's own content, each that exists and isn't inside another.
pub fn sources(options: &SourceOptions) -> Vec<Source> {
    let (home, system) = (home_of(options), system_of(options));
    let mut found: Vec<Source> = Vec::new();
    let mut add = |path: String, label: &str, kind: SourceKind| {
        if is_folder(&path) {
            found.push(Source { path: crate::library::sources::on_disk(&path), label: label.into(), kind });
        }
    };
    let user = join(&join(&home, "Music"), "Audio Music Apps");
    for folder in USER_FOLDERS {
        add(join(&user, folder), folder, SourceKind::UserLibrary);
    }
    add(join(&join(&join(&home, "Library"), "Audio"), "Apple Loops"), "Apple Loops (yours)", SourceKind::UserLibrary);
    for folder in options.folders.as_deref().unwrap_or_default() {
        if let Some(path) = expand_folder(folder, &home) {
            let label = basename(&path);
            add(path, &label, SourceKind::Folder);
        }
    }
    // Splice's own app keeps its downloads in ~/Splice, whatever the DAW.
    add(join(&home, "Splice"), "Splice", SourceKind::Splice);
    add(join(&join(&join(&system, "Audio"), "Apple Loops"), "Apple"), "Apple Loops", SourceKind::Pack);
    let logic = join(&join(&system, "Application Support"), "Logic");
    for folder in SYSTEM_FOLDERS {
        add(join(&logic, folder), &format!("Logic Pro {folder}"), SourceKind::Core);
    }
    crate::library::sources::unique(found)
}

/// Where Logic projects usually are: Logic's own folder (`~/Music/Logic`) and the folder holding each project it opened
/// lately (or the folder holding that, for a project kept in a folder of its own beside its audio).
pub fn project_folders(recent: &[String], home: &str) -> Vec<String> {
    let mut folders = indexmap::IndexSet::new();
    folders.insert(join(&join(home, "Music"), "Logic"));
    for package in recent {
        let holder = dirname(package);
        let in_folder = ["Audio Files", "Bounces", "Freeze Files"].iter().any(|beside| Path::new(&join(&holder, beside)).is_dir());
        let folder = if in_folder { dirname(&holder) } else { holder };
        if folder != home && folder.encode_utf16().count() > home.encode_utf16().count() {
            folders.insert(folder);
        }
    }
    folders.into_iter().collect()
}

/// The projects Logic opened lately, as its bridge last read them from File > Open Recent (the ones whose place it
/// knew), each still there.
pub fn recent(options: &SourceOptions) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for path in options.recent.as_deref().unwrap_or_default() {
        let path = path.trim_end_matches('/').to_string();
        if path.to_lowercase().ends_with(PROJECT_EXTENSION) && Path::new(&path).is_dir() && !found.contains(&path) {
            found.push(path);
        }
    }
    found
}

/// What a Logic preset is, from its place under its source: a plug-in setting by its plug-in's folder (Plug-In
/// Settings/Channel EQ/Vocal.pst), an Audio Unit's preset likewise, a Sampler instrument; a patch and a channel strip
/// setting are whole strips, which no one plug-in names.
pub fn preset_facts(format: &str, relative: &str) -> PresetFacts {
    let parts: Vec<&str> = relative.split(['\\', '/']).filter(|part| !part.is_empty()).collect();
    let folder = parts.len().checked_sub(2).map(|at| parts[at].to_string());
    match format {
        "pst" | "aupreset" => PresetFacts { category: Some(PresetCategory::Plugin), device: folder, ..Default::default() },
        "exs" => PresetFacts { category: Some(PresetCategory::Instrument), device: Some("Sampler".into()), ..Default::default() },
        _ => PresetFacts::default(),
    }
}

/// What Logic's presets are, as find_presets' kind offers them: an instrument (a Sampler instrument) or a plug-in's
/// setting; a patch or a channel strip setting is a whole strip, of no one kind.
const PRESET_KINDS: [&str; 2] = ["instrument", "plug-in"];
/// What the library tools say in Live's way, as Logic says it: a region where Live has a clip, the change that loads a
/// sample, presets without Live's devices.
const TOOL_WORDS: [(&str, &str); 4] = [
    ("an audio clip in the project (its clipRef)", "an audio region in the project (its ref)"),
    ("an audio clip's clipRef", "an audio region's ref"),
    ("(for load_sample and the other sample tools)", "(for make_changes' sample_load)"),
    ("Find presets and devices the producer has:", "Find presets the producer has:"),
];
/// my_sets over Logic's projects, which Kumi counts by their files.
const MY_PROJECTS: &str = "The producer's own Logic Pro projects, as Kumi found them on this Mac (in Logic Pro's own folder, and the ones it opened lately), each counted by its file: Kumi doesn't read inside a project. Find projects by words in their names, best and newest first; or give set (a path from a result, or a project's name) for where that project is and when it was saved. Open one in Logic Pro to look inside it.";

/// The library tools in Logic's words, over what the profile's words made of Live's: a region where Live has a clip,
/// the change that loads a sample, its presets' kinds, and its projects counted by their files (Kumi doesn't read
/// inside one, so there's no track, tempo or key in it to find or filter by).
pub fn tool_words(tool: &str, description: &mut String, schema: &mut JsonObject) {
    let said = |text: &str| TOOL_WORDS.iter().fold(text.to_owned(), |text, (live, logic)| text.replace(live, logic));
    *description = said(description);
    let Some(properties) = schema.get_mut("properties").and_then(Value::as_object_mut) else { return };
    for property in properties.values_mut() {
        if let Some(text) = property.get("description").and_then(Value::as_str).map(said) {
            property["description"] = json!(text);
        }
    }
    match tool {
        FIND_PRESETS_TOOL => {
            if let Some(kind) = properties.get_mut("kind") {
                kind["enum"] = json!(PRESET_KINDS);
            }
        }
        MY_SETS_TOOL => {
            *description = MY_PROJECTS.into();
            for unread in ["min_tempo", "max_tempo", "key"] {
                properties.remove(unread);
            }
            if let Some(words) = properties.get_mut("words") {
                words["description"] = json!("Every word must be in the project's name");
            }
            if let Some(set) = properties.get_mut("set") {
                set["description"] = json!("A project's path (from a result) or name");
            }
        }
        _ => {}
    }
}

/// Logic's words for its library, as the registry hands them to the library.
pub const LIBRARY: crate::library::sources::DawLibrary = crate::library::sources::DawLibrary {
    daw: DawId::Logic,
    sample_extensions: SAMPLE_EXTENSIONS,
    preset_extensions: PRESET_EXTENSIONS,
    preset_packages: PRESET_PACKAGES,
    project_package: PROJECT_EXTENSION,
    sources,
    recent,
    project_folders,
    preset_facts,
    tool_words,
};
