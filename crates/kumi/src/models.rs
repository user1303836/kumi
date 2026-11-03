use crate::{
    config::{read_settings, write_settings},
    model_lists::{self, KeptLists},
};
use futures::{
    future::{LocalBoxFuture, Shared},
    FutureExt,
};
use kumi_ai::{
    auth::{
        openai_codex::{login_codex_browser, login_codex_device, LoginOptions, OPENAI_CODEX},
        store::{Credential, CredentialStore},
    },
    binding::ModelBinding,
    http::{Fetch, FetchInit},
    providers::{
        api_key_from, chat_only,
        local::{self, LocalBinding, LocalKind, LocalModelOptions, LocalServer},
        models::{check_api_key, list_models, ApiKeyCheck, ListOptions, ModelInfo, ServiceTier, Transport},
        parse_model_id, resolve_model, table, Effort, KeySource, ProviderId, ResolveModelOptions, SignIn,
    },
};
use kumi_common::{
    abort::{self, Signal},
    js::string::trim,
};
use kumi_types::{
    errors::{FailureKind, KumiError, RuntimeError},
    system::Env,
};
use serde_json::json;
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    rc::Rc,
};
// What the app's model controls are made of is kumi-ai's, so the session's protocol can name it.
pub use kumi_ai::control::{missing_model, ChatGptSignIn, CurrentModel, DefaultModel, LocalStatus, ModelController, ProviderStatus};
pub type Changed = Rc<dyn Fn() -> LocalBoxFuture<'static, Result<(), RuntimeError>>>;
pub struct ModelControlOptions {
    pub store: Rc<dyn CredentialStore>,
    pub settings_file: String,
    pub env: Env,
    pub changed: Changed,
    pub fetch: Option<Rc<dyn Fetch>>,
    pub say: Option<Rc<dyn Fn(String)>>,
    pub installed: Option<Rc<dyn Fn(LocalKind) -> bool>>,
    /// Where the cloud providers' model lists are kept between runs (None: not kept, each run lists them). Kept lists
    /// are listed again in the background, which needs a `LocalSet`.
    pub lists: Option<KeptLists>,
}
#[derive(Clone)]
enum Bound {
    /// A cloud model, and what the producer hears of it once (a model that can't use tools).
    Cloud(Rc<ModelBinding>, Option<String>),
    Local(Rc<LocalBinding>),
}
impl Bound {
    fn original(&self) -> &ModelBinding {
        match self {
            Self::Cloud(b, _) => b,
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
        match self {
            Self::Local(b) => b.note(),
            Self::Cloud(_, note) => note.clone(),
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
/// How soon after one warm-up another may start: typing more of the same message needs none.
const WARM_AGAIN: std::time::Duration = std::time::Duration::from_secs(20);
type Defaulting = Shared<LocalBoxFuture<'static, Result<Option<DefaultModel>, RuntimeError>>>;
/// A cloud provider's list, asked for: under way, or answered.
type Listing = Shared<LocalBoxFuture<'static, Result<Vec<ModelInfo>, RuntimeError>>>;
/// What a model's binding reads of its list's entry: the faster tier it asks for, whether it's sent tools, its name.
fn bound_by(info: Option<ModelInfo>) -> Option<(Option<String>, Option<bool>, String)> {
    info.map(|info| (info.service_tiers.into_iter().next().map(|tier| tier.id), info.tools, info.name))
}
struct State {
    model: Option<String>,
    /// The model that carries on while this one can't (`fallbackModel`).
    fallback: Option<String>,
    effort: Option<Effort>,
    fast: bool,
    catalog: HashMap<String, Vec<ModelInfo>>,
    /// The cloud providers whose list in `catalog` came from the provider this run (not from what was kept).
    listed: HashSet<String>,
    /// Each cloud provider's latest ask for its list this run.
    listing: HashMap<String, Listing>,
    /// Counts sign-in changes: a list asked for before one isn't kept after it.
    signed: u64,
    bound: Option<CachedBinding>,
    named: Option<Vec<LocalServer>>,
    heard: HashSet<String>,
    defaulting: Option<Defaulting>,
    /// When the producer last began a message (`warm`).
    warmed: Option<std::time::Instant>,
    /// A binding being made, for the model, effort and tier it's for: another ask for the same joins it (a warm-up and the
    /// message after it), so a sign-in being refreshed is waited on once, not raced for its lock.
    binding: Option<((String, Option<Effort>, Option<String>), Binding)>,
}
type Binding = Shared<LocalBoxFuture<'static, Result<Bound, RuntimeError>>>;
struct Inner {
    options: ModelControlOptions,
    /// The cloud providers' HTTP: their lists, sign-ins, key checks and model calls go through one client. For an API-key
    /// provider, the connection the start's check of a saved model opened (its list) is the one the first message goes on.
    /// ChatGPT's answers go over a WebSocket of their own beside it (`SocketFetch`), chosen because this is Kumi's own
    /// HTTP (`Fetch::is_kumis_own`): it must stay a bare `HttpFetch` in the app. A model server keeps its own client
    /// (`options.fetch`, its proxies the variables' alone).
    cloud: Rc<dyn Fetch>,
    pinned: bool,
    state: RefCell<State>,
    /// Cloud models found this session to turn tools down: they're sent none from then on.
    learned: chat_only::Learned,
    /// Fired as Kumi closes: a provider's list or a model server's probe still under way ends with it, so Kumi's exit
    /// doesn't wait on an answer nobody will read.
    closed: Signal,
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
            cloud: options.fetch.clone().unwrap_or_else(kumi_ai::http::default_fetch),
            options,
            pinned,
            state: RefCell::new(State {
                model,
                fallback: settings.fallback_model,
                effort: settings.effort,
                fast: settings.fast == Some(true),
                catalog: HashMap::new(),
                listed: HashSet::new(),
                listing: HashMap::new(),
                signed: 0,
                bound: None,
                named: None,
                heard: HashSet::new(),
                defaulting: None,
                warmed: None,
                binding: None,
            }),
            learned: Default::default(),
            closed: Signal::new(),
        }),
    }
}
impl ModelControl {
    /// Ends the lookups under way, and any asked for later, as Kumi closes.
    pub fn close(&self) {
        self.inner.closed.cancel();
    }
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
        // A server first: a custom server keeps an ID that a provider added later has too (`hidden_providers`).
        if let Some(local) = local::parse_local_model_id(id, &self.servers()) {
            return Some(Parsed { provider: local.server.id.clone(), model: local.model, server: Some(local.server) });
        }
        parse_model_id(id).map(|cloud| Parsed { provider: cloud.provider.to_string(), model: cloud.model, server: None })
    }
    /// The cloud provider of the model chosen now, if it's one.
    fn current_provider(&self) -> Option<ProviderId> {
        let model = self.inner.state.borrow().model.clone();
        self.parse(model.as_deref()).filter(|parsed| parsed.server.is_none()).and_then(|parsed| ProviderId::parse(&parsed.provider))
    }
    /// The providers a custom server hides, keeping their IDs (`hidden_providers`).
    fn hidden(&self) -> Vec<ProviderId> {
        local::hidden_providers(&self.servers()).into_iter().map(|(provider, _)| provider).collect()
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
        let profile = *crate::daw::profile();
        // A custom server that takes Responses or Anthropic's format: bound as a cloud model is, under Kumi's one policy.
        if let Some(Parsed { server: Some(server), model, .. }) =
            self.parse(Some(id)).filter(|parsed| parsed.server.as_ref().is_some_and(|s| s.format.is_some()))
        {
            let binding = local::resolve_server_model(
                &server,
                model.clone(),
                LocalModelOptions { fetch: self.inner.options.fetch.clone(), effort, on_note: None },
            );
            return Ok(Bound::Cloud(Rc::new(chat_only::chat_only(binding, false, self.inner.learned.clone(), profile, model)), None));
        }
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
        let binding = resolve_model(ResolveModelOptions {
            model: id.into(),
            store: self.inner.options.store.clone(),
            env: Some(self.inner.options.env.clone()),
            fetch: Some(self.inner.cloud.clone()),
            effort,
            service_tier: tier,
        })
        .await?;
        // A model its list says can't use tools is still there to talk with, as one on this computer is.
        let info = self.info_for(Some(id));
        let toolless = info.as_ref().is_some_and(|info| info.tools == Some(false));
        let name = info.map(|info| info.name).unwrap_or_else(|| id.split_once('/').map_or(id, |(_, model)| model).to_string());
        let note = toolless.then(|| chat_only::talk_only(&name, &profile, "/model chooses one that can."));
        Ok(Bound::Cloud(Rc::new(chat_only::chat_only(binding, toolless, self.inner.learned.clone(), profile, name)), note))
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
        let hidden = self.hidden();
        // Every provider's sign-in, from one read of the credential file.
        let saved = self.inner.options.store.list().await?;
        for id in table::offered().into_iter().filter(|id| !hidden.contains(id)) {
            let info = id.row();
            // The variable a key from the environment came from, else the first it may be in.
            let mut key_env = info.first_key_env().map(str::to_string);
            let via = if info.sign_in == SignIn::Chatgpt {
                matches!(saved.get(OPENAI_CODEX), Some(Credential::Oauth(_))).then(|| "chatgpt".into())
            } else {
                api_key_from(id, saved.get(info.credential), Some(&self.inner.options.env)).map(|k| {
                    if k.source == KeySource::Env {
                        key_env = k.variable.or(key_env.take());
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
                key_env,
            });
        }
        Ok(statuses)
    }
    pub async fn local(&self) -> Vec<LocalStatus> {
        self.inner.state.borrow_mut().named = None;
        futures::future::join_all(self.servers().into_iter().map(|server| async move {
            let running =
                local::probe_local(&server, Transport { fetch: self.inner.options.fetch.clone(), signal: Some(self.inner.closed.clone()) })
                    .await;
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
        if let Some(server) = self.servers().into_iter().find(|s| s.id == id) {
            server.name
        } else {
            ProviderId::parse(id).map_or_else(|| id.into(), |provider| provider.row().name.into())
        }
    }
    /// The cloud provider `id` names, unless it's a custom server's (`hidden_providers`).
    pub fn cloud_provider(&self, id: &str) -> Option<ProviderId> {
        ProviderId::parse(id).filter(|_| !self.servers().iter().any(|server| server.id == id))
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
            // The provider's list as it gives it this run: a default is never chosen from a list kept from an earlier one.
            if let Some(first) = self.listed_now(provider.id.as_str()).await.unwrap_or_default().into_iter().next() {
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
            // Kept from an earlier run for this sign-in: used now, while the provider lists it again behind it.
            if let Some(kept) = self.kept(provider).await {
                self.inner.state.borrow_mut().catalog.insert(provider.into(), kept.clone());
                self.list_behind(provider);
                return Ok(kept);
            }
        }
        if server.is_none() {
            if let Some(cloud) = ProviderId::parse(provider) {
                return self.list(cloud, refresh).await;
            }
        }
        let signal = Some(abort::any([abort::timeout(15000), self.inner.closed.clone()]));
        let listed = if let Some(server) = server {
            local::list_local_models_in(&server, Transport { fetch: self.inner.options.fetch.clone(), signal }, crate::daw::profile())
                .await?
        } else {
            Vec::new()
        };
        self.inner.state.borrow_mut().catalog.insert(provider.into(), listed.clone());
        Ok(listed)
    }
    /// `provider`'s list as the provider gives it this run: the list it gave already, the ask under way, or a new ask.
    /// What the saved model is checked against (#328): never a list kept from an earlier run.
    pub async fn listed_now(&self, provider: &str) -> Result<Vec<ModelInfo>, RuntimeError> {
        let Some(cloud) = self.cloud_provider(provider) else { return self.models(provider, false).await };
        let listed = {
            let state = self.inner.state.borrow();
            state.listed.contains(provider).then(|| state.catalog.get(provider).cloned()).flatten()
        };
        match listed {
            Some(listed) => Ok(listed),
            None => self.list(cloud, false).await,
        }
    }
    /// The list kept for `provider` and the sign-in it would be listed with now.
    async fn kept(&self, provider: &str) -> Option<Vec<ModelInfo>> {
        let lists = self.inner.options.lists.as_ref()?;
        let cloud = self.cloud_provider(provider)?;
        let who = self.identity(cloud).await?;
        lists.read(provider, &who, kumi_common::time::now_ms())
    }
    async fn identity(&self, provider: ProviderId) -> Option<String> {
        let saved = self.inner.options.store.get(provider.row().credential).await.ok().flatten();
        model_lists::identity(provider, saved.as_ref(), &self.inner.options.env)
    }
    /// The provider lists `provider` again, in the background, unless it has this run: asked now, so whatever wants its
    /// list next waits for this ask rather than making another.
    fn list_behind(&self, provider: &str) {
        let Some(cloud) = ProviderId::parse(provider) else { return };
        if !self.inner.state.borrow().listed.contains(provider) {
            let listing = self.ask(cloud, false);
            tokio::task::spawn_local(async move {
                let _ = listing.await;
            });
        }
    }
    /// The provider's list, asked for now (or the ask under way, unless `refresh` wants a newer one). Once answered it's
    /// what Kumi knows of the provider's models, and it's kept for the next run, unless a sign-in changed meanwhile.
    async fn list(&self, provider: ProviderId, refresh: bool) -> Result<Vec<ModelInfo>, RuntimeError> {
        self.ask(provider, refresh).await
    }
    fn ask(&self, provider: ProviderId, refresh: bool) -> Listing {
        let id = provider.as_str();
        let under_way = self.inner.state.borrow().listing.get(id).filter(|listing| listing.peek().is_none()).cloned();
        if let Some(listing) = under_way.filter(|_| !refresh) {
            return listing;
        }
        // What the ask needs, and the control only weakly: it keeps its asks, which mustn't keep it.
        // (Through the cloud providers' one client, whose connection a first message reuses.)
        let (signed, store, env, fetch, lists) = (
            self.inner.state.borrow().signed,
            self.inner.options.store.clone(),
            self.inner.options.env.clone(),
            Some(self.inner.cloud.clone()),
            self.inner.options.lists.clone(),
        );
        let (inner, closed) = (Rc::downgrade(&self.inner), self.inner.closed.clone());
        let listing: Listing = async move {
            let saved = store.get(provider.row().credential).await.ok().flatten();
            let who = model_lists::identity(provider, saved.as_ref(), &env);
            let signal = Some(abort::any([abort::timeout(15000), closed]));
            let listed = match list_models(provider, ListOptions { store, env: Some(env), fetch, signal }).await {
                Ok(listed) => listed,
                Err(error) => {
                    // The provider refuses this sign-in: a list kept for it mustn't hide that. It goes, and the next look
                    // asks again and says so, as without one.
                    let refused = error.kumi().is_some_and(|failure| failure.kind == FailureKind::Auth);
                    if let Some(inner) = inner.upgrade().filter(|inner| refused && inner.state.borrow().signed == signed) {
                        let id = provider.as_str();
                        {
                            let mut state = inner.state.borrow_mut();
                            if !state.listed.contains(id) {
                                state.catalog.remove(id);
                            }
                        }
                        if let Some(lists) = &lists {
                            lists.forget(&[id]);
                        }
                    }
                    return Err(error);
                }
            };
            let Some(inner) = inner.upgrade() else { return Ok(listed) };
            if inner.state.borrow().signed == signed {
                let id = provider.as_str();
                let control = ModelControl { inner: inner.clone() };
                // What the model in use was, as the list kept from an earlier run said (it was bound from that).
                let model = inner.state.borrow().model.clone();
                let kept = !inner.state.borrow().listed.contains(id) && inner.state.borrow().catalog.contains_key(id);
                let before = kept.then(|| bound_by(control.info_for(model.as_deref()))).flatten();
                {
                    let mut state = inner.state.borrow_mut();
                    state.catalog.insert(id.into(), listed.clone());
                    state.listed.insert(id.into());
                }
                if let (Some(lists), Some(who)) = (lists, who) {
                    lists.keep(id, &who, kumi_common::time::now_ms(), &listed);
                }
                // The provider says otherwise now of what the binding read of it (its faster tier, whether it takes tools,
                // its name): bound again, from the next message on, as when the producer changes the model.
                if kept
                    && control.parse(model.as_deref()).is_some_and(|parsed| parsed.provider == id)
                    && before != bound_by(control.info_for(model.as_deref()))
                {
                    // (and a binding being made from the kept list isn't joined)
                    let mut state = inner.state.borrow_mut();
                    state.bound = None;
                    state.binding = None;
                    drop(state);
                    let _ = (inner.options.changed)().await;
                }
            }
            Ok(listed)
        }
        .boxed_local()
        .shared();
        self.inner.state.borrow_mut().listing.insert(id.into(), listing.clone());
        listing
    }
    /// A sign-in changed for these providers: what they listed, here and kept, goes; they list afresh.
    fn signed_anew(&self, providers: &[&str]) {
        {
            let mut state = self.inner.state.borrow_mut();
            state.signed += 1;
            for provider in providers {
                state.catalog.remove(*provider);
                state.listed.remove(*provider);
                state.listing.remove(*provider);
            }
        }
        if let Some(lists) = &self.inner.options.lists {
            lists.forget(providers);
        }
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
        let info = provider.row();
        if info.sign_in != SignIn::ApiKey {
            return Err(KumiError::new(FailureKind::Config, format!("{} signs in with ChatGPT, not a key.", info.name)).into());
        }
        let key = trim(key).to_string();
        let verdict = check_api_key(provider, &key, Transport { fetch: Some(self.inner.cloud.clone()), signal }).await;
        if verdict == ApiKeyCheck::Refused {
            return Ok(verdict);
        }
        self.inner
            .options
            .store
            .update(info.credential, Box::new(move |_| async move { Ok(Some(Credential::ApiKey { key })) }.boxed_local()))
            .await?;
        // Every provider the key opens lists afresh.
        let opened: Vec<&str> = table::rows().filter(|row| row.credential == info.credential).map(|row| row.id).collect();
        self.signed_anew(&opened);
        if self.current_provider().is_some_and(|p| p.row().credential == info.credential) {
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
        let chatgpt: Vec<&str> = table::rows().filter(|row| row.sign_in == SignIn::Chatgpt).map(|row| row.id).collect();
        self.signed_anew(&chatgpt);
        let model = self.inner.state.borrow().model.clone();
        if model.as_ref().is_none_or(|s| s.is_empty()) {
            self.choose_default().await?;
            return Ok(());
        }
        if self.current_provider().is_some_and(|provider| provider.row().sign_in == SignIn::Chatgpt) {
            self.inner.state.borrow_mut().bound = None;
            (self.inner.options.changed)().await?;
        }
        Ok(())
    }
    pub async fn sign_out(&self, provider: ProviderId) -> Result<bool, RuntimeError> {
        let info = provider.row();
        if self.inner.options.store.get(info.credential).await?.is_none() {
            return Ok(false);
        }
        self.inner.options.store.update(info.credential, Box::new(|_| async { Ok(None) }.boxed_local())).await?;
        self.signed_anew(&[provider.as_str()]);
        if self.current_provider().is_some_and(|p| p.row().credential == info.credential) {
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
            let key = (model.clone(), effort, tier.clone());
            let joined =
                self.inner.state.borrow().binding.as_ref().filter(|(making, _)| *making == key).map(|(_, binding)| binding.clone());
            let making = joined.unwrap_or_else(|| {
                let control = self.clone();
                let made = key.clone();
                let binding: Binding = async move {
                    let (model, effort, tier) = made.clone();
                    let bound = control.bind(&model, effort, tier.clone()).await;
                    let mut state = control.inner.state.borrow_mut();
                    if state.binding.as_ref().is_some_and(|(making, _)| *making == made) {
                        state.binding = None;
                    }
                    let binding = bound?;
                    state.bound = Some(CachedBinding { model, effort, tier, binding: binding.clone() });
                    drop(state);
                    if let Some(asked) = binding.asked() {
                        let saying = binding.clone();
                        tokio::task::spawn_local(async move {
                            asked.await;
                            if let Some(note) = saying.note() {
                                control.say(note);
                            }
                        });
                    }
                    Ok(binding)
                }
                .boxed_local()
                .shared();
                self.inner.state.borrow_mut().binding = Some((key, binding.clone()));
                binding
            });
            making.await?
        };
        Ok(BindingInfo { binding: binding.binding(), asked: binding.asked(), backing: binding })
    }
    pub async fn binding(&self) -> Result<ModelBinding, RuntimeError> {
        Ok(self.binding_with_info().await?.binding)
    }
    /// Says, once, that the fallback can't be used and why (not signed in there, say).
    pub fn fallback_unusable(&self, error: &RuntimeError) {
        let fallback = self.inner.state.borrow().fallback.clone().unwrap_or_default();
        self.say(format!("Your fallback model, {fallback}, can't carry on for now: {}", error.message()));
    }
    /// The fallback's binding, unless it's the model already chosen (or none is set); an error when it can't be used
    /// (not signed in there), which a kernel goes without.
    pub async fn fallback_binding(&self) -> Result<Option<ModelBinding>, RuntimeError> {
        let (fallback, model) = {
            let state = self.inner.state.borrow();
            (state.fallback.clone(), state.model.clone())
        };
        let Some(fallback) = fallback.filter(|fallback| Some(fallback) != model.as_ref()) else { return Ok(None) };
        Ok(Some(self.bind(&fallback, None, None).await?.binding()))
    }
    /// The producer began a message: the chosen cloud model is bound (its sign-in read, refreshed when due, and its
    /// faster tier's list when `/fast` is on) and an API-key provider's connection opened, so the message waits on
    /// neither: a message sent meanwhile joins the binding being made. At
    /// most once every `WARM_AGAIN`; it says nothing and changes nothing the producer sees, whatever comes of it: no model
    /// chosen, a model server's, not signed in, offline.
    pub async fn warm(&self) {
        let model = self.inner.state.borrow().model.clone().filter(|model| !model.is_empty());
        let Some(Parsed { provider, server: None, .. }) = self.parse(model.as_deref()) else { return };
        let Some(provider) = ProviderId::parse(&provider) else { return };
        {
            let mut state = self.inner.state.borrow_mut();
            if state.warmed.is_some_and(|at| at.elapsed() < WARM_AGAIN) {
                return;
            }
            state.warmed = Some(std::time::Instant::now());
        }
        if self.binding_with_info().await.is_err() {
            return;
        }
        // ChatGPT's answers go over a WebSocket of their own, which its conversation's kernel readies (`Kernel::warm`); a
        // session's first message still opens it. Through a proxy they go over HTTP, readied here as any provider's.
        let direct =
            reqwest::Url::parse(provider.row().base_url).is_ok_and(|address| kumi_net::proxy::current().proxy_for(&address).is_none());
        if provider.row().sign_in == SignIn::Chatgpt && direct {
            return;
        }
        // Any answer will do: the connection it went on stays open for the message (a HEAD, so nothing is read).
        let signal = abort::any([abort::timeout(5_000), self.inner.closed.clone()]);
        let init = FetchInit { method: "HEAD".into(), signal: Some(signal), ..Default::default() };
        let _ = self.inner.cloud.fetch(provider.row().base_url, init).await;
    }
    /// Another model the producer chose for some work (background workers), at its provider's default effort.
    pub async fn binding_for(&self, model: &str) -> Result<ModelBinding, RuntimeError> {
        let tier = self.tier_for(model).await;
        Ok(self.bind(model, None, tier).await?.binding())
    }
}

#[async_trait::async_trait(?Send)]
impl ModelController for ModelControl {
    fn current(&self) -> CurrentModel {
        ModelControl::current(self)
    }
    async fn providers(&self) -> Result<Vec<ProviderStatus>, RuntimeError> {
        ModelControl::providers(self).await
    }
    async fn warm(&self) {
        ModelControl::warm(self).await
    }
    async fn local(&self) -> Vec<LocalStatus> {
        ModelControl::local(self).await
    }
    fn provider_name(&self, id: &str) -> String {
        ModelControl::provider_name(self, id)
    }
    fn cloud_provider(&self, id: &str) -> Option<ProviderId> {
        ModelControl::cloud_provider(self, id)
    }
    fn context_window(&self, model: &str) -> Option<f64> {
        self.info_for(Some(model)).and_then(|info| info.context)
    }
    async fn choose_default(&self) -> Result<Option<DefaultModel>, RuntimeError> {
        ModelControl::choose_default(self).await
    }
    async fn models(&self, provider: &str, refresh: bool) -> Result<Vec<ModelInfo>, RuntimeError> {
        ModelControl::models(self, provider, refresh).await
    }
    async fn listed_now(&self, provider: &str) -> Result<Vec<ModelInfo>, RuntimeError> {
        ModelControl::listed_now(self, provider).await
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
    fn fallback(&self) -> Option<String> {
        self.inner.state.borrow().fallback.clone()
    }
    async fn set_fallback(&self, model: Option<&str>) -> Result<(), RuntimeError> {
        let model = model.filter(|model| !model.is_empty()).map(str::to_string);
        if let Some(model) = &model {
            if self.parse(Some(model)).is_none() {
                return Err(KumiError::new(FailureKind::Config, format!("{model} isn't a model Kumi knows how to reach.")).into());
            }
        }
        // Everything else as it is (model, effort and fast are written only as given); null clears it.
        let mut settings = serde_json::to_value(read_settings(&self.inner.options.settings_file)).unwrap_or_else(|_| json!({}));
        settings["fallbackModel"] = json!(model);
        write_settings(&self.inner.options.settings_file, &settings)?;
        self.inner.state.borrow_mut().fallback = model;
        // The next kernel carries it.
        (self.inner.options.changed)().await
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
    fn server_names(&self) -> Vec<(String, String)> {
        self.servers().into_iter().map(|server| (server.id, server.name)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kumi_ai::auth::store::open_credential_store;

    /// The app makes its model controls with no fetch of its own: their cloud client is then Kumi's own HTTP, so ChatGPT's
    /// answers still go over its WebSocket. A wrapper around it would quietly send them over HTTP.
    #[test]
    fn the_apps_cloud_client_is_kumis_own_http_so_chatgpt_keeps_its_websocket() {
        let folder = tempfile::tempdir().unwrap();
        let made = |fetch: Option<Rc<dyn Fetch>>| {
            create_model_control(ModelControlOptions {
                store: Rc::new(open_credential_store(folder.path().join("auth.json"))),
                settings_file: folder.path().join("settings.json").to_string_lossy().into_owned(),
                env: Env::new(),
                changed: Rc::new(|| async { Ok(()) }.boxed_local()),
                fetch,
                say: None,
                installed: None,
                lists: None,
            })
        };
        assert!(made(None).inner.cloud.is_kumis_own());
        // A test's or a probe's fetch is used as given, and isn't Kumi's own.
        assert!(!made(Some(Rc::new(Given))).inner.cloud.is_kumis_own());
    }

    struct Given;
    #[async_trait::async_trait(?Send)]
    impl Fetch for Given {
        async fn fetch(&self, _: &str, _: kumi_ai::http::FetchInit) -> Result<kumi_ai::http::Response, kumi_ai::error::LanguageModelError> {
            Ok(kumi_ai::http::Response::text_response(200, ""))
        }
    }
}
