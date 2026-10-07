//! Native bridge configuration and retained legacy configuration compatibility.
use crate::{live::LiveError, platform::current_platform};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};
#[path = "delivery_acl.rs"]
mod acl;
#[path = "delivery_diagnostics.rs"]
mod diagnostics;
#[path = "delivery_install.rs"]
mod install;
pub use diagnostics::{default_package_root, diagnostics, diagnostics_async, native_entrypoint, DiagnosticReport};
pub use install::{install_remote_script, registry_digest, reject_symlink_tree, InstallOptions, InstallResult};
pub const CONFIG_VERSION: u8 = 1;
pub const BRIDGE_CONFIG_VERSION: u8 = 2;
pub const PACKAGE_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const SUPPORTED_NODE_MAJORS: &[u32] = &[22, 24];
pub const NODE_ENGINE_RANGE: &str = ">=22 <23 || >=24 <25";
pub const SUPPORTED_PLATFORMS: &[&str] = &["darwin", "linux", "win32"];
pub const BRIDGE_DIAGNOSTICS_MAX_BYTES: u64 = 16 * 1024 * 1024;
pub const REMOTE_SCRIPT_ASSET: &str = "ableton_mcp_remote_script.py";
pub const REMOTE_SCRIPT_PACKAGE: &str = "AbletonMcpBridge";
pub const OPERATION_REGISTRY_ASSET: &str = "ableton-live-v1.operations.json";
/// Willington's runtime files, inside the Remote Script package when the bridge carries them.
pub const WILLINGTON_FOLDER: &str = "willington";
/// The producer's switch for Willington, beside the Remote Script: absent, Willington stays off.
pub const WILLINGTON_CONFIG: &str = "willington.json";
/// Willington's Follow Action self-test receipt for the bridge's own copy: the bridge turns Follow Action
/// edits on only with a passing one for the library it loads.
pub const WILLINGTON_RECEIPT: &str = "willington/WillingtonBindings/self-test.json";
/// The producer's files in an installed Remote Script, not the release's: they come and go after an install
/// without counting as drift, and an install carries them over.
pub const PRODUCER_FILES: [&str; 2] = [WILLINGTON_CONFIG, WILLINGTON_RECEIPT];
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerCommand {
    pub command: String,
    pub args: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerConfig {
    pub version: u8,
    pub server: ServerCommand,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeDiagnosticsConfig {
    pub path: PathBuf,
    pub max_bytes: u64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeConnection {
    pub host: String,
    pub port: f64,
    pub secret_file: PathBuf,
    pub timeout_ms: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realtime_port: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<BridgeDiagnosticsConfig>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BridgeConfig {
    pub version: u8,
    pub server: ServerCommand,
    pub bridge: BridgeConnection,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AnyConfig {
    Bridge(BridgeConfig),
    Server(ServerConfig),
}
impl AnyConfig {
    pub fn server(&self) -> &ServerCommand {
        match self {
            Self::Bridge(c) => &c.server,
            Self::Server(c) => &c.server,
        }
    }
    pub fn bridge(&self) -> Option<&BridgeConnection> {
        match self {
            Self::Bridge(c) => Some(&c.bridge),
            _ => None,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SecretPermissions {
    OwnerOnly,
    Unavailable,
    Invalid,
}
fn fail(message: impl Into<String>) -> LiveError {
    LiveError::error(message)
}
pub(crate) fn io_error(error: &std::io::Error, syscall: &str, paths: &[&Path]) -> LiveError {
    use std::io::ErrorKind;
    let (code, message) = match error.kind() {
        ErrorKind::NotFound => ("ENOENT", "no such file or directory"),
        ErrorKind::PermissionDenied => ("EACCES", "permission denied"),
        ErrorKind::AlreadyExists => ("EEXIST", "file already exists"),
        ErrorKind::NotADirectory => ("ENOTDIR", "not a directory"),
        ErrorKind::IsADirectory => ("EISDIR", "illegal operation on a directory"),
        _ => return fail(error.to_string()),
    };
    let paths = paths.iter().map(|path| format!("'{}'", path.display())).collect::<Vec<_>>().join(" -> ");
    fail(format!("{code}: {message}, {syscall} {paths}"))
}
fn lstat(path: &Path) -> Result<fs::Metadata, LiveError> {
    fs::symlink_metadata(path).map_err(|e| io_error(&e, "lstat", &[path]))
}
fn read(path: &Path) -> Result<Vec<u8>, LiveError> {
    fs::read(path).map_err(|e| io_error(&e, "open", &[path]))
}
fn path_argument(value: Option<&Value>) -> Result<&str, LiveError> {
    if let Some(Value::String(path)) = value {
        return Ok(path);
    }
    let received = match value {
        None => "undefined".into(),
        Some(Value::Null) => "null".into(),
        Some(Value::Bool(value)) => format!("type boolean ({value})"),
        Some(Value::Number(value)) => format!("type number ({})", kumi_common::js::json::number(value)),
        Some(Value::Array(_)) => "an instance of Array".into(),
        _ => "an instance of Object".into(),
    };
    Err(LiveError::type_error(format!("The \"path\" argument must be of type string. Received {received}")))
}
fn path_is_absolute(path: &Path) -> bool {
    path.is_absolute() || (cfg!(windows) && path.has_root())
}
fn safe_absolute(path: &Path) -> bool {
    path_is_absolute(path) && !path.to_string_lossy().contains('\0')
}
fn validate_loopback(host: &str) -> Result<(), LiveError> {
    if !["127.0.0.1", "::1"].contains(&host) {
        return Err(fail("bridge host must be an exact loopback address (127.0.0.1 or ::1)"));
    }
    Ok(())
}
fn validate_secret_path(path: &Path) -> Result<(), LiveError> {
    if !safe_absolute(path) {
        return Err(fail("secret file must be an absolute safe path"));
    }
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => Err(fail("secret file must not be a symbolic link")),
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(io_error(&e, "lstat", &[path])),
        _ => Ok(()),
    }
}
pub fn secret_permissions(path: &Path) -> SecretPermissions {
    if cfg!(windows) {
        return if acl::owner_only(path) { SecretPermissions::OwnerOnly } else { SecretPermissions::Invalid };
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if fs::metadata(path).is_ok_and(|meta| meta.mode() & 0o077 == 0 && meta.uid() == unsafe { libc::getuid() }) {
            return SecretPermissions::OwnerOnly;
        }
    }
    SecretPermissions::Invalid
}
pub fn secure_windows_file(path: &Path) -> Result<(), LiveError> {
    if cfg!(windows) {
        acl::secure_file(path)
    } else {
        Ok(())
    }
}
pub fn secure_windows_directory(path: &Path) -> Result<(), LiveError> {
    if cfg!(windows) {
        acl::secure_directory(path)
    } else {
        Ok(())
    }
}
fn validate_diagnostics_file(path: &Path) -> Result<(), LiveError> {
    if !safe_absolute(path) {
        return Err(fail("diagnostics file must be an absolute safe path"));
    }
    let mut cursor = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                cursor.pop();
            }
            component => cursor.push(component.as_os_str()),
        }
    }
    while cursor.parent().is_some_and(|parent| parent != cursor) {
        if cursor.exists() {
            let entry = lstat(&cursor)?;
            let compatibility = cfg!(target_os = "macos") && (cursor == Path::new("/var") || cursor == Path::new("/tmp"));
            if entry.file_type().is_symlink() && !compatibility {
                return Err(fail("diagnostics path must not contain a symbolic-link or junction ancestor"));
            }
        }
        cursor = cursor.parent().unwrap().into();
    }
    let parent = path.parent().unwrap_or(Path::new("."));
    let entry = lstat(parent)?;
    if !entry.is_dir() || entry.file_type().is_symlink() || secret_permissions(parent) != SecretPermissions::OwnerOnly {
        return Err(fail("diagnostics directory must be owner-only and non-linked"));
    }
    let entry = lstat(path)?;
    #[cfg(unix)]
    let links = {
        use std::os::unix::fs::MetadataExt;
        entry.nlink()
    };
    #[cfg(windows)]
    let links = {
        let file = fs::File::open(path).map_err(|e| io_error(&e, "open", &[path]))?;
        crate::platform::windows_file_identity(&file).map_err(|e| io_error(&e, "fstat", &[path]))?.2
    };
    #[cfg(not(any(unix, windows)))]
    let links = 0;
    if !entry.is_file() || entry.file_type().is_symlink() || links != 1 || secret_permissions(path) != SecretPermissions::OwnerOnly {
        return Err(fail("diagnostics file must be an owner-only single-link regular file"));
    }
    Ok(())
}
pub fn generate_secret(bytes: Option<f64>) -> Result<String, LiveError> {
    let bytes = bytes.unwrap_or(32.0);
    if bytes.fract() != 0.0 || !(32.0..=128.0).contains(&bytes) {
        return Err(fail("secret size must be between 32 and 128 bytes"));
    }
    let mut data = vec![0; bytes as usize];
    rand::rng().fill_bytes(&mut data);
    Ok(URL_SAFE_NO_PAD.encode(data))
}
/// A file that didn't exist, opened to write.
fn create_new(path: &Path, mode: u32) -> Result<fs::File, LiveError> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(mode);
    }
    options.open(path).map_err(|e| io_error(&e, "open", &[path]))
}
fn write_new(path: &Path, bytes: &[u8], mode: u32) -> Result<(), LiveError> {
    create_new(path, mode)?.write_all(bytes).map_err(|e| io_error(&e, "write", &[path]))
}
fn chmod(path: &Path, mode: u32) -> Result<(), LiveError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(|e| io_error(&e, "chmod", &[path]))?;
    }
    #[cfg(not(unix))]
    let _ = (path, mode);
    Ok(())
}
pub fn write_secret_file(path: &Path, secret: Option<&str>) -> Result<(), LiveError> {
    let generated;
    let secret = match secret {
        Some(secret) => secret,
        None => {
            generated = generate_secret(None)?;
            &generated
        }
    };
    validate_secret_path(path)?;
    if kumi_common::js::string::utf16_len(secret) < 32 || secret.contains(['\n', '\r']) {
        return Err(fail("secret is invalid"));
    }
    let parent = path.parent().unwrap_or(Path::new("."));
    if !parent.exists() {
        return Err(fail(format!("secret directory does not exist: {}", parent.display())));
    }
    let mut file = create_new(path, 0o600)?;
    // The file is this call's from here: one left half-written, or not owner-only, would make every later install
    // refuse it, so a failure takes it away again.
    let written = file.write_all(format!("{secret}\n").as_bytes()).map_err(|e| io_error(&e, "write", &[path]));
    drop(file);
    let result = written.and_then(|()| chmod(path, 0o600)).and_then(|()| secure_windows_file(path));
    if result.is_err() {
        let _ = fs::remove_file(path);
    }
    result
}
fn js_whitespace(c: char) -> bool {
    matches!(c,'\u{0009}'..='\u{000d}'|'\u{0020}'|'\u{00a0}'|'\u{1680}'|'\u{2000}'..='\u{200a}'|'\u{2028}'|'\u{2029}'|'\u{202f}'|'\u{205f}'|'\u{3000}'|'\u{feff}')
}
pub fn read_secret_file(path: &Path) -> Result<String, LiveError> {
    validate_secret_path(path)?;
    let raw = String::from_utf8_lossy(&read(path)?).into_owned();
    let secret = raw.strip_suffix('\n').map(|s| s.strip_suffix('\r').unwrap_or(s)).unwrap_or(&raw);
    if secret.is_empty() || secret.chars().any(js_whitespace) || kumi_common::js::string::utf16_len(secret) < 32 {
        return Err(fail("secret file is invalid"));
    }
    if secret_permissions(path) != SecretPermissions::OwnerOnly {
        return Err(fail("secret file permissions must be conclusively owner-only"));
    }
    Ok(secret.into())
}
fn unknown(value: &Value, allowed: &[&str]) -> bool {
    value.as_object().is_some_and(|object| object.keys().any(|key| !allowed.contains(&key.as_str())))
}
const BRIDGE_FIELDS: &[&str] = &["host", "port", "secretFile", "timeoutMs", "realtimePort", "diagnostics"];
/// An explicit runner retains the legacy Node shape; no runner generates a direct native command.
pub fn config_for_bridge(
    entrypoint: &Path,
    bridge: &Value,
    legacy_runner: Option<&str>,
    config_path: Option<&Path>,
    validate_destination: bool,
) -> Result<BridgeConfig, LiveError> {
    if unknown(bridge, BRIDGE_FIELDS) {
        return Err(fail("unsupported bridge configuration fields"));
    }
    if !path_is_absolute(entrypoint) {
        return Err(fail("entrypoint must be an absolute path"));
    }
    if !bridge["port"].as_f64().is_some_and(|p| p.fract() == 0.0 && (1.0..=65535.0).contains(&p)) {
        return Err(fail("bridge port must be between 1 and 65535"));
    }
    validate_loopback(bridge["host"].as_str().unwrap_or_default())?;
    validate_secret_path(Path::new(path_argument(bridge.get("secretFile"))?))?;
    if config_path.is_some_and(|path| !safe_absolute(path)) {
        return Err(fail("configuration path must be absolute"));
    }
    if !bridge["timeoutMs"].as_f64().is_some_and(|ms| ms.fract() == 0.0 && (100.0..=60000.0).contains(&ms)) {
        return Err(fail("bridge timeout must be between 100 and 60000 ms"));
    }
    if let Some(port) = bridge.get("realtimePort") {
        if !port.as_f64().is_some_and(|p| p.fract() == 0.0 && (1.0..=65535.0).contains(&p) && Some(p) != bridge["port"].as_f64()) {
            return Err(fail("realtime port must be distinct and between 1 and 65535"));
        }
    }
    if let Some(diagnostics) = bridge.get("diagnostics") {
        if !diagnostics.is_object()
            || unknown(diagnostics, &["path", "maxBytes"])
            || diagnostics["maxBytes"].as_f64() != Some(BRIDGE_DIAGNOSTICS_MAX_BYTES as f64)
        {
            return Err(fail("bridge diagnostics configuration is invalid"));
        }
        let path = diagnostics["path"]
            .as_str()
            .map(Path::new)
            .filter(|path| safe_absolute(path))
            .ok_or_else(|| fail("bridge diagnostics path is invalid"))?;
        if validate_destination {
            validate_diagnostics_file(path)?;
        }
    }
    let entrypoint = entrypoint.to_string_lossy().into_owned();
    let command = legacy_runner.unwrap_or(&entrypoint);
    if command.is_empty() {
        return Err(fail("node command must be a non-empty string"));
    }
    let mut args = if legacy_runner.is_some() { vec![entrypoint.clone()] } else { vec![] };
    if let Some(config) = config_path.filter(|path| !path.as_os_str().is_empty()) {
        args.extend(["--config".into(), config.to_string_lossy().into_owned()]);
    }
    Ok(BridgeConfig {
        version: 2,
        server: ServerCommand { command: command.into(), args },
        bridge: BridgeConnection {
            host: bridge["host"].as_str().unwrap().into(),
            port: bridge["port"].as_f64().unwrap(),
            secret_file: PathBuf::from(bridge["secretFile"].as_str().unwrap()),
            timeout_ms: bridge["timeoutMs"].as_f64().unwrap(),
            realtime_port: bridge.get("realtimePort").and_then(Value::as_f64),
            diagnostics: bridge.get("diagnostics").map(|value| BridgeDiagnosticsConfig {
                path: PathBuf::from(value["path"].as_str().unwrap()),
                max_bytes: BRIDGE_DIAGNOSTICS_MAX_BYTES,
            }),
        },
    })
}
pub fn config_for_entrypoint(entrypoint: &Path, legacy_runner: Option<&str>) -> Result<ServerConfig, LiveError> {
    if !path_is_absolute(entrypoint) {
        return Err(fail("entrypoint must be an absolute path"));
    }
    let entrypoint = entrypoint.to_string_lossy().into_owned();
    let command = legacy_runner.unwrap_or(&entrypoint);
    if command.is_empty() {
        return Err(fail("node command must be a non-empty string"));
    }
    Ok(ServerConfig {
        version: 1,
        server: ServerCommand { command: command.into(), args: if legacy_runner.is_some() { vec![entrypoint.clone()] } else { vec![] } },
    })
}
fn server_valid(value: &Value) -> bool {
    !unknown(value, &["command", "args"])
        && value["command"].as_str().is_some_and(|s| !s.is_empty())
        && value["args"].as_array().is_some_and(|args| args.iter().all(Value::is_string))
}
pub fn parse_config(value: &Value) -> Result<ServerConfig, LiveError> {
    if !value.is_object() {
        return Err(fail("configuration must be an object"));
    }
    if unknown(value, &["version", "server"]) || value["version"].as_f64() != Some(1.0) || !value["server"].is_object() {
        return Err(fail("unsupported configuration version"));
    }
    if !server_valid(&value["server"]) {
        return Err(fail("invalid server configuration"));
    }
    Ok(ServerConfig { version: 1, server: serde_json::from_value(value["server"].clone())? })
}
pub fn parse_bridge_config(value: &Value) -> Result<BridgeConfig, LiveError> {
    if !value.is_object() {
        return Err(fail("configuration must be an object"));
    }
    if unknown(value, &["version", "server", "bridge"]) {
        return Err(fail("unsupported configuration fields"));
    }
    let server = &value["server"];
    let bridge = &value["bridge"];
    if value["version"].as_f64() != Some(2.0) || !(server.is_object() || server.is_array()) || !(bridge.is_object() || bridge.is_array()) {
        return Err(fail("unsupported configuration version"));
    }
    if unknown(server, &["command", "args"])
        || unknown(bridge, BRIDGE_FIELDS)
        || server.as_array().is_some_and(|v| !v.is_empty())
        || bridge.as_array().is_some_and(|v| !v.is_empty())
    {
        return Err(fail("unsupported configuration fields"));
    }
    if !server_valid(server) {
        return Err(fail("invalid server configuration"));
    }
    if !bridge["host"].is_string()
        || !bridge["port"].is_number()
        || !bridge["secretFile"].is_string()
        || !bridge["timeoutMs"].is_number()
        || bridge.get("realtimePort").is_some_and(|v| !v.is_number())
        || bridge.get("diagnostics").is_some_and(|v| !v.is_object())
    {
        return Err(fail("invalid bridge configuration"));
    }
    let command = server["command"].as_str().unwrap();
    let args = server["args"].as_array().unwrap();
    let native = args.len() == 2 && args[0] == "--config" && path_is_absolute(Path::new(command));
    let legacy = args.len() == 3 && args[1] == "--config";
    let config_index = if native { 1 } else { 2 };
    if (!native && !legacy) || !args.get(config_index).and_then(Value::as_str).is_some_and(|path| path_is_absolute(Path::new(path))) {
        return Err(fail("version-2 server configuration must include --config PATH"));
    }
    let config = config_for_bridge(
        Path::new(if native { command } else { args[0].as_str().unwrap() }),
        bridge,
        if native { None } else { Some(command) },
        Some(Path::new(args[config_index].as_str().unwrap())),
        false,
    )?;
    read_secret_file(&config.bridge.secret_file)?;
    Ok(config)
}
fn parse_any(value: &Value) -> Result<AnyConfig, LiveError> {
    if value["version"].as_f64() == Some(2.0) {
        parse_bridge_config(value).map(AnyConfig::Bridge)
    } else {
        parse_config(value).map(AnyConfig::Server)
    }
}
pub fn read_config(path: &Path) -> Result<ServerConfig, LiveError> {
    parse_config(&serde_json::from_str(&String::from_utf8_lossy(&read(path)?))?)
}
pub fn read_any_config(path: &Path) -> Result<AnyConfig, LiveError> {
    parse_any(&serde_json::from_str(&String::from_utf8_lossy(&read(path)?))?)
}
pub fn is_supported_platform(platform: Option<&str>) -> bool {
    SUPPORTED_PLATFORMS.contains(&platform.unwrap_or_else(|| current_platform()))
}
pub fn supported_node_major(version: &str) -> bool {
    let parts: Vec<_> = version.split('.').collect();
    parts.len() == 3
        && parts.iter().all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
        && parts[0].parse::<u32>().is_ok_and(|major| SUPPORTED_NODE_MAJORS.contains(&major))
}
pub fn unsupported_node_message(version: &str) -> Option<String> {
    (!supported_node_major(version)).then(|| {
        format!(
            "Unsupported Node.js {version}. Supported major versions: {}. Install one of those versions and retry.",
            SUPPORTED_NODE_MAJORS.iter().map(u32::to_string).collect::<Vec<_>>().join(", ")
        )
    })
}
fn remove_file_missing_ok(path: &Path) -> Result<(), LiveError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(io_error(&e, "unlink", &[path])),
    }
}
fn rename(from: &Path, to: &Path) -> Result<(), LiveError> {
    fs::rename(from, to).map_err(|e| io_error(&e, "rename", &[from, to]))
}
fn temporary_directory(parent: &Path, prefix: &str) -> Result<PathBuf, LiveError> {
    let path = parent.join(format!("{prefix}{}-{}", uuid::Uuid::new_v4(), generate_secret(Some(32.0))?.get(..6).unwrap()));
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(&path).map_err(|e| io_error(&e, "mkdtemp", &[&path]))?;
    Ok(path)
}
/// Validate before staging, reject linked destinations, and retain the original if replacement fails.
pub fn write_config(path: &Path, config: &impl Serialize, force: bool) -> Result<(), LiveError> {
    let config = parse_any(&serde_json::to_value(config)?)?;
    let bytes = kumi_common::js::json::file_text(&serde_json::to_value(config)?);
    replace_owner_file(path, path.parent().unwrap_or(Path::new(".")), bytes.as_bytes(), force)
}
/// An owner-only file, put in place whole (an existing one is replaced) the way `write_config` puts a
/// configuration. It's staged in `staging`, a folder on the same volume: outside the installed Remote Script,
/// whose files are checked, so a write cut short leaves nothing there.
pub fn write_owner_file(path: &Path, staging: &Path, bytes: &[u8]) -> Result<(), LiveError> {
    replace_owner_file(path, staging, bytes, true)
}
fn replace_owner_file(path: &Path, staging: &Path, bytes: &[u8], force: bool) -> Result<(), LiveError> {
    let mut exists = false;
    match fs::symlink_metadata(path) {
        Ok(destination) => {
            if destination.is_dir() {
                return Err(fail(format!("refusing to replace configuration directory: {}", path.display())));
            }
            exists = destination.is_file() || destination.file_type().is_symlink();
            if destination.file_type().is_symlink() {
                return Err(fail(format!("refusing to write through symbolic link: {}", path.display())));
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(io_error(&e, "lstat", &[path])),
    }
    if exists && !force {
        return Err(fail(format!("refusing to overwrite existing file: {}", path.display())));
    }
    let parent = path.parent().unwrap_or(Path::new("."));
    if !parent.exists() {
        return Err(fail(format!("configuration directory does not exist: {}", parent.display())));
    }
    let directory = temporary_directory(staging, ".ableton-mcp-")?;
    let staged = directory.join("config.json");
    // One rename puts the new file in the old one's place (atomically on both platforms): the old file is never
    // only a backup that a failed step could lose.
    let result = (|| {
        write_new(&staged, bytes, 0o600)?;
        chmod(&staged, 0o600)?;
        secure_windows_file(&staged)?;
        rename(&staged, path)
    })();
    // Cleanup can't change the outcome: the file is in place, or the old one never moved.
    let _ = remove_file_missing_ok(&staged);
    let _ = fs::remove_dir(&directory);
    result
}
pub fn migrate_config(input: &Path, output: &Path, force: bool, bridge: Option<&Value>) -> Result<AnyConfig, LiveError> {
    let source: Value = serde_json::from_str(&String::from_utf8_lossy(&read(input)?))?;
    let mut config = if source.is_object() && source["version"].as_f64() == Some(2.0) {
        AnyConfig::Bridge(parse_bridge_config(&source)?)
    } else if source.is_object() && source["version"].as_f64() == Some(1.0) {
        AnyConfig::Server(parse_config(&source)?)
    } else if source.is_object() {
        if !source["command"].as_str().is_some_and(|v| !v.is_empty())
            || !source["args"].as_array().is_some_and(|args| args.iter().all(Value::is_string))
        {
            return Err(fail("legacy configuration must contain command and string args"));
        }
        AnyConfig::Server(ServerConfig {
            version: 1,
            server: ServerCommand {
                command: source["command"].as_str().unwrap().into(),
                args: source["args"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().into()).collect(),
            },
        })
    } else {
        return Err(fail("configuration must be an object"));
    };
    if let Some(bridge) = bridge {
        let server = config.server();
        let native =
            Path::new(&server.command).file_name().is_some_and(|name| name == "ableton-mcp-server" || name == "ableton-mcp-server.exe");
        let entry = if native { Some(server.command.as_str()) } else { server.args.first().map(String::as_str) };
        let entry = entry
            .filter(|path| !path.is_empty() && path_is_absolute(Path::new(path)))
            .ok_or_else(|| fail("version-2 migration requires an absolute server entrypoint as the first argument"))?;
        read_secret_file(Path::new(bridge["secretFile"].as_str().unwrap_or_default()))?;
        config = AnyConfig::Bridge(config_for_bridge(
            Path::new(entry),
            bridge,
            if native { None } else { Some(&server.command) },
            Some(output),
            true,
        )?);
    }
    write_config(output, &config, force)?;
    Ok(config)
}
pub fn write_bridge_reference(path: &Path, config_path: &Path, force: bool) -> Result<(), LiveError> {
    if !safe_absolute(path) || !safe_absolute(config_path) {
        return Err(fail("bridge reference paths must be absolute"));
    }
    if lstat(config_path)?.file_type().is_symlink()
        || !fs::metadata(config_path).map_err(|e| io_error(&e, "stat", &[config_path]))?.is_file()
    {
        return Err(fail("bridge configuration must be a regular file"));
    }
    if path.exists() && (lstat(path)?.file_type().is_symlink() || !force) {
        return Err(fail(format!("refusing to overwrite existing bridge reference: {}", path.display())));
    }
    let parent = path.parent().unwrap_or(Path::new("."));
    if !parent.exists() {
        return Err(fail(format!("configuration directory does not exist: {}", parent.display())));
    }
    let text = format!("{}\n", kumi_common::js::json::stringify(&json!({"config":config_path})));
    if force {
        let mut options = fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options.open(path).and_then(|mut file| file.write_all(text.as_bytes())).map_err(|e| io_error(&e, "open", &[path]))?;
    } else {
        write_new(path, text.as_bytes(), 0o600)?;
    }
    chmod(path, 0o600)?;
    secure_windows_file(path)
}
