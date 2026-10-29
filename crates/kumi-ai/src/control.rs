//! The app's model controls, as the session and the screen reach them: the model chosen now, the providers' sign-ins
//! and models, the servers on this computer, and ChatGPT's sign-in. The app implements `ModelController`; the trait
//! lives here, below the session, so the session's protocol can name what it answers.

use crate::{
    auth::openai_codex::DevicePrompt,
    providers::{
        models::{ApiKeyCheck, EffortInfo, ModelInfo, ServiceTier},
        Effort, ProviderId, SignIn,
    },
};
use kumi_common::abort::Signal;
use kumi_types::errors::RuntimeError;
use serde::Serialize;
use std::rc::Rc;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderStatus {
    pub id: ProviderId,
    pub name: String,
    pub sign_in: SignIn,
    pub signed_in: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_page: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_env: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LocalStatus {
    pub id: String,
    pub name: String,
    pub r#where: String,
    pub running: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CurrentModel {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<Effort>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_effort: Option<Effort>,
    pub efforts: Vec<EffortInfo>,
    pub pinned: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#where: Option<String>,
    /// The faster tier in use ("Fast"): `/fast` is on and this model's provider offers one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fast: Option<ServiceTier>,
}
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DefaultModel {
    #[serde(flatten)]
    pub model: ModelInfo,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}
pub enum ChatGptSignIn {
    Browser { signal: Signal, on_url: Rc<dyn Fn(String)> },
    Device { signal: Signal, on_code: Rc<dyn Fn(DevicePrompt)> },
}

/// The app-facing model controls, including the injectable source test contract.
#[async_trait::async_trait(?Send)]
pub trait ModelController {
    fn current(&self) -> CurrentModel;
    async fn providers(&self) -> Result<Vec<ProviderStatus>, RuntimeError>;
    async fn local(&self) -> Vec<LocalStatus>;
    fn provider_name(&self, id: &str) -> String;
    /// The cloud provider `id` names (an error's or a model's provider), unless it's a model server's.
    fn cloud_provider(&self, id: &str) -> Option<ProviderId> {
        ProviderId::parse(id)
    }
    async fn choose_default(&self) -> Result<Option<DefaultModel>, RuntimeError>;
    async fn models(&self, provider: &str, refresh: bool) -> Result<Vec<ModelInfo>, RuntimeError>;
    /// The tokens `model` reads at once, as its provider's list (or its server) says: the context meter's whole.
    fn context_window(&self, _model: &str) -> Option<f64> {
        None
    }
    async fn choose(&self, next: &str) -> Result<Option<String>, RuntimeError>;
    async fn set_effort(&self, next: Option<Effort>) -> Result<(), RuntimeError>;
    /// Whether `/fast` is on (the setting, whether or not this model has a faster tier).
    fn fast_enabled(&self) -> bool {
        false
    }
    /// The model that carries on while the chosen one's plan limit lasts or its provider is down.
    fn fallback(&self) -> Option<String> {
        None
    }
    /// Sets (or, with None, clears) the fallback; from the next message.
    async fn set_fallback(&self, model: Option<&str>) -> Result<(), RuntimeError> {
        let _ = model;
        Ok(())
    }
    /// `/fast`: the model's faster tier on or off; None when turning it on and the model has none.
    async fn set_fast(&self, on: bool) -> Result<Option<ServiceTier>, RuntimeError> {
        let _ = on;
        Ok(None)
    }
    async fn save_key(&self, provider: ProviderId, key: &str, signal: Option<Signal>) -> Result<ApiKeyCheck, RuntimeError>;
    async fn sign_in_chatgpt(&self, io: ChatGptSignIn) -> Result<(), RuntimeError>;
    async fn sign_out(&self, provider: ProviderId) -> Result<bool, RuntimeError>;
}

/// The saved model, when its provider's own list (the one /model shows, read once a session) no longer shows it:
/// "ChatGPT no longer offers gpt-5.5 to your sign-in". None while it's listed, for a model `KUMI_MODEL` picks or one on
/// this computer, and when the list can't be read (no sign-in, no network): then nothing new happens.
pub async fn missing_model(models: &dyn ModelController) -> Option<String> {
    let current = models.current();
    if current.r#where.is_some() {
        return None;
    }
    let (model, provider) = (current.model.filter(|_| !current.pinned)?, current.provider?);
    ProviderId::parse(&provider)?;
    let listed = models.models(&provider, false).await.ok()?;
    if listed.is_empty() || listed.iter().any(|info| info.id == model) {
        return None;
    }
    let shown = model.strip_prefix(&format!("{provider}/")).unwrap_or(&model);
    Some(format!("{} no longer offers {shown} to your sign-in", models.provider_name(&provider)))
}
