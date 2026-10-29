//! `kumi -p "<request>"`: one request, answered, for a script. The answer's words go to stdout as the model writes them
//! (or, with `--json`, every event the session sends, a JSON line each), what went wrong goes to stderr, and the exit code
//! says how it ended. The session is reached only through its protocol.
use crate::{config::safe_error_message, terminal::Terminal, text::StreamingText, tui::tty::TtyOutput};
use futures::{future::LocalBoxFuture, FutureExt};
use kumi_agent::core::{
    contracts::*,
    phrases,
    protocol::{op, InProcess, SessionHandle},
};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};

/// The answer came in full.
pub const ANSWERED: i32 = 0;
/// Something went wrong: Kumi couldn't start, or the answer failed (not signed in, say). stderr says what.
pub const FAILED: i32 = 1;
/// The answer stopped at its step limit: what it did so far stands, and asking it to carry on goes on from there.
pub const STEP_LIMIT: i32 = 3;
/// Stopped with ctrl+c.
pub const STOPPED: i32 = 130;

pub struct PrintOptions {
    pub controller: Rc<dyn SessionController>,
    pub request: String,
    pub json: bool,
    pub out: Rc<dyn TtyOutput>,
    pub err: Rc<dyn TtyOutput>,
    /// What never reaches stdout or stderr (keys and tokens), taken out as plain mode and the TUI take it out.
    pub secrets: Vec<String>,
    /// Fixed phrases ("tempo 124") change the DAW at once, with no model call (`instantPhrases`).
    pub instant: bool,
    /// Said to stderr before the request goes (the bridge's word, an unprotected sign-in file).
    pub startup_notice: Option<String>,
}

#[derive(Clone)]
pub struct Print(Rc<Inner>);

struct Inner {
    session: Rc<dyn SessionHandle>,
    options: PrintOptions,
    /// The answer's words made safe to show, as they come.
    words: RefCell<StreamingText>,
    /// Whatever went to stdout ends with a newline (or nothing did).
    ended_line: Cell<bool>,
    /// How the answer ended, by the session's events.
    stop: Cell<Option<StopReason>>,
    failed: Cell<bool>,
    /// Ctrl+c came: the answer is being stopped.
    stopping: Cell<bool>,
}

impl Print {
    pub fn new(options: PrintOptions) -> Print {
        let session: Rc<dyn SessionHandle> = InProcess::new(options.controller.clone());
        let words = RefCell::new(StreamingText::new(&options.secrets));
        Print(Rc::new(Inner {
            session,
            options,
            words,
            ended_line: Cell::new(true),
            stop: Cell::new(None),
            failed: Cell::new(false),
            stopping: Cell::new(false),
        }))
    }
}

impl Inner {
    fn write(&self, text: &str) {
        if !text.is_empty() {
            self.options.out.write(text);
            self.ended_line.set(text.ends_with('\n'));
        }
    }
    fn say(&self, message: &str) {
        self.options.err.write(&format!("{}\n", safe_error_message(Some(message), &self.options.secrets)));
    }
    /// The answer's words so far, ended with a newline.
    fn end_words(&self) {
        let tail = self.words.borrow_mut().finish();
        self.write(&tail);
        if !self.ended_line.get() {
            self.write("\n");
        }
    }
    /// An event as a JSON line, its secrets taken out.
    fn json_line(&self, event: &SessionEvent) {
        let Ok(mut line) = serde_json::to_string(event) else { return };
        for secret in self.options.secrets.iter().filter(|secret| !secret.is_empty()) {
            line = line.replace(secret.as_str(), "[redacted]");
        }
        self.write(&format!("{line}\n"));
    }
    /// A fixed phrase, run at once with no model call: how it ended, or None when the request isn't one (it goes to the
    /// model as written).
    async fn instant(&self, request: &str) -> Option<i32> {
        if !self.options.instant || !self.session.capabilities().instant || phrases::parse(request).is_none() {
            return None;
        }
        let outcome = self.session.op(op::Instant { text: request.into() }).await.ok().flatten()?;
        // (with --json, its changes came as events)
        if !self.options.json {
            if outcome.is_error {
                self.say(&outcome.text);
            } else {
                self.write(&format!("{}\n", safe_error_message(Some(&outcome.text), &self.options.secrets)));
            }
        }
        Some(if outcome.is_error { FAILED } else { ANSWERED })
    }
    /// How the answer ended, by what the session said.
    fn ending(&self) -> i32 {
        if self.failed.get() {
            return FAILED;
        }
        match self.stop.get() {
            Some(StopReason::Completed) => ANSWERED,
            Some(StopReason::MaxSteps) => STEP_LIMIT,
            Some(StopReason::Cancelled) => STOPPED,
            None if self.stopping.get() => STOPPED,
            None => FAILED,
        }
    }
    /// Closing, within the time the TUI gives it: quitting never waits on the network.
    async fn close(&self) {
        let _ = tokio::time::timeout(Duration::from_secs(6), self.session.op(op::Close {})).await;
    }
}

impl Terminal for Print {
    fn run(&self) -> LocalBoxFuture<'static, i32> {
        let inner = self.0.clone();
        async move {
            if let Some(notice) = &inner.options.startup_notice {
                inner.say(notice);
            }
            if let Err(error) = inner.session.op(op::Start {}).await {
                inner.say(&format!("Kumi couldn't start: {}", error.message()));
                inner.close().await;
                return FAILED;
            }
            let request = inner.options.request.clone();
            let code = match inner.instant(&request).await {
                Some(code) => code,
                None => {
                    let asked = inner.session.op(op::Submit { input: request, pinned: None, attachments: vec![] }).await;
                    // An answer that failed part way ends its line too (one that came in full ended at turn-complete).
                    if !inner.options.json {
                        inner.end_words();
                    }
                    match asked {
                        Ok(()) => inner.ending(),
                        Err(error) => {
                            inner.say(&error.message());
                            FAILED
                        }
                    }
                }
            };
            inner.close().await;
            code
        }
        .boxed_local()
    }
    fn handle_event(&self, event: SessionEvent) {
        let inner = &self.0;
        match &event {
            SessionEvent::Error { message, .. } => {
                inner.failed.set(true);
                if !inner.options.json {
                    inner.say(message);
                }
            }
            SessionEvent::TurnComplete { result, .. } => {
                inner.stop.set(Some(result.stop_reason));
                if !inner.options.json {
                    inner.end_words();
                    if result.stop_reason == StopReason::MaxSteps {
                        inner.say("Kumi reached its step limit for one answer. Ask it to carry on.");
                    }
                }
            }
            SessionEvent::Kernel(KernelEvent::Text { text }) if !inner.options.json => {
                let shown = inner.words.borrow_mut().push(text);
                inner.write(&shown);
            }
            SessionEvent::Notice { message } if !inner.options.json => inner.say(message),
            // The rest (steps, changes, the DAW's state) is for a screen, or for --json.
            _ => {}
        }
        if inner.options.json {
            inner.json_line(&event);
        }
    }
    fn offer_update(&self, _: &str) {}
    /// Ctrl+c stops the answer (and Kumi ends with 130); a second one stops waiting for it.
    fn interrupt(&self) {
        if self.0.stopping.replace(true) {
            let inner = self.0.clone();
            tokio::task::spawn_local(async move { inner.close().await });
            return;
        }
        self.0.session.tell(op::Cancel {}.into());
    }
    fn close(&self) -> LocalBoxFuture<'static, i32> {
        let inner = self.0.clone();
        async move {
            inner.close().await;
            STOPPED
        }
        .boxed_local()
    }
}
