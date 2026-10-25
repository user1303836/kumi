//! Coalesces redraw requests into frames; keeps frames coming only while something animates.
//!
//! Frames are timed with Tokio, so a scheduler lives inside a `LocalSet`.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use tokio::task::JoinHandle;

use kumi_common::time::perf_now;

/// How often a frame is drawn while something animates, unless the app says otherwise: the activity glyphs step every
/// 100 ms and the shimmer moves a cell or two in this time, so nothing on screen changes faster. A request (a key, an
/// event) is still drawn within the app's 16 ms.
pub const ANIMATION_MS: f64 = 50.0;

struct State {
    timer: Option<JoinHandle<()>>,
    /// When the pending frame is due, by the scheduler's clock; infinite without one.
    due: f64,
    last: f64,
    animating: bool,
    disposed: bool,
}

struct Inner {
    draw: Rc<dyn Fn()>,
    interval_ms: f64,
    animation_ms: f64,
    now: Box<dyn Fn() -> f64>,
    state: RefCell<State>,
}

#[derive(Clone)]
pub struct FrameScheduler {
    inner: Rc<Inner>,
}

impl FrameScheduler {
    /// Frames at most every `interval_ms`, timed by `performance.now()`, and one every `interval_ms` while animating.
    pub fn new(draw: Rc<dyn Fn()>, interval_ms: f64) -> FrameScheduler {
        FrameScheduler::with_intervals(draw, interval_ms, interval_ms)
    }

    /// Frames at most every `interval_ms` when asked for (16 for the app), and one every `animation_ms` while something
    /// animates (`ANIMATION_MS` for the app): a request is still drawn within `interval_ms`, whatever the animation's pace.
    pub fn with_intervals(draw: Rc<dyn Fn()>, interval_ms: f64, animation_ms: f64) -> FrameScheduler {
        FrameScheduler::with_clock(draw, interval_ms, animation_ms, Box::new(perf_now))
    }

    pub fn with_clock(draw: Rc<dyn Fn()>, interval_ms: f64, animation_ms: f64, now: Box<dyn Fn() -> f64>) -> FrameScheduler {
        FrameScheduler {
            inner: Rc::new(Inner {
                draw,
                interval_ms,
                animation_ms: animation_ms.max(interval_ms),
                now,
                state: RefCell::new(State { timer: None, due: f64::INFINITY, last: f64::NEG_INFINITY, animating: false, disposed: false }),
            }),
        }
    }

    /// Draw soon; many requests before the next frame produce one draw.
    pub fn request(&self) {
        self.request_within(self.inner.interval_ms);
    }

    /// Draw at the animation's pace: for what a frame finds still moving as it draws (a glyph in a panel), and for
    /// what arrives on its own (a model's words, the DAW's events) rather than from the producer's hands.
    pub fn request_animation(&self) {
        self.request_within(self.inner.animation_ms);
    }

    /// A frame `interval_ms` after the last one (at once when that has passed), unless one is due sooner already.
    fn request_within(&self, interval_ms: f64) {
        let mut state = self.inner.state.borrow_mut();
        if state.disposed {
            return;
        }
        let now = (self.inner.now)();
        let due = (state.last + interval_ms).max(now);
        if let Some(timer) = &state.timer {
            if state.due <= due {
                return;
            }
            // An animation's frame was coming later: the request brings it forward.
            timer.abort();
        }
        state.due = due;
        let scheduler = self.clone();
        state.timer = Some(tokio::task::spawn_local(async move {
            tokio::time::sleep(Duration::from_secs_f64((due - now) / 1000.0)).await;
            scheduler.tick();
        }));
    }

    /// While on, a frame is drawn every `animation_ms`, for pulses and moving knobs.
    pub fn set_animating(&self, on: bool) {
        self.inner.state.borrow_mut().animating = on;
        if on {
            self.request();
        }
    }

    /// Draw now if a frame is pending; for tests and before exit.
    pub fn flush(&self) {
        let Some(timer) = self.inner.state.borrow_mut().timer.take() else { return };
        timer.abort();
        self.tick();
    }

    pub fn dispose(&self) {
        let mut state = self.inner.state.borrow_mut();
        state.disposed = true;
        if let Some(timer) = state.timer.take() {
            timer.abort();
        }
    }

    fn tick(&self) {
        {
            let mut state = self.inner.state.borrow_mut();
            state.timer = None;
            state.due = f64::INFINITY;
            if state.disposed {
                return;
            }
            state.last = (self.inner.now)();
        }
        (self.inner.draw)();
        if self.inner.state.borrow().animating {
            self.request_within(self.inner.animation_ms);
        }
    }
}
