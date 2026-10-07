//! File authority and persistent transaction-owned media from the source host.
use super::*;
use crate::{delivery::io_error, drum_sampler_preset::*};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use kumi_common::js::string;
use rand::RngCore;
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

const MAX_BYTES: u64 = 512 * 1024 * 1024;
pub(super) struct ImportFiles {
    configured: Option<String>,
    root: RefCell<Option<String>>,
    library: Option<String>,
    resources: Option<String>,
    template: RefCell<Option<DrumSamplerTemplate>>,
}
fn random(bytes: usize) -> Vec<u8> {
    let mut bytes = vec![0; bytes];
    rand::rng().fill_bytes(&mut bytes);
    bytes
}
fn path_text(path: &Path) -> String {
    let value = path.to_string_lossy();
    #[cfg(windows)]
    {
        return value
            .strip_prefix(r"\\?\UNC\")
            .map(|v| format!(r"\\{v}"))
            .or_else(|| value.strip_prefix(r"\\?\").map(str::to_owned))
            .unwrap_or_else(|| value.into_owned());
    }
    #[cfg(not(windows))]
    value.into_owned()
}
fn canonical(path: &Path) -> Result<String, LiveError> {
    fs::canonicalize(path).map(|p| path_text(&p)).map_err(|e| {
        // Node realpath resolves relative input before reporting its failing lstat.
        let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        io_error(&e, "lstat", &[&absolute])
    })
}
fn chmod(path: &Path, mode: u32) -> Result<(), LiveError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(|e| io_error(&e, "chmod", &[path]))
    }
    #[cfg(not(unix))]
    {
        let mut permissions = fs::metadata(path).map_err(|e| io_error(&e, "stat", &[path]))?.permissions();
        permissions.set_readonly(mode & 0o200 == 0);
        fs::set_permissions(path, permissions).map_err(|e| io_error(&e, "chmod", &[path]))
    }
}
fn mkdir(path: &Path, recursive: bool, mode: u32) -> Result<(), LiveError> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(recursive);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(mode);
    }
    #[cfg(not(unix))]
    let _ = mode;
    builder.create(path).map_err(|e| io_error(&e, "mkdir", &[path]))
}
fn mtime_ms(metadata: &fs::Metadata) -> f64 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        metadata.mtime() as f64 * 1000. + metadata.mtime_nsec() as f64 / 1e6
    }
    #[cfg(not(unix))]
    {
        metadata.modified().ok().and_then(|v| v.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0., |v| v.as_secs_f64() * 1000.)
    }
}
fn same_file(a: &fs::Metadata, b: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        a.dev() == b.dev() && a.ino() == b.ino() && a.len() == b.len()
    }
    // Both metadata reads address the same open descriptor; rename does not replace its identity.
    #[cfg(not(unix))]
    {
        a.len() == b.len()
    }
}
fn header_matches(extension: &str, header: &[u8]) -> bool {
    match extension {
        ".wav" | ".wave" => header.len() >= 12 && &header[..4] == b"RIFF" && &header[8..12] == b"WAVE",
        ".aif" | ".aiff" => header.len() >= 12 && &header[..4] == b"FORM" && [&b"AIFF"[..], &b"AIFC"[..]].contains(&&header[8..12]),
        ".flac" => header.starts_with(b"fLaC"),
        ".ogg" => header.starts_with(b"OggS"),
        ".mp3" => header.len() >= 3 && (header.starts_with(b"ID3") || header[0] == 0xff && header[1] & 0xe0 == 0xe0),
        ".m4a" => header.len() >= 8 && &header[4..8] == b"ftyp",
        _ => false,
    }
}
async fn open_source(path: &str) -> Result<tokio::fs::File, LiveError> {
    let mut options = tokio::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW);
    options.open(path).await.map_err(|e| io_error(&e, "open", &[Path::new(path)]))
}
async fn header(file: &mut tokio::fs::File) -> Result<Vec<u8>, LiveError> {
    let mut bytes = vec![0; 12];
    let n = file.read(&mut bytes).await.map_err(|e| io_error(&e, "read", &[]))?;
    bytes.truncate(n);
    Ok(bytes)
}
async fn hash_file(file: &mut tokio::fs::File, bounded: bool) -> Result<String, LiveError> {
    let mut hash = Sha256::new();
    let mut buffer = vec![0u8; 65536];
    let mut bytes = 0u64;
    loop {
        let n = file.read(&mut buffer).await.map_err(|e| io_error(&e, "read", &[]))?;
        if n == 0 {
            break;
        }
        bytes += n as u64;
        if bounded && bytes > MAX_BYTES {
            return Err(LiveError::error("audio file exceeds the import bound"));
        }
        hash.update(&buffer[..n]);
    }
    Ok(hex::encode(hash.finalize()))
}
impl ImportFiles {
    pub(super) fn new(options: &McpHostOptions) -> Self {
        Self {
            configured: options.import_staging_dir.clone(),
            root: RefCell::new(None),
            library: options.user_library_dir.clone(),
            resources: options.live_resources_dir.clone(),
            template: RefCell::new(None),
        }
    }
    fn root(&self) -> Result<String, LiveError> {
        if let Some(root) = self.root.borrow().as_ref() {
            return Ok(root.clone());
        }
        let configured = self.configured.clone().or_else(|| std::env::var("ABLETON_MCP_IMPORT_STAGING_DIR").ok());
        if configured.as_ref().is_some_and(|p| !Path::new(p).is_absolute()) {
            return Err(LiveError::error("import staging directory override must be an absolute path"));
        }
        let root = configured.map(PathBuf::from).unwrap_or_else(|| {
            let home = home::home_dir().unwrap_or_default();
            if cfg!(windows) {
                std::env::var("APPDATA")
                    .ok()
                    .map(PathBuf::from)
                    .filter(|p| p.is_absolute())
                    .unwrap_or(home.join("AppData/Roaming"))
                    .join("ableton-mcp/import-staging")
            } else {
                home.join(".config/ableton-mcp/import-staging")
            }
        });
        mkdir(&root, true, 0o700)?;
        let stat = fs::symlink_metadata(&root).map_err(|e| io_error(&e, "lstat", &[&root]))?;
        if !stat.is_dir() {
            return Err(LiveError::error("import staging root is not a directory"));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if stat.uid() != unsafe { libc::getuid() } {
                return Err(LiveError::error("import staging root is not owned by the current user"));
            }
        }
        chmod(&root, 0o700)?;
        let root = canonical(&root)?;
        *self.root.borrow_mut() = Some(root.clone());
        Ok(root)
    }
    fn preset_root(&self) -> Result<PathBuf, LiveError> {
        let library = self
            .library
            .clone()
            .or_else(|| std::env::var("ABLETON_MCP_USER_LIBRARY").ok())
            .unwrap_or_else(|| default_user_library(None, None));
        let path = Path::new(&library);
        if !path.is_absolute() || !path.exists() {
            return Err(LiveError::error("drum pad loading into Drum Sampler needs Live's User Library, which wasn't found; use Simpler"));
        }
        Ok(Path::new(&canonical(path)?).join("Kumi"))
    }
    fn write_preset(&self, staging: &str, name: &str) -> Result<Value, LiveError> {
        if self.template.borrow().is_none() {
            *self.template.borrow_mut() =
                find_drum_sampler_template(&self.resources.clone().map(|v| vec![v]).unwrap_or_else(|| live_resource_folders(None, None)));
        }
        let template = self.template.borrow();
        let Some(template) = template.as_ref() else {
            return Err(LiveError::error(
                "drum pad loading into Drum Sampler needs Live 12's Drum Sampler, which wasn't found; use Simpler",
            ));
        };
        let root = self.preset_root()?;
        mkdir(&root, true, 0o755)?;
        let safe: String = name.chars().map(|c| if "\\/:*?\"<>|".contains(c) || c <= '\u{1f}' { ' ' } else { c }).collect();
        let safe = string::head(string::trim(&safe), 100);
        let safe = if safe.is_empty() { "Sample" } else { &safe };
        let name = format!("{safe} {}.adv", hex::encode(random(4)));
        let path = root.join(&name);
        let stat = fs::metadata(staging).map_err(|e| io_error(&e, "stat", &[Path::new(staging)]))?;
        let bytes = drum_sampler_preset(
            template,
            &DrumSamplerSample { path: staging.into(), size: stat.len() as f64, modified_seconds: mtime_ms(&stat) / 1000. },
        )
        .map_err(|e| LiveError::error(e.to_string()))?;
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o644);
        }
        use std::io::Write;
        options.open(&path).and_then(|mut file| file.write_all(&bytes)).map_err(|e| io_error(&e, "open", &[&path]))?;
        Ok(json!({"presetPath":path_text(&path),"presetItemId":format!("user_library/Kumi/{name}")}))
    }
    fn release_preset(&self, path: &Value) {
        let Some(path) = path.as_str() else { return };
        let Ok(root) = self.preset_root() else { return };
        if Path::new(path).parent() == Some(root.as_path()) {
            let _ = fs::remove_file(path);
        }
    }
    fn release(&self, path: &Value) {
        let Some(path) = path.as_str() else { return };
        let root = self.root.borrow();
        let Some(root) = root.as_ref() else { return };
        if !path.starts_with(&format!("{root}{}", std::path::MAIN_SEPARATOR)) {
            return;
        }
        let path = Path::new(path);
        if chmod(path, 0o600).is_err() || fs::remove_file(path).is_err() {
            return;
        }
        if let Some(folder) = path.parent().filter(|p| *p != Path::new(root) && p.starts_with(root)) {
            let _ = fs::remove_file(format!("{}.asd", path.display()));
            let _ = fs::remove_dir(folder);
        }
    }
    pub(super) fn release_for(&self, transaction: &Value) {
        let kind = transaction["kind"].as_str();
        let payload = &transaction["payload"];
        if matches!(kind, Some("session-audio-create" | "simpler")) {
            self.release(&payload["filePath"])
        }
        if matches!(kind, Some("device" | "drum-pad")) {
            self.release(&payload["samplePath"])
        }
        if kind == Some("drum-pad") {
            for pad in payload["pads"].as_array().into_iter().flatten().filter(|v| v.is_object()) {
                self.release(&pad["samplePath"])
            }
            self.release_presets(transaction)
        }
    }
    fn release_presets(&self, transaction: &Value) {
        let payload = &transaction["payload"];
        self.release_preset(&payload["presetPath"]);
        for pad in payload["pads"].as_array().into_iter().flatten().filter(|v| v.is_object()) {
            self.release_preset(&pad["presetPath"])
        }
    }
}
impl McpHost {
    pub(super) async fn audio_import_file_authority(&self, file_path: &Value, allowed_root: &Value) -> Result<Value, LiveError> {
        if !is_non_empty_string(file_path, 1024) || !is_non_empty_string(allowed_root, 1024) {
            return Err(LiveError::error("filePath and allowedRoot are required"));
        }
        let path = file_path.as_str().unwrap();
        // Before anything touches either path: opening a share sends Windows' credentials to its host.
        if kumi_common::path::network_or_device(path) || kumi_common::path::network_or_device(allowed_root.as_str().unwrap()) {
            return Err(LiveError::error("files on a network share aren't imported: copy the file onto this computer first"));
        }
        let bytes = path.as_bytes();
        if !path.starts_with('/') && !(bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':') {
            return Err(LiveError::error("filePath must be an absolute path"));
        }
        let root = canonical(Path::new(allowed_root.as_str().unwrap()))?;
        let path = canonical(Path::new(path))?;
        let prefix = if root.ends_with(std::path::MAIN_SEPARATOR) { root.clone() } else { format!("{root}{}", std::path::MAIN_SEPARATOR) };
        if path != root && !path.starts_with(&prefix) {
            return Err(LiveError::error("filePath escapes the allowed root"));
        }
        let stat = fs::metadata(&path).map_err(|e| io_error(&e, "stat", &[Path::new(&path)]))?;
        if !stat.is_file() {
            return Err(LiveError::error("filePath is not a regular file"));
        }
        if stat.len() == 0 || stat.len() > MAX_BYTES {
            return Err(LiveError::error("filePath size is outside the import bound"));
        }
        let extension = path
            .rfind('.')
            .map(|i| path[i..].to_lowercase())
            .unwrap_or_else(|| path.chars().last().map(|c| c.to_lowercase().to_string()).unwrap_or_default());
        if [".mid", ".midi"].contains(&extension.as_str()) {
            return Err(LiveError::error("MIDI file import has no negotiated canonical operation in this version; no Session MIDI-file import is claimed (follow-up surface)"));
        }
        if ![".wav", ".wave", ".aif", ".aiff", ".mp3", ".flac", ".ogg", ".m4a"].contains(&extension.as_str()) {
            return Err(LiveError::error("filePath type is not an importable audio file"));
        }
        {
            let mut file = open_source(&path).await?;
            if !header_matches(&extension, &header(&mut file).await?) {
                return Err(LiveError::error("file content does not match the declared audio format"));
            }
        }
        let mut file = tokio::fs::File::open(&path).await.map_err(|e| io_error(&e, "open", &[Path::new(&path)]))?;
        let hash = hash_file(&mut file, false).await?;
        Ok(json!({"canonicalPath":path,"size":stat.len(),"mtimeMs":mtime_ms(&stat),"sha256":hash}))
    }
    pub(super) async fn stage_verified_import_file(&self, path: &str, expected: &Value) -> Result<String, LiveError> {
        let mut source = open_source(path).await?;
        let before = source.metadata().await.map_err(|e| io_error(&e, "fstat", &[]))?;
        if !before.is_file() || Some(before.len() as f64) != expected["size"].as_f64() || before.len() == 0 || before.len() > MAX_BYTES {
            return Err(LiveError::error("audio file changed since preview"));
        }
        let extension = Path::new(path).extension().map(|s| format!(".{}", s.to_string_lossy().to_lowercase())).unwrap_or_default();
        if !header_matches(&extension, &header(&mut source).await?) {
            return Err(LiveError::error("audio file content no longer matches the declared format"));
        }
        source.seek(std::io::SeekFrom::Start(0)).await.map_err(|e| io_error(&e, "read", &[]))?;
        let hash = hash_file(&mut source, true).await?;
        let after = source.metadata().await.map_err(|e| io_error(&e, "fstat", &[]))?;
        if !same_file(&before, &after) || expected["sha256"] != hash {
            return Err(LiveError::error("audio file changed since preview"));
        }
        let folder = Path::new(&self.import_files.root()?).join(URL_SAFE_NO_PAD.encode(random(12)));
        mkdir(&folder, false, 0o700)?;
        let staging = folder.join(Path::new(path).file_name().unwrap_or_default());
        let mut options = tokio::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o444);
        let mut output = match options.open(&staging).await {
            Ok(file) => file,
            Err(e) => {
                let _ = fs::remove_dir(&folder);
                return Err(io_error(&e, "open", &[&staging]));
            }
        };
        source.seek(std::io::SeekFrom::Start(0)).await.map_err(|e| io_error(&e, "read", &[]))?;
        let mut hash = Sha256::new();
        let mut buffer = vec![0u8; 65536];
        loop {
            let n = source.read(&mut buffer).await.map_err(|e| io_error(&e, "read", &[]))?;
            if n == 0 {
                break;
            }
            hash.update(&buffer[..n]);
            output.write_all(&buffer[..n]).await.map_err(|e| io_error(&e, "write", &[]))?;
        }
        output.flush().await.map_err(|e| io_error(&e, "write", &[]))?;
        drop(output);
        if expected["sha256"] != hex::encode(hash.finalize()) {
            return Err(LiveError::error("audio file changed since preview"));
        }
        chmod(&staging, 0o444)?;
        Ok(path_text(&staging))
    }
    pub(super) async fn verify_staged_import_file(&self, path: &str, expected: &Value) -> Result<(), LiveError> {
        let canonical = canonical(Path::new(path))?;
        if !canonical.starts_with(&format!("{}{}", self.import_files.root()?, std::path::MAIN_SEPARATOR)) {
            return Err(LiveError::error("staged import path escapes the transaction staging root"));
        }
        let stat = fs::metadata(&canonical).map_err(|e| io_error(&e, "stat", &[Path::new(&canonical)]))?;
        if !stat.is_file() || Some(stat.len() as f64) != expected["size"].as_f64() {
            return Err(LiveError::error("staged audio file changed since preview"));
        }
        let mut file = tokio::fs::File::open(&canonical).await.map_err(|e| io_error(&e, "open", &[Path::new(&canonical)]))?;
        if expected["sha256"] != hash_file(&mut file, false).await? {
            return Err(LiveError::error("staged audio file changed since preview"));
        }
        Ok(())
    }
    pub(super) fn release_staged_import_file(&self, path: &Value) {
        self.import_files.release(path)
    }
    pub(super) fn release_staged_import_for(&self, transaction: &Value) {
        self.import_files.release_for(transaction)
    }
    pub(super) fn write_drum_sampler_preset(&self, path: &str, name: &str) -> Result<Value, LiveError> {
        self.import_files.write_preset(path, name)
    }
    pub(super) fn release_drum_sampler_preset(&self, path: &Value) {
        self.import_files.release_preset(path)
    }
    pub(super) fn release_drum_sampler_presets(&self, transaction: &Value) {
        self.import_files.release_presets(transaction)
    }
    pub(super) fn drum_pad_load_args(&self, pad: &Value) -> Value {
        let mut args = super::device_parameter::fields(pad, &["ref", "expectedObjectIdentity", "name"]);
        if pad["instrument"] == "Drum Sampler" {
            args["instrument"] = json!("Drum Sampler");
            if let Some(value) = pad.get("presetItemId") {
                args["presetItemId"] = value.clone();
            }
        } else if let Some(value) = pad.get("samplePath") {
            args["samplePath"] = value.clone();
        }
        args
    }
    pub(super) async fn await_browser_items(&self, items: &[String]) -> Result<(), LiveError> {
        let deadline = kumi_common::time::now_ms_f64() + 8000.;
        for item in items {
            loop {
                if self
                    .async_adapter()
                    .invoke_async(
                        &LiveInvocation::new("browser.inspect", json!({"itemId":item})),
                        Some(&LiveOperationContext::with_deadline(self.deadline(super::reads::AUDITION_DEADLINE_MS))),
                    )
                    .await
                    .is_ok()
                {
                    break;
                }
                if kumi_common::time::now_ms_f64() > deadline {
                    return Err(LiveError::error("drum pad loading into Drum Sampler timed out waiting for Live's Browser to see the preset; try again or use Simpler"));
                }
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            }
        }
        Ok(())
    }
}
