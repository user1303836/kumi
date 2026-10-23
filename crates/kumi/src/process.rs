//! Asking in the terminal, for whatever Kumi installs or updates: a yes/no line read in the terminal's line mode, and
//! the terminal as a DAW's steps ask through it. Running programs is kumi-daw's (`kumi_daw::process`).
use crate::{input::TerminalInput, tui::tty::TtyOutput};
use futures::future::LocalBoxFuture;
use kumi_common::js::string::trim;
use std::rc::Rc;
/// Asks the producer a yes/no question.
pub type Confirm = Rc<dyn Fn(String) -> LocalBoxFuture<'static, bool>>;
struct InputGuard(Option<Rc<dyn TerminalInput>>);
impl Drop for InputGuard {
    fn drop(&mut self) {
        if let Some(input) = &self.0 {
            input.pause();
        }
    }
}
/// Read a yes/no line in the terminal's normal line mode. Without a terminal nobody can answer, so it's no: the
/// question is still said, with `ahead` (how to answer it beforehand, "add --yes") when there's a way.
pub async fn ask_yes_no(input: Option<Rc<dyn TerminalInput>>, out: Rc<dyn TtyOutput>, question: &str, ahead: Option<&str>) -> bool {
    let Some(input) = input.filter(|input| input.is_tty()) else {
        out.write(&format!("{question} (no terminal to answer in{})\n", ahead.map(|how| format!("; {how}")).unwrap_or_default()));
        return false;
    };
    out.write(&format!("{question} [y/N] "));
    let _guard = InputGuard(Some(input.clone()));
    let (send, receive) = tokio::sync::oneshot::channel();
    let send = Rc::new(std::cell::RefCell::new(Some(send)));
    let bytes = Rc::new(std::cell::RefCell::new(vec![]));
    input.on_end(Rc::new({
        let send = send.clone();
        let bytes = bytes.clone();
        let input = Rc::downgrade(&input);
        move || {
            if let Some(send) = send.borrow_mut().take() {
                if let Some(input) = input.upgrade() {
                    input.pause();
                }
                let _ = send.send(String::from_utf8_lossy(&bytes.borrow()).into_owned());
            }
        }
    }));
    input.resume(Rc::new({
        let send = send.clone();
        let input = Rc::downgrade(&input);
        move |chunk| {
            let end = chunk.iter().position(|b| matches!(b, b'\n' | b'\r'));
            bytes.borrow_mut().extend_from_slice(&chunk[..end.unwrap_or(chunk.len())]);
            if end.is_some() {
                if let Some(send) = send.borrow_mut().take() {
                    if let Some(input) = input.upgrade() {
                        input.pause();
                    }
                    let _ = send.send(String::from_utf8_lossy(&bytes.borrow()).into_owned());
                }
            }
        }
    }));
    receive.await.is_ok_and(|answer| matches!(trim(&answer).to_ascii_lowercase().as_str(), "y" | "yes"))
}
/// The terminal as a DAW's steps ask through it ([`kumi_daw::io::Ask`]): a test's answers (`confirm`), else a yes/no
/// line read from `input` and said on `out`, which nobody at a terminal answers no.
pub struct TerminalAsk {
    pub input: Option<Rc<dyn TerminalInput>>,
    pub out: Rc<dyn TtyOutput>,
    pub confirm: Option<Confirm>,
}

impl kumi_daw::io::Ask for TerminalAsk {
    fn answers(&self) -> bool {
        self.confirm.is_some() || self.input.as_ref().is_some_and(|input| input.is_tty())
    }
    fn yes_no<'a>(&'a self, question: &'a str, ahead: Option<&'a str>) -> LocalBoxFuture<'a, bool> {
        Box::pin(async move {
            match &self.confirm {
                Some(confirm) => confirm(question.to_string()).await,
                None => ask_yes_no(self.input.clone(), self.out.clone(), question, ahead).await,
            }
        })
    }
    /// In raw mode, so the key needs no Enter after it: Enter, Esc or Ctrl-C.
    fn stop_key(&self) -> kumi_daw::io::StopKey {
        let Some(input) = self.input.clone().filter(|input| input.is_tty()) else { return kumi_daw::io::StopKey::never() };
        let signal = kumi_common::abort::Signal::new();
        let _ = input.set_raw_mode(true);
        input.resume(Rc::new({
            let signal = signal.clone();
            move |chunk| {
                if chunk.iter().any(|b| matches!(b, 3 | 27 | b'\r' | b'\n')) {
                    signal.cancel();
                }
            }
        }));
        kumi_daw::io::StopKey::new(signal, move || {
            let _ = input.set_raw_mode(false);
            input.pause();
        })
    }
}
