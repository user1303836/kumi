//! Windows owner-only ACL scripts and their bounded execution.
const WINDOWS_ACL_TARGET:&str="$p=[Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($env:ABLETON_MCP_ACL_PATH));$sid=[System.Security.Principal.WindowsIdentity]::GetCurrent().User;";
const WINDOWS_ACL_CHECKS:&str="$c=[System.IO.File]::GetAccessControl($p);if ($c.GetOwner([System.Security.Principal.SecurityIdentifier]).Value -ne $sid.Value) { exit 2 }if (-not $c.AreAccessRulesProtected) { exit 3 }$rules=@($c.Access); if ($rules.Count -ne 1) { exit 4 }$rule=$rules[0];if ($rule.IdentityReference.Translate([System.Security.Principal.SecurityIdentifier]).Value -ne $sid.Value) { exit 5 }if ($rule.IsInherited) { exit 6 }if ($rule.AccessControlType.ToString() -ne 'Allow') { exit 7 }if (($rule.FileSystemRights -band [System.Security.AccessControl.FileSystemRights]::FullControl) -ne [System.Security.AccessControl.FileSystemRights]::FullControl) { exit 8 }exit 0";
const SECURE_FILE:&str="$a=New-Object System.Security.AccessControl.FileSecurity;$a.SetAccessRuleProtection($true,$false);$rule=New-Object System.Security.AccessControl.FileSystemAccessRule -ArgumentList @($sid,[System.Security.AccessControl.FileSystemRights]::FullControl,[System.Security.AccessControl.AccessControlType]::Allow);[void]$a.AddAccessRule($rule);[System.IO.File]::SetAccessControl($p,$a);if ([System.IO.File]::GetAccessControl($p).GetOwner([System.Security.Principal.SecurityIdentifier]).Value -ne $sid.Value) { $o=New-Object System.Security.AccessControl.FileSecurity;$o.SetOwner($sid);[System.IO.File]::SetAccessControl($p,$o) };";
const SECURE_DIRECTORY:&str="$a=New-Object System.Security.AccessControl.DirectorySecurity;$a.SetAccessRuleProtection($true,$false);$inherit=[System.Security.AccessControl.InheritanceFlags]'ContainerInherit,ObjectInherit';$rule=New-Object System.Security.AccessControl.FileSystemAccessRule -ArgumentList @($sid,[System.Security.AccessControl.FileSystemRights]::FullControl,$inherit,[System.Security.AccessControl.PropagationFlags]::None,[System.Security.AccessControl.AccessControlType]::Allow);[void]$a.AddAccessRule($rule);[System.IO.Directory]::SetAccessControl($p,$a);if ([System.IO.Directory]::GetAccessControl($p).GetOwner([System.Security.Principal.SecurityIdentifier]).Value -ne $sid.Value) { $o=New-Object System.Security.AccessControl.DirectorySecurity;$o.SetOwner($sid);[System.IO.Directory]::SetAccessControl($p,$o) };";

use crate::{live::LiveError, platform::windows_powershell};
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
pub(super) fn owner_only(path: &Path) -> bool {
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
pub(super) fn secure_file(path: &Path) -> Result<(), LiveError> {
    apply(path, SECURE_FILE, "ACL")
}
pub(super) fn secure_directory(path: &Path) -> Result<(), LiveError> {
    apply(path, SECURE_DIRECTORY, "directory ACL")
}
