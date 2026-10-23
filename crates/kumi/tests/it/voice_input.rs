//! Voice state transitions, exercised with a fake microphone.
use async_trait::async_trait;
use kumi::{
    tui::{transcript::NoticeTone, voice::*},
    voice::{VoiceChange, VoiceChoices, VoiceIo},
};
use kumi_common::abort::{Aborted, Signal};
use kumi_runtime::{
    core::errors::RuntimeError,
    voice::{Heard, VoiceError, VoiceFailure, VoiceTrouble},
};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};
#[derive(Default)]
struct Host {
    words: RefCell<Vec<String>>,
    sent: Cell<usize>,
    notices: RefCell<Vec<(String, NoticeTone)>>,
    offers: RefCell<Vec<VoiceTrouble>>,
    names: RefCell<Vec<String>>,
    moving: Cell<bool>,
    redraws: Cell<usize>,
}
impl VoiceHost for Host {
    fn insert(&self, text: &str) {
        self.words.borrow_mut().push(text.into());
    }
    fn send(&self) {
        self.sent.set(self.sent.get() + 1);
    }
    fn notice(&self, text: &str, tone: NoticeTone) {
        self.notices.borrow_mut().push((text.into(), tone));
    }
    fn offer(&self, trouble: VoiceTrouble) {
        self.offers.borrow_mut().push(trouble);
    }
    fn names(&self) -> Vec<String> {
        self.names.borrow().clone()
    }
    fn redraw(&self) {
        self.redraws.set(self.redraws.get() + 1);
    }
    fn animate(&self, on: bool) {
        self.moving.set(on);
    }
}
struct Mic {
    calls: Rc<RefCell<Vec<String>>>,
    seconds: Cell<f64>,
    spoke: Cell<bool>,
    quiet: Cell<f64>,
    level: Cell<f64>,
    end: tokio::sync::watch::Sender<Option<Option<VoiceError>>>,
}
#[async_trait(?Send)]
impl VoiceListening for Mic {
    fn seconds(&self) -> f64 {
        self.seconds.get()
    }
    fn spoke(&self) -> bool {
        self.spoke.get()
    }
    fn quiet_ms(&self) -> f64 {
        self.quiet.get()
    }
    fn level(&self) -> f64 {
        self.level.get()
    }
    async fn stop(&self) -> Result<Heard, VoiceFailure> {
        self.calls.borrow_mut().push("stop".into());
        Ok(Heard { seconds: 2., spoke: true, ..Default::default() })
    }
    fn cancel(&self) {
        self.calls.borrow_mut().push("cancel".into());
        self.end.send_replace(Some(None));
    }
    async fn ended(&self) -> Option<VoiceError> {
        let mut rx = self.end.subscribe();
        loop {
            if let Some(why) = rx.borrow().clone() {
                return why;
            }
            if rx.changed().await.is_err() {
                return None;
            }
        }
    }
}
struct Control {
    calls: Rc<RefCell<Vec<String>>>,
    mic: Rc<Mic>,
    choices: RefCell<VoiceChoices>,
    listen_error: RefCell<Option<VoiceFailure>>,
    write_error: RefCell<Option<VoiceFailure>>,
    listen_gate: RefCell<Option<Signal>>,
    write_gate: RefCell<Option<Signal>>,
    io: RefCell<Option<VoiceIo>>,
}
#[async_trait(?Send)]
impl VoiceController for Control {
    fn system_language(&self) -> String {
        "ja".into()
    }
    fn choices(&self) -> VoiceChoices {
        self.choices.borrow().clone()
    }
    fn choose(&self, change: VoiceChange) -> Result<(), RuntimeError> {
        let mut choices = self.choices.borrow_mut();
        if let Some(send) = change.send {
            choices.send = send;
        }
        if let Some(language) = change.language {
            choices.language = language;
        }
        if let Some(mic) = change.microphone {
            choices.microphone = mic;
        }
        Ok(())
    }
    async fn listen(&self, io: VoiceIo) -> Result<Rc<dyn VoiceListening>, VoiceFailure> {
        self.calls.borrow_mut().push("listen".into());
        *self.io.borrow_mut() = Some(io.clone());
        let gate = self.listen_gate.borrow().clone();
        if let Some(gate) = gate {
            gate.cancelled().await;
        }
        if let Some(error) = self.listen_error.borrow().clone() {
            return Err(error);
        }
        self.mic.end.send_replace(None);
        Ok(self.mic.clone())
    }
    async fn write_down(&self, _: Heard, io: VoiceIo, names: Vec<String>) -> Result<String, VoiceFailure> {
        self.calls.borrow_mut().push(format!("write {}", names.join(", ")));
        *self.io.borrow_mut() = Some(io.clone());
        let gate = self.write_gate.borrow().clone();
        if let Some(gate) = gate {
            gate.cancelled().await;
        }
        if let Some(error) = self.write_error.borrow().clone() {
            return Err(error);
        }
        Ok("make the bass darker".into())
    }
    async fn microphones(&self) -> Result<Vec<String>, VoiceFailure> {
        Ok(vec!["MacBook Pro Microphone".into(), "Scarlett 2i2 USB".into()])
    }
    fn has_privacy(&self) -> bool {
        true
    }
    fn open_privacy(&self) {
        self.calls.borrow_mut().push("privacy".into());
    }
}
struct Harness {
    input: VoiceInput,
    control: Rc<Control>,
    host: Rc<Host>,
    start: tokio::time::Instant,
}
impl Harness {
    fn new() -> Self {
        let calls = Rc::new(RefCell::new(vec![]));
        let mic = Rc::new(Mic {
            calls: calls.clone(),
            seconds: Cell::new(2.),
            spoke: Cell::new(false),
            quiet: Cell::new(0.),
            level: Cell::new(0.8),
            end: tokio::sync::watch::channel(None).0,
        });
        let control = Rc::new(Control {
            calls,
            mic,
            choices: RefCell::new(VoiceChoices { send: false, language: "en".into(), microphone: None }),
            listen_error: RefCell::new(None),
            write_error: RefCell::new(None),
            listen_gate: RefCell::new(None),
            write_gate: RefCell::new(None),
            io: RefCell::new(None),
        });
        let host = Rc::new(Host::default());
        let start = tokio::time::Instant::now();
        let input = VoiceInput::with_clock(control.clone(), host.clone(), Rc::new(move || start.elapsed().as_secs_f64() * 1000.));
        Self { input, control, host, start }
    }
    fn view(&self) -> VoiceView {
        self.input.view(self.start.elapsed().as_secs_f64() * 1000.).unwrap()
    }
    fn calls(&self) -> Vec<String> {
        self.control.calls.borrow().clone()
    }
}
async fn flush() {
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
}
async fn advance(ms: u64) {
    tokio::time::advance(Duration::from_millis(ms)).await;
    flush().await;
}
#[tokio::test(start_paused = true)]
async fn tap_listens_meter_then_second_tap_inserts_with_names() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            *h.host.names.borrow_mut() = vec!["Night Drive".into()];
            h.input.press(false);
            h.input.release();
            flush().await;
            assert_eq!(h.calls(), ["listen"]);
            assert_eq!(h.input.phase(), VoicePhase::Listening);
            assert_eq!(h.view().hint, "ctrl+t to stop · enter to send · esc to cancel");
            advance(200).await;
            assert!(h.view().status.iter().any(|s| s.text == "▇"));
            advance(250).await;
            h.input.press(false);
            h.input.release();
            flush().await;
            assert_eq!(h.calls(), ["listen", "stop", "write Night Drive"]);
            assert_eq!(*h.host.words.borrow(), ["make the bass darker"]);
            assert_eq!(h.host.sent.get(), 0);
            assert!(!h.input.active());
            assert!(!h.host.moving.get());
        })
        .await;
}
#[tokio::test(start_paused = true)]
async fn explicit_and_legacy_holds_stop_on_release_and_short_takes_are_dropped() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            h.input.press(false);
            flush().await;
            h.input.press(true);
            assert_eq!(h.view().hint, "let go to stop · esc to cancel");
            advance(400).await;
            h.input.release();
            flush().await;
            assert_eq!(h.calls(), ["listen", "stop", "write "]);
            let h = Harness::new();
            h.input.press(false);
            flush().await;
            advance(450).await;
            for _ in 0..6 {
                h.input.press(false);
                advance(10).await;
            }
            assert_eq!(h.calls(), ["listen"]);
            advance(350).await;
            assert_eq!(h.calls(), ["listen", "stop", "write "]);
            advance(350).await;
            h.input.press(false);
            flush().await;
            h.input.press(false);
            advance(350).await;
            assert_eq!(h.calls().iter().filter(|s| s.starts_with("write")).count(), 1);
            assert!(!h.input.active());
        })
        .await;
}
#[tokio::test(start_paused = true)]
async fn enter_sends_cancel_discards_and_quiet_auto_stops() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            h.input.press(false);
            h.input.release();
            flush().await;
            advance(450).await;
            assert!(h.input.enter());
            flush().await;
            assert_eq!(h.host.sent.get(), 1);
            h.input.start(false);
            flush().await;
            assert!(h.input.cancel());
            assert!(!h.input.cancel());
            assert_eq!(h.calls().last().unwrap(), "cancel");
            h.input.start(false);
            flush().await;
            advance(450).await;
            h.control.mic.spoke.set(true);
            h.control.mic.quiet.set(3200.);
            advance(60).await;
            assert!(!h.input.active());
            assert_eq!(h.host.words.borrow().len(), 2);
            h.control.choose(VoiceChange { send: Some(true), ..Default::default() }).unwrap();
            h.input.start(false);
            flush().await;
            advance(450).await;
            assert_eq!(h.host.sent.get(), 2);
        })
        .await;
}
#[tokio::test(start_paused = true)]
async fn trouble_tones_offers_and_abort_match_source() {
    tokio::task::LocalSet::new()
        .run_until(async {
            for (trouble, tone) in [
                (VoiceTrouble::Permission, NoticeTone::Warn),
                (VoiceTrouble::Quiet, NoticeTone::Info),
                (VoiceTrouble::Words, NoticeTone::Info),
                (VoiceTrouble::Silence, NoticeTone::Warn),
            ] {
                let h = Harness::new();
                *h.control.listen_error.borrow_mut() = Some(VoiceError { trouble, message: "explanation".into() }.into());
                h.input.start(false);
                flush().await;
                assert!(!h.input.active());
                assert_eq!(*h.host.notices.borrow(), [("explanation".into(), tone)]);
                assert_eq!(*h.host.offers.borrow(), [trouble]);
            }
            let h = Harness::new();
            *h.control.listen_error.borrow_mut() = Some(VoiceFailure::Aborted(Aborted));
            h.input.start(false);
            flush().await;
            assert!(h.host.notices.borrow().is_empty());
        })
        .await;
}
#[tokio::test(start_paused = true)]
async fn pending_start_progress_cancel_and_stale_write_never_insert() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            let gate = Signal::new();
            *h.control.listen_gate.borrow_mut() = Some(gate.clone());
            h.input.press(false);
            flush().await;
            advance(3100).await;
            assert!(h.view().status.iter().any(|s| s.text.contains("waiting for the microphone")));
            let io = h.control.io.borrow().clone().unwrap();
            (io.on_progress)("getting speech model");
            (io.on_fetch)("fetching model");
            flush().await;
            h.input.press(false);
            assert_eq!(h.input.phase(), VoicePhase::Starting);
            assert!(h.view().status.iter().any(|s| s.text.contains("getting speech model")));
            assert!(!h.input.enter());
            assert!(io.signal.is_cancelled());
            gate.cancel();
            flush().await;
            assert_eq!(h.calls(), ["listen", "cancel"]);
            assert_eq!(h.host.notices.borrow()[0].0, "fetching model");
            let h = Harness::new();
            let gate = Signal::new();
            *h.control.write_gate.borrow_mut() = Some(gate.clone());
            h.input.start(false);
            flush().await;
            advance(450).await;
            h.input.stop(false, None);
            flush().await;
            assert_eq!(h.input.phase(), VoicePhase::Writing);
            assert!(h.input.enter());
            assert_eq!(h.view().hint, "sends once it's written · esc to cancel");
            h.input.cancel();
            gate.cancel();
            flush().await;
            assert!(h.host.words.borrow().is_empty());
            assert_eq!(h.host.sent.get(), 0);
        })
        .await;
}
#[tokio::test(start_paused = true)]
async fn ended_microphone_and_two_minute_limit_preserve_long_takes() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let h = Harness::new();
            h.input.start(false);
            flush().await;
            advance(450).await;
            h.control.mic.end.send_replace(Some(Some(VoiceError { trouble: VoiceTrouble::Device, message: "unplugged".into() })));
            flush().await;
            assert_eq!(h.host.notices.borrow()[0], ("unplugged".into(), NoticeTone::Warn));
            assert_eq!(h.host.words.borrow().len(), 1);
            let h = Harness::new();
            h.control.mic.seconds.set(0.5);
            h.input.start(false);
            flush().await;
            h.control.mic.end.send_replace(Some(Some(VoiceError { trouble: VoiceTrouble::Device, message: "unplugged".into() })));
            flush().await;
            assert_eq!(*h.host.offers.borrow(), [VoiceTrouble::Device]);
            assert!(h.host.words.borrow().is_empty());
            let h = Harness::new();
            h.input.start(false);
            flush().await;
            advance(120000).await;
            assert!(!h.input.active());
            assert!(h.host.notices.borrow()[0].0.contains("two minutes at a time"));
            assert_eq!(h.host.words.borrow().len(), 1);
        })
        .await;
}
