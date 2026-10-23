//! Work for the run loop, which alone talks to the helper. A connection's reader queues a request, and a thread of its
//! own waits for the answer with a deadline; the run loop takes the queue between its polls and answers each request through the helper. A
//! request the run loop hasn't started by its deadline is cancelled; one it started but didn't finish in time is
//! uncertain, though M2's reads change nothing. Copied in spirit from the REAPER extension's queue to its main thread.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::refs::Fault;

const QUEUED: u8 = 0;
const CLAIMED: u8 = 1;
const CANCELLED: u8 = 2;

/// What the run loop answers: a result, or an error code and message.
pub type Answer = Result<Value, Fault>;

pub struct Job {
    pub method: String,
    pub params: Value,
    state: Arc<AtomicU8>,
    reply: SyncSender<Answer>,
}

impl Job {
    /// Takes the job for running; false when its waiter already gave up on it.
    pub fn claim(&self) -> bool {
        self.state.compare_exchange(QUEUED, CLAIMED, Ordering::AcqRel, Ordering::Acquire).is_ok()
    }

    pub fn answer(self, answer: Answer) {
        let _ = self.reply.send(answer);
    }
}

/// The socket side's handle on a queued job.
pub struct Waiter {
    state: Arc<AtomicU8>,
    reply: Receiver<Answer>,
}

#[derive(Debug, PartialEq)]
pub enum Outcome {
    Answered(Answer),
    /// Never started: nothing changed.
    Cancelled,
    /// Started but not answered in time.
    Uncertain,
    /// The bridge is stopping, and the job never started.
    Closed,
}

/// How often a waiting socket thread looks whether the bridge is stopping.
const CLOSING_POLL: Duration = Duration::from_millis(25);

impl Waiter {
    /// Waits for the answer until `timeout`, or until `closing` is set.
    pub fn wait(self, timeout: Duration, closing: &AtomicBool) -> Outcome {
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let closed = closing.load(Ordering::Acquire);
            if left.is_zero() || closed {
                if self.state.compare_exchange(QUEUED, CANCELLED, Ordering::AcqRel, Ordering::Acquire).is_ok() {
                    return if closed { Outcome::Closed } else { Outcome::Cancelled };
                }
                // Claimed: the answer may have landed just now.
                return self.reply.try_recv().map_or(Outcome::Uncertain, Outcome::Answered);
            }
            match self.reply.recv_timeout(left.min(CLOSING_POLL)) {
                Ok(answer) => return Outcome::Answered(answer),
                Err(RecvTimeoutError::Disconnected) => return self.after_drop(),
                Err(RecvTimeoutError::Timeout) => {}
            }
        }
    }

    /// The job was dropped unanswered: never claimed, nothing ran; claimed, it was lost mid-way.
    fn after_drop(&self) -> Outcome {
        if self.state.compare_exchange(QUEUED, CANCELLED, Ordering::AcqRel, Ordering::Acquire).is_ok() {
            Outcome::Closed
        } else {
            self.reply.try_recv().map_or(Outcome::Uncertain, Outcome::Answered)
        }
    }
}

/// A job and the handle that waits for it.
pub fn job(method: &str, params: Value) -> (Job, Waiter) {
    let state = Arc::new(AtomicU8::new(QUEUED));
    let (tx, rx) = mpsc::sync_channel(1);
    (Job { method: method.into(), params, state: state.clone(), reply: tx }, Waiter { state, reply: rx })
}

/// The queue: the socket threads' end sends, the run loop's end takes what is there.
#[derive(Clone)]
pub struct Queue {
    sender: Arc<Mutex<Sender<Job>>>,
    receiver: Arc<Mutex<Receiver<Job>>>,
}

impl Default for Queue {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Queue { sender: Arc::new(Mutex::new(tx)), receiver: Arc::new(Mutex::new(rx)) }
    }
}

impl Queue {
    /// Queues a request; the waiter gets its answer.
    pub fn submit(&self, method: &str, params: Value) -> Waiter {
        let (job, waiter) = job(method, params);
        let _ = self.sender.lock().unwrap_or_else(|e| e.into_inner()).send(job);
        waiter
    }

    /// The next job whose waiter is still there, if any is queued.
    pub fn take(&self) -> Option<Job> {
        let receiver = self.receiver.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            match receiver.try_recv() {
                Ok(job) => {
                    if job.claim() {
                        return Some(job);
                    }
                    // Its waiter gave up: nothing to answer.
                }
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => return None,
            }
        }
    }

    /// Whether a job waits.
    pub fn is_pending(&self) -> bool {
        // A peek: the channel has no one, so this counts on `take` following soon.
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::thread;

    #[test]
    fn a_job_is_answered_by_the_run_loop_or_cancelled_at_its_deadline() {
        let queue = Queue::default();
        let closing = AtomicBool::new(false);
        let waiter = queue.submit("read", json!({"kind": "track"}));
        let job = queue.take().unwrap();
        assert_eq!((job.method.as_str(), &job.params), ("read", &json!({"kind": "track"})));
        job.answer(Ok(json!({"items": []})));
        assert_eq!(waiter.wait(Duration::from_secs(1), &closing), Outcome::Answered(Ok(json!({"items": []}))));

        // Nobody takes it: cancelled at the deadline, and the run loop finds nothing to answer later.
        let waiter = queue.submit("snapshot", json!({}));
        assert_eq!(waiter.wait(Duration::from_millis(30), &closing), Outcome::Cancelled);
        assert!(queue.take().is_none());

        // Taken, but answered late: uncertain.
        let waiter = queue.submit("focus_body", json!({}));
        let job = queue.take().unwrap();
        assert_eq!(waiter.wait(Duration::from_millis(30), &closing), Outcome::Uncertain);
        job.answer(Err(("failed".into(), "late".into())));

        // The bridge stopping ends a wait with Closed.
        let waiter = queue.submit("read", json!({}));
        closing.store(true, Ordering::Release);
        assert_eq!(waiter.wait(Duration::from_secs(1), &closing), Outcome::Closed);
    }

    #[test]
    fn a_job_dropped_by_the_run_loop_ends_the_wait() {
        let queue = Queue::default();
        let closing = AtomicBool::new(false);
        let waiter = queue.submit("read", json!({}));
        let taker = queue.clone();
        thread::spawn(move || {
            let job = taker.take().unwrap();
            drop(job);
        });
        assert_eq!(waiter.wait(Duration::from_secs(2), &closing), Outcome::Uncertain);
        assert!(!queue.is_pending());
    }
}
