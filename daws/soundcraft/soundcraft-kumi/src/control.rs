//! The client of SoundCraft's control channel: JSON lines on the loopback port the app opens when it's started with
//! `--control` (docs/control-protocol.md in SoundCraft's repository). One request a line, `{"id", "method", "params"}`,
//! and one answer a line, `{"id", "ok": true, "result"}` or `{"id", "ok": false, "error"}`. The channel has a few methods
//! of its own (`engine.commands`, `session.inspect`, the `ui.*` ones, `app.quit`). Every engine command goes inside one
//! of them, `engine.execute {command, params}`, and any other method answers `unknown method`.
//!
//! The app reads a connection's next line only once the request before it has its answer (or its 90 s are up), and takes
//! at most 16 connections from everyone. So a request that outlives its deadline here keeps its connection: the
//! connection waits for the answer the app owes it and throws it away before it takes another request, and meanwhile the
//! next request goes on another connection, up to [`CONNECTIONS`]. A caller that stops waiting (its future dropped) is the
//! same as a deadline once the request is sent; before that, nothing was sent and the connection is closed. Nothing is
//! ever sent twice. A request the app may still run (its deadline passed, the app's own 90 s passed, the connection went
//! with it out, or its answer came garbled) is uncertain, for the caller to read back.
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
/// The longest request line the app reads (control_server.rs's `MAX_LINE`: a 64 MiB plug-in state in base64, and 1 MiB
/// around it). It closes the connection on a longer one, without reading the rest.
const MAX_REQUEST: usize = (64usize << 20).div_ceil(3) * 4 + (1 << 20);
/// How long a new connection waits before its first request, for a refusal the app sends at once (it has as many
/// connections as it takes). On Windows the reset that follows a refusal throws away what wasn't read yet.
const REFUSAL_WAIT: Duration = Duration::from_millis(10);

/// Why a request has no result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Nothing answers on the port: the app isn't running, or runs without its channel.
    Unreachable(String),
    /// The app has as many connections as it takes (16), other programs' among them. Nothing ran. (Read before the request
    /// goes: a refusal that comes later than [`REFUSAL_WAIT`] may arrive on Windows as [`Error::Closed`].)
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
    /// The app's engine said no: the command refused, or failed and the engine put the document back. Nothing ran. Its
    /// words.
    Refused(String),
    /// The request reached a part of the app that failed part way: its handler panicked, or a gesture (`ui.click`,
    /// `ui.drag`…) was cut off after some of its events. Whether, or how much, it ran is unknown.
    Broken(String),
    /// The request was sent and its answer was garbled: out of step (another request's id), not JSON, or too long. The
    /// app has the request and will run it.
    Garbled(String),
    /// Kumi's fault, proved by the app's own words or before sending: a method the app hasn't got, a line it couldn't read,
    /// a request longer than it reads. Nothing ran.
    Protocol(String),
}

impl Error {
    /// Whether the request may have run: a write that must never be sent again, and whose effect is read back.
    pub fn uncertain(&self) -> bool {
        matches!(self, Error::Deadline | Error::Stalled | Error::Closed | Error::Broken(_) | Error::Garbled(_))
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
            Error::Unauthorized(said) | Error::Refused(said) | Error::Broken(said) | Error::Garbled(said) | Error::Protocol(said) => {
                f.write_str(said)
            }
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
        _ if said.starts_with("synthetic input was not delivered") => Error::Broken(said.into()),
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
    /// Connections open: idle, in use, or waiting for an answer the app owes. Each holds a [`Slot`] of it: the count
    /// alone, never the pool, which holds the idle ones.
    open: Arc<AtomicUsize>,
    ids: AtomicU64,
}

/// One of the connections Kumi may open, held by a connection and given back when the connection closes, however the
/// connection goes. It holds only the count, as SoundCraft's own server's slot does: an idle connection holding its pool
/// would keep the pool, and every connection in it, open once the last `Control` has gone.
struct Slot(Arc<AtomicUsize>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
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
            open: Arc::default(),
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
        let id = self.0.ids.fetch_add(1, Ordering::Relaxed) + 1;
        let mut request = json!({"id": id, "method": method, "params": params});
        if let Some(token) = &self.0.token {
            request["token"] = token.as_str().into();
        }
        let mut line = serde_json::to_vec(&request).expect("a request is JSON");
        line.push(b'\n');
        // (the app's limit is on the line without its newline)
        if line.len() > MAX_REQUEST + 1 {
            return Err(Error::Protocol(format!("a request of {} MiB is longer than SoundCraft reads", line.len() >> 20)));
        }
        // (dropped before its request is sent whole, a connection closes and gives its slot back: the app runs nothing)
        let mut channel = self.channel(until).await?;
        match tokio::time::timeout(until.saturating_duration_since(Instant::now()), channel.writer.write_all(&line)).await {
            Ok(Ok(())) => {}
            Ok(Err(_)) => return Err(Error::Closed),
            // (part of the line went: the connection can't take another, and the app may yet read it whole)
            Err(_) => return Err(Error::Deadline),
        }
        // Sent: from here a connection dropped before its answer (a missed deadline, a caller that stopped waiting) waits
        // for the answer the app owes it.
        let mut flight = InFlight { control: self.clone(), channel: Some(channel), id };
        let reading = flight.channel.as_mut().expect("in flight").read_line();
        let line = match tokio::time::timeout(until.saturating_duration_since(Instant::now()), reading).await {
            Err(_) => return Err(Error::Deadline),
            Ok(Ok(Some(line))) => line,
            Ok(Ok(None)) => {
                flight.channel = None;
                return Err(Error::Closed);
            }
            Ok(Err(error)) => {
                flight.channel = None;
                return Err(error);
            }
        };
        let channel = flight.channel.take().expect("in flight");
        let Ok(Value::Object(mut answer)) = serde_json::from_slice::<Value>(&line) else {
            return Err(Error::Garbled(format!("an answer that isn't a JSON object: {}", String::from_utf8_lossy(&line))));
        };
        match answer.remove("id") {
            Some(said) if said == json!(id) => {
                // (the app closes a connection whose request had no token, or the wrong one)
                if !answer.get("error").and_then(Value::as_str).is_some_and(|said| said.starts_with("unauthorized")) {
                    self.give_back(channel);
                }
                Ok(Value::Object(answer))
            }
            // A line the app answers without an id is one it couldn't take as a request: a line it couldn't read (Kumi's
            // fault). Anything else without an id is garbled.
            None | Some(Value::Null) => match answer.get("error").and_then(Value::as_str).map(refusal) {
                Some(error @ (Error::Protocol(_) | Error::Crowded)) => Err(error),
                _ => Err(Error::Garbled(format!("an answer with no id: {}", Value::Object(answer)))),
            },
            Some(other) => Err(Error::Garbled(format!("an answer out of step: request {id} was answered as {other}"))),
        }
    }

    /// An idle connection that still works, else a new one, unless as many are open as Kumi may open.
    async fn channel(&self, until: Instant) -> Result<Channel, Error> {
        loop {
            let idle = self.0.idle.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).pop();
            match idle {
                Some(channel) if channel.sound() => return Ok(channel),
                Some(_) => {}
                None => break,
            }
        }
        if self.0.open.fetch_add(1, Ordering::SeqCst) >= self.0.connections {
            self.0.open.fetch_sub(1, Ordering::SeqCst);
            return Err(Error::Busy);
        }
        // (from here a connect that fails, or is dropped, gives the slot back)
        let slot = Slot(self.0.open.clone());
        Channel::open(self.0.address, until, slot).await
    }

    fn give_back(&self, channel: Channel) {
        self.0.idle.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push(channel);
    }

    /// A connection whose request outlived its caller's wait waits for the answer the app owes it, throws it away, and
    /// goes back to work. One whose answer doesn't come within the owed wait, or comes out of step, is closed.
    fn owe(&self, mut channel: Channel, id: u64) {
        let control = self.clone();
        tokio::spawn(async move {
            let owed = tokio::time::timeout(control.0.owed, channel.read_line()).await;
            let answered = |line: &[u8]| serde_json::from_slice::<Value>(line).is_ok_and(|answer| answer.get("id") == Some(&json!(id)));
            if let Ok(Ok(Some(line))) = owed {
                if answered(&line) {
                    control.give_back(channel);
                }
            }
        });
    }
}

/// A connection whose request has been sent and not answered yet. Dropped before its answer (a missed deadline, a caller
/// that stopped waiting), it waits for the answer the app owes it ([`Control::owe`]); taken out once answered.
struct InFlight {
    control: Control,
    channel: Option<Channel>,
    id: u64,
}

impl Drop for InFlight {
    fn drop(&mut self) {
        if let Some(channel) = self.channel.take() {
            // (with no runtime left to wait in, the connection just closes)
            if tokio::runtime::Handle::try_current().is_ok() {
                self.control.owe(channel, self.id);
            }
        }
    }
}

/// An answer's result, or why it has none.
fn result_of(answer: Value) -> Result<Value, Error> {
    let Value::Object(mut answer) = answer else { return Err(Error::Garbled(format!("an answer that isn't an object: {answer}"))) };
    match answer.get("ok").and_then(Value::as_bool) {
        Some(true) => Ok(answer.remove("result").unwrap_or(Value::Null)),
        Some(false) => Err(refusal(answer.get("error").and_then(Value::as_str).unwrap_or("an error with no words"))),
        None => Err(Error::Garbled(format!("an answer with no `ok`: {}", Value::Object(answer)))),
    }
}

/// One connection to the app, holding its slot.
struct Channel {
    reader: BufReader<OwnedReadHalf>,
    writer: OwnedWriteHalf,
    /// An answer's line as far as it has come: a read a deadline cut short goes on from here.
    line: Vec<u8>,
    _slot: Slot,
}

impl Channel {
    /// A new connection, made by `until`. An app that has as many connections as it takes refuses it at once and closes it
    /// without reading it, so a refusal is read for a moment before any request is sent: on Windows, the reset that
    /// closing sends would throw it away.
    async fn open(address: SocketAddr, until: Instant, slot: Slot) -> Result<Self, Error> {
        let left = || until.saturating_duration_since(Instant::now());
        let within = CONNECT.min(left());
        let stream = match tokio::time::timeout(within, TcpStream::connect(address)).await {
            Ok(Ok(stream)) => stream,
            Ok(Err(error)) => return Err(Error::Unreachable(format!("nothing answers on {address}: {error}"))),
            Err(_) => return Err(Error::Unreachable(format!("nothing answered on {address} within {} ms", within.as_millis()))),
        };
        let _ = stream.set_nodelay(true);
        let (read, write) = stream.into_split();
        let mut channel = Self { reader: BufReader::new(read), writer: write, line: Vec::new(), _slot: slot };
        if let Ok(Ok(())) = tokio::time::timeout(REFUSAL_WAIT.min(left()), channel.reader.get_ref().readable()).await {
            // Before any request, what comes is a refusal or the connection closing: nothing was sent, so nothing ran.
            match tokio::time::timeout(REFUSAL_WAIT.min(left()), channel.read_line()).await {
                Ok(Ok(Some(line))) => {
                    let said =
                        serde_json::from_slice::<Value>(&line).ok().and_then(|answer| answer.get("error")?.as_str().map(str::to_owned));
                    return Err(match said.as_deref() {
                        Some("too many control connections") => Error::Crowded,
                        _ => Error::Protocol(format!("a line before any request: {}", String::from_utf8_lossy(&line))),
                    });
                }
                Ok(Ok(None) | Err(_)) => return Err(Error::Unreachable(format!("{address} closed the connection at once"))),
                // (readiness the socket didn't have, which tokio allows: nothing came, and the connection is used)
                Err(_) if channel.line.is_empty() => {}
                Err(_) => return Err(Error::Protocol("part of a line before any request".into())),
            }
        }
        Ok(channel)
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
                return Err(Error::Garbled(format!("an answer longer than {} MiB", MAX_ANSWER >> 20)));
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
            (
                "synthetic input was not delivered: no UI pass within 15 s",
                Error::Broken("synthetic input was not delivered: no UI pass within 15 s".into()),
                true,
            ),
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
