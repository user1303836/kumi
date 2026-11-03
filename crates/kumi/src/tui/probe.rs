//! Asking the terminal what it is, once, as the full screen starts: its name and version (XTVERSION), whether it draws a
//! frame at once (mode 2026), whether it speaks the kitty keyboard protocol, and its colours (OSC 10 and 11). DA1 goes
//! last, so its answer says the others have come. Over SSH, where `TERM_PROGRAM` doesn't arrive, the answers are what
//! turn on kitty's pictures and the terminal's own notifications; the environment stays the fallback.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use kumi_common::abort::Signal;

use super::keys::{InputEvent, InputParser, Reply};
use super::style::Rgb;
use super::tty::{TtyInput, TtyOutput};
use crate::input::Utf8Decoder;

/// The questions, in the order they're answered: XTVERSION, DECRQM 2026, the kitty keyboard query, OSC 10, OSC 11, DA1.
pub const QUERIES: &str = "\u{1b}[>0q\u{1b}[?2026$p\u{1b}[?u\u{1b}]10;?\u{1b}\\\u{1b}]11;?\u{1b}\\\u{1b}[c";
/// How long Kumi waits for the answers: a terminal that's quiet past it is taken at its environment's word.
pub const BUDGET: Duration = Duration::from_millis(250);

/// What the terminal said of itself.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Answers {
    /// Its name and version, as XTVERSION gives them (`ghostty 1.1.3`).
    pub version: Option<String>,
    /// Whether it draws a frame at once (mode 2026): None when it didn't say.
    pub sync: Option<bool>,
    pub kitty_keys: bool,
    pub foreground: Option<Rgb>,
    pub background: Option<Rgb>,
    /// DA1 came: what was asked before it has been answered, or won't be.
    pub complete: bool,
}

impl Answers {
    pub fn take(&mut self, reply: Reply) {
        match reply {
            Reply::Version(version) => self.version = Some(version.trim().to_string()).filter(|version| !version.is_empty()),
            // 1 and 2 are set and reset, 3 and 4 always so; 0 is a mode the terminal doesn't know.
            Reply::Mode { mode: 2026, value } => self.sync = Some(matches!(value, 1 | 2 | 3)),
            Reply::Mode { .. } => {}
            Reply::KittyKeys(_) => self.kitty_keys = true,
            Reply::Color { which: 10, rgb } => self.foreground = rgb.or(self.foreground),
            Reply::Color { rgb, .. } => self.background = rgb.or(self.background),
            Reply::Attributes(_) => self.complete = true,
            // (asked for by nothing here: once the screen is Kumi's, a theme that follows the terminal reads it)
            Reply::Scheme { .. } => {}
        }
    }
}

/// What the probe heard, and every byte it read: keys typed meanwhile are among them, for the screen's own reader to
/// read again, which also takes the answers there as answers.
#[derive(Clone, Debug, Default)]
pub struct Probed {
    pub answers: Answers,
    pub read: Vec<u8>,
}

/// Asks, and waits for DA1's answer or `budget`, whichever comes first. The terminal is raw meanwhile, and put back as
/// it was. A terminal that can't be made raw isn't asked.
pub async fn probe(input: &Rc<dyn TtyInput>, output: &Rc<dyn TtyOutput>, budget: Duration) -> Probed {
    let Some(asking) = ask(input, output) else { return Probed::default() };
    asking.answered(budget, &Signal::new()).await;
    asking.end()
}

/// The terminal asked what it is ([`ask`]): raw, and what it says (and any key typed meanwhile) read. Once DA1 has come
/// it's given back as it was at once, so ctrl+c stops a slow start again. A terminal that hasn't answered by the end of
/// the wait stays raw and read until [`Asking::end`], so answers that come late are read, never echoed onto the screen.
/// Dropped without being ended (a start that fails), it's given back all the same.
pub struct Asking {
    input: Rc<dyn TtyInput>,
    was_raw: bool,
    answers: Rc<RefCell<Answers>>,
    read: Rc<RefCell<Vec<u8>>>,
    done: Signal,
    parser: InputParser,
    ended: Cell<bool>,
}

/// Makes the terminal raw, reads it, and asks: None when it can't be made raw (then it isn't asked).
pub fn ask(input: &Rc<dyn TtyInput>, output: &Rc<dyn TtyOutput>) -> Option<Asking> {
    let was_raw = input.is_raw();
    input.set_raw_mode(true).ok()?;
    let answers = Rc::new(RefCell::new(Answers::default()));
    let read = Rc::new(RefCell::new(Vec::new()));
    let done = Signal::new();
    let parser = InputParser::new(Rc::new({
        let (answers, done) = (answers.clone(), done.clone());
        move |event| {
            if let InputEvent::Reply(reply) = event {
                let mut answers = answers.borrow_mut();
                answers.take(reply);
                if answers.complete {
                    done.cancel();
                }
            }
        }
    }));
    let decoder = Rc::new(RefCell::new(Utf8Decoder::new()));
    input.resume(Rc::new({
        let (read, parser) = (read.clone(), parser.clone());
        move |chunk| {
            read.borrow_mut().extend_from_slice(chunk);
            let text = decoder.borrow_mut().write(chunk);
            parser.push(&text);
        }
    }));
    output.write(QUERIES);
    Some(Asking { input: input.clone(), was_raw, answers, read, done, parser, ended: Cell::new(false) })
}

impl Asking {
    /// Until DA1's answer (what was asked before it has been answered, or won't be), `budget`, or `stop`. With DA1 in,
    /// the terminal is given back now.
    pub async fn answered(&self, budget: Duration, stop: &Signal) {
        tokio::select! {
            _ = tokio::time::timeout(budget, self.done.cancelled()) => {}
            _ = stop.cancelled() => {}
        }
        if self.answers.borrow().complete {
            self.give_back();
        }
    }

    /// Gives the terminal back as it was: what it said, and every byte read (keys typed meanwhile among them, for the
    /// screen's own reader to read again, which also takes the answers there as answers).
    pub fn end(self) -> Probed {
        self.give_back();
        let answers = self.answers.borrow().clone();
        let read = std::mem::take(&mut *self.read.borrow_mut());
        Probed { answers, read }
    }

    fn give_back(&self) {
        if !self.ended.replace(true) {
            self.input.pause();
            self.parser.dispose();
            let _ = self.input.set_raw_mode(self.was_raw);
        }
    }
}

impl Drop for Asking {
    fn drop(&mut self) {
        self.give_back();
    }
}
