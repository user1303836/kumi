//! Responses over a WebSocket, as ChatGPT serves them. One connection stays open beside the conversation and each
//! streamed `POST …/responses` goes over it as a `response.create` message. A request that carries on from the
//! response before it (the same instructions, tools and options; its input the last one's, then the model's output as
//! the request sends it back, then something new) sends only what's new, with `previous_response_id`: the connection
//! keeps that response, `store: false` as it is. The events come back as the server-sent events HTTP would have
//! carried, so nothing else changes. Anything else goes over HTTP as before: a proxy in the way, a request while one is
//! on the connection, and, for a while, a server that won't open one or connections that keep failing.
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
    future::Future,
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
/// A connection lasts an hour at most: past this one is opened again before a request, so an answer begun on it has ten
/// minutes left rather than being cut off.
const LIFETIME: Duration = Duration::from_secs(50 * 60);
/// As HTTP's: opening gives up after 10 s, and an answer that goes quiet for 300 s has stopped.
const CONNECT: Duration = Duration::from_secs(10);
const QUIET: Duration = Duration::from_secs(300);
/// The server says it has the request at once (`response.created`): a connection used before that doesn't within this
/// went quiet while it waited (a sleeping laptop, a NAT that forgot it), and the request goes on a new one.
const FIRST: Duration = Duration::from_secs(15);
/// A request bigger than this (pictures, say) goes over HTTP: a message goes as one frame.
const MOST_BYTES: usize = 8 * 1024 * 1024;
/// Connections that failed in a row before HTTP takes over for a while, and how long.
const FAILURES: u8 = 2;
const REST: Duration = Duration::from_secs(10 * 60);
/// The most connections one request opens (one that had its hour, then one more).
const OPENINGS: u8 = 2;
const ABORTED: &str = "This operation was aborted";

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

struct Line {
    socket: Socket,
    opened: Instant,
    /// The headers it was opened with: who's asking (the sign-in, the account, the conversation). A request with others
    /// (a refreshed token, another conversation) opens a connection of its own.
    identity: Headers,
    /// The opening's answer (the plan's usage, as HTTP's answers carry it), for the first answer on the connection
    /// only: given to every answer, a meter read at the opening would stand in for newer ones.
    usage: Option<Headers>,
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
    /// HTTP until then: the server wouldn't open one, or connections kept failing.
    resting: Cell<Option<Instant>>,
}

impl Shared {
    fn resting(&self) -> bool {
        self.resting.get().is_some_and(|until| Instant::now() < until)
    }
    fn rest(&self) {
        self.resting.set(Some(Instant::now() + REST));
        self.failures.set(0);
    }
    fn failed(&self) {
        self.failures.set(self.failures.get() + 1);
        if self.failures.get() >= FAILURES {
            self.rest();
        }
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

/// A message's words, whether its content is text or parts.
fn words(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts.iter().filter_map(|part| part["text"].as_str()).collect(),
        _ => String::new(),
    }
}

/// What an output item holds, as the server made it and as the next request sends it back: the call (its id, name and
/// arguments, as JSON: Kumi writes back the input it read, not the model's spacing), the reasoning (its encrypted
/// content), the message's words.
fn gist(item: &Value) -> Option<Value> {
    match item["type"].as_str() {
        Some("reasoning") => Some(json!(["reasoning", item["encrypted_content"]])),
        Some("function_call") => {
            let arguments = item["arguments"].as_str().and_then(|text| serde_json::from_str::<Value>(text).ok());
            Some(json!(["call", item["call_id"], item["name"], arguments.unwrap_or_else(|| item["arguments"].clone())]))
        }
        Some("message") | None if item["role"] == "assistant" => Some(json!(["message", words(&item["content"])])),
        _ => None,
    }
}

/// What's new in the request after `last`: its input, then each of its output items as the request sends them back,
/// then at least one more item. None when the request doesn't carry on from it (Kumi cleared or dropped earlier parts
/// of the conversation, say, or its instructions changed), and the whole request goes.
fn carried_on(last: &Last, body: &Map<String, Value>) -> Option<Vec<Value>> {
    let input = body.get("input")?.as_array()?;
    let (asked, made) = (last.input.len(), last.output.len());
    if made == 0 || fields(body) != last.fields || input.len() <= asked + made || input[..asked] != last.input[..] {
        return None;
    }
    let sent_back = &input[asked..asked + made];
    let same = sent_back.iter().zip(&last.output).all(|(sent, made)| gist(sent).is_some_and(|sent| Some(sent) == gist(made)));
    same.then(|| input[asked + made..].to_vec())
}

/// The headers a connection is opened with: the request's own, without those of its body.
fn identity(headers: &Headers) -> Headers {
    headers
        .iter()
        .filter(|(name, _)| !matches!(name.as_str(), "content-type" | "content-length" | "accept" | "openai-beta"))
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect()
}

/// The answer broke off after it began: retryable, with the status it began with, as a dropped HTTP stream is.
fn broke_off(why: String, url: &str) -> LanguageModelError {
    let mut error = ApiCallError::new("Failed to process successful response", url, None, Some(200));
    error.is_retryable = true;
    error.cause = Some(why);
    error.into()
}

fn aborted() -> LanguageModelError {
    LanguageModelError::other(ABORTED)
}

async fn open(url: &str, identity: &Headers) -> Result<Line, tungstenite::Error> {
    let address = match url.split_once("://") {
        Some(("https", rest)) => format!("wss://{rest}"),
        Some(("http", rest)) => format!("ws://{rest}"),
        _ => url.to_string(),
    };
    let mut request = address.into_client_request()?;
    for (name, value) in identity {
        if let (Ok(name), Ok(value)) = (tungstenite::http::HeaderName::try_from(name.as_str()), HeaderValue::from_str(value)) {
            request.headers_mut().insert(name, value);
        }
    }
    request.headers_mut().insert("openai-beta", HeaderValue::from_static(BETA));
    let connector = Connector::Rustls(Arc::new(kumi_net::http1_tls()));
    let opening = tokio_tungstenite::connect_async_tls_with_config(request, None, true, Some(connector));
    let (socket, answer) =
        tokio::time::timeout(CONNECT, opening).await.map_err(|_| tungstenite::Error::Io(std::io::ErrorKind::TimedOut.into()))??;
    let usage = answer.headers().iter().filter_map(|(k, v)| v.to_str().ok().map(|v| (k.as_str().to_string(), v.to_string()))).collect();
    Ok(Line { socket, opened: Instant::now(), identity: identity.clone(), usage: Some(usage) })
}

enum Next {
    Event(String, Value),
    /// The connection ended, broke or went quiet: why.
    Gone(String),
    Aborted,
}

/// `work`, unless the request is cancelled first (None).
async fn unless_aborted<T>(signal: &Signal, work: impl Future<Output = T>) -> Option<T> {
    tokio::select! {
        biased;
        _ = signal.cancelled() => None,
        done = work => Some(done),
    }
}

/// The connection's next event, waiting at most `wait`.
async fn next_event(socket: &mut Socket, signal: &Signal, wait: Duration) -> Next {
    loop {
        let Some(message) = unless_aborted(signal, tokio::time::timeout(wait, socket.next())).await else { return Next::Aborted };
        match message {
            Err(_) => return Next::Gone("The answer went quiet".into()),
            Ok(None | Some(Ok(Message::Close(_)))) => return Next::Gone("The connection closed".into()),
            Ok(Some(Ok(Message::Text(text)))) => {
                let event = serde_json::from_str(text.as_str()).unwrap_or(Value::Null);
                return Next::Event(text.as_str().to_string(), event);
            }
            Ok(Some(Ok(_))) => continue,
            Ok(Some(Err(error))) => return Next::Gone(error.to_string()),
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

/// The connection while a request has it: given back once its answer ends, or let go with it (cancelled, broken off,
/// or after an error, in case the server says more).
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

/// The answer's stream, and what it keeps to remember its response for the next request.
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
                let next = match next_event(&mut line.socket, &self.signal, QUIET).await {
                    Next::Event(text, event) => Ok((text, event)),
                    Next::Aborted => Err(aborted()),
                    Next::Gone(why) => Err(broke_off(why, &self.url)),
                };
                match next {
                    Ok(next) => next,
                    Err(error) => {
                        // The connection goes with the answer.
                        self.holding.line = None;
                        self.ended = true;
                        return Some((Err(error), self));
                    }
                }
            }
        };
        if terminal(&event) {
            self.ended = true;
            let shared = self.holding.shared.clone();
            shared.failures.set(0);
            if event["type"] == "error" {
                self.holding.line = None;
            } else {
                let response = &event["response"];
                *shared.last.borrow_mut() = response["id"].as_str().filter(|_| event["type"] == "response.completed").map(|id| Last {
                    fields: std::mem::take(&mut self.fields),
                    input: std::mem::take(&mut self.input),
                    id: id.to_string(),
                    output: response["output"].as_array().cloned().unwrap_or_default(),
                });
                self.holding.release();
            }
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
        let body = streamed(url, &init).filter(|_| direct && !shared.resting() && !shared.busy.get());
        let Some(body) = body else { return self.http.fetch(url, init).await };
        shared.busy.set(true);
        let mut holding = Holding { shared: shared.clone(), line: shared.line.borrow_mut().take(), released: false };
        let signal = init.signal.clone().unwrap_or_default();
        let identity = identity(&init.headers);
        let mut carry = shared.last.borrow_mut().take().and_then(|last| carried_on(&last, &body).map(|rest| (last.id, rest)));
        let full = init.body.as_deref().map_or(0, str::len);
        let mut openings = 0;
        let (first, usage) = loop {
            let reused = holding.line.as_ref().is_some_and(|line| line.opened.elapsed() < LIFETIME && line.identity == identity);
            if !reused {
                // A new connection keeps no earlier response.
                holding.line = None;
                carry = None;
                if openings == OPENINGS {
                    drop(holding);
                    return self.http.fetch(url, init).await;
                }
                openings += 1;
                let Some(opened) = unless_aborted(&signal, open(url, &identity)).await else { return Err(aborted()) };
                match opened {
                    Ok(line) => holding.line = Some(line),
                    Err(error) => {
                        // A server that won't take one rests it at once; a refused sign-in, a busy server, a redirect
                        // (a captive portal) or a network that failed counts.
                        match error {
                            tungstenite::Error::Http(answer) if !matches!(answer.status().as_u16(), 300..=399 | 401 | 403 | 408 | 429 | 500..=599) => {
                                shared.rest()
                            }
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
            let sending = tokio::time::timeout(QUIET, line.socket.send(Message::text(text)));
            let Some(sent) = unless_aborted(&signal, sending).await else { return Err(aborted()) };
            let next = match sent {
                Ok(Ok(())) => next_event(&mut line.socket, &signal, if reused { FIRST } else { QUIET }).await,
                Ok(Err(error)) => Next::Gone(error.to_string()),
                Err(_) => Next::Gone("The request couldn't be sent".into()),
            };
            match next {
                Next::Event(text, event) => {
                    let code = event["error"]["code"].as_str().unwrap_or("");
                    // The connection no longer has that response: the whole request again.
                    if event["type"] == "error" && code == "previous_response_not_found" && carry.is_some() {
                        carry = None;
                        continue;
                    }
                    // The connection had its hour: the whole request on a new one.
                    if event["type"] == "error" && code == "websocket_connection_limit_reached" {
                        holding.line = None;
                        continue;
                    }
                    crate::timing::sent_instead(full, size);
                    let usage = holding.line.as_mut().and_then(|line| line.usage.take()).unwrap_or_default();
                    // Refused outright, with HTTP's status: answered as HTTP answers, so it's told and retried the same.
                    // The connection goes, in case the server says more.
                    if let Some(status) = event["status"].as_u64().filter(|status| event["type"] == "error" && (400..600).contains(status))
                    {
                        drop(holding);
                        let refusal = json!({"error": event["error"]}).to_string().into_bytes();
                        let body = futures::stream::once(async move { Ok(refusal) });
                        return Ok(Response {
                            status: status as u16,
                            status_text: String::new(),
                            headers: usage,
                            body: Some(Box::pin(body)),
                        });
                    }
                    break ((text, event), usage);
                }
                Next::Aborted => return Err(aborted()),
                // A connection used before ended or went quiet while it waited: the whole request on a new one.
                Next::Gone(_) if reused => {
                    holding.line = None;
                    continue;
                }
                // One just opened that fails: this request over HTTP.
                Next::Gone(_) => {
                    shared.failed();
                    drop(holding);
                    return self.http.fetch(url, init).await;
                }
            }
        };
        let input = body.get("input").and_then(Value::as_array).cloned().unwrap_or_default();
        let answer = Answer { holding, pending: Some(first), signal, fields: fields(&body), input, url: url.to_string(), ended: false };
        let stream = futures::stream::unfold(answer, Answer::next);
        Ok(Response { status: 200, status_text: "OK".into(), headers: usage, body: Some(Box::pin(stream) as ByteStream) })
    }
}
