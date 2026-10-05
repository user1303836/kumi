//! Part of the `@ai-sdk` replacement (see `ai/mod.rs`).

use super::error::{ApiCallError, LanguageModelError};
use async_trait::async_trait;
use futures::{Stream, StreamExt};
use kumi_common::{abort::Signal, js::json::stringify};
use serde_json::Value;
use std::{collections::BTreeMap, pin::Pin, rc::Rc};

pub type Headers = BTreeMap<String, String>;
pub type ByteStream = Pin<Box<dyn Stream<Item = Result<Vec<u8>, LanguageModelError>>>>;
#[derive(Clone, Default)]
pub struct FetchInit {
    /// Empty means GET, as with fetch.
    pub method: String,
    pub headers: Headers,
    pub body: Option<String>,
    pub signal: Option<Signal>,
}
pub struct Response {
    pub status: u16,
    pub status_text: String,
    pub headers: Headers,
    pub body: Option<ByteStream>,
}
impl Response {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
    pub fn text_response(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            status_text: reqwest::StatusCode::from_u16(status).ok().and_then(|s| s.canonical_reason()).unwrap_or("").into(),
            headers: Headers::new(),
            body: Some(Box::pin(futures::stream::once(futures::future::ready(Ok(body.into().into_bytes()))))),
        }
    }
    pub fn json_response(status: u16, body: Value) -> Self {
        let mut response = Self::text_response(status, stringify(&body));
        response.headers.insert("content-type".into(), "application/json".into());
        response
    }
    pub async fn text(self) -> Result<String, LanguageModelError> {
        let mut bytes = Vec::new();
        if let Some(mut body) = self.body {
            while let Some(chunk) = body.next().await {
                bytes.extend_from_slice(&chunk?);
            }
        }
        // fetch's text decoder strips an initial UTF-8 BOM.
        let text = String::from_utf8_lossy(&bytes).into_owned();
        Ok(text.strip_prefix('\u{feff}').unwrap_or(&text).into())
    }
    pub async fn json(self) -> Result<Value, LanguageModelError> {
        serde_json::from_str(&self.text().await?).map_err(|error| LanguageModelError::other(error.to_string()))
    }
}
#[async_trait(?Send)]
pub trait Fetch {
    async fn fetch(&self, url: &str, init: FetchInit) -> Result<Response, LanguageModelError>;
}
#[derive(Clone)]
pub struct HttpFetch {
    client: reqwest::Client,
}
impl Default for HttpFetch {
    /// Node's fetch gave up connecting after 10 s and on a body that went quiet for 300 s (headers
    /// included), both retryable; without them a stalled stream waited for the session's quiet timer.
    fn default() -> Self {
        Self::with_timeouts(std::time::Duration::from_secs(10), std::time::Duration::from_secs(300))
    }
}
impl HttpFetch {
    pub fn with_timeouts(connect: std::time::Duration, read: std::time::Duration) -> Self {
        Self { client: reqwest::Client::builder().connect_timeout(connect).read_timeout(read).build().expect("an HTTP client") }
    }
}
pub fn default_fetch() -> Rc<dyn Fetch> {
    Rc::new(HttpFetch::default())
}
#[async_trait(?Send)]
impl Fetch for HttpFetch {
    async fn fetch(&self, url: &str, init: FetchInit) -> Result<Response, LanguageModelError> {
        let method = if init.method.is_empty() {
            reqwest::Method::GET
        } else {
            reqwest::Method::from_bytes(init.method.as_bytes()).map_err(|error| LanguageModelError::other(error.to_string()))?
        };
        let mut request = self.client.request(method, url);
        for (key, value) in init.headers {
            request = request.header(key, value);
        }
        if let Some(body) = init.body {
            request = request.body(body);
        }
        let signal = init.signal.unwrap_or_default();
        let response = tokio::select! {
            biased;
            _ = signal.cancelled() => return Err(LanguageModelError::other("This operation was aborted")),
            response = request.send() => response.map_err(|error| {
                let mut api = ApiCallError::new(format!("Cannot connect to API: {error}"), url, None, None);
                api.is_retryable = error.is_connect() || error.is_timeout() || error.is_body() || error.is_request();
                api.cause = Some(error.to_string()); LanguageModelError::from(api)
            })?,
        };
        let status = response.status();
        let headers = response
            .headers()
            .iter()
            .filter_map(|(key, value)| value.to_str().ok().map(|value| (key.as_str().into(), value.into())))
            .collect();
        let source = response.bytes_stream();
        let body = futures::stream::unfold((Box::pin(source), signal, false), |(mut source, signal, ended)| async move {
            if ended {
                return None;
            }
            let next = tokio::select! {
                biased;
                _ = signal.cancelled() => Some(Err(LanguageModelError::other("This operation was aborted"))),
                chunk = source.next() => chunk.map(|chunk| chunk.map(|bytes| bytes.to_vec()).map_err(|error| LanguageModelError::other(error.to_string()))),
            };
            let ended = next.as_ref().is_some_and(Result::is_err);
            next.map(|next| (next, (source, signal, ended)))
        });
        Ok(Response {
            status: status.as_u16(),
            status_text: status.canonical_reason().unwrap_or("").into(),
            headers,
            body: Some(Box::pin(body)),
        })
    }
}

/// The SDK's JSON POST and HTTP error normalization; streaming decoders own successful bodies.
pub async fn post_json(
    fetch: &dyn Fetch,
    url: &str,
    headers: Headers,
    body: Value,
    signal: Option<Signal>,
) -> Result<Response, LanguageModelError> {
    let mut headers = headers;
    headers.entry("content-type".into()).or_insert_with(|| "application/json".into());
    let text = stringify(&body);
    crate::core::timing::sent(text.len());
    let mut response = fetch.fetch(url, FetchInit { method: "POST".into(), headers, body: Some(text), signal: signal.clone() }).await?;
    if response.ok() && response.body.is_some() {
        let status = response.status;
        let response_headers = response.headers.clone();
        let url = url.to_string();
        response.body = response.body.take().map(|source| {
            Box::pin(source.map(move |chunk| {
                chunk.map_err(|error| {
                    if signal.as_ref().is_some_and(Signal::is_cancelled) || matches!(error, LanguageModelError::ApiCall(_)) {
                        return error;
                    }
                    let mut wrapped = ApiCallError::new("Failed to process successful response", &url, Some(body.clone()), Some(status));
                    // The connection failed mid-stream (a reset, a roam, a stalled read): the SDK's
                    // network errors are retryable, and the kernel retries only while nothing has
                    // reached the producer.
                    wrapped.is_retryable = true;
                    wrapped.cause = Some(error.to_string());
                    wrapped.response_headers = Some(response_headers.clone());
                    wrapped.into()
                })
            })) as ByteStream
        });
        return Ok(response);
    }
    let status = response.status;
    let response_headers = response.headers.clone();
    let status_text = response.status_text.clone();
    let missing_body = response.ok();
    let text = response.text().await.unwrap_or_default();
    let parsed = super::sse::safe_json(&text).ok();
    let message = if missing_body {
        "Failed to process successful response".into()
    } else {
        parsed
            .as_ref()
            .and_then(|data| data.get("error"))
            .and_then(|error| error.get("message"))
            .and_then(Value::as_str)
            .unwrap_or(&status_text)
            .to_string()
    };
    let mut error = ApiCallError::new(message, url, Some(body), Some(status));
    error.response_headers = Some(response_headers);
    error.response_body = Some(text);
    error.data = parsed;
    Err(error.into())
}

#[cfg(test)]
mod transport_tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// A provider that answers 200, sends the start of a stream, then stalls or drops the connection.
    async fn provider(drop_connection: bool) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/v1/responses", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = vec![0; 8192];
            let _ = socket.read(&mut request).await;
            let head = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n";
            let part = "data: {\"type\":\"response.reasoning.delta\"}\n\n";
            socket.write_all(format!("{head}{:x}\r\n{part}\r\n", part.len()).as_bytes()).await.unwrap();
            if drop_connection {
                // Mid-chunk, then gone: a reset or a Wi-Fi roam.
                socket.write_all(b"40\r\npartial").await.unwrap();
                drop(socket);
            } else {
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            }
        });
        url
    }

    #[tokio::test]
    async fn a_stream_that_drops_or_stalls_after_200_is_retryable() {
        for drop_connection in [true, false] {
            let fetch = HttpFetch::with_timeouts(std::time::Duration::from_secs(2), std::time::Duration::from_millis(300));
            let url = provider(drop_connection).await;
            let mut response = post_json(&fetch, &url, Headers::new(), serde_json::json!({"stream":true}), None).await.unwrap();
            let mut body = response.body.take().unwrap();
            let mut failure = None;
            while let Some(chunk) = body.next().await {
                if let Err(error) = chunk {
                    failure = Some(error);
                    break;
                }
            }
            let failure = failure.expect("the stream failed");
            let api = failure.api_call().expect("an API call error");
            assert_eq!(api.status_code, Some(200));
            assert!(api.is_retryable, "dropped: {drop_connection}: {failure:?}");
            assert_eq!(crate::kernel::failure::retry_delay_ms(&failure, 0), Some(750.0));
        }
    }
}
