//! Work for REAPER's main thread. The socket threads queue jobs; the timer and the control surface's Run() drain
//! them within a per-tick budget. Each job has a deadline: a job the main thread hasn't claimed by then is
//! cancelled (nothing changed); one it claimed but didn't answer in time is uncertain.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;

const QUEUED: u8 = 0;
const CLAIMED: u8 = 1;
const CANCELLED: u8 = 2;

/// What the main thread answers: a result, or an error code and message.
pub type Answer = Result<Value, (String, String)>;

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
    /// Started but not answered in time: it may have changed something.
    Uncertain,
    /// The extension is unloading, and the job never started: nothing changed.
    Closed,
}

/// How often a waiting socket thread looks whether the extension is closing.
const CLOSING_POLL: Duration = Duration::from_millis(25);

impl Waiter {
    /// Waits for the answer until `timeout`, or until `closing` is set (the extension is unloading, and its main thread
    /// may be the one waiting for this thread to end).
    pub fn wait(self, timeout: Duration, closing: &AtomicBool) -> Outcome {
        self.wait_unless(timeout, closing, &|| false)
    }

    /// Whether the main thread has claimed the job: it runs, or ran.
    pub fn claimed(&self) -> impl Fn() -> bool {
        let state = self.state.clone();
        move || state.load(Ordering::Acquire) == CLAIMED
    }

    /// As `wait`, and gives up early, as a timeout does, once `give_up` says so (checked every few milliseconds): a
    /// request that waits for a main thread that has been found to be in a dialog.
    pub fn wait_unless(self, timeout: Duration, closing: &AtomicBool, give_up: &dyn Fn() -> bool) -> Outcome {
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let closed = closing.load(Ordering::Acquire);
            if left.is_zero() || closed || give_up() {
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

    fn after_drop(&self) -> Outcome {
        match self.reply.try_recv() {
            Ok(answer) => Outcome::Answered(answer),
            Err(_) if self.state.load(Ordering::Acquire) == CLAIMED => Outcome::Uncertain,
            Err(_) => Outcome::Closed,
        }
    }
}

pub fn job(method: &str, params: Value) -> (Job, Waiter) {
    let state = Arc::new(AtomicU8::new(QUEUED));
    let (reply, rx) = mpsc::sync_channel(1);
    (Job { method: method.to_string(), params, state: state.clone(), reply }, Waiter { state, reply: rx })
}

pub fn channel() -> (Sender<Job>, Receiver<Job>) {
    mpsc::channel()
}

/// How long one tick may spend on jobs, and how long it waits for one more after the queue empties, so a client's
/// next request in a quick exchange runs this tick rather than the next.
#[derive(Debug, Clone, Copy)]
pub struct Budget {
    pub per_tick: Duration,
    pub linger: Duration,
}

impl Default for Budget {
    fn default() -> Self {
        Budget { per_tick: Duration::from_millis(8), linger: Duration::from_millis(3) }
    }
}

/// Runs queued jobs until the budget is spent or the queue stays empty for the linger. At least one job runs when
/// any is queued. Returns how many ran.
pub fn drain(jobs: &Receiver<Job>, budget: Budget, mut run: impl FnMut(Job)) -> usize {
    let start = Instant::now();
    let mut ran = 0;
    loop {
        let job = match jobs.try_recv() {
            Ok(job) => job,
            Err(TryRecvError::Disconnected) => break,
            Err(TryRecvError::Empty) => {
                let left = budget.per_tick.saturating_sub(start.elapsed()).min(budget.linger);
                if ran == 0 || left.is_zero() {
                    break;
                }
                match jobs.recv_timeout(left) {
                    Ok(job) => job,
                    Err(_) => break,
                }
            }
        };
        if !job.claim() {
            continue;
        }
        run(job);
        ran += 1;
        if start.elapsed() >= budget.per_tick {
            break;
        }
    }
    ran
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::thread;

    const NO_LINGER: Budget = Budget { per_tick: Duration::from_millis(50), linger: Duration::ZERO };
    static OPEN: AtomicBool = AtomicBool::new(false);

    #[test]
    fn a_drained_job_is_answered() {
        let (tx, rx) = channel();
        let (job, waiter) = job("status", json!({}));
        tx.send(job).unwrap();
        assert_eq!(drain(&rx, NO_LINGER, |job| job.answer(Ok(json!({"ok": true})))), 1);
        assert_eq!(waiter.wait(Duration::from_secs(1), &OPEN), Outcome::Answered(Ok(json!({"ok": true}))));
    }

    #[test]
    fn a_job_past_its_deadline_is_cancelled_and_never_runs() {
        let (tx, rx) = channel();
        let (job, waiter) = job("quit", json!({}));
        tx.send(job).unwrap();
        assert_eq!(waiter.wait(Duration::from_millis(1), &OPEN), Outcome::Cancelled);
        assert_eq!(drain(&rx, NO_LINGER, |_| panic!("a cancelled job ran")), 0);
    }

    #[test]
    fn a_claimed_job_that_misses_its_deadline_is_uncertain() {
        let (tx, rx) = channel();
        let (job, waiter) = job("slow", json!({}));
        tx.send(job).unwrap();
        let (started, running) = mpsc::channel();
        let main = thread::spawn(move || {
            drain(&rx, NO_LINGER, |job| {
                started.send(()).unwrap();
                thread::sleep(Duration::from_millis(300));
                job.answer(Ok(Value::Null));
            })
        });
        // Claimed and running (not just about to be): the wait's end finds a job that has started.
        running.recv().unwrap();
        assert_eq!(waiter.wait(Duration::from_millis(10), &OPEN), Outcome::Uncertain);
        main.join().unwrap();
    }

    #[test]
    fn a_job_dropped_unanswered_reports_closing() {
        let (tx, rx) = channel();
        let (job, waiter) = job("status", json!({}));
        tx.send(job).unwrap();
        drop(rx);
        assert_eq!(waiter.wait(Duration::from_secs(1), &OPEN), Outcome::Closed);
    }

    #[test]
    fn a_wait_that_gives_up_early_cancels_a_queued_job_and_leaves_a_claimed_one_be() {
        let (tx, rx) = channel();
        let (first, waiter) = job("read", json!({}));
        tx.send(first).unwrap();
        let started = Instant::now();
        let outcome = waiter.wait_unless(Duration::from_secs(30), &OPEN, &|| started.elapsed() > Duration::from_millis(60));
        assert_eq!(outcome, Outcome::Cancelled);
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(drain(&rx, NO_LINGER, |_| panic!("a job that was given up on ran")), 0);
        // A job already running isn't taken back: it may have changed something.
        let (second, second_waiter) = job("action", json!({}));
        tx.send(second).unwrap();
        let held = rx.recv().unwrap();
        assert!(held.claim());
        assert_eq!(second_waiter.wait_unless(Duration::from_secs(30), &OPEN, &|| true), Outcome::Uncertain);
    }

    #[test]
    fn closing_ends_a_wait_at_once_and_cancels_a_queued_job() {
        let (tx, rx) = channel();
        let (job, waiter) = job("action", json!({}));
        tx.send(job).unwrap();
        let closing = Arc::new(AtomicBool::new(false));
        let flag = closing.clone();
        let waiting = thread::spawn(move || {
            let started = Instant::now();
            (waiter.wait(Duration::from_secs(30), &flag), started.elapsed())
        });
        thread::sleep(Duration::from_millis(50));
        closing.store(true, Ordering::Release);
        let (outcome, took) = waiting.join().unwrap();
        assert_eq!(outcome, Outcome::Closed);
        assert!(took < Duration::from_secs(2), "{took:?}");
        assert_eq!(drain(&rx, NO_LINGER, |_| panic!("a job cancelled at closing ran")), 0);
    }

    #[test]
    fn closing_while_a_job_runs_is_uncertain() {
        let (tx, rx) = channel();
        let (job, waiter) = job("action", json!({}));
        tx.send(job).unwrap();
        let held = rx.recv().unwrap();
        assert!(held.claim());
        let closing = AtomicBool::new(true);
        assert_eq!(waiter.wait(Duration::from_secs(30), &closing), Outcome::Uncertain);
        held.answer(Ok(Value::Null));
    }

    #[test]
    fn an_answer_that_lands_as_the_wait_ends_is_kept() {
        let (tx, rx) = channel();
        let (job, waiter) = job("action", json!({}));
        tx.send(job).unwrap();
        let held = rx.recv().unwrap();
        assert!(held.claim());
        held.answer(Ok(json!(1)));
        assert_eq!(waiter.wait(Duration::ZERO, &OPEN), Outcome::Answered(Ok(json!(1))));
    }

    #[test]
    fn quick_jobs_share_a_tick_and_a_slow_one_ends_it() {
        let (tx, rx) = channel();
        let waiters: Vec<_> = (0..5)
            .map(|_| {
                let (job, waiter) = job("ping", json!({}));
                tx.send(job).unwrap();
                waiter
            })
            .collect();
        let quick = |job: Job| job.answer(Ok(Value::Null));
        assert_eq!(drain(&rx, Budget { per_tick: Duration::from_secs(5), linger: Duration::ZERO }, quick), 5);
        assert!(waiters.into_iter().all(|w| w.wait(Duration::from_secs(1), &OPEN) == Outcome::Answered(Ok(Value::Null))));

        for _ in 0..3 {
            tx.send(job("slow", json!({})).0).unwrap();
        }
        let budget = Budget { per_tick: Duration::from_millis(5), linger: Duration::from_millis(5) };
        let slow = |job: Job| {
            thread::sleep(Duration::from_millis(6));
            job.answer(Ok(Value::Null));
        };
        assert_eq!(drain(&rx, budget, slow), 1);
        assert_eq!(drain(&rx, budget, slow), 1);
        assert_eq!(drain(&rx, budget, slow), 1);
        assert_eq!(drain(&rx, budget, slow), 0);
    }

    #[test]
    fn the_linger_picks_up_a_quick_follow_up() {
        let (tx, rx) = channel();
        let (first, _w1) = job("a", json!({}));
        tx.send(first).unwrap();
        let sender = thread::spawn(move || {
            thread::sleep(Duration::from_millis(5));
            let (second, w2) = job("b", json!({}));
            tx.send(second).unwrap();
            w2
        });
        let budget = Budget { per_tick: Duration::from_millis(200), linger: Duration::from_millis(100) };
        let mut names = Vec::new();
        drain(&rx, budget, |job| {
            names.push(job.method.clone());
            job.answer(Ok(Value::Null));
        });
        assert_eq!(names, ["a", "b"]);
        sender.join().unwrap();
    }
}
