//! Part of the `@ai-sdk` replacement (see `ai/mod.rs`): `APICallError`, and the error a model's
//! stream carries.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::core::errors::KumiError;

/// `APICallError` from `@ai-sdk/provider`: a provider's HTTP call that failed, with what the SDK
/// kept of the request and the response. Header names are lowercase.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiCallError {
    pub message: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_body_values: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_code: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_headers: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_body: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cause: Option<String>,
    pub is_retryable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl ApiCallError {
    pub const NAME: &'static str = "AI_APICallError";

    /// An error with the SDK's default `isRetryable` for its status (set the other fields as needed).
    pub fn new(message: impl Into<String>, url: impl Into<String>, request_body_values: Option<Value>, status_code: Option<u16>) -> Self {
        Self {
            message: message.into(),
            url: url.into(),
            request_body_values,
            status_code,
            response_headers: None,
            response_body: None,
            cause: None,
            is_retryable: Self::default_retryable(status_code),
            data: None,
        }
    }

    /// `isRetryable = statusCode != null && (408 || 409 || 429 || >= 500)`.
    pub fn default_retryable(status_code: Option<u16>) -> bool {
        matches!(status_code, Some(408 | 409 | 429)) || status_code.is_some_and(|status| status >= 500)
    }
}

impl fmt::Display for ApiCallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ApiCallError {}

/// What a model call throws, or its stream carries in an `error` part: the SDK's `APICallError`,
/// one of Kumi's own, or any other error by its message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
#[allow(clippy::large_enum_variant)]
pub enum LanguageModelError {
    ApiCall(ApiCallError),
    Kumi(KumiError),
    Other(String),
    /// A provider's normalized in-stream error, including retry and status metadata.
    ProviderStream(serde_json::Map<String, Value>),
}

impl LanguageModelError {
    pub fn other(message: impl Into<String>) -> Self {
        Self::Other(message.into())
    }

    /// `APICallError.isInstance(error) ? error : undefined`.
    pub fn api_call(&self) -> Option<&ApiCallError> {
        match self {
            Self::ApiCall(error) => Some(error),
            _ => None,
        }
    }
}

impl fmt::Display for LanguageModelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ApiCall(error) => f.write_str(&error.message),
            Self::Kumi(error) => f.write_str(&error.message),
            Self::Other(message) => f.write_str(message),
            Self::ProviderStream(error) => f.write_str(error.get("message").and_then(Value::as_str).unwrap_or("Provider stream error")),
        }
    }
}

impl std::error::Error for LanguageModelError {}

impl From<ApiCallError> for LanguageModelError {
    fn from(error: ApiCallError) -> Self {
        Self::ApiCall(error)
    }
}

impl From<KumiError> for LanguageModelError {
    fn from(error: KumiError) -> Self {
        Self::Kumi(error)
    }
}
