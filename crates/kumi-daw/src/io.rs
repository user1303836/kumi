//! What a DAW's steps get from the app: where they say things to the producer and show that a step is working, how they
//! ask the producer, the environment, how they run programs, and answers to what they'd otherwise find out for
//! themselves (a test's, or the app's).
use crate::process::{default_run, Run};
use futures::future::LocalBoxFuture;
use kumi_common::abort::Signal;
use kumi_library::sources::join;
use kumi_types::system::Env;
use std::{
    any::{Any, TypeId},
    cell::RefCell,
    future::Future,
    rc::Rc,
};

/// Kumi's own folder: `KUMI_HOME`, else `.kumi` in the producer's home folder.
pub fn kumi_home(env: &Env) -> String {
    let home = || home::home_dir().unwrap_or_default().to_string_lossy().into_owned();
    env.get("KUMI_HOME").filter(|folder| !folder.is_empty()).cloned().unwrap_or_else(|| join(&home(), ".kumi"))
}

/// Where a DAW's steps say things to the producer: the app's terminal (which shows a step's words with a spinner while
/// it works), or what a quiet step or a test keeps.
pub trait Out {
    fn write(&self, text: &str);
    /// `line`, a step that takes a while ("Copying the bridge…"), shown until what this returns is stopped or dropped:
    /// kept, the line stays as its words. Where nothing moves, it's just the line, when kept.
    fn working(&self, line: &str, keep: bool) -> Working {
        if keep {
            self.write(&format!("{line}\n"));
        }
        Working::done()
    }
}

/// A step shown working ([`Out::working`]): stopping it, or dropping it, ends it. Stopping it twice does nothing.
pub struct Working(Option<Box<dyn FnOnce()>>);

impl Working {
    /// A step shown working, which `stop` ends.
    pub fn new(stop: impl FnOnce() + 'static) -> Self {
        Self(Some(Box::new(stop)))
    }
    /// Nothing to end.
    pub fn done() -> Self {
        Self(None)
    }
    pub fn stop(mut self) {
        if let Some(stop) = self.0.take() {
            stop();
        }
    }
}

impl Drop for Working {
    fn drop(&mut self) {
        if let Some(stop) = self.0.take() {
            stop();
        }
    }
}

/// `line` (a step: "Copying the bridge…") shown working while `work` runs.
pub async fn step<T, F: Future<Output = T>>(out: &dyn Out, line: &str, work: F, keep: bool) -> T {
    let working = out.working(line, keep);
    let result = work.await;
    working.stop();
    result
}

/// What a step said, kept rather than shown: a quiet step's words, for when it fails.
#[derive(Default)]
pub struct Kept(RefCell<String>);

impl Kept {
    pub fn text(&self) -> String {
        self.0.borrow().clone()
    }
}

impl Out for Kept {
    fn write(&self, text: &str) {
        self.0.borrow_mut().push_str(text);
    }
}

/// Saying nothing (the doctor's checks, which say what they found in their checks).
pub struct Quiet;

impl Out for Quiet {
    fn write(&self, _text: &str) {}
}

/// How a DAW's steps ask the producer: the app's terminal, or a test's answers.
pub trait Ask {
    /// Whether anyone can answer (a terminal, or a test's answers): a question nobody can answer is answered no.
    fn answers(&self) -> bool;
    /// A yes/no question; `ahead` says how to answer it beforehand, where a flag does ("add --yes").
    fn yes_no<'a>(&'a self, question: &'a str, ahead: Option<&'a str>) -> LocalBoxFuture<'a, bool>;
    /// The producer's key that stops a wait (Enter, Esc or Ctrl-C), heard until what this returns is dropped. Where
    /// there's no key to press, it never comes.
    fn stop_key(&self) -> StopKey {
        StopKey::never()
    }
}

/// A wait's stop key ([`Ask::stop_key`]): `signal` fires when the producer presses it. Dropped, the key isn't heard
/// any more.
pub struct StopKey {
    pub signal: Signal,
    release: Option<Box<dyn FnOnce()>>,
}

impl StopKey {
    /// A key heard until `release` runs (when this is dropped).
    pub fn new(signal: Signal, release: impl FnOnce() + 'static) -> Self {
        Self { signal, release: Some(Box::new(release)) }
    }
    /// No key to press.
    pub fn never() -> Self {
        Self { signal: Signal::new(), release: None }
    }
}

impl Drop for StopKey {
    fn drop(&mut self) {
        if let Some(release) = self.release.take() {
            release();
        }
    }
}

/// Nobody at a terminal to ask: a question is said on the output, with how to answer it beforehand where there's a way,
/// and answered no.
pub struct NoAnswer(pub Rc<dyn Out>);

impl Ask for NoAnswer {
    fn answers(&self) -> bool {
        false
    }
    fn yes_no<'a>(&'a self, question: &'a str, ahead: Option<&'a str>) -> LocalBoxFuture<'a, bool> {
        self.0.write(&format!("{question} (no terminal to answer in{})\n", ahead.map(|how| format!("; {how}")).unwrap_or_default()));
        Box::pin(async { false })
    }
}

/// A test's answers to a step's questions.
pub struct Answering(pub Rc<dyn Fn(String) -> LocalBoxFuture<'static, bool>>);

impl Ask for Answering {
    fn answers(&self) -> bool {
        true
    }
    fn yes_no<'a>(&'a self, question: &'a str, _ahead: Option<&'a str>) -> LocalBoxFuture<'a, bool> {
        (self.0)(question.to_string())
    }
}

/// Answers to what a DAW's steps would otherwise find out for themselves, by type: a test's (which Lives are installed,
/// a stand-in for the DAW's integration), or the app's (the doctor's look through each DAW's bridge). A DAW's crate
/// defines the types it reads; one of each is kept.
#[derive(Clone, Default)]
pub struct Answers(Rc<Vec<(TypeId, Rc<dyn Any>)>>);

impl Answers {
    /// These answers, with `answer` in place of any of its type.
    pub fn with<T: 'static>(self, answer: T) -> Self {
        let mut answers: Vec<(TypeId, Rc<dyn Any>)> = self.0.iter().filter(|(kind, _)| *kind != TypeId::of::<T>()).cloned().collect();
        answers.push((TypeId::of::<T>(), Rc::new(answer)));
        Self(Rc::new(answers))
    }
    /// The answer of type `T`, when there's one.
    pub fn get<T: 'static>(&self) -> Option<&T> {
        self.0.iter().find(|(kind, _)| *kind == TypeId::of::<T>()).and_then(|(_, answer)| answer.downcast_ref())
    }
    /// Whether there are none: the app has none for a session (each DAW's own integration starts there).
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// What a DAW's part of a flow gets from the app: where it says things, how it asks, the environment, how it runs
/// programs, and answers to what it would otherwise find out for itself. The app's own I/O for each flow (the doctor's,
/// an install's or an update's) gives one.
#[derive(Clone)]
pub struct DawIo {
    pub out: Rc<dyn Out>,
    pub ask: Rc<dyn Ask>,
    pub env: Env,
    pub run: Run,
    pub answers: Answers,
}

impl DawIo {
    /// Saying nothing, asking nobody, running this computer's programs, answering nothing.
    pub fn new(env: Env) -> Self {
        let out: Rc<dyn Out> = Rc::new(Quiet);
        Self { ask: Rc::new(NoAnswer(out.clone())), out, env, run: default_run(), answers: Answers::default() }
    }
}
