//! Part of the `@ai-sdk` replacement (see `ai/mod.rs`).

use super::error::{ApiCallError, LanguageModelError};
use async_trait::async_trait;
use futures::{Stream, StreamExt};
use kumi_common::{abort::Signal, js::json::stringify};
use kumi_net::proxy;
use serde_json::Value;
use std::{collections::BTreeMap, pin::Pin, rc::Rc, sync::Arc};

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
/// The proxy settings a client goes by, asked as each connection is made.
pub type Proxies = Arc<dyn Fn() -> Arc<proxy::Settings> + Send + Sync>;
#[derive(Clone)]
pub struct HttpFetch {
    client: reqwest::Client,
    /// What the client's proxies come from: after a 407 from the system's proxy, a request tries once directly.
    proxies: Proxies,
}
/// Node's fetch gave up connecting after 10 s and on a body that went quiet for 300 s (headers included), both
/// retryable; without them a stalled stream waited for the session's quiet timer.
const CONNECT: std::time::Duration = std::time::Duration::from_secs(10);
const QUIET: std::time::Duration = std::time::Duration::from_secs(300);
impl Default for HttpFetch {
    fn default() -> Self {
        Self::with_timeouts(CONNECT, QUIET)
    }
}
impl HttpFetch {
    pub fn with_timeouts(connect: std::time::Duration, read: std::time::Duration) -> Self {
        Self::through(Arc::new(proxy::current), connect, read)
    }
    /// For a model server the producer names: the variables' proxies alone ([`kumi_net::model_servers`]).
    pub fn model_servers() -> Self {
        Self::through(Arc::new(proxy::environment), CONNECT, QUIET)
    }
    /// With the proxies `proxies` gives.
    pub fn through(proxies: Proxies, connect: std::time::Duration, read: std::time::Duration) -> Self {
        let asked = Arc::clone(&proxies);
        let client = kumi_net::client_through(move || asked()).connect_timeout(connect).read_timeout(read).build();
        Self::with_client(client.expect("an HTTP client"), proxies)
    }
    /// A client made already, which goes by `proxies`.
    pub fn with_client(client: reqwest::Client, proxies: Proxies) -> Self {
        Self { client, proxies }
    }
}
pub fn default_fetch() -> Rc<dyn Fetch> {
    Rc::new(HttpFetch::default())
}
/// Kumi's HTTP for a model server the producer names (Ollama, LM Studio, settings.json's modelServers).
pub fn model_server_fetch() -> Rc<dyn Fetch> {
    Rc::new(HttpFetch::model_servers())
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
        // Past the system's proxy, once, when it's in the way (`net::send`).
        let response = tokio::select! {
            biased;
            _ = signal.cancelled() => return Err(LanguageModelError::other("This operation was aborted")),
            response = kumi_net::send_through(request, (self.proxies)()) => response,
        };
        let response = response.map_err(|error| {
            let mut api = ApiCallError::new(format!("Cannot connect to API: {error}"), url, None, None);
            api.is_retryable = error.is_connect() || error.is_timeout() || error.is_body() || error.is_request();
            api.cause = Some(kumi_net::cause::chain(&error));
            LanguageModelError::from(api)
        })?;
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
                // What went wrong is kept with its causes: `web::cause` reads them.
                chunk = source.next() => chunk.map(|chunk| chunk.map(|bytes| bytes.to_vec()).map_err(|error| LanguageModelError::other(kumi_net::cause::chain(&error)))),
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
    body: Rc<Value>,
    signal: Option<Signal>,
) -> Result<Response, LanguageModelError> {
    let mut headers = headers;
    headers.entry("content-type".into()).or_insert_with(|| "application/json".into());
    let text = stringify(&body);
    crate::timing::sent(text.len());
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
                    let mut wrapped =
                        ApiCallError::new("Failed to process successful response", &url, Some(Value::clone(&body)), Some(status));
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
    let mut error = ApiCallError::new(message, url, Some(Value::clone(&body)), Some(status));
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
            let mut response = post_json(&fetch, &url, Headers::new(), Rc::new(serde_json::json!({"stream":true})), None).await.unwrap();
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
            assert_eq!(crate::failure::retry_delay_ms(&failure, 0), Some(750.0));
        }
    }

    /// A server that answers each connection with `answer`, counting them.
    async fn answering(answer: &'static [u8]) -> (std::net::SocketAddr, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = std::sync::Arc::clone(&count);
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                tokio::spawn(async move {
                    let mut request = vec![0; 8192];
                    let _ = socket.read(&mut request).await;
                    let _ = socket.write_all(answer).await;
                });
            }
        });
        (address, count)
    }

    #[tokio::test]
    async fn a_system_proxy_that_wants_a_sign_in_kumi_cant_give_is_passed_by() {
        use kumi_net::proxy::{pass_by, Settings};
        let _turn = kumi_net::proxy::PASSING_BY.lock().await;
        kumi_net::proxy::stop_passing_by();
        let wants = b"HTTP/1.1 407 Proxy Authentication Required\r\nproxy-authenticate: NTLM\r\ncontent-length: 0\r\n\r\n";
        let (proxy, asked) = answering(wants).await;
        let (target, _) = answering(b"HTTP/1.1 200 OK\r\ncontent-length: 6\r\n\r\ndirect").await;
        let (gone, _) = answering(b"").await;
        let through = |settings: Settings| {
            let settings = std::sync::Arc::new(settings);
            let proxies: Proxies = std::sync::Arc::new(move || std::sync::Arc::clone(&settings));
            let asked = std::sync::Arc::clone(&proxies);
            // A name for this computer that isn't one by its name, so a proxy is asked for it.
            let client = kumi_net::client_through(move || asked()).resolve("kumi-pass-by.test", target).resolve("kumi-gone.test", gone);
            HttpFetch::with_client(client.build().unwrap(), proxies)
        };
        let get = |fetch: HttpFetch, url: String| async move {
            let response = fetch.fetch(&url, FetchInit::default()).await.unwrap();
            (response.status, response.text().await.unwrap())
        };
        let url = format!("http://kumi-pass-by.test:{}/", target.port());
        // The system's proxy: its 407, then once directly, which works, and directly from then on.
        let system = || Settings::system_for_tests(Some(&format!("http://{proxy}")), false);
        assert_eq!(get(through(system()), url.clone()).await, (200, "direct".to_string()));
        assert_eq!(get(through(system()), url.clone()).await, (200, "direct".to_string()));
        assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), 1, "passed by after its first 407");
        // A variable's proxy is the producer's to sign in to: its 407 stands.
        let env = kumi_types::system::Env::from([("HTTP_PROXY".to_string(), format!("http://{proxy}"))]);
        let variables = Settings::from_env(&env).unwrap();
        assert!(!pass_by(&variables, &reqwest::Url::parse(&url).unwrap()));
        assert_eq!(get(through(variables), url.clone()).await.0, 407);
        // Directly fails too: its 407 is what's said, and the proxy is asked again next time.
        kumi_net::proxy::stop_passing_by();
        let nowhere = format!("http://kumi-gone.test:{}/", gone.port());
        assert_eq!(get(through(system()), nowhere).await.0, 407);
        let before = asked.load(std::sync::atomic::Ordering::SeqCst);
        assert_eq!(get(through(system()), url).await, (200, "direct".to_string()));
        assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), before + 1, "asked again, then passed by");
    }
}

/// Why a call got no answer (kumi-net's `cause`), as Kumi's own HTTP client keeps it for each cause.
#[cfg(test)]
mod cause_tests {
    use super::*;
    use kumi_net::cause::NetCause;
    use kumi_net::proxy;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// A local server that does `answer` with each connection.
    async fn server(answer: &'static [u8], close: bool) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap().to_string();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut request = vec![0; 4096];
                    let _ = socket.read(&mut request).await;
                    let _ = socket.write_all(answer).await;
                    if !close {
                        tokio::time::sleep(Duration::from_secs(30)).await;
                    }
                });
            }
        });
        address
    }

    /// An address nothing listens on.
    async fn closed_port() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap().to_string();
        drop(listener);
        address
    }

    async fn failed(fetch: &HttpFetch, url: &str) -> LanguageModelError {
        match fetch.fetch(url, FetchInit::default()).await {
            Ok(response) => response.text().await.expect_err("a broken body"),
            Err(error) => error,
        }
    }

    #[tokio::test]
    async fn kumis_own_http_keeps_what_went_wrong_for_each_cause() {
        // No proxy, whatever this machine has set: each cause as the network gives it.
        let direct =
            |read| HttpFetch::through(std::sync::Arc::new(|| std::sync::Arc::new(proxy::Settings::none())), Duration::from_secs(5), read);
        let fetch = direct(Duration::from_millis(300));
        // The read timeout counts from the request on, connecting and looking the name up included, and Windows tries a
        // closed port again for a second or two before it says refused: these two wait as long as they need.
        let patient = direct(Duration::from_secs(5));
        // Refused.
        let refused = failed(&patient, &format!("http://{}/", closed_port().await)).await;
        assert_eq!(NetCause::of_call(&refused), Some(NetCause::Offline), "{refused:?}");
        // A name that never resolves (RFC 6761).
        let unknown = failed(&patient, "http://kumi-net-cause.invalid/").await;
        assert_eq!(NetCause::of_call(&unknown), Some(NetCause::Dns), "{unknown:?}");
        // Something that answers HTTPS with plain words: a captive portal, say.
        let portal = server(b"HTTP/1.1 400 Bad Request\r\ncontent-length: 0\r\n\r\n", true).await;
        let tls = failed(&fetch, &format!("https://{portal}/")).await;
        assert_eq!(NetCause::of_call(&tls), Some(NetCause::Tls), "{tls:?}");
        // A server that accepts but never answers.
        let silent = server(b"", false).await;
        let slow = failed(&fetch, &format!("http://{silent}/")).await;
        assert_eq!(NetCause::of_call(&slow), Some(NetCause::Timeout), "{slow:?}");
        // A body that stops partway, then the connection goes.
        let partial = server(b"HTTP/1.1 200 OK\r\ncontent-length: 1000\r\n\r\npartial", true).await;
        let cut = failed(&fetch, &format!("http://{partial}/")).await;
        assert!(matches!(NetCause::of_call(&cut), Some(NetCause::Offline)), "{cut:?}");
        let LanguageModelError::Other(words) = &cut else { panic!("{cut:?}") };
        assert!(words.starts_with("error decoding response body"), "the first words stay as they were: {words}");
    }
}
