//! The client of SoundCraft's control channel: JSON lines on the loopback port the app opens when it's started with
//! `--control` (docs/control-protocol.md in SoundCraft's repository). One request a line, `{"id", "method", "params"}`,
//! and one answer a line, `{"id", "ok": true, "result"}` or `{"id", "ok": false, "error"}`. The channel has a few methods
//! of its own (`engine.commands`, `session.inspect`, the `ui.*` ones, `app.quit`). Every engine command goes inside one
//! of them, `engine.execute {command, params}`, and any other method answers `unknown method`.
//!
//! The app reads a connection's next line only once the request before it has its answer (or its 90 s are up), and takes
//! at most 16 connections from everyone. So a request that outlives its deadline here keeps its connection: the
//! connection waits for the answer the app owes it and throws it away before it takes another request, and meanwhile the
//! next request goes on another connection, up to [`CONNECTIONS`]. Nothing is ever sent twice. A request the app may still
//! run (its deadline passed, the app's own 90 s passed, or the connection went with it out) is uncertain, for the caller
//! to read back.
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::TcpStream;

/// The most connections Kumi opens to one app, which takes 16 from everyone.
pub const CONNECTIONS: usize = 8;
/// The longest a connect takes: the app accepts at once, even while its window is busy.
pub const CONNECT: Duration = Duration::from_millis(800);
/// How long a connection waits for an answer the app owes it before it's closed: past the app's own 90 s, after which
/// the app answers `timeout` itself.
pub const OWED: Duration = Duration::from_secs(95);
/// The longest answer read. A screenshot's is the biggest.
const MAX_ANSWER: usize = 128 << 20;

/// Why a request has no result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Nothing answers on the port: the app isn't running, or runs without its channel.
    Unreachable(String),
    /// The app has as many connections as it takes (16), other programs' among them. Nothing ran.
    Crowded,
    /// Nothing ran: the app's queue is full (16 requests wait for its window), or each connection Kumi may open waits
    /// for an answer the app owes it.
    Busy,
    /// No answer within the request's deadline. The app may still run it.
    Deadline,
    /// The app answered that it waited 90 s for its window to take the request. It's still queued, and may run later.
    Stalled,
    /// The connection closed with the request sent: it may or may not have run.
    Closed,
    /// The app wants a token this client hasn't got, or not this one (`unauthorized: …`), and closed the connection.
    Unauthorized(String),
    /// The app said no, and nothing ran: the command refused, or failed and the app put the document back. Its words.
    Refused(String),
    /// The request reached a part of the app that failed (its handler panicked): whether it ran is unknown.
    Broken(String),
    /// Kumi's fault, or a port that isn't SoundCraft's channel: an answer out of step or not JSON, a method the app hasn't
    /// got, or a line it couldn't read.
    Protocol(String),
}

impl Error {
    /// Whether the request may have run: a write that must never be sent again, and whose effect is read back.
    pub fn uncertain(&self) -> bool {
        matches!(self, Error::Deadline | Error::Stalled | Error::Closed | Error::Broken(_))
    }

    /// The command an `unknown command` refusal names: one this app hasn't got (or no longer has).
    pub fn unknown_command(&self) -> Option<&str> {
        let Error::Refused(said) = self else { return None };
        said.strip_prefix("unknown command `")?.strip_suffix('`')
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Unreachable(why) => write!(f, "SoundCraft's control channel can't be reached: {why}"),
            Error::Crowded => f.write_str("SoundCraft has as many control connections as it takes"),
            Error::Busy => f.write_str("SoundCraft is busy: it takes no more requests now"),
            Error::Deadline => f.write_str("SoundCraft didn't answer in time"),
            Error::Stalled => f.write_str("SoundCraft's window didn't take the request within 90 s"),
            Error::Closed => f.write_str("SoundCraft closed the connection with a request out"),
            Error::Unauthorized(said) | Error::Refused(said) | Error::Broken(said) | Error::Protocol(said) => f.write_str(said),
        }
    }
}

impl std::error::Error for Error {}

/// What an answer's `error` says, as an [`Error`]. The words are SoundCraft's own: its control server's, its window's
/// (control.rs) and its engine's errors.
fn refusal(said: &str) -> Error {
    match said {
        "timeout" => Error::Stalled,
        "control queue full" => Error::Busy,
        "too many control connections" => Error::Crowded,
        "control request failed" => Error::Broken(said.into()),
        _ if said.starts_with("unauthorized") => Error::Unauthorized(said.into()),
        _ if ["unknown method `", "bad JSON", "request line longer than"].iter().any(|start| said.starts_with(start)) => {
            Error::Protocol(said.into())
        }
        _ => Error::Refused(said.into()),
    }
}

/// Kumi's connections to one running SoundCraft's control channel. Clones share them.
#[derive(Clone)]
pub struct Control(Arc<Pool>);

struct Pool {
    address: SocketAddr,
    token: Option<String>,
    connections: usize,
    owed: Duration,
    idle: Mutex<Vec<Channel>>,
    /// Connections open: idle, in use, or waiting for an answer the app owes.
    open: AtomicUsize,
    ids: AtomicU64,
}

impl Control {
    /// The channel on `port` (what the app printed as `SOUNDCRAFT_CONTROL_PORT`). Each request carries `token` when
    /// there is one (the app's `SOUNDCRAFT_CONTROL_TOKEN`): a SoundCraft that checks tokens needs it, and one that doesn't
    /// ignores it.
    pub fn new(port: u16, token: Option<String>) -> Self {
        Self::with_limits(port, token, CONNECTIONS, OWED)
    }

    /// As [`Control::new`], with at most `connections` open, and an owed answer waited for `owed`.
    pub fn with_limits(port: u16, token: Option<String>, connections: usize, owed: Duration) -> Self {
        Self(Arc::new(Pool {
            address: SocketAddr::from(([127, 0, 0, 1], port)),
            token,
            connections,
            owed,
            idle: Mutex::default(),
            open: AtomicUsize::new(0),
            ids: AtomicU64::new(0),
        }))
    }

    /// How many connections are open: idle, in use, or waiting for an answer the app owes.
    pub fn open(&self) -> usize {
        self.0.open.load(Ordering::SeqCst)
    }

    /// One of the channel's own methods (`session.inspect`, `ui.inspect`, `engine.commands`…): its result.
    pub async fn call(&self, method: &str, params: Value, deadline: Duration) -> Result<Value, Error> {
        result_of(self.exchange(method, params, deadline).await?)
    }

    /// An engine command, sent as `engine.execute {command, params}`: its result.
    pub async fn execute(&self, command: &str, params: Value, deadline: Duration) -> Result<Value, Error> {
        self.call("engine.execute", json!({"command": command, "params": params}), deadline).await
    }

    /// One request and the app's whole answer to it, less its id, as the app wrote it: a refusal (`"ok": false`) too.
    pub async fn exchange(&self, method: &str, params: Value, deadline: Duration) -> Result<Value, Error> {
        let until = Instant::now() + deadline;
        let mut channel = self.channel(until).await?;
        let id = self.0.ids.fetch_add(1, Ordering::Relaxed) + 1;
        let mut request = json!({"id": id, "method": method, "params": params});
        if let Some(token) = &self.0.token {
            request["token"] = token.as_str().into();
        }
        match tokio::time::timeout(until.saturating_duration_since(Instant::now()), channel.send(&request)).await {
            Ok(Ok(())) => {}
            Ok(Err(_)) => {
                self.close(channel);
                return Err(Error::Closed);
            }
            // (part of the line went: the connection can't take another, and the app may yet read it whole)
            Err(_) => {
                self.close(channel);
                return Err(Error::Deadline);
            }
        }
        let line = match tokio::time::timeout(until.saturating_duration_since(Instant::now()), channel.read_line()).await {
            Err(_) => {
                self.owe(channel, id);
                return Err(Error::Deadline);
            }
            Ok(Ok(Some(line))) => line,
            Ok(Ok(None)) => {
                self.close(channel);
                return Err(Error::Closed);
            }
            Ok(Err(error)) => {
                self.close(channel);
                return Err(error);
            }
        };
        let Ok(Value::Object(mut answer)) = serde_json::from_slice::<Value>(&line) else {
            self.close(channel);
            return Err(Error::Protocol(format!("an answer that isn't a JSON object: {}", String::from_utf8_lossy(&line))));
        };
        match answer.remove("id") {
            Some(said) if said == json!(id) => {
                // (the app closes a connection whose request had no token, or the wrong one)
                if answer.get("error").and_then(Value::as_str).is_some_and(|said| said.starts_with("unauthorized")) {
                    self.close(channel);
                } else {
                    self.give_back(channel);
                }
                Ok(Value::Object(answer))
            }
            // A line the app answers without an id is one it couldn't take as a request: too many connections (it closes
            // this one), or a line it couldn't read (Kumi's fault).
            None | Some(Value::Null) => {
                self.close(channel);
                let said = answer.get("error").and_then(Value::as_str);
                Err(said
                    .map(refusal)
                    .unwrap_or_else(|| Error::Protocol(format!("an answer with no id: {}", Value::Object(answer.clone())))))
            }
            Some(other) => {
                self.close(channel);
                Err(Error::Protocol(format!("an answer out of step: request {id} was answered as {other}")))
            }
        }
    }

    /// An idle connection that still works, else a new one, unless as many are open as Kumi may open.
    async fn channel(&self, until: Instant) -> Result<Channel, Error> {
        loop {
            let idle = self.0.idle.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).pop();
            match idle {
                Some(channel) if channel.sound() => return Ok(channel),
                Some(gone) => self.close(gone),
                None => break,
            }
        }
        if self.0.open.fetch_add(1, Ordering::SeqCst) >= self.0.connections {
            self.0.open.fetch_sub(1, Ordering::SeqCst);
            return Err(Error::Busy);
        }
        let within = CONNECT.min(until.saturating_duration_since(Instant::now()));
        Channel::open(self.0.address, within).await.inspect_err(|_| {
            self.0.open.fetch_sub(1, Ordering::SeqCst);
        })
    }

    fn give_back(&self, channel: Channel) {
        self.0.idle.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push(channel);
    }

    fn close(&self, channel: Channel) {
        drop(channel);
        self.0.open.fetch_sub(1, Ordering::SeqCst);
    }

    /// A connection whose request outlived its deadline waits for the answer the app owes it, throws it away, and goes
    /// back to work. One whose answer doesn't come within the owed wait, or comes out of step, is closed.
    fn owe(&self, mut channel: Channel, id: u64) {
        let control = self.clone();
        tokio::spawn(async move {
            let owed = tokio::time::timeout(control.0.owed, channel.read_line()).await;
            let answered = |line: &[u8]| serde_json::from_slice::<Value>(line).is_ok_and(|answer| answer.get("id") == Some(&json!(id)));
            match owed {
                Ok(Ok(Some(line))) if answered(&line) => control.give_back(channel),
                _ => control.close(channel),
            }
        });
    }
}

/// An answer's result, or why it has none.
fn result_of(answer: Value) -> Result<Value, Error> {
    let Value::Object(mut answer) = answer else { return Err(Error::Protocol(format!("an answer that isn't an object: {answer}"))) };
    match answer.get("ok").and_then(Value::as_bool) {
        Some(true) => Ok(answer.remove("result").unwrap_or(Value::Null)),
        Some(false) => Err(refusal(answer.get("error").and_then(Value::as_str).unwrap_or("an error with no words"))),
        None => Err(Error::Protocol(format!("an answer with no `ok`: {}", Value::Object(answer)))),
    }
}

/// One connection to the app.
struct Channel {
    reader: BufReader<OwnedReadHalf>,
    writer: OwnedWriteHalf,
    /// An answer's line as far as it has come: a read a deadline cut short goes on from here.
    line: Vec<u8>,
}

impl Channel {
    async fn open(address: SocketAddr, within: Duration) -> Result<Self, Error> {
        let stream = match tokio::time::timeout(within, TcpStream::connect(address)).await {
            Ok(Ok(stream)) => stream,
            Ok(Err(error)) => return Err(Error::Unreachable(format!("nothing answers on {address}: {error}"))),
            Err(_) => return Err(Error::Unreachable(format!("nothing answered on {address} within {} ms", within.as_millis()))),
        };
        let _ = stream.set_nodelay(true);
        let (read, write) = stream.into_split();
        Ok(Self { reader: BufReader::new(read), writer: write, line: Vec::new() })
    }

    /// Whether an idle connection can take a request: nothing has come on it since its last answer, and the app hasn't
    /// closed it (as it does when it quits).
    fn sound(&self) -> bool {
        if !self.line.is_empty() || !self.reader.buffer().is_empty() {
            return false;
        }
        let mut byte = [0u8; 1];
        matches!(self.reader.get_ref().try_read(&mut byte), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
    }

    async fn send(&mut self, request: &Value) -> std::io::Result<()> {
        let mut line = serde_json::to_vec(request).expect("a request is JSON");
        line.push(b'\n');
        self.writer.write_all(&line).await
    }

    /// The next answer's line, without its newline; None once the app has closed the connection. Cut short (a deadline),
    /// what has come of the line is kept for the next read.
    async fn read_line(&mut self) -> Result<Option<Vec<u8>>, Error> {
        loop {
            let room = (MAX_ANSWER + 1).saturating_sub(self.line.len()) as u64;
            let read = (&mut self.reader).take(room).read_until(b'\n', &mut self.line).await.map_err(|_| Error::Closed)?;
            if self.line.last() == Some(&b'\n') {
                let mut line = std::mem::take(&mut self.line);
                line.pop();
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                return Ok(Some(line));
            }
            if self.line.len() > MAX_ANSWER {
                return Err(Error::Protocol(format!("an answer longer than {} MiB", MAX_ANSWER >> 20)));
            }
            if read == 0 {
                return Ok(None);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_apps_words_say_whether_a_request_may_have_run() {
        let cases = [
            ("timeout", Error::Stalled, true),
            ("control queue full", Error::Busy, false),
            ("too many control connections", Error::Crowded, false),
            ("control request failed", Error::Broken("control request failed".into()), true),
            ("unauthorized: wrong token", Error::Unauthorized("unauthorized: wrong token".into()), false),
            ("unknown method `mix.volume`", Error::Protocol("unknown method `mix.volume`".into()), false),
            ("bad JSON: EOF while parsing", Error::Protocol("bad JSON: EOF while parsing".into()), false),
            (
                "`edit.cut` is not available: nothing selected",
                Error::Refused("`edit.cut` is not available: nothing selected".into()),
                false,
            ),
        ];
        for (said, error, uncertain) in cases {
            assert_eq!(refusal(said), error, "{said}");
            assert_eq!(error.uncertain(), uncertain, "{said}");
        }
        assert_eq!(refusal("unknown command `mix.nope`").unknown_command(), Some("mix.nope"));
        assert_eq!(refusal("bad parameters for `mix.volume`: missing db").unknown_command(), None);
    }
}
