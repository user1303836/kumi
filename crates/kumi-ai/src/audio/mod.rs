//! Native audio generation/transcription, independent of conversational model providers.
mod muscriptor;
mod wire;

use crate::auth::store::{Credential, CredentialStore};
use kumi_common::abort::Signal;
use kumi_types::{errors::RuntimeError, system::Env};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{collections::BTreeMap, path::PathBuf, rc::Rc};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Task {
    TextToSfx,
    VideoToSfx,
    Extend,
    Inpaint,
    AudioToMidi,
}
impl Task {
    pub const ALL: [Self; 5] = [Self::TextToSfx, Self::VideoToSfx, Self::Extend, Self::Inpaint, Self::AudioToMidi];
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TextToSfx => "text-to-sfx",
            Self::VideoToSfx => "video-to-sfx",
            Self::Extend => "extend",
            Self::Inpaint => "inpaint",
            Self::AudioToMidi => "audio-to-midi",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::TextToSfx => "Text to sound effects",
            Self::VideoToSfx => "Video to sound effects",
            Self::Extend => "Extend audio",
            Self::Inpaint => "Inpaint audio",
            Self::AudioToMidi => "Audio to MIDI",
        }
    }
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|task| task.as_str() == value)
    }
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    #[serde(default)]
    pub tasks: BTreeMap<Task, TaskConfig>,
}
impl Config {
    pub fn task(&self, task: Task) -> TaskConfig {
        self.tasks.get(&task).cloned().unwrap_or_default()
    }
    pub fn validate(&self) -> Result<(), RuntimeError> {
        for (task, config) in &self.tasks {
            if let Some(endpoint) = &config.local {
                wire::base_url(&endpoint.url).map_err(RuntimeError::plain)?;
                if endpoint.protocol == LocalProtocol::Muscriptor && *task != Task::AudioToMidi {
                    return Err(RuntimeError::plain("MuScriptor supports audio-to-midi only"));
                }
                if endpoint.model.as_ref().is_some_and(|model| model.trim().is_empty() || model == "latest") {
                    return Err(RuntimeError::plain("Choose an explicit audio model release, not an empty name or latest"));
                }
            }
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskConfig {
    #[serde(default = "enabled")]
    pub auto_mirelo: bool,
    #[serde(default)]
    pub local: Option<Endpoint>,
    #[serde(default)]
    pub fallback: bool,
}
fn enabled() -> bool {
    true
}
impl Default for TaskConfig {
    fn default() -> Self {
        Self { auto_mirelo: true, local: None, fallback: false }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Endpoint {
    pub url: String,
    pub protocol: LocalProtocol,
    #[serde(default)]
    pub model: Option<String>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LocalProtocol {
    Muscriptor,
    Mirelo,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Selection {
    #[default]
    Auto,
    Mirelo,
    Local,
}
pub struct ServiceOptions {
    pub store: Rc<dyn CredentialStore>,
    pub env: Env,
    pub read_config: Rc<dyn Fn() -> Config>,
    pub write_config: Rc<dyn Fn(&Config) -> Result<(), RuntimeError>>,
    pub output_dir: PathBuf,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub config: Config,
    pub mirelo_configured: bool,
    pub key_source: Option<String>,
}
pub struct Service {
    options: ServiceOptions,
}
impl Service {
    pub fn new(options: ServiceOptions) -> Rc<Self> {
        Rc::new(Self { options })
    }
    pub fn config(&self) -> Config {
        (self.options.read_config)()
    }
    pub fn configure(&self, config: Config) -> Result<(), RuntimeError> {
        config.validate()?;
        (self.options.write_config)(&config)
    }
    async fn mirelo_key(&self) -> Result<Option<(String, String)>, RuntimeError> {
        if let Some(Credential::ApiKey { key }) = self.options.store.get("mirelo").await? {
            if !key.trim().is_empty() {
                return Ok(Some((key, "saved".into())));
            }
        }
        Ok(self.options.env.get("MIRELO_API_KEY").filter(|key| !key.trim().is_empty()).map(|key| (key.clone(), "MIRELO_API_KEY".into())))
    }
    pub async fn status(&self) -> Result<Status, RuntimeError> {
        let key = self.mirelo_key().await?;
        Ok(Status { config: self.config(), mirelo_configured: key.is_some(), key_source: key.map(|(_, source)| source) })
    }
    pub async fn preferred(&self, task: Task) -> Result<Option<Selection>, RuntimeError> {
        let config = self.config();
        config.validate()?;
        let config = config.task(task);
        if config.auto_mirelo && self.mirelo_key().await?.is_some() {
            Ok(Some(Selection::Mirelo))
        } else if config.local.is_some() {
            Ok(Some(Selection::Local))
        } else {
            Ok(None)
        }
    }
    pub async fn save_mirelo_key(&self, key: &str, signal: &Signal) -> Result<Value, RuntimeError> {
        let key = valid_key(key)?;
        let client = wire::Client::new("https://api.mirelo.ai", Some(&key), false).map_err(wire::Failure::runtime)?;
        let account = client.json(reqwest::Method::GET, "/v3/me", None, signal).await.map_err(wire::Failure::runtime)?;
        if signal.is_cancelled() {
            return Err(RuntimeError::plain("Operation cancelled"));
        }
        self.set_key("mirelo", Some(key)).await?;
        // Never return arbitrary account fields (or an accidentally echoed credential).
        let mut summary = json!({"configured":true});
        for field in ["credits_available", "spend_capacity", "billing_mode", "provisioning_state"] {
            if let Some(value) = account.get(field) {
                if value.is_number()
                    || value.is_null()
                    || matches!(value.as_str(), Some("metered" | "unmetered" | "ready" | "pending" | "stalled"))
                {
                    summary[field] = value.clone();
                }
            }
        }
        Ok(summary)
    }
    pub async fn remove_mirelo_key(&self) -> Result<(), RuntimeError> {
        self.set_key("mirelo", None).await
    }
    pub async fn save_local_key(&self, task: Task, key: &str) -> Result<(), RuntimeError> {
        let config = self.config().task(task);
        let endpoint =
            config.local.as_ref().ok_or_else(|| RuntimeError::plain("Configure this task's local endpoint before its credential"))?;
        let name = local_credential(task, endpoint).map_err(wire::Failure::runtime)?;
        self.set_key(&name, if key.trim().is_empty() { None } else { Some(valid_key(key)?) }).await
    }
    async fn set_key(&self, name: &str, key: Option<String>) -> Result<(), RuntimeError> {
        self.options.store.update(name, Box::new(move |_| Box::pin(async move { Ok(key.map(|key| Credential::ApiKey { key })) }))).await?;
        Ok(())
    }
    pub async fn execute(&self, task: Task, input: Value, selection: Selection, signal: &Signal) -> Result<Value, RuntimeError> {
        if signal.is_cancelled() {
            return Err(RuntimeError::plain("Operation cancelled"));
        }
        let config = self.config();
        config.validate()?;
        let config = config.task(task);
        let key = if selection == Selection::Mirelo || (selection == Selection::Auto && config.auto_mirelo) {
            self.mirelo_key().await?
        } else {
            None
        };
        let selected = if selection == Selection::Auto {
            if config.auto_mirelo && key.is_some() {
                Selection::Mirelo
            } else if config.local.is_some() {
                Selection::Local
            } else {
                return Err(RuntimeError::plain(format!("No audio provider configured for {}", task.as_str())));
            }
        } else {
            selection
        };
        if selected == Selection::Local {
            return self.local(task, &input, &config, signal).await.map_err(wire::Failure::runtime);
        }
        let key = key.ok_or_else(|| RuntimeError::plain("Mirelo is not configured; save a key or set MIRELO_API_KEY"))?.0;
        let client = wire::Client::new("https://api.mirelo.ai", Some(&key), false).map_err(wire::Failure::runtime)?;
        match client.execute(task, &input, None, &self.options.output_dir, signal).await {
            Ok(result) => Ok(result),
            Err(error)
                if selection == Selection::Auto
                    && config.fallback
                    && config.local.is_some()
                    && error.availability
                    && !signal.is_cancelled() =>
            {
                let mut result = self.local(task, &input, &config, signal).await.map_err(wire::Failure::runtime)?;
                result["fallback"] = json!({"from":"mirelo","reason":"Mirelo unavailable before submission; no paid create was sent"});
                Ok(result)
            }
            Err(error) => Err(error.runtime()),
        }
    }
    async fn local(&self, task: Task, input: &Value, config: &TaskConfig, signal: &Signal) -> Result<Value, wire::Failure> {
        let endpoint = config.local.as_ref().ok_or_else(|| wire::Failure::new("No local endpoint configured for this task"))?;
        let key = self
            .options
            .store
            .get(&local_credential(task, endpoint)?)
            .await
            .map_err(|_| wire::Failure::new("Could not read local audio credential"))?;
        let key = match &key {
            Some(Credential::ApiKey { key }) => Some(key.as_str()),
            _ => None,
        };
        let client = wire::Client::new(&endpoint.url, key, true)?;
        match endpoint.protocol {
            LocalProtocol::Muscriptor => muscriptor::execute(&client, input, &self.options.output_dir, signal).await,
            LocalProtocol::Mirelo => client.execute(task, input, endpoint.model.as_deref(), &self.options.output_dir, signal).await,
        }
    }
}
fn valid_key(key: &str) -> Result<String, RuntimeError> {
    let key = key.trim();
    if key.is_empty() || reqwest::header::HeaderValue::from_str(&format!("Bearer {key}")).is_err() {
        return Err(RuntimeError::plain("The audio API key is empty or contains invalid header characters"));
    }
    Ok(key.into())
}
fn local_credential(task: Task, endpoint: &Endpoint) -> Result<String, wire::Failure> {
    use sha2::{Digest, Sha256};
    let origin = wire::base_url(&endpoint.url).map_err(wire::Failure::new)?.origin().ascii_serialization();
    Ok(format!("audio-local:{}:{}", task.as_str(), hex::encode(Sha256::digest(origin.as_bytes()))))
}
