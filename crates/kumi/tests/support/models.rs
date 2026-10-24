#![allow(dead_code)]
//! Fake models shared by the plain terminal and full-screen app tests.
use async_trait::async_trait;
use kumi::models::{ChatGptSignIn, CurrentModel, DefaultModel, LocalStatus, ModelController, ProviderStatus};
use kumi_ai::{
    auth::openai_codex::DevicePrompt,
    providers::{
        models::{ApiKeyCheck, FactsFrom, ModelInfo},
        table, Effort, ProviderId, SignIn,
    },
};
use kumi_common::abort::Signal;
use kumi_types::errors::{FailureKind, KumiError, RuntimeError};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    rc::Rc,
};
/// The browser sign-in's link: longer than a narrow window, as a real one is.
pub const CHATGPT_LINK: &str =
    "https://auth.example.test/oauth/authorize?response_type=code&client=kumi&redirect_uri=http%3A%2F%2Flocalhost%3A1455%2Fauth%2Fcallback&state=fixture";
pub struct FakeModels {
    pub calls: RefCell<Vec<String>>,
    pub model: RefCell<Option<String>>,
    pub effort: RefCell<Option<Effort>>,
    pub fast: RefCell<bool>,
    pub fallback: RefCell<Option<String>>,
    pub signed_in: RefCell<HashSet<ProviderId>>,
    pub lists: RefCell<HashMap<String, Vec<ModelInfo>>>,
    pub local: RefCell<Vec<LocalStatus>>,
    pub notes: RefCell<HashMap<String, String>>,
    pub finish_chatgpt: RefCell<Option<tokio::sync::oneshot::Sender<()>>>,
    /// How many times a default model was looked for.
    pub looked: std::cell::Cell<usize>,
}
impl Default for FakeModels {
    fn default() -> Self {
        Self {
            calls: RefCell::new(vec![]),
            model: RefCell::new(None),
            effort: RefCell::new(None),
            fast: RefCell::new(false),
            fallback: RefCell::new(None),
            signed_in: RefCell::new(HashSet::new()),
            lists: RefCell::new(HashMap::new()),
            local: RefCell::new(vec![]),
            notes: RefCell::new(HashMap::new()),
            finish_chatgpt: RefCell::new(None),
            looked: std::cell::Cell::new(0),
        }
    }
}
impl FakeModels {
    pub fn chosen() -> Rc<Self> {
        let models = Rc::new(Self::default());
        *models.model.borrow_mut() = Some("openai-codex/fixture".into());
        models.signed_in.borrow_mut().insert(ProviderId::parse("openai-codex").unwrap());
        models
    }
    pub fn catalog() -> Rc<Self> {
        let models = Self::chosen();
        *models.model.borrow_mut() = Some("openai-codex/gpt-6-astra".into());
        let values = serde_json::json!({"openai-codex":[{"id":"openai-codex/gpt-6-astra","provider":"openai-codex","model":"gpt-6-astra","name":"GPT-6 Astra","description":"Frontier model for complex work","efforts":[{"effort":"low"},{"effort":"medium"},{"effort":"high"},{"effort":"xhigh"}],"defaultEffort":"medium","serviceTiers":[{"id":"priority","name":"Fast","description":"2x speed, increased usage"}]},{"id":"openai-codex/gpt-6-luna","provider":"openai-codex","model":"gpt-6-luna","name":"GPT-6 Luna","description":"Fast and light","efforts":[{"effort":"low"},{"effort":"medium"}],"defaultEffort":"low"}],"anthropic":[{"id":"anthropic/claude-sonnet-5-5","provider":"anthropic","model":"claude-sonnet-5-5","name":"Claude Sonnet 5.5","efforts":[{"effort":"low"},{"effort":"medium"},{"effort":"high"},{"effort":"xhigh"},{"effort":"max"}]},{"id":"anthropic/claude-haiku-4-5","provider":"anthropic","model":"claude-haiku-4-5","name":"Claude Haiku 4.5","efforts":[]}]});
        *models.lists.borrow_mut() = serde_json::from_value(values).unwrap();
        models
    }
    /// A list as long as a gateway's: OpenCode Zen's forty models, some that can't use tools, unverified, private or
    /// deprecated.
    pub fn long_catalog() -> Rc<Self> {
        let models = Self::chosen();
        models.signed_in.borrow_mut().insert(ProviderId::parse("opencode").unwrap());
        let list: Vec<ModelInfo> = (1..=40)
            .map(|n| {
                let mut info = ModelInfo {
                    id: format!("opencode/model-{n:02}"),
                    provider: "opencode".into(),
                    model: format!("model-{n:02}"),
                    name: format!("Model {n:02}"),
                    ..Default::default()
                };
                match n % 5 {
                    1 => info.tools = Some(false),
                    2 => info.facts_from = Some(FactsFrom::Guess),
                    3 => info.labels = vec!["private".into()],
                    4 => info.status = Some("deprecated".into()),
                    _ => info.description = Some("A model the list describes".into()),
                }
                info
            })
            .collect();
        models.lists.borrow_mut().insert("opencode".into(), list);
        models
    }
    fn provider(&self, id: &str) -> Option<String> {
        id.split_once('/').map(|(p, _)| p.to_string())
    }
    fn info(&self, id: &str) -> Option<ModelInfo> {
        self.lists.borrow().get(&self.provider(id)?).and_then(|list| list.iter().find(|m| m.id == id)).cloned()
    }
    fn require(&self, provider: &str) -> Result<(), RuntimeError> {
        if self.local.borrow().iter().any(|s| s.id == provider) {
            return Ok(());
        }
        let provider = ProviderId::parse(provider).ok_or_else(|| RuntimeError::plain("Unknown provider"))?;
        if !self.signed_in.borrow().contains(&provider) {
            return Err(KumiError::with_provider(
                FailureKind::Auth,
                format!("Not signed in to {}.", provider.row().name),
                provider.as_str(),
            )
            .into());
        }
        Ok(())
    }
    fn shared(&self, provider: ProviderId) -> Vec<ProviderId> {
        table::rows().filter(|row| row.credential == provider.row().credential).map(|row| ProviderId::parse(row.id).unwrap()).collect()
    }
}
#[async_trait(?Send)]
impl ModelController for FakeModels {
    fn current(&self) -> CurrentModel {
        let model = self.model.borrow().clone();
        let provider = model.as_ref().and_then(|m| self.provider(m));
        let info = model.as_ref().and_then(|m| self.info(m));
        let local = self.local.borrow().iter().find(|s| Some(&s.id) == provider.as_ref()).cloned();
        CurrentModel {
            model,
            provider,
            name: info.as_ref().map(|m| m.name.clone()),
            effort: *self.effort.borrow(),
            default_effort: info.as_ref().and_then(|m| m.default_effort),
            efforts: info.as_ref().map(|m| m.efforts.clone()).unwrap_or_default(),
            pinned: false,
            r#where: local.map(|s| s.r#where),
            fast: info.as_ref().filter(|_| *self.fast.borrow()).and_then(|m| m.service_tiers.first()).cloned(),
        }
    }
    async fn providers(&self) -> Result<Vec<ProviderStatus>, RuntimeError> {
        Ok(table::offered()
            .into_iter()
            .map(|id| {
                let info = id.row();
                let signed_in = self.signed_in.borrow().contains(&id);
                ProviderStatus {
                    id,
                    name: info.name.into(),
                    sign_in: info.sign_in,
                    signed_in,
                    via: signed_in.then(|| if info.sign_in == SignIn::Chatgpt { "chatgpt" } else { "saved key" }.into()),
                    key_page: info.key_page.map(str::to_string),
                    key_env: info.first_key_env().map(str::to_string),
                }
            })
            .collect())
    }
    async fn local(&self) -> Vec<LocalStatus> {
        self.local.borrow().clone()
    }
    fn provider_name(&self, id: &str) -> String {
        ProviderId::parse(id)
            .map(|p| p.row().name.into())
            .or_else(|| self.local.borrow().iter().find(|s| s.id == id).map(|s| s.name.clone()))
            .unwrap_or(id.into())
    }
    async fn choose_default(&self) -> Result<Option<DefaultModel>, RuntimeError> {
        self.looked.set(self.looked.get() + 1);
        if self.model.borrow().is_some() {
            return Ok(None);
        }
        let provider = [
            ProviderId::parse("openai-codex").unwrap(),
            ProviderId::parse("anthropic").unwrap(),
            ProviderId::parse("openai").unwrap(),
            ProviderId::parse("opencode").unwrap(),
            ProviderId::parse("opencode-go").unwrap(),
        ]
        .into_iter()
        .find(|p| self.signed_in.borrow().contains(p) && self.lists.borrow().get(p.as_str()).is_some_and(|s| !s.is_empty()))
        .map(|p| p.as_str().to_string())
        .or_else(|| {
            self.local
                .borrow()
                .iter()
                .find(|s| s.running && self.lists.borrow().get(&s.id).is_some_and(|list| !list.is_empty()))
                .map(|s| s.id.clone())
        });
        let first = provider.and_then(|p| self.lists.borrow().get(&p).and_then(|s| s.first()).cloned());
        if let Some(model) = first {
            *self.model.borrow_mut() = Some(model.id.clone());
            self.calls.borrow_mut().push(format!("default:{}", model.id));
            let note = self.notes.borrow().get(&model.id).cloned();
            Ok(Some(DefaultModel { model, note }))
        } else {
            Ok(None)
        }
    }
    async fn models(&self, provider: &str, _: bool) -> Result<Vec<ModelInfo>, RuntimeError> {
        self.calls.borrow_mut().push(format!("list:{provider}"));
        self.require(provider)?;
        Ok(self.lists.borrow().get(provider).cloned().unwrap_or_default())
    }
    async fn choose(&self, next: &str) -> Result<Option<String>, RuntimeError> {
        self.require(&self.provider(next).unwrap_or_default())?;
        if let Some(info) = self.info(next) {
            let effort = *self.effort.borrow();
            if effort.is_some_and(|e| !info.efforts.iter().any(|i| i.effort == e)) {
                *self.effort.borrow_mut() = None;
            }
        }
        *self.model.borrow_mut() = Some(next.into());
        self.calls.borrow_mut().push(format!("choose:{next}"));
        Ok(self.notes.borrow().get(next).cloned())
    }
    async fn set_effort(&self, next: Option<Effort>) -> Result<(), RuntimeError> {
        *self.effort.borrow_mut() = next;
        self.calls.borrow_mut().push(format!("effort:{}", next.map(|e| e.as_str()).unwrap_or("default")));
        Ok(())
    }
    fn fallback(&self) -> Option<String> {
        self.fallback.borrow().clone()
    }
    async fn set_fallback(&self, model: Option<&str>) -> Result<(), RuntimeError> {
        self.calls.borrow_mut().push(format!("fallback:{}", model.unwrap_or("")));
        *self.fallback.borrow_mut() = model.map(str::to_string);
        Ok(())
    }
    fn fast_enabled(&self) -> bool {
        *self.fast.borrow()
    }
    async fn set_fast(&self, on: bool) -> Result<Option<kumi_ai::providers::models::ServiceTier>, RuntimeError> {
        let tier = self.model.borrow().as_ref().and_then(|m| self.info(m)).and_then(|m| m.service_tiers.first().cloned());
        if on && tier.is_none() {
            return Ok(None);
        }
        *self.fast.borrow_mut() = on;
        self.calls.borrow_mut().push(format!("fast:{on}"));
        Ok(tier.filter(|_| on))
    }
    async fn save_key(&self, provider: ProviderId, key: &str, _: Option<Signal>) -> Result<ApiKeyCheck, RuntimeError> {
        self.calls.borrow_mut().push(format!("key:{}:{}", provider.as_str(), key.encode_utf16().count()));
        if key.starts_with("refused") {
            return Ok(ApiKeyCheck::Refused);
        }
        self.signed_in.borrow_mut().extend(self.shared(provider));
        Ok(ApiKeyCheck::Ok)
    }
    async fn sign_in_chatgpt(&self, io: ChatGptSignIn) -> Result<(), RuntimeError> {
        let signal = match io {
            ChatGptSignIn::Browser { signal, on_url } => {
                self.calls.borrow_mut().push("chatgpt".into());
                on_url(CHATGPT_LINK.into());
                signal
            }
            ChatGptSignIn::Device { signal, on_code } => {
                self.calls.borrow_mut().push("chatgpt code".into());
                on_code(DevicePrompt { url: "https://auth.example.test/codex/device".into(), code: "KUMI-2026".into() });
                signal
            }
        };
        let (tx, rx) = tokio::sync::oneshot::channel();
        *self.finish_chatgpt.borrow_mut() = Some(tx);
        tokio::select! {_=rx=>{},_=signal.cancelled()=>return Err(RuntimeError::plain("cancelled"))};
        self.signed_in.borrow_mut().insert(ProviderId::parse("openai-codex").unwrap());
        if self.model.borrow().is_none() {
            *self.model.borrow_mut() = self.lists.borrow().get("openai-codex").and_then(|s| s.first()).map(|m| m.id.clone());
        }
        Ok(())
    }
    async fn sign_out(&self, provider: ProviderId) -> Result<bool, RuntimeError> {
        self.calls.borrow_mut().push(format!("signout:{}", provider.as_str()));
        if !self.signed_in.borrow().contains(&provider) {
            return Ok(false);
        }
        for p in self.shared(provider) {
            self.signed_in.borrow_mut().remove(&p);
        }
        Ok(true)
    }
}
