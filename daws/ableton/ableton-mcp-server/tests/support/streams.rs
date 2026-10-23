//! Stream doubles for the stdio tests, standing in for the `PassThrough` input and the custom
//! `Writable` outputs the TypeScript tests built: a pipe the test feeds, a writer whose chunks are
//! inspectable and whose writes can be held (backpressure or a late callback) or failed, and the
//! `destroyed` flag Node streams carry.

#![allow(dead_code)]

use std::cell::RefCell;
use std::collections::VecDeque;
use std::io;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use serde_json::Value;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

#[derive(Default)]
struct PipeState {
    buffer: VecDeque<u8>,
    ended: bool,
    destroyed: bool,
    waker: Option<Waker>,
}

/// The writing end of a `PassThrough`, kept by the test.
#[derive(Clone, Default)]
pub struct Pipe(Rc<RefCell<PipeState>>);

/// The reading end, given to `serve_stdio`.
pub struct PipeReader(Rc<RefCell<PipeState>>);

impl Pipe {
    pub fn new() -> (Pipe, PipeReader) {
        let state = Rc::new(RefCell::new(PipeState::default()));
        (Pipe(state.clone()), PipeReader(state))
    }

    /// `input.write(text)`.
    pub fn write(&self, text: &str) {
        let mut state = self.0.borrow_mut();
        state.buffer.extend(text.as_bytes());
        if let Some(waker) = state.waker.take() {
            waker.wake();
        }
    }

    pub fn write_bytes(&self, bytes: &[u8]) {
        let mut state = self.0.borrow_mut();
        state.buffer.extend(bytes);
        if let Some(waker) = state.waker.take() {
            waker.wake();
        }
    }

    /// `input.end()`.
    pub fn end(&self) {
        let mut state = self.0.borrow_mut();
        state.ended = true;
        if let Some(waker) = state.waker.take() {
            waker.wake();
        }
    }

    /// `input.end(text)`.
    pub fn end_with(&self, text: &str) {
        self.write(text);
        self.end();
    }

    pub fn end_with_bytes(&self, bytes: &[u8]) {
        self.write_bytes(bytes);
        self.end();
    }

    /// `input.destroyed`: the reading end was dropped.
    pub fn destroyed(&self) -> bool {
        self.0.borrow().destroyed
    }
}

impl AsyncRead for PipeReader {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        let mut state = self.0.borrow_mut();
        if !state.buffer.is_empty() {
            let count = state.buffer.len().min(buf.remaining());
            for _ in 0..count {
                let byte = state.buffer.pop_front().expect("byte");
                buf.put_slice(&[byte]);
            }
            return Poll::Ready(Ok(()));
        }
        if state.ended {
            return Poll::Ready(Ok(()));
        }
        state.waker = Some(cx.waker().clone());
        Poll::Pending
    }
}

impl Drop for PipeReader {
    fn drop(&mut self) {
        self.0.borrow_mut().destroyed = true;
    }
}

/// How a held write stalls: before it is accepted (`write()` returned false, a drain is awaited) or
/// after (its callback has not come).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hold {
    Write,
    Flush,
}

struct WriterState {
    received: Vec<String>,
    writes: usize,
    destroyed: bool,
    hold_kind: Hold,
    should_hold: Box<dyn FnMut(&str, usize) -> bool>,
    fail_write: Option<String>,
    fail_flush: Option<String>,
    holding: bool,
    resume_write: bool,
    waker: Option<Waker>,
}

/// The test's view of a `Writable`.
#[derive(Clone)]
pub struct WriterHandle(Rc<RefCell<WriterState>>);

/// The `Writable` given to `serve_stdio`.
pub struct Writer(Rc<RefCell<WriterState>>);

impl WriterHandle {
    fn build(
        hold_kind: Hold,
        should_hold: Box<dyn FnMut(&str, usize) -> bool>,
        fail_write: Option<String>,
        fail_flush: Option<String>,
    ) -> (WriterHandle, Writer) {
        let state = Rc::new(RefCell::new(WriterState {
            received: Vec::new(),
            writes: 0,
            destroyed: false,
            hold_kind,
            should_hold,
            fail_write,
            fail_flush,
            holding: false,
            resume_write: false,
            waker: None,
        }));
        (WriterHandle(state.clone()), Writer(state))
    }

    /// A writable that completes every write at once.
    pub fn new() -> (WriterHandle, Writer) {
        Self::build(Hold::Write, Box::new(|_, _| false), None, None)
    }

    /// A writable that holds the writes `should_hold(chunk, index)` picks (index counts from 1) until
    /// `release()`; one write is held at a time.
    pub fn holding(kind: Hold, should_hold: impl FnMut(&str, usize) -> bool + 'static) -> (WriterHandle, Writer) {
        Self::build(kind, Box::new(should_hold), None, None)
    }

    /// A writable whose `write` fails with `message` (`callback(new Error(message))`).
    pub fn failing_write(message: &str) -> (WriterHandle, Writer) {
        Self::build(Hold::Write, Box::new(|_, _| false), Some(message.to_string()), None)
    }

    /// A writable that accepts each write, then fails it from its callback.
    pub fn failing_flush(message: &str) -> (WriterHandle, Writer) {
        Self::build(Hold::Write, Box::new(|_, _| false), None, Some(message.to_string()))
    }

    /// The chunks written, each trimmed as `String(chunk).trim()`.
    pub fn received(&self) -> Vec<String> {
        self.0.borrow().received.iter().map(|chunk| chunk.trim().to_string()).collect()
    }

    /// `JSON.parse(chunk).id` for each chunk.
    pub fn received_ids(&self) -> Vec<Value> {
        self.received()
            .iter()
            .map(|chunk| serde_json::from_str::<Value>(chunk).ok().and_then(|frame| frame.get("id").cloned()).unwrap_or(Value::Null))
            .collect()
    }

    pub fn writes(&self) -> usize {
        self.0.borrow().writes
    }

    pub fn is_holding(&self) -> bool {
        self.0.borrow().holding
    }

    /// `output.destroyed`: the writable was dropped.
    pub fn destroyed(&self) -> bool {
        self.0.borrow().destroyed
    }

    /// The held write completes (its callback or drain).
    pub fn release(&self) {
        let mut state = self.0.borrow_mut();
        if !state.holding {
            return;
        }
        state.holding = false;
        if state.hold_kind == Hold::Write {
            state.resume_write = true;
        }
        if let Some(waker) = state.waker.take() {
            waker.wake();
        }
    }
}

impl AsyncWrite for Writer {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        let mut state = self.0.borrow_mut();
        if let Some(message) = &state.fail_write {
            return Poll::Ready(Err(io::Error::other(message.clone())));
        }
        if state.resume_write {
            state.resume_write = false;
            return Poll::Ready(Ok(buf.len()));
        }
        if state.holding && state.hold_kind == Hold::Write {
            state.waker = Some(cx.waker().clone());
            return Poll::Pending;
        }
        let text = String::from_utf8_lossy(buf).to_string();
        state.writes += 1;
        let index = state.writes;
        state.received.push(text.clone());
        let hold = (state.should_hold)(&text, index);
        if hold {
            state.holding = true;
            if state.hold_kind == Hold::Write {
                state.waker = Some(cx.waker().clone());
                return Poll::Pending;
            }
        }
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let mut state = self.0.borrow_mut();
        if let Some(message) = &state.fail_flush {
            return Poll::Ready(Err(io::Error::other(message.clone())));
        }
        if state.holding && state.hold_kind == Hold::Flush {
            state.waker = Some(cx.waker().clone());
            return Poll::Pending;
        }
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        self.0.borrow_mut().destroyed = true;
    }
}
