//! Responses over a WebSocket, as ChatGPT serves them. One connection stays open beside the conversation and each
//! streamed `POST …/responses` goes over it as a `response.create` message. A request that carries on from the
//! response before it (the same instructions, tools and options; its input the last one's, then what the model sent
//! and what came of it) sends only what's new, with `previous_response_id`: the connection keeps that response,
//! `store: false` as it is. The events come back as the server-sent events HTTP would have carried, so nothing else
//! changes. Anything else goes over HTTP as before: a proxy in the way, a request while one is on the connection, a
//! server that won't open one, or a connection that keeps failing.
use crate::{
    error::{ApiCallError, LanguageModelError},
    http::{ByteStream, Fetch, FetchInit, Headers, Response},
};
use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use kumi_common::abort::Signal;
use serde_json::{json, Map, Value};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::net::TcpStream;
use tokio_tungstenite::{
    tungstenite::{self, client::IntoClientRequest, http::HeaderValue, Message},
    Connector, MaybeTlsStream, WebSocketStream,
};

/// What the opening asks for, as ChatGPT's own client asks.
const BETA: &str = "responses_websockets=2026-02-06";
/// A connection lasts an hour at most: one past this is opened again before a request rather than cut off during it.
const LIFETIME: Duration = Duration::from_secs(55 * 60);
/// As HTTP's: opening gives up after 10 s, and an answer that goes quiet for 300 s has stopped.
const CONNECT: Duration = Duration::from_secs(10);
const QUIET: Duration = Duration::from_secs(300);
/// A request bigger than this (pictures, say) goes over HTTP: a message goes as one frame.
const MOST_BYTES: usize = 8 * 1024 * 1024;
/// Connections that failed in a row (opening, or ending mid-answer) before HTTP takes over for good.
const FAILURES: u8 = 2;
const ABORTED: &str = "This operation was aborted";

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

struct Line {
    socket: Socket,
    opened: Instant,
    /// The opening's answer: the plan's usage headers, as HTTP's answers carry them.
    headers: Headers,
}

/// The response a later request may carry on from.
struct Last {
    /// The request without its input: what must stay the same.
    fields: Map<String, Value>,
    input: Vec<Value>,
    id: String,
    output: Vec<Value>,
}

#[derive(Default)]
struct Shared {
    line: RefCell<Option<Line>>,
    last: RefCell<Option<Last>>,
    busy: Cell<bool>,
    failures: Cell<u8>,
    /// The server wouldn't open one, or connections kept failing: HTTP from then on.
    off: Cell<bool>,
}

impl Shared {
    fn failed(&self) {
        self.failures.set(self.failures.get() + 1);
        self.off.set(self.off.get() || self.failures.get() >= FAILURES);
    }
}

/// ChatGPT's Responses over a WebSocket, beside `http` for everything else.
pub struct SocketFetch {
    http: Rc<dyn Fetch>,
    shared: Rc<Shared>,
}

impl SocketFetch {
    pub fn new(http: Rc<dyn Fetch>) -> Self {
        Self { http, shared: Rc::default() }
    }
}

/// The request's fields that must match the last one's for this one to carry on from it.
fn fields(body: &Map<String, Value>) -> Map<String, Value> {
    body.iter().filter(|(key, _)| !matches!(key.as_str(), "input" | "stream")).map(|(k, v)| (k.clone(), v.clone())).collect()
}

/// What an output item is, to pair it with how the next request sends it back.
fn kind(item: &Value) -> Option<(&str, Option<&str>)> {
    match item["type"].as_str() {
        Some("reasoning") => Some(("reasoning", None)),
        Some("function_call") => Some(("function_call", item["call_id"].as_str())),
        Some("message") | None if item["role"] == "assistant" => Some(("message", None)),
        _ => None,
    }
}

/// What's new in the request after `last`: its input, then each of its output items as the request sends them back,
/// then at least one more item. None when the request doesn't carry on from it (Kumi cleared or dropped earlier parts
/// of the conversation, say, or its instructions changed).
fn carried_on(last: &Last, body: &Map<String, Value>) -> Option<Vec<Value>> {
    let input = body.get("input")?.as_array()?;
    let (asked, made) = (last.input.len(), last.output.len());
    if fields(body) != last.fields || input.len() <= asked + made || input[..asked] != last.input[..] {
        return None;
    }
    let sent_back = &input[asked..asked + made];
    sent_back
        .iter()
        .zip(&last.output)
        .all(|(sent, made)| kind(sent).is_some() && kind(sent) == kind(made))
        .then(|| input[asked + made..].to_vec())
}

fn retryable(message: impl Into<String>, url: &str) -> LanguageModelError {
    let mut error = ApiCallError::new(message.into(), url, None, None);
    error.is_retryable = true;
    error.into()
}

async fn open(url: &str, headers: &Headers) -> Result<Line, tungstenite::Error> {
    let address = match url.split_once("://") {
        Some(("https", rest)) => format!("wss://{rest}"),
        Some(("http", rest)) => format!("ws://{rest}"),
        _ => url.to_string(),
    };
    let mut request = address.into_client_request()?;
    for (name, value) in headers {
        if matches!(name.as_str(), "content-type" | "content-length" | "accept" | "openai-beta") {
            continue;
        }
        if let (Ok(name), Ok(value)) = (tungstenite::http::HeaderName::try_from(name.as_str()), HeaderValue::from_str(value)) {
            request.headers_mut().insert(name, value);
        }
    }
    request.headers_mut().insert("openai-beta", HeaderValue::from_static(BETA));
    let connector = Connector::Rustls(Arc::new(kumi_net::http1_tls()));
    let opening = tokio_tungstenite::connect_async_tls_with_config(request, None, true, Some(connector));
    let (socket, answer) =
        tokio::time::timeout(CONNECT, opening).await.map_err(|_| tungstenite::Error::Io(std::io::ErrorKind::TimedOut.into()))??;
    let headers = answer.headers().iter().filter_map(|(k, v)| v.to_str().ok().map(|v| (k.as_str().to_string(), v.to_string()))).collect();
    Ok(Line { socket, opened: Instant::now(), headers })
}

enum Next {
    Event(String, Value),
    Closed,
    Broken(String),
    Aborted,
}

/// The connection's next event.
async fn next_event(socket: &mut Socket, signal: &Signal) -> Next {
    loop {
        let message = tokio::select! {
            biased;
            _ = signal.cancelled() => return Next::Aborted,
            message = tokio::time::timeout(QUIET, socket.next()) => message,
        };
        match message {
            Err(_) => return Next::Broken("The answer went quiet".into()),
            Ok(None | Some(Ok(Message::Close(_)))) => return Next::Closed,
            Ok(Some(Ok(Message::Text(text)))) => {
                let event = serde_json::from_str(text.as_str()).unwrap_or(Value::Null);
                return Next::Event(text.as_str().to_string(), event);
            }
            Ok(Some(Ok(_))) => continue,
            Ok(Some(Err(error))) => return Next::Broken(error.to_string()),
        }
    }
}

/// An event as HTTP's stream carries it.
fn framed(text: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(text.len() + 8);
    for line in text.split('\n') {
        bytes.extend_from_slice(b"data: ");
        bytes.extend_from_slice(line.trim_end_matches('\r').as_bytes());
        bytes.push(b'\n');
    }
    bytes.push(b'\n');
    bytes
}

fn terminal(event: &Value) -> bool {
    matches!(event["type"].as_str(), Some("response.completed" | "response.failed" | "response.incomplete" | "error"))
}

/// The connection while a request has it: given back once its answer ends, or let go with it.
struct Holding {
    shared: Rc<Shared>,
    line: Option<Line>,
    released: bool,
}

impl Holding {
    fn release(&mut self) {
        *self.shared.line.borrow_mut() = self.line.take();
        self.shared.busy.set(false);
        self.released = true;
    }
}

impl Drop for Holding {
    fn drop(&mut self) {
        if !self.released {
            self.shared.busy.set(false);
        }
    }
}

/// What the answer's stream keeps to remember its response for the next request.
struct Answer {
    holding: Holding,
    pending: Option<(String, Value)>,
    signal: Signal,
    fields: Map<String, Value>,
    input: Vec<Value>,
    url: String,
    ended: bool,
}

impl Answer {
    async fn next(mut self) -> Option<(Result<Vec<u8>, LanguageModelError>, Self)> {
        if self.ended {
            return None;
        }
        let (text, event) = match self.pending.take() {
            Some(first) => first,
            None => {
                let line = self.holding.line.as_mut()?;
                let failure = match next_event(&mut line.socket, &self.signal).await {
                    Next::Event(text, event) => Ok((text, event)),
                    Next::Aborted => Err(LanguageModelError::other(ABORTED)),
                    Next::Closed => Err(retryable("The connection closed during the answer", &self.url)),
                    Next::Broken(why) => Err(retryable(why, &self.url)),
                };
                match failure {
                    Ok(next) => next,
                    Err(error) => {
                        // Ended mid-answer: the connection goes with it.
                        self.holding.line = None;
                        if matches!(error, LanguageModelError::ApiCall(_)) {
                            self.holding.shared.failed();
                        }
                        self.ended = true;
                        return Some((Err(error), self));
                    }
                }
            }
        };
        if terminal(&event) {
            let shared = self.holding.shared.clone();
            shared.failures.set(0);
            let response = &event["response"];
            *shared.last.borrow_mut() = response["id"].as_str().filter(|_| event["type"] == "response.completed").map(|id| Last {
                fields: std::mem::take(&mut self.fields),
                input: std::mem::take(&mut self.input),
                id: id.to_string(),
                output: response["output"].as_array().cloned().unwrap_or_default(),
            });
            self.holding.release();
            self.ended = true;
        }
        Some((Ok(framed(&text)), self))
    }
}

/// The request's body when it can go over the connection: a streamed Responses request, not in the background.
fn streamed(url: &str, init: &FetchInit) -> Option<Map<String, Value>> {
    if init.method != "POST" || !url.ends_with("/responses") {
        return None;
    }
    let body = init.body.as_deref().filter(|body| body.len() <= MOST_BYTES)?;
    match serde_json::from_str::<Value>(body).ok()? {
        Value::Object(body) if body.get("stream") == Some(&Value::Bool(true)) && body.get("background") != Some(&Value::Bool(true)) => {
            Some(body)
        }
        _ => None,
    }
}

#[async_trait(?Send)]
impl Fetch for SocketFetch {
    async fn fetch(&self, url: &str, init: FetchInit) -> Result<Response, LanguageModelError> {
        let shared = &self.shared;
        let direct = url::Url::parse(url).is_ok_and(|address| kumi_net::proxy::current().proxy_for(&address).is_none());
        let body = streamed(url, &init).filter(|_| direct && !shared.off.get() && !shared.busy.get());
        let Some(body) = body else { return self.http.fetch(url, init).await };
        shared.busy.set(true);
        let mut holding = Holding { shared: shared.clone(), line: shared.line.borrow_mut().take(), released: false };
        let signal = init.signal.clone().unwrap_or_default();
        let mut carry = shared.last.borrow_mut().take().and_then(|last| carried_on(&last, &body).map(|rest| (last.id, rest)));
        let full = init.body.as_deref().map_or(0, str::len);
        let mut opened_now = false;
        let first = loop {
            if holding.line.as_ref().is_none_or(|line| line.opened.elapsed() > LIFETIME) {
                // A new connection keeps no earlier response.
                carry = None;
                match open(url, &init.headers).await {
                    Ok(line) => {
                        holding.line = Some(line);
                        opened_now = true;
                    }
                    Err(error) => {
                        // A server that answers the opening with an HTTP error won't take one, unless it's the sign-in
                        // (HTTP says so as it always has); anything else may pass.
                        match error {
                            tungstenite::Error::Http(answer) if !matches!(answer.status().as_u16(), 401 | 403) => shared.off.set(true),
                            tungstenite::Error::Http(_) => {}
                            _ => shared.failed(),
                        }
                        drop(holding);
                        return self.http.fetch(url, init).await;
                    }
                }
            }
            let mut message = body.clone();
            message.shift_remove("stream");
            message.shift_remove("background");
            if let Some((id, rest)) = &carry {
                message.insert("previous_response_id".into(), json!(id));
                message.insert("input".into(), Value::Array(rest.clone()));
            }
            message.insert("type".into(), json!("response.create"));
            let text = serde_json::to_string(&message).map_err(|error| LanguageModelError::other(error.to_string()))?;
            let size = text.len();
            let line = holding.line.as_mut().expect("a connection");
            if line.socket.send(Message::text(text)).await.is_err() {
                holding.line = None;
                if opened_now {
                    shared.failed();
                    drop(holding);
                    return self.http.fetch(url, init).await;
                }
                // The connection had ended (the server closes one it's done with): once more on a new one.
                continue;
            }
            match next_event(&mut line.socket, &signal).await {
                Next::Event(text, event) => {
                    let code = event["error"]["code"].as_str().unwrap_or("");
                    // The connection no longer has that response, or has had its hour: the whole request again.
                    if event["type"] == "error" && code == "previous_response_not_found" && carry.is_some() {
                        carry = None;
                        continue;
                    }
                    if event["type"] == "error" && code == "websocket_connection_limit_reached" {
                        holding.line = None;
                        continue;
                    }
                    crate::timing::sent_instead(full, size);
                    // Refused outright, with HTTP's status: answered as HTTP answers, so it's told and retried the same.
                    if let Some(status) = event["status"].as_u64().filter(|status| event["type"] == "error" && (400..600).contains(status))
                    {
                        let headers = holding.line.as_ref().map(|line| line.headers.clone()).unwrap_or_default();
                        holding.release();
                        let refusal = json!({"error": event["error"]}).to_string().into_bytes();
                        let body = futures::stream::once(async move { Ok(refusal) });
                        return Ok(Response { status: status as u16, status_text: String::new(), headers, body: Some(Box::pin(body)) });
                    }
                    break (text, event);
                }
                Next::Closed if !opened_now => {
                    holding.line = None;
                    continue;
                }
                Next::Aborted => return Err(LanguageModelError::other(ABORTED)),
                Next::Closed => {
                    shared.failed();
                    return Err(retryable("The connection closed before an answer", url));
                }
                Next::Broken(why) => {
                    shared.failed();
                    return Err(retryable(why, url));
                }
            }
        };
        let headers = holding.line.as_ref().map(|line| line.headers.clone()).unwrap_or_default();
        let input = body.get("input").and_then(Value::as_array).cloned().unwrap_or_default();
        let answer = Answer { holding, pending: Some(first), signal, fields: fields(&body), input, url: url.to_string(), ended: false };
        let stream = futures::stream::unfold(answer, Answer::next);
        Ok(Response { status: 200, status_text: "OK".into(), headers, body: Some(Box::pin(stream) as ByteStream) })
    }
}
