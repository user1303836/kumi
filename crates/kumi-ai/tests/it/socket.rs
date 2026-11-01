//! ChatGPT's Responses over a WebSocket: what goes over the connection, what's carried on from the response before,
//! and what's left to HTTP. A server on this computer stands in for ChatGPT's.

use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use kumi_ai::{
    error::LanguageModelError,
    http::{Fetch, FetchInit, Headers, Response},
    socket::SocketFetch,
    sse::json_stream,
};
use kumi_common::abort::Signal;
use serde_json::{json, Value};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::mpsc,
};
use tokio_tungstenite::tungstenite::{
    handshake::server::{Request, Response as Opening},
    http::HeaderValue,
    Message,
};

/// What the server answers each `response.create` with, in order: the events of one answer. "close" in an answer closes
/// the connection there; "hold" says no more and keeps it open.
type Script = Vec<Vec<Value>>;

/// A WebSocket server that answers each message with the next answer's events. It reports every message it got, and
/// the headers of each connection's opening; its opening's answer says the plan is 60% used.
struct Server {
    url: String,
    messages: mpsc::UnboundedReceiver<Value>,
    openings: Arc<Mutex<Vec<Vec<(String, String)>>>>,
}

async fn server(script: Script) -> Server {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let url = format!("http://{}/backend-api/codex/responses", listener.local_addr().unwrap());
    let (sent, messages) = mpsc::unbounded_channel();
    let openings = Arc::new(Mutex::new(vec![]));
    let heard = openings.clone();
    tokio::spawn(async move {
        let script = Arc::new(Mutex::new(script.into_iter()));
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let (sent, heard, script) = (sent.clone(), heard.clone(), script.clone());
            tokio::spawn(async move {
                let socket = tokio_tungstenite::accept_hdr_async(stream, |request: &Request, mut answer: Opening| {
                    let headers = request.headers().iter().map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string())).collect();
                    heard.lock().unwrap().push(headers);
                    answer.headers_mut().insert("x-codex-primary-used-percent", HeaderValue::from_static("60"));
                    Ok(answer)
                });
                let mut socket = socket.await.unwrap();
                while let Some(Ok(message)) = socket.next().await {
                    // A ping is answered as it's read.
                    let Message::Text(text) = message else { continue };
                    let _ = sent.send(serde_json::from_str::<Value>(text.as_str()).unwrap());
                    let answer = script.lock().unwrap().next().unwrap_or_default();
                    for event in answer {
                        if event == "close" {
                            return;
                        }
                        if event == "hold" {
                            std::future::pending::<()>().await;
                        }
                        socket.send(Message::text(event.to_string())).await.unwrap();
                    }
                }
            });
        }
    });
    Server { url, messages, openings }
}

/// A server that takes each connection and answers its opening with `answer` (raw HTTP, or nothing at all), counting
/// the openings.
async fn refusing(answer: &'static [u8]) -> (String, Arc<Mutex<usize>>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let url = format!("http://{}/backend-api/codex/responses", listener.local_addr().unwrap());
    let tries = Arc::new(Mutex::new(0));
    let counted = tries.clone();
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            *counted.lock().unwrap() += 1;
            let mut bytes = [0; 4096];
            let _ = stream.read(&mut bytes).await;
            let _ = stream.write_all(answer).await;
        }
    });
    (url, tries)
}

/// HTTP, for what the connection doesn't carry: it records each request's URL and answers with one event.
#[derive(Default)]
struct Http(RefCell<Vec<String>>);
#[async_trait(?Send)]
impl Fetch for Http {
    async fn fetch(&self, url: &str, _: FetchInit) -> Result<Response, LanguageModelError> {
        self.0.borrow_mut().push(url.to_string());
        let mut response = Response::text_response(200, "data: {\"type\":\"over http\"}\n\n");
        response.headers.insert("x-over".into(), "http".into());
        Ok(response)
    }
}

fn post(body: Value) -> FetchInit {
    let headers = Headers::from([
        ("authorization".into(), "Bearer token".into()),
        ("content-type".into(), "application/json".into()),
        ("openai-beta".into(), "responses=experimental".into()),
        ("session-id".into(), "s1".into()),
    ]);
    FetchInit { method: "POST".into(), headers, body: Some(body.to_string()), signal: None }
}

fn request(input: Value) -> Value {
    json!({"model": "gpt-test", "instructions": "Be brief.", "tools": [], "store": false, "stream": true, "input": input})
}

/// The answer's events, as the Responses client reads them.
async fn events(response: Response) -> Vec<Value> {
    json_stream(response.body.unwrap()).map(Result::unwrap).collect().await
}

fn completed(id: &str, output: Value) -> Vec<Value> {
    vec![
        json!({"type": "response.created", "response": {"id": id}}),
        json!({"type": "response.completed", "response": {"id": id, "output": output}}),
    ]
}

fn user(text: &str) -> Value {
    json!({"role": "user", "content": [{"type": "input_text", "text": text}]})
}

fn call(id: &str) -> Value {
    json!({"type": "function_call", "call_id": id, "name": "tempo", "arguments": "{}"})
}

fn result(id: &str) -> Value {
    json!({"type": "function_call_output", "call_id": id, "output": "done"})
}

fn local<F: std::future::Future>(work: F) -> F::Output {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    tokio::task::LocalSet::new().block_on(&runtime, work)
}

/// A first request that leaves a call to answer, and the next one that answers it.
fn a_call_then_its_result() -> (Value, Value) {
    (request(json!([user("go")])), request(json!([user("go"), call("c1"), result("c1")])))
}

#[test]
fn a_streamed_request_goes_over_the_connection_and_its_events_come_back_as_http_would_stream_them() {
    local(async {
        let mut server = server(vec![completed("r1", json!([])), completed("r2", json!([]))]).await;
        let http = Rc::new(Http::default());
        let fetch = SocketFetch::new(http.clone());
        let response = fetch.fetch(&server.url, post(request(json!([user("hi")])))).await.unwrap();
        assert_eq!(response.status, 200);
        // The plan's usage from the opening goes with its first answer only: read later, it would be out of date.
        assert_eq!(response.headers.get("x-codex-primary-used-percent").map(String::as_str), Some("60"));
        assert_eq!(events(response).await, completed("r1", json!([])));
        let again = fetch.fetch(&server.url, post(request(json!([user("hi again")])))).await.unwrap();
        assert_eq!(again.headers.get("x-codex-primary-used-percent"), None);
        events(again).await;
        let message = server.messages.recv().await.unwrap();
        assert_eq!(message["type"], "response.create");
        assert_eq!(message.get("stream"), None, "a message has no stream flag");
        assert_eq!(message["input"], json!([user("hi")]));
        let opening = server.openings.lock().unwrap()[0].clone();
        let header = |name: &str| opening.iter().find(|(key, _)| key == name).map(|(_, value)| value.clone());
        assert_eq!(header("authorization").as_deref(), Some("Bearer token"));
        assert_eq!(header("session-id").as_deref(), Some("s1"));
        assert_eq!(header("openai-beta").as_deref(), Some("responses_websockets=2026-02-06"));
        assert_eq!(header("content-type"), None);
        assert_eq!(server.openings.lock().unwrap().len(), 1);
        assert!(http.0.borrow().is_empty());
    });
}

#[test]
fn a_request_that_carries_on_from_the_last_response_sends_only_whats_new_on_the_same_connection() {
    local(async {
        // The model's arguments as it spaced them; Kumi sends back the input it read from them.
        let spaced =
            json!({"type": "function_call", "id": "fc1", "call_id": "c1", "name": "tempo", "arguments": "{ }", "status": "completed"});
        let mut server = server(vec![
            completed("r1", json!([{"type": "reasoning", "id": "rs1", "encrypted_content": "sealed", "summary": []}, spaced])),
            completed("r2", json!([{"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "Done."}]}])),
            completed("r3", json!([])),
        ])
        .await;
        let fetch = SocketFetch::new(Rc::new(Http::default()));
        let first = vec![user("set the tempo")];
        events(fetch.fetch(&server.url, post(request(json!(first)))).await.unwrap()).await;
        // The model's reasoning and call as Kumi sends them back, then the call's result.
        let mut second = first.clone();
        second.extend([json!({"type": "reasoning", "encrypted_content": "sealed", "summary": []}), call("c1"), result("c1")]);
        events(fetch.fetch(&server.url, post(request(json!(second)))).await.unwrap()).await;
        // Other instructions: not the same conversation's next step, so the whole request.
        let mut third = second.clone();
        third.extend([json!({"role": "assistant", "content": "Done."}), user("thanks")]);
        let mut changed = request(json!(third));
        changed["instructions"] = json!("Be thorough.");
        events(fetch.fetch(&server.url, post(changed)).await.unwrap()).await;
        let sent: Vec<Value> = [server.messages.recv().await, server.messages.recv().await, server.messages.recv().await]
            .into_iter()
            .map(Option::unwrap)
            .collect();
        assert_eq!(sent[0].get("previous_response_id"), None);
        assert_eq!(sent[1]["previous_response_id"], "r1");
        assert_eq!(sent[1]["input"], json!([result("c1")]));
        assert_eq!(sent[2].get("previous_response_id"), None);
        assert_eq!(sent[2]["input"], json!(third));
        assert_eq!(server.openings.lock().unwrap().len(), 1, "one connection for all three");
    });
}

#[test]
fn output_sent_back_otherwise_than_the_model_made_it_sends_the_whole_request() {
    local(async {
        let mut server = server(vec![completed("r1", json!([call("c1")])), completed("r2", json!([]))]).await;
        let fetch = SocketFetch::new(Rc::new(Http::default()));
        let (first, _) = a_call_then_its_result();
        events(fetch.fetch(&server.url, post(first)).await.unwrap()).await;
        // The same call, its arguments changed: the server's copy would be another context than Kumi's.
        let changed = json!({"type": "function_call", "call_id": "c1", "name": "tempo", "arguments": "{\"bpm\":90}"});
        let whole = json!([user("go"), changed, result("c1")]);
        events(fetch.fetch(&server.url, post(request(whole.clone()))).await.unwrap()).await;
        server.messages.recv().await.unwrap();
        let again = server.messages.recv().await.unwrap();
        assert_eq!((again.get("previous_response_id"), &again["input"]), (None, &whole));
    });
}

#[test]
fn another_sign_in_or_conversation_opens_a_connection_of_its_own() {
    local(async {
        let mut server = server(vec![completed("r1", json!([call("c1")])), completed("r2", json!([])), completed("r3", json!([]))]).await;
        let fetch = SocketFetch::new(Rc::new(Http::default()));
        let (first, next) = a_call_then_its_result();
        events(fetch.fetch(&server.url, post(first)).await.unwrap()).await;
        let mut refreshed = post(next);
        refreshed.headers.insert("authorization".into(), "Bearer newer".into());
        events(fetch.fetch(&server.url, refreshed).await.unwrap()).await;
        let mut elsewhere = post(request(json!([user("hello")])));
        elsewhere.headers.insert("authorization".into(), "Bearer newer".into());
        elsewhere.headers.insert("session-id".into(), "s2".into());
        events(fetch.fetch(&server.url, elsewhere).await.unwrap()).await;
        server.messages.recv().await.unwrap();
        // A new connection keeps no earlier response: the whole request.
        assert_eq!(server.messages.recv().await.unwrap().get("previous_response_id"), None);
        let openings = server.openings.lock().unwrap().clone();
        let header = |opening: &Vec<(String, String)>, name: &str| opening.iter().find(|(key, _)| key == name).unwrap().1.clone();
        let seen: Vec<(String, String)> = openings.iter().map(|o| (header(o, "authorization"), header(o, "session-id"))).collect();
        let expected = [("Bearer token", "s1"), ("Bearer newer", "s1"), ("Bearer newer", "s2")];
        assert_eq!(seen, expected.map(|(a, s)| (a.to_string(), s.to_string())));
    });
}

#[test]
fn a_response_the_connection_no_longer_has_is_asked_for_again_with_the_whole_request() {
    local(async {
        let lost = json!({"type": "error", "status": 400, "error": {"code": "previous_response_not_found", "message": "gone"}});
        let mut server = server(vec![completed("r1", json!([call("c1")])), vec![lost], completed("r2", json!([]))]).await;
        let fetch = SocketFetch::new(Rc::new(Http::default()));
        let (first, next) = a_call_then_its_result();
        events(fetch.fetch(&server.url, post(first)).await.unwrap()).await;
        assert_eq!(events(fetch.fetch(&server.url, post(next.clone())).await.unwrap()).await, completed("r2", json!([])));
        server.messages.recv().await.unwrap();
        assert_eq!(server.messages.recv().await.unwrap()["previous_response_id"], "r1");
        let again = server.messages.recv().await.unwrap();
        assert_eq!((again.get("previous_response_id"), &again["input"]), (None, &next["input"]));
        assert_eq!(server.openings.lock().unwrap().len(), 2, "on a new connection, in case the old one says more");
    });
}

#[test]
fn reasoning_or_words_sent_back_otherwise_send_the_whole_request() {
    let reasoning = |sealed: &str| json!({"type": "reasoning", "encrypted_content": sealed, "summary": []});
    let said = json!({"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "Done."}]});
    for (made, sent_back) in [(reasoning("sealed"), reasoning("resealed")), (said, json!({"role": "assistant", "content": "Done!"}))] {
        local(async {
            let mut server = server(vec![completed("r1", json!([made])), completed("r2", json!([]))]).await;
            let fetch = SocketFetch::new(Rc::new(Http::default()));
            events(fetch.fetch(&server.url, post(request(json!([user("go")])))).await.unwrap()).await;
            let whole = json!([user("go"), sent_back, user("and then")]);
            events(fetch.fetch(&server.url, post(request(whole.clone()))).await.unwrap()).await;
            server.messages.recv().await.unwrap();
            let again = server.messages.recv().await.unwrap();
            assert_eq!((again.get("previous_response_id"), &again["input"]), (None, &whole));
        });
    }
}

#[test]
fn a_connection_unused_for_a_while_is_pinged_first_and_one_that_doesnt_answer_gives_way() {
    let quick = |http| SocketFetch::new(http).with_timing(Duration::ZERO, Duration::from_millis(300), Duration::from_secs(600));
    // It answers: the request carries on, on the same connection.
    local(async {
        let mut server = server(vec![completed("r1", json!([call("c1")])), completed("r2", json!([]))]).await;
        let fetch = quick(Rc::new(Http::default()));
        let (first, next) = a_call_then_its_result();
        events(fetch.fetch(&server.url, post(first)).await.unwrap()).await;
        events(fetch.fetch(&server.url, post(next)).await.unwrap()).await;
        server.messages.recv().await.unwrap();
        assert_eq!(server.messages.recv().await.unwrap()["previous_response_id"], "r1");
        assert_eq!(server.openings.lock().unwrap().len(), 1);
    });
    // It went deaf after its answer (a sleeping laptop, a NAT that forgot it): the whole request on a new one.
    local(async {
        let mut deaf = completed("r1", json!([call("c1")]));
        deaf.push(json!("hold"));
        let mut server = server(vec![deaf, completed("r2", json!([]))]).await;
        let fetch = quick(Rc::new(Http::default()));
        let (first, next) = a_call_then_its_result();
        events(fetch.fetch(&server.url, post(first)).await.unwrap()).await;
        assert_eq!(events(fetch.fetch(&server.url, post(next.clone())).await.unwrap()).await, completed("r2", json!([])));
        server.messages.recv().await.unwrap();
        let again = server.messages.recv().await.unwrap();
        assert_eq!((again.get("previous_response_id"), &again["input"]), (None, &next["input"]));
        assert_eq!(server.openings.lock().unwrap().len(), 2);
    });
}

#[test]
fn a_connection_opened_with_the_plan_read_high_is_opened_again_to_read_it_anew() {
    local(async {
        // The server's opening says 60% used: past 60%, read again (here at once).
        let server = server(vec![completed("r1", json!([])), completed("r2", json!([]))]).await;
        let fetch = SocketFetch::new(Rc::new(Http::default())).with_timing(Duration::from_secs(30), Duration::from_secs(5), Duration::ZERO);
        events(fetch.fetch(&server.url, post(request(json!([user("one")])))).await.unwrap()).await;
        let again = fetch.fetch(&server.url, post(request(json!([user("two")])))).await.unwrap();
        assert_eq!(again.headers.get("x-codex-primary-used-percent").map(String::as_str), Some("60"));
        events(again).await;
        assert_eq!(server.openings.lock().unwrap().len(), 2);
    });
}

#[test]
fn a_cancelled_request_opens_or_sends_nothing_and_leaves_the_connection_to_the_next() {
    local(async {
        let server = server(vec![completed("r1", json!([])), completed("r2", json!([]))]).await;
        let fetch = SocketFetch::new(Rc::new(Http::default()));
        let cancelled = || {
            let signal = Signal::new();
            signal.cancel();
            let mut init = post(request(json!([user("hi")])));
            init.signal = Some(signal);
            init
        };
        // Before the opening.
        assert!(fetch.fetch(&server.url, cancelled()).await.err().unwrap().to_string().contains("aborted"));
        assert!(server.openings.lock().unwrap().is_empty());
        events(fetch.fetch(&server.url, post(request(json!([user("hi")])))).await.unwrap()).await;
        // Before the sending, on the connection that's open: it goes, in case it was half sent.
        assert!(fetch.fetch(&server.url, cancelled()).await.err().unwrap().to_string().contains("aborted"));
        assert_eq!(events(fetch.fetch(&server.url, post(request(json!([user("hi")])))).await.unwrap()).await, completed("r2", json!([])));
        assert_eq!(server.openings.lock().unwrap().len(), 2);
    });
}

#[test]
fn the_hour_s_limit_on_a_connection_just_opened_leaves_the_request_to_http() {
    local(async {
        let limit = json!({"type": "error", "status": 400, "error": {"code": "websocket_connection_limit_reached", "message": "limit"}});
        let server = server(vec![completed("r1", json!([call("c1")])), vec![limit.clone()], vec![limit]]).await;
        let http = Rc::new(Http::default());
        let fetch = SocketFetch::new(http.clone());
        let (first, next) = a_call_then_its_result();
        events(fetch.fetch(&server.url, post(first)).await.unwrap()).await;
        // The used connection's limit opens one more; the limit again on that one isn't its hour: HTTP, after two.
        let response = fetch.fetch(&server.url, post(next)).await.unwrap();
        assert_eq!(response.headers["x-over"], "http");
        assert_eq!(server.openings.lock().unwrap().len(), 2);
        assert_eq!(http.0.borrow().len(), 1);
    });
}

#[test]
fn a_refused_sign_in_is_answered_as_http_answers_it_and_the_connection_goes() {
    local(async {
        let refused = json!({"type": "error", "status": 401, "error": {"code": "invalid_token", "message": "Sign in again"}});
        let server = server(vec![vec![refused], completed("r1", json!([]))]).await;
        let fetch = SocketFetch::new(Rc::new(Http::default()));
        let answer = fetch.fetch(&server.url, post(request(json!([user("hi")])))).await.unwrap();
        assert_eq!(answer.status, 401);
        assert_eq!(events(fetch.fetch(&server.url, post(request(json!([user("hi")])))).await.unwrap()).await, completed("r1", json!([])));
        assert_eq!(server.openings.lock().unwrap().len(), 2);
    });
}

#[test]
fn a_connection_that_had_its_hour_or_closed_while_unused_is_opened_again_for_the_whole_request() {
    local(async {
        let limit =
            json!({"type": "error", "status": 400, "error": {"code": "websocket_connection_limit_reached", "message": "60 minutes"}});
        let mut server = server(vec![
            completed("r1", json!([call("c1")])),
            vec![limit],
            completed("r2", json!([call("c2")])),
            vec![json!("close")],
            completed("r3", json!([])),
        ])
        .await;
        let http = Rc::new(Http::default());
        let fetch = SocketFetch::new(http.clone());
        let (first, next) = a_call_then_its_result();
        events(fetch.fetch(&server.url, post(first)).await.unwrap()).await;
        assert_eq!(events(fetch.fetch(&server.url, post(next.clone())).await.unwrap()).await, completed("r2", json!([call("c2")])));
        let mut last = next["input"].as_array().unwrap().clone();
        last.extend([call("c2"), result("c2")]);
        assert_eq!(events(fetch.fetch(&server.url, post(request(json!(last.clone())))).await.unwrap()).await, completed("r3", json!([])));
        let sent: Vec<Value> = (0..5).map(|_| server.messages.try_recv().unwrap()).collect();
        assert_eq!(sent[1]["previous_response_id"], "r1");
        assert_eq!((sent[2].get("previous_response_id"), &sent[2]["input"]), (None, &next["input"]));
        assert_eq!(sent[3]["previous_response_id"], "r2");
        assert_eq!((sent[4].get("previous_response_id"), &sent[4]["input"]), (None, &json!(last)));
        assert_eq!(server.openings.lock().unwrap().len(), 3);
        assert!(http.0.borrow().is_empty());
    });
}

#[test]
fn a_refusal_with_a_status_is_answered_as_http_answers_it_and_the_connection_goes() {
    local(async {
        let limited = json!({"type": "error", "status": 429, "error": {"code": "rate_limit_exceeded", "message": "Slow down"}});
        let server = server(vec![vec![limited], completed("r1", json!([]))]).await;
        let fetch = SocketFetch::new(Rc::new(Http::default()));
        let refused = fetch.fetch(&server.url, post(request(json!([user("hi")])))).await.unwrap();
        assert_eq!(refused.status, 429);
        assert_eq!(refused.json().await.unwrap(), json!({"error": {"code": "rate_limit_exceeded", "message": "Slow down"}}));
        assert_eq!(events(fetch.fetch(&server.url, post(request(json!([user("hi")])))).await.unwrap()).await, completed("r1", json!([])));
        assert_eq!(server.openings.lock().unwrap().len(), 2, "a new connection after an error");
    });
}

#[test]
fn a_cancelled_answer_lets_its_connection_go() {
    local(async {
        let started = json!({"type": "response.created", "response": {"id": "r1"}});
        let server = server(vec![vec![started.clone(), json!("hold")], completed("r2", json!([]))]).await;
        let fetch = SocketFetch::new(Rc::new(Http::default()));
        let signal = Signal::new();
        let mut init = post(request(json!([user("hi")])));
        init.signal = Some(signal.clone());
        let mut body = fetch.fetch(&server.url, init).await.unwrap().body.unwrap();
        assert!(body.next().await.unwrap().is_ok());
        signal.cancel();
        assert!(body.next().await.unwrap().unwrap_err().to_string().contains("aborted"));
        assert!(body.next().await.is_none());
        // Whatever the server still had to say can't reach the next request: it goes on a new connection.
        assert_eq!(events(fetch.fetch(&server.url, post(request(json!([user("hi")])))).await.unwrap()).await, completed("r2", json!([])));
        assert_eq!(server.openings.lock().unwrap().len(), 2);
    });
}

#[test]
fn an_answer_that_breaks_off_is_retryable_with_the_status_it_began_with() {
    local(async {
        let started = json!({"type": "response.created", "response": {"id": "r1"}});
        let server = server(vec![vec![started, json!("close")]]).await;
        let fetch = SocketFetch::new(Rc::new(Http::default()));
        let mut body = fetch.fetch(&server.url, post(request(json!([user("hi")])))).await.unwrap().body.unwrap();
        assert!(body.next().await.unwrap().is_ok());
        let error = body.next().await.unwrap().unwrap_err();
        let error = error.api_call().expect("the call as it failed");
        assert_eq!((error.status_code, error.is_retryable), (Some(200), true));
    });
}

#[test]
fn what_the_connection_doesnt_carry_goes_over_http() {
    local(async {
        let server = server(vec![]).await;
        let http = Rc::new(Http::default());
        let fetch = SocketFetch::new(http.clone());
        let mut quiet = request(json!([user("hi")]));
        quiet["stream"] = json!(false);
        let mut background = request(json!([user("hi")]));
        background["background"] = json!(true);
        let big = request(json!([user(&"a".repeat(9 * 1024 * 1024))]));
        let models = server.url.replace("/responses", "/models");
        for (url, init) in [
            (server.url.as_str(), post(quiet)),
            (server.url.as_str(), post(background)),
            (server.url.as_str(), post(big)),
            (models.as_str(), post(request(json!([user("hi")])))),
            (server.url.as_str(), FetchInit { method: "GET".into(), ..Default::default() }),
        ] {
            let response = fetch.fetch(url, init).await.unwrap();
            assert_eq!(response.headers["x-over"], "http");
        }
        assert_eq!(http.0.borrow().len(), 5);
        assert!(server.openings.lock().unwrap().is_empty());
    });
}

#[test]
fn while_one_answer_streams_another_request_goes_over_http() {
    local(async {
        let server = server(vec![completed("r1", json!([]))]).await;
        let http = Rc::new(Http::default());
        let fetch = SocketFetch::new(http.clone());
        let streaming = fetch.fetch(&server.url, post(request(json!([user("one")])))).await.unwrap();
        let aside = fetch.fetch(&server.url, post(request(json!([user("two")])))).await.unwrap();
        assert_eq!(aside.headers["x-over"], "http");
        assert_eq!(events(streaming).await, completed("r1", json!([])));
    });
}

#[test]
fn a_server_that_wont_open_a_connection_leaves_requests_to_http_for_a_while() {
    local(async {
        let (url, tries) = refusing(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n").await;
        let http = Rc::new(Http::default());
        let fetch = SocketFetch::new(http.clone());
        for _ in 0..3 {
            let response = fetch.fetch(&url, post(request(json!([user("hi")])))).await.unwrap();
            assert_eq!(response.headers["x-over"], "http");
        }
        assert_eq!(*tries.lock().unwrap(), 1, "asked once, then left to HTTP");
        assert_eq!(http.0.borrow().len(), 3);
    });
}

#[test]
fn openings_that_fail_twice_in_a_row_leave_requests_to_http_for_a_while() {
    // A refused sign-in, and a network that drops the connection before it opens, count alike.
    for answer in [&b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n"[..], b""] {
        local(async {
            let (url, tries) = refusing(answer).await;
            let http = Rc::new(Http::default());
            let fetch = SocketFetch::new(http.clone());
            for _ in 0..4 {
                let response = fetch.fetch(&url, post(request(json!([user("hi")])))).await.unwrap();
                assert_eq!(response.headers["x-over"], "http");
            }
            assert_eq!(*tries.lock().unwrap(), 2);
            assert_eq!(http.0.borrow().len(), 4);
        });
    }
}
