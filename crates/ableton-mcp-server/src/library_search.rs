//! Opt-in read-only query surface over Live's own library database
//! (Live-files-*.db and Live-plugins-*.db), issue #54.
//!
//! Schema evidence is enumerated explicitly and fail-closed:
//! - Files database version 12300 (platform 2) was shape-probed first-hand on
//!   Live 12.4.5 (macOS): tables files, keywords, ancestors, places, metadata,
//!   metadata_values, vfolders, devices, file_devices, fe_values, version.
//!   Tags are file rows under the `<keywords>` root; keywords(file_id, keyw_id,
//!   is_auto) links content to them. file_type is a big-endian fourcc.
//! - Plugins database version 1: plugins, plugin_modules, plugin_domains with
//!   dev_identifier-driven format classification (device:vst3:, device:vst:,
//!   device:au:), shape-probed against the same install.
//! Any other version reports `unsupported` with the observed version rather
//! than guessing. The schema is undocumented and unofficial; results are
//! labeled discovery evidence, and loadability still flows through
//! live_browser_inspect identity fencing.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use base64::Engine;
use kumi_common::js::json::stringify;
use kumi_common::js::number::is_safe_integer;
use kumi_common::js::string::utf16_len;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::sqlite_reader::{SqliteError, SqliteReader, SqliteRow, SqliteValue};

pub const LIBRARY_SEARCH_SCHEMA: &str = "ableton-mcp-library-search/v1";
pub const SUPPORTED_FILES_SCHEMA_VERSIONS: &[i64] = &[12300];
pub const SUPPORTED_PLUGINS_SCHEMA_VERSIONS: &[i64] = &[1];
pub const MAX_LIBRARY_ROWS_SCANNED: usize = 100_000;
pub const MAX_LIBRARY_MATCHES: usize = 1_000;
pub const MAX_LIBRARY_ITEMS: usize = 100;
pub const MAX_TAG_VOCABULARY: usize = 512;

pub const LIBRARY_KINDS: &[&str] =
    &["audio", "midi", "set", "preset", "device-group", "device", "clip", "max-device", "pack", "scale", "other"];

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum LibraryKind {
    Audio,
    Midi,
    Set,
    Preset,
    DeviceGroup,
    Device,
    Clip,
    MaxDevice,
    Pack,
    Scale,
    Other,
}

impl LibraryKind {
    pub fn as_str(self) -> &'static str {
        match self {
            LibraryKind::Audio => "audio",
            LibraryKind::Midi => "midi",
            LibraryKind::Set => "set",
            LibraryKind::Preset => "preset",
            LibraryKind::DeviceGroup => "device-group",
            LibraryKind::Device => "device",
            LibraryKind::Clip => "clip",
            LibraryKind::MaxDevice => "max-device",
            LibraryKind::Pack => "pack",
            LibraryKind::Scale => "scale",
            LibraryKind::Other => "other",
        }
    }

    pub fn parse(text: &str) -> Option<LibraryKind> {
        [
            LibraryKind::Audio,
            LibraryKind::Midi,
            LibraryKind::Set,
            LibraryKind::Preset,
            LibraryKind::DeviceGroup,
            LibraryKind::Device,
            LibraryKind::Clip,
            LibraryKind::MaxDevice,
            LibraryKind::Pack,
            LibraryKind::Scale,
            LibraryKind::Other,
        ]
        .into_iter()
        .find(|kind| kind.as_str() == text)
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum PluginFormat {
    Vst3,
    Vst2,
    Au,
    Clap,
    Unknown,
}

impl PluginFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            PluginFormat::Vst3 => "vst3",
            PluginFormat::Vst2 => "vst2",
            PluginFormat::Au => "au",
            PluginFormat::Clap => "clap",
            PluginFormat::Unknown => "unknown",
        }
    }

    pub fn parse(text: &str) -> Option<PluginFormat> {
        [PluginFormat::Vst3, PluginFormat::Vst2, PluginFormat::Au, PluginFormat::Clap, PluginFormat::Unknown]
            .into_iter()
            .find(|format| format.as_str() == text)
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LibraryTag {
    pub name: String,
    pub path: String,
    pub is_auto: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BrowserCandidate {
    pub item_id: String,
    pub resolution: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LibraryItem {
    pub name: String,
    pub kind: LibraryKind,
    pub kind_evidence: String,
    pub tags: Vec<LibraryTag>,
    pub sources: Vec<String>,
    pub use_count: i64,
    pub mod_date: Option<i64>,
    pub device_class: Option<String>,
    pub browser_candidate: Option<BrowserCandidate>,
    pub discovery_only: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LibraryPluginItem {
    pub name: String,
    pub vendor: Option<String>,
    pub version: Option<String>,
    pub sdk_version: Option<String>,
    pub format: PluginFormat,
    pub format_evidence: String,
    pub enabled: bool,
    pub scanned: bool,
    pub module_basename: Option<String>,
    pub subcategories: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LibraryTagEntry {
    pub path: String,
    pub name: String,
    pub usage_count: i64,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LibraryMode {
    Files,
    Plugins,
    Tags,
}

impl LibraryMode {
    pub fn as_str(self) -> &'static str {
        match self {
            LibraryMode::Files => "files",
            LibraryMode::Plugins => "plugins",
            LibraryMode::Tags => "tags",
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum LibrarySort {
    UseCount,
    Modified,
    Name,
}

impl LibrarySort {
    pub fn as_str(self) -> &'static str {
        match self {
            LibrarySort::UseCount => "useCount",
            LibrarySort::Modified => "modified",
            LibrarySort::Name => "name",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LibraryQuery {
    /// Original validated host values retain JavaScript enum coercion and cursor identity.
    pub host_values: Option<Value>,
    pub mode: LibraryMode,
    pub query: Option<String>,
    pub tags: Option<Vec<String>>,
    pub kinds: Option<Vec<LibraryKind>>,
    pub sources: Option<Vec<String>>,
    pub vendors: Option<Vec<String>>,
    pub formats: Option<Vec<PluginFormat>>,
    pub sort: Option<LibrarySort>,
    pub limit: usize,
    pub cursor: Option<String>,
}

impl LibraryQuery {
    /// A query with only the mode and limit set, as the host builds one before adding filters.
    pub fn new(mode: LibraryMode, limit: usize) -> LibraryQuery {
        LibraryQuery {
            host_values: None,
            mode,
            query: None,
            tags: None,
            kinds: None,
            sources: None,
            vendors: None,
            formats: None,
            sort: None,
            limit,
            cursor: None,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LibraryPaging {
    pub limit: usize,
    pub returned: usize,
    pub total: usize,
    pub complete: bool,
    pub scanned_rows: usize,
    pub truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LibraryPage<T> {
    pub items: Vec<T>,
    pub paging: LibraryPaging,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag_vocabulary_note: Option<String>,
}

/// The library cannot answer: the message is the reason, the details join the tool's error text.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("{message}")]
pub struct LibraryUnavailable {
    pub message: String,
    pub details: Map<String, Value>,
}

impl LibraryUnavailable {
    pub fn new(message: impl Into<String>, details: Value) -> LibraryUnavailable {
        LibraryUnavailable { message: message.into(), details: details.as_object().cloned().unwrap_or_default() }
    }
}

/// What a query throws: `LibraryUnavailable` (reported as `unavailable`), or the reader's own error.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum LibrarySearchError {
    #[error(transparent)]
    Unavailable(#[from] LibraryUnavailable),
    #[error(transparent)]
    Sqlite(#[from] SqliteError),
}

fn unavailable(message: impl Into<String>, details: Value) -> LibrarySearchError {
    LibrarySearchError::Unavailable(LibraryUnavailable::new(message, details))
}

/// JavaScript's default string order: UTF-16 code units.
fn js_str_cmp(a: &str, b: &str) -> Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

fn fourcc(value: Option<i64>) -> String {
    let Some(value) = value.filter(|value| *value >= 0) else { return "????".to_string() };
    let bytes = (value as u32).to_be_bytes();
    // TextDecoder drops a leading byte-order mark.
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes);
    let text = String::from_utf8_lossy(bytes).replace(['-', '\0'], "");
    let text = kumi_common::js::string::trim(&text);
    if text.is_empty() {
        "????".to_string()
    } else {
        text.to_string()
    }
}

struct Classification {
    kind: LibraryKind,
    evidence: String,
}

fn classify_file_type(file_type: Option<i64>, name: &str) -> Classification {
    static AUDIO_EXTENSION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\.(wav|aiff?|flac|mp3|ogg|m4a)$").unwrap());
    static MIDI_EXTENSION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\.mid(i)?$").unwrap());
    let code = fourcc(file_type);
    let classified = |kind: LibraryKind, evidence: String| Classification { kind, evidence };
    match code.as_str() {
        "wav" | "aiff" | "aif" | "flac" | "mp3" | "ogg" | "m4a" => classified(LibraryKind::Audio, format!("file_type '{code}'")),
        "als" => classified(LibraryKind::Set, "file_type 'als'".to_string()),
        "adg" => classified(LibraryKind::DeviceGroup, "file_type 'adg' (Ableton Device Group)".to_string()),
        "adv" => classified(LibraryKind::Preset, "file_type 'adv' (Ableton device preset)".to_string()),
        "alc" => classified(LibraryKind::Clip, "file_type 'alc' (Ableton Live clip)".to_string()),
        "amp" => classified(LibraryKind::MaxDevice, "file_type 'amp' (Max for Live device)".to_string()),
        "dfld" => classified(LibraryKind::Device, "file_type 'dfld' (browser device entry)".to_string()),
        "alp" | "apck" => classified(LibraryKind::Pack, format!("file_type '{code}'")),
        "scl" => classified(LibraryKind::Scale, "file_type 'scl'".to_string()),
        _ => {
            if AUDIO_EXTENSION.is_match(name) {
                return classified(LibraryKind::Audio, format!("name extension (file_type '{code}' unclassified)"));
            }
            if MIDI_EXTENSION.is_match(name) {
                return classified(LibraryKind::Midi, format!("name extension (file_type '{code}' unclassified)"));
            }
            classified(LibraryKind::Other, format!("file_type '{code}' is not classified in this build"))
        }
    }
}

fn device_category(device_id: &str) -> Option<&'static str> {
    static INSTRUMENTS: LazyLock<Regex> = LazyLock::new(|| Regex::new("^device:[^:]*:instr:").unwrap());
    static AUDIO_EFFECTS: LazyLock<Regex> = LazyLock::new(|| Regex::new("^device:[^:]*:audiofx:").unwrap());
    static MIDI_EFFECTS: LazyLock<Regex> = LazyLock::new(|| Regex::new("^device:[^:]*:midifx:").unwrap());
    if INSTRUMENTS.is_match(device_id) {
        return Some("instruments");
    }
    if AUDIO_EFFECTS.is_match(device_id) {
        return Some("audio_effects");
    }
    if MIDI_EFFECTS.is_match(device_id) {
        return Some("midi_effects");
    }
    None
}

struct FormatEvidence {
    format: PluginFormat,
    evidence: String,
}

fn plugin_format(dev_identifier: Option<&SqliteValue>) -> FormatEvidence {
    let Some(SqliteValue::Text(dev_identifier)) = dev_identifier else {
        return FormatEvidence { format: PluginFormat::Unknown, evidence: "no dev_identifier".to_string() };
    };
    let evidence = |format: PluginFormat, evidence: &str| FormatEvidence { format, evidence: evidence.to_string() };
    if dev_identifier.starts_with("device:vst3:") {
        return evidence(PluginFormat::Vst3, "dev_identifier prefix device:vst3:");
    }
    if dev_identifier.starts_with("device:vst:") {
        return evidence(PluginFormat::Vst2, "dev_identifier prefix device:vst:");
    }
    if dev_identifier.starts_with("device:au:") {
        return evidence(PluginFormat::Au, "dev_identifier prefix device:au:");
    }
    if dev_identifier.starts_with("device:clap:") {
        return evidence(PluginFormat::Clap, "dev_identifier prefix device:clap:");
    }
    FormatEvidence {
        format: PluginFormat::Unknown,
        evidence: format!("dev_identifier prefix is not classified ({})", kumi_common::js::string::head(dev_identifier, 32)),
    }
}

fn require_columns(reader: &SqliteReader, table: &str, columns: &[&str]) -> Result<(), LibrarySearchError> {
    let Some(present) = reader.table_columns(table) else {
        return Err(unavailable(format!("the library database is missing the {table} table"), json!({ "table": table })));
    };
    let missing: Vec<&str> = columns.iter().copied().filter(|column| !present.iter().any(|name| name == column)).collect();
    if !missing.is_empty() {
        return Err(unavailable(
            format!("the library database {table} table lacks expected columns"),
            json!({ "table": table, "missing": missing, "present": present }),
        ));
    }
    Ok(())
}

fn row_number(row: &SqliteRow, index: usize) -> Option<i64> {
    match row.get(index)? {
        SqliteValue::Integer(value) => Some(*value),
        SqliteValue::Real(value) if is_safe_integer(*value) => Some(*value as i64),
        _ => None,
    }
}

fn row_text(row: &SqliteRow, index: usize) -> Option<String> {
    row.get(index)?.as_text().map(str::to_string)
}

struct FileRow {
    file_id: i64,
    parent_id: Option<i64>,
    file_type: Option<i64>,
    mod_date: Option<i64>,
    name: String,
    use_count: i64,
    place_id: Option<i64>,
    device_id: Option<String>,
}

mod files_columns {
    pub const FILE_ID: usize = 0;
    pub const PARENT_ID: usize = 1;
    pub const FILE_TYPE: usize = 2;
    pub const MOD_DATE: usize = 5;
    pub const NAME: usize = 8;
    pub const USE_COUNT: usize = 12;
    pub const PLACE_ID: usize = 13;
    pub const DEVICE_ID: usize = 17;
}

fn read_library_files(reader: &SqliteReader) -> Result<Vec<FileRow>, LibrarySearchError> {
    require_columns(reader, "files", &["file_id", "parent_id", "file_type", "mod_date", "name", "use_count", "place_id", "device_id"])?;
    let mut rows: Vec<FileRow> = Vec::new();
    for scanned in reader.scan_table("files", MAX_LIBRARY_ROWS_SCANNED)? {
        let row = &scanned.row;
        // file_id is an INTEGER PRIMARY KEY alias: the record stores NULL and the
        // authoritative value is the cell rowid.
        let file_id = row_number(row, files_columns::FILE_ID).unwrap_or(scanned.row_id);
        let Some(name) = row_text(row, files_columns::NAME) else {
            return Err(unavailable("the files table contains malformed identity rows", json!({})));
        };
        rows.push(FileRow {
            file_id,
            parent_id: row_number(row, files_columns::PARENT_ID),
            file_type: row_number(row, files_columns::FILE_TYPE),
            mod_date: row_number(row, files_columns::MOD_DATE),
            name,
            use_count: row_number(row, files_columns::USE_COUNT).unwrap_or(0),
            place_id: row_number(row, files_columns::PLACE_ID),
            device_id: row_text(row, files_columns::DEVICE_ID),
        });
    }
    Ok(rows)
}

fn schema_version(reader: &SqliteReader) -> Result<i64, LibrarySearchError> {
    require_columns(reader, "version", &["version", "platform"])?;
    let rows = reader.scan_table("version", 4)?;
    if rows.len() != 1 {
        return Err(unavailable("the library database version row is missing or ambiguous", json!({})));
    }
    row_number(&rows[0].row, 0).ok_or_else(|| unavailable("the library database version is unreadable", json!({})))
}

pub fn assert_supported_files_schema(reader: &SqliteReader) -> Result<i64, LibrarySearchError> {
    let version = schema_version(reader)?;
    if !SUPPORTED_FILES_SCHEMA_VERSIONS.contains(&version) {
        return Err(unavailable(
            "the library database schema version is not enumerated in this build",
            json!({ "observedVersion": version, "supportedVersions": SUPPORTED_FILES_SCHEMA_VERSIONS }),
        ));
    }
    require_columns(reader, "keywords", &["file_id", "keyw_id", "is_auto"])?;
    require_columns(reader, "ancestors", &["file_id", "ancestor_id"])?;
    require_columns(reader, "places", &["file_id", "folder_kind", "level", "name"])?;
    Ok(version)
}

pub fn assert_supported_plugins_schema(reader: &SqliteReader) -> Result<i64, LibrarySearchError> {
    let version = schema_version(reader)?;
    if !SUPPORTED_PLUGINS_SCHEMA_VERSIONS.contains(&version) {
        return Err(unavailable(
            "the plug-in database schema version is not enumerated in this build",
            json!({ "observedVersion": version, "supportedVersions": SUPPORTED_PLUGINS_SCHEMA_VERSIONS }),
        ));
    }
    require_columns(
        reader,
        "plugins",
        &["plugin_id", "module_id", "dev_identifier", "name", "vendor", "version", "sdk_version", "scanstate", "enabled"],
    )?;
    Ok(version)
}

struct TagIndex {
    tags_by_file: HashMap<i64, Vec<LibraryTag>>,
    vocabulary: Vec<LibraryTagEntry>,
}

/// Deeper than any real tag tree: a keyword more than this many levels below the root has no path.
const MAX_TAG_DEPTH: usize = 512;
/// The longest path the index keeps, in UTF-16 units.
const MAX_TAG_PATH: usize = 512;
/// A keyword's path below the keywords root ("Drums|Kick"), walked up its parents without recursion. Each id's answer
/// (its path and depth) is kept in `known` for the whole vocabulary, so a shared ancestor is walked once. None for a
/// chain that never reaches the root (a missing or zero parent, or a loop) or reaches it past MAX_TAG_DEPTH levels.
fn tag_path(
    by_id: &HashMap<i64, &FileRow>,
    keyword_root: Option<i64>,
    file_id: i64,
    known: &mut HashMap<i64, Option<(String, usize)>>,
) -> Option<String> {
    let mut pending: Vec<(i64, &FileRow)> = Vec::new();
    let mut walked = HashSet::new();
    let mut at = file_id;
    let mut path = loop {
        if let Some(found) = known.get(&at) {
            break found.clone();
        }
        if !walked.insert(at) {
            // A loop: none of these reaches the root.
            break None;
        }
        let Some(row) = by_id.get(&at) else {
            known.insert(at, None);
            break None;
        };
        if keyword_root == Some(row.file_id) {
            known.insert(at, Some((String::new(), 0)));
            break Some((String::new(), 0));
        }
        let Some(parent) = row.parent_id.filter(|parent| *parent != 0) else {
            known.insert(at, None);
            break None;
        };
        if pending.len() >= MAX_TAG_DEPTH {
            // Too deep from here, whatever the nodes above are: nothing is kept, so each gets its own answer.
            return None;
        }
        pending.push((at, row));
        at = parent;
    };
    for (id, row) in pending.into_iter().rev() {
        path = path.filter(|(_, depth)| *depth < MAX_TAG_DEPTH).and_then(|(parent, depth)| {
            // Longer than the index keeps, and every path below it is longer still: none, so `known` never holds a
            // long path (measured before it's made, so a long name isn't copied only to be dropped).
            let name = utf16_len(&row.name);
            let length = if parent.is_empty() { name } else { utf16_len(&parent) + 1 + name };
            (length <= MAX_TAG_PATH)
                .then(|| (if parent.is_empty() { row.name.clone() } else { format!("{parent}|{}", row.name) }, depth + 1))
        });
        known.insert(id, path.clone());
    }
    path.map(|(path, _)| path)
}

fn leaf_name(path: &str) -> String {
    path.rsplit('|').next().unwrap_or(path).to_string()
}

fn build_tag_index(reader: &SqliteReader, files: &[FileRow]) -> Result<TagIndex, LibrarySearchError> {
    let by_id: HashMap<i64, &FileRow> = files.iter().map(|row| (row.file_id, row)).collect();
    let keyword_root = files.iter().find(|row| row.name == "<keywords>" && matches!(row.parent_id, None | Some(0))).map(|row| row.file_id);
    // Tag files in first-seen order, as a Map keeps them.
    let mut tag_files: Vec<(i64, String)> = Vec::new();
    let mut tag_file_index: HashMap<i64, usize> = HashMap::new();
    let mut known = HashMap::new();
    for row in files {
        if fourcc(row.file_type) != "keyw" {
            continue;
        }
        let path = tag_path(&by_id, keyword_root, row.file_id, &mut known);
        if let Some(path) = path {
            let length = utf16_len(&path);
            if length > 0 && length <= MAX_TAG_PATH {
                match tag_file_index.get(&row.file_id) {
                    Some(index) => tag_files[*index].1 = path,
                    None => {
                        tag_file_index.insert(row.file_id, tag_files.len());
                        tag_files.push((row.file_id, path));
                    }
                }
            }
        }
    }
    let mut tags_by_file: HashMap<i64, Vec<LibraryTag>> = HashMap::new();
    let mut usage: HashMap<i64, i64> = HashMap::new();
    for scanned in reader.scan_table("keywords", MAX_LIBRARY_ROWS_SCANNED)? {
        let row = &scanned.row;
        let (Some(file_id), Some(keyw_id)) = (row_number(row, 0), row_number(row, 1)) else { continue };
        let is_auto = row_number(row, 2) == Some(1);
        let Some(index) = tag_file_index.get(&keyw_id) else { continue };
        let path = tag_files[*index].1.clone();
        let name = leaf_name(&path);
        tags_by_file.entry(file_id).or_default().push(LibraryTag { name, path, is_auto });
        *usage.entry(keyw_id).or_insert(0) += 1;
    }
    let mut vocabulary: Vec<LibraryTagEntry> = tag_files
        .iter()
        .map(|(keyw_id, path)| LibraryTagEntry {
            path: path.clone(),
            name: leaf_name(path),
            usage_count: usage.get(keyw_id).copied().unwrap_or(0),
        })
        .collect();
    vocabulary.sort_by(|a, b| js_str_cmp(&a.path, &b.path));
    if vocabulary.len() > MAX_TAG_VOCABULARY {
        return Err(unavailable("the tag vocabulary exceeds its bound", json!({ "bound": MAX_TAG_VOCABULARY })));
    }
    Ok(TagIndex { tags_by_file, vocabulary })
}

fn place_map(reader: &SqliteReader) -> Result<HashMap<i64, String>, LibrarySearchError> {
    let mut map: HashMap<i64, String> = HashMap::new();
    for scanned in reader.scan_table("places", MAX_LIBRARY_ROWS_SCANNED)? {
        if let (Some(file_id), Some(name)) = (row_number(&scanned.row, 0), row_text(&scanned.row, 3)) {
            map.insert(file_id, name);
        }
    }
    Ok(map)
}

fn wildcard_to_regex(query: &str) -> Regex {
    let mut escaped = String::new();
    for char in query.chars() {
        match char {
            '.' | '+' | '^' | '$' | '{' | '}' | '(' | ')' | '|' | '[' | ']' | '\\' => {
                escaped.push('\\');
                escaped.push(char);
            }
            '*' => escaped.push_str(".*"),
            '?' => escaped.push('.'),
            _ => escaped.push(char),
        }
    }
    Regex::new(&format!("(?i)^{escaped}$")).unwrap_or_else(|_| Regex::new(r"[^\s\S]").unwrap())
}

/// `Buffer.from(text, "base64url")`: either alphabet, other characters skipped, a `=` ends the input.
fn node_base64_decode(text: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let mut accumulator: u32 = 0;
    let mut bits = 0;
    for char in text.chars() {
        let value = match char {
            'A'..='Z' => char as u32 - 'A' as u32,
            'a'..='z' => char as u32 - 'a' as u32 + 26,
            '0'..='9' => char as u32 - '0' as u32 + 52,
            '+' | '-' => 62,
            '/' | '_' => 63,
            '=' => break,
            _ => continue,
        };
        accumulator = (accumulator << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((accumulator >> bits) as u8);
            accumulator &= (1 << bits) - 1;
        }
    }
    out
}

/// `items.slice(start, end)`.
fn js_slice_bounds(length: usize, start: i64, end: i64) -> (usize, usize) {
    let length = length as i64;
    let clamp = |index: i64| if index < 0 { (length + index).max(0) } else { index.min(length) };
    let from = clamp(start);
    let to = clamp(end).max(from);
    (from as usize, to as usize)
}

fn page_ids<T: Serialize>(
    items: Vec<T>,
    id_of: impl Fn(&T) -> &str,
    query: &LibraryQuery,
    sort: LibrarySort,
    revision_seed: &str,
    scanned_rows: usize,
) -> Result<LibraryPage<T>, LibrarySearchError> {
    let limit = query.limit;
    let mut offset: i64 = 0;
    let mut revision_input = Map::new();
    revision_input.insert("seed".to_string(), Value::String(revision_seed.to_string()));
    if let Some(first) = items.first() {
        revision_input.insert("ids".to_string(), Value::String(id_of(first).to_string()));
    }
    revision_input.insert("count".to_string(), Value::from(items.len()));
    revision_input.insert(
        "sort".to_string(),
        if query.mode == LibraryMode::Tags {
            json!("name")
        } else {
            query.host_values.as_ref().and_then(|v| v.get("sort")).cloned().unwrap_or(json!(sort.as_str()))
        },
    );
    revision_input
        .insert("mode".to_string(), query.host_values.as_ref().and_then(|v| v.get("mode")).cloned().unwrap_or(json!(query.mode.as_str())));
    let revision = hex::encode(Sha256::digest(stringify(&Value::Object(revision_input)).as_bytes()));
    if let Some(cursor) = &query.cursor {
        let decoded: Value = serde_json::from_str(&String::from_utf8_lossy(&node_base64_decode(cursor)))
            .map_err(|_| unavailable("the paging cursor is invalid", json!({})))?;
        let decoded_offset = match &decoded {
            Value::Object(object) if object.get("revision").and_then(Value::as_str) == Some(revision.as_str()) => {
                object.get("offset").and_then(Value::as_f64).filter(|offset| is_safe_integer(*offset))
            }
            _ => None,
        };
        let Some(decoded_offset) = decoded_offset else {
            return Err(unavailable("the paging cursor is stale; request a fresh first page", json!({})));
        };
        offset = (decoded_offset as i64).min(items.len() as i64);
    }
    let total = items.len();
    let (from, to) = js_slice_bounds(total, offset, offset + limit as i64);
    let page: Vec<T> = items.into_iter().skip(from).take(to - from).collect();
    let next_offset = offset + page.len() as i64;
    let complete = next_offset >= total as i64;
    let next_cursor = if complete {
        None
    } else {
        Some(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(stringify(&json!({ "revision": revision, "offset": next_offset }))))
    };
    Ok(LibraryPage {
        paging: LibraryPaging {
            limit,
            returned: page.len(),
            total,
            complete,
            scanned_rows,
            truncated: total >= MAX_LIBRARY_MATCHES,
            next_cursor,
        },
        items: page,
        tag_vocabulary_note: None,
    })
}

fn sorted_unique(values: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut unique: Vec<String> = Vec::new();
    for value in values {
        if !unique.contains(&value) {
            unique.push(value);
        }
    }
    unique.sort_by(|a, b| js_str_cmp(a, b));
    unique
}

fn host_enum_seed(query: &LibraryQuery, key: &str) -> Option<String> {
    let raw = query.host_values.as_ref()?.get(key)?.as_array()?;
    let mut unique: Vec<&Value> = Vec::new();
    for value in raw {
        if value.is_array() || value.is_object() || !unique.contains(&value) {
            unique.push(value);
        }
    }
    let mut strings: Vec<String> =
        unique.into_iter().map(|v| crate::host::helpers::js_string(v).expect("validated library enum")).collect();
    strings.sort_by(|a, b| js_str_cmp(a, b));
    Some(strings.join(","))
}

pub fn query_library_files(reader: &SqliteReader, query: &LibraryQuery) -> Result<LibraryPage<LibraryItem>, LibrarySearchError> {
    let files = read_library_files(reader)?;
    let TagIndex { tags_by_file, .. } = build_tag_index(reader, &files)?;
    let places = place_map(reader)?;
    let excluded_fourccs = ["keyw", "fldr", "pref"];
    let query_text = query.query.as_deref().filter(|text| !text.is_empty());
    let name_matcher = query_text.filter(|text| text.contains('*') || text.contains('?')).map(wildcard_to_regex);
    let lowered_query = query_text.map(str::to_lowercase);
    let requested_tags: Vec<String> = query.tags.iter().flatten().map(|tag| tag.to_lowercase()).collect();
    let requested_kinds: Option<HashSet<LibraryKind>> =
        query.kinds.as_ref().filter(|kinds| !kinds.is_empty()).map(|kinds| kinds.iter().copied().collect());
    let requested_sources: Option<HashSet<String>> =
        query.sources.as_ref().filter(|sources| !sources.is_empty()).map(|sources| sources.iter().cloned().collect());
    let mut matched: Vec<LibraryItem> = Vec::new();
    let mut scanned_rows = 0;
    for row in &files {
        scanned_rows += 1;
        if matched.len() >= MAX_LIBRARY_MATCHES {
            break;
        }
        let code = fourcc(row.file_type);
        if excluded_fourccs.contains(&code.as_str()) {
            continue;
        }
        let classification = classify_file_type(row.file_type, &row.name);
        if query.host_values.as_ref().and_then(|v| v["kinds"].as_array()).filter(|a| !a.is_empty()).map_or_else(
            || requested_kinds.as_ref().is_some_and(|kinds| !kinds.contains(&classification.kind)),
            |a| !a.iter().any(|v| v == classification.kind.as_str()),
        ) {
            continue;
        }
        let name_rejected = match (&name_matcher, &lowered_query) {
            (Some(matcher), _) => !matcher.is_match(&row.name),
            (None, Some(lowered)) => !row.name.to_lowercase().contains(lowered.as_str()),
            (None, None) => false,
        };
        if name_rejected {
            continue;
        }
        let tags = tags_by_file.get(&row.file_id).cloned().unwrap_or_default();
        if !requested_tags.is_empty() {
            let owned: HashSet<String> = tags.iter().flat_map(|tag| [tag.name.to_lowercase(), tag.path.to_lowercase()]).collect();
            if !requested_tags.iter().all(|tag| owned.contains(tag)) {
                continue;
            }
        }
        let source_name = row.place_id.and_then(|place_id| places.get(&place_id));
        if let Some(sources) = &requested_sources {
            if !source_name.is_some_and(|name| sources.contains(name)) {
                continue;
            }
        }
        let category = row.device_id.as_deref().and_then(device_category);
        let browser_candidate = match category {
            Some(category) if matches!(classification.kind, LibraryKind::Device | LibraryKind::DeviceGroup | LibraryKind::Preset) => Some(BrowserCandidate {
                item_id: format!("{category}/{}", row.name),
                resolution: "candidate from the library row's name and device class; loadability still requires a fresh live_browser_inspect result".to_string(),
            }),
            _ => None,
        };
        matched.push(LibraryItem {
            name: row.name.clone(),
            kind: classification.kind,
            kind_evidence: classification.evidence,
            tags,
            sources: source_name.map(|name| vec![name.clone()]).unwrap_or_default(),
            use_count: row.use_count,
            mod_date: row.mod_date,
            device_class: row.device_id.clone(),
            discovery_only: browser_candidate.is_none(),
            browser_candidate,
        });
    }
    let sort = query.sort.unwrap_or(LibrarySort::UseCount);
    matched.sort_by(|a, b| {
        if sort == LibrarySort::UseCount && a.use_count != b.use_count {
            return b.use_count.cmp(&a.use_count);
        }
        if sort == LibrarySort::Modified && a.mod_date.unwrap_or(0) != b.mod_date.unwrap_or(0) {
            return b.mod_date.unwrap_or(0).cmp(&a.mod_date.unwrap_or(0));
        }
        js_str_cmp(&a.name, &b.name)
    });
    let revision_seed = format!(
        "files|{}|{}|{}|{}",
        query.query.as_deref().unwrap_or(""),
        requested_tags.join(","),
        host_enum_seed(query, "kinds")
            .unwrap_or_else(|| sorted_unique(requested_kinds.iter().flatten().map(|kind| kind.as_str().to_string())).join(",")),
        sorted_unique(requested_sources.iter().flatten().cloned()).join(",")
    );
    let mut page = page_ids(matched, |item| &item.name, query, sort, &revision_seed, scanned_rows)?;
    page.tag_vocabulary_note = Some("tag filters accept a leaf name (\"Delay\") or a full path (\"Devices|Synthesizer|FM\") from the tag vocabulary; use mode=tags to list it".to_string());
    Ok(page)
}

pub fn query_library_plugins(reader: &SqliteReader, query: &LibraryQuery) -> Result<LibraryPage<LibraryPluginItem>, LibrarySearchError> {
    require_columns(reader, "plugin_modules", &["module_id", "path"])?;
    let rows = reader.scan_table("plugins", MAX_LIBRARY_ROWS_SCANNED)?;
    let mut modules: HashMap<i64, String> = HashMap::new();
    for scanned in reader.scan_table("plugin_modules", MAX_LIBRARY_ROWS_SCANNED)? {
        let module_id = row_number(&scanned.row, 0).unwrap_or(scanned.row_id);
        if let Some(path) = row_text(&scanned.row, 1) {
            modules.insert(module_id, path.rsplit(['\\', '/']).next().unwrap_or(&path).to_string());
        }
    }
    let requested_vendors: Option<HashSet<String>> = query
        .vendors
        .as_ref()
        .filter(|vendors| !vendors.is_empty())
        .map(|vendors| vendors.iter().map(|vendor| vendor.to_lowercase()).collect());
    let requested_formats: Option<HashSet<PluginFormat>> =
        query.formats.as_ref().filter(|formats| !formats.is_empty()).map(|formats| formats.iter().copied().collect());
    let lowered_query = query.query.as_deref().filter(|text| !text.is_empty()).map(str::to_lowercase);
    let mut items: Vec<LibraryPluginItem> = Vec::new();
    let mut scanned_rows = 0;
    for scanned in &rows {
        scanned_rows += 1;
        if items.len() >= MAX_LIBRARY_MATCHES {
            break;
        }
        let row = &scanned.row;
        let Some(name) = row_text(row, 3) else { continue };
        if lowered_query.as_ref().is_some_and(|lowered| !name.to_lowercase().contains(lowered.as_str())) {
            continue;
        }
        let vendor = row_text(row, 4);
        if let Some(vendors) = &requested_vendors {
            if !vendor.as_ref().is_some_and(|vendor| vendors.contains(&vendor.to_lowercase())) {
                continue;
            }
        }
        let format = plugin_format(row.get(2));
        if query.host_values.as_ref().and_then(|v| v["formats"].as_array()).filter(|a| !a.is_empty()).map_or_else(
            || requested_formats.as_ref().is_some_and(|formats| !formats.contains(&format.format)),
            |a| !a.iter().any(|v| v == format.format.as_str()),
        ) {
            continue;
        }
        let module_id = row_number(row, 1);
        items.push(LibraryPluginItem {
            name,
            vendor,
            version: row_text(row, 5),
            sdk_version: row_text(row, 6),
            format: format.format,
            format_evidence: format.evidence,
            enabled: row_number(row, 9) == Some(1),
            scanned: row_number(row, 7) == Some(1),
            module_basename: module_id.and_then(|module_id| modules.get(&module_id).cloned()),
            subcategories: row_text(row, 10),
        });
    }
    items.sort_by(|a, b| js_str_cmp(&a.name, &b.name));
    let revision_seed = format!(
        "plugins|{}|{}|{}",
        query.query.as_deref().unwrap_or(""),
        sorted_unique(requested_vendors.iter().flatten().cloned()).join(","),
        host_enum_seed(query, "formats")
            .unwrap_or_else(|| sorted_unique(requested_formats.iter().flatten().map(|format| format.as_str().to_string())).join(","))
    );
    page_ids(items, |item| &item.name, query, query.sort.unwrap_or(LibrarySort::UseCount), &revision_seed, scanned_rows)
}

pub fn query_library_tag_vocabulary(
    reader: &SqliteReader,
    query: &LibraryQuery,
) -> Result<LibraryPage<LibraryTagEntry>, LibrarySearchError> {
    let files = read_library_files(reader)?;
    let TagIndex { vocabulary, .. } = build_tag_index(reader, &files)?;
    let filtered: Vec<LibraryTagEntry> = match query.query.as_deref().filter(|text| !text.is_empty()) {
        None => vocabulary,
        Some(text) => {
            let lowered = text.to_lowercase();
            vocabulary.into_iter().filter(|entry| entry.path.to_lowercase().contains(&lowered)).collect()
        }
    };
    let scanned_rows = filtered.len();
    page_ids(
        filtered,
        |entry| &entry.path,
        query,
        LibrarySort::Name,
        &format!("tags|{}", query.query.as_deref().unwrap_or("")),
        scanned_rows,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn row(file_id: i64, parent_id: Option<i64>, name: &str) -> FileRow {
        FileRow { file_id, parent_id, file_type: None, mod_date: None, name: name.into(), use_count: 0, place_id: None, device_id: None }
    }
    #[test]
    fn a_tag_path_walks_its_parents_without_recursion_and_the_same_way_in_any_order() {
        // The keywords root, "Drums|Kick", then 100,000 levels of "x" below Kick (id k's path is "Drums|Kick" and k - 3
        // "|x"), a chain of 600 empty names below the root (ids 400,001 on), whose path never grows, and a loop.
        let mut rows = vec![row(1, None, "<keywords>"), row(2, Some(1), "Drums"), row(3, Some(2), "Kick")];
        rows.extend((4..100_004).map(|id| row(id, Some(id - 1), "x")));
        rows.extend((400_001..400_601).map(|id| row(id, Some(if id == 400_001 { 1 } else { id - 1 }), "")));
        rows.extend([row(200_000, Some(200_001), "a"), row(200_001, Some(200_000), "b"), row(300_000, Some(0), "orphan")]);
        let by_id: HashMap<i64, &FileRow> = rows.iter().map(|row| (row.file_id, row)).collect();
        let length = |path: Option<String>| path.map(|path| utf16_len(&path));
        for order in
            [[3, 100_003, 254, 255, 400_512, 400_513, 200_000, 300_000], [400_513, 400_512, 255, 254, 300_000, 200_000, 100_003, 3]]
        {
            let mut known = HashMap::new();
            let answers: HashMap<i64, Option<usize>> =
                order.iter().map(|id| (*id, length(tag_path(&by_id, Some(1), *id, &mut known)))).collect();
            assert_eq!(tag_path(&by_id, Some(1), 3, &mut known).as_deref(), Some("Drums|Kick"));
            // A path of 512 units is kept; one longer, and every keyword below it (100,002 levels down here, which the
            // recursion overflowed the stack on), has none, and none is kept in `known`.
            assert_eq!((answers[&254], answers[&255], answers[&100_003]), (Some(512), None, None), "{order:?}");
            assert!(known.values().flatten().all(|(path, _)| utf16_len(path) <= MAX_TAG_PATH));
            // Depth still bounds a path that doesn't grow: 512 levels have one, 513 don't.
            assert_eq!((answers[&400_512], answers[&400_513]), (Some(0), None), "{order:?}");
            assert_eq!((answers[&200_000], answers[&300_000]), (None, None), "{order:?}");
        }
    }
}
