pub mod compat;
pub mod local;
pub mod models;
pub mod ollama;

use crate::{
    ai::{
        anthropic::{anthropic, AnthropicSettings},
        error::LanguageModelError,
        http::{default_fetch, Fetch, FetchInit, Response},
        openai_compatible::{openai_compatible, CompatibleSettings},
        openai_responses::{openai_responses, ResponsesSettings},
        types::*,
    },
    auth::{
        openai_codex::{codex_token_source, TokenOptions, TokenSource},
        store::{Credential, CredentialStore},
    },
    core::errors::{FailureKind, KumiError, RuntimeError},
    kernel::agent::{ModelBinding, ModelRequest},
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::{collections::HashMap, fmt, rc::Rc, sync::LazyLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderId {
    OpenaiCodex,
    Openai,
    Anthropic,
    Opencode,
    OpencodeGo,
}
impl ProviderId {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenaiCodex => "openai-codex",
            Self::Openai => "openai",
            Self::Anthropic => "anthropic",
            Self::Opencode => "opencode",
            Self::OpencodeGo => "opencode-go",
        }
    }
    pub fn parse(value: &str) -> Option<Self> {
        PROVIDERS.iter().copied().find(|provider| provider.as_str() == value)
    }
}
impl fmt::Display for ProviderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
pub const PROVIDERS: [ProviderId; 5] =
    [ProviderId::OpenaiCodex, ProviderId::Openai, ProviderId::Anthropic, ProviderId::Opencode, ProviderId::OpencodeGo];
pub const API_KEY_ENV: [(ProviderId, &str); 4] = [
    (ProviderId::Openai, "OPENAI_API_KEY"),
    (ProviderId::Anthropic, "ANTHROPIC_API_KEY"),
    (ProviderId::Opencode, "OPENCODE_API_KEY"),
    (ProviderId::OpencodeGo, "OPENCODE_API_KEY"),
];
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Effort {
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}
impl Effort {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        }
    }
    pub fn parse(value: &str) -> Option<Self> {
        EFFORTS.iter().copied().find(|e| e.as_str() == value)
    }
}
pub const EFFORTS: [Effort; 5] = [Effort::Low, Effort::Medium, Effort::High, Effort::Xhigh, Effort::Max];
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SignIn {
    Chatgpt,
    ApiKey,
}
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInfo {
    pub id: ProviderId,
    pub name: &'static str,
    pub sign_in: SignIn,
    pub credential: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_env: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_page: Option<&'static str>,
}
pub const PROVIDER_INFO: [ProviderInfo; 5] = [
    ProviderInfo {
        id: ProviderId::OpenaiCodex,
        name: "ChatGPT",
        sign_in: SignIn::Chatgpt,
        credential: "openai-codex",
        key_env: None,
        key_page: None,
    },
    ProviderInfo {
        id: ProviderId::Anthropic,
        name: "Anthropic",
        sign_in: SignIn::ApiKey,
        credential: "anthropic",
        key_env: Some("ANTHROPIC_API_KEY"),
        key_page: Some("platform.claude.com (API keys)"),
    },
    ProviderInfo {
        id: ProviderId::Openai,
        name: "OpenAI API",
        sign_in: SignIn::ApiKey,
        credential: "openai",
        key_env: Some("OPENAI_API_KEY"),
        key_page: Some("platform.openai.com/api-keys"),
    },
    ProviderInfo {
        id: ProviderId::Opencode,
        name: "OpenCode Zen",
        sign_in: SignIn::ApiKey,
        credential: "opencode",
        key_env: Some("OPENCODE_API_KEY"),
        key_page: Some("opencode.ai/auth"),
    },
    ProviderInfo {
        id: ProviderId::OpencodeGo,
        name: "OpenCode Go",
        sign_in: SignIn::ApiKey,
        credential: "opencode",
        key_env: Some("OPENCODE_API_KEY"),
        key_page: Some("opencode.ai/auth"),
    },
];
pub fn provider_info(provider: ProviderId) -> &'static ProviderInfo {
    PROVIDER_INFO.iter().find(|info| info.id == provider).unwrap()
}
pub static USER_AGENT: LazyLock<String> = LazyLock::new(|| {
    let platform = match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "x64",
        "x86" => "ia32",
        other => other,
    };
    #[cfg(unix)]
    let release = {
        let mut value = std::mem::MaybeUninit::<libc::utsname>::zeroed();
        // uname writes the entire utsname on success, with a NUL-terminated release field.
        unsafe {
            if libc::uname(value.as_mut_ptr()) == 0 {
                std::ffi::CStr::from_ptr(value.assume_init().release.as_ptr()).to_string_lossy().into_owned()
            } else {
                String::new()
            }
        }
    };
    #[cfg(windows)]
    let release = {
        #[repr(C)]
        struct Version {
            size: u32,
            major: u32,
            minor: u32,
            build: u32,
            platform: u32,
            service_pack: [u16; 128],
        }
        #[link(name = "ntdll")]
        unsafe extern "system" {
            fn RtlGetVersion(version: *mut Version) -> i32;
        }
        let mut version =
            Version { size: std::mem::size_of::<Version>() as u32, major: 0, minor: 0, build: 0, platform: 0, service_pack: [0; 128] };
        // RtlGetVersion reports the actual OS version independently of application manifests.
        if unsafe { RtlGetVersion(&mut version) } == 0 {
            format!("{}.{}.{}", version.major, version.minor, version.build)
        } else {
            String::new()
        }
    };
    #[cfg(not(any(unix, windows)))]
    let release = String::new();
    format!("kumi/{} ({platform} {release}; {arch})", crate::version::KUMI_VERSION)
});
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeySource {
    Env,
    Saved,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiKey {
    pub key: String,
    pub source: KeySource,
}
pub async fn api_key_for(
    provider: ProviderId,
    store: &dyn CredentialStore,
    env: Option<&HashMap<String, String>>,
) -> Result<Option<ApiKey>, RuntimeError> {
    let info = provider_info(provider);
    if info.sign_in != SignIn::ApiKey {
        return Ok(None);
    }
    if let Some(Credential::ApiKey { key }) = store.get(info.credential).await? {
        return Ok(Some(ApiKey { key, source: KeySource::Saved }));
    }
    Ok(info
        .key_env
        .and_then(|name| env.and_then(|env| env.get(name)))
        .filter(|key| !key.is_empty())
        .map(|key| ApiKey { key: key.clone(), source: KeySource::Env }))
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedModelId {
    pub provider: ProviderId,
    pub model: String,
}
pub fn parse_model_id(value: &str) -> Option<ParsedModelId> {
    static MODEL_ID: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"^(openai-codex|openai|anthropic|opencode|opencode-go)/([a-zA-Z0-9][a-zA-Z0-9._:-]{0,127})$").unwrap()
    });
    let matched = MODEL_ID.captures(value)?;
    Some(ParsedModelId { provider: ProviderId::parse(&matched[1])?, model: matched[2].into() })
}
pub struct ResolveModelOptions {
    pub model: String,
    pub store: Rc<dyn CredentialStore>,
    pub env: Option<HashMap<String, String>>,
    pub fetch: Option<Rc<dyn Fetch>>,
    pub effort: Option<Effort>,
    /// A faster tier the model's list offers ("priority"), asked for on every request.
    pub service_tier: Option<String>,
}
struct IdentifiedFetch(Rc<dyn Fetch>);
#[async_trait(?Send)]
impl Fetch for IdentifiedFetch {
    async fn fetch(&self, url: &str, mut init: FetchInit) -> Result<Response, LanguageModelError> {
        let agent = init.headers.get("user-agent").cloned().unwrap_or_default();
        init.headers.insert("user-agent".into(), format!("{} {agent}", *USER_AGENT).trim().into());
        self.0.fetch(url, init).await
    }
}
struct CodexFetch {
    fetch: Rc<dyn Fetch>,
    token: TokenSource,
}
#[async_trait(?Send)]
impl Fetch for CodexFetch {
    async fn fetch(&self, url: &str, mut init: FetchInit) -> Result<Response, LanguageModelError> {
        let token = (self.token)().await.map_err(|e| match e {
            RuntimeError::Kumi(e) => LanguageModelError::Kumi(e),
            e => LanguageModelError::other(e.to_string()),
        })?;
        init.headers.insert("authorization".into(), format!("Bearer {}", token.access));
        init.headers.insert("chatgpt-account-id".into(), token.account_id);
        init.headers.insert("originator".into(), "kumi".into());
        init.headers.insert("openai-beta".into(), "responses=experimental".into());
        self.fetch.fetch(url, init).await
    }
}
pub async fn resolve_model(options: ResolveModelOptions) -> Result<ModelBinding, RuntimeError> {
    let parsed = parse_model_id(&options.model).ok_or_else(|| {
        KumiError::new(
            FailureKind::Config,
            format!(
                "KUMI_MODEL must be <provider>/<model> with provider one of {}.",
                PROVIDERS.iter().map(|p| p.as_str()).collect::<Vec<_>>().join(", ")
            ),
        )
    })?;
    let provider = parsed.provider;
    let model = parsed.model;
    let base = options.fetch.unwrap_or_else(default_fetch);
    let identified: Rc<dyn Fetch> = Rc::new(IdentifiedFetch(base.clone()));
    let effort = options.effort;
    let service_tier = options.service_tier.clone();
    let bind = |model, prepare| ModelBinding { id: options.model.clone(), model, prepare, budget: None };
    if provider == ProviderId::OpenaiCodex {
        let token = codex_token_source(options.store, TokenOptions { fetch: Some(base), now: None });
        token().await?;
        let model = openai_responses(ResponsesSettings {
            model,
            base_url: "https://chatgpt.com/backend-api/codex".into(),
            api_key: "kumi-oauth".into(),
            headers: Default::default(),
            fetch: Rc::new(CodexFetch { fetch: identified, token }),
        });
        return Ok(bind(
            model,
            Box::new(move |request: ModelRequest| {
                let session = request.session_id.clone();
                let mut extra = Map::from_iter([("textVerbosity".into(), json!("low"))]);
                if let Some(effort) = effort {
                    extra.insert("reasoningEffort".into(), json!(effort));
                }
                if let Some(tier) = &service_tier {
                    extra.insert("serviceTier".into(), json!(tier));
                }
                let mut options = responses_request(request, extra);
                options.headers = Some([(String::from("session-id"), session)].into());
                options
            }),
        ));
    }
    let info = provider_info(provider);
    let key =
        api_key_for(provider, options.store.as_ref(), options.env.as_ref()).await?.map(|k| k.key).filter(|k| !k.is_empty()).ok_or_else(
            || {
                KumiError::with_provider(
                    FailureKind::Auth,
                    format!("Not signed in to {}: add its API key with /login (or set {}).", info.name, info.key_env.unwrap_or("")),
                    provider.as_str(),
                )
            },
        )?;
    let openai = provider == ProviderId::Openai
        || matches!(provider, ProviderId::Opencode | ProviderId::OpencodeGo)
            && ["gpt-", "grok-", "muse-"].iter().any(|prefix| model.starts_with(prefix));
    let claude = provider == ProviderId::Anthropic
        || matches!(provider, ProviderId::Opencode | ProviderId::OpencodeGo) && model.starts_with("claude-");
    let gateway = matches!(provider, ProviderId::Opencode | ProviderId::OpencodeGo);
    let base_url = match provider {
        ProviderId::Openai => "https://api.openai.com/v1",
        ProviderId::Anthropic => "https://api.anthropic.com/v1",
        ProviderId::Opencode => "https://opencode.ai/zen/v1",
        ProviderId::OpencodeGo => "https://opencode.ai/zen/go/v1",
        ProviderId::OpenaiCodex => unreachable!(),
    }
    .to_string();
    if openai {
        let model = openai_responses(ResponsesSettings { model, base_url, api_key: key, headers: Default::default(), fetch: identified });
        return Ok(bind(
            model,
            Box::new(move |request: ModelRequest| {
                let session = request.session_id.clone();
                let extra = effort.map(|e| Map::from_iter([("reasoningEffort".into(), json!(e))])).unwrap_or_default();
                let mut options = responses_request(request, extra);
                if gateway {
                    options.headers = Some([(String::from("x-opencode-session"), session)].into());
                }
                options
            }),
        ));
    }
    if claude {
        let model = anthropic(AnthropicSettings {
            model,
            base_url,
            api_key: (!gateway).then(|| key.clone()),
            auth_token: gateway.then_some(key),
            headers: Default::default(),
            fetch: identified,
        });
        return Ok(bind(
            model,
            Box::new(move |request: ModelRequest| {
                let session = request.session_id.clone();
                let mut options = anthropic_request(request, effort);
                if gateway {
                    options.headers = Some([(String::from("x-opencode-session"), session)].into());
                }
                options
            }),
        ));
    }
    if model.starts_with("gemini-") {
        return Err(KumiError::new(FailureKind::Config, "Gemini models through OpenCode are not supported yet.").into());
    }
    let model = openai_compatible(CompatibleSettings {
        name: provider.as_str().into(),
        model,
        base_url,
        api_key: Some(key),
        headers: Default::default(),
        fetch: identified,
        include_usage: true,
    });
    Ok(bind(
        model,
        Box::new(|request: ModelRequest| {
            let mut prompt = vec![Message::System { content: request.instructions, provider_options: None }];
            prompt.extend(words_only(request.messages));
            let mut options = CallOptions {
                prompt,
                headers: Some([(String::from("x-opencode-session"), request.session_id)].into()),
                ..Default::default()
            };
            tool_options(&mut options, request.tools);
            options
        }),
    ))
}
fn tool_options(options: &mut CallOptions, tools: Vec<FunctionTool>) {
    if !tools.is_empty() {
        options.tools = Some(tools);
        options.tool_choice = Some(ToolChoice::Auto);
    }
}
fn map_images(mut messages: Vec<Message>, change: impl Fn(&mut ToolResultOutput)) -> Vec<Message> {
    for message in &mut messages {
        if let Message::Tool { content, .. } = message {
            for part in content {
                if let ToolPart::ToolResult(result) = part {
                    if matches!(result.output, ToolResultOutput::Content { .. }) {
                        change(&mut result.output);
                    }
                }
            }
        }
    }
    messages
}
pub fn words_only(messages: Vec<Message>) -> Vec<Message> {
    map_images(messages, |output| {
        if let ToolResultOutput::Content { value, .. } = output {
            let words = value
                .iter()
                .filter_map(|item| match item {
                    ToolResultContentItem::Text { text, .. } if !text.is_empty() => Some(text.as_str()),
                    ToolResultContentItem::File { .. } => Some("[An image this model can't be shown.]"),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            *output = ToolResultOutput::text(words);
        }
    })
}
fn in_detail(messages: Vec<Message>) -> Vec<Message> {
    map_images(messages, |output| {
        if let ToolResultOutput::Content { value, .. } = output {
            for item in value {
                if let ToolResultContentItem::File { provider_options, .. } = item {
                    let options = provider_options.get_or_insert_with(Map::new);
                    let openai = options.entry("openai".to_string()).or_insert_with(|| json!({}));
                    if !openai.is_object() {
                        *openai = json!({});
                    }
                    openai["imageDetail"] = json!("high");
                }
            }
        }
    })
}
fn responses_request(request: ModelRequest, extra: Map<String, Value>) -> CallOptions {
    let mut openai = json!({"instructions":request.instructions,"store":false,"promptCacheKey":request.session_id});
    openai.as_object_mut().unwrap().extend(extra);
    let mut options = CallOptions {
        prompt: in_detail(request.messages),
        provider_options: Some(Map::from_iter([("openai".into(), openai)])),
        ..Default::default()
    };
    tool_options(&mut options, request.tools);
    options
}
fn anthropic_request(request: ModelRequest, effort: Option<Effort>) -> CallOptions {
    let cache = Map::from_iter([("anthropic".into(), json!({"cacheControl":{"type":"ephemeral"}}))]);
    let mut messages = request.messages;
    if let Some(last) = messages.last_mut() {
        last.provider_options_mut().get_or_insert_with(Map::new).extend(cache.clone());
    }
    let mut prompt = vec![Message::System { content: request.instructions, provider_options: Some(cache) }];
    prompt.extend(messages);
    let mut options = CallOptions {
        prompt,
        provider_options: effort.map(|e| Map::from_iter([("anthropic".into(), json!({"effort":e}))])),
        ..Default::default()
    };
    tool_options(&mut options, request.tools);
    options
}
