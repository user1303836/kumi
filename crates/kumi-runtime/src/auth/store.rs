use crate::core::errors::{FailureKind, KumiError, RuntimeError};
use async_trait::async_trait;
use futures::future::LocalBoxFuture;
use indexmap::IndexMap;
use kumi_common::{js::json::file_text, time::now_ms};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::{fs, io::AsyncWriteExt};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthCredential {
    pub access: String,
    pub refresh: String,
    /// Epoch milliseconds.
    pub expires: f64,
    pub account_id: String,
}
/// A provider API key; converting to `Credential` adds its serialized discriminator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiKeyCredential {
    pub key: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Credential {
    Oauth(OAuthCredential),
    ApiKey { key: String },
}
impl From<ApiKeyCredential> for Credential {
    fn from(value: ApiKeyCredential) -> Self {
        Self::ApiKey { key: value.key }
    }
}
impl From<OAuthCredential> for Credential {
    fn from(value: OAuthCredential) -> Self {
        Self::Oauth(value)
    }
}
impl Credential {
    pub fn valid(&self) -> bool {
        match self {
            Self::ApiKey { key } => valid_api_key(key),
            Self::Oauth(value) => {
                !value.access.is_empty() && !value.refresh.is_empty() && value.expires.is_finite() && !value.account_id.is_empty()
            }
        }
    }
}
pub type CredentialChange = Box<dyn FnOnce(Option<Credential>) -> LocalBoxFuture<'static, Result<Option<Credential>, RuntimeError>>>;
#[async_trait(?Send)]
pub trait CredentialStore {
    fn path(&self) -> &Path;
    async fn get(&self, provider: &str) -> Result<Option<Credential>, RuntimeError>;
    async fn list(&self) -> Result<IndexMap<String, Credential>, RuntimeError>;
    /// Read-modify-write under an exclusive cross-process lock; None removes the entry.
    async fn update(&self, provider: &str, change: CredentialChange) -> Result<Option<Credential>, RuntimeError>;
}
/// How old a lock is before it's taken for one a stopped Kumi left: longer than any hold, as a token refresh inside
/// one gives up at 30 s (at 30 s a slow refresh's lock was broken, and two processes posted one rotating token).
const LOCK_STALE_MS: i64 = 60_000;
const LOCK_WAIT_MS: i64 = 10_000;
#[derive(Clone)]
pub struct FileCredentialStore {
    path: PathBuf,
}
/// Owner-only JSON file. Credentials never leave it except as request headers.
pub fn open_credential_store(path: impl Into<PathBuf>) -> FileCredentialStore {
    FileCredentialStore { path: path.into() }
}
fn io(error: std::io::Error) -> RuntimeError {
    RuntimeError::plain(error.to_string())
}
fn auth(message: impl Into<String>) -> RuntimeError {
    KumiError::new(FailureKind::Auth, message).into()
}

// On Windows the credential file is made owner-only as the bridge makes its own files: a protected DACL whose one rule
// gives the current user full control, then checked. These are the twins of delivery_acl.rs's WINDOWS_ACL_TARGET,
// WINDOWS_ACL_CHECKS and SECURE_FILE (in ableton-mcp-server): keep them in step. KUMI_HOME or KUMI_AUTH_FILE can put
// auth.json where other accounts read (D:\kumi), and login says it's owner-only.
const ACL_TARGET: &str = "$p=[Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($env:KUMI_ACL_PATH));$sid=[System.Security.Principal.WindowsIdentity]::GetCurrent().User;";
const ACL_CHECKS: &str = "$c=[System.IO.File]::GetAccessControl($p);if ($c.GetOwner([System.Security.Principal.SecurityIdentifier]).Value -ne $sid.Value) { exit 2 }if (-not $c.AreAccessRulesProtected) { exit 3 }$rules=@($c.Access); if ($rules.Count -ne 1) { exit 4 }$rule=$rules[0];if ($rule.IdentityReference.Translate([System.Security.Principal.SecurityIdentifier]).Value -ne $sid.Value) { exit 5 }if ($rule.IsInherited) { exit 6 }if ($rule.AccessControlType.ToString() -ne 'Allow') { exit 7 }if (($rule.FileSystemRights -band [System.Security.AccessControl.FileSystemRights]::FullControl) -ne [System.Security.AccessControl.FileSystemRights]::FullControl) { exit 8 }exit 0";
const SECURE_FILE: &str = "$a=New-Object System.Security.AccessControl.FileSecurity;$a.SetAccessRuleProtection($true,$false);$rule=New-Object System.Security.AccessControl.FileSystemAccessRule -ArgumentList @($sid,[System.Security.AccessControl.FileSystemRights]::FullControl,[System.Security.AccessControl.AccessControlType]::Allow);[void]$a.AddAccessRule($rule);[System.IO.File]::SetAccessControl($p,$a);if ([System.IO.File]::GetAccessControl($p).GetOwner([System.Security.Principal.SecurityIdentifier]).Value -ne $sid.Value) { $o=New-Object System.Security.AccessControl.FileSecurity;$o.SetOwner($sid);[System.IO.File]::SetAccessControl($p,$o) };";
/// What a check that failed found, by its exit code (as delivery_acl.rs says them).
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
/// Runs an ACL script on `path` in Windows PowerShell, with no window, nothing on stdin and at most `timeout_ms`:
/// Ok when it exits 0, else why not.
fn run_acl(path: &Path, script: &str, timeout_ms: u64) -> Result<(), String> {
    use base64::{engine::general_purpose::STANDARD, Engine};
    use std::{io::Read, process::Stdio};
    let mut command = std::process::Command::new(crate::system::system_program_default(crate::system::SystemProgram::Powershell));
    command
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script])
        .env("KUMI_ACL_PATH", STANDARD.encode(path.to_string_lossy().as_bytes()))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
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
        Some(code) => Err(acl_reason(code)
            .map(str::to_string)
            .unwrap_or_else(|| kumi_common::js::string::head(kumi_common::js::string::trim(&said), 300))),
        None => Err("PowerShell stopped".to_string()),
    }
}
/// Gives the file a protected DACL with one rule, full control for you, and checks it took (Windows).
fn make_owner_only(path: &Path) -> Result<(), String> {
    run_acl(path, &format!("$ErrorActionPreference='Stop';{ACL_TARGET}{SECURE_FILE}{ACL_CHECKS}"), 30_000)
}
/// Whether the file is owner-only as Kumi makes it on Windows: you own it, and one rule, not inherited, gives you full
/// control.
pub fn windows_owner_only(path: &Path) -> bool {
    run_acl(path, &format!("{ACL_TARGET}{ACL_CHECKS}"), 15_000).is_ok()
}

impl FileCredentialStore {
    async fn read(&self) -> Result<Value, RuntimeError> {
        let text: Result<Result<String, RuntimeError>, std::io::Error> = async {
            // Windows profile folders supply the privacy guarantee; POSIX mode bits are checked.
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let metadata = fs::metadata(&self.path).await?;
                if metadata.permissions().mode() & 0o077 != 0 {
                    return Ok::<_, std::io::Error>(Err(auth(format!(
                        "Credential file {} is readable by other users; run: chmod 600 {}",
                        self.path.display(),
                        self.path.display()
                    ))));
                }
            }
            Ok(Ok(fs::read_to_string(&self.path).await?))
        }
        .await;
        let text = match text {
            Ok(text) => text?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(json!({"version":1,"credentials":{}})),
            Err(error) => return Err(io(error)),
        };
        let data = serde_json::from_str::<Value>(&text).ok().filter(|data| {
            data.get("version").and_then(Value::as_f64) == Some(1.0)
                && data.get("credentials").and_then(Value::as_object).is_some_and(|credentials| {
                    credentials
                        .values()
                        .all(|value| serde_json::from_value::<Credential>(value.clone()).is_ok_and(|credential| credential.valid()))
                })
        });
        data.ok_or_else(|| auth(format!("Credential file {} is malformed; remove it and sign in again.", self.path.display())))
    }
    async fn mkdir(&self) -> Result<(), RuntimeError> {
        let parent = self.path.parent().filter(|path| !path.as_os_str().is_empty()).unwrap_or(Path::new("."));
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        builder.mode(0o700);
        builder.create(parent).await.map_err(io)
    }
    async fn write(&self, data: &Value) -> Result<(), RuntimeError> {
        self.mkdir().await?;
        let mut random = [0u8; 6];
        rand::rng().fill_bytes(&mut random);
        let temporary = PathBuf::from(format!("{}.{}.{}.tmp", self.path.display(), std::process::id(), hex::encode(random)));
        let result = async {
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            options.mode(0o600);
            let mut handle = options.open(&temporary).await?;
            handle.write_all(file_text(data).as_bytes()).await?;
            handle.sync_all().await?;
            drop(handle);
            // Owner-only before it takes the credential file's place (a rename keeps its DACL), on a blocking thread:
            // PowerShell takes about a second.
            if cfg!(windows) {
                let secured = temporary.clone();
                let made = tokio::task::spawn_blocking(move || make_owner_only(&secured))
                    .await
                    .unwrap_or_else(|error| Err(error.to_string()));
                if let Err(why) = made {
                    return Ok(Err(auth(format!(
                        "Kumi couldn't make the credential file {} readable only by you ({why}); keep it in your user folder (unset KUMI_HOME or KUMI_AUTH_FILE), or give that folder's permissions to you alone.",
                        self.path.display()
                    ))));
                }
            }
            fs::rename(&temporary, &self.path).await.map(Ok)
        }
        .await;
        if !matches!(result, Ok(Ok(()))) {
            let _ = fs::remove_file(&temporary).await;
        }
        result.map_err(io)?
    }
    pub async fn update_with<F, Fut>(&self, provider: &str, change: F) -> Result<Option<Credential>, RuntimeError>
    where
        F: FnOnce(Option<Credential>) -> Fut,
        Fut: std::future::Future<Output = Result<Option<Credential>, RuntimeError>>,
    {
        let lock = PathBuf::from(format!("{}.lock", self.path.display()));
        self.mkdir().await?;
        // Written in the lock, it says whose the lock is: a release removes only its own.
        let mut nonce = [0u8; 16];
        rand::rng().fill_bytes(&mut nonce);
        let nonce = hex::encode(nonce);
        let deadline = now_ms() + LOCK_WAIT_MS;
        loop {
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            options.mode(0o600);
            match options.open(&lock).await {
                Ok(mut handle) => {
                    let written = async {
                        handle.write_all(nonce.as_bytes()).await?;
                        handle.flush().await
                    }
                    .await;
                    drop(handle);
                    if let Err(error) = written {
                        let _ = fs::remove_file(&lock).await;
                        return Err(io(error));
                    }
                    break;
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    let age = fs::metadata(&lock)
                        .await
                        .ok()
                        .and_then(|metadata| metadata.modified().ok())
                        .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|modified| now_ms() - modified.as_millis() as i64)
                        .unwrap_or(0);
                    if age > LOCK_STALE_MS {
                        match fs::remove_file(&lock).await {
                            Ok(()) => {}
                            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                            Err(error) => return Err(io(error)),
                        };
                        continue;
                    }
                    if now_ms() > deadline {
                        return Err(auth("The credential store is locked by another Kumi process; try again."));
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Err(error) => return Err(io(error)),
            }
        }
        let mut guard = LockGuard(Some((lock, nonce)));
        let result = async {
            let mut data = self.read().await?;
            let current = data["credentials"]
                .get(provider)
                .cloned()
                .map(serde_json::from_value)
                .transpose()
                .map_err(|_| auth("Refusing to store a malformed credential."))?;
            let next = change(current).await?;
            let credentials = data["credentials"].as_object_mut().expect("validated credentials");
            if let Some(next) = &next {
                if !next.valid() {
                    return Err(auth("Refusing to store a malformed credential."));
                }
                credentials.insert(provider.into(), serde_json::to_value(next).expect("credential serializes"));
            } else {
                credentials.shift_remove(provider);
            }
            self.write(&data).await?;
            Ok(next)
        }
        .await;
        if let Some((lock, nonce)) = guard.0.take() {
            // Broken as stale while this held it, the lock is another's now: it stays.
            match fs::read(&lock).await {
                Ok(held) if held == nonce.as_bytes() => match fs::remove_file(&lock).await {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(io(error)),
                },
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(io(error)),
            }
        }
        result
    }
}
/// The lock this holds, and its nonce, removed if it's still this one's when the hold ends early.
struct LockGuard(Option<(PathBuf, String)>);
impl Drop for LockGuard {
    fn drop(&mut self) {
        if let Some((lock, nonce)) = &self.0 {
            if std::fs::read(lock).is_ok_and(|held| held == nonce.as_bytes()) {
                let _ = std::fs::remove_file(lock);
            }
        }
    }
}
#[async_trait(?Send)]
impl CredentialStore for FileCredentialStore {
    fn path(&self) -> &Path {
        &self.path
    }
    async fn get(&self, provider: &str) -> Result<Option<Credential>, RuntimeError> {
        self.read().await?["credentials"]
            .get(provider)
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .map_err(|_| auth("Refusing to store a malformed credential."))
    }
    async fn list(&self) -> Result<IndexMap<String, Credential>, RuntimeError> {
        self.read().await?["credentials"]
            .as_object()
            .expect("validated credentials")
            .iter()
            .map(|(provider, value)| {
                serde_json::from_value(value.clone())
                    .map(|credential| (provider.clone(), credential))
                    .map_err(|_| auth("Refusing to store a malformed credential."))
            })
            .collect()
    }
    async fn update(&self, provider: &str, change: CredentialChange) -> Result<Option<Credential>, RuntimeError> {
        self.update_with(provider, change).await
    }
}
/// An API key as providers issue them: one word of printable characters.
pub fn valid_api_key(value: &str) -> bool {
    (8..=4096).contains(&value.len()) && value.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
}
