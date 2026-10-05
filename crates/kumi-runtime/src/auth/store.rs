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
const LOCK_STALE_MS: i64 = 30_000;
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
            fs::rename(&temporary, &self.path).await
        }
        .await;
        if result.is_err() {
            let _ = fs::remove_file(&temporary).await;
        }
        result.map_err(io)
    }
    pub async fn update_with<F, Fut>(&self, provider: &str, change: F) -> Result<Option<Credential>, RuntimeError>
    where
        F: FnOnce(Option<Credential>) -> Fut,
        Fut: std::future::Future<Output = Result<Option<Credential>, RuntimeError>>,
    {
        let lock = PathBuf::from(format!("{}.lock", self.path.display()));
        self.mkdir().await?;
        let deadline = now_ms() + LOCK_WAIT_MS;
        loop {
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            options.mode(0o600);
            match options.open(&lock).await {
                Ok(handle) => {
                    drop(handle);
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
        let mut guard = LockGuard(Some(lock));
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
        if let Some(lock) = guard.0.take() {
            match fs::remove_file(lock).await {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(io(error)),
            }
        }
        result
    }
}
struct LockGuard(Option<PathBuf>);
impl Drop for LockGuard {
    fn drop(&mut self) {
        if let Some(lock) = &self.0 {
            let _ = std::fs::remove_file(lock);
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
