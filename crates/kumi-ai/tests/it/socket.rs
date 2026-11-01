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
use serde_json::{json, Value};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::mpsc,
};
use tokio_tungstenite::tungstenite::{
    handshake::server::{Request, Response as Opening},
    Message,
};

/// What the server answers each `response.create` with, in order: the events of one answer.
type Script = Vec<Vec<Value>>;

/// A WebSocket server that answers each message with the next answer's events. It reports every message it got, and
/// the headers of each connection's opening.
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
                let socket = tokio_tungstenite::accept_hdr_async(stream, |request: &Request, answer: Opening| {
                    let headers = request.headers().iter().map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string())).collect();
                    heard.lock().unwrap().push(headers);
                    Ok(answer)
                });
                let mut socket = socket.await.unwrap();
                while let Some(Ok(Message::Text(text))) = socket.next().await {
                    let _ = sent.send(serde_json::from_str::<Value>(text.as_str()).unwrap());
                    let answer = script.lock().unwrap().next().unwrap_or_default();
                    for event in answer {
                        socket.send(Message::text(event.to_string())).await.unwrap();
                    }
                }
            });
        }
    });
    Server { url, messages, openings }
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

#[test]
fn a_streamed_request_goes_over_the_connection_and_its_events_come_back_as_http_would_stream_them() {
    local(async {
        let mut server = server(vec![completed("r1", json!([]))]).await;
        let http = Rc::new(Http::default());
        let fetch = SocketFetch::new(http.clone());
        let response = fetch.fetch(&server.url, post(request(json!([user("hi")])))).await.unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(events(response).await, completed("r1", json!([])));
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
        assert!(http.0.borrow().is_empty());
    });
}

#[test]
fn a_request_that_carries_on_from_the_last_response_sends_only_whats_new_on_the_same_connection() {
    local(async {
        let mut server = server(vec![
            completed("r1", json!([{"type": "reasoning", "id": "rs1"}, call("c1")])),
            completed("r2", json!([{"type": "message", "role": "assistant", "content": []}])),
            completed("r3", json!([])),
        ])
        .await;
        let fetch = SocketFetch::new(Rc::new(Http::default()));
        let first = vec![user("set the tempo")];
        events(fetch.fetch(&server.url, post(request(json!(first)))).await.unwrap()).await;
        // The model's reasoning and call as Kumi sends them back, then the call's result.
        let mut second = first.clone();
        second.extend([json!({"type": "reasoning", "encrypted_content": "…", "summary": []}), call("c1"), result("c1")]);
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
fn a_response_the_connection_no_longer_has_is_asked_for_again_with_the_whole_request() {
    local(async {
        let lost = json!({"type": "error", "status": 400, "error": {"code": "previous_response_not_found", "message": "gone"}});
        let mut server = server(vec![completed("r1", json!([call("c1")])), vec![lost], completed("r2", json!([]))]).await;
        let fetch = SocketFetch::new(Rc::new(Http::default()));
        events(fetch.fetch(&server.url, post(request(json!([user("go")])))).await.unwrap()).await;
        let whole = json!([user("go"), call("c1"), result("c1")]);
        assert_eq!(events(fetch.fetch(&server.url, post(request(whole.clone()))).await.unwrap()).await, completed("r2", json!([])));
        server.messages.recv().await.unwrap();
        assert_eq!(server.messages.recv().await.unwrap()["previous_response_id"], "r1");
        let again = server.messages.recv().await.unwrap();
        assert_eq!((again.get("previous_response_id"), &again["input"]), (None, &whole));
    });
}

#[test]
fn a_refusal_with_a_status_is_answered_as_http_answers_it() {
    local(async {
        let limited = json!({"type": "error", "status": 429, "error": {"code": "rate_limit_exceeded", "message": "Slow down"}});
        let server = server(vec![vec![limited], completed("r1", json!([]))]).await;
        let fetch = SocketFetch::new(Rc::new(Http::default()));
        let refused = fetch.fetch(&server.url, post(request(json!([user("hi")])))).await.unwrap();
        assert_eq!(refused.status, 429);
        assert_eq!(refused.json().await.unwrap(), json!({"error": {"code": "rate_limit_exceeded", "message": "Slow down"}}));
        // The connection is still good for the next request.
        assert_eq!(events(fetch.fetch(&server.url, post(request(json!([user("hi")])))).await.unwrap()).await, completed("r1", json!([])));
        assert_eq!(server.openings.lock().unwrap().len(), 1);
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
        let models = server.url.replace("/responses", "/models");
        for (url, init) in [
            (server.url.as_str(), post(quiet)),
            (server.url.as_str(), post(background)),
            (models.as_str(), post(request(json!([user("hi")])))),
            (server.url.as_str(), FetchInit { method: "GET".into(), ..Default::default() }),
        ] {
            let response = fetch.fetch(url, init).await.unwrap();
            assert_eq!(response.headers["x-over"], "http");
        }
        assert_eq!(http.0.borrow().len(), 4);
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
fn a_server_that_wont_open_a_connection_leaves_every_request_to_http() {
    local(async {
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
                let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n").await;
            }
        });
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
