pub mod compat;
pub mod local;
pub mod models;
pub mod ollama;
pub mod table;
pub mod usage;

use crate::{
    anthropic::{anthropic, AnthropicSettings},
    auth::{
        openai_codex::{codex_token_source, TokenOptions, TokenSource},
        store::{Credential, CredentialStore},
    },
    binding::{ModelBinding, ModelRequest},
    error::LanguageModelError,
    http::{default_fetch, Fetch, FetchInit, Response},
    openai_compatible::{openai_compatible, CompatibleSettings},
    openai_responses::{openai_responses, ResponsesSettings},
    types::*,
};
use async_trait::async_trait;
use kumi_common::js::string::{trim, utf16_len};
use kumi_types::errors::{FailureKind, KumiError, RuntimeError};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::{collections::HashMap, rc::Rc, sync::LazyLock};
pub use table::{Format, KeyHeader, ProviderId, ProviderRow, SignIn};

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
    format!("kumi/{} ({platform} {release}; {arch})", kumi_types::version::KUMI_VERSION)
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
    let row = provider.row();
    if row.sign_in != SignIn::ApiKey {
        return Ok(None);
    }
    if let Some(Credential::ApiKey { key }) = store.get(row.credential).await? {
        return Ok(Some(ApiKey { key, source: KeySource::Saved }));
    }
    Ok(row
        .key_env
        .iter()
        .find_map(|name| env.and_then(|env| env.get(*name)).filter(|key| !key.is_empty()))
        .map(|key| ApiKey { key: key.clone(), source: KeySource::Env }))
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedModelId {
    pub provider: ProviderId,
    pub model: String,
}
/// A model ID, `<provider>/<model>`, split at its first `/`: a provider's ID as Kumi makes them (`[a-z0-9][a-z0-9-]{0,39}`)
/// and the model as its provider names it, `/` and all, in at most 256 UTF-16 units, with no control characters and no
/// spaces at either end. Cloud providers and model servers alike.
pub fn split_model_id(value: &str) -> Option<(&str, &str)> {
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
    Some((id, model))
}
pub fn parse_model_id(value: &str) -> Option<ParsedModelId> {
    let (provider, model) = split_model_id(value)?;
    Some(ParsedModelId { provider: ProviderId::parse(provider)?, model: model.into() })
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
/// Every cloud request: says it's Kumi, and keeps what the answer's headers say of the producer's plan.
struct IdentifiedFetch {
    fetch: Rc<dyn Fetch>,
    provider: ProviderId,
}
#[async_trait(?Send)]
impl Fetch for IdentifiedFetch {
    async fn fetch(&self, url: &str, mut init: FetchInit) -> Result<Response, LanguageModelError> {
        let agent = init.headers.get("user-agent").cloned().unwrap_or_default();
        init.headers.insert("user-agent".into(), format!("{} {agent}", *USER_AGENT).trim().into());
        let response = self.fetch.fetch(url, init).await?;
        usage::record(self.provider.as_str(), &response.headers);
        Ok(response)
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
/// The headers a provider's requests carry beside the format's own: the conversation's ID, where it takes one.
fn session_headers(header: Option<&'static str>, session: String) -> Option<crate::http::Headers> {
    header.map(|header| [(String::from(header), session)].into())
}
pub async fn resolve_model(options: ResolveModelOptions) -> Result<ModelBinding, RuntimeError> {
    let parsed = parse_model_id(&options.model).ok_or_else(|| {
        KumiError::new(
            FailureKind::Config,
            format!(
                "KUMI_MODEL must be <provider>/<model> with provider one of {}.",
                table::rows().map(|row| row.id).collect::<Vec<_>>().join(", ")
            ),
        )
    })?;
    let provider = parsed.provider;
    let model = parsed.model;
    let row = provider.row();
    let base = options.fetch.unwrap_or_else(default_fetch);
    let identified: Rc<dyn Fetch> = Rc::new(IdentifiedFetch { fetch: base.clone(), provider });
    let (key, fetch): (String, Rc<dyn Fetch>) = match row.sign_in {
        SignIn::Chatgpt => {
            let token = codex_token_source(options.store, TokenOptions { fetch: Some(base), now: None });
            token().await?;
            ("kumi-oauth".into(), Rc::new(CodexFetch { fetch: identified, token }))
        }
        SignIn::ApiKey => {
            let key = api_key_for(provider, options.store.as_ref(), options.env.as_ref())
                .await?
                .map(|k| k.key)
                .filter(|k| !k.is_empty())
                .ok_or_else(|| {
                KumiError::with_provider(
                    FailureKind::Auth,
                    format!("Not signed in to {}: add its API key with /login (or set {}).", row.name, row.first_key_env().unwrap_or("")),
                    provider.as_str(),
                )
            })?;
            (key, identified)
        }
    };
    let format = row.format_for(&model).map_err(|why| KumiError::new(FailureKind::Config, why))?;
    let effort = options.effort;
    let extras = row.extras;
    let service_tier = options.service_tier.clone().filter(|_| extras.service_tier);
    let bind = |model, prepare| ModelBinding { id: options.model.clone(), model, prepare, budget: None };
    let base_url = row.base_url.to_string();
    Ok(match format {
        Format::Responses => bind(
            openai_responses(ResponsesSettings { model, base_url, api_key: key, headers: Default::default(), fetch }),
            Box::new(move |request: ModelRequest| {
                let session = request.session_id.clone();
                let mut extra = Map::new();
                if let Some(verbosity) = extras.verbosity {
                    extra.insert("textVerbosity".into(), json!(verbosity));
                }
                if let Some(effort) = effort {
                    extra.insert("reasoningEffort".into(), json!(effort));
                }
                if let Some(tier) = &service_tier {
                    extra.insert("serviceTier".into(), json!(tier));
                }
                let mut options = responses_request(request, extra);
                options.headers = session_headers(extras.session_header, session);
                options
            }),
        ),
        Format::Messages => {
            let bearer = row.key_header == KeyHeader::Bearer;
            bind(
                anthropic(AnthropicSettings {
                    model,
                    base_url,
                    api_key: (!bearer).then(|| key.clone()),
                    auth_token: bearer.then_some(key),
                    headers: Default::default(),
                    fetch,
                }),
                Box::new(move |request: ModelRequest| {
                    let session = request.session_id.clone();
                    let mut options = anthropic_request(request, effort);
                    options.headers = session_headers(extras.session_header, session);
                    options
                }),
            )
        }
        Format::ChatCompletions => bind(
            openai_compatible(CompatibleSettings {
                name: provider.as_str().into(),
                model,
                base_url,
                api_key: Some(key),
                headers: Default::default(),
                fetch,
                include_usage: true,
            }),
            Box::new(move |request: ModelRequest| {
                let mut prompt = vec![Message::System { content: request.instructions, provider_options: None }];
                prompt.extend(words_only(request.messages));
                let mut options =
                    CallOptions { prompt, headers: session_headers(extras.session_header, request.session_id), ..Default::default() };
                tool_options(&mut options, request.tools);
                options
            }),
        ),
    })
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
    // The tools and instructions stay cached for an hour, through a long render or listen; the conversation, which
    // changes each call, for the default five minutes. Anthropic takes the longer one first.
    let lasting = Map::from_iter([("anthropic".into(), json!({"cacheControl":{"type":"ephemeral","ttl":"1h"}}))]);
    let cache = Map::from_iter([("anthropic".into(), json!({"cacheControl":{"type":"ephemeral"}}))]);
    let mut messages = request.messages;
    if let Some(last) = messages.last_mut() {
        last.provider_options_mut().get_or_insert_with(Map::new).extend(cache);
    }
    let mut prompt = vec![Message::System { content: request.instructions, provider_options: Some(lasting) }];
    prompt.extend(messages);
    let mut options = CallOptions {
        prompt,
        provider_options: effort.map(|e| Map::from_iter([("anthropic".into(), json!({"effort":e}))])),
        ..Default::default()
    };
    tool_options(&mut options, request.tools);
    options
}
