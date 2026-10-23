#![allow(dead_code, unused_imports)]
use async_trait::async_trait;
use kumi::{
    tui::{transcript::NoticeTone, voice::*},
    voice::{VoiceChange, VoiceChoices, VoiceIo},
};
use kumi_common::abort::{Aborted, Signal};
use kumi_media::voice::{Heard, VoiceError, VoiceFailure, VoiceTrouble};
use kumi_types::errors::RuntimeError;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};
pub struct Mic {
    pub calls: Rc<RefCell<Vec<String>>>,
    pub seconds: Cell<f64>,
    pub spoke: Cell<bool>,
    pub quiet: Cell<f64>,
    pub level: Cell<f64>,
    pub end: tokio::sync::watch::Sender<Option<Option<VoiceError>>>,
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
pub struct Control {
    pub words: RefCell<String>,
    pub calls: Rc<RefCell<Vec<String>>>,
    pub mic: Rc<Mic>,
    pub choices: RefCell<VoiceChoices>,
    pub listen_error: RefCell<Option<VoiceFailure>>,
    pub write_error: RefCell<Option<VoiceFailure>>,
    pub listen_gate: RefCell<Option<Signal>>,
    pub write_gate: RefCell<Option<Signal>>,
    pub io: RefCell<Option<VoiceIo>>,
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
        Ok(self.words.borrow().clone())
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
impl Control {
    pub fn new(words: &str) -> Rc<Self> {
        let calls = Rc::new(RefCell::new(vec![]));
        let mic = Rc::new(Mic {
            calls: calls.clone(),
            seconds: Cell::new(2.0),
            spoke: Cell::new(false),
            quiet: Cell::new(0.0),
            level: Cell::new(0.8),
            end: tokio::sync::watch::channel(None).0,
        });
        Rc::new(Self {
            words: RefCell::new(words.into()),
            calls,
            mic,
            choices: RefCell::new(VoiceChoices { send: false, language: "en".into(), microphone: None }),
            listen_error: RefCell::new(None),
            write_error: RefCell::new(None),
            listen_gate: RefCell::new(None),
            write_gate: RefCell::new(None),
            io: RefCell::new(None),
        })
    }
}
