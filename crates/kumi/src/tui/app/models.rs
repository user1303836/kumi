//! The session's models as the screen reaches them: what it read and asked of a model controller before, now through the
//! session's protocol. What's chosen is read at once from the session; the rest are requests.
use super::*;
use kumi_agent::core::protocol::{ModelsView, Secret};
use kumi_ai::{
    control::{CurrentModel, DefaultModel, LocalStatus, ProviderStatus},
    providers::{
        models::{ApiKeyCheck, ModelInfo, ServiceTier},
        Effort, ProviderId,
    },
};

/// The session's models, for a screen that has them ([`TuiApp::models`]).
#[derive(Clone)]
pub(super) struct Models(Rc<dyn SessionHandle>);

impl Models {
    fn view(&self) -> ModelsView {
        // (the session's models stay as they are while it runs: a screen that has them always reads them)
        self.0.models().unwrap_or_else(|| ModelsView { current: no_model(), fast: false, fallback: None })
    }
    pub(super) fn current(&self) -> CurrentModel {
        self.view().current
    }
    pub(super) fn fast_enabled(&self) -> bool {
        self.view().fast
    }
    pub(super) fn fallback(&self) -> Option<String> {
        self.view().fallback
    }
    pub(super) fn provider_name(&self, id: &str) -> String {
        self.0.provider_name(id)
    }
    pub(super) fn cloud_provider(&self, id: &str) -> Option<ProviderId> {
        self.0.cloud_provider(id)
    }
    pub(super) fn context_window(&self, model: &str) -> Option<f64> {
        self.0.context_window(model)
    }
    pub(super) async fn providers(&self) -> Result<Vec<ProviderStatus>, RuntimeError> {
        self.0.op(op::Providers {}).await
    }
    pub(super) async fn local(&self) -> Vec<LocalStatus> {
        self.0.op(op::LocalModels {}).await.unwrap_or_default()
    }
    pub(super) async fn choose_default(&self) -> Result<Option<DefaultModel>, RuntimeError> {
        self.0.op(op::ChooseDefault {}).await
    }
    pub(super) async fn models(&self, provider: &str, refresh: bool) -> Result<Vec<ModelInfo>, RuntimeError> {
        self.0.op(op::Models { provider: provider.into(), refresh }).await
    }
    pub(super) async fn choose(&self, model: &str) -> Result<Option<String>, RuntimeError> {
        self.0.op(op::ChooseModel { model: model.into() }).await
    }
    pub(super) async fn set_effort(&self, effort: Option<Effort>) -> Result<(), RuntimeError> {
        self.0.op(op::SetEffort { effort }).await
    }
    pub(super) async fn set_fast(&self, on: bool) -> Result<Option<ServiceTier>, RuntimeError> {
        self.0.op(op::SetFast { on }).await
    }
    pub(super) async fn set_fallback(&self, model: Option<&str>) -> Result<(), RuntimeError> {
        self.0.op(op::SetFallback { model: model.map(str::to_string) }).await
    }
    pub(super) async fn save_key(&self, provider: ProviderId, key: &str) -> Result<ApiKeyCheck, RuntimeError> {
        self.0.op(op::SaveKey { provider, key: Secret(key.into()) }).await
    }
    pub(super) async fn sign_out(&self, provider: ProviderId) -> Result<bool, RuntimeError> {
        self.0.op(op::SignOut { provider }).await
    }
    /// ChatGPT's sign-in `id`, in the browser or `by_code`: its address or code come as `sign-in` events.
    pub(super) async fn sign_in_chatgpt(&self, id: u32, by_code: bool) -> Result<(), RuntimeError> {
        self.0.op(op::SignInChatgpt { id, by_code }).await
    }
    /// The saved model, when its provider's own list no longer shows it: what to say, or None.
    pub(super) async fn missing(&self) -> Option<String> {
        self.0.op(op::MissingModel {}).await.ok().flatten()
    }
    /// Stops ChatGPT's sign-in `id` (before it starts, too).
    pub(super) fn stop_sign_in(&self, id: u32) {
        self.0.tell(op::StopSignIn { id }.into());
    }
}

/// What a session without a model chosen says it has.
fn no_model() -> CurrentModel {
    CurrentModel {
        model: None,
        provider: None,
        name: None,
        effort: None,
        default_effort: None,
        efforts: vec![],
        pinned: false,
        r#where: None,
        fast: None,
    }
}

impl TuiApp {
    /// The session's models, where it has them (`kumi -p`'s and a test's may not).
    pub(super) fn models(&self) -> Option<Models> {
        self.can().models.then(|| Models(self.0.options.session.clone()))
    }
}
