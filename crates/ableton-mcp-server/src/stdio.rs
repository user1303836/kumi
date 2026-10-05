//! `serve_stdio` spawns a task per request with `tokio::task::spawn_local`, so it runs inside a
//! `tokio::task::LocalSet` on a current-thread runtime, as every binary does.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::future::poll_fn;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Poll, Waker};

use futures::future::{FutureExt, LocalBoxFuture, Shared};
use kumi_common::abort::{Controller, Signal};
use kumi_common::js::{json, number, string};
use serde_json::{json as json_value, Value};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::oneshot;
use tokio::task::{spawn_local, JoinHandle};

use crate::framing::{FrameError, FrameEvent, NdjsonFramer};

/// What a handler answers: the record to write, nothing (a notification has no answer), or a failure
/// (a `throw`), which is answered with `-32603 Internal error`.
pub type HandlerOutcome = Result<Option<String>, String>;

/// `RecordHandler`: the context comes with a request (a record with an id), not with a notification.
pub type RecordHandler = Rc<dyn Fn(String, Option<RecordContext>) -> LocalBoxFuture<'static, HandlerOutcome>>;

/// A JSON-RPC request id the transport tracks: a string of 1 to 128 UTF-16 units or a safe integer.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RequestId {
    String(String),
    Number(i64),
}

impl RequestId {
    /// The id as it is written back in a frame.
    pub fn to_value(&self) -> Value {
        match self {
            RequestId::String(text) => Value::String(text.clone()),
            RequestId::Number(n) => Value::from(*n),
        }
    }

    /// `requestKey`: `string:<id>` or `number:<id>`.
    fn key(&self) -> String {
        match self {
            RequestId::String(text) => format!("string:{text}"),
            RequestId::Number(n) => format!("number:{n}"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct RecordContext {
    pub request_id: RequestId,
    pub signal: Signal,
}

/// The server-initiated emitter a `notifier` is given.
pub type Notify = Rc<dyn Fn(String) -> LocalBoxFuture<'static, Result<(), StdioError>>>;

#[derive(Default)]
pub struct StdioOptions {
    /// Maximum number of handler calls that may be active at once.
    pub max_in_flight: Option<usize>,
    /// Register a server-initiated emitter (used for event notifications).
    pub notifier: Option<Box<dyn FnOnce(Notify)>>,
    /// Cooperative termination: checked after each input chunk; when true the
    /// read loop ends cleanly so pending responses flush before return.
    pub should_stop: Option<Box<dyn Fn() -> bool>>,
}

/// What `serveStdio` threw: its message.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct StdioError(pub String);

fn request_key(value: Option<&Value>) -> Option<RequestId> {
    match value? {
        Value::String(text) => {
            let length = string::utf16_len(text);
            (length > 0 && length <= 128).then(|| RequestId::String(text.clone()))
        }
        Value::Number(n) => {
            let value = n.as_f64()?;
            number::is_safe_integer(value).then_some(RequestId::Number(value as i64))
        }
        _ => None,
    }
}

fn cancellation_target(value: &Value) -> Option<String> {
    let record = value.as_object()?;
    if record.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
        || record.contains_key("id")
        || record.get("method").and_then(Value::as_str) != Some("notifications/cancelled")
    {
        return None;
    }
    let params = record.get("params")?.as_object()?;
    if record.keys().any(|key| !["jsonrpc", "method", "params"].contains(&key.as_str())) {
        return None;
    }
    if params.keys().any(|key| !["requestId", "reason", "_meta"].contains(&key.as_str()))
        || params.get("reason").is_some_and(|reason| !reason.is_string())
        || params.get("_meta").is_some_and(|meta| !meta.is_object())
    {
        return None;
    }
    Some(request_key(params.get("requestId"))?.key())
}

fn request_id(value: &Value) -> Option<RequestId> {
    request_key(value.as_object()?.get("id"))
}

fn error_frame(id: Value, code: i64, message: &str) -> String {
    json::stringify(&json_value!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }))
}

type WriteFuture = LocalBoxFuture<'static, Result<(), StdioError>>;

struct Pending {
    #[allow(dead_code)]
    id: RequestId,
    task: JoinHandle<Result<(), StdioError>>,
}

struct State {
    input: Option<Pin<Box<dyn AsyncRead>>>,
    output: Option<Pin<Box<dyn AsyncWrite>>>,
    controllers: HashMap<String, Rc<Controller>>,
    pending: HashMap<u64, Pending>,
    next_sequence: u64,
    active: usize,
    waiters: VecDeque<oneshot::Sender<()>>,
    closed: bool,
    queued_writes: usize,
    write_tail: Shared<LocalBoxFuture<'static, ()>>,
    /// Who is parked on the streams: destroying a stream must wake them, as Node's `close` event
    /// ended a pending read or write.
    read_waker: Option<Waker>,
    write_waker: Option<Waker>,
}

struct Server {
    max_in_flight: usize,
    max_pending: usize,
    max_queued_writes: usize,
    handler: RecordHandler,
    state: RefCell<State>,
}

impl Server {
    async fn acquire(self: &Rc<Self>) {
        let waiter = {
            let mut state = self.state.borrow_mut();
            if state.active < self.max_in_flight {
                state.active += 1;
                return;
            }
            let (sender, receiver) = oneshot::channel();
            state.waiters.push_back(sender);
            receiver
        };
        let _ = waiter.await;
        self.state.borrow_mut().active += 1;
    }

    fn release(&self) {
        let mut state = self.state.borrow_mut();
        state.active -= 1;
        if let Some(waiter) = state.waiters.pop_front() {
            let _ = waiter.send(());
        }
    }

    async fn write_raw(self: Rc<Self>, value: String) -> Result<(), StdioError> {
        if self.state.borrow().output.is_none() {
            return Err(StdioError("output unavailable".to_string()));
        }
        let line = format!("{value}\n");
        let mut written = 0;
        while written < line.len() {
            let count = poll_fn(|cx| {
                let mut state = self.state.borrow_mut();
                let polled = match state.output.as_mut() {
                    None => return Poll::Ready(Err(StdioError("output closed".to_string()))),
                    Some(output) => {
                        output.as_mut().poll_write(cx, &line.as_bytes()[written..]).map_err(|cause| StdioError(cause.to_string()))
                    }
                };
                if polled.is_pending() {
                    state.write_waker = Some(cx.waker().clone());
                }
                polled
            })
            .await?;
            if count == 0 {
                return Err(StdioError("output write failed".to_string()));
            }
            written += count;
        }
        poll_fn(|cx| {
            let mut state = self.state.borrow_mut();
            let polled = match state.output.as_mut() {
                None => return Poll::Ready(Err(StdioError("output closed".to_string()))),
                Some(output) => output.as_mut().poll_flush(cx).map_err(|cause| StdioError(cause.to_string())),
            };
            if polled.is_pending() {
                state.write_waker = Some(cx.waker().clone());
            }
            polled
        })
        .await
    }

    /// Queues one line behind the writes before it. The error is the synchronous throw of the
    /// TypeScript (a saturated queue); the future is the write itself.
    fn write(self: &Rc<Self>, value: String, begin_write: Option<Box<dyn FnOnce() -> bool>>) -> Result<WriteFuture, StdioError> {
        let mut state = self.state.borrow_mut();
        if state.queued_writes >= self.max_queued_writes {
            return Err(StdioError("bounded output queue is saturated".to_string()));
        }
        state.queued_writes += 1;
        let previous = state.write_tail.clone();
        let server = self.clone();
        let result = async move {
            previous.await;
            // This is the emission boundary, not merely admission to the write queue.
            // Do not yield between the final ownership/cancellation check and writeRaw.
            let outcome = match begin_write {
                Some(begin) => {
                    if begin() {
                        server.clone().write_raw(value).await
                    } else {
                        Ok(())
                    }
                }
                None => server.clone().write_raw(value).await,
            };
            server.state.borrow_mut().queued_writes -= 1;
            outcome
        }
        .boxed_local()
        .shared();
        state.write_tail = result.clone().map(|_| ()).boxed_local().shared();
        drop(state);
        Ok(result.boxed_local())
    }

    /// `failOutput`: the connection's work authority is gone; every request is aborted and both
    /// streams are destroyed.
    fn fail_output(&self) {
        let mut state = self.state.borrow_mut();
        if state.closed {
            return;
        }
        state.closed = true;
        for controller in state.controllers.values() {
            controller.abort();
        }
        state.input = None;
        state.output = None;
        if let Some(waker) = state.read_waker.take() {
            waker.wake();
        }
        if let Some(waker) = state.write_waker.take() {
            waker.wake();
        }
    }

    /// `notify`: admission to the write queue happens as the emitter is called (an async function
    /// runs to its first `await` at once); the future is the write itself.
    fn notify(self: &Rc<Self>, value: String) -> WriteFuture {
        match self.write(value, None) {
            Ok(write) => {
                let server = self.clone();
                async move {
                    let outcome = write.await;
                    if outcome.is_err() {
                        server.fail_output();
                    }
                    outcome
                }
                .boxed_local()
            }
            Err(cause) => {
                self.fail_output();
                futures::future::ready(Err(cause)).boxed_local()
            }
        }
    }

    async fn process(self: &Rc<Self>, event: FrameEvent) -> Result<(), StdioError> {
        if self.state.borrow().closed {
            return Ok(());
        }
        let value = match event {
            FrameEvent::Error(kind) => {
                let oversized = kind == FrameError::Oversized;
                let frame = error_frame(
                    Value::Null,
                    if oversized { -32600 } else { -32700 },
                    if oversized { "Message exceeds size limit" } else { "Parse error" },
                );
                return self.write(frame, None)?.await;
            }
            FrameEvent::Record(value) => value,
        };
        let parsed: Option<Value> = serde_json::from_str(&value).ok();
        if let Some(target) = parsed.as_ref().and_then(cancellation_target) {
            let controller = self.state.borrow().controllers.get(&target).cloned();
            if let Some(controller) = controller {
                controller.abort();
            }
            return Ok(());
        }
        let Some(id) = parsed.as_ref().and_then(request_id) else {
            let handled: Result<(), ()> = async {
                let result = (self.handler)(value, None).await.map_err(|_| ())?;
                if let Some(result) = result {
                    self.write(result, None).map_err(|_| ())?.await.map_err(|_| ())?;
                }
                Ok(())
            }
            .await;
            if handled.is_err() {
                self.write(error_frame(Value::Null, -32603, "Internal error"), None)?.await?;
            }
            return Ok(());
        };
        if self.state.borrow().pending.len() >= self.max_pending {
            // Never stop reading the control stream behind saturated work: doing so
            // would strand cancellation notifications behind the request they must
            // abort. Refuse excess work immediately and keep the bounded control
            // plane responsive.
            // Queue the bounded busy response without blocking input consumption, so
            // a following cancellation can still abort its matching in-flight work
            // even while stdout is backpressured.
            let busy = error_frame(id.to_value(), -32000, "Server is busy; retry after in-flight work completes");
            match self.write(busy, None) {
                Ok(write) => {
                    let server = self.clone();
                    spawn_local(async move {
                        if write.await.is_err() {
                            server.fail_output();
                        }
                    });
                }
                // A peer that supplies more response-producing requests than the
                // bounded output queue can hold loses the connection's work
                // authority immediately; no mutation may remain stranded behind it.
                Err(_) => self.fail_output(),
            }
            return Ok(());
        }
        let key = id.key();
        let sequence = {
            let mut state = self.state.borrow_mut();
            let sequence = state.next_sequence;
            state.next_sequence += 1;
            sequence
        };
        // Each answer is written as soon as its work is done (JSON-RPC matches answers to requests by id), so
        // a slow request, such as a big Set's export, doesn't hold up the answers to the requests after it.
        let duplicate = self.state.borrow().controllers.contains_key(&key);
        let server = self.clone();
        let task = if duplicate {
            let frame = error_frame(id.to_value(), -32600, "Duplicate in-flight request identifier");
            spawn_local(async move { server.delivered(sequence, key, Some(frame), None).await })
        } else {
            let controller = Rc::new(Controller::new());
            self.state.borrow_mut().controllers.insert(key.clone(), controller.clone());
            let request = id.clone();
            spawn_local(async move {
                let result = server.run_request(value, request, &controller).await;
                server.delivered(sequence, key, result, Some(controller)).await
            })
        };
        self.state.borrow_mut().pending.insert(sequence, Pending { id, task });
        Ok(())
    }

    /// The handler stage of a request: bounded by `maxInFlight`, silent once cancelled, and a
    /// `-32603` answer when the handler throws.
    async fn run_request(self: &Rc<Self>, value: String, id: RequestId, controller: &Controller) -> Option<String> {
        self.acquire().await;
        let signal = controller.signal.clone();
        let result = if signal.is_cancelled() {
            None
        } else {
            let context = RecordContext { request_id: id.clone(), signal: signal.clone() };
            match (self.handler)(value, Some(context)).await {
                Ok(result) if !signal.is_cancelled() => result,
                Err(_) if !signal.is_cancelled() => Some(error_frame(id.to_value(), -32603, "Internal error")),
                _ => None,
            }
        };
        self.release();
        result
    }

    /// `delivered`: the answer is emitted, a failed emission closes the output, and the request
    /// leaves the pending set.
    async fn delivered(
        self: Rc<Self>,
        sequence: u64,
        key: String,
        result: Option<String>,
        controller: Option<Rc<Controller>>,
    ) -> Result<(), StdioError> {
        let outcome = self.emit(key, result, controller).await;
        if outcome.is_err() {
            self.fail_output();
        }
        self.state.borrow_mut().pending.remove(&sequence);
        outcome
    }

    async fn emit(self: &Rc<Self>, key: String, result: Option<String>, controller: Option<Rc<Controller>>) -> Result<(), StdioError> {
        // A client may already have reused this ID while the old write's callback/drain is pending; never
        // remove that newer controller.
        let retire_controller: Rc<dyn Fn()> = {
            let server = self.clone();
            let key = key.clone();
            let controller = controller.clone();
            Rc::new(move || {
                if let Some(controller) = &controller {
                    let mut state = server.state.borrow_mut();
                    if state.controllers.get(&key).is_some_and(|current| Rc::ptr_eq(current, controller)) {
                        state.controllers.remove(&key);
                    }
                }
            })
        };
        let closed = self.state.borrow().closed;
        let aborted = controller.as_ref().is_some_and(|controller| controller.signal.is_cancelled());
        let outcome = match result {
            Some(result) if !closed && !aborted => {
                let begin_write: Box<dyn FnOnce() -> bool> = {
                    let server = self.clone();
                    let controller = controller.clone();
                    let retire_controller = retire_controller.clone();
                    Box::new(move || {
                        // An earlier write (including a busy reply) can delay emission long after the handler
                        // completes. Cancellation still owns that window.
                        if server.state.borrow().closed || controller.as_ref().is_some_and(|controller| controller.signal.is_cancelled()) {
                            return false;
                        }
                        // Retire before output.write can synchronously expose the response. Callback
                        // completion/backpressure must not delay sequential reuse.
                        retire_controller();
                        true
                    })
                };
                match self.write(result, Some(begin_write)) {
                    Ok(write) => write.await,
                    Err(cause) => Err(cause),
                }
            }
            _ => Ok(()),
        };
        retire_controller();
        outcome
    }
}

/// Connects a bounded framer to streams and observes Writable backpressure.
pub async fn serve_stdio<I, O>(input: I, output: O, handler: RecordHandler, options: StdioOptions) -> Result<(), StdioError>
where
    I: AsyncRead + 'static,
    O: AsyncWrite + 'static,
{
    let max_in_flight = options.max_in_flight.unwrap_or(16);
    if !(1..=64).contains(&max_in_flight) {
        return Err(StdioError("maxInFlight must be an integer from 1 to 64".to_string()));
    }
    let max_pending = max_in_flight * 4;
    let max_queued_writes = max_pending * 4;
    let mut framer = NdjsonFramer::new();
    let server = Rc::new(Server {
        max_in_flight,
        max_pending,
        max_queued_writes,
        handler,
        state: RefCell::new(State {
            input: Some(Box::pin(input)),
            output: Some(Box::pin(output)),
            controllers: HashMap::new(),
            pending: HashMap::new(),
            next_sequence: 0,
            active: 0,
            waiters: VecDeque::new(),
            closed: false,
            queued_writes: 0,
            write_tail: futures::future::ready(()).boxed_local().shared(),
            read_waker: None,
            write_waker: None,
        }),
    });
    if let Some(notifier) = options.notifier {
        let emitter = server.clone();
        notifier(Rc::new(move |value| emitter.notify(value)));
    }
    let should_stop = options.should_stop;
    let run = async {
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            let read = poll_fn(|cx| {
                let mut state = server.state.borrow_mut();
                match state.input.as_mut() {
                    // The input was destroyed under the read loop, as `failOutput` does.
                    None => Poll::Ready(Err(StdioError("Premature close".to_string()))),
                    Some(input) => {
                        let mut read_buffer = ReadBuf::new(&mut buffer);
                        match input.as_mut().poll_read(cx, &mut read_buffer) {
                            Poll::Pending => {
                                state.read_waker = Some(cx.waker().clone());
                                Poll::Pending
                            }
                            Poll::Ready(Ok(())) => Poll::Ready(Ok(read_buffer.filled().len())),
                            Poll::Ready(Err(cause)) => Poll::Ready(Err(StdioError(cause.to_string()))),
                        }
                    }
                }
            })
            .await?;
            if read == 0 {
                break;
            }
            for event in framer.push(&buffer[..read]) {
                server.process(event).await?;
                // `await process(event)` gave the work it started a turn before the next record is
                // read, so a request's handler runs (and listens for cancellation) before the
                // notification that cancels it can be seen.
                tokio::task::yield_now().await;
            }
            if should_stop.as_ref().is_some_and(|should_stop| should_stop()) {
                break;
            }
        }
        for event in framer.end() {
            server.process(event).await?;
            tokio::task::yield_now().await;
        }
        let tasks: Vec<JoinHandle<Result<(), StdioError>>> =
            server.state.borrow_mut().pending.drain().map(|(_, entry)| entry.task).collect();
        for task in tasks {
            if let Ok(Err(cause)) = task.await {
                return Err(cause);
            }
        }
        let tail = server.state.borrow().write_tail.clone();
        tail.await;
        Ok(())
    };
    let outcome = run.await;
    if outcome.is_err() {
        server.fail_output();
    }
    {
        let mut state = server.state.borrow_mut();
        state.closed = true;
        for controller in state.controllers.values() {
            controller.abort();
        }
        state.controllers.clear();
        state.pending.clear();
        for waiter in state.waiters.drain(..) {
            let _ = waiter.send(());
        }
    }
    outcome
}
