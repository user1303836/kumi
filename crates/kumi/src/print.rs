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
use tokio::sync::Notify;

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
    /// The answer's words made safe to show, as they come (with --json too: a key split across two of them is still
    /// a key).
    words: RefCell<StreamingText>,
    /// Whatever went to stdout ends with a newline (or nothing did).
    ended_line: Cell<bool>,
    /// How the answer ended, by the session's events.
    ended_with: Cell<Option<StopReason>>,
    failed: Cell<bool>,
    /// Ctrl+c came: the answer is being stopped.
    stopping: Cell<bool>,
    /// A second ctrl+c, or Kumi told to close: stop waiting for the session.
    done_waiting: Notify,
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
            ended_with: Cell::new(None),
            failed: Cell::new(false),
            stopping: Cell::new(false),
            done_waiting: Notify::new(),
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
    /// The answer's words so far: what's held back of them (the start of what may be a key), then a newline; with
    /// --json, what's held back as a text event of its own.
    fn end_words(&self) {
        let tail = self.words.borrow_mut().finish();
        if self.options.json {
            if !tail.is_empty() {
                self.json_line(&SessionEvent::Kernel(KernelEvent::Text { text: tail }));
            }
        } else {
            self.write(&tail);
            if !self.ended_line.get() {
                self.write("\n");
            }
        }
    }
    /// `asking`, unless Kumi stops waiting first (a second ctrl+c, or told to close): None then.
    async fn unless_stopped<T>(&self, asking: impl std::future::Future<Output = T>) -> Option<T> {
        tokio::select! {
            biased;
            _ = self.done_waiting.notified() => None,
            asked = asking => Some(asked),
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
        match self.ended_with.get() {
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
            match inner.unless_stopped(inner.session.op(op::Start {})).await {
                Some(Ok(())) => {}
                Some(Err(error)) => {
                    inner.say(&format!("Kumi couldn't start: {}", error.message()));
                    inner.close().await;
                    return FAILED;
                }
                None => {
                    inner.close().await;
                    return STOPPED;
                }
            }
            let request = inner.options.request.clone();
            let code = match inner.instant(&request).await {
                Some(code) => code,
                None => {
                    let asked = inner.unless_stopped(inner.session.op(op::Submit { input: request, pinned: None, attachments: vec![] }));
                    let asked = asked.await;
                    // An answer that failed or was left part way ends its words too (one that came in full ended them at
                    // turn-complete).
                    inner.end_words();
                    match asked {
                        Some(Ok(())) => inner.ending(),
                        Some(Err(error)) => {
                            inner.say(&error.message());
                            FAILED
                        }
                        None => STOPPED,
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
                if inner.options.json {
                    inner.end_words();
                } else {
                    inner.say(message);
                }
            }
            SessionEvent::TurnComplete { result, .. } => {
                inner.ended_with.set(Some(result.stop_reason));
                inner.end_words();
                if !inner.options.json && result.stop_reason == StopReason::MaxSteps {
                    inner.say("Kumi reached its step limit for one answer. Ask it to carry on.");
                }
            }
            // The words go as they're made safe; with --json, as text events of what's safe so far.
            SessionEvent::Kernel(KernelEvent::Text { text }) => {
                let shown = inner.words.borrow_mut().push(text);
                if !inner.options.json {
                    inner.write(&shown);
                } else if !shown.is_empty() {
                    inner.json_line(&SessionEvent::Kernel(KernelEvent::Text { text: shown }));
                }
                return;
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
            self.0.done_waiting.notify_one();
            return;
        }
        self.0.session.tell(op::Cancel {}.into());
    }
    /// Told to close (SIGTERM): Kumi stops waiting for the answer, closes the session and ends with 130.
    fn close(&self) -> LocalBoxFuture<'static, i32> {
        self.0.stopping.set(true);
        self.0.done_waiting.notify_one();
        async { STOPPED }.boxed_local()
    }
}
