//! Owner-state and compensation primitives for the bridge's install lifecycle.
use super::*;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Component, Path, PathBuf},
};
pub(super) fn fail(message: impl Into<String>) -> LiveError {
    LiveError::error(message)
}
pub(super) fn sha256(bytes: impl AsRef<[u8]>) -> String {
    hex::encode(Sha256::digest(bytes.as_ref()))
}
pub(super) fn read(path: &Path) -> Result<Vec<u8>, LiveError> {
    fs::read(path).map_err(|e| io_error(&e, "open", &[path]))
}
pub(super) fn lstat(path: &Path) -> Result<fs::Metadata, LiveError> {
    fs::symlink_metadata(path).map_err(|e| io_error(&e, "lstat", &[path]))
}
pub(super) fn file_digest(path: &Path) -> Result<String, LiveError> {
    let meta = lstat(path)?;
    if !meta.is_file() || meta.file_type().is_symlink() {
        return Err(fail(format!("managed file is not a regular file: {}", path.display())));
    }
    Ok(sha256(read(path)?))
}
pub(super) fn validate_absolute_path(path: &Path, label: &str) -> Result<(), LiveError> {
    if !path.is_absolute() || path.to_string_lossy().contains('\0') {
        return Err(fail(format!("{label} must be an absolute safe path")));
    }
    if crate::command::resolve(path)?.parent().is_none() {
        return Err(fail(format!("{label} must not be a filesystem root")));
    }
    Ok(())
}
pub fn assert_no_linked_ancestors(path: &Path) -> Result<(), LiveError> {
    validate_absolute_path(path, "lifecycle path")?;
    let mut cursor = PathBuf::new();
    for part in crate::command::resolve(path)?.components() {
        cursor.push(part.as_os_str());
        if matches!(part, Component::Prefix(_) | Component::RootDir) {
            continue;
        }
        if !cursor.exists() {
            break;
        }
        if lstat(&cursor)?.file_type().is_symlink()
            && !(cfg!(target_os = "macos") && (cursor == Path::new("/var") || cursor == Path::new("/tmp")))
        {
            return Err(fail(format!("refusing symbolic-link or junction ancestor: {}", cursor.display())));
        }
    }
    Ok(())
}
pub(super) fn chmod(path: &Path, mode: u32) -> Result<(), LiveError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(|e| io_error(&e, "chmod", &[path]))?;
    }
    #[cfg(not(unix))]
    let _ = (path, mode);
    Ok(())
}
pub(super) fn ensure_owner_directory(path: &Path) -> Result<(), LiveError> {
    assert_no_linked_ancestors(path.parent().unwrap_or(path))?;
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path).map_err(|e| io_error(&e, "mkdir", &[path]))?;
    chmod(path, 0o700)?;
    secure_windows_directory(path)
}
pub(super) fn owner_only(path: &Path) -> bool {
    secret_permissions(path) == SecretPermissions::OwnerOnly
}
pub(super) fn single_link(path: &Path, meta: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let _ = path;
        meta.nlink() == 1
    }
    #[cfg(windows)]
    {
        let _ = meta;
        fs::File::open(path).and_then(|f| crate::platform::windows_file_identity(&f)).is_ok_and(|(_, _, links)| links == 1)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (path, meta);
        false
    }
}
pub(super) fn diagnostics_file_valid(path: &Path) -> bool {
    (|| -> Result<bool, LiveError> {
        assert_no_linked_ancestors(path)?;
        let parent = path.parent().unwrap_or(path);
        let p = lstat(parent)?;
        let e = lstat(path)?;
        Ok(p.is_dir()
            && !p.file_type().is_symlink()
            && owner_only(parent)
            && e.is_file()
            && !e.file_type().is_symlink()
            && single_link(path, &e)
            && owner_only(path))
    })()
    .unwrap_or(false)
}
pub(super) fn write_new(path: &Path, bytes: &[u8]) -> Result<(), LiveError> {
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(|e| io_error(&e, "open", &[path]))?;
    file.write_all(bytes).map_err(|e| io_error(&e, "write", &[path]))
}
pub(super) fn ensure_diagnostics_file(path: &Path) -> Result<bool, LiveError> {
    assert_no_linked_ancestors(path)?;
    if path.exists() {
        if !diagnostics_file_valid(path) {
            return Err(fail("bridge diagnostics destination must be an owner-only single-link regular file"));
        }
        if lstat(path)?.len() > BRIDGE_DIAGNOSTICS_MAX_BYTES {
            OpenOptions::new().write(true).open(path).and_then(|f| f.set_len(0)).map_err(|e| io_error(&e, "truncate", &[path]))?;
        }
        return Ok(false);
    }
    let mut created = false;
    let result = (|| {
        write_new(path, b"")?;
        created = true;
        chmod(path, 0o600)?;
        secure_windows_file(path)?;
        let entry = lstat(path)?;
        if !entry.is_file() || entry.file_type().is_symlink() || !single_link(path, &entry) || !owner_only(path) {
            return Err(fail("could not establish a safe bridge diagnostics destination"));
        }
        Ok(true)
    })();
    if result.is_err() && created && !try_remove(path, false) {
        return Err(fail("bridge diagnostics creation and compensation both failed"));
    }
    result
}
fn stale_lock(path: &Path) -> bool {
    let Ok(entry) = lstat(path) else { return false };
    if !entry.is_file() || entry.file_type().is_symlink() {
        return false;
    }
    let Ok(value) = read(path).and_then(|bytes| serde_json::from_slice::<Value>(&bytes).map_err(Into::into)) else {
        return false;
    };
    let Some(pid) =
        value["pid"].as_f64().filter(|v| v.fract() == 0. && *v > 0. && *v <= i32::MAX as f64 && *v != std::process::id() as f64)
    else {
        return false;
    };
    #[cfg(unix)]
    {
        (unsafe { libc::kill(pid as i32, 0) }) == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
    }
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut std::ffi::c_void;
            fn GetLastError() -> u32;
            fn GetExitCodeProcess(process: *mut std::ffi::c_void, code: *mut u32) -> i32;
            fn CloseHandle(process: *mut std::ffi::c_void) -> i32;
        }
        unsafe {
            let handle = OpenProcess(0x1000, 0, pid as u32);
            if handle.is_null() {
                return GetLastError() == 87;
            }
            let mut code = 259;
            let ok = GetExitCodeProcess(handle, &mut code);
            CloseHandle(handle);
            ok != 0 && code != 259
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
        false
    }
}
pub(super) struct LifecycleLock {
    path: PathBuf,
}
impl LifecycleLock {
    pub(super) fn take(state: &Path) -> Result<Self, LiveError> {
        let path = state.join("lifecycle.lock");
        assert_no_linked_ancestors(&path)?;
        let bytes = format!(
            "{}\n",
            kumi_common::js::json::stringify(
                &json!({"version":1,"pid":std::process::id(),"startedAt":kumi_common::time::iso_string(kumi_common::time::now_ms())})
            )
        );
        let take = || write_new(&path, bytes.as_bytes());
        let refused = || fail("another lifecycle operation owns the state lock; inspect the owner before removing a stale lock");
        if take().is_err() {
            if !stale_lock(&path) {
                return Err(refused());
            }
            remove(&path, false)?;
            take().map_err(|_| refused())?;
        }
        let lock = Self { path };
        chmod(&lock.path, 0o600)?;
        secure_windows_file(&lock.path)?;
        Ok(lock)
    }
}
impl Drop for LifecycleLock {
    fn drop(&mut self) {
        if self.path.exists() {
            let _ = remove(&self.path, false);
        }
    }
}
pub(super) fn rename(from: &Path, to: &Path) -> Result<(), LiveError> {
    fs::rename(from, to).map_err(|e| io_error(&e, "rename", &[from, to]))
}
pub(super) fn write_owner_json(path: &Path, value: &Value) -> Result<(), LiveError> {
    validate_absolute_path(path, "managed JSON path")?;
    assert_no_linked_ancestors(path)?;
    let temporary = PathBuf::from(format!("{}.{}.{}.tmp", path.display(), std::process::id(), kumi_common::time::now_ms()));
    write_new(&temporary, kumi_common::js::json::file_text(value).as_bytes())?;
    chmod(&temporary, 0o600)?;
    secure_windows_file(&temporary)?;
    let result = (|| {
        if path.exists() {
            let current = lstat(path)?;
            if current.file_type().is_symlink() || !current.is_file() || !owner_only(path) {
                return Err(fail(format!("refusing to replace unowned or non-regular managed JSON: {}", path.display())));
            }
            #[cfg(windows)]
            {
                use base64::Engine;
                let backup = PathBuf::from(format!("{}.{}.replace-backup", path.display(), std::process::id()));
                if backup.exists() {
                    remove(&backup, false)?;
                }
                let script="$t=[Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($env:MCP_TEMP));$p=[Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($env:MCP_PATH));$b=[Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($env:MCP_BACKUP));[IO.File]::Replace($t,$p,$b,$true);[IO.File]::Delete($b)";
                let status = std::process::Command::new(crate::platform::windows_powershell(None))
                    .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script])
                    .env("MCP_TEMP", base64::engine::general_purpose::STANDARD.encode(temporary.to_string_lossy().as_bytes()))
                    .env("MCP_PATH", base64::engine::general_purpose::STANDARD.encode(path.to_string_lossy().as_bytes()))
                    .env("MCP_BACKUP", base64::engine::general_purpose::STANDARD.encode(backup.to_string_lossy().as_bytes()))
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status()
                    .map_err(|e| fail(e.to_string()))?;
                if !status.success() {
                    return Err(fail("managed JSON replacement failed"));
                }
            }
            #[cfg(not(windows))]
            rename(&temporary, path)?;
        } else {
            rename(&temporary, path)?;
        }
        secure_windows_file(path)
    })();
    if temporary.exists() {
        remove(&temporary, false)?;
    }
    result
}
pub(super) fn try_write_owner_json(path: &Path, value: &Value) -> bool {
    write_owner_json(path, value).is_ok()
}
pub(super) fn finalize_failed_journal(path: &Path, action: &str, generation: Value, error: &LiveError) {
    if path.exists() {
        if let Ok(value) = read(path).and_then(|bytes| serde_json::from_slice::<Value>(&bytes).map_err(Into::into)) {
            if value["state"] != "applying" {
                return;
            }
        }
    }
    try_write_owner_json(
        path,
        &json!({"version":1,"action":action,"state":"failed","receiptGeneration":generation,"reason":error.message()}),
    );
}
pub(super) fn path_entry_exists(path: &Path) -> Result<bool, LiveError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(io_error(&e, "lstat", &[path])),
    }
}
pub(super) fn remove(path: &Path, recursive: bool) -> Result<(), LiveError> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(io_error(&e, "lstat", &[path])),
    };
    let result =
        if recursive && meta.is_dir() && !meta.file_type().is_symlink() { fs::remove_dir_all(path) } else { fs::remove_file(path) };
    result.map_err(|e| io_error(&e, "rm", &[path]))
}
pub(super) fn try_remove(path: &Path, recursive: bool) -> bool {
    remove(path, recursive).is_ok()
}
fn copy_folder(from: &Path, to: &Path, missing_only: bool) -> Result<(), LiveError> {
    if missing_only { fs::create_dir_all(to) } else { fs::create_dir(to) }.map_err(|e| io_error(&e, "mkdir", &[to]))?;
    for entry in fs::read_dir(from).map_err(|e| io_error(&e, "scandir", &[from]))? {
        let entry = entry.map_err(|e| fail(e.to_string()))?;
        let source = entry.path();
        let target = to.join(entry.file_name());
        let meta = lstat(&source)?;
        if meta.is_dir() {
            copy_folder(&source, &target, missing_only)?;
        } else if meta.is_file() {
            if !missing_only || !target.exists() {
                let mut input = fs::File::open(&source).map_err(|e| io_error(&e, "open", &[&source]))?;
                let mut options = OpenOptions::new();
                options.create_new(true).write(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
                    options.mode(meta.permissions().mode());
                }
                let mut output = options.open(&target).map_err(|e| io_error(&e, "copyfile", &[&source, &target]))?;
                std::io::copy(&mut input, &mut output).map_err(|e| io_error(&e, "copyfile", &[&source, &target]))?;
            }
        } else {
            return Err(fail(format!("managed tree contains an unsupported entry: {}", source.display())));
        }
    }
    Ok(())
}
pub(super) fn move_remote_folder(from: &Path, to: &Path) -> Result<(), LiveError> {
    move_remote_folder_with_rename(from, to, |from, to| fs::rename(from, to))
}
fn move_remote_folder_with_rename(
    from: &Path,
    to: &Path,
    rename: impl FnOnce(&Path, &Path) -> std::io::Result<()>,
) -> Result<(), LiveError> {
    match rename(from, to) {
        Ok(()) => return Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::CrossesDevices => {}
        Err(e) => return Err(io_error(&e, "rename", &[from, to])),
    }
    let copy = |source: &Path, target: &Path, restore: bool| -> Result<(), LiveError> {
        copy_folder(source, target, restore)?;
        for name in ["bridge-reference.json", "willington.json"] {
            let file = target.join(name);
            if file.exists() {
                secure_windows_file(&file)?;
            }
        }
        Ok(())
    };
    if let Err(error) = copy(from, to, false) {
        remove(to, true)?;
        return Err(error);
    }
    let mut result = remove(from, true);
    for i in 1..=5 {
        if result.is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(i * 100));
        result = remove(from, true);
    }
    if let Err(error) = result {
        copy(to, from, true)?;
        remove(to, true)?;
        return Err(error);
    }
    Ok(())
}
pub(super) fn hash_regular_tree(root: &Path) -> Result<serde_json::Map<String, Value>, LiveError> {
    reject_symlink_tree(root)?;
    let mut output = serde_json::Map::new();
    fn walk(root: &Path, dir: &Path, output: &mut serde_json::Map<String, Value>) -> Result<(), LiveError> {
        let mut names = fs::read_dir(dir)
            .map_err(|e| io_error(&e, "scandir", &[dir]))?
            .map(|entry| entry.map(|e| e.file_name()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| fail(e.to_string()))?;
        names.sort_by(|a, b| a.to_string_lossy().encode_utf16().cmp(b.to_string_lossy().encode_utf16()));
        for name in names {
            let path = dir.join(name);
            let stat = lstat(&path)?;
            if stat.is_dir() {
                walk(root, &path, output)?;
            } else if stat.is_file() && !stat.file_type().is_symlink() {
                output.insert(
                    path.strip_prefix(root).unwrap().to_string_lossy().replace(std::path::MAIN_SEPARATOR, "/"),
                    file_digest(&path)?.into(),
                );
            } else {
                return Err(fail(format!("managed tree contains an unsupported entry: {}", path.display())));
            }
        }
        Ok(())
    }
    walk(root, root, &mut output)?;
    Ok(output)
}
pub(super) fn verify_files(root: &Path, expected: &Value) -> Result<Value, LiveError> {
    let mut required = expected.as_object().cloned().unwrap_or_default();
    required.entry("__pycache__").or_insert_with(|| sha256(b"").into());
    let current = if root.exists() { hash_regular_tree(root)? } else { serde_json::Map::new() };
    let missing = required.keys().filter(|name| !current.contains_key(*name)).cloned().collect::<Vec<_>>();
    let changed =
        required.keys().filter(|name| current.contains_key(*name) && current[*name] != required[*name]).cloned().collect::<Vec<_>>();
    let unknown = current.keys().filter(|name| !required.contains_key(*name)).cloned().collect::<Vec<_>>();
    Ok(json!({"valid":missing.is_empty()&&changed.is_empty()&&unknown.is_empty(),"missing":missing,"changed":changed,"unknown":unknown}))
}
pub(super) fn contained(state: &Path, path: &Path) -> bool {
    let Ok(state) = crate::command::resolve(state) else { return false };
    let Ok(path) = crate::command::resolve(path) else { return false };
    path.strip_prefix(state).is_ok_and(|relative| !relative.to_string_lossy().starts_with(".."))
}
pub(super) fn valid_hash(value: &Value) -> bool {
    value.as_str().is_some_and(|v| v.len() == 64 && v.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)))
}
pub(super) fn safe_integer(value: &Value) -> bool {
    value.as_f64().is_some_and(|v| v.is_finite() && v.fract() == 0. && v.abs() <= 9_007_199_254_740_991.)
}
pub(super) fn parse_receipt(path: &Path) -> Result<Value, LiveError> {
    assert_no_linked_ancestors(path)?;
    let entry = lstat(path)?;
    if !entry.is_file() || entry.file_type().is_symlink() || !owner_only(path) {
        return Err(fail("installation receipt is not an owner-only regular file"));
    }
    let v: Value = serde_json::from_slice(&read(path)?)?;
    let hashes_valid = |v: &Value| {
        ["artifactSha256", "releaseManifestSha256", "registryHash", "configSha256"].iter().all(|key| valid_hash(&v[*key]))
            && v["remoteFiles"].as_object().is_none_or(|files| files.values().all(valid_hash))
    };
    let absolute = |v: &Value| v.as_str().is_some_and(|v| Path::new(v).is_absolute());
    let state = Path::new(v["stateDirectory"].as_str().unwrap_or(""));
    let previous = &v["previous"];
    let previous_valid = previous.is_null() && v.get("previous").is_some()
        || previous.is_object()
            && hashes_valid(previous)
            && absolute(&previous["packageRoot"])
            && absolute(&previous["remoteBackup"])
            && previous["config"]["version"] == 2;
    let retained_valid = v.get("retained").is_none()
        || ["pendingCleanup", "preserved"].iter().all(|key| {
            v["retained"][*key]
                .as_array()
                .is_some_and(|items| items.iter().all(|item| absolute(item) && contained(state, Path::new(item.as_str().unwrap()))))
        });
    let config = &v["config"];
    let diagnostics = config["bridge"].get("diagnostics");
    let diagnostics_valid = diagnostics
        .is_none_or(|d| d["path"] == json!(state.join("bridge-diagnostics.log")) && d["maxBytes"] == BRIDGE_DIAGNOSTICS_MAX_BYTES);
    if v["version"] != 1
        || !["installed-restart-required", "activated", "uninstalled"].contains(&v["status"].as_str().unwrap_or(""))
        || !safe_integer(&v["generation"])
        || v["generation"].as_f64().unwrap_or(0.) < 1.
        || !["packageRoot", "stateDirectory", "remoteScriptsDirectory", "remoteScriptDirectory", "configPath", "secretPath"]
            .iter()
            .all(|key| absolute(&v[*key]))
        || !hashes_valid(&v)
        || !previous_valid
        || !retained_valid
        || !diagnostics_valid
        || config["version"] != 2
        || !config["bridge"]["host"].is_string()
        || !safe_integer(&config["bridge"]["port"])
        || !v["secretCreatedByLifecycle"].is_boolean()
        || !v["activation"]["required"].is_boolean()
        || !v["activation"]["realLiveVerified"].is_boolean()
    {
        return Err(fail("installation receipt is invalid or unsupported"));
    }
    Ok(v)
}
pub(super) fn quarantine_path(state: &Path, label: &str) -> Result<PathBuf, LiveError> {
    let root = state.join("quarantine");
    assert_no_linked_ancestors(&root)?;
    ensure_owner_directory(&root)?;
    Ok(root.join(format!("{label}-{}-{}-{}", kumi_common::time::now_ms(), std::process::id(), uuid::Uuid::new_v4().simple())))
}
pub(super) fn restore_backup(destination: &Path, backup: Option<&Path>) -> Result<(), LiveError> {
    if destination.exists() {
        remove(destination, true)?;
    }
    if let Some(backup) = backup.filter(|p| p.exists()) {
        move_remote_folder(backup, destination)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cross_device_move_copies_nested_unicode_files_and_retains_owner_only_authority() {
        let temp = tempfile::tempdir().unwrap();
        let from = temp.path().join("Remote ü");
        let to = temp.path().join("State ü");
        std::fs::create_dir_all(from.join("nested")).unwrap();
        std::fs::write(from.join("nested/source.py"), "payload").unwrap();
        for name in ["bridge-reference.json", "willington.json"] {
            write_new(&from.join(name), b"{}").unwrap();
        }
        move_remote_folder_with_rename(&from, &to, |_, _| Err(std::io::Error::from(std::io::ErrorKind::CrossesDevices))).unwrap();
        assert!(!from.exists());
        assert_eq!(std::fs::read_to_string(to.join("nested/source.py")).unwrap(), "payload");
        for name in ["bridge-reference.json", "willington.json"] {
            assert!(owner_only(&to.join(name)));
        }
    }
    #[cfg(unix)]
    #[test]
    fn cross_device_copy_rejects_link_and_removes_partial_destination_without_changing_source() {
        let temp = tempfile::tempdir().unwrap();
        let from = temp.path().join("source");
        let to = temp.path().join("destination");
        std::fs::create_dir(&from).unwrap();
        std::fs::write(from.join("payload"), "original").unwrap();
        std::os::unix::fs::symlink(from.join("payload"), from.join("link")).unwrap();
        assert!(move_remote_folder_with_rename(&from, &to, |_, _| Err(std::io::Error::from(std::io::ErrorKind::CrossesDevices)))
            .unwrap_err()
            .message()
            .contains("unsupported entry"));
        assert!(!to.exists());
        assert_eq!(std::fs::read_to_string(from.join("payload")).unwrap(), "original");
        assert!(std::fs::symlink_metadata(from.join("link")).unwrap().file_type().is_symlink());
    }
}
