//! The plain terminal's goodbye is always its last line: a /status still working out its conversation when /quit
//! comes says its second line first, and nothing is said after the goodbye.
#[path = "../support/models.rs"]
mod models;
use async_trait::async_trait;
use kumi::{
    input::{ByteListener, TerminalInput},
    terminal::{create_terminal, Terminal, TerminalOptions},
    tui::tty::TtyOutput,
};
use kumi_agent::core::contracts::*;
use kumi_types::contracts::*;
use kumi_types::errors::RuntimeError;
use models::FakeModels;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
use tokio::sync::Notify;

const FAREWELL: &str = "Kumi closed. Each Set's conversation continues next time.";

#[derive(Default)]
struct Input {
    data: RefCell<Option<ByteListener>>,
}
impl TerminalInput for Input {
    fn is_tty(&self) -> bool {
        false
    }
    fn is_raw(&self) -> bool {
        false
    }
    fn set_raw_mode(&self, _: bool) -> std::io::Result<()> {
        Ok(())
    }
    fn resume(&self, f: ByteListener) {
        *self.data.borrow_mut() = Some(f);
    }
    fn pause(&self) {
        self.data.borrow_mut().take();
    }
    fn on_end(&self, _: Rc<dyn Fn()>) {}
}
impl Input {
    fn write(&self, s: &str) {
        let callback = self.data.borrow().clone();
        if let Some(callback) = callback {
            callback(s.as_bytes());
        }
    }
}
#[derive(Default)]
struct Out {
    text: RefCell<String>,
}
impl TtyOutput for Out {
    fn is_tty(&self) -> bool {
        false
    }
    fn columns(&self) -> Option<i32> {
        Some(80)
    }
    fn rows(&self) -> Option<i32> {
        Some(24)
    }
    fn write(&self, s: &str) {
        self.text.borrow_mut().push_str(s);
    }
}
/// A session whose first look at its conversation (the one /status asks for) waits until it's let go; later ones
/// answer at once.
#[derive(Default)]
struct Control {
    asked: Cell<usize>,
    gate: Notify,
}
#[async_trait(?Send)]
impl SessionController for Control {
    async fn start(&self) -> Result<(), RuntimeError> {
        Ok(())
    }
    async fn submit(&self, _: &str, _: Option<PinnedNode>) -> Result<(), RuntimeError> {
        Ok(())
    }
    async fn refresh(&self) -> Result<(), RuntimeError> {
        Ok(())
    }
    async fn new_conversation(&self) -> Result<(), RuntimeError> {
        Ok(())
    }
    async fn cancel(&self) -> Result<(), RuntimeError> {
        Ok(())
    }
    async fn close(&self) -> Result<(), RuntimeError> {
        Ok(())
    }
    fn status(&self) -> SessionStatus {
        SessionStatus { state: TurnState::Idle, connection: ConnectionState::Disconnected, turns: 0, max_turns: None, observation: None }
    }
    async fn undo(&self, _: Option<&str>) -> Result<Option<ChangeRecord>, RuntimeError> {
        Ok(None)
    }
    async fn conversation_now(&self) -> Option<ConversationNow> {
        self.asked.set(self.asked.get() + 1);
        if self.asked.get() == 1 {
            self.gate.notified().await;
        }
        Some(ConversationNow { id: "now001".into(), title: "Bass ideas".into(), kept: true, touched: true })
    }
}

async fn flush() {
    for _ in 0..16 {
        tokio::task::yield_now().await;
    }
}

/// /status, then /quit, with /status's conversation let go before /quit or only once Kumi is closing: what was said.
async fn status_then_quit(let_go_first: bool) -> String {
    let (input, out, control) = (Rc::new(Input::default()), Rc::new(Out::default()), Rc::new(Control::default()));
    let options = TerminalOptions::new(control.clone(), input.clone(), out.clone(), FakeModels::chosen(), "inference-only");
    let terminal = create_terminal(options);
    drop(terminal.run());
    flush().await;
    input.write("/status\n");
    flush().await;
    assert_eq!(control.asked.get(), 1, "/status asked for its conversation");
    if let_go_first {
        control.gate.notify_one();
        flush().await;
    }
    input.write("/quit\n");
    flush().await;
    if !let_go_first {
        assert!(!out.text.borrow().contains(FAREWELL), "the goodbye waits for /status: {}", out.text.borrow());
        control.gate.notify_one();
    }
    assert_eq!(terminal.close().await, 0);
    flush().await;
    let text = out.text.borrow().clone();
    text
}

/// Both of /status's lines, in order, then the goodbye, with nothing tagged after it.
fn ends_with_the_goodbye(said: &str) {
    let lines: Vec<&str> = said.lines().filter(|line| !line.is_empty()).collect();
    let goodbye = lines.iter().position(|line| *line == FAREWELL).expect("Kumi said goodbye");
    let status: Vec<&&str> = lines[..goodbye].iter().filter(|line| line.starts_with("[status] ")).collect();
    assert_eq!(status.len(), 2, "{said}");
    assert!(status[0].starts_with("[status] idle;") && status[1].starts_with("[status] Conversation: Bass ideas"), "{said}");
    assert!(lines[goodbye + 1..].iter().all(|line| !line.starts_with('[')), "{said}");
}

#[tokio::test]
async fn a_status_still_working_when_quit_comes_finishes_before_the_goodbye() {
    tokio::task::LocalSet::new().run_until(async { ends_with_the_goodbye(&status_then_quit(false).await) }).await;
}

#[tokio::test]
async fn a_status_done_before_quit_says_both_its_lines_before_the_goodbye() {
    tokio::task::LocalSet::new().run_until(async { ends_with_the_goodbye(&status_then_quit(true).await) }).await;
}
