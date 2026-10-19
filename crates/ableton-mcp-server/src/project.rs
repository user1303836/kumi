//! Host-side project file operations. The current set file is read only after
//! its identity is proven through the authenticated bridge; backups are written
//! only into the set's own directory with atomic replacement and sha256
//! verification. Referenced media is checked for existence only (metadata),
//! never read.

use std::cmp::Ordering;
use std::collections::HashSet;
use std::fs;
use std::io::Read;
use std::path::Path;
use std::sync::LazyLock;

use flate2::read::MultiGzDecoder;
use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// A refused or failed project operation; the text is the TypeScript's.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ProjectError(pub String);

fn fail(message: impl Into<String>) -> ProjectError {
    ProjectError(message.into())
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectManifest {
    pub path: String,
    pub size: u64,
    pub mtime_ms: f64,
    pub sha256: String,
    pub tracks: usize,
    pub scenes: usize,
    pub media_refs: usize,
}

/// `projectInfo`'s result: the manifest, the missing media, and `exists: true`, in that order.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectInfo {
    pub path: String,
    pub size: u64,
    pub mtime_ms: f64,
    pub sha256: String,
    pub tracks: usize,
    pub scenes: usize,
    pub media_refs: usize,
    pub missing_media: Vec<String>,
    pub exists: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct AbletonRootAttributes {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub creator: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub major_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minor_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema_change_count: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ReferenceResolution {
    Absolute,
    SetRelative,
    Unresolved,
    Network,
    Oversized,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectReference {
    pub value: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exists: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_local: Option<bool>,
    pub resolution: ReferenceResolution,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ObservedKind {
    Exact,
    LowerBound,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceBounds {
    pub observed: usize,
    pub observed_kind: ObservedKind,
    pub included: usize,
    pub omitted: usize,
    pub complete: bool,
}

/// Internal read-only evidence used by semantic Set exports. Paths remain
/// host-local here and must be policy-redacted before they cross the MCP
/// boundary. Referenced files are never opened.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSourceEvidence {
    pub manifest: ProjectManifest,
    pub ableton: AbletonRootAttributes,
    pub references: Vec<ProjectReference>,
    pub reference_bounds: ReferenceBounds,
}

const MAX_SET_BYTES: u64 = 64 * 1024 * 1024;
const MAX_FILE_REFERENCES: usize = 4096;
const MAX_FILE_REFERENCE_LENGTH: usize = 4096;

#[cfg(windows)]
const SEPARATORS: &[char] = &['\\', '/'];
#[cfg(not(windows))]
const SEPARATORS: &[char] = &['/'];

const SEP: char = std::path::MAIN_SEPARATOR;

/// JavaScript's default string order: UTF-16 code units.
fn js_str_cmp(a: &str, b: &str) -> Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// `path.isAbsolute`.
fn is_absolute(path: &str) -> bool {
    #[cfg(windows)]
    {
        let bytes = path.as_bytes();
        let separator = |byte: u8| byte == b'\\' || byte == b'/';
        !bytes.is_empty()
            && (separator(bytes[0]) || (bytes.len() > 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && separator(bytes[2])))
    }
    #[cfg(not(windows))]
    {
        path.starts_with('/')
    }
}

/// `path.resolve(path)`: normalized, from the working directory when relative.
fn resolve(path: &str) -> String {
    #[cfg(windows)]
    {
        std::path::absolute(path).map(|resolved| resolved.to_string_lossy().into_owned()).unwrap_or_else(|_| path.to_string())
    }
    #[cfg(not(windows))]
    {
        let joined = if path.starts_with('/') {
            path.to_string()
        } else {
            format!("{}/{path}", std::env::current_dir().map(|dir| dir.to_string_lossy().into_owned()).unwrap_or_default())
        };
        let mut parts: Vec<&str> = Vec::new();
        for part in joined.split('/') {
            match part {
                "" | "." => {}
                ".." => {
                    parts.pop();
                }
                other => parts.push(other),
            }
        }
        format!("/{}", parts.join("/"))
    }
}

/// `path.extname`.
fn extname(path: &str) -> String {
    let name = path.trim_end_matches(SEPARATORS).rsplit(SEPARATORS).next().unwrap_or("");
    if name == ".." {
        return String::new();
    }
    match name.rfind('.') {
        None | Some(0) => String::new(),
        Some(index) => name[index..].to_string(),
    }
}

/// `path.dirname`.
fn dirname(path: &str) -> String {
    Path::new(path)
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(|parent| parent.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

/// `path.basename`.
fn basename(path: &str) -> String {
    Path::new(path).file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default()
}

/// `path.join(directory, name)`.
fn join(directory: &str, name: &str) -> String {
    Path::new(directory).join(name).to_string_lossy().into_owned()
}

/// A Node `fs` error's text: `ENOENT: no such file or directory, lstat '/x'`.
fn node_error(error: &std::io::Error, syscall: &str, paths: &[&str]) -> ProjectError {
    use std::io::ErrorKind;
    let named = match error.kind() {
        ErrorKind::NotFound => Some(("ENOENT", "no such file or directory")),
        ErrorKind::PermissionDenied => Some(("EACCES", "permission denied")),
        ErrorKind::AlreadyExists => Some(("EEXIST", "file already exists")),
        ErrorKind::NotADirectory => Some(("ENOTDIR", "not a directory")),
        ErrorKind::IsADirectory => Some(("EISDIR", "illegal operation on a directory")),
        ErrorKind::ReadOnlyFilesystem => Some(("EROFS", "read-only file system")),
        ErrorKind::StorageFull => Some(("ENOSPC", "no space left on device")),
        ErrorKind::ResourceBusy => Some(("EBUSY", "resource busy or locked")),
        ErrorKind::DirectoryNotEmpty => Some(("ENOTEMPTY", "directory not empty")),
        _ => None,
    };
    let Some((code, description)) = named else { return fail(error.to_string()) };
    let paths = paths.iter().map(|path| format!("'{path}'")).collect::<Vec<_>>().join(" -> ");
    fail(format!("{code}: {description}, {syscall} {paths}"))
}

fn lstat(path: &str) -> Result<fs::Metadata, ProjectError> {
    fs::symlink_metadata(path).map_err(|error| node_error(&error, "lstat", &[path]))
}

/// `fs.realpathSync`, without the `\\?\` prefix Windows canonical paths carry.
fn realpath(path: &str) -> Result<String, ProjectError> {
    let real = fs::canonicalize(path).map_err(|error| node_error(&error, "realpath", &[path]))?;
    let real = real.to_string_lossy().into_owned();
    Ok(real
        .strip_prefix(r"\\?\UNC\")
        .map(|rest| format!(r"\\{rest}"))
        .or_else(|| real.strip_prefix(r"\\?\").map(str::to_string))
        .unwrap_or(real))
}

/// `stats.mtimeMs`: seconds × 1000 + nanoseconds ÷ 1e6, as Node computes it.
fn mtime_ms(metadata: &fs::Metadata) -> f64 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        metadata.mtime() as f64 * 1000.0 + metadata.mtime_nsec() as f64 / 1_000_000.0
    }
    #[cfg(not(unix))]
    {
        metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs() as f64 * 1000.0 + duration.subsec_nanos() as f64 / 1_000_000.0)
            .unwrap_or(0.0)
    }
}

/// The device and inode a regular file is identified by, where the platform exposes them.
fn file_identity(metadata: &fs::Metadata) -> (u64, u64) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        (metadata.dev(), metadata.ino())
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        (0, 0)
    }
}

fn assert_safe_set_path(path: &str) -> Result<String, ProjectError> {
    if !is_absolute(path) || path.contains('\0') {
        return Err(fail("set path must be absolute and safe"));
    }
    let resolved = resolve(path);
    if extname(&resolved).to_lowercase() != ".als" {
        return Err(fail("set path must be an .als file"));
    }
    let stats = lstat(&resolved)?;
    if stats.is_symlink() {
        return Err(fail("set file must not be a symbolic link"));
    }
    if !stats.is_file() {
        return Err(fail("set path is not a regular file"));
    }
    if stats.len() > MAX_SET_BYTES {
        return Err(fail("set file exceeds the bounded size"));
    }
    Ok(resolved)
}

fn sha256_file(path: &str) -> Result<String, ProjectError> {
    Ok(sha256_hex(&fs::read(path).map_err(|error| node_error(&error, "open", &[path]))?))
}

#[derive(Debug, Clone, PartialEq)]
pub struct SetSourceRead {
    pub path: String,
    pub raw: Vec<u8>,
    pub xml: String,
    pub size: u64,
    pub mtime_ms: f64,
    pub sha256: String,
}

/// `gunzipSync(raw, { maxOutputLength: MAX_SET_BYTES })`.
fn gunzip_bounded(raw: &[u8]) -> Result<Vec<u8>, ProjectError> {
    let mut decoder = MultiGzDecoder::new(raw);
    let mut xml: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 65536];
    loop {
        match decoder.read(&mut chunk) {
            Ok(0) => return Ok(xml),
            Ok(count) => {
                xml.extend_from_slice(&chunk[..count]);
                if xml.len() as u64 > MAX_SET_BYTES {
                    return Err(fail("decompressed set exceeds the bounded size"));
                }
            }
            Err(_) => return Err(fail("set file is not a valid gzip-compressed Live set")),
        }
    }
}

/// Read once, then verify that the regular-file identity did not change while
/// it was open. XML, size, and hash therefore describe the same bounded bytes.
pub fn read_set_source(path: &str) -> Result<SetSourceRead, ProjectError> {
    let resolved = assert_safe_set_path(path)?;
    let before = lstat(&resolved)?;
    let raw = fs::read(&resolved).map_err(|error| node_error(&error, "open", &[&resolved]))?;
    let after = lstat(&resolved)?;
    if after.is_symlink()
        || !after.is_file()
        || file_identity(&before) != file_identity(&after)
        || before.len() != after.len()
        || mtime_ms(&before) != mtime_ms(&after)
        || raw.len() as u64 != after.len()
    {
        return Err(fail("set file identity changed during the bounded read"));
    }
    let xml_bytes = gunzip_bounded(&raw)?;
    let sha256 = sha256_hex(&raw);
    Ok(SetSourceRead {
        path: resolved,
        xml: String::from_utf8_lossy(&xml_bytes).into_owned(),
        size: after.len(),
        mtime_ms: mtime_ms(&after),
        sha256,
        raw,
    })
}

pub fn decode_xml_attribute(value: &str) -> Result<String, ProjectError> {
    static ENTITY: LazyLock<Regex> = LazyLock::new(|| Regex::new("&(?:amp|lt|gt|quot|apos|#[0-9]+|#x[0-9a-fA-F]+);").unwrap());
    let mut decoded = String::with_capacity(value.len());
    let mut last = 0;
    for found in ENTITY.find_iter(value) {
        decoded.push_str(&value[last..found.start()]);
        last = found.end();
        let entity = found.as_str();
        let named = match entity {
            "&amp;" => Some('&'),
            "&lt;" => Some('<'),
            "&gt;" => Some('>'),
            "&quot;" => Some('"'),
            "&apos;" => Some('\''),
            _ => None,
        };
        if let Some(character) = named {
            decoded.push(character);
            continue;
        }
        let hexadecimal = entity.starts_with("&#x");
        let digits = &entity[if hexadecimal { 3 } else { 2 }..entity.len() - 1];
        let code_point = u32::from_str_radix(digits, if hexadecimal { 16 } else { 10 })
            .ok()
            .filter(|code_point| *code_point <= 0x10ffff && !(0xd800..=0xdfff).contains(code_point))
            .and_then(char::from_u32);
        let Some(character) = code_point else { return Err(fail("Live Set contains an invalid XML path entity")) };
        decoded.push(character);
    }
    decoded.push_str(&value[last..]);
    Ok(decoded)
}

struct ReferencedMediaCollection {
    values: Vec<String>,
    observed: usize,
    omitted: usize,
    complete: bool,
}

fn referenced_media_values(xml: &str) -> Result<ReferencedMediaCollection, ProjectError> {
    static FILE_REF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<FileRef(?-u:\b)[^>]*>(?s:(.*?))</FileRef\s*>").unwrap());
    static PATH_VALUE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"<Path(?-u:\b)[^>]*(?-u:\b)Value="([^"]*)""#).unwrap());
    let mut values: Vec<String> = Vec::new();
    let mut seen: HashSet<[u8; 32]> = HashSet::new();
    for file_ref in FILE_REF.captures_iter(xml) {
        let Some(path) = PATH_VALUE.captures(&file_ref[1]).map(|path| path[1].to_string()).filter(|path| !path.is_empty()) else {
            continue;
        };
        let value = decode_xml_attribute(&path)?;
        let identity: [u8; 32] = Sha256::digest(value.as_bytes()).into();
        if seen.contains(&identity) {
            continue;
        }
        // Stop after the first distinct overflow reference. This keeps both the
        // retained strings and the deduplication set bounded; observed/omitted are
        // then explicit lower bounds rather than pretending to be exact counts.
        if values.len() >= MAX_FILE_REFERENCES {
            let observed = values.len() + 1;
            return Ok(ReferencedMediaCollection { values, observed, omitted: 1, complete: false });
        }
        seen.insert(identity);
        values.push(value);
    }
    let observed = values.len();
    Ok(ReferencedMediaCollection { values, observed, omitted: 0, complete: true })
}

fn parse_manifest(source: &SetSourceRead, references: &ReferencedMediaCollection) -> ProjectManifest {
    static TRACKS: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"<(?:AudioTrack|MidiTrack|GroupTrack|ReturnTrack|MasterTrack|MainTrack)(?-u:\b)").unwrap());
    static SCENES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<Scene(?-u:\b)").unwrap());
    let tracks = TRACKS.find_iter(&source.xml).count();
    let scenes = SCENES.find_iter(&source.xml).count();
    ProjectManifest {
        path: source.path.clone(),
        size: source.size,
        mtime_ms: source.mtime_ms,
        sha256: source.sha256.clone(),
        tracks,
        scenes,
        media_refs: references.observed,
    }
}

fn ableton_root_attributes(xml: &str) -> Result<AbletonRootAttributes, ProjectError> {
    static ROOT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<Ableton(?-u:\b)([^>]*)>").unwrap());
    static ATTRIBUTE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?-u:\b)([A-Za-z][A-Za-z0-9]*)="([^"]*)""#).unwrap());
    let root = ROOT.captures(xml).map(|found| found[1].to_string()).unwrap_or_default();
    let mut attributes: Vec<(String, String)> = Vec::new();
    for found in ATTRIBUTE.captures_iter(&root) {
        let decoded = decode_xml_attribute(&found[2])?;
        match attributes.iter_mut().find(|(name, _)| *name == &found[1]) {
            Some(entry) => entry.1 = decoded,
            None => attributes.push((found[1].to_string(), decoded)),
        }
    }
    let get = |name: &str| attributes.iter().find(|(key, _)| key == name).map(|(_, value)| value.clone());
    Ok(AbletonRootAttributes {
        creator: get("Creator"),
        major_version: get("MajorVersion"),
        minor_version: get("MinorVersion"),
        schema_change_count: get("SchemaChangeCount"),
    })
}

static WINDOWS_DRIVE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z]:[\\/]").unwrap());

fn is_network_or_device_path(value: &str) -> bool {
    static DEVICE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\\\\[?.]\\").unwrap());
    static SCHEME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^[A-Za-z][A-Za-z0-9+.-]*:[\\/]{1,2}").unwrap());
    let normalized = value.replace('/', "\\");
    let windows_drive = WINDOWS_DRIVE.is_match(value);
    normalized.starts_with("\\\\") || DEVICE.is_match(&normalized) || (!windows_drive && SCHEME.is_match(value))
}

pub fn project_source_evidence(path: &str) -> Result<ProjectSourceEvidence, ProjectError> {
    let source = read_set_source(path)?;
    let mut collected = referenced_media_values(&source.xml)?;
    collected.values.sort_by(|a, b| js_str_cmp(a, b));
    let unresolved = |value: String, resolution: ReferenceResolution| ProjectReference {
        value,
        resolved_path: None,
        exists: None,
        project_local: None,
        resolution,
    };
    // The Set's own folder, resolved once (and only if a reference needs it), not once per reference.
    let real_project = std::cell::OnceCell::new();
    let references: Vec<ProjectReference> = collected
        .values
        .iter()
        .map(|raw_value| {
            if kumi_common::js::string::utf16_len(raw_value) > MAX_FILE_REFERENCE_LENGTH {
                return unresolved(format!("oversized-{}", sha256_hex(raw_value.as_bytes())), ReferenceResolution::Oversized);
            }
            let value = raw_value.clone();
            if value.contains('\0') {
                return unresolved(format!("unsafe-{}", sha256_hex(value.as_bytes())), ReferenceResolution::Unresolved);
            }
            if is_network_or_device_path(&value) {
                return unresolved(format!("network-{}", sha256_hex(value.as_bytes())), ReferenceResolution::Network);
            }
            let windows_absolute = WINDOWS_DRIVE.is_match(&value);
            if is_absolute(&value) || windows_absolute {
                let resolved_path = if windows_absolute && !is_absolute(&value) { value.clone() } else { resolve(&value) };
                let exists = Path::new(&resolved_path).exists();
                let mut project_local = false;
                if !windows_absolute {
                    if exists {
                        let real_project = real_project.get_or_init(|| realpath(&dirname(&source.path)).ok()).as_deref();
                        project_local = match (real_project, realpath(&resolved_path)) {
                            (Some(real_project), Ok(real_reference)) => {
                                real_reference == real_project || real_reference.starts_with(&format!("{real_project}{SEP}"))
                            }
                            _ => false,
                        };
                    } else {
                        let lexical = resolve(&resolved_path);
                        let project_directory = dirname(&source.path);
                        project_local = lexical != project_directory && lexical.starts_with(&format!("{project_directory}{SEP}"));
                    }
                }
                return ProjectReference {
                    value,
                    resolved_path: Some(resolved_path),
                    exists: Some(exists),
                    project_local: Some(project_local),
                    resolution: ReferenceResolution::Absolute,
                };
            }
            unresolved(value, ReferenceResolution::Unresolved)
        })
        .collect();
    let included = references.len();
    Ok(ProjectSourceEvidence {
        manifest: parse_manifest(&source, &collected),
        ableton: ableton_root_attributes(&source.xml)?,
        references,
        reference_bounds: ReferenceBounds {
            observed: collected.observed,
            observed_kind: if collected.complete { ObservedKind::Exact } else { ObservedKind::LowerBound },
            included,
            omitted: collected.omitted,
            complete: collected.complete,
        },
    })
}

pub fn project_info(path: &str) -> Result<ProjectInfo, ProjectError> {
    let evidence = project_source_evidence(path)?;
    let missing: Vec<String> = evidence
        .references
        .iter()
        .filter(|reference| reference.resolution == ReferenceResolution::Absolute && reference.exists == Some(false))
        .filter_map(|reference| reference.resolved_path.clone())
        .collect();
    let manifest = evidence.manifest;
    Ok(ProjectInfo {
        path: manifest.path,
        size: manifest.size,
        mtime_ms: manifest.mtime_ms,
        sha256: manifest.sha256,
        tracks: manifest.tracks,
        scenes: manifest.scenes,
        media_refs: manifest.media_refs,
        missing_media: missing,
        exists: true,
    })
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ProjectBackupOptions {
    pub allowed_root: Option<String>,
    pub expected_sha256: Option<String>,
    pub expected_size: Option<u64>,
    pub expected_mtime_ms: Option<f64>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectBackupResult {
    pub backup: String,
    pub manifest: ProjectManifest,
    pub verified: bool,
}

pub fn project_backup(path: &str, options: &ProjectBackupOptions) -> Result<ProjectBackupResult, ProjectError> {
    let resolved = assert_safe_set_path(path)?;
    if let Some(allowed_root) = &options.allowed_root {
        if !is_absolute(allowed_root) || allowed_root.contains('\0') {
            return Err(fail("backup allowlist root must be an absolute safe directory"));
        }
        let root = resolve(allowed_root);
        let root_stats = lstat(&root)?;
        if !root_stats.is_dir() || root_stats.is_symlink() {
            return Err(fail("backup allowlist root must be a real directory"));
        }
        let real_root = realpath(&root)?;
        let real_set = realpath(&resolved)?;
        if real_set != real_root && !real_set.starts_with(&format!("{real_root}{SEP}")) {
            return Err(fail("set path is outside the explicit backup allowlist root"));
        }
    }
    let source = read_set_source(&resolved)?;
    let source_manifest = parse_manifest(&source, &referenced_media_values(&source.xml)?);
    if options.expected_sha256.is_some()
        && (options.expected_sha256.as_deref() != Some(source_manifest.sha256.as_str())
            || options.expected_size != Some(source_manifest.size)
            || options.expected_mtime_ms != Some(source_manifest.mtime_ms))
    {
        return Err(fail("set content changed since backup preview"));
    }
    let directory = dirname(&resolved);
    let stamp = kumi_common::time::iso_string(kumi_common::time::now_ms()).replace([':', '.'], "-");
    let set_name = basename(&resolved);
    let backup_name = format!("{}.backup-{stamp}.als", set_name.strip_suffix(".als").unwrap_or(&set_name));
    let target = join(&directory, &backup_name);
    let temporary = join(&directory, &format!(".ableton-mcp-backup-{}-{}.tmp", std::process::id(), kumi_common::time::now_ms()));
    if Path::new(&target).exists() {
        return Err(fail("backup target already exists"));
    }
    let result = (|| {
        fs::copy(&resolved, &temporary).map_err(|error| node_error(&error, "copyfile", &[&resolved, &temporary]))?;
        let source_sha = sha256_file(&resolved)?;
        let copy_sha = sha256_file(&temporary)?;
        if source_sha != source_manifest.sha256 || source_sha != copy_sha {
            return Err(fail("set changed during backup or copy verification failed"));
        }
        fs::rename(&temporary, &target).map_err(|error| node_error(&error, "rename", &[&temporary, &target]))?;
        let verified = sha256_file(&target)? == source_sha;
        let backup_source = read_set_source(&target)?;
        let manifest = parse_manifest(&backup_source, &referenced_media_values(&backup_source.xml)?);
        Ok(ProjectBackupResult { backup: target.clone(), manifest, verified })
    })();
    if Path::new(&temporary).exists() {
        fs::remove_file(&temporary).map_err(|error| node_error(&error, "unlink", &[&temporary]))?;
    }
    result
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectLimitation {
    pub available: bool,
    pub operation: String,
    pub reason: String,
    pub extension_point: String,
}

/// Save/save-as/open/new/export/collect/bounce are not exposed by the Live
/// 12.4.5b8 Remote Script API. Report the precise negotiated limitation.
pub fn project_limitation(operation: &str) -> ProjectLimitation {
    ProjectLimitation {
        available: false,
        operation: operation.to_string(),
        reason: format!("{operation} is not exposed by the Live Remote Script API in this Live version and is not fabricated"),
        extension_point: "canonical project.new/open/save/save-as/collect/export/bounce operations are reserved for a future adapter and remain unadvertised until executable; project.info and project.backup are available now".to_string(),
    }
}
