//! Files and folders only their owner can read: mode 0600/0700 on unix; on Windows a protected DACL whose one rule
//! gives the current user full control, then checked. The PowerShell scripts are the twins of kumi-runtime's
//! auth/store.rs and ableton-mcp-server's delivery_acl.rs: keep them in step.
use std::io::Write;
use std::path::{Path, PathBuf};
#[cfg(windows)]
use std::time::Duration;

#[cfg(windows)]
const ACL_TARGET: &str = "$p=[Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($env:KUMI_ACL_PATH));$sid=[System.Security.Principal.WindowsIdentity]::GetCurrent().User;";
#[cfg(windows)]
const FILE_CHECKS: &str = "$c=[System.IO.File]::GetAccessControl($p);if ($c.GetOwner([System.Security.Principal.SecurityIdentifier]).Value -ne $sid.Value) { exit 2 }if (-not $c.AreAccessRulesProtected) { exit 3 }$rules=@($c.Access); if ($rules.Count -ne 1) { exit 4 }$rule=$rules[0];if ($rule.IdentityReference.Translate([System.Security.Principal.SecurityIdentifier]).Value -ne $sid.Value) { exit 5 }if ($rule.IsInherited) { exit 6 }if ($rule.AccessControlType.ToString() -ne 'Allow') { exit 7 }if (($rule.FileSystemRights -band [System.Security.AccessControl.FileSystemRights]::FullControl) -ne [System.Security.AccessControl.FileSystemRights]::FullControl) { exit 8 }exit 0";
#[cfg(windows)]
const SECURE_FILE: &str = "$a=New-Object System.Security.AccessControl.FileSecurity;$a.SetAccessRuleProtection($true,$false);$rule=New-Object System.Security.AccessControl.FileSystemAccessRule -ArgumentList @($sid,[System.Security.AccessControl.FileSystemRights]::FullControl,[System.Security.AccessControl.AccessControlType]::Allow);[void]$a.AddAccessRule($rule);[System.IO.File]::SetAccessControl($p,$a);if ([System.IO.File]::GetAccessControl($p).GetOwner([System.Security.Principal.SecurityIdentifier]).Value -ne $sid.Value) { $o=New-Object System.Security.AccessControl.FileSecurity;$o.SetOwner($sid);[System.IO.File]::SetAccessControl($p,$o) };";
#[cfg(windows)]
const SECURE_DIRECTORY: &str = "$a=New-Object System.Security.AccessControl.DirectorySecurity;$a.SetAccessRuleProtection($true,$false);$inherit=[System.Security.AccessControl.InheritanceFlags]'ContainerInherit,ObjectInherit';$rule=New-Object System.Security.AccessControl.FileSystemAccessRule -ArgumentList @($sid,[System.Security.AccessControl.FileSystemRights]::FullControl,$inherit,[System.Security.AccessControl.PropagationFlags]::None,[System.Security.AccessControl.AccessControlType]::Allow);[void]$a.AddAccessRule($rule);[System.IO.Directory]::SetAccessControl($p,$a);if ([System.IO.Directory]::GetAccessControl($p).GetOwner([System.Security.Principal.SecurityIdentifier]).Value -ne $sid.Value) { $o=New-Object System.Security.AccessControl.DirectorySecurity;$o.SetOwner($sid);[System.IO.Directory]::SetAccessControl($p,$o) };";
#[cfg(windows)]
const DIRECTORY_CHECKS: &str = "$c=[System.IO.Directory]::GetAccessControl($p);if ($c.GetOwner([System.Security.Principal.SecurityIdentifier]).Value -ne $sid.Value) { exit 2 }if (-not $c.AreAccessRulesProtected) { exit 3 }exit 0";

/// What a failed Windows check found, by its exit code.
#[cfg(windows)]
fn acl_reason(code: i32) -> Option<&'static str> {
    match code {
        2 => Some("its owner isn't you"),
        3 => Some("it inherits its folder's permissions"),
        4 => Some("it has more than one access rule"),
        5 => Some("an access rule is for another account"),
        6 => Some("an access rule is inherited"),
        7 => Some("an access rule isn't an allow rule"),
        8 => Some("an access rule doesn't give full control"),
        _ => None,
    }
}

#[cfg(windows)]
thread_local! {
    /// The PowerShell runs this thread started to set or check permissions: each costs seconds (Defender scans it).
    static ACL_RUNS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// How many PowerShell runs this thread started to set or check permissions (tests count them).
#[cfg(windows)]
#[doc(hidden)]
pub fn acl_runs_on_this_thread() -> usize {
    ACL_RUNS.with(std::cell::Cell::get)
}

#[cfg(windows)]
fn run_acl(path: &Path, script: &str, timeout_ms: u64) -> Result<(), String> {
    ACL_RUNS.with(|runs| runs.set(runs.get() + 1));
    use base64::{engine::general_purpose::STANDARD, Engine};
    use std::io::Read;
    use std::os::windows::process::CommandExt;
    use std::process::Stdio;
    let root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
    let powershell = Path::new(&root).join("System32").join("WindowsPowerShell").join("v1.0").join("powershell.exe");
    let mut command = std::process::Command::new(powershell);
    command
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script])
        .env("KUMI_ACL_PATH", STANDARD.encode(path.to_string_lossy().as_bytes()))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .creation_flags(0x0800_0000);
    let mut child = command.spawn().map_err(|error| format!("PowerShell didn't start ({error})"))?;
    let stderr = child.stderr.take();
    let reader = std::thread::spawn(move || {
        let mut said = vec![];
        if let Some(stderr) = stderr {
            let _ = stderr.take(64 * 1024).read_to_end(&mut said);
        }
        said
    });
    let start = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status.code()),
            Ok(None) if start.elapsed() < Duration::from_millis(timeout_ms) => std::thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                break Err("PowerShell didn't finish in time".to_string());
            }
            Err(error) => break Err(error.to_string()),
        }
    };
    let said = String::from_utf8_lossy(&reader.join().unwrap_or_default()).replace(path.to_string_lossy().as_ref(), "<the file>");
    match status? {
        Some(0) => Ok(()),
        Some(code) => Err(acl_reason(code).map(str::to_string).unwrap_or_else(|| said.trim().chars().take(300).collect())),
        None => Err("PowerShell stopped".to_string()),
    }
}

/// Whether only its owner, the current user, can read the file or folder.
pub fn is_owner_only(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        std::fs::symlink_metadata(path).is_ok_and(|meta| meta.mode() & 0o077 == 0 && meta.uid() == unsafe { libc::getuid() })
    }
    #[cfg(windows)]
    {
        let checks = if path.is_dir() { DIRECTORY_CHECKS } else { FILE_CHECKS };
        run_acl(path, &format!("{ACL_TARGET}{checks}"), 15_000).is_ok()
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        false
    }
}

/// Makes the folder and its missing parents; the folder itself (not its parents) becomes owner-only.
pub fn create_owner_only_dir(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        match std::fs::DirBuilder::new().mode(0o700).create(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists && path.is_dir() => {
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
            }
            Err(error) => return Err(error),
        }
    }
    // Windows: a folder this made gets its owner-only permissions (a PowerShell run, seconds each); one made already
    // (by an earlier action, which locked it then) is left as it is, so an action with nothing to do runs none.
    #[cfg(not(unix))]
    let made = match std::fs::create_dir(path) {
        Ok(()) => true,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists && path.is_dir() => false,
        Err(error) => return Err(error),
    };
    #[cfg(windows)]
    if made {
        run_acl(path, &format!("$ErrorActionPreference='Stop';{ACL_TARGET}{SECURE_DIRECTORY}{DIRECTORY_CHECKS}"), 30_000)
            .map_err(|why| std::io::Error::new(std::io::ErrorKind::PermissionDenied, why))?;
    }
    #[cfg(all(not(unix), not(windows)))]
    let _ = made;
    Ok(())
}

/// Writes the file whole, owner-only from its first byte: a new file beside it, then renamed over it, so a reader
/// sees the old contents or the new, never part.
pub fn write_owner_only(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    write_whole(path, bytes, true)
}

/// Writes the file whole (a new file beside it, renamed over it), mode 0600 on unix, without Windows' ACL step (a
/// PowerShell run): for files holding no secret in a folder already made owner-only, written from inside a DAW.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    write_whole(path, bytes, false)
}

fn write_whole(path: &Path, bytes: &[u8], acl: bool) -> std::io::Result<()> {
    let temporary = temporary_beside(path);
    let written = (|| {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        #[cfg(windows)]
        if acl {
            drop(file);
            run_acl(&temporary, &format!("$ErrorActionPreference='Stop';{ACL_TARGET}{SECURE_FILE}{FILE_CHECKS}"), 30_000)
                .map_err(|why| std::io::Error::new(std::io::ErrorKind::PermissionDenied, why))?;
            file = std::fs::OpenOptions::new().write(true).truncate(true).open(&temporary)?;
        }
        #[cfg(not(windows))]
        let _ = acl;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    written
}

/// A name beside `path` nothing else uses: `.<name>.tmp-<random>`.
pub fn temporary_beside(path: &Path) -> PathBuf {
    let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    path.with_file_name(format!(".{name}.tmp-{}", &crate::auth::random_hex16()[..12]))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn written_files_and_made_folders_are_owner_only() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("a").join("b");
        create_owner_only_dir(&folder).unwrap();
        assert!(is_owner_only(&folder));
        let file = folder.join("f");
        write_owner_only(&file, b"one").unwrap();
        write_owner_only(&file, b"two").unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"two");
        assert!(is_owner_only(&file));
        assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 1);
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(!is_owner_only(&file));
    }
}
