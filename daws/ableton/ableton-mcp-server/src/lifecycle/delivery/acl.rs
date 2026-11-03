//! Windows owner-only ACL scripts and their bounded execution. native_acl.rs reads and writes the one shape SECURE_FILE
//! makes with Windows' own calls, and passes it without these: a check made stricter here (WINDOWS_ACL_CHECKS) must be
//! made stricter there too, or the fast path goes on saying yes. kumi-ai's auth/store.rs and auth/windows_acl.rs are
//! the twins of both, for Kumi's credential file.
const WINDOWS_ACL_TARGET:&str="$p=[Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($env:ABLETON_MCP_ACL_PATH));$sid=[System.Security.Principal.WindowsIdentity]::GetCurrent().User;";
const WINDOWS_ACL_CHECKS:&str="$c=[System.IO.File]::GetAccessControl($p);if ($c.GetOwner([System.Security.Principal.SecurityIdentifier]).Value -ne $sid.Value) { exit 2 }if (-not $c.AreAccessRulesProtected) { exit 3 }$rules=@($c.Access); if ($rules.Count -ne 1) { exit 4 }$rule=$rules[0];if ($rule.IdentityReference.Translate([System.Security.Principal.SecurityIdentifier]).Value -ne $sid.Value) { exit 5 }if ($rule.IsInherited) { exit 6 }if ($rule.AccessControlType.ToString() -ne 'Allow') { exit 7 }if (($rule.FileSystemRights -band [System.Security.AccessControl.FileSystemRights]::FullControl) -ne [System.Security.AccessControl.FileSystemRights]::FullControl) { exit 8 }exit 0";
const SECURE_FILE:&str="$a=New-Object System.Security.AccessControl.FileSecurity;$a.SetAccessRuleProtection($true,$false);$rule=New-Object System.Security.AccessControl.FileSystemAccessRule -ArgumentList @($sid,[System.Security.AccessControl.FileSystemRights]::FullControl,[System.Security.AccessControl.AccessControlType]::Allow);[void]$a.AddAccessRule($rule);[System.IO.File]::SetAccessControl($p,$a);if ([System.IO.File]::GetAccessControl($p).GetOwner([System.Security.Principal.SecurityIdentifier]).Value -ne $sid.Value) { $o=New-Object System.Security.AccessControl.FileSecurity;$o.SetOwner($sid);[System.IO.File]::SetAccessControl($p,$o) };";
const SECURE_DIRECTORY:&str="$a=New-Object System.Security.AccessControl.DirectorySecurity;$a.SetAccessRuleProtection($true,$false);$inherit=[System.Security.AccessControl.InheritanceFlags]'ContainerInherit,ObjectInherit';$rule=New-Object System.Security.AccessControl.FileSystemAccessRule -ArgumentList @($sid,[System.Security.AccessControl.FileSystemRights]::FullControl,$inherit,[System.Security.AccessControl.PropagationFlags]::None,[System.Security.AccessControl.AccessControlType]::Allow);[void]$a.AddAccessRule($rule);[System.IO.Directory]::SetAccessControl($p,$a);if ([System.IO.Directory]::GetAccessControl($p).GetOwner([System.Security.Principal.SecurityIdentifier]).Value -ne $sid.Value) { $o=New-Object System.Security.AccessControl.DirectorySecurity;$o.SetOwner($sid);[System.IO.Directory]::SetAccessControl($p,$o) };";

use crate::{lifecycle::platform::windows_powershell, live::LiveError};
use base64::{engine::general_purpose::STANDARD, Engine};
use std::{
    io::Read,
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};
fn reason(code: i32) -> Option<&'static str> {
    match code {
        2 => Some("file owner is not the current process token"),
        3 => Some("DACL inheritance protection is disabled"),
        4 => Some("DACL does not contain exactly one access rule"),
        5 => Some("an access rule references a non-owner SID"),
        6 => Some("an access rule is inherited"),
        7 => Some("an access rule is not an allow rule"),
        8 => Some("an access rule does not grant full control"),
        _ => None,
    }
}
struct ResultRow {
    status: Option<i32>,
    ran: bool,
    stderr: String,
}
fn run(path: &Path, script: &str, timeout: u64) -> ResultRow {
    let mut command = Command::new(windows_powershell(None));
    command
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script])
        .env("ABLETON_MCP_ACL_PATH", STANDARD.encode(path.to_string_lossy().as_bytes()))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let Ok(mut child) = command.spawn() else {
        return ResultRow { status: None, ran: false, stderr: String::new() };
    };
    let stderr = child.stderr.take().unwrap();
    let reader = std::thread::spawn(move || {
        let mut output = vec![];
        let _ = stderr.take(1024 * 1024).read_to_end(&mut output);
        output
    });
    let start = Instant::now();
    let (mut status, mut ran) = (None, true);
    loop {
        match child.try_wait() {
            Ok(Some(exit)) => {
                status = exit.code();
                break;
            }
            Ok(None) => {}
            Err(_) => {
                ran = false;
                break;
            }
        }
        if start.elapsed() >= Duration::from_millis(timeout) {
            ran = false;
            let _ = child.kill();
            let _ = child.wait();
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let bytes = reader.join().unwrap_or_default();
    let stderr = String::from_utf8_lossy(&bytes).replace(path.to_string_lossy().as_ref(), "<redacted-path>");
    let whitespace =
        regex::Regex::new(r"[\t-\r \x{00a0}\x{1680}\x{2000}-\x{200a}\x{2028}\x{2029}\x{202f}\x{205f}\x{3000}\x{feff}]+").unwrap();
    let stderr = whitespace.replace_all(&stderr, " ");
    ResultRow { status, ran, stderr: kumi_common::js::string::head(kumi_common::js::string::trim(&stderr), 512) }
}
/// Whether the file is owner-only as the bridge makes it. Kumi's own shape is read with Windows' own calls
/// (`native_acl`); any other is the PowerShell check's to judge.
pub(super) fn owner_only(path: &Path) -> bool {
    #[cfg(windows)]
    if super::native_acl::is_kumis_shape(path) {
        return true;
    }
    let result = run(path, &format!("{WINDOWS_ACL_TARGET}{WINDOWS_ACL_CHECKS}"), 15000);
    result.ran && result.status == Some(0)
}
fn apply(path: &Path, script: &str, what: &str) -> Result<(), LiveError> {
    let result = run(path, &format!("$ErrorActionPreference='Stop';{WINDOWS_ACL_TARGET}{script}{WINDOWS_ACL_CHECKS}"), 30000);
    if result.ran && result.status == Some(0) {
        return Ok(());
    }
    let detail = if !result.ran {
        "the command could not run".into()
    } else if let Some(known) = result.status.and_then(reason) {
        format!("verification rejected the applied descriptor: {known}")
    } else {
        result.stderr
    };
    Err(LiveError::error(format!(
        "could not establish an owner-only Windows {what}{}",
        if detail.is_empty() { String::new() } else { format!(": {detail}") }
    )))
}
/// Gives the file the owner-only DACL and checks it took. Windows' own calls do it in a moment; when one of them fails,
/// PowerShell's step runs, as it always did, with its own words.
pub(super) fn secure_file(path: &Path) -> Result<(), LiveError> {
    #[cfg(windows)]
    if super::native_acl::make_kumis_shape(path).is_ok() {
        return Ok(());
    }
    apply(path, SECURE_FILE, "ACL")
}
pub(super) fn secure_directory(path: &Path) -> Result<(), LiveError> {
    apply(path, SECURE_DIRECTORY, "directory ACL")
}

/// The PowerShell check and step, for the bridge's tests (tests/it/delivery.rs): the Windows fast path is held to them.
/// Not for the bridge itself.
#[cfg(windows)]
pub mod powershell {
    use super::{run, SECURE_DIRECTORY, SECURE_FILE, WINDOWS_ACL_CHECKS, WINDOWS_ACL_TARGET};
    use std::path::Path;

    /// The check on one file, as `owner_only` runs it when the fast path doesn't say: its exit code, 0 owner-only, 2-8
    /// why not (`reason`), -1 when it didn't run.
    pub fn check(path: &Path) -> i32 {
        let result = run(path, &format!("{WINDOWS_ACL_TARGET}{WINDOWS_ACL_CHECKS}"), 60_000);
        if result.ran {
            result.status.unwrap_or(-1)
        } else {
            -1
        }
    }

    /// The same check on many files in one PowerShell, its `exit`s made a function's `return`s: one code per file, as
    /// `check` gives it.
    pub fn checks(paths: &[&Path]) -> Vec<i32> {
        use base64::{engine::general_purpose::STANDARD, Engine};
        let function = WINDOWS_ACL_CHECKS.replace("exit ", "return ");
        let script = format!(
            "$sid=[System.Security.Principal.WindowsIdentity]::GetCurrent().User;function Check($p) {{ {function} }};foreach ($p in ([Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($env:ABLETON_MCP_ACL_PATHS)) -split \"`n\")) {{ Check $p }}"
        );
        let joined = paths.iter().map(|path| path.display().to_string()).collect::<Vec<_>>().join("\n");
        let said = std::process::Command::new(crate::lifecycle::platform::windows_powershell(None))
            .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", &script])
            .env("ABLETON_MCP_ACL_PATHS", STANDARD.encode(joined.as_bytes()))
            .env_remove("PSModulePath")
            .stdin(std::process::Stdio::null())
            .output()
            .expect("PowerShell starts");
        String::from_utf8_lossy(&said.stdout).lines().filter_map(|line| line.trim().parse().ok()).collect()
    }

    /// PowerShell's own step of `secure_directory`: SECURE_DIRECTORY, then the check.
    pub fn secure_directory(path: &Path) -> Result<(), String> {
        let result =
            run(path, &format!("$ErrorActionPreference='Stop';{WINDOWS_ACL_TARGET}{SECURE_DIRECTORY}{WINDOWS_ACL_CHECKS}"), 60_000);
        if result.ran && result.status == Some(0) {
            Ok(())
        } else {
            Err(format!("{:?} {}", result.status, result.stderr))
        }
    }

    /// PowerShell's own step of `secure_file`: SECURE_FILE, then the check.
    pub fn secure(path: &Path) -> Result<(), String> {
        let result = run(path, &format!("$ErrorActionPreference='Stop';{WINDOWS_ACL_TARGET}{SECURE_FILE}{WINDOWS_ACL_CHECKS}"), 60_000);
        if result.ran && result.status == Some(0) {
            Ok(())
        } else {
            Err(format!("{:?} {}", result.status, result.stderr))
        }
    }
}
