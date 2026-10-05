//! Coalesces redraw requests into frames; keeps frames coming only while something animates.
//!
//! Frames are timed with Tokio, so a scheduler lives inside a `LocalSet`.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use tokio::task::JoinHandle;

use kumi_common::time::perf_now;

struct State {
    timer: Option<JoinHandle<()>>,
    last: f64,
    animating: bool,
    disposed: bool,
}

struct Inner {
    draw: Rc<dyn Fn()>,
    interval_ms: f64,
    now: Box<dyn Fn() -> f64>,
    state: RefCell<State>,
}

#[derive(Clone)]
pub struct FrameScheduler {
    inner: Rc<Inner>,
}

impl FrameScheduler {
    /// Frames at most every `interval_ms` (16 for the app), timed by `performance.now()`.
    pub fn new(draw: Rc<dyn Fn()>, interval_ms: f64) -> FrameScheduler {
        FrameScheduler::with_clock(draw, interval_ms, Box::new(perf_now))
    }

    pub fn with_clock(draw: Rc<dyn Fn()>, interval_ms: f64, now: Box<dyn Fn() -> f64>) -> FrameScheduler {
        FrameScheduler {
            inner: Rc::new(Inner {
                draw,
                interval_ms,
                now,
                state: RefCell::new(State { timer: None, last: f64::NEG_INFINITY, animating: false, disposed: false }),
            }),
        }
    }

    /// Draw soon; many requests before the next frame produce one draw.
    pub fn request(&self) {
        let mut state = self.inner.state.borrow_mut();
        if state.timer.is_some() || state.disposed {
            return;
        }
        let wait = (state.last + self.inner.interval_ms - (self.inner.now)()).max(0.0);
        let scheduler = self.clone();
        state.timer = Some(tokio::task::spawn_local(async move {
            tokio::time::sleep(Duration::from_secs_f64(wait / 1000.0)).await;
            scheduler.tick();
        }));
    }

    /// While on, a frame is drawn every interval, for pulses and moving knobs.
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
            if state.disposed {
                return;
            }
            state.last = (self.inner.now)();
        }
        (self.inner.draw)();
        if self.inner.state.borrow().animating {
            self.request();
        }
    }
}
