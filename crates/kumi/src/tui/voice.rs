//! Taps, holds, quiet-stop, transcription and cancellation.
use super::{
    activity::{activity_glyph, shimmer, Activity},
    style::{palette, Style},
    transcript::NoticeTone,
    wrap::Span,
};
use crate::voice::{VoiceChange, VoiceChoices, VoiceControl, VoiceIo};
use async_trait::async_trait;
use kumi_common::{abort::Signal, js::number, time::perf_now};
use kumi_runtime::{
    core::errors::RuntimeError,
    voice::{Heard, Listening, VoiceError, VoiceFailure, VoiceTrouble},
};
use std::{cell::RefCell, rc::Rc, sync::Arc, time::Duration};
#[async_trait(?Send)]
pub trait VoiceListening {
    fn seconds(&self) -> f64;
    fn spoke(&self) -> bool;
    fn quiet_ms(&self) -> f64;
    fn level(&self) -> f64;
    async fn stop(&self) -> Result<Heard, VoiceFailure>;
    fn cancel(&self);
    async fn ended(&self) -> Option<VoiceError>;
}
#[async_trait(?Send)]
impl VoiceListening for Listening {
    fn seconds(&self) -> f64 {
        Listening::seconds(self)
    }
    fn spoke(&self) -> bool {
        Listening::spoke(self)
    }
    fn quiet_ms(&self) -> f64 {
        Listening::quiet_ms(self)
    }
    fn level(&self) -> f64 {
        Listening::level(self)
    }
    async fn stop(&self) -> Result<Heard, VoiceFailure> {
        Ok(Listening::stop(self).await)
    }
    fn cancel(&self) {
        Listening::cancel(self)
    }
    async fn ended(&self) -> Option<VoiceError> {
        Listening::ended(self).await
    }
}
#[async_trait(?Send)]
pub trait VoiceController {
    fn system_language(&self) -> String;
    fn choices(&self) -> VoiceChoices;
    fn choose(&self, change: VoiceChange) -> Result<(), RuntimeError>;
    async fn listen(&self, io: VoiceIo) -> Result<Rc<dyn VoiceListening>, VoiceFailure>;
    async fn write_down(&self, heard: Heard, io: VoiceIo, names: Vec<String>) -> Result<String, VoiceFailure>;
    async fn microphones(&self) -> Result<Vec<String>, VoiceFailure>;
    fn has_privacy(&self) -> bool;
    fn open_privacy(&self);
}
#[async_trait(?Send)]
impl VoiceController for VoiceControl {
    fn system_language(&self) -> String {
        VoiceControl::system_language(self).into()
    }
    fn choices(&self) -> VoiceChoices {
        VoiceControl::choices(self)
    }
    fn choose(&self, change: VoiceChange) -> Result<(), RuntimeError> {
        VoiceControl::choose(self, change)
    }
    async fn listen(&self, io: VoiceIo) -> Result<Rc<dyn VoiceListening>, VoiceFailure> {
        Ok(Rc::new(VoiceControl::listen(self, io).await?))
    }
    async fn write_down(&self, heard: Heard, io: VoiceIo, names: Vec<String>) -> Result<String, VoiceFailure> {
        VoiceControl::write_down(self, heard, io, names).await
    }
    async fn microphones(&self) -> Result<Vec<String>, VoiceFailure> {
        VoiceControl::microphones(self).await
    }
    fn has_privacy(&self) -> bool {
        VoiceControl::has_privacy(self)
    }
    fn open_privacy(&self) {
        VoiceControl::open_privacy(self)
    }
}
pub trait VoiceHost {
    fn insert(&self, text: &str);
    fn send(&self);
    fn notice(&self, text: &str, tone: NoticeTone);
    fn offer(&self, trouble: VoiceTrouble);
    fn names(&self) -> Vec<String>;
    fn redraw(&self);
    fn animate(&self, on: bool);
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VoicePhase {
    #[default]
    Idle,
    Starting,
    Listening,
    Writing,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Held {
    Release,
    Repeat,
}
#[derive(Default)]
struct State {
    phase: VoicePhase,
    take: u64,
    abort: Option<Signal>,
    mic: Option<Rc<dyn VoiceListening>>,
    since: f64,
    pressed_at: f64,
    keyed: bool,
    releases: bool,
    held: Option<Held>,
    let_go: Option<Signal>,
    quiet_until: f64,
    last_press: f64,
    send_after: bool,
    progress: Option<String>,
    levels: Vec<f64>,
    sampler: Option<Signal>,
}
struct Inner {
    state: RefCell<State>,
    control: Rc<dyn VoiceController>,
    host: Rc<dyn VoiceHost>,
    now: Rc<dyn Fn() -> f64>,
}
impl Drop for Inner {
    fn drop(&mut self) {
        let state = self.state.get_mut();
        if let Some(abort) = state.abort.take() {
            abort.cancel();
        }
        if let Some(mic) = state.mic.take() {
            mic.cancel();
        }
        if let Some(timer) = state.let_go.take() {
            timer.cancel();
        }
        if let Some(timer) = state.sampler.take() {
            timer.cancel();
        }
    }
}
#[derive(Clone)]
pub struct VoiceInput(Rc<Inner>);
#[derive(Clone, Debug)]
pub struct VoiceView {
    pub status: Vec<Span>,
    pub hint: String,
    pub placeholder: String,
}
impl VoiceInput {
    pub fn new(control: Rc<dyn VoiceController>, host: Rc<dyn VoiceHost>) -> Self {
        Self::with_clock(control, host, Rc::new(perf_now))
    }
    pub fn with_clock(control: Rc<dyn VoiceController>, host: Rc<dyn VoiceHost>, now: Rc<dyn Fn() -> f64>) -> Self {
        Self(Rc::new(Inner { state: RefCell::new(State::default()), control, host, now }))
    }
    pub fn phase(&self) -> VoicePhase {
        self.0.state.borrow().phase
    }
    pub fn active(&self) -> bool {
        self.phase() != VoicePhase::Idle
    }
    pub fn listening(&self) -> bool {
        matches!(self.phase(), VoicePhase::Starting | VoicePhase::Listening)
    }
    pub fn press(&self, repeat: bool) {
        let now = (self.0.now)();
        let mut state = self.0.state.borrow_mut();
        if repeat {
            if matches!(state.phase, VoicePhase::Starting | VoicePhase::Listening) {
                state.held = Some(Held::Release);
            }
            return;
        }
        if now < state.quiet_until {
            state.quiet_until = now + 350.;
            return;
        }
        if state.phase == VoicePhase::Idle {
            drop(state);
            self.start(true);
            return;
        }
        if !matches!(state.phase, VoicePhase::Starting | VoicePhase::Listening)
            || (state.phase == VoicePhase::Starting && state.progress.as_ref().is_some_and(|p| !p.is_empty()))
        {
            return;
        }
        if state.held == Some(Held::Repeat) || (!state.releases && state.keyed && now - state.pressed_at < 1500.) {
            state.held = Some(Held::Repeat);
            state.last_press = now;
            drop(state);
            self.extend();
            return;
        }
        state.quiet_until = now + 350.;
        drop(state);
        self.stop(false, None);
    }
    pub fn release(&self) {
        let mut state = self.0.state.borrow_mut();
        state.releases = true;
        let stop =
            matches!(state.phase, VoicePhase::Starting | VoicePhase::Listening) && state.keyed && (self.0.now)() - state.pressed_at >= 400.;
        drop(state);
        if stop {
            self.stop(false, None);
        }
    }
    pub fn enter(&self) -> bool {
        match self.phase() {
            VoicePhase::Listening => {
                self.stop(true, None);
                true
            }
            VoicePhase::Writing => {
                self.0.state.borrow_mut().send_after = true;
                self.0.host.redraw();
                true
            }
            VoicePhase::Starting => {
                self.cancel();
                false
            }
            VoicePhase::Idle => false,
        }
    }
    pub fn cancel(&self) -> bool {
        let mut state = self.0.state.borrow_mut();
        if state.phase == VoicePhase::Idle {
            return false;
        }
        state.take = state.take.wrapping_add(1);
        if let Some(abort) = &state.abort {
            abort.cancel();
        }
        let mic = state.mic.clone();
        drop(state);
        self.reset();
        if let Some(mic) = mic {
            mic.cancel();
        }
        true
    }
    pub fn start(&self, keyed: bool) {
        let now = (self.0.now)();
        let mut state = self.0.state.borrow_mut();
        if state.phase != VoicePhase::Idle {
            return;
        }
        state.take = state.take.wrapping_add(1);
        let take = state.take;
        let abort = Signal::new();
        state.abort = Some(abort.clone());
        state.phase = VoicePhase::Starting;
        state.since = now;
        state.pressed_at = now;
        state.keyed = keyed;
        state.held = None;
        state.send_after = false;
        state.progress = None;
        state.levels.clear();
        drop(state);
        self.0.host.animate(true);
        self.0.host.redraw();
        let weak = Rc::downgrade(&self.0);
        let control = self.0.control.clone();
        let io = self.io(take, abort);
        tokio::task::spawn_local(async move {
            let result = control.listen(io).await;
            let Some(inner) = weak.upgrade() else {
                if let Ok(mic) = result {
                    mic.cancel();
                }
                return;
            };
            let this = Self(inner);
            if take != this.0.state.borrow().take {
                if let Ok(mic) = result {
                    mic.cancel();
                }
                return;
            }
            match result {
                Err(error) => this.fail(error),
                Ok(mic) => {
                    let timer = Signal::new();
                    {
                        let mut state = this.0.state.borrow_mut();
                        state.mic = Some(mic.clone());
                        state.phase = VoicePhase::Listening;
                        state.since = (this.0.now)();
                        state.progress = None;
                        state.sampler = Some(timer.clone());
                    }
                    let weak = Rc::downgrade(&this.0);
                    tokio::task::spawn_local(async move {
                        let mut interval =
                            tokio::time::interval_at(tokio::time::Instant::now() + Duration::from_millis(60), Duration::from_millis(60));
                        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                        loop {
                            tokio::select! {_=timer.cancelled()=>break,_=interval.tick()=>{let Some(inner)=weak.upgrade()else{break};Self(inner).sample();}}
                        }
                    });
                    let weak = Rc::downgrade(&this.0);
                    tokio::task::spawn_local(async move {
                        let why = mic.ended().await;
                        if let Some(inner) = weak.upgrade() {
                            let this = Self(inner);
                            let current = {
                                let state = this.0.state.borrow();
                                state.take == take && state.phase == VoicePhase::Listening
                            };
                            if current {
                                this.ended(why);
                            }
                        }
                    });
                    this.0.host.redraw();
                }
            }
        });
    }
    pub fn stop(&self, send: bool, at: Option<f64>) {
        let mut state = self.0.state.borrow_mut();
        if let Some(timer) = state.let_go.take() {
            timer.cancel();
        }
        if state.phase == VoicePhase::Starting {
            drop(state);
            self.cancel();
            return;
        }
        let Some(mic) = state.mic.clone().filter(|_| state.phase == VoicePhase::Listening) else {
            return;
        };
        let take = state.take;
        let abort = state.abort.clone().expect("listening take has a signal");
        let short = at.unwrap_or_else(|| (self.0.now)()) - state.since < 400.;
        if let Some(timer) = state.sampler.take() {
            timer.cancel();
        }
        state.mic = None;
        state.phase = VoicePhase::Writing;
        state.since = (self.0.now)();
        state.progress = None;
        state.send_after |= send || self.0.control.choices().send;
        drop(state);
        self.0.host.redraw();
        let weak = Rc::downgrade(&self.0);
        tokio::task::spawn_local(async move {
            let heard = mic.stop().await;
            let Some(inner) = weak.upgrade() else {
                return;
            };
            let this = Self(inner);
            if take != this.0.state.borrow().take {
                return;
            }
            let heard = match heard {
                Ok(heard) => heard,
                Err(error) => {
                    this.fail(error);
                    return;
                }
            };
            if short {
                this.reset();
                return;
            }
            let io = this.io(take, abort);
            let names = this.0.host.names();
            let control = this.0.control.clone();
            drop(this);
            let words = control.write_down(heard, io, names).await;
            let Some(inner) = weak.upgrade() else {
                return;
            };
            let this = Self(inner);
            if take != this.0.state.borrow().take {
                return;
            }
            match words {
                Err(error) => this.fail(error),
                Ok(words) => {
                    let sending = this.0.state.borrow().send_after;
                    this.reset();
                    this.0.host.insert(&words);
                    if sending {
                        this.0.host.send();
                    }
                }
            }
        });
    }
    pub fn view(&self, now: f64) -> Option<VoiceView> {
        let state = self.0.state.borrow();
        let elapsed = now - state.since;
        let dim = |text: String| Span::styled(text, Style::fg(palette::DIM));
        let faint = |text: String| Span::styled(text, Style::fg(palette::FAINT));
        match state.phase {
            VoicePhase::Starting => {
                let words = state.progress.clone().unwrap_or_else(|| {
                    if elapsed > 3000. { "waiting for the microphone: if your computer asks, allow it" } else { "opening the microphone" }
                        .into()
                });
                Some(VoiceView {
                    status: vec![activity_glyph(Activity::Listen, elapsed, false), dim(format!(" {words}"))],
                    hint: "esc to cancel".into(),
                    placeholder: "Getting ready to listen…".into(),
                })
            }
            VoicePhase::Listening => {
                let lit = (elapsed / 500.).floor() % 2. == 0.;
                let seconds = (elapsed / 1000.).floor().max(0.) as u64;
                let mut status = vec![
                    Span::styled("●", Style::fg(if lit { palette::ACCENT } else { palette::PULSE })),
                    dim(format!(" {}:{:02}  ", seconds / 60, seconds % 60)),
                ];
                for level in std::iter::repeat_n(0., 16usize.saturating_sub(state.levels.len())).chain(state.levels.iter().copied()) {
                    let bars = ["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];
                    status.push(Span::styled(
                        bars[number::round(level * 7.) as usize],
                        Style::fg(if level >= 0.55 {
                            palette::ACCENT
                        } else if level >= 0.15 {
                            palette::DIM
                        } else {
                            palette::RULE
                        }),
                    ));
                }
                Some(VoiceView {
                    status,
                    hint: if state.held.is_some() {
                        "let go to stop · esc to cancel"
                    } else {
                        "ctrl+t to stop · enter to send · esc to cancel"
                    }
                    .into(),
                    placeholder: "Listening…".into(),
                })
            }
            VoicePhase::Writing => {
                let mut status = shimmer("writing it down", elapsed);
                if let Some(progress) = state.progress.as_ref().filter(|p| !p.is_empty()) {
                    status.push(faint(format!(" · {progress}")));
                }
                Some(VoiceView {
                    status,
                    hint: if state.send_after { "sends once it's written · esc to cancel" } else { "esc to cancel" }.into(),
                    placeholder: "Writing down what you said…".into(),
                })
            }
            VoicePhase::Idle => None,
        }
    }
    fn io(&self, take: u64, abort: Signal) -> VoiceIo {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(bool, String)>();
        let progress = tx.clone();
        let weak = Rc::downgrade(&self.0);
        tokio::task::spawn_local(async move {
            while let Some((fetch, text)) = rx.recv().await {
                let Some(inner) = weak.upgrade() else {
                    break;
                };
                if fetch {
                    inner.host.notice(&text, NoticeTone::Info);
                } else if take == inner.state.borrow().take {
                    inner.state.borrow_mut().progress = Some(text);
                    inner.host.redraw();
                }
            }
        });
        VoiceIo {
            signal: abort,
            on_fetch: Arc::new(move |text| {
                let _ = tx.send((true, text.into()));
            }),
            on_progress: Arc::new(move |text| {
                let _ = progress.send((false, text.into()));
            }),
        }
    }
    fn sample(&self) {
        let mut state = self.0.state.borrow_mut();
        let Some(mic) = state.mic.clone().filter(|_| state.phase == VoicePhase::Listening) else {
            return;
        };
        state.levels.push(mic.level());
        if state.levels.len() > 16 {
            state.levels.remove(0);
        }
        let limit = (self.0.now)() - state.since >= 120000.;
        let quiet = state.held.is_none() && mic.spoke() && mic.quiet_ms() >= 3000.;
        drop(state);
        if limit {
            self.0.host.notice("Kumi listens for two minutes at a time: it's writing down what you said so far.", NoticeTone::Info);
            self.stop(false, None);
        } else if quiet {
            self.stop(false, None);
        }
    }
    fn extend(&self) {
        let timer = Signal::new();
        let deadline = tokio::time::Instant::now() + Duration::from_millis(350);
        {
            let mut state = self.0.state.borrow_mut();
            if let Some(old) = state.let_go.replace(timer.clone()) {
                old.cancel();
            }
        }
        let weak = Rc::downgrade(&self.0);
        tokio::task::spawn_local(async move {
            tokio::select! {_=timer.cancelled()=>{},_=tokio::time::sleep_until(deadline)=>{if let Some(inner)=weak.upgrade(){let this=Self(inner);let mut state=this.0.state.borrow_mut();state.let_go=None;if matches!(state.phase,VoicePhase::Starting|VoicePhase::Listening){state.quiet_until=(this.0.now)()+350.;let at=state.last_press;drop(state);this.stop(false,Some(at));}}}}
        });
    }
    fn ended(&self, why: Option<VoiceError>) {
        if let Some(why) = why {
            if self.0.state.borrow().mic.as_ref().map_or(0., |mic| mic.seconds()) < 1. {
                self.fail(why.into());
                return;
            }
            self.0.host.notice(&why.message, NoticeTone::Warn);
        }
        self.stop(false, None);
    }
    fn fail(&self, error: VoiceFailure) {
        let mic = self.0.state.borrow().mic.clone();
        self.reset();
        if let Some(mic) = mic {
            mic.cancel();
        }
        if matches!(error, VoiceFailure::Aborted(_)) {
            return;
        }
        let trouble = if let VoiceFailure::Voice(error) = &error { Some(error.trouble) } else { None };
        self.0.host.notice(
            &error.to_string(),
            if matches!(trouble, Some(VoiceTrouble::Quiet | VoiceTrouble::Words)) { NoticeTone::Info } else { NoticeTone::Warn },
        );
        if let Some(trouble) = trouble {
            self.0.host.offer(trouble);
        }
    }
    fn reset(&self) {
        let mut state = self.0.state.borrow_mut();
        if let Some(timer) = state.let_go.take() {
            timer.cancel();
        }
        if let Some(timer) = state.sampler.take() {
            timer.cancel();
        }
        state.phase = VoicePhase::Idle;
        state.mic = None;
        state.abort = None;
        state.held = None;
        state.send_after = false;
        state.progress = None;
        drop(state);
        self.0.host.animate(false);
        self.0.host.redraw();
    }
}
