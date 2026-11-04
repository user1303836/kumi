//! A stand-in for SoundCraft's control server, as v0.17.0's behaves (apps/soundcraft/src/control_server.rs): one request
//! at a time on a connection, at most `connections` of them (the one past that is told so, with no id, and closed), and
//! a line that isn't JSON answered `bad JSON` with no id. Each request's answer is the test's to give: at once, late,
//! out of step, never, or the connection closed instead.
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

/// What the fake answers a request: `{ok, result | error}`, to which it adds the request's id.
pub enum Answer {
    Now(Value),
    After(Duration, Value),
    /// Answered with another request's id.
    OutOfStep(Value),
    /// Answered, then the connection closed (as the app does to a request without the right token).
    ThenClose(Value),
    /// Never answered: as on a window that never takes the request, nothing more is read on the connection, which ends
    /// when the client closes it.
    Never,
}

pub fn ok(result: Value) -> Value {
    json!({"ok": true, "result": result})
}

pub fn refused(error: &str) -> Value {
    json!({"ok": false, "error": error})
}

type Answers = Arc<dyn Fn(&Value) -> Answer + Send + Sync>;

#[derive(Default)]
struct Seen {
    requests: Mutex<Vec<Value>>,
    /// Connections taken, ever.
    connections: AtomicUsize,
    /// Connections open now.
    open: AtomicUsize,
}

pub struct Fake {
    port: u16,
    seen: Arc<Seen>,
    drop_all: tokio::sync::watch::Sender<u64>,
    accepting: tokio::task::JoinHandle<()>,
}

impl Fake {
    pub async fn start(connections: usize, answers: impl Fn(&Value) -> Answer + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = Arc::new(Seen::default());
        let (drop_all, dropped) = tokio::sync::watch::channel(0u64);
        let answers: Answers = Arc::new(answers);
        let shared = seen.clone();
        let accepting = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                if shared.open.load(Ordering::SeqCst) >= connections {
                    let _ = stream.write_all(b"{\"ok\":false,\"error\":\"too many control connections\"}\n").await;
                    continue;
                }
                shared.connections.fetch_add(1, Ordering::SeqCst);
                shared.open.fetch_add(1, Ordering::SeqCst);
                let (seen, answers, mut dropped) = (shared.clone(), answers.clone(), dropped.clone());
                dropped.mark_unchanged();
                tokio::spawn(async move {
                    tokio::select! {
                        _ = serve(stream, &seen, &answers) => {}
                        _ = dropped.changed() => {}
                    }
                    seen.open.fetch_sub(1, Ordering::SeqCst);
                });
            }
        });
        Self { port, seen, drop_all, accepting }
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Quits, as the app does: no connection is taken any more, and those open close.
    pub fn quit(&self) {
        self.accepting.abort();
        self.drop_connections();
    }

    /// Every request read, in order.
    pub fn requests(&self) -> Vec<Value> {
        self.seen.requests.lock().unwrap().clone()
    }

    /// Connections taken, ever.
    pub fn connections(&self) -> usize {
        self.seen.connections.load(Ordering::SeqCst)
    }

    /// Connections open now.
    pub fn open(&self) -> usize {
        self.seen.open.load(Ordering::SeqCst)
    }

    /// Closes every connection open now, as an app that quits does. New ones are taken as before.
    pub fn drop_connections(&self) {
        self.drop_all.send_modify(|n| *n += 1);
    }
}

async fn serve(stream: TcpStream, seen: &Seen, answers: &Answers) {
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        let request: Value = match serde_json::from_str(&line) {
            Ok(request) => request,
            Err(error) => {
                let _ = write.write_all(format!("{}\n", json!({"ok": false, "error": format!("bad JSON: {error}")})).as_bytes()).await;
                continue;
            }
        };
        seen.requests.lock().unwrap().push(request.clone());
        let id = request.get("id").cloned().unwrap_or(Value::Null);
        let with_id = |mut answer: Value, id: Value| {
            answer["id"] = id;
            format!("{answer}\n")
        };
        let (line, close) = match answers(&request) {
            Answer::Now(answer) => (with_id(answer, id), false),
            Answer::After(wait, answer) => {
                tokio::time::sleep(wait).await;
                (with_id(answer, id), false)
            }
            Answer::OutOfStep(answer) => (with_id(answer, json!(id.as_u64().unwrap_or(0) + 1000)), false),
            Answer::ThenClose(answer) => (with_id(answer, id), true),
            Answer::Never => {
                while let Ok(Some(_)) = lines.next_line().await {}
                return;
            }
        };
        if write.write_all(line.as_bytes()).await.is_err() || close {
            return;
        }
    }
}
