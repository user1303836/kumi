use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::LazyLock;

use kumi_common::js::json::stringify;
use kumi_common::js::number::parse as js_number;
use kumi_common::js::string::{head, trim};
use regex::Regex;
use serde_json::Value;

use crate::ai::error::{ApiCallError, LanguageModelError};
use crate::core::errors::{FailureKind, KumiError};
use crate::web::cause::{self, NetCause};

/// How many times a model call is tried again (a busy or overloaded provider usually answers soon).
pub const MAX_RETRIES: u32 = 3;
/// The longest Kumi waits for a provider before giving up on the answer: a plan's limit resetting later
/// than this (a weekly window, say) stops it, saying when, so the producer can pick another way.
pub const LONGEST_WAIT_MS: f64 = 6.0 * 3600.0 * 1000.0;
/// How long after a plan's limit resets Kumi tries again: the provider's clock and Kumi's may differ.
pub const RESET_MARGIN_MS: f64 = 30_000.0;
/// Waits between tries while there's no connection: each a little longer, then every 30 s.
const OFFLINE_MAX_WAIT_MS: f64 = 30_000.0;
/// Error codes that mean a plan's credit or spend is used up: waiting won't help.
const QUOTA_CODES: [&str; 5] = [
    "insufficient_quota",
    "credit_balance_exhausted",
    "organization_spend_limit_exceeded",
    "project_spend_limit_exceeded",
    "organization_usage_limit_exceeded",
];

/// A plan's limit, as the provider reported it (ChatGPT's 429 body, a quota code).
#[derive(Debug, Clone, PartialEq)]
pub enum PlanLimit {
    /// The plan's usage is used up until it resets (epoch ms), when the provider says.
    Reached { resets_at_ms: Option<f64> },
    /// The plan doesn't include this model: waiting won't help.
    NotIncluded,
    /// Credit or a spend limit is used up: waiting won't help.
    Quota,
}

/// What Kumi does about a failed model call, when nothing of it has reached the producer yet.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Wait {
    /// Try again after this many ms (a busy or overloaded provider, a request limit): counted against MAX_RETRIES.
    Again(f64),
    /// A plan's limit resets after this many ms (a margin included): then the same answer carries on.
    Reset(f64),
    /// There's no connection: try again after this many ms, for as long as the answer may run.
    Offline(f64),
}

impl Wait {
    pub fn ms(self) -> f64 {
        match self {
            Wait::Again(ms) | Wait::Reset(ms) | Wait::Offline(ms) => ms,
        }
    }
}

/// The failed call an error stands for: its API call's, or one the provider reported inside its stream (Anthropic's
/// overloaded_error, OpenAI's response.failed), with the status and retryability the provider gave it.
fn failed_call(error: &LanguageModelError) -> Option<Cow<'_, ApiCallError>> {
    match error {
        LanguageModelError::ApiCall(error) => Some(Cow::Borrowed(error)),
        LanguageModelError::ProviderStream(error) => {
            let status = error.get("statusCode").and_then(Value::as_u64).and_then(|status| u16::try_from(status).ok());
            let mut call = ApiCallError::new(error.get("message").and_then(Value::as_str).unwrap_or(""), "", None, status);
            call.is_retryable = error.get("isRetryable").and_then(Value::as_bool).unwrap_or(false);
            call.response_body = error.get("data").map(stringify);
            Some(Cow::Owned(call))
        }
        _ => None,
    }
}

/// Delay before retry number `attempt` (from 0), or None when the failure is not worth retrying: what the
/// provider asks for, or 0.75 s, 2.25 s, 6.75 s when it doesn't say; a plan's limit until it resets.
pub fn retry_delay_ms(error: &LanguageModelError, attempt: u32) -> Option<f64> {
    wait_for(error, attempt, attempt, kumi_common::time::now_ms_f64()).map(Wait::ms)
}

/// What to do about a failed call before any of it reached the producer: try again soon (`attempt` tries so far),
/// wait for a plan's limit to reset, or keep trying while offline (`offline` tries so far); None to stop. A wait
/// past [`LONGEST_WAIT_MS`] is none.
pub fn wait_for(error: &LanguageModelError, attempt: u32, offline: u32, now_ms: f64) -> Option<Wait> {
    let call = failed_call(error)?;
    let wait = match plan_limit(error, now_ms) {
        Some(PlanLimit::Reached { resets_at_ms }) => {
            let until = resets_at_ms.map(|at| (at - now_ms).max(0.0)).or_else(|| requested(&call, now_ms))?;
            Wait::Reset(until + RESET_MARGIN_MS)
        }
        Some(PlanLimit::NotIncluded | PlanLimit::Quota) => return None,
        None if !call.is_retryable => return None,
        // No answer at all: the connection, not the provider.
        None if call.status_code.is_none() => Wait::Offline((750.0 * 3f64.powi(offline.min(8) as i32)).min(OFFLINE_MAX_WAIT_MS)),
        None => Wait::Again(requested(&call, now_ms).unwrap_or_else(|| 750.0 * 3f64.powi(attempt as i32)).max(250.0)),
    };
    (wait.ms() <= LONGEST_WAIT_MS).then_some(wait)
}

/// The wait the provider asked for: Retry-After in ms or seconds, or as a date.
fn requested(call: &ApiCallError, now_ms: f64) -> Option<f64> {
    let headers = call.response_headers.as_ref()?;
    let after_ms = headers.get("retry-after-ms").and_then(|value| js_number(value)).unwrap_or(f64::NAN);
    if after_ms.is_finite() && after_ms > 0.0 {
        return Some(after_ms);
    }
    crate::web::net::retry_after(headers, None, Some(now_ms)).filter(|ms| ms.is_finite() && *ms > 0.0)
}

/// The plan limit a failed call reports: ChatGPT's `usage_limit_reached` (with when it resets), a plan that doesn't
/// include the model, or used-up credit or spend.
pub fn plan_limit(error: &LanguageModelError, now_ms: f64) -> Option<PlanLimit> {
    let call = failed_call(error)?;
    let body = call.response_body.as_deref().and_then(|body| serde_json::from_str::<Value>(body).ok()).or_else(|| call.data.clone())?;
    let mut codes = Vec::new();
    collect_codes(&body, 0, &mut codes);
    if codes.contains(&"usage_limit_reached") {
        let seconds = |key: &str| find_number(&body, key, 0).filter(|seconds| seconds.is_finite() && *seconds > 0.0);
        let resets_at_ms = seconds("resets_at").map(|at| at * 1000.0).or_else(|| seconds("resets_in_seconds").map(|s| now_ms + s * 1000.0));
        return Some(PlanLimit::Reached { resets_at_ms });
    }
    if codes.contains(&"usage_not_included") {
        return Some(PlanLimit::NotIncluded);
    }
    // Anthropic says an empty balance only in words, on a 400 invalid_request_error.
    let words = format!("{} {}", call.message, call.response_body.as_deref().unwrap_or("")).to_lowercase();
    (codes.iter().any(|code| QUOTA_CODES.contains(code)) || words.contains("credit balance is too low")).then_some(PlanLimit::Quota)
}

/// Words a provider uses when it doesn't offer the model asked for: ChatGPT's "The '…' model is not supported…",
/// "Model not found", "unknown model", a model "no longer available" or "retired".
static MODEL_GONE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\bmodel\b[^.]{0,80}?\b(is not supported|isn't supported|is not available|isn't available|is no longer (available|supported|offered)|has been (retired|deprecated|removed)|does not exist|doesn't exist|not found)\b|\b(unsupported|unknown) model\b|\bno such model\b",
    )
    .expect("regex")
});

/// Whether a failed call says the provider doesn't offer the model it was asked for: `model_not_found`, or a 400, 404 or
/// 422 that says so (ChatGPT's `{"detail":"The 'x' model is not supported …"}`, Anthropic's `not_found_error` about the
/// model). Not a plan that leaves the model out (`plan_limit`), nor a setting the model doesn't take.
pub fn model_unavailable(error: &LanguageModelError) -> bool {
    let Some(call) = failed_call(error) else { return false };
    let body = call.response_body.as_deref().and_then(|body| serde_json::from_str::<Value>(body).ok()).or_else(|| call.data.clone());
    let mut codes = Vec::new();
    if let Some(body) = &body {
        collect_codes(body, 0, &mut codes);
    }
    if codes.contains(&"model_not_found") {
        return true;
    }
    // About one of the request's settings (`"param":"reasoning.effort"`, an unsupported value), not the model.
    let param = body.as_ref().and_then(|body| find_text(body, "param", 0));
    if !matches!(call.status_code, Some(400 | 404 | 422))
        || param.is_some_and(|param| param != "model")
        || codes.iter().any(|code| code.starts_with("unsupported_"))
    {
        return false;
    }
    let words = format!("{} {}", call.message, call.response_body.as_deref().unwrap_or(""));
    (codes.contains(&"not_found_error") && words.to_lowercase().contains("model")) || MODEL_GONE.is_match(&words)
}

/// The values of `type` and `code` fields, a few levels down (`{"error":{"type":…}}`, a stream's frame).
fn collect_codes<'v>(value: &'v Value, depth: u32, out: &mut Vec<&'v str>) {
    let Some(object) = value.as_object().filter(|_| depth < 5) else { return };
    for (key, value) in object {
        match value {
            Value::String(code) if key == "type" || key == "code" => out.push(code),
            Value::Object(_) => collect_codes(value, depth + 1, out),
            _ => {}
        }
    }
}

fn find_text<'v>(value: &'v Value, key: &str, depth: u32) -> Option<&'v str> {
    let object = value.as_object().filter(|_| depth < 5)?;
    object.get(key).and_then(Value::as_str).or_else(|| object.values().find_map(|value| find_text(value, key, depth + 1)))
}

fn find_number(value: &Value, key: &str, depth: u32) -> Option<f64> {
    let object = value.as_object().filter(|_| depth < 5)?;
    object.get(key).and_then(Value::as_f64).or_else(|| object.values().find_map(|value| find_number(value, key, depth + 1)))
}

/// Why a model call is being tried again, in a few words for the status line: "ChatGPT is busy (HTTP 429)",
/// "ChatGPT's plan limit".
pub fn retry_reason(error: &LanguageModelError, binding_id: &str) -> String {
    let name = provider_name(binding_id);
    if matches!(plan_limit(error, 0.0), Some(PlanLimit::Reached { .. })) {
        return format!("{name}'s plan limit");
    }
    match failed_call(error).and_then(|error| error.status_code) {
        // A stream that broke off after the response began keeps its 200.
        Some(200..=299) => format!("{name}'s answer broke off"),
        Some(429) => format!("{name} is busy (HTTP 429)"),
        Some(status @ (503 | 529)) => format!("{name} is overloaded (HTTP {status})"),
        Some(status) if status >= 500 => format!("{name} is having trouble (HTTP {status})"),
        Some(status) => format!("{name} asked Kumi to wait (HTTP {status})"),
        None => match refused_on_the_way(error)
            .and_then(|_| cause::refused_words(error, &crate::system::process_env(), &crate::net::proxy::current()))
        {
            Some(words) => format!("{name} couldn't be reached: {words}"),
            None => format!("{name} couldn't be reached"),
        },
    }
}

/// A call that got no answer because the way there refused it: a certificate this computer doesn't trust (TLS) or a
/// proxy. That's not being offline, so the words name it, and the fix.
pub fn refused_on_the_way(error: &LanguageModelError) -> Option<NetCause> {
    NetCause::of_call(error).filter(|cause| matches!(cause, NetCause::Tls | NetCause::Proxy))
}

/// A provider by the name producers know it ("ChatGPT"), from a binding id ("openai-codex/gpt-6-sol").
pub fn provider_name(binding_id: &str) -> &str {
    let provider = binding_id.split('/').next().unwrap_or(binding_id);
    PROVIDER_NAMES.get(provider).copied().unwrap_or(provider)
}

/// When a plan's limit resets, in the producer's words: "14:20", "Tue 09:00" past a day away.
pub fn reset_words(resets_at_ms: f64, now_ms: f64) -> String {
    use chrono::{Local, TimeZone};
    let Some(at) = Local.timestamp_millis_opt(resets_at_ms as i64).single() else { return "later".into() };
    if resets_at_ms - now_ms < 20.0 * 3600.0 * 1000.0 {
        at.format("%H:%M").to_string()
    } else {
        at.format("%a %H:%M").to_string()
    }
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
    let now = kumi_common::time::now_ms_f64();
    match plan_limit(error, now) {
        Some(PlanLimit::Reached { resets_at_ms }) => {
            let when = resets_at_ms.map(|at| format!("; it resets at {}", reset_words(at, now))).unwrap_or_default();
            return KumiError::with_provider(
                FailureKind::UsageLimit,
                format!("{name}'s plan limit is reached{when}. Choose a fallback model (/model), another model, or sign in to another provider."),
                provider,
            );
        }
        Some(PlanLimit::NotIncluded) => {
            return KumiError::with_provider(
                FailureKind::Model,
                format!("{name}'s plan doesn't include {model}; choose another model."),
                provider,
            );
        }
        Some(PlanLimit::Quota) => {
            return KumiError::with_provider(
                FailureKind::Billing,
                format!("{name} has no credit left for this sign-in: add credit, or choose another model."),
                provider,
            );
        }
        None => {}
    }
    // A model the provider no longer offers (retired from a plan's sign-in, say), or never did.
    if model_unavailable(error) {
        return KumiError::with_provider(
            FailureKind::Model,
            format!("{name} doesn't offer {model} to your sign-in; choose another model."),
            provider,
        );
    }
    // A certificate or a proxy in the way: the cause, its fix and the proxy that call went through.
    let env = crate::system::process_env();
    let system = crate::net::proxy::current();
    let on_the_way = refused_on_the_way(error).and_then(|_| {
        let url = failed_call(error).map(|call| call.url.clone()).unwrap_or_default();
        cause::unreachable_call(name, &url, error, &env, &system)
    });
    if let Some(error) = failed_call(error) {
        let status = error.status_code;
        let detail = provider_detail(&error);
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
            None => KumiError::with_provider(
                FailureKind::Network,
                match on_the_way {
                    Some(on_the_way) => on_the_way,
                    // A network that lets nothing out but through the proxy an auto-config file names: say to set it.
                    None => match cause::auto_config_words(&env, &system) {
                        Some(words) => format!("Kumi couldn't reach {name}; check the connection. {words}"),
                        None => format!("Kumi couldn't reach {name}; check the connection."),
                    },
                },
                provider,
            ),
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn stream_failure(frame: Value, status: u64) -> LanguageModelError {
        let fields = json!({"message":"failed","statusCode":status,"isRetryable":true,"data":frame});
        LanguageModelError::ProviderStream(fields.as_object().unwrap().clone())
    }

    #[test]
    fn a_plans_limit_is_read_from_a_streams_own_frame() {
        let now = 1_000_000.0;
        let frame =
            json!({"type":"response.failed","response":{"error":{"code":"usage_limit_reached","message":"Limit","resets_in_seconds":60}}});
        let failure = stream_failure(frame, 429);
        assert_eq!(plan_limit(&failure, now), Some(PlanLimit::Reached { resets_at_ms: Some(now + 60_000.0) }));
        assert_eq!(wait_for(&failure, 0, 0, now), Some(Wait::Reset(60_000.0 + RESET_MARGIN_MS)));
        // resets_at (seconds since the epoch) wins over resets_in_seconds.
        let both = json!({"error":{"type":"usage_limit_reached","resets_at":2_000,"resets_in_seconds":60}});
        assert_eq!(plan_limit(&stream_failure(both, 429), now), Some(PlanLimit::Reached { resets_at_ms: Some(2_000_000.0) }));
        // A limit that doesn't say when, with no Retry-After either: nothing to wait for.
        let unsaid = stream_failure(json!({"error":{"type":"usage_limit_reached"}}), 429);
        assert_eq!(wait_for(&unsaid, 0, 0, now), None);
    }

    fn refused(status: u16, message: &str, body: &str) -> LanguageModelError {
        let mut call = ApiCallError::new(message, "u", None, Some(status));
        call.response_body = Some(body.into());
        LanguageModelError::ApiCall(call)
    }

    // Each provider's answer to a model it doesn't offer: ChatGPT's as it answered on 2026-10-13, the others' as their
    // documentation and SDKs show them.
    #[test]
    fn a_model_the_provider_doesnt_offer_is_a_model_failure_with_its_fix() {
        let gone = [
            (
                "openai-codex/gpt-5.5",
                refused(
                    400,
                    "Bad Request",
                    r#"{"detail":"The 'gpt-5.5' model is not supported when using Codex with a ChatGPT account."}"#,
                ),
            ),
            (
                "openai/gpt-0",
                refused(
                    404,
                    "The model `gpt-0` does not exist or you do not have access to it.",
                    r#"{"error":{"message":"The model `gpt-0` does not exist or you do not have access to it.","type":"invalid_request_error","param":null,"code":"model_not_found"}}"#,
                ),
            ),
            (
                "openai/gpt-0",
                refused(
                    400,
                    "The requested model 'gpt-0' does not exist.",
                    r#"{"error":{"message":"The requested model 'gpt-0' does not exist.","type":"invalid_request_error","param":"model","code":"model_not_found"}}"#,
                ),
            ),
            ("openai-codex/gpt-5.5", refused(400, "Bad Request", r#"{"detail":"Unsupported model"}"#)),
            (
                "anthropic/claude-0",
                refused(404, "model: claude-0", r#"{"type":"error","error":{"type":"not_found_error","message":"model: claude-0"}}"#),
            ),
            (
                "opencode/kimi-0",
                refused(400, "Bad Request", r#"{"error":{"message":"Model not found: kimi-0","type":"invalid_request_error"}}"#),
            ),
            ("opencode-go/glm-0", refused(422, "Unprocessable Entity", r#"{"error":{"message":"Unknown model: glm-0"}}"#)),
        ];
        for (binding, error) in gone {
            assert!(model_unavailable(&error), "{binding}");
            let described = describe_failure(&error, binding);
            assert_eq!(described.kind, FailureKind::Model, "{binding}");
            let (provider, model) = binding.split_once('/').unwrap();
            let said = format!("{} doesn't offer {model} to your sign-in; choose another model.", provider_name(binding));
            assert_eq!(described.message, said);
            assert_eq!(described.provider.as_deref(), Some(provider));
        }
        // Not the model: a parameter it doesn't take, a request too big, a plan that leaves it out.
        for (error, kind) in [
            (
                refused(
                    400,
                    "Unsupported parameter: 'temperature' is not supported with this model.",
                    r#"{"error":{"code":"unsupported_parameter"}}"#,
                ),
                FailureKind::Request,
            ),
            (
                refused(
                    400,
                    "Unsupported value: 'xhigh' is not supported with the 'gpt-6-luna' model. Supported values are: 'low' and 'medium'.",
                    r#"{"error":{"message":"Unsupported value: 'xhigh' is not supported with the 'gpt-6-luna' model.","type":"invalid_request_error","param":"reasoning.effort","code":"unsupported_value"}}"#,
                ),
                FailureKind::Request,
            ),
            (refused(400, "Bad Request", r#"{"detail":"Input is too long for this request."}"#), FailureKind::Request),
            (refused(403, "Forbidden", r#"{"error":{"type":"usage_not_included"}}"#), FailureKind::Model),
        ] {
            assert!(!model_unavailable(&error));
            let described = describe_failure(&error, "openai-codex/gpt-6-sol");
            assert_eq!(described.kind, kind, "{}", described.message);
        }
    }

    #[test]
    fn an_empty_credit_balance_is_a_quota_whatever_its_status() {
        let mut low = ApiCallError::new("invalid_request_error", "u", None, Some(400));
        low.response_body = Some(
            json!({"type":"error","error":{"type":"invalid_request_error","message":"Your credit balance is too low to access the Anthropic API."}})
                .to_string(),
        );
        let low = LanguageModelError::ApiCall(low);
        assert_eq!(plan_limit(&low, 0.0), Some(PlanLimit::Quota));
        assert_eq!(wait_for(&low, 0, 0, 0.0), None);
        assert_eq!(describe_failure(&low, "anthropic/claude-sonnet-5-5").kind, FailureKind::Billing);
    }

    #[test]
    fn busy_and_down_providers_wait_as_they_ask_or_a_growing_while() {
        let now = 1_000_000.0;
        let down = LanguageModelError::ApiCall(ApiCallError::new("down", "u", None, Some(502)));
        assert_eq!([0, 1, 2].map(|attempt| wait_for(&down, attempt, 0, now)), [750.0, 2250.0, 6750.0].map(|ms| Some(Wait::Again(ms))));
        let refused = LanguageModelError::ApiCall(ApiCallError::new("no", "u", None, Some(400)));
        assert_eq!(wait_for(&refused, 0, 0, now), None);
        // Past the longest wait, none.
        let mut later = ApiCallError::new("busy", "u", None, Some(429));
        later.response_headers = Some([("retry-after".to_string(), "86400".to_string())].into_iter().collect());
        assert_eq!(wait_for(&LanguageModelError::ApiCall(later), 0, 0, now), None);
    }
}
