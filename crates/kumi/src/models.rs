//! Port of `apps/kumi/src/models.ts`.
use crate::config::{read_settings, write_settings};
use futures::{
    future::{LocalBoxFuture, Shared},
    FutureExt,
};
use kumi_common::{
    abort::{self, Signal},
    js::string::trim,
};
use kumi_runtime::{
    ai::http::Fetch,
    auth::{
        openai_codex::{login_codex_browser, login_codex_device, DevicePrompt, LoginOptions, OPENAI_CODEX},
        store::{Credential, CredentialStore},
    },
    core::errors::{FailureKind, KumiError, RuntimeError},
    kernel::agent::ModelBinding,
    providers::{
        api_key_for,
        local::{self, LocalBinding, LocalKind, LocalModelOptions, LocalServer},
        models::{check_api_key, list_models, ApiKeyCheck, EffortInfo, ListOptions, ModelInfo, ServiceTier, Transport},
        parse_model_id, provider_info, resolve_model, Effort, KeySource, ProviderId, ResolveModelOptions, SignIn, PROVIDERS,
    },
    system::Env,
};
use serde::Serialize;
use serde_json::json;
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    rc::Rc,
};
pub const OFFER_ORDER: [ProviderId; 5] =
    [ProviderId::OpenaiCodex, ProviderId::Anthropic, ProviderId::Openai, ProviderId::Opencode, ProviderId::OpencodeGo];
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
pub type Changed = Rc<dyn Fn() -> LocalBoxFuture<'static, Result<(), RuntimeError>>>;
pub struct ModelControlOptions {
    pub store: Rc<dyn CredentialStore>,
    pub settings_file: String,
    pub env: Env,
    pub changed: Changed,
    pub fetch: Option<Rc<dyn Fetch>>,
    pub say: Option<Rc<dyn Fn(String)>>,
    pub installed: Option<Rc<dyn Fn(LocalKind) -> bool>>,
}
pub enum ChatGptSignIn {
    Browser { signal: Signal, on_url: Rc<dyn Fn(String)> },
    Device { signal: Signal, on_code: Rc<dyn Fn(DevicePrompt)> },
}
#[derive(Clone)]
enum Bound {
    Cloud(Rc<ModelBinding>),
    Local(Rc<LocalBinding>),
}
impl Bound {
    fn original(&self) -> &ModelBinding {
        match self {
            Self::Cloud(b) => b,
            Self::Local(b) => &b.binding,
        }
    }
    fn binding(&self) -> ModelBinding {
        let original = self.original();
        let prepare = self.clone();
        let budget = self.clone();
        ModelBinding {
            id: original.id.clone(),
            model: original.model.clone(),
            prepare: Box::new(move |request| (prepare.original().prepare)(request)),
            budget: original.budget.as_ref().map(|_| Box::new(move |fixed| (budget.original().budget.as_ref().unwrap())(fixed)) as Box<_>),
        }
    }
    fn asked(&self) -> Option<Shared<LocalBoxFuture<'static, ()>>> {
        if let Self::Local(b) = self {
            Some(b.asked.clone())
        } else {
            None
        }
    }
    fn note(&self) -> Option<String> {
        if let Self::Local(b) = self {
            b.note()
        } else {
            None
        }
    }
}
pub struct BindingInfo {
    pub binding: ModelBinding,
    pub asked: Option<Shared<LocalBoxFuture<'static, ()>>>,
    backing: Bound,
}
impl BindingInfo {
    pub fn note(&self) -> Option<String> {
        self.backing.note()
    }
}
#[derive(Clone)]
struct CachedBinding {
    model: String,
    effort: Option<Effort>,
    tier: Option<String>,
    binding: Bound,
}
type Defaulting = Shared<LocalBoxFuture<'static, Result<Option<DefaultModel>, RuntimeError>>>;
struct State {
    model: Option<String>,
    effort: Option<Effort>,
    fast: bool,
    catalog: HashMap<String, Vec<ModelInfo>>,
    bound: Option<CachedBinding>,
    named: Option<Vec<LocalServer>>,
    heard: HashSet<String>,
    defaulting: Option<Defaulting>,
}
struct Inner {
    options: ModelControlOptions,
    pinned: bool,
    state: RefCell<State>,
}
#[derive(Clone)]
pub struct ModelControl {
    inner: Rc<Inner>,
}
struct Parsed {
    provider: String,
    model: String,
    server: Option<LocalServer>,
}
pub fn create_model_control(options: ModelControlOptions) -> ModelControl {
    let settings = read_settings(&options.settings_file);
    let model = options.env.get("KUMI_MODEL").cloned().or(settings.model);
    let pinned = options.env.get("KUMI_MODEL").is_some_and(|s| !s.is_empty());
    ModelControl {
        inner: Rc::new(Inner {
            options,
            pinned,
            state: RefCell::new(State {
                model,
                effort: settings.effort,
                fast: settings.fast == Some(true),
                catalog: HashMap::new(),
                bound: None,
                named: None,
                heard: HashSet::new(),
                defaulting: None,
            }),
        }),
    }
}
impl ModelControl {
    fn servers(&self) -> Vec<LocalServer> {
        if let Some(named) = &self.inner.state.borrow().named {
            return named.clone();
        }
        let named = local::local_servers(&read_settings(&self.inner.options.settings_file).model_servers, &self.inner.options.env)
            .expect("validated model server settings");
        self.inner.state.borrow_mut().named = Some(named.clone());
        named
    }
    fn parse(&self, id: Option<&str>) -> Option<Parsed> {
        let id = id.filter(|s| !s.is_empty())?;
        if let Some(cloud) = parse_model_id(id) {
            return Some(Parsed { provider: cloud.provider.to_string(), model: cloud.model, server: None });
        }
        local::parse_local_model_id(id, &self.servers()).map(|local| Parsed {
            provider: local.server.id.clone(),
            model: local.model,
            server: Some(local.server),
        })
    }
    fn info_for(&self, id: Option<&str>) -> Option<ModelInfo> {
        let parsed = self.parse(id)?;
        self.inner.state.borrow().catalog.get(&parsed.provider)?.iter().find(|i| Some(i.id.as_str()) == id).cloned()
    }
    fn once(&self, note: Option<String>) -> Option<String> {
        note.filter(|s| !s.is_empty()).filter(|s| self.inner.state.borrow_mut().heard.insert(s.clone()))
    }
    fn say(&self, note: String) {
        if let Some(fresh) = self.once(Some(note)) {
            if let Some(say) = &self.inner.options.say {
                say(fresh);
            }
        }
    }
    fn save(&self) -> Result<(), RuntimeError> {
        let state = self.inner.state.borrow();
        let kept = if self.inner.pinned { read_settings(&self.inner.options.settings_file).model } else { state.model.clone() };
        let mut settings = json!({});
        if let Some(kept) = kept.filter(|s| !s.is_empty()) {
            settings["model"] = json!(kept);
        }
        if let Some(effort) = state.effort {
            settings["effort"] = json!(effort);
        }
        if state.fast {
            settings["fast"] = json!(true);
        }
        write_settings(&self.inner.options.settings_file, &settings)
    }
    /// The faster tier this model's provider lists for it, if any (read from its model list, once).
    async fn tier_of(&self, id: &str) -> Option<ServiceTier> {
        let parsed = self.parse(Some(id))?;
        if parsed.server.is_some() {
            return None;
        }
        if self.info_for(Some(id)).is_none() {
            let _ = self.models(&parsed.provider, false).await;
        }
        self.info_for(Some(id))?.service_tiers.into_iter().next()
    }
    /// The tier to ask for: `/fast` is on and the model has one.
    async fn tier_for(&self, id: &str) -> Option<String> {
        if !self.inner.state.borrow().fast {
            return None;
        }
        self.tier_of(id).await.map(|tier| tier.id)
    }
    async fn bind(&self, id: &str, effort: Option<Effort>, tier: Option<String>) -> Result<Bound, RuntimeError> {
        if let Some(Parsed { server: Some(server), model, .. }) = self.parse(Some(id)) {
            let weak = Rc::downgrade(&self.inner);
            return Ok(Bound::Local(Rc::new(local::resolve_local_model(
                server,
                model,
                LocalModelOptions {
                    fetch: self.inner.options.fetch.clone(),
                    effort,
                    on_note: Some(Rc::new(move |note| {
                        if let Some(inner) = weak.upgrade() {
                            ModelControl { inner }.say(note);
                        }
                    })),
                },
            ))));
        }
        Ok(Bound::Cloud(Rc::new(
            resolve_model(ResolveModelOptions {
                model: id.into(),
                store: self.inner.options.store.clone(),
                env: Some(self.inner.options.env.clone()),
                fetch: self.inner.options.fetch.clone(),
                effort,
                service_tier: tier,
            })
            .await?,
        )))
    }
    pub fn current(&self) -> CurrentModel {
        let (model, effort) = {
            let state = self.inner.state.borrow();
            (state.model.clone(), state.effort)
        };
        let info = self.info_for(model.as_deref());
        let parsed = self.parse(model.as_deref());
        let fast = self.inner.state.borrow().fast;
        CurrentModel {
            fast: info.as_ref().filter(|_| fast).and_then(|i| i.service_tiers.first()).cloned(),
            model: model.filter(|s| !s.is_empty()),
            provider: parsed.as_ref().map(|p| p.provider.clone()),
            name: info.as_ref().map(|i| i.name.clone()),
            effort,
            default_effort: info.as_ref().and_then(|i| i.default_effort),
            efforts: info.map(|i| i.efforts).unwrap_or_default(),
            pinned: self.inner.pinned,
            r#where: parsed.and_then(|p| p.server.map(|s| s.r#where)),
        }
    }
    pub async fn providers(&self) -> Result<Vec<ProviderStatus>, RuntimeError> {
        let mut statuses = Vec::new();
        for id in OFFER_ORDER.into_iter().chain(PROVIDERS.into_iter().filter(|id| !OFFER_ORDER.contains(id))) {
            let info = provider_info(id);
            let via = if info.sign_in == SignIn::Chatgpt {
                matches!(self.inner.options.store.get(OPENAI_CODEX).await?, Some(Credential::Oauth(_))).then(|| "chatgpt".into())
            } else {
                api_key_for(id, self.inner.options.store.as_ref(), Some(&self.inner.options.env)).await?.map(|k| {
                    if k.source == KeySource::Env {
                        "environment".into()
                    } else {
                        "saved key".into()
                    }
                })
            };
            statuses.push(ProviderStatus {
                id,
                name: info.name.into(),
                sign_in: info.sign_in,
                signed_in: via.is_some(),
                via,
                key_page: info.key_page.map(str::to_string),
                key_env: info.key_env.map(str::to_string),
            });
        }
        Ok(statuses)
    }
    pub async fn local(&self) -> Vec<LocalStatus> {
        self.inner.state.borrow_mut().named = None;
        futures::future::join_all(self.servers().into_iter().map(|server| async move {
            let running = local::probe_local(&server, Transport { fetch: self.inner.options.fetch.clone(), signal: None }).await;
            let theirs =
                server.kind == LocalKind::OpenaiCompatible
                    || server.kind == LocalKind::Ollama && self.inner.options.env.get("OLLAMA_HOST").is_some_and(|s| !s.is_empty())
                    || self.inner.options.installed.as_ref().map_or_else(
                        || local::local_installed(server.kind, Some(&self.inner.options.env)),
                        |installed| installed(server.kind),
                    );
            if !running && !theirs {
                return None;
            }
            let start = (!running).then(|| local::start_hint(&server));
            Some(LocalStatus { id: server.id, name: server.name, r#where: server.r#where, running, start })
        }))
        .await
        .into_iter()
        .flatten()
        .collect()
    }
    pub fn provider_name(&self, id: &str) -> String {
        if let Some(provider) = ProviderId::parse(id) {
            provider_info(provider).name.into()
        } else {
            self.servers().into_iter().find(|s| s.id == id).map(|s| s.name).unwrap_or_else(|| id.into())
        }
    }
    pub fn choose_default(&self) -> LocalBoxFuture<'static, Result<Option<DefaultModel>, RuntimeError>> {
        let control = self.clone();
        async move {
            if control.inner.state.borrow().model.as_ref().is_some_and(|s| !s.is_empty()) {
                return Ok(None);
            }
            let existing = control.inner.state.borrow().defaulting.clone();
            let defaulting = if let Some(defaulting) = existing {
                defaulting
            } else {
                let choosing = control.clone();
                let future = async move {
                    let result = choosing.choose_first().await;
                    choosing.inner.state.borrow_mut().defaulting = None;
                    result
                }
                .boxed_local()
                .shared();
                control.inner.state.borrow_mut().defaulting = Some(future.clone());
                future
            };
            defaulting.await
        }
        .boxed_local()
    }
    async fn choose_first(&self) -> Result<Option<DefaultModel>, RuntimeError> {
        for provider in self.providers().await?.into_iter().filter(|p| p.signed_in) {
            if let Some(first) = self.models(provider.id.as_str(), false).await.unwrap_or_default().into_iter().next() {
                self.choose(&first.id).await?;
                return Ok(Some(DefaultModel { model: first, note: None }));
            }
        }
        for server in self.local().await.into_iter().filter(|s| s.running) {
            let listed = self.models(&server.id, false).await.unwrap_or_default();
            let pick = listed
                .iter()
                .find(|i| i.tools != Some(false) && i.loaded == Some(true))
                .or_else(|| listed.iter().find(|i| i.tools != Some(false)))
                .or_else(|| listed.first());
            if let Some(pick) = pick {
                let note = self.choose(&pick.id).await?;
                return Ok(Some(DefaultModel { model: pick.clone(), note }));
            }
        }
        Ok(None)
    }
    pub async fn models(&self, provider: &str, refresh: bool) -> Result<Vec<ModelInfo>, RuntimeError> {
        let server = self.servers().into_iter().find(|s| s.id == provider);
        if server.is_none() && !refresh {
            if let Some(known) = self.inner.state.borrow().catalog.get(provider) {
                return Ok(known.clone());
            }
        }
        let signal = Some(abort::timeout(15000));
        let listed = if let Some(server) = server {
            local::list_local_models(&server, Transport { fetch: self.inner.options.fetch.clone(), signal }).await?
        } else if let Some(provider) = ProviderId::parse(provider) {
            list_models(
                provider,
                ListOptions {
                    store: self.inner.options.store.clone(),
                    env: Some(self.inner.options.env.clone()),
                    fetch: self.inner.options.fetch.clone(),
                    signal,
                },
            )
            .await?
        } else {
            Vec::new()
        };
        self.inner.state.borrow_mut().catalog.insert(provider.into(), listed.clone());
        Ok(listed)
    }
    pub async fn choose(&self, next: &str) -> Result<Option<String>, RuntimeError> {
        if self.parse(Some(next)).is_none() {
            return Err(KumiError::new(FailureKind::Config, format!("{next} isn't a model Kumi knows how to reach.")).into());
        }
        let info = self.info_for(Some(next));
        let keep = self
            .inner
            .state
            .borrow()
            .effort
            .filter(|effort| info.as_ref().is_none_or(|info| info.efforts.iter().any(|level| level.effort == *effort)));
        let tier = self.tier_for(next).await;
        let binding = self.bind(next, keep, tier.clone()).await?;
        {
            let mut state = self.inner.state.borrow_mut();
            state.model = Some(next.into());
            state.effort = keep;
            state.bound = Some(CachedBinding { model: next.into(), effort: keep, tier, binding: binding.clone() });
        }
        self.save()?;
        (self.inner.options.changed)().await?;
        if let Some(asked) = binding.asked() {
            asked.await;
        }
        Ok(self.once(binding.note()))
    }
    pub async fn set_effort(&self, next: Option<Effort>) -> Result<(), RuntimeError> {
        {
            let mut state = self.inner.state.borrow_mut();
            state.effort = next;
            state.bound = None;
        }
        self.save()?;
        (self.inner.options.changed)().await
    }
    /// Whether `/fast` is on, whether or not the current model has a faster tier.
    pub fn fast_enabled(&self) -> bool {
        self.inner.state.borrow().fast
    }
    /// Turn the model's faster tier on or off. On: the tier the model's provider lists, or None (and
    /// nothing changes) when it lists none.
    pub async fn set_fast(&self, on: bool) -> Result<Option<ServiceTier>, RuntimeError> {
        let model = self.inner.state.borrow().model.clone();
        let tier = match model {
            Some(model) if on => self.tier_of(&model).await,
            _ => None,
        };
        if on && tier.is_none() {
            return Ok(None);
        }
        {
            let mut state = self.inner.state.borrow_mut();
            state.fast = on;
            state.bound = None;
        }
        self.save()?;
        (self.inner.options.changed)().await?;
        Ok(tier)
    }
    pub async fn set_effort_name(&self, next: Option<&str>) -> Result<(), RuntimeError> {
        let effort = next
            .map(|name| Effort::parse(name).ok_or_else(|| KumiError::new(FailureKind::Config, format!("{name} isn't an effort level."))))
            .transpose()?;
        self.set_effort(effort).await
    }
    pub async fn save_key(&self, provider: ProviderId, key: &str, signal: Option<Signal>) -> Result<ApiKeyCheck, RuntimeError> {
        let info = provider_info(provider);
        if info.sign_in != SignIn::ApiKey {
            return Err(KumiError::new(FailureKind::Config, format!("{} signs in with ChatGPT, not a key.", info.name)).into());
        }
        let key = trim(key).to_string();
        let verdict = check_api_key(provider, &key, Transport { fetch: self.inner.options.fetch.clone(), signal }).await;
        if verdict == ApiKeyCheck::Refused {
            return Ok(verdict);
        }
        self.inner
            .options
            .store
            .update(info.credential, Box::new(move |_| async move { Ok(Some(Credential::ApiKey { key })) }.boxed_local()))
            .await?;
        let current = {
            let mut state = self.inner.state.borrow_mut();
            state.catalog.remove(provider.as_str());
            if provider == ProviderId::Opencode {
                state.catalog.remove("opencode-go");
            }
            if provider == ProviderId::OpencodeGo {
                state.catalog.remove("opencode");
            }
            state.model.clone().and_then(|m| parse_model_id(&m)).map(|m| m.provider)
        };
        if current.is_some_and(|p| provider_info(p).credential == info.credential) {
            self.inner.state.borrow_mut().bound = None;
            (self.inner.options.changed)().await?;
        }
        Ok(verdict)
    }
    pub async fn sign_in_chatgpt(&self, io: ChatGptSignIn) -> Result<(), RuntimeError> {
        let credential = match io {
            ChatGptSignIn::Browser { signal, on_url } => {
                login_codex_browser(LoginOptions { signal, ..Default::default() }, on_url, None).await?
            }
            ChatGptSignIn::Device { signal, on_code } => login_codex_device(LoginOptions { signal, ..Default::default() }, on_code).await?,
        };
        self.inner.options.store.update(OPENAI_CODEX, Box::new(move |_| async move { Ok(Some(credential.into())) }.boxed_local())).await?;
        let model = {
            let mut state = self.inner.state.borrow_mut();
            state.catalog.remove("openai-codex");
            state.model.clone()
        };
        if model.as_ref().is_none_or(|s| s.is_empty()) {
            self.choose_default().await?;
            return Ok(());
        }
        if model.as_deref().and_then(parse_model_id).is_some_and(|m| m.provider == ProviderId::OpenaiCodex) {
            self.inner.state.borrow_mut().bound = None;
            (self.inner.options.changed)().await?;
        }
        Ok(())
    }
    pub async fn sign_out(&self, provider: ProviderId) -> Result<bool, RuntimeError> {
        let info = provider_info(provider);
        if self.inner.options.store.get(info.credential).await?.is_none() {
            return Ok(false);
        }
        self.inner.options.store.update(info.credential, Box::new(|_| async { Ok(None) }.boxed_local())).await?;
        let current = {
            let mut state = self.inner.state.borrow_mut();
            state.catalog.remove(provider.as_str());
            state.model.clone().and_then(|m| parse_model_id(&m)).map(|m| m.provider)
        };
        if current.is_some_and(|p| provider_info(p).credential == info.credential) {
            self.inner.state.borrow_mut().bound = None;
            (self.inner.options.changed)().await?;
        }
        Ok(true)
    }
    pub async fn binding_with_info(&self) -> Result<BindingInfo, RuntimeError> {
        if self.inner.state.borrow().model.as_ref().is_none_or(|s| s.is_empty()) {
            let _ = self.choose_default().await;
        }
        let (model, effort, cached) = {
            let state = self.inner.state.borrow();
            (state.model.clone(), state.effort, state.bound.clone())
        };
        let model = model
            .filter(|s| !s.is_empty())
            .ok_or_else(|| KumiError::new(FailureKind::Config, "Choose a model to talk to: type /model."))?;
        let tier = self.tier_for(&model).await;
        let binding = if let Some(cached) = cached.filter(|b| b.model == model && b.effort == effort && b.tier == tier) {
            cached.binding
        } else {
            let binding = self.bind(&model, effort, tier.clone()).await?;
            self.inner.state.borrow_mut().bound = Some(CachedBinding { model, effort, tier, binding: binding.clone() });
            if let Some(asked) = binding.asked() {
                let control = self.clone();
                let saying = binding.clone();
                tokio::task::spawn_local(async move {
                    asked.await;
                    if let Some(note) = saying.note() {
                        control.say(note);
                    }
                });
            }
            binding
        };
        Ok(BindingInfo { binding: binding.binding(), asked: binding.asked(), backing: binding })
    }
    pub async fn binding(&self) -> Result<ModelBinding, RuntimeError> {
        Ok(self.binding_with_info().await?.binding)
    }
}

/// The app-facing model controls, including the injectable source test contract.
#[async_trait::async_trait(?Send)]
pub trait ModelController {
    fn current(&self) -> CurrentModel;
    async fn providers(&self) -> Result<Vec<ProviderStatus>, RuntimeError>;
    async fn local(&self) -> Vec<LocalStatus>;
    fn provider_name(&self, id: &str) -> String;
    async fn choose_default(&self) -> Result<Option<DefaultModel>, RuntimeError>;
    async fn models(&self, provider: &str, refresh: bool) -> Result<Vec<ModelInfo>, RuntimeError>;
    async fn choose(&self, next: &str) -> Result<Option<String>, RuntimeError>;
    async fn set_effort(&self, next: Option<Effort>) -> Result<(), RuntimeError>;
    /// Whether `/fast` is on (the setting, whether or not this model has a faster tier).
    fn fast_enabled(&self) -> bool {
        false
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
#[async_trait::async_trait(?Send)]
impl ModelController for ModelControl {
    fn current(&self) -> CurrentModel {
        ModelControl::current(self)
    }
    async fn providers(&self) -> Result<Vec<ProviderStatus>, RuntimeError> {
        ModelControl::providers(self).await
    }
    async fn local(&self) -> Vec<LocalStatus> {
        ModelControl::local(self).await
    }
    fn provider_name(&self, id: &str) -> String {
        ModelControl::provider_name(self, id)
    }
    async fn choose_default(&self) -> Result<Option<DefaultModel>, RuntimeError> {
        ModelControl::choose_default(self).await
    }
    async fn models(&self, provider: &str, refresh: bool) -> Result<Vec<ModelInfo>, RuntimeError> {
        ModelControl::models(self, provider, refresh).await
    }
    async fn choose(&self, next: &str) -> Result<Option<String>, RuntimeError> {
        ModelControl::choose(self, next).await
    }
    async fn set_effort(&self, next: Option<Effort>) -> Result<(), RuntimeError> {
        ModelControl::set_effort(self, next).await
    }
    fn fast_enabled(&self) -> bool {
        ModelControl::fast_enabled(self)
    }
    async fn set_fast(&self, on: bool) -> Result<Option<ServiceTier>, RuntimeError> {
        ModelControl::set_fast(self, on).await
    }
    async fn save_key(&self, provider: ProviderId, key: &str, signal: Option<Signal>) -> Result<ApiKeyCheck, RuntimeError> {
        ModelControl::save_key(self, provider, key, signal).await
    }
    async fn sign_in_chatgpt(&self, io: ChatGptSignIn) -> Result<(), RuntimeError> {
        ModelControl::sign_in_chatgpt(self, io).await
    }
    async fn sign_out(&self, provider: ProviderId) -> Result<bool, RuntimeError> {
        ModelControl::sign_out(self, provider).await
    }
}
