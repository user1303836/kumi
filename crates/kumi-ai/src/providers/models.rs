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
    let status = response.status;
    let refused = || -> RuntimeError {
        KumiError::with_provider(FailureKind::Auth, format!("{name} didn't accept this sign-in (HTTP {status})."), provider.as_str()).into()
    };
    if matches!(status, 401 | 403) {
        return Err(refused());
    }
    if !response.ok() {
        // Gemini turns down a key it doesn't know with a 400 that says so.
        if status == 400 && response.text().await.is_ok_and(|body| body.contains("API_KEY_INVALID") || body.contains("API key not valid")) {
            return Err(refused());
        }
        return Err(KumiError::with_provider(
            FailureKind::Provider,
            format!("{name} couldn't list its models (HTTP {status})."),
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
/// The headers that carry a key to a provider's API, with those it wants on every request.
fn key_headers(row: &ProviderRow, key: &str) -> Headers {
    let mut headers: Headers = match row.key_header {
        KeyHeader::Bearer => [("authorization".into(), format!("Bearer {key}"))].into(),
        KeyHeader::Anthropic => [("x-api-key".into(), key.into()), ("anthropic-version".into(), "2023-06-01".into())].into(),
    };
    headers.extend(row.extras.headers.iter().map(|(name, value)| (name.to_string(), value.to_string())));
    headers
}
async fn list_with_key(provider: ProviderId, key: &str, options: &Transport) -> Result<Vec<ModelInfo>, RuntimeError> {
    let row = provider.row();
    let listing = row.list.1;
    if listing == Listing::Chatgpt {
        return Ok(vec![]);
    }
    let body = get_json(&list_url(row), key_headers(row, key), options, provider).await?;
    // OpenAI's shape (`data`), or the list alone, as some providers answer it.
    let mut rows: Vec<_> = body["data"].as_array().or(body.as_array()).into_iter().flatten().filter(|r| r["id"].is_string()).collect();
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
    if matches!(listing, Listing::OpenRouter | Listing::Venice | Listing::HuggingFace) {
        return Ok(rows
            .into_iter()
            .filter_map(|listed| {
                let described = match listing {
                    Listing::OpenRouter => openrouter_model(provider, listed)?,
                    Listing::Venice => venice_model(provider, listed)?,
                    _ => huggingface_model(provider, listed)?,
                };
                with_facts(row, described, &catalog)
            })
            .collect());
    }
    if listing == Listing::Gemini {
        // `models/gemini-…`, as Gemini names them; a request names them without `models/`.
        static OTHER: LazyLock<regex::Regex> =
            LazyLock::new(|| regex::Regex::new("embedding|imagen|veo|tts|image|live|audio|aqa").unwrap());
        return Ok(rows
            .into_iter()
            .filter_map(|listed| {
                let id = listed["id"].as_str()?;
                let model = id.strip_prefix("models/").unwrap_or(id);
                // One models.dev doesn't know is offered by its name: a Gemini or Gemma model that chats.
                let known = row.catalog.and_then(|provider| catalog.model(provider, model)).is_some();
                if !known && (!(model.starts_with("gemini-") || model.starts_with("gemma-")) || OTHER.is_match(model)) {
                    return None;
                }
                with_facts(row, Listed::named(named(provider, model, None)), &catalog)
            })
            .collect());
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
/// The effort levels a list names, in Kumi's order, and the one it starts at.
fn named_efforts(values: &Value, default: &Value) -> (Vec<EffortInfo>, Option<Effort>) {
    let named: Vec<&str> = values.as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    let efforts = levels(&EFFORTS.into_iter().filter(|effort| named.contains(&effort.as_str())).collect::<Vec<_>>());
    let default = default.as_str().and_then(Effort::parse).filter(|effort| efforts.iter().any(|level| level.effort == *effort));
    (efforts, default)
}
/// Dollars per million tokens, from a price per token as OpenRouter writes it ("0.000002").
fn per_million(value: &Value) -> Option<f64> {
    value
        .as_str()
        .and_then(number::parse)
        .or_else(|| value.as_f64())
        .filter(|n| n.is_finite() && *n >= 0.)
        .map(|n| (n * 1e6 * 1e6).round() / 1e6)
}
/// One model of OpenRouter's list, described; None for one that doesn't answer in words.
fn openrouter_model(provider: ProviderId, listed: &Value) -> Option<Listed> {
    let architecture = &listed["architecture"];
    if architecture["output_modalities"].as_array().is_some_and(|out| !out.contains(&Value::from("text"))) {
        return None;
    }
    let mut info = named(provider, listed["id"].as_str()?, text(&listed["name"], 60));
    let parameters: Vec<&str> = listed["supported_parameters"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    info.tools = Some(parameters.contains(&"tools"));
    let reasoning = &listed["reasoning"];
    (info.efforts, info.default_effort) = named_efforts(&reasoning["supported_efforts"], &reasoning["default_effort"]);
    info.reasoning = Some(parameters.contains(&"reasoning") || !info.efforts.is_empty());
    info.inputs = architecture["input_modalities"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter_map(|input| Input::parse(if input == "file" { "pdf" } else { input }))
        .collect();
    info.context = local_count(&listed["context_length"]).or_else(|| local_count(&listed["top_provider"]["context_length"]));
    info.max_output = local_count(&listed["top_provider"]["max_completion_tokens"]);
    let pricing = &listed["pricing"];
    info.cost = Some(Cost {
        input: per_million(&pricing["prompt"]),
        output: per_million(&pricing["completion"]),
        cache_read: per_million(&pricing["input_cache_read"]),
        cache_write: per_million(&pricing["input_cache_write"]),
    })
    .filter(|cost| cost.input.is_some() || cost.output.is_some());
    // A model OpenRouter is about to retire carries the date it goes.
    info.status = listed["expiration_date"].as_str().filter(|date| !date.is_empty()).map(|_| "deprecated".to_string());
    Some(Listed { info, efforts: true, described: true })
}
/// One text model of Venice's list, described; None for one that's offline.
fn venice_model(provider: ProviderId, listed: &Value) -> Option<Listed> {
    let spec = &listed["model_spec"];
    if spec["offline"] == true {
        return None;
    }
    let capabilities = &spec["capabilities"];
    let mut info = named(provider, listed["id"].as_str()?, text(&spec["name"], 60));
    info.tools = capabilities["supportsFunctionCalling"].as_bool();
    info.reasoning = capabilities["supportsReasoning"].as_bool();
    if capabilities["supportsReasoningEffort"] == true {
        (info.efforts, info.default_effort) =
            named_efforts(&capabilities["reasoningEffortOptions"], &capabilities["defaultReasoningEffort"]);
    }
    info.inputs = [("supportsVision", Input::Image), ("supportsAudioInput", Input::Audio), ("supportsVideoInput", Input::Video)]
        .into_iter()
        .filter(|(capability, _)| capabilities[*capability] == true)
        .map(|(_, input)| input)
        .collect();
    info.context = local_count(&spec["availableContextTokens"]).or_else(|| local_count(&listed["context_length"]));
    info.max_output = local_count(&spec["maxCompletionTokens"]);
    let pricing = &spec["pricing"];
    info.cost = Some(Cost {
        input: pricing["input"]["usd"].as_f64(),
        output: pricing["output"]["usd"].as_f64(),
        cache_read: pricing["cache_input"]["usd"].as_f64(),
        cache_write: None,
    })
    .filter(|cost| cost.input.is_some() || cost.output.is_some());
    // Venice says whether a model's prompts stay private to it, or reach its host anonymized.
    info.labels = text(&spec["privacy"], 20).into_iter().collect();
    info.status =
        if spec["deprecation"].is_object() { Some("deprecated".into()) } else { (spec["betaModel"] == true).then(|| "beta".into()) };
    Some(Listed { info, efforts: true, described: true })
}
/// What a provider makes of an empty request with `key` (`KeyCheck::Empty`).
async fn empty_request(row: &ProviderRow, key: &str, path: &str, options: &Transport) -> ApiKeyCheck {
    let mut headers = key_headers(row, key);
    headers.insert("user-agent".into(), USER_AGENT.clone());
    headers.insert("content-type".into(), "application/json".into());
    let init = FetchInit { method: "POST".into(), headers, body: Some("{}".into()), signal: options.signal.clone() };
    match options.fetch.clone().unwrap_or_else(default_fetch).fetch(&format!("{}{path}", row.base_url), init).await {
        Ok(response) if matches!(response.status, 401 | 403) => ApiKeyCheck::Refused,
        Ok(response) if response.ok() || matches!(response.status, 400 | 422) => ApiKeyCheck::Ok,
        _ => ApiKeyCheck::Unreachable,
    }
}
/// One model of Hugging Face's router, described by the providers that serve it: it takes tools when one of them does.
fn huggingface_model(provider: ProviderId, listed: &Value) -> Option<Listed> {
    let mut info = named(provider, listed["id"].as_str()?, None);
    let served: Vec<&Value> = listed["providers"].as_array().into_iter().flatten().filter(|p| p["status"] != "error").collect();
    if !served.is_empty() {
        info.tools = Some(served.iter().any(|p| p["supports_tools"] == true));
        info.context = served.iter().filter_map(|p| local_count(&p["context_length"])).reduce(f64::max);
    }
    info.inputs = listed["architecture"]["input_modalities"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter_map(Input::parse)
        .collect();
    let described = !served.is_empty();
    Some(Listed { info, efforts: false, described })
}
/// A positive whole count (tokens), as a list writes it.
fn local_count(value: &Value) -> Option<f64> {
    value.as_f64().filter(|n| *n > 0. && n.fract() == 0. && *n <= 1e12)
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApiKeyCheck {
    Ok,
    Refused,
    Unreachable,
}
pub async fn check_api_key(provider: ProviderId, key: &str, options: Transport) -> ApiKeyCheck {
    let row = provider.row();
    let checked = match row.key_check {
        KeyCheck::List => list_with_key(provider, key, &options).await.map(|_| ()),
        KeyCheck::Get(path) => {
            let url = if path.starts_with("https://") { path.to_string() } else { format!("{}{path}", row.base_url) };
            get_json(&url, key_headers(row, key), &options, provider).await.map(|_| ())
        }
        KeyCheck::Empty(path) => return empty_request(row, key, path, &options).await,
    };
    match checked {
        Ok(()) => ApiKeyCheck::Ok,
        Err(RuntimeError::Kumi(error)) if error.kind == FailureKind::Auth => ApiKeyCheck::Refused,
        Err(_) => ApiKeyCheck::Unreachable,
    }
}
