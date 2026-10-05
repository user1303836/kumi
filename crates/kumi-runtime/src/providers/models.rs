//! Port of `packages/runtime/src/providers/models.ts`: model lists come from the provider itself.
use super::{api_key_for, provider_info, Effort, ProviderId, EFFORTS, USER_AGENT};
use crate::{
    ai::{
        error::LanguageModelError,
        http::{default_fetch, Fetch, FetchInit, Headers},
    },
    auth::{
        openai_codex::{codex_token_source, TokenOptions},
        store::CredentialStore,
    },
    core::errors::{FailureKind, KumiError, RuntimeError},
};
use kumi_common::{
    abort::Signal,
    js::{
        number,
        string::{head, trim},
    },
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::HashMap, rc::Rc, sync::LazyLock};
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub id: String,
    pub provider: String,
    pub model: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub efforts: Vec<EffortInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_effort: Option<Effort>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loaded: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r#where: Option<String>,
    /// Faster processing the provider offers for this model (ChatGPT's "Fast"), as its list names it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub service_tiers: Vec<ServiceTier>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceTier {
    /// What a request asks for ("priority").
    pub id: String,
    /// What the provider calls it ("Fast").
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffortInfo {
    pub effort: Effort,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}
pub struct ListOptions {
    pub store: Rc<dyn CredentialStore>,
    pub env: Option<HashMap<String, String>>,
    pub fetch: Option<Rc<dyn Fetch>>,
    pub signal: Option<Signal>,
}
#[derive(Default, Clone)]
pub struct Transport {
    pub fetch: Option<Rc<dyn Fetch>>,
    pub signal: Option<Signal>,
}
fn error(error: LanguageModelError) -> RuntimeError {
    match error {
        LanguageModelError::Kumi(e) => e.into(),
        e => RuntimeError::plain(e.to_string()),
    }
}
async fn get_json(url: &str, mut headers: Headers, options: &Transport, provider: ProviderId) -> Result<Value, RuntimeError> {
    headers.entry("user-agent".into()).or_insert_with(|| USER_AGENT.clone());
    let response = options
        .fetch
        .clone()
        .unwrap_or_else(default_fetch)
        .fetch(url, FetchInit { headers, signal: options.signal.clone(), ..Default::default() })
        .await
        .map_err(error)?;
    let name = provider_info(provider).name;
    if matches!(response.status, 401 | 403) {
        return Err(KumiError::with_provider(
            FailureKind::Auth,
            format!("{name} didn't accept this sign-in (HTTP {}).", response.status),
            provider.as_str(),
        )
        .into());
    }
    if !response.ok() {
        return Err(KumiError::with_provider(
            FailureKind::Provider,
            format!("{name} couldn't list its models (HTTP {}).", response.status),
            provider.as_str(),
        )
        .into());
    }
    response.json().await.map_err(error)
}
fn text(value: &Value, max: usize) -> Option<String> {
    value.as_str().map(trim).filter(|v| !v.is_empty()).map(|v| head(v, max))
}
fn levels(efforts: &[Effort]) -> Vec<EffortInfo> {
    efforts.iter().map(|effort| EffortInfo { effort: *effort, description: None }).collect()
}
fn from_id(provider: ProviderId, model: &str, name: Option<String>) -> ModelInfo {
    static GPT: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"^gpt-(\d+)").unwrap());
    let gpt = GPT.captures(model).and_then(|m| m[1].parse::<u32>().ok()).unwrap_or(0);
    let o = model.starts_with('o') && model.as_bytes().get(1).is_some_and(u8::is_ascii_digit);
    let efforts = if model.starts_with("claude-haiku") {
        vec![]
    } else if model.starts_with("claude-") || gpt >= 6 {
        levels(&EFFORTS)
    } else if gpt >= 5 || o {
        levels(&[Effort::Low, Effort::Medium, Effort::High])
    } else {
        vec![]
    };
    ModelInfo {
        id: format!("{provider}/{model}"),
        provider: provider.as_str().into(),
        model: model.into(),
        name: name.unwrap_or_else(|| model.into()),
        description: None,
        efforts,
        default_effort: None,
        tools: None,
        context: None,
        loaded: None,
        r#where: None,
        service_tiers: Vec::new(),
    }
}
fn rank(value: &Value) -> f64 {
    value
        .as_f64()
        .or_else(|| value.as_str().filter(|s| !trim(s).is_empty()).and_then(number::parse))
        .filter(|n| n.is_finite())
        .unwrap_or(999.)
}
fn array_text(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Array(a) => a.iter().map(array_text).collect::<Vec<_>>().join(","),
        Value::Object(_) => "[object Object]".into(),
    }
}
fn created(value: &Value) -> f64 {
    match value {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => number::parse(s),
        Value::Bool(b) => Some(if *b { 1. } else { 0. }),
        Value::Null => Some(0.),
        Value::Array(_) => number::parse(&array_text(value)),
        Value::Object(_) => None,
    }
    .filter(|n| !n.is_nan())
    .unwrap_or(0.)
}
pub async fn list_models(provider: ProviderId, options: ListOptions) -> Result<Vec<ModelInfo>, RuntimeError> {
    let transport = Transport { fetch: options.fetch.clone(), signal: options.signal };
    if provider == ProviderId::OpenaiCodex {
        let token = codex_token_source(options.store, TokenOptions { fetch: options.fetch, now: None })();
        let token = token.await?;
        let body = get_json(
            "https://chatgpt.com/backend-api/codex/models?client_version=1.0.0",
            [
                ("authorization".into(), format!("Bearer {}", token.access)),
                ("chatgpt-account-id".into(), token.account_id),
                ("originator".into(), "kumi".into()),
            ]
            .into(),
            &transport,
            provider,
        )
        .await?;
        let mut rows: Vec<_> =
            body["models"].as_array().into_iter().flatten().filter(|r| r["slug"].is_string() && r["visibility"] != "hide").collect();
        rows.sort_by(|a, b| rank(&a["priority"]).partial_cmp(&rank(&b["priority"])).unwrap());
        return Ok(rows
            .into_iter()
            .map(|row| {
                let model = row["slug"].as_str().unwrap();
                let mut info = from_id(provider, model, text(&row["display_name"], 60));
                info.description = text(&row["description"], 200);
                info.efforts = row["supported_reasoning_levels"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|level| {
                        Some(EffortInfo {
                            effort: Effort::parse(level["effort"].as_str()?)?,
                            description: text(&level["description"], 200),
                        })
                    })
                    .collect();
                info.default_effort = row["default_reasoning_level"].as_str().and_then(Effort::parse);
                info.service_tiers = row["service_tiers"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|tier| {
                        let id =
                            tier["id"].as_str().filter(|id| (1..=40).contains(&id.len()) && id.bytes().all(|c| c.is_ascii_graphic()))?;
                        Some(ServiceTier { id: id.into(), name: text(&tier["name"], 40)?, description: text(&tier["description"], 120) })
                    })
                    .collect();
                info
            })
            .collect());
    }
    let key = api_key_for(provider, options.store.as_ref(), options.env.as_ref())
        .await?
        .map(|k| k.key)
        .filter(|k| !k.is_empty())
        .ok_or_else(|| {
            KumiError::with_provider(FailureKind::Auth, format!("Not signed in to {}.", provider_info(provider).name), provider.as_str())
        })?;
    list_with_key(provider, &key, &transport).await
}
async fn list_with_key(provider: ProviderId, key: &str, options: &Transport) -> Result<Vec<ModelInfo>, RuntimeError> {
    let (url, headers) = match provider {
        ProviderId::Anthropic => (
            "https://api.anthropic.com/v1/models?limit=100",
            [("x-api-key".into(), key.into()), ("anthropic-version".into(), "2023-06-01".into())].into(),
        ),
        ProviderId::Openai => ("https://api.openai.com/v1/models", [("authorization".into(), format!("Bearer {key}"))].into()),
        ProviderId::Opencode => ("https://opencode.ai/zen/v1/models", [("authorization".into(), format!("Bearer {key}"))].into()),
        ProviderId::OpencodeGo => ("https://opencode.ai/zen/go/v1/models", [("authorization".into(), format!("Bearer {key}"))].into()),
        ProviderId::OpenaiCodex => return Ok(vec![]),
    };
    let body = get_json(url, headers, options, provider).await?;
    let mut rows: Vec<_> = body["data"].as_array().into_iter().flatten().filter(|r| r["id"].is_string()).collect();
    if provider == ProviderId::Openai {
        static EXCLUDED: LazyLock<regex::Regex> =
            LazyLock::new(|| regex::Regex::new("audio|realtime|tts|transcribe|image|search|embedding|instruct|moderation").unwrap());
        rows.retain(|row| {
            let model = row["id"].as_str().unwrap();
            (model.starts_with("gpt-") || model.starts_with('o') && model.as_bytes().get(1).is_some_and(u8::is_ascii_digit))
                && !EXCLUDED.is_match(model)
        });
        rows.sort_by(|a, b| created(&b["created"]).partial_cmp(&created(&a["created"])).unwrap_or(std::cmp::Ordering::Equal));
    } else if matches!(provider, ProviderId::Opencode | ProviderId::OpencodeGo) {
        rows.retain(|row| !row["id"].as_str().unwrap().starts_with("gemini-"));
    }
    Ok(rows
        .into_iter()
        .map(|row| {
            let mut info = from_id(
                provider,
                row["id"].as_str().unwrap(),
                if provider == ProviderId::Anthropic { text(&row["display_name"], 60) } else { None },
            );
            if provider == ProviderId::Anthropic {
                let effort = &row["capabilities"]["effort"];
                if effort["supported"] == false {
                    info.efforts.clear();
                } else if effort["supported"] == true {
                    info.efforts = levels(&EFFORTS.into_iter().filter(|e| effort[e.as_str()]["supported"] == true).collect::<Vec<_>>());
                }
            }
            info
        })
        .collect())
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApiKeyCheck {
    Ok,
    Refused,
    Unreachable,
}
pub async fn check_api_key(provider: ProviderId, key: &str, options: Transport) -> ApiKeyCheck {
    match list_with_key(provider, key, &options).await {
        Ok(_) => ApiKeyCheck::Ok,
        Err(RuntimeError::Kumi(error)) if error.kind == FailureKind::Auth => ApiKeyCheck::Refused,
        Err(_) => ApiKeyCheck::Unreachable,
    }
}
