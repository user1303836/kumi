//! Atomic installation of the exact Remote Script payload and its registry.
use super::*;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Default)]
pub struct InstallOptions {
    pub dry_run: bool,
    pub force: bool,
    pub config_path: Option<PathBuf>,
    /// The folder the producer's files (Willington's switch and self-test receipt) are carried from, when it isn't
    /// the destination itself: repair has moved the installed copy into its quarantine by then.
    pub producer_files_from: Option<PathBuf>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallResult {
    pub installed: PathBuf,
    pub backup: Option<PathBuf>,
    pub reference: Option<PathBuf>,
    pub dry_run: bool,
}
pub fn registry_digest() -> &'static str {
    crate::registry::live_registry_hash()
}
pub fn reject_symlink_tree(path: &Path) -> Result<(), LiveError> {
    let entry = lstat(path)?;
    if entry.file_type().is_symlink() {
        return Err(fail(format!("refusing symbolic-link destination: {}", path.display())));
    }
    if entry.is_dir() {
        for child in fs::read_dir(path).map_err(|e| io_error(&e, "scandir", &[path]))? {
            let child = child.map_err(|e| io_error(&e, "scandir", &[path]))?;
            reject_symlink_tree(&child.path())?;
        }
    }
    Ok(())
}
pub(crate) fn file_digest(path: &Path) -> Result<String, LiveError> {
    Ok(hex::encode(Sha256::digest(read(path)?)))
}
fn copy(source: &Path, destination: &Path) -> Result<(), LiveError> {
    // copyFile preserves an existing destination's mode; std::fs::copy replaces it.
    let mode = fs::metadata(destination).ok().map(|entry| entry.permissions());
    fs::copy(source, destination).map_err(|e| io_error(&e, "copyfile", &[source, destination]))?;
    if let Some(mode) = mode {
        fs::set_permissions(destination, mode).map_err(|e| io_error(&e, "chmod", &[destination]))?;
    }
    Ok(())
}
/// A folder of runtime files the bridge carries, copied file by file: regular files only, and in each folder
/// with Python files a `__pycache__` blocker like the package's own, so Live's Python adds nothing to the
/// installed tree (whose files the install receipt records).
fn copy_runtime_tree(source: &Path, destination: &Path) -> Result<(), LiveError> {
    let entry = lstat(source)?;
    if !entry.is_dir() || entry.file_type().is_symlink() {
        return Err(fail(format!("bridge payload must be a regular folder: {}", source.display())));
    }
    fs::create_dir(destination).map_err(|e| io_error(&e, "mkdir", &[destination]))?;
    let mut python = false;
    for child in fs::read_dir(source).map_err(|e| io_error(&e, "scandir", &[source]))? {
        let child = child.map_err(|e| io_error(&e, "scandir", &[source]))?;
        let (from, to) = (child.path(), destination.join(child.file_name()));
        let kind = lstat(&from)?;
        if kind.file_type().is_symlink() || child.file_name() == "__pycache__" {
            return Err(fail(format!("bridge payload can't contain {}", from.display())));
        }
        if kind.is_dir() {
            copy_runtime_tree(&from, &to)?;
        } else if kind.is_file() {
            copy(&from, &to)?;
            chmod(&to, 0o644)?;
            python |= from.extension().is_some_and(|extension| extension == "py");
        } else {
            return Err(fail(format!("bridge payload must hold only regular files: {}", from.display())));
        }
    }
    if python {
        write_new(&destination.join("__pycache__"), b"", 0o400)?;
    }
    Ok(())
}
pub fn install_remote_script(
    source_file: &Path,
    destination_directory: &Path,
    options: &InstallOptions,
) -> Result<InstallResult, LiveError> {
    if !path_is_absolute(source_file) || !path_is_absolute(destination_directory) {
        return Err(fail("installer paths must be absolute"));
    }
    let source = lstat(source_file)?;
    if !source.is_file() || source.file_type().is_symlink() {
        return Err(fail("Remote Script source must be a regular file"));
    }
    if destination_directory.exists() {
        reject_symlink_tree(destination_directory)?;
        if !options.force {
            return Err(fail(format!("refusing to overwrite existing Remote Script: {}", destination_directory.display())));
        }
    }
    let parent = destination_directory.parent().unwrap_or(Path::new("."));
    if !parent.exists() {
        return Err(fail(format!("destination parent does not exist: {}", parent.display())));
    }
    if options.config_path.as_ref().is_some_and(|path| !path_is_absolute(path)) {
        return Err(fail("bridge configuration path must be absolute"));
    }
    let reference = options.config_path.as_ref().map(|_| destination_directory.join("bridge-reference.json"));
    if options.dry_run {
        return Ok(InstallResult {
            installed: destination_directory.into(),
            backup: destination_directory.exists().then(|| PathBuf::from(format!("{}.backup", destination_directory.display()))),
            reference,
            dry_run: true,
        });
    }
    let staging = temporary_directory(parent, ".ableton-mcp-install-")?;
    let staged_package = staging.join(REMOTE_SCRIPT_PACKAGE);
    let backup = destination_directory.exists().then(|| {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis();
        PathBuf::from(format!("{}.backup-{now}", destination_directory.display()))
    });
    let result = (|| {
        fs::create_dir(&staged_package).map_err(|e| io_error(&e, "mkdir", &[&staged_package]))?;
        let staged_asset = staged_package.join(REMOTE_SCRIPT_ASSET);
        copy(source_file, &staged_asset)?;
        chmod(&staged_asset, 0o600)?;
        let source_parent = source_file.parent().unwrap_or(Path::new("."));
        let package_source = if source_parent.file_name().is_some_and(|name| name == REMOTE_SCRIPT_PACKAGE) {
            source_parent.to_path_buf()
        } else {
            source_parent.join(REMOTE_SCRIPT_PACKAGE)
        };
        let init = package_source.join("__init__.py");
        if !init.exists() {
            return Err(fail("Remote Script package is missing __init__.py"));
        }
        copy(&init, &staged_package.join("__init__.py"))?;
        let module_source = package_source.join(REMOTE_SCRIPT_ASSET);
        copy(if module_source.exists() { &module_source } else { source_file }, &staged_asset)?;
        // Willington's runtime files, when this bridge carries them. Inside the package, Live doesn't list
        // their folders as Control Surfaces of their own.
        let willington_files = package_source.join(WILLINGTON_FOLDER);
        if willington_files.exists() {
            copy_runtime_tree(&willington_files, &staged_package.join(WILLINGTON_FOLDER))?;
        }
        let carried = options.producer_files_from.as_deref().unwrap_or(destination_directory);
        let willington = carried.join(WILLINGTON_CONFIG);
        if willington.exists() {
            let entry = lstat(&willington)?;
            if !entry.is_file() || entry.file_type().is_symlink() || entry.len() > 4096 {
                return Err(fail("Willington configuration must be a bounded regular file"));
            }
            let destination = staged_package.join(WILLINGTON_CONFIG);
            copy(&willington, &destination)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                chmod(&destination, entry.permissions().mode() & 0o777)?;
            }
            // A copy on Windows takes the folder's permissions, and the bridge reads only an owner-only file.
            secure_windows_file(&destination)?;
        }
        // Willington's self-test receipt stays with the bridge's copy of Willington, unless this release ships
        // its own. The bridge checks it against the library it loads, so a receipt for an older one does nothing.
        for name in [WILLINGTON_RECEIPT, WILLINGTON_EDITING_RECEIPT] {
            let receipt = carried.join(name);
            let staged_receipt = staged_package.join(name);
            if name == WILLINGTON_EDITING_RECEIPT && secret_permissions(&receipt) != SecretPermissions::OwnerOnly {
                continue; // Do not turn an unsafe receipt into trusted evidence during an update.
            }
            if receipt.exists() && staged_receipt.parent().is_some_and(Path::is_dir) && !staged_receipt.exists() {
                let entry = lstat(&receipt)?;
                if entry.is_file() && !entry.file_type().is_symlink() && entry.len() <= 1024 * 1024 {
                    copy(&receipt, &staged_receipt)?;
                    chmod(&staged_receipt, 0o600)?;
                    secure_windows_file(&staged_receipt)?;
                }
            }
        }
        // The native bridge carries the registry it was built against. This same text
        // feeds the authenticated wire validator; a caller's working directory cannot replace it.
        write_new(&staged_package.join(OPERATION_REGISTRY_ASSET), crate::registry::LIVE_REGISTRY_TEXT.as_bytes(), 0o644)?;
        let mut hashes = serde_json::Map::new();
        for name in ["__init__.py", REMOTE_SCRIPT_ASSET, OPERATION_REGISTRY_ASSET] {
            hashes.insert(name.into(), file_digest(&staged_package.join(name))?.into());
        }
        let manifest = json!({"package":REMOTE_SCRIPT_PACKAGE,"algorithm":"sha256","registryHash":registry_digest(),"files":hashes});
        write_new(&staged_package.join("manifest.json"), format!("{}\n", kumi_common::js::json::stringify(&manifest)).as_bytes(), 0o600)?;
        write_new(&staged_package.join("__pycache__"), b"", 0o400)?;
        if let Some(config) = &options.config_path {
            write_bridge_reference(&staged_package.join("bridge-reference.json"), config, false)?;
        }
        if let Some(backup) = &backup {
            fs::rename(destination_directory, backup).map_err(|e| io_error(&e, "rename", &[destination_directory, backup]))?;
        }
        fs::rename(&staged_package, destination_directory)
            .map_err(|e| io_error(&e, "rename", &[&staged_package, destination_directory]))?;
        Ok(InstallResult { installed: destination_directory.into(), backup: backup.clone(), reference, dry_run: false })
    })();
    let result = if result.is_err() {
        if let Some(backup) = backup.as_ref().filter(|backup| !destination_directory.exists() && backup.exists()) {
            fs::rename(backup, destination_directory).map_err(|e| io_error(&e, "rename", &[backup, destination_directory])).and(result)
        } else {
            result
        }
    } else {
        result
    };
    // Cleanup cannot override the authoritative replacement or rollback outcome.
    let _ = fs::remove_dir_all(staging);
    result
}
