use std::collections::HashMap;
use std::sync::LazyLock;

use kumi_common::js::number::parse as js_number;
use kumi_common::js::string::{head, trim};
use regex::Regex;
use serde_json::Value;

use crate::ai::error::{ApiCallError, LanguageModelError};
use crate::core::errors::{FailureKind, KumiError};

const MAX_RETRY_WAIT_MS: f64 = 30_000.0;
/// How many times a model call is tried again (a busy or overloaded provider usually answers soon).
pub const MAX_RETRIES: u32 = 3;

/// Delay before retry number `attempt` (from 0), or None when the failure is not worth retrying:
/// what the provider asks for, up to 30 s, or 0.75 s, 2.25 s, 6.75 s when it doesn't say.
pub fn retry_delay_ms(error: &LanguageModelError, attempt: u32) -> Option<f64> {
    let error = error.api_call()?;
    if !error.is_retryable {
        return None;
    }
    let header = |name: &str| {
        error.response_headers.as_ref().and_then(|headers| headers.get(name)).and_then(|value| js_number(value)).unwrap_or(f64::NAN)
    };
    let after_ms = header("retry-after-ms");
    let after_seconds = header("retry-after");
    let requested = if after_ms.is_finite() && after_ms > 0.0 {
        after_ms
    } else if after_seconds.is_finite() && after_seconds > 0.0 {
        after_seconds * 1000.0
    } else {
        750.0 * 3f64.powi(attempt as i32)
    };
    (requested <= MAX_RETRY_WAIT_MS).then(|| requested.max(250.0))
}

/// Providers by the name producers know them; the binding id's prefix otherwise.
static PROVIDER_NAMES: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| {
    HashMap::from([
        ("openai-codex", "ChatGPT"),
        ("openai", "OpenAI"),
        ("anthropic", "Anthropic"),
        ("opencode", "OpenCode Zen"),
        ("opencode-go", "OpenCode Go"),
    ])
});

/// Map any inference failure to a message Kumi wrote, tagged with the provider it concerns so the
/// app can offer the fix (sign in again, choose another model). Provider detail is bounded and never
/// includes credentials.
pub fn describe_failure(error: &LanguageModelError, binding_id: &str) -> KumiError {
    if let LanguageModelError::Kumi(error) = error {
        return error.clone();
    }
    let provider = binding_id.split('/').next().unwrap_or(binding_id);
    let model = binding_id.get(provider.len() + 1..).filter(|rest| !rest.is_empty()).unwrap_or(binding_id);
    let name = PROVIDER_NAMES.get(provider).copied().unwrap_or(provider);
    if let LanguageModelError::ApiCall(error) = error {
        let status = error.status_code;
        let detail = provider_detail(error);
        let with = |lead: &str| if detail.is_empty() { String::new() } else { format!("{lead}{detail}") };
        return match status {
            Some(401) => KumiError::with_provider(
                FailureKind::Auth,
                format!("{name} didn't accept Kumi's sign-in (HTTP 401): sign in again, or check the key."),
                provider,
            ),
            Some(403) => KumiError::with_provider(
                FailureKind::Auth,
                format!("{name} says this sign-in can't use {model} (HTTP 403){}.", with(": ")),
                provider,
            ),
            Some(402) => KumiError::with_provider(
                FailureKind::Billing,
                format!("{name} needs billing sorted before it answers (HTTP 402){}.", with(": ")),
                provider,
            ),
            Some(404) => KumiError::with_provider(
                FailureKind::Model,
                format!("{name} doesn't offer {model} to this sign-in (HTTP 404); choose another model."),
                provider,
            ),
            Some(429) => KumiError::with_provider(
                FailureKind::RateLimit,
                format!(
                    "{name}'s rate or usage limit was reached (HTTP 429); try again in a moment{}.",
                    if detail.is_empty() { String::new() } else { format!(" ({detail})") }
                ),
                provider,
            ),
            Some(status @ (529 | 503)) => KumiError::with_provider(
                FailureKind::Provider,
                format!("{name} is overloaded right now (HTTP {status}); try again in a moment."),
                provider,
            ),
            Some(status) if status >= 500 => {
                KumiError::with_provider(FailureKind::Provider, format!("{name} is having trouble (HTTP {status}); try again."), provider)
            }
            Some(status) => KumiError::with_provider(
                FailureKind::Request,
                format!("{name} turned the request down (HTTP {status}){}.", with(": ")),
                provider,
            ),
            None => KumiError::with_provider(FailureKind::Network, format!("Kumi couldn't reach {name}; check the connection."), provider),
        };
    }
    KumiError::with_provider(FailureKind::Provider, "The model didn't answer; check the model, sign-in and connection.", provider)
}

static STATUS_TEXT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(bad request|not found|unprocessable entity|unknown error)$").expect("regex"));

/// The provider's own short explanation of a rejected request (e.g. an unsupported parameter).
fn provider_detail(error: &ApiCallError) -> String {
    let mut detail = error.message.clone();
    if trim(&detail).is_empty() || STATUS_TEXT.is_match(trim(&detail)) {
        if let Ok(body) = serde_json::from_str::<Value>(error.response_body.as_deref().unwrap_or("")) {
            let record = body.as_object().cloned().unwrap_or_default();
            let nested = match record.get("error") {
                Some(Value::Object(error)) => error.get("message"),
                other => other,
            };
            let candidate = [record.get("detail"), nested, record.get("message")]
                .into_iter()
                .flatten()
                .find_map(|value| value.as_str().filter(|text| !trim(text).is_empty()));
            if let Some(candidate) = candidate {
                detail = candidate.to_string();
            }
        }
    }
    clean_detail(&detail)
}

static CONTROL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\x00-\x1f\x7f-\x9f]").expect("regex"));
static BEARER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)(?-u:\b)Bearer\s+\S+").expect("regex"));
static TOKEN_LIKE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[A-Za-z0-9._~+/=-]{32,}").expect("regex"));

/// A server's own words, safe to show: one line, no tokens or anything that looks like one, bounded.
pub fn clean_detail(detail: &str) -> String {
    let spaced = CONTROL.replace_all(detail, " ");
    let unbearered = BEARER.replace_all(&spaced, "Bearer [redacted]");
    let redacted = TOKEN_LIKE.replace_all(&unbearered, "[redacted]");
    head(trim(&redacted), 300)
}
