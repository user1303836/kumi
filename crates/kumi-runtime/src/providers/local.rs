pub use super::models::Transport;
use super::{
    compat::mending_fetch,
    models::{EffortInfo, ModelInfo},
    ollama::{ollama_chat, FailurePhase, OllamaChatSettings, OllamaShape},
    words_only, Effort, PROVIDERS, USER_AGENT,
};
use crate::{
    ai::{
        error::{ApiCallError, LanguageModelError},
        http::{default_fetch, Fetch, FetchInit, Response},
        openai_compatible::{openai_compatible, CompatibleSettings},
        types::*,
    },
    core::errors::{FailureKind, KumiError, RuntimeError},
    kernel::{
        agent::{LanguageModel, ModelBinding, ModelRequest},
        budget::{budget_for, BYTES_PER_TOKEN},
        failure::clean_detail,
    },
};
use async_trait::async_trait;
use futures::{
    future::{join_all, LocalBoxFuture, Shared},
    FutureExt, StreamExt,
};
use kumi_common::{
    abort::{self, Signal},
    js::{
        json::stringify,
        number,
        string::{head, trim, utf16_len},
    },
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    path::Path,
    rc::Rc,
    sync::LazyLock,
};
use url::Url;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LocalKind {
    Ollama,
    Lmstudio,
    OpenaiCompatible,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalServer {
    pub id: String,
    pub kind: LocalKind,
    pub name: String,
    #[serde(rename = "baseURL")]
    pub base_url: String,
    #[serde(rename = "apiKey", default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    pub r#where: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerSetting {
    pub name: String,
    #[serde(rename = "baseURL")]
    pub base_url: String,
    #[serde(rename = "apiKey", default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
}
pub const LOCAL_PROVIDERS: [&str; 2] = ["ollama", "lmstudio"];
const HERE: &str = "on this computer";
const PROBE_MS: u64 = 1500;
const LIST_MS: u64 = 10000;
const ROOM: f64 = 48. * 1024.;
const ANSWER: f64 = 8192.;
const STEP: f64 = 8192.;
pub fn context_for(fixed: f64, most: Option<f64>) -> f64 {
    let wanted = ((((fixed + ROOM) / BYTES_PER_TOKEN).ceil() + ANSWER) / STEP).ceil() * STEP;
    most.filter(|n| *n > 0.).map(|n| wanted.min(n)).unwrap_or(wanted)
}
fn too_small(window: f64, fixed: usize) -> bool {
    window < (fixed as f64 / 4.).ceil() + 2048.
}
fn enough(window: f64, fixed: usize, most: Option<f64>) -> bool {
    window >= context_for(fixed as f64, most).min(((fixed as f64 + 16. * 1024.) / BYTES_PER_TOKEN).ceil() + ANSWER)
}
fn text(value: &Value, max: usize) -> Option<String> {
    value.as_str().map(trim).filter(|s| !s.is_empty()).map(|s| head(s, max))
}
fn count(value: &Value) -> Option<f64> {
    value.as_f64().filter(|n| number::is_safe_integer(*n) && *n > 0.)
}
fn rows(value: &Value) -> Vec<&Value> {
    value.as_array().into_iter().flatten().filter(|v| v.is_object()).collect()
}
fn words(values: &[&Value]) -> Vec<String> {
    values.iter().filter_map(|v| text(v, 40)).collect()
}
fn levels(value: &Value) -> Vec<Effort> {
    value.as_array().into_iter().flatten().filter_map(|v| v.as_str().and_then(Effort::parse)).collect()
}
fn tokens(value: f64) -> String {
    let text = number::to_string(value);
    let (integer, fraction) = text.split_once('.').map(|(a, b)| (a, Some(b))).unwrap_or((&text, None));
    let mut grouped = String::new();
    for (i, c) in integer.chars().enumerate() {
        if i > 0 && (integer.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(c);
    }
    if let Some(fraction) = fraction {
        grouped.push('.');
        grouped.push_str(fraction);
    }
    grouped
}
fn server(id: &str, kind: LocalKind, name: &str, base_url: String, api_key: Option<String>) -> Result<LocalServer, RuntimeError> {
    let parsed = Url::parse(&base_url).map_err(|e| RuntimeError::plain(e.to_string()))?;
    let host = parsed.host_str().unwrap_or("").trim_start_matches('[').trim_end_matches(']');
    static LOOPBACK: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"(?i)^(localhost|127(\.\d{1,3}){3}|::1)$").unwrap());
    let location = if LOOPBACK.is_match(host) { HERE.into() } else { format!("on {host}") };
    Ok(LocalServer { id: id.into(), kind, name: name.into(), base_url, api_key: api_key.filter(|k| !k.is_empty()), r#where: location })
}
fn ollama_address(host: Option<&String>) -> String {
    let value = host.map(|v| trim(v)).unwrap_or("");
    if !value.is_empty() {
        static SCHEME: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"(?i)^[a-z][a-z0-9+.-]*://").unwrap());
        static PORT: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"^[^/]*:\d+(/|$)").unwrap());
        // Kumi reaches Ollama over http or https only: another scheme (tcp://, unix://) has no origin to name.
        let parsed = Url::parse(&if SCHEME.is_match(value) { value.into() } else { format!("http://{value}") });
        if let Some(mut url) = parsed.ok().filter(|url| matches!(url.scheme(), "http" | "https")) {
            if url.host_str() == Some("0.0.0.0") {
                let _ = url.set_host(Some("127.0.0.1"));
            }
            if url.host_str() == Some("[::]") {
                let _ = url.set_host(Some("[::1]"));
            }
            if url.port().is_none() && !PORT.is_match(&SCHEME.replace(value, "")) && url.scheme() == "http" {
                let _ = url.set_port(Some(11434));
            }
            return format!("{}{}", url.origin().ascii_serialization(), url.path().trim_end_matches('/'));
        }
    }
    "http://127.0.0.1:11434".into()
}
fn compatible_address(value: &str) -> Result<String, RuntimeError> {
    let url = Url::parse(value).map_err(|e| RuntimeError::plain(e.to_string()))?;
    let path = url.path().trim_end_matches('/');
    Ok(format!("{}{}", url.origin().ascii_serialization(), if path.is_empty() { "/v1" } else { path }))
}
pub fn local_servers(settings: &[ServerSetting], env: &HashMap<String, String>) -> Result<Vec<LocalServer>, RuntimeError> {
    let mut servers = vec![
        server("ollama", LocalKind::Ollama, "Ollama", ollama_address(env.get("OLLAMA_HOST")), None)?,
        server("lmstudio", LocalKind::Lmstudio, "LM Studio", "http://127.0.0.1:1234/v1".into(), env.get("LM_API_TOKEN").cloned())?,
    ];
    let mut taken: HashSet<String> =
        PROVIDERS.iter().map(|p| p.as_str().into()).chain(LOCAL_PROVIDERS.iter().map(|p| (*p).into())).collect();
    static WORD: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new("[^a-z0-9]+").unwrap());
    for setting in settings {
        let word = head(WORD.replace_all(&setting.name.to_lowercase(), "-").trim_matches('-'), 32);
        let word = if word.is_empty() { "server".into() } else { word };
        let mut id = word.clone();
        let mut n = 2;
        while taken.contains(&id) {
            id = format!("{word}-{n}");
            n += 1;
        }
        taken.insert(id.clone());
        servers.push(server(
            &id,
            LocalKind::OpenaiCompatible,
            &setting.name,
            compatible_address(&setting.base_url)?,
            setting.api_key.clone(),
        )?);
    }
    Ok(servers)
}
#[derive(Debug, Clone)]
pub struct ParsedLocalModelId {
    pub server: LocalServer,
    pub model: String,
}
pub fn parse_local_model_id(value: &str, servers: &[LocalServer]) -> Option<ParsedLocalModelId> {
    let (id, model) = value.split_once('/')?;
    static ID: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"^[a-z0-9][a-z0-9-]{0,39}$").unwrap());
    if !ID.is_match(id)
        || model.is_empty()
        || utf16_len(model) > 256
        || trim(model) != model
        || model.chars().any(|c| c <= '\u{1f}' || c == '\u{7f}')
    {
        return None;
    }
    let server = servers.iter().find(|s| s.id == id)?.clone();
    Some(ParsedLocalModelId { server, model: model.into() })
}
pub fn start_hint(server: &LocalServer) -> String {
    if server.r#where != HERE {
        return format!("Check that {} is running {} and can be reached from here", server.name, server.r#where);
    }
    match server.kind {
        LocalKind::Ollama => "Open Ollama, or run: ollama serve".into(),
        LocalKind::Lmstudio => "Open LM Studio and start its server (Developer tab), or run: lms server start".into(),
        LocalKind::OpenaiCompatible => format!("Start it, or check its address ({}) in ~/.kumi/settings.json", server.base_url),
    }
}
pub fn local_installed(kind: LocalKind, env: Option<&HashMap<String, String>>) -> bool {
    let process = crate::system::process_env();
    let env = env.unwrap_or(&process);
    let home = home::home_dir().unwrap_or_default();
    let on_path = |name: &str| {
        std::env::split_paths(env.get("PATH").map(String::as_str).unwrap_or(""))
            .any(|dir| !dir.as_os_str().is_empty() && (dir.join(name).exists() || dir.join(format!("{name}.exe")).exists()))
    };
    match kind {
        LocalKind::Ollama => {
            on_path("ollama")
                || Path::new("/Applications/Ollama.app").exists()
                || home.join("Applications/Ollama.app").exists()
                || env.get("LOCALAPPDATA").is_some_and(|v| !v.is_empty() && Path::new(v).join("Programs/Ollama").exists())
        }
        LocalKind::Lmstudio => home.join(".lmstudio").exists() || Path::new("/Applications/LM Studio.app").exists() || on_path("lms"),
        LocalKind::OpenaiCompatible => false,
    }
}
async fn request_json(
    url: &str,
    server: &LocalServer,
    options: &Transport,
    body: Option<Value>,
    ms: Option<u64>,
) -> Result<Value, LanguageModelError> {
    let timeout = ms.map(abort::timeout);
    let signal = if let Some(timeout) = &timeout {
        Some(abort::any(options.signal.clone().into_iter().chain([timeout.clone()])))
    } else {
        options.signal.clone()
    };
    let mut headers = [("user-agent".into(), USER_AGENT.clone())].into_iter().collect::<crate::ai::http::Headers>();
    if body.is_some() {
        headers.insert("content-type".into(), "application/json".into());
    }
    if let Some(key) = &server.api_key {
        headers.insert("authorization".into(), format!("Bearer {key}"));
    }
    let result = options
        .fetch
        .clone()
        .unwrap_or_else(default_fetch)
        .fetch(
            url,
            FetchInit { method: if body.is_some() { "POST" } else { "GET" }.into(), headers, body: body.as_ref().map(stringify), signal },
        )
        .await;
    let response = match result {
        Err(_) if timeout.as_ref().is_some_and(Signal::is_cancelled) && !options.signal.as_ref().is_some_and(Signal::is_cancelled) => {
            return Err(LanguageModelError::other("TimeoutError"))
        }
        other => other?,
    };
    let status = response.status;
    let ok = response.ok();
    let answer = response.text().await?;
    let refused = |message: String| {
        let mut error = ApiCallError::new(message, url, Some(body.clone().unwrap_or(json!({}))), Some(status));
        error.response_body = Some(head(&answer, 4000));
        error.is_retryable = false;
        LanguageModelError::ApiCall(error)
    };
    if !ok {
        return Err(refused(format!("HTTP {status}")));
    }
    serde_json::from_str(&answer).map_err(|_| refused("The server's answer wasn't JSON.".into()))
}
pub async fn probe_local(server: &LocalServer, options: Transport) -> bool {
    let url = format!("{}/{}", server.base_url, if server.kind == LocalKind::Ollama { "api/version" } else { "models" });
    match request_json(&url, server, &options, None, Some(PROBE_MS)).await {
        Ok(_) => true,
        Err(LanguageModelError::ApiCall(error)) => matches!(error.status_code, Some(401 | 403)),
        _ => false,
    }
}
#[derive(Clone, Debug)]
struct CopyInfo {
    id: String,
    context: Option<f64>,
}
#[derive(Clone, Debug)]
struct Facts {
    model: String,
    name: String,
    tools: Option<bool>,
    thinks: Option<bool>,
    levels: Vec<Effort>,
    think_default: Option<Value>,
    vision: Option<bool>,
    most: Option<f64>,
    served: Option<f64>,
    loaded: Vec<CopyInfo>,
    in_memory: bool,
    details: Vec<String>,
}
impl Facts {
    fn new(model: String) -> Self {
        Self {
            name: model.clone(),
            model,
            tools: None,
            thinks: None,
            levels: vec![],
            think_default: None,
            vision: None,
            most: None,
            served: None,
            loaded: vec![],
            in_memory: false,
            details: vec![],
        }
    }
}
fn ollama_facts(name: &str, details: &Value, show: &Value, in_memory: bool) -> Option<Facts> {
    let capabilities = show["capabilities"].as_array();
    if capabilities.is_some_and(|v| !v.contains(&json!("completion"))) {
        return None;
    }
    let info = &show["model_info"];
    let architecture = text(&info["general.architecture"], 40);
    let most = architecture
        .and_then(|a| count(&info[format!("{a}.context_length")]))
        .or_else(|| info.as_object().and_then(|m| m.iter().find(|(key, _)| key.ends_with(".context_length"))).and_then(|(_, v)| count(v)));
    let thinking = &show["thinking"];
    let shown = &show["details"];
    let mut facts = Facts::new(name.into());
    facts.levels = levels(&thinking["values"]);
    if let Some(capabilities) = capabilities {
        facts.tools = Some(capabilities.contains(&json!("tools")));
        facts.thinks = Some(capabilities.contains(&json!("thinking")));
        facts.vision = Some(capabilities.contains(&json!("vision")));
    }
    facts.think_default = thinking.get("default").filter(|v| v.is_boolean() || v.is_string()).cloned();
    facts.most = most;
    facts.in_memory = in_memory;
    facts.details = words(&[
        details.get("parameter_size").filter(|v| !v.is_null()).unwrap_or(&shown["parameter_size"]),
        details.get("quantization_level").filter(|v| !v.is_null()).unwrap_or(&shown["quantization_level"]),
    ]);
    Some(facts)
}
async fn ollama_list(server: &LocalServer, options: &Transport) -> Result<Vec<Facts>, LanguageModelError> {
    let tags = request_json(&format!("{}/api/tags", server.base_url), server, options, None, None).await?;
    let running = request_json(&format!("{}/api/ps", server.base_url), server, options, None, None).await.unwrap_or(Value::Null);
    let running: HashSet<String> = rows(&running["models"])
        .into_iter()
        .map(|r| text(r.get("name").filter(|v| !v.is_null()).unwrap_or(&r["model"]), 300).unwrap_or_default())
        .collect();
    let listed: Vec<_> = rows(&tags["models"])
        .into_iter()
        .filter_map(|r| text(r.get("name").filter(|v| !v.is_null()).unwrap_or(&r["model"]), 300).map(|name| (name, r["details"].clone())))
        .collect();
    let mut facts = vec![];
    for group in listed.chunks(8) {
        let running = &running;
        let replies = join_all(group.iter().map(|(name, details)| async move {
            let show = request_json(&format!("{}/api/show", server.base_url), server, options, Some(json!({"model":name})), None)
                .await
                .unwrap_or(Value::Null);
            ollama_facts(name, details, &show, running.contains(name))
        }))
        .await;
        facts.extend(replies.into_iter().flatten());
    }
    Ok(facts)
}
fn otherwise(error: &LanguageModelError) -> bool {
    matches!(error,LanguageModelError::ApiCall(e)if e.status_code.is_some()&&!matches!(e.status_code,Some(401|403)))
}
fn origin(server: &LocalServer) -> String {
    Url::parse(&server.base_url).unwrap().origin().ascii_serialization()
}
async fn compatible_list(server: &LocalServer, options: &Transport) -> Result<Vec<Facts>, LanguageModelError> {
    let body = request_json(&format!("{}/models", server.base_url), server, options, None, None).await?;
    let listed: Vec<_> =
        rows(&body["data"]).into_iter().filter(|r| r["id"].as_str().is_some_and(|s| !s.to_lowercase().contains("embed"))).collect();
    let started = if listed.iter().any(|r| r["owned_by"] == "llamacpp") {
        request_json(&format!("{}/props", origin(server)), server, options, None, None)
            .await
            .ok()
            .and_then(|p| count(&p["default_generation_settings"]["n_ctx"]))
    } else {
        None
    };
    Ok(listed
        .into_iter()
        .map(|r| {
            let mut f = Facts::new(r["id"].as_str().unwrap().into());
            f.served = started
                .or_else(|| count(&r["max_model_len"]))
                .or_else(|| count(&r["context_length"]))
                .or_else(|| count(&r["max_context_length"]));
            f
        })
        .collect())
}
async fn lm_studio_list(server: &LocalServer, options: &Transport) -> Result<(Vec<Facts>, bool), LanguageModelError> {
    let ask = |url: String| async move {
        match request_json(&url, server, options, None, None).await {
            Ok(v) => Ok(v),
            Err(e) if otherwise(&e) => Ok(Value::Null),
            Err(e) => Err(e),
        }
    };
    let current = ask(format!("{}/api/v1/models", origin(server))).await?;
    if current["models"].is_array() {
        return Ok((
            rows(&current["models"])
                .into_iter()
                .filter(|r| r["type"] == "llm" && r["key"].is_string())
                .map(|r| {
                    let mut f = Facts::new(r["key"].as_str().unwrap().into());
                    f.name = text(&r["display_name"], 60).unwrap_or_else(|| f.model.clone());
                    let capabilities = &r["capabilities"];
                    let reasoning = &capabilities["reasoning"];
                    f.levels = levels(&reasoning["allowed_options"]);
                    f.tools = capabilities["trained_for_tool_use"].as_bool();
                    f.think_default = reasoning.get("default").filter(|v| v.is_string()).cloned();
                    f.vision = (capabilities["vision"] == true).then_some(true);
                    f.most = count(&r["max_context_length"]);
                    f.loaded = rows(&r["loaded_instances"])
                        .into_iter()
                        .filter_map(|copy| {
                            Some(CopyInfo { id: copy["id"].as_str()?.into(), context: count(&copy["config"]["context_length"]) })
                        })
                        .collect();
                    f.in_memory = !f.loaded.is_empty();
                    f.details = words(&[&r["params_string"], &r["quantization"]["name"]]);
                    f
                })
                .collect(),
            true,
        ));
    }
    let older = ask(format!("{}/api/v0/models", origin(server))).await?;
    if older["data"].is_array() {
        return Ok((
            rows(&older["data"])
                .into_iter()
                .filter(|r| matches!(r["type"].as_str(), Some("llm" | "vlm")) && r["id"].is_string())
                .map(|r| {
                    let mut f = Facts::new(r["id"].as_str().unwrap().into());
                    f.tools = r["capabilities"].as_array().map(|v| v.contains(&json!("tool_use")));
                    f.vision = (r["type"] == "vlm").then_some(true);
                    f.most = count(&r["max_context_length"]);
                    f.served = count(&r["loaded_context_length"]);
                    f.in_memory = r["state"] == "loaded";
                    f.details = words(&[&r["quantization"]]);
                    f
                })
                .collect(),
            false,
        ));
    }
    Ok((compatible_list(server, options).await?, false))
}
async fn list_facts(server: &LocalServer, options: &Transport) -> Result<Vec<Facts>, LanguageModelError> {
    match server.kind {
        LocalKind::Ollama => ollama_list(server, options).await,
        LocalKind::Lmstudio => Ok(lm_studio_list(server, options).await?.0),
        LocalKind::OpenaiCompatible => compatible_list(server, options).await,
    }
}
fn info_of(server: &LocalServer, facts: Facts) -> ModelInfo {
    let mut description = facts.details;
    if facts.in_memory {
        description.push("loaded".into());
    }
    if facts.tools == Some(false) {
        description.push("can't change the Set".into());
    }
    let description = description.join(" · ");
    ModelInfo {
        id: format!("{}/{}", server.id, facts.model),
        provider: server.id.clone(),
        model: facts.model,
        name: facts.name,
        description: (!description.is_empty()).then_some(description),
        efforts: facts.levels.into_iter().map(|effort| EffortInfo { effort, description: None }).collect(),
        default_effort: facts.think_default.as_ref().and_then(Value::as_str).and_then(Effort::parse),
        tools: facts.tools,
        context: facts.served.or(facts.most),
        loaded: facts.in_memory.then_some(true),
        r#where: Some(server.r#where.clone()),
        service_tiers: Vec::new(),
    }
}
pub async fn list_local_models(server: &LocalServer, options: Transport) -> Result<Vec<ModelInfo>, RuntimeError> {
    let timeout = abort::timeout(LIST_MS);
    let transport = Transport { fetch: options.fetch, signal: Some(abort::any(options.signal.into_iter().chain([timeout.clone()]))) };
    match list_facts(server, &transport).await {
        Ok(facts) => Ok(facts.into_iter().map(|f| info_of(server, f)).collect()),
        Err(error) => {
            let error = if timeout.is_cancelled() { LanguageModelError::other("TimeoutError") } else { error };
            if unanswered(&error) || matches!(error,LanguageModelError::ApiCall(ref e)if matches!(e.status_code,Some(401|403))) {
                return Err(failure(server, error, FailurePhase::Request, None, None, None).into());
            }
            let status = error.api_call().and_then(|e| e.status_code).map(|s| format!(" (HTTP {s})")).unwrap_or_default();
            Err(KumiError::with_provider(
                FailureKind::Provider,
                format!("{} couldn't list its models{status}; check that {} is the server's address.", server.name, server.base_url),
                &server.id,
            )
            .into())
        }
    }
}

/// A local model binding and the capability note learned while it starts or runs.
pub struct LocalBinding {
    pub binding: ModelBinding,
    pub asked: Shared<LocalBoxFuture<'static, ()>>,
    state: Rc<LocalState>,
}
impl LocalBinding {
    pub fn note(&self) -> Option<String> {
        self.state.note.borrow().clone()
    }
    pub fn into_binding(self) -> ModelBinding {
        self.binding
    }
}
#[derive(Default)]
pub struct LocalModelOptions {
    pub fetch: Option<Rc<dyn Fetch>>,
    pub effort: Option<Effort>,
    pub on_note: Option<Rc<dyn Fn(String)>>,
}
const NO_TOOLS: &str = "This model can't use tools here: you see the Set only as it's described above, and can't read more of it or change anything. When asked for a change, say so plainly, and that /model chooses a model that can make it.";
struct Identified(Rc<dyn Fetch>);
#[async_trait(?Send)]
impl Fetch for Identified {
    async fn fetch(&self, url: &str, mut init: FetchInit) -> Result<Response, LanguageModelError> {
        init.headers.insert("user-agent".into(), USER_AGENT.clone());
        self.0.fetch(url, init).await
    }
}
struct LocalState {
    server: LocalServer,
    model: String,
    identified: Rc<dyn Fetch>,
    effort: Option<Effort>,
    on_note: Option<Rc<dyn Fn(String)>>,
    facts: RefCell<Option<Facts>>,
    note: RefCell<Option<String>>,
    window: Cell<Option<f64>>,
    learned: Cell<Option<f64>>,
    fixed: Cell<usize>,
    instance: RefCell<Option<String>>,
    squeezed: Cell<f64>,
}
impl LocalState {
    fn kumi_sets(&self) -> bool {
        self.server.kind == LocalKind::Ollama || self.instance.borrow().is_some()
    }
    fn tell(&self, said: String, quiet: bool) {
        if self.note.borrow().as_ref() == Some(&said) {
            return;
        }
        *self.note.borrow_mut() = Some(said.clone());
        if !quiet {
            if let Some(on_note) = &self.on_note {
                on_note(said);
            }
        }
    }
    fn transport(&self, signal: Option<Signal>) -> Transport {
        Transport { fetch: Some(self.identified.clone()), signal }
    }
    async fn learn(&self, signal: Option<Signal>, quiet: bool) -> Result<Facts, LanguageModelError> {
        let transport = self.transport(signal);
        let found = if self.server.kind == LocalKind::Ollama {
            let show = request_json(
                &format!("{}/api/show", self.server.base_url),
                &self.server,
                &transport,
                Some(json!({"model":self.model})),
                None,
            )
            .await?;
            ollama_facts(&self.model, &json!({}), &show, false).ok_or_else(|| {
                KumiError::with_provider(
                    FailureKind::Model,
                    format!("{} on Ollama doesn't chat (it embeds, or makes pictures): choose another model with /model.", self.model),
                    &self.server.id,
                )
            })?
        } else {
            let listed = if self.server.kind == LocalKind::Lmstudio {
                lm_studio_list(&self.server, &transport).await?.0
            } else {
                compatible_list(&self.server, &transport).await?
            };
            let found = listed
                .iter()
                .find(|item| item.model == self.model)
                .or_else(|| listed.iter().find(|item| item.loaded.iter().any(|copy| copy.id == self.model)))
                .cloned();
            if found.is_none() && self.server.kind == LocalKind::Lmstudio {
                return Err(KumiError::with_provider(FailureKind::Model, missing(&self.server, &self.model), &self.server.id).into());
            }
            found.unwrap_or_else(|| Facts::new(self.model.clone()))
        };
        *self.facts.borrow_mut() = Some(found.clone());
        if found.tools == Some(false) {
            let others = list_facts(&self.server, &transport).await.ok();
            let able = others.as_ref().and_then(|others| {
                others
                    .iter()
                    .find(|item| item.tools == Some(true) && item.in_memory)
                    .or_else(|| others.iter().find(|item| item.tools == Some(true)))
            });
            let instead = if let Some(able) = able {
                format!("{} on {} can: /model chooses it.", able.name, self.server.name)
            } else if others.is_some() {
                format!(
                    "None of {}'s models can; {} one that can use tools, then choose it with /model.",
                    self.server.name,
                    if self.server.kind == LocalKind::Ollama { "pull" } else { "get" }
                )
            } else {
                "/model chooses one that can.".into()
            };
            self.tell(
                format!("{} can't use tools, so Kumi can talk with it about your Set but can't change anything. {instead}", found.name),
                quiet,
            );
        }
        Ok(found)
    }
    async fn readied(
        &self,
        mut call: CallOptions,
        asked: Shared<LocalBoxFuture<'static, ()>>,
    ) -> Result<(CallOptions, String), LanguageModelError> {
        asked.await;
        let known = self.facts.borrow().clone();
        let known = if let Some(known) = known { known } else { self.learn(call.abort_signal.clone(), false).await? };
        if known.tools == Some(false) {
            if let Some(Message::System { content, .. }) = call.prompt.first_mut() {
                content.push_str("\n\n");
                content.push_str(NO_TOOLS);
            }
            call.tools = None;
            call.tool_choice = None;
        }
        let system = match call.prompt.first() {
            Some(Message::System { content, .. }) => content.len(),
            _ => 0,
        };
        self.fixed.set(system + stringify(&serde_json::to_value(call.tools.as_deref().unwrap_or(&[])).expect("tools serialize")).len());
        if self.server.kind == LocalKind::Ollama {
            self.window.set(Some(self.window.get().unwrap_or(0.).max(context_for(self.fixed.get() as f64, known.most))));
        } else if self.server.kind == LocalKind::Lmstudio {
            self.load_with_room(&known, call.abort_signal.clone()).await?;
        }
        let room = if self.kumi_sets() { self.window.get() } else { known.served.or(self.learned.get()) };
        if let Some(room) = room.filter(|room| too_small(*room, self.fixed.get())) {
            return Err(KumiError::with_provider(FailureKind::Model, self.small(room), &self.server.id).into());
        }
        let model = self.instance.borrow().clone().unwrap_or(known.model);
        Ok((call, model))
    }
    async fn load_with_room(&self, known: &Facts, signal: Option<Signal>) -> Result<(), LanguageModelError> {
        let transport = self.transport(signal);
        let (listed, managed) = lm_studio_list(&self.server, &transport).await?;
        if !managed {
            *self.instance.borrow_mut() = None;
            return Ok(());
        }
        let current = listed.iter().find(|item| item.model == known.model).unwrap_or(known);
        if let Some(copy) =
            current.loaded.iter().find(|copy| copy.context.is_none_or(|context| enough(context, self.fixed.get(), current.most)))
        {
            *self.instance.borrow_mut() = Some(copy.id.clone());
            self.window.set(Some(copy.context.unwrap_or_else(|| context_for(self.fixed.get() as f64, current.most))));
            return Ok(());
        }
        let wanted = context_for(self.fixed.get() as f64, current.most);
        if too_small(wanted, self.fixed.get()) {
            *self.instance.borrow_mut() = Some(known.model.clone());
            self.window.set(Some(wanted));
            return Ok(());
        }
        let origin = origin(&self.server);
        for old in &current.loaded {
            let _ = request_json(
                &format!("{origin}/api/v1/models/unload"),
                &self.server,
                &transport,
                Some(json!({"instance_id":old.id})),
                None,
            )
            .await;
        }
        let loaded = match request_json(
            &format!("{origin}/api/v1/models/load"),
            &self.server,
            &transport,
            Some(json!({"model":known.model,"context_length":wanted,"echo_load_config":true})),
            None,
        )
        .await
        {
            Ok(loaded) => loaded,
            Err(LanguageModelError::ApiCall(error)) if matches!(error.status_code, Some(404 | 405)) => {
                *self.instance.borrow_mut() = None;
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        *self.instance.borrow_mut() = Some(text(&loaded["instance_id"], 300).unwrap_or_else(|| known.model.clone()));
        let window = count(&loaded["load_config"]["context_length"]).unwrap_or(wanted);
        self.window.set(Some(window));
        if let Some(before) = current.loaded.first().and_then(|copy| copy.context) {
            self.tell(
                format!(
                    "LM Studio had {} loaded with room for {} tokens, too few for Kumi; Kumi loaded it again with room for {}.",
                    current.name,
                    tokens(before),
                    tokens(window)
                ),
                false,
            );
        }
        Ok(())
    }
    fn small(&self, room: f64) -> String {
        let need = format!("about {}", tokens(1000_f64.max(number::round(self.fixed.get() as f64 / 4. / 1000.) * 1000.)));
        let facts = self.facts.borrow();
        let name = facts.as_ref().map(|f| f.name.as_str()).unwrap_or(&self.model);
        if self.kumi_sets() {
            return format!("{name} reads at most {} tokens at once, too few for Kumi's instructions and tools ({need}): choose a model that reads more with /model.", tokens(room));
        }
        let target = tokens(context_for(self.fixed.get() as f64, facts.as_ref().and_then(|f| f.most)));
        if self.server.kind == LocalKind::Lmstudio {
            return format!("LM Studio gives {name} room for {} tokens, too few for Kumi's instructions and tools ({need}): load it in LM Studio with a context length of {target} or more, or choose another model with /model.", tokens(room));
        }
        format!("{} gives {name} room for {} tokens, too few for Kumi's instructions and tools ({need}): start it with a context of {target} tokens or more, or choose another model with /model.", self.server.name, tokens(room))
    }
    fn fail(&self, error: LanguageModelError, phase: FailurePhase) -> KumiError {
        let model = self.facts.borrow().as_ref().map(|f| f.name.clone()).unwrap_or_else(|| self.model.clone());
        failure(
            &self.server,
            error,
            phase,
            Some(&model),
            self.window.get(),
            Some(&|room| {
                if !self.kumi_sets() {
                    self.learned.set(Some(room));
                }
                if too_small(room, self.fixed.get()) {
                    return Some(self.small(room));
                }
                self.squeezed.set(self.squeezed.get() * 0.8);
                None
            }),
        )
    }
    fn effort_of(&self) -> Option<Effort> {
        self.effort.filter(|effort| self.facts.borrow().as_ref().is_some_and(|f| f.levels.contains(effort)))
    }
}
struct LocalCompatible {
    state: Rc<LocalState>,
    asked: Shared<LocalBoxFuture<'static, ()>>,
}
#[async_trait(?Send)]
impl LanguageModel for LocalCompatible {
    async fn do_stream(&self, call: CallOptions) -> Result<StreamParts, LanguageModelError> {
        let signal = call.abort_signal.clone();
        let (mut call, model) = self.state.readied(call, self.asked.clone()).await.map_err(|error| {
            if signal.as_ref().is_some_and(Signal::is_cancelled) {
                error
            } else {
                self.state.fail(error, FailurePhase::Request).into()
            }
        })?;
        if let Some(level) = self.state.effort_of() {
            call.provider_options.get_or_insert_with(Map::new).insert(self.state.server.id.clone(), json!({"reasoningEffort":level}));
            crate::core::timing::asked_effort(level.as_str());
        }
        let provider = openai_compatible(CompatibleSettings {
            name: self.state.server.id.clone(),
            model,
            base_url: self.state.server.base_url.clone(),
            api_key: self.state.server.api_key.clone(),
            headers: Default::default(),
            fetch: mending_fetch(self.state.identified.clone()),
            include_usage: true,
        });
        let stream = provider.do_stream(call).await.map_err(|error| {
            if signal.as_ref().is_some_and(Signal::is_cancelled) {
                error
            } else {
                self.state.fail(error, FailurePhase::Request).into()
            }
        })?;
        let state = self.state.clone();
        Ok(Box::pin(stream.map(move |part| match part {
            StreamPart::Error { error } => StreamPart::Error { error: state.fail(error, FailurePhase::Answer).into() },
            other => other,
        })))
    }
}
/// Starts the short capability lookup immediately. Call from the runtime's `LocalSet`.
pub fn resolve_local_model(server: LocalServer, model: String, options: LocalModelOptions) -> LocalBinding {
    let state = Rc::new(LocalState {
        server,
        model,
        identified: Rc::new(Identified(options.fetch.unwrap_or_else(default_fetch))),
        effort: options.effort,
        on_note: options.on_note,
        facts: RefCell::new(None),
        note: RefCell::new(None),
        window: Cell::new(None),
        learned: Cell::new(None),
        fixed: Cell::new(0),
        instance: RefCell::new(None),
        squeezed: Cell::new(1.),
    });
    let learning = state.clone();
    let task = tokio::task::spawn_local(async move {
        let _ = learning.learn(Some(abort::timeout(PROBE_MS)), true).await;
    });
    let asked = async move {
        let _ = task.await;
    }
    .boxed_local()
    .shared();
    let model: Rc<dyn LanguageModel> = if state.server.kind == LocalKind::Ollama {
        let shape_state = state.clone();
        let failure_state = state.clone();
        let shape_asked = asked.clone();
        ollama_chat(OllamaChatSettings {
            base_url: state.server.base_url.clone(),
            model: state.model.clone(),
            fetch: state.identified.clone(),
            failure: Rc::new(move |error, phase| failure_state.fail(error, phase)),
            shape: Rc::new(move |call| {
                let state = shape_state.clone();
                let asked = shape_asked.clone();
                async move {
                    let signal = call.abort_signal.clone();
                    let (call, _) = state.readied(call, asked).await.map_err(|error| {
                        if signal.as_ref().is_some_and(Signal::is_cancelled) {
                            error
                        } else {
                            state.fail(error, FailurePhase::Request).into()
                        }
                    })?;
                    let facts = state.facts.borrow();
                    let facts = facts.as_ref().expect("readied learned facts");
                    let asked = state.effort_of();
                    if let Some(effort) = asked {
                        crate::core::timing::asked_effort(effort.as_str());
                    }
                    let think = asked
                        .map(|effort| json!(effort))
                        .or_else(|| (facts.thinks == Some(true)).then(|| facts.think_default.clone().unwrap_or(json!(true))));
                    Ok(OllamaShape {
                        num_ctx: state.window.get().expect("Ollama context set"),
                        think,
                        options: call,
                        images: facts.vision == Some(true),
                    })
                }
                .boxed_local()
            }),
        })
    } else {
        Rc::new(LocalCompatible { state: state.clone(), asked: asked.clone() })
    };
    let kind = state.server.kind;
    let budget_state = state.clone();
    let binding = ModelBinding {
        id: format!("{}/{}", state.server.id, state.model),
        model,
        prepare: Box::new(move |request: ModelRequest| {
            let mut prompt = vec![Message::System { content: request.instructions, provider_options: None }];
            prompt.extend(if kind == LocalKind::Ollama { request.messages } else { words_only(request.messages) });
            let has_tools = !request.tools.is_empty();
            CallOptions {
                prompt,
                tools: has_tools.then_some(request.tools),
                tool_choice: has_tools.then_some(ToolChoice::Auto),
                ..Default::default()
            }
        }),
        budget: Some(Box::new(move |size| {
            let facts = budget_state.facts.borrow();
            let guessed = || context_for(size as f64, facts.as_ref().and_then(|f| f.most));
            let room = if budget_state.kumi_sets() {
                budget_state.window.get().unwrap_or_else(guessed)
            } else {
                facts.as_ref().and_then(|f| f.served).or(budget_state.learned.get()).unwrap_or_else(guessed)
            };
            budget_for((room * budget_state.squeezed.get()).floor(), size as f64, ANSWER)
        })),
    };
    LocalBinding { binding, asked, state }
}
fn unanswered(error: &LanguageModelError) -> bool {
    if let LanguageModelError::ApiCall(error) = error {
        return error.status_code.is_none();
    }
    static NETWORK: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"^(ECONNREFUSED|ECONNRESET|ENOTFOUND|EHOSTUNREACH|ENETUNREACH|ETIMEDOUT|EAI_AGAIN|EPIPE|UND_ERR_\w+)$").unwrap()
    });
    static FETCH: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"(?i)fetch failed|terminated|other side closed|socket").unwrap());
    // Native reqwest's body read failure is the same transport failure as fetch's TypeError.
    if let LanguageModelError::Other(message) = error {
        return message == "TimeoutError"
            || message == "fetch failed"
            || message.starts_with("error decoding response body")
            || NETWORK.is_match(message);
    }
    if let LanguageModelError::ProviderStream(error) = error {
        let value = Value::Object(error.clone());
        let mut cause = &value;
        for _ in 0..5 {
            if cause["code"].as_str().is_some_and(|s| NETWORK.is_match(s))
                || cause["name"] == "TimeoutError"
                || (cause["name"] == "TypeError" && cause["message"].as_str().is_some_and(|s| FETCH.is_match(s)))
            {
                return true;
            }
            cause = &cause["cause"];
        }
    }
    false
}
fn server_words(error: &LanguageModelError) -> String {
    let raw = error.api_call().and_then(|error| error.response_body.as_deref()).unwrap_or("");
    let mut said = String::new();
    if !raw.is_empty() {
        match serde_json::from_str::<Value>(raw) {
            Ok(body) => {
                said = [&body["error"]["message"], &body["error"], &body["message"], &body["detail"]]
                    .iter()
                    .find_map(|v| v.as_str().filter(|s| !trim(s).is_empty()))
                    .unwrap_or("")
                    .into();
            }
            Err(_) => said = raw.into(),
        }
    }
    if said.is_empty() {
        said = match error {
            LanguageModelError::ProviderStream(error) => error.get("message").and_then(|v| text(v, 1000)).unwrap_or_default(),
            _ => error.to_string(),
        };
    }
    static HTTP: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"^HTTP \d+$").unwrap());
    clean_detail(if HTTP.is_match(&said) { "" } else { &said })
}
fn missing(server: &LocalServer, model: &str) -> String {
    match server.kind {
        LocalKind::Ollama => format!("Ollama doesn't have {model}: run `ollama pull {model}`, or choose one you have with /model."),
        LocalKind::Lmstudio => format!("LM Studio doesn't have {model}: download it in LM Studio, or choose one you have with /model."),
        LocalKind::OpenaiCompatible => format!("{} doesn't serve {model}; choose one it does with /model.", server.name),
    }
}
fn failure(
    server: &LocalServer,
    error: LanguageModelError,
    phase: FailurePhase,
    model: Option<&str>,
    window: Option<f64>,
    outgrown: Option<&dyn Fn(f64) -> Option<String>>,
) -> KumiError {
    if let LanguageModelError::Kumi(error) = error {
        return error;
    }
    let name = &server.name;
    let model = model.unwrap_or("the model");
    let again = if server.r#where == HERE && server.kind != LocalKind::OpenaiCompatible {
        "if it closed, open it again, then send your message again"
    } else {
        "send your message again"
    };
    let fail = |kind, message| KumiError::with_provider(kind, message, &server.id);
    if unanswered(&error) {
        if phase == FailurePhase::Answer {
            return fail(
                FailureKind::Network,
                format!(
                    "{name} stopped answering partway{}: {again}.",
                    if server.kind == LocalKind::Ollama { " (it may have quit, or run out of memory)" } else { "" }
                ),
            );
        }
        if server.r#where != HERE {
            return fail(
                FailureKind::Network,
                format!(
                    "Kumi can't reach {name} at {}: check that it's running {}, then send your message again.",
                    server.base_url, server.r#where
                ),
            );
        }
        if server.kind == LocalKind::Ollama {
            return fail(
                FailureKind::Network,
                "Ollama isn't running: open it, or run `ollama serve`, then send your message again.".into(),
            );
        }
        if server.kind == LocalKind::Lmstudio {
            return fail(FailureKind::Network, "LM Studio's server isn't running: open LM Studio and start it in the Developer tab, or run `lms server start`, then send your message again.".into());
        }
        return fail(
            FailureKind::Network,
            format!("{name} isn't answering at {}: start it, then send your message again.", server.base_url),
        );
    }
    let status = error.api_call().and_then(|e| e.status_code);
    let said = server_words(&error);
    let detail = if said.is_empty() { String::new() } else { format!(" ({said})") };
    if matches!(status, Some(401 | 403)) {
        let fix = match server.kind {
            LocalKind::Lmstudio => "set LM_API_TOKEN to a token from LM Studio's server settings",
            LocalKind::Ollama => "check that OLLAMA_HOST is where Ollama runs",
            LocalKind::OpenaiCompatible => "set its apiKey in ~/.kumi/settings.json",
        };
        return fail(
            FailureKind::Auth,
            format!(
                "{name} didn't accept {} (HTTP {}): {fix}.",
                if server.api_key.is_some() { "the key Kumi has for it" } else { "a request without a key" },
                status.unwrap()
            ),
        );
    }
    static MEMORY: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"(?i)memory|\boom\b|cudamalloc|unable to allocate|failed to allocate|signal: killed").unwrap());
    if MEMORY.is_match(&said) {
        let room = window.map(|n| format!(" with the room Kumi needs ({} tokens)", tokens(n))).unwrap_or_default();
        return fail(FailureKind::Model, format!("{name} couldn't fit {model} in this computer's memory{room}: choose a smaller model with /model, or close other apps and send your message again{detail}."));
    }
    static TOOLS: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"(?i)does not support tools|tools? (?:is |are )?not supported|--jinja").unwrap());
    if TOOLS.is_match(&said) {
        return fail(
            FailureKind::Model,
            if said.contains("--jinja") {
                format!("{name} needs --jinja to use tools: start it with --jinja, or choose another model with /model.")
            } else {
                format!("{model} can't use tools, so Kumi can't change your Set with it: choose another model with /model.")
            },
        );
    }
    static CONTEXT: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"(?i)context (?:length|size|window|limit)|n_ctx|maximum context|too long|too many tokens|tokens to keep|exceeds? (?:the )?(?:available|maximum|context)").unwrap()
    });
    if CONTEXT.is_match(&said) {
        let raw = format!("{} {said}", error.api_call().and_then(|e| e.response_body.as_deref()).unwrap_or(""));
        static REPORTED: LazyLock<Vec<regex::Regex>> = LazyLock::new(|| {
            [
                r#""n_ctx"\s*:\s*(\d+)"#,
                r"(?i)context length of only (\d+)",
                r"(?i)maximum context length is (\d+)",
                r"(?i)context (?:size|length|window)(?: is| of)? (\d+)",
            ]
            .into_iter()
            .map(|r| regex::Regex::new(r).unwrap())
            .collect()
        });
        let room = REPORTED.iter().find_map(|regex| regex.captures(&raw).and_then(|c| c[1].parse::<f64>().ok())).or(window);
        if let Some(fix) = room.filter(|n| *n != 0.).and_then(|n| outgrown.and_then(|f| f(n))) {
            return fail(FailureKind::Model, fix);
        }
        let room = room.filter(|n| *n != 0.).map(|n| format!(" ({} tokens)", tokens(n))).unwrap_or_default();
        return fail(FailureKind::Request, format!("The conversation outgrew the room {name} gives {model}{room}; Kumi keeps it shorter from now on, so send your message again."));
    }
    static MISSING: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"(?i)not found|try pulling|no such model|does not exist|unknown model").unwrap());
    if status == Some(404) || MISSING.is_match(&said) {
        return fail(FailureKind::Model, missing(server, model));
    }
    if phase == FailurePhase::Answer {
        return fail(FailureKind::Network, format!("{name} stopped answering partway{detail}: {again}."));
    }
    if status.is_some_and(|s| s >= 500) {
        return fail(
            FailureKind::Provider,
            format!("{name} had trouble answering (HTTP {}){detail}; send your message again.", status.unwrap()),
        );
    }
    fail(
        FailureKind::Request,
        format!("{name} turned the request down{}{detail}.", status.map(|s| format!(" (HTTP {s})")).unwrap_or_default()),
    )
}
