//! Cancellation in the shape the TypeScript used: an `AbortSignal` that work checks and awaits.
//!
//! A [`Signal`] uses `tokio_util` cancellation with synchronous parent checks. [`timeout`] is
//! `AbortSignal.timeout`, [`any`] is `AbortSignal.any`, [`Signal::check`] is `signal.throwIfAborted()`, and a
//! [`Controller`] is an `AbortController`.

use std::{
    future::{poll_fn, Future},
    pin::Pin,
    sync::Arc,
    task::Poll,
    time::Duration,
};

/// A cancellation signal. A combined signal sees a parent abort immediately, including when a
/// callback aborts between two awaits that both complete without yielding to the scheduler.
#[derive(Debug, Clone, Default)]
pub struct Signal {
    token: tokio_util::sync::CancellationToken,
    parents: Arc<[Signal]>,
}
impl Signal {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.token.cancel();
    }
    /// Whether `other` is a copy of this signal: the one a turn gives each of its tools, say. Only a
    /// combined signal (from [`any`]) can be told apart this way; a plain one is never the same.
    pub fn same_as(&self, other: &Signal) -> bool {
        !self.parents.is_empty() && Arc::ptr_eq(&self.parents, &other.parents)
    }
    pub fn is_cancelled(&self) -> bool {
        self.token.is_cancelled() || self.parents.iter().any(Self::is_cancelled)
    }
    pub fn cancelled(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            if self.is_cancelled() {
                return;
            }
            let mut waits: Vec<_> = self.parents.iter().map(Self::cancelled).collect();
            waits.push(Box::pin(self.token.cancelled()));
            poll_fn(|cx| {
                for wait in &mut waits {
                    if wait.as_mut().poll(cx).is_ready() {
                        return Poll::Ready(());
                    }
                }
                Poll::Pending
            })
            .await;
        })
    }
}

/// `new Error("...")` thrown by `throwIfAborted`: the work was cancelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Aborted;

impl std::fmt::Display for Aborted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Operation cancelled")
    }
}

impl std::error::Error for Aborted {}

/// `AbortSignal` methods beyond the token's own.
pub trait SignalExt {
    /// `signal.throwIfAborted()`.
    fn check(&self) -> Result<(), Aborted>;
    /// `signal.aborted`.
    fn aborted(&self) -> bool;
}

impl SignalExt for Signal {
    fn check(&self) -> Result<(), Aborted> {
        if self.is_cancelled() {
            Err(Aborted)
        } else {
            Ok(())
        }
    }
    fn aborted(&self) -> bool {
        self.is_cancelled()
    }
}

/// `AbortSignal.timeout(ms)`: a signal that fires after `ms` (needs a Tokio runtime).
pub fn timeout(ms: u64) -> Signal {
    let signal = Signal::new();
    let fired = signal.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(ms)).await;
        fired.cancel();
    });
    signal
}

/// `AbortSignal.any([...])`: a signal that fires when any parent does. No scheduler turn is needed
/// to observe cancellation and no background forwarding task outlives an unused combined signal.
pub fn any<I: IntoIterator<Item = Signal>>(signals: I) -> Signal {
    Signal { token: tokio_util::sync::CancellationToken::new(), parents: signals.into_iter().collect() }
}

/// A signal that never fires: `new AbortController().signal` left alone.
pub fn never() -> Signal {
    Signal::new()
}

/// `AbortController`: owns a signal and fires it.
#[derive(Debug, Clone, Default)]
pub struct Controller {
    pub signal: Signal,
}

impl Controller {
    pub fn new() -> Self {
        Self { signal: Signal::new() }
    }
    /// `controller.abort()`.
    pub fn abort(&self) {
        self.signal.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_combined_signal_is_the_same_as_its_copies_only() {
        let turn = any([Signal::new()]);
        assert!(turn.same_as(&turn.clone()));
        assert!(!turn.same_as(&any([Signal::new()])));
        assert!(!turn.same_as(&Signal::new()));
        let plain = Signal::new();
        assert!(!plain.same_as(&plain.clone()));
    }

    #[tokio::test]
    async fn any_fires_with_the_first() {
        let a = Controller::new();
        let b = Controller::new();
        let both = any([a.signal.clone(), b.signal.clone()]);
        assert!(!both.aborted());
        b.abort();
        assert!(both.aborted());
        assert!(both.check().is_err());
        both.cancelled().await;
        assert!(both.check().is_err());
    }

    #[test]
    fn nested_any_observes_cancellation_without_a_runtime() {
        let parent = Signal::new();
        let peer = Signal::new();
        let inner = any([parent.clone(), peer.clone()]);
        let outer = any([Signal::new(), inner.clone()]);
        parent.cancel();
        assert!(inner.is_cancelled());
        assert!(outer.is_cancelled());
        assert!(!peer.is_cancelled());
        let own = any([peer.clone()]);
        own.cancel();
        assert!(own.is_cancelled());
        assert!(!peer.is_cancelled());
    }

    #[tokio::test]
    async fn a_waiting_nested_any_wakes_when_a_parent_aborts() {
        let parent = Signal::new();
        let outer = any([any([parent.clone()]), Signal::new()]);
        let waiting = tokio::spawn(async move {
            outer.cancelled().await;
        });
        tokio::task::yield_now().await;
        parent.cancel();
        tokio::time::timeout(Duration::from_secs(1), waiting).await.unwrap().unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn timeout_fires_after_its_delay() {
        let signal = timeout(50);
        assert!(!signal.aborted());
        tokio::time::advance(Duration::from_millis(60)).await;
        signal.cancelled().await;
        assert!(signal.aborted());
    }
}
