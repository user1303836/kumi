//! Model lists come from the provider itself.
use super::{
    api_key_for, catalog,
    facts::{with_facts, Listed},
    table::{Format, KeyCheck, KeyHeader, Listing},
    Effort, ProviderId, ProviderRow, SignIn, EFFORTS, USER_AGENT,
};
use crate::{
    auth::{
        openai_codex::{codex_token_source, TokenOptions},
        store::CredentialStore,
    },
    error::LanguageModelError,
    http::{default_fetch, Fetch, FetchInit, Headers},
};
use kumi_common::{
    abort::Signal,
    js::{
        number,
        string::{head, trim},
    },
};
use kumi_types::errors::{FailureKind, KumiError, RuntimeError};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::HashMap, rc::Rc, sync::LazyLock};
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
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
    /// Whether it reasons before it answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<bool>,
    /// What it takes beside words.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<Input>,
    /// The longest answer it gives, in tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<Cost>,
    /// How a request to it is written, where that isn't its provider's own way.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<Format>,
    /// "deprecated", "beta" or "alpha".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// What its provider says of it beside its name (Venice's "private").
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    /// Where its facts come from: its provider's list, models.dev, or a guess from its name (unverified).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facts_from: Option<FactsFrom>,
}
/// What a model takes beside words.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Input {
    Image,
    Audio,
    Video,
    Pdf,
}
impl Input {
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "image" => Self::Image,
            "audio" => Self::Audio,
            "video" => Self::Video,
            "pdf" => Self::Pdf,
            _ => return None,
        })
    }
}
/// What a model costs, in dollars per million tokens.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cost {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write: Option<f64>,
}
/// Where a model's facts come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FactsFrom {
    /// Its provider's own list described it.
    Provider,
    /// models.dev knows it.
    Catalog,
    /// Neither: what its name suggests, unverified.
    Guess,
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
    let name = provider.row().name;
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
/// A model its provider's list names, with nothing else known of it yet.
fn named(provider: ProviderId, model: &str, name: Option<String>) -> ModelInfo {
    ModelInfo {
        id: format!("{provider}/{model}"),
        provider: provider.as_str().into(),
        model: model.into(),
        name: name.unwrap_or_else(|| model.into()),
        ..Default::default()
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
    let row = provider.row();
    if row.sign_in == SignIn::Chatgpt {
        let token = codex_token_source(options.store, TokenOptions { fetch: options.fetch, now: None })();
        let token = token.await?;
        let body = get_json(
            &list_url(row),
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
        let catalog = catalog::current();
        return Ok(rows
            .into_iter()
            .filter_map(|row| {
                let model = row["slug"].as_str().unwrap();
                let mut info = named(provider, model, text(&row["display_name"], 60));
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
                // ChatGPT's list says how hard each model thinks; models.dev, what else it knows.
                with_facts(provider.row(), Listed { info, efforts: true, described: true }, &catalog)
            })
            .collect());
    }
    let key = api_key_for(provider, options.store.as_ref(), options.env.as_ref())
        .await?
        .map(|k| k.key)
        .filter(|k| !k.is_empty())
        .ok_or_else(|| KumiError::with_provider(FailureKind::Auth, format!("Not signed in to {}.", row.name), provider.as_str()))?;
    list_with_key(provider, &key, &transport).await
}
/// Where a provider lists its models.
fn list_url(row: &ProviderRow) -> String {
    format!("{}{}", row.base_url, row.list.0)
}
/// The headers that carry a key to a provider's API.
fn key_headers(row: &ProviderRow, key: &str) -> Headers {
    match row.key_header {
        KeyHeader::Bearer => [("authorization".into(), format!("Bearer {key}"))].into(),
        KeyHeader::Anthropic => [("x-api-key".into(), key.into()), ("anthropic-version".into(), "2023-06-01".into())].into(),
    }
}
async fn list_with_key(provider: ProviderId, key: &str, options: &Transport) -> Result<Vec<ModelInfo>, RuntimeError> {
    let row = provider.row();
    let listing = row.list.1;
    if listing == Listing::Chatgpt {
        return Ok(vec![]);
    }
    let body = get_json(&list_url(row), key_headers(row, key), options, provider).await?;
    let mut rows: Vec<_> = body["data"].as_array().into_iter().flatten().filter(|r| r["id"].is_string()).collect();
    let catalog = catalog::current();
    if listing == Listing::OpenaiChat {
        // OpenAI lists every model it has: models.dev says which chat, and for one it doesn't know, the name decides.
        static EXCLUDED: LazyLock<regex::Regex> =
            LazyLock::new(|| regex::Regex::new("audio|realtime|tts|transcribe|image|search|embedding|instruct|moderation").unwrap());
        rows.retain(|listed| {
            let model = listed["id"].as_str().unwrap();
            row.catalog.and_then(|provider| catalog.model(provider, model)).is_some()
                || (model.starts_with("gpt-") || model.starts_with('o') && model.as_bytes().get(1).is_some_and(u8::is_ascii_digit))
                    && !EXCLUDED.is_match(model)
        });
        rows.sort_by(|a, b| created(&b["created"]).partial_cmp(&created(&a["created"])).unwrap_or(std::cmp::Ordering::Equal));
    }
    Ok(rows
        .into_iter()
        .filter_map(|listed| {
            let anthropic = listing == Listing::Anthropic;
            let info = named(provider, listed["id"].as_str().unwrap(), if anthropic { text(&listed["display_name"], 60) } else { None });
            if !anthropic {
                return with_facts(row, Listed::named(info), &catalog);
            }
            // Anthropic's list says which effort levels each model takes, where it says anything.
            let effort = &listed["capabilities"]["effort"];
            let said = effort["supported"].as_bool();
            let mut listed = Listed { info, efforts: said.is_some(), described: said.is_some() };
            if said == Some(true) {
                listed.info.efforts = levels(&EFFORTS.into_iter().filter(|e| effort[e.as_str()]["supported"] == true).collect::<Vec<_>>());
            }
            with_facts(row, listed, &catalog)
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
    let checked = match provider.row().key_check {
        KeyCheck::List => list_with_key(provider, key, &options).await.map(|_| ()),
    };
    match checked {
        Ok(()) => ApiKeyCheck::Ok,
        Err(RuntimeError::Kumi(error)) if error.kind == FailureKind::Auth => ApiKeyCheck::Refused,
        Err(_) => ApiKeyCheck::Unreachable,
    }
}
