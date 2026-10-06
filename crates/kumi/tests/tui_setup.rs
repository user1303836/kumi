//! First-run setup in the app: only the missing steps, each done in place, then the session.
#[path = "support/models.rs"]
mod models;
#[path = "support/tui_app.rs"]
mod support;
use async_trait::async_trait;
use kumi::terminal::Terminal;
use kumi::tui::{
    app::{ConnectLive, LiveSetup, TuiOptions},
    icons::IconStyle,
    style::ColorDepth,
};
use kumi_common::abort::Signal;
use models::FakeModels;
use serde_json::json;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
use support::*;
macro_rules! case {
    ($name:ident,$body:expr) => {
        #[tokio::test(flavor = "current_thread")]
        async fn $name() {
            tokio::task::LocalSet::new().run_until($body).await;
        }
    };
}

const LIVE: &str = "/Applications/Ableton Live 12 Suite.app";
const MISSING: &str = "The Ableton bridge isn't in Live yet, so Kumi can't see your Set.";

/// Live, as far as setup sees it: running or not, quitting when asked or not (its save dialog
/// cancelled), and an install that can be held until the test lets it end.
struct FakeLive {
    calls: RefCell<Vec<String>>,
    /// Live's app while it runs.
    open: RefCell<Option<String>>,
    /// Live quits when Kumi asks it to; false is a save dialog cancelled (or still up).
    quits: Cell<bool>,
    installs: RefCell<Vec<Result<String, String>>>,
    /// An install ends only once this fires.
    gate: RefCell<Option<Signal>>,
}
impl FakeLive {
    fn new(open: bool) -> Rc<Self> {
        Rc::new(Self {
            calls: RefCell::default(),
            open: RefCell::new(open.then(|| LIVE.to_string())),
            quits: Cell::new(true),
            installs: RefCell::default(),
            gate: RefCell::default(),
        })
    }
    fn calls(&self) -> Vec<String> {
        self.calls.borrow().clone()
    }
    fn count(&self, call: &str) -> usize {
        self.calls.borrow().iter().filter(|c| *c == call).count()
    }
    /// The producer quits Live (or answers its save dialog).
    fn quit_now(&self) {
        self.open.borrow_mut().take();
    }
    async fn wait_for_call(&self, call: &str) {
        for _ in 0..500 {
            if self.calls.borrow().iter().any(|c| c == call) {
                return;
            }
            delay(10).await;
        }
        panic!("Missing {call:?}: {:?}", self.calls());
    }
}
#[async_trait(?Send)]
impl LiveSetup for FakeLive {
    async fn open_live(&self) -> Option<String> {
        self.calls.borrow_mut().push("open?".into());
        self.open.borrow().clone()
    }
    async fn ask_to_quit(&self) {
        self.calls.borrow_mut().push("quit".into());
        if self.quits.get() {
            self.quit_now();
        }
    }
    async fn closed(&self, stop: &Signal) -> bool {
        self.calls.borrow_mut().push("closed?".into());
        loop {
            if self.open.borrow().is_none() {
                return true;
            }
            if stop.is_cancelled() {
                return false;
            }
            tokio::select! {
                _ = delay(10) => {}
                _ = stop.cancelled() => {}
            }
        }
    }
    async fn install(&self) -> Result<String, String> {
        self.calls.borrow_mut().push("install".into());
        let gate = self.gate.borrow().clone();
        if let Some(gate) = gate {
            gate.cancelled().await;
        }
        let next = self.installs.borrow_mut().pop();
        next.unwrap_or_else(|| Ok("1.0.74".into()))
    }
    async fn start(&self, app: Option<String>) -> bool {
        self.calls.borrow_mut().push(format!("start {}", app.clone().unwrap_or_default()).trim().into());
        *self.open.borrow_mut() = Some(app.unwrap_or_else(|| LIVE.into()));
        true
    }
    async fn connect(&self) {
        self.calls.borrow_mut().push("connect".into());
    }
    async fn version(&self, _: Option<String>) -> Option<String> {
        Some("12.4".into())
    }
    fn quit_wait(&self) -> std::time::Duration {
        std::time::Duration::from_millis(300)
    }
}

fn unsigned() -> Rc<FakeModels> {
    let models = FakeModels::catalog();
    *models.model.borrow_mut() = None;
    models.signed_in.borrow_mut().clear();
    models
}
fn setup(
    width: i32,
    models: Rc<FakeModels>,
    live: Option<(Option<&str>, Rc<FakeLive>)>,
    configure: impl FnOnce(&mut TuiOptions),
) -> Harness {
    Harness::with(width, 30, Rc::new(Control::default()), |o| {
        o.models = Some(models);
        o.connect_live = live.map(|(why, live)| ConnectLive { why: why.map(str::to_string), bridge: "1.0.74".into(), live });
        configure(o)
    })
}
/// The screen's words, wrapped lines run together: for notices longer than a line.
fn said(h: &Harness) -> String {
    h.screen().join(" ").split_whitespace().collect::<Vec<_>>().join(" ")
}
async fn wait_for_said(h: &Harness, words: &str) {
    for _ in 0..300 {
        if said(h).contains(words) {
            return;
        }
        delay(20).await;
    }
    panic!("Missing {words:?} in:\n{}", h.screen().join("\n"));
}
fn rows(h: &Harness) -> String {
    let lines = h.screen();
    let last = lines.iter().rposition(|l| !l.trim().is_empty()).unwrap_or(0);
    lines[..=last].iter().map(|l| l.trim_end()).collect::<Vec<_>>().join("\n")
}

case!(a_first_run_signs_in_puts_the_bridge_in_and_waits_for_the_control_surface, async {
    let live = FakeLive::new(false);
    let models = unsigned();
    let h = setup(80, models.clone(), Some((Some(MISSING), live.clone())), |_| {});
    h.start().await;
    h.wait_for("How do you want to sign in?").await;
    let first = rows(&h);
    println!("--- sign in, 80 columns ---\n{first}");
    let kumi = first.lines().nth(1).unwrap();
    assert!(kumi.starts_with("  kumi") && kumi.ends_with(kumi_runtime::KUMI_VERSION), "{kumi}");
    assert_eq!(first.lines().nth(2).unwrap().trim(), kumi::tui::app::waveform(kumi::tui::app::WAVE_WIDTH, None), "still");
    for line in [
        "  Sign in             now",
        "  Connect to Live",
        "  Control Surface",
        "  How do you want to sign in?",
        "  › ChatGPT           your ChatGPT plan · opens your browser",
        "    Anthropic         paste an API key",
        "    OpenCode          paste an API key",
        "  ↑↓ choose · enter select · esc later",
    ] {
        assert!(first.lines().any(|l| l == line), "{line:?} in\n{first}");
    }
    // Anthropic: a pasted key, checked, then the provider's own first model.
    h.type_text("\x1b[B\r").await;
    h.wait_for("Paste your Anthropic API key").await;
    h.has("Sign in             paste a key");
    h.input.write("sk-ant-fixture");
    h.type_text("\r").await;
    h.wait_for("Anthropic · done").await;
    assert_eq!(models.model.borrow().as_deref(), Some("anthropic/claude-sonnet-5-5"), "the first in Anthropic's list");
    // Live was closed: the bridge goes in, Live opens, and the session connects through it.
    h.wait_for("waiting for Live").await;
    let surface = rows(&h);
    println!("--- control surface ---\n{surface}");
    for line in [
        "  Sign in             Anthropic · done",
        "  Connect to Live     bridge 1.0.74 · done",
        "  Control Surface     waiting for Live",
        "  In Live, open Settings › Link, Tempo & MIDI and set a",
        "  Control Surface to AbletonMcpBridge. Kumi notices by itself.",
        "  esc later",
    ] {
        assert!(surface.lines().any(|l| l == line), "{line:?} in\n{surface}");
    }
    assert_eq!(live.calls(), ["open?", "install", "start", "connect"]);
    h.emit(json!({"type":"connection","state":"connected"}));
    h.wait_for("You're set.").await;
    let set = rows(&h);
    println!("--- set ---\n{set}");
    h.has("Control Surface     Live 12.4 · connected");
    h.type_text("\r").await;
    h.wait_until_hidden("You're set.").await;
    h.has("Kumi talks to Claude Sonnet 5.5, Anthropic's first choice.");
    h.close().await;
});

case!(an_open_live_is_restarted_only_when_asked_and_esc_puts_setup_off, async {
    // "I'll quit it": Kumi waits for the producer to quit Live, and esc leaves setup for later.
    let live = FakeLive::new(true);
    let h = setup(80, FakeModels::catalog(), Some((Some(MISSING), live.clone())), |_| {});
    h.start().await;
    h.wait_for("Restart Live now").await;
    let restart = rows(&h);
    println!("--- live open ---\n{restart}");
    for line in [
        "  Sign in             ChatGPT · done",
        "  Connect to Live     Live is open · restart needed",
        "  › Restart Live now",
        "    I'll quit it      Kumi waits, then opens Live again",
    ] {
        assert!(restart.lines().any(|l| l == line), "{line:?} in\n{restart}");
    }
    for line in ["  Live loads the bridge when it starts.", "  Live will ask to save first."] {
        assert!(restart.lines().any(|l| l == line), "{line:?} in\n{restart}");
    }
    h.type_text("\x1b[B\r").await;
    h.wait_for("waiting for you to quit Live").await;
    h.type_text("\x1b").await;
    h.wait_for("Kumi chats without Live for now, and offers to connect it next time.").await;
    assert_eq!(live.calls(), ["open?", "closed?", "open?"], "nothing quit, nothing installed, Live left open");
    h.close().await;
    // "I'll quit it", and the producer does: the bridge goes in, and the same Live opens again.
    let live = FakeLive::new(true);
    let h = setup(80, FakeModels::catalog(), Some((Some(MISSING), live.clone())), |_| {});
    h.start().await;
    h.wait_for("Restart Live now").await;
    h.type_text("\x1b[B\r").await;
    h.wait_for("waiting for you to quit Live").await;
    live.quit_now();
    h.wait_for("waiting for Live").await;
    assert_eq!(live.calls(), ["open?", "closed?", "install", format!("start {LIVE}").as_str(), "connect"]);
    h.close().await;
    // "Restart Live now": Live is asked to quit (it asks to save), then the bridge goes in and the same Live opens.
    let live = FakeLive::new(true);
    let h = setup(80, FakeModels::catalog(), Some((Some(MISSING), live.clone())), |_| {});
    h.start().await;
    h.wait_for("Restart Live now").await;
    h.type_text("\r").await;
    h.wait_for("waiting for Live").await;
    assert_eq!(live.calls(), ["open?", "quit", "closed?", "install", format!("start {LIVE}").as_str(), "connect"]);
    h.close().await;
});

case!(after_kumi_restarts_live_the_control_surface_waits_for_the_live_that_opens, async {
    // #167: the session was connected to the Live Kumi restarts. That connection went with it, so the
    // Control Surface step waits for the Live that opens next rather than calling it connected at once.
    let live = FakeLive::new(true);
    let h = setup(80, FakeModels::catalog(), Some((Some(MISSING), live.clone())), |_| {});
    h.start().await;
    h.connect();
    h.wait_for("Restart Live now").await;
    h.type_text("\r").await;
    h.wait_for("waiting for Live").await;
    support::delay(400).await;
    h.has("Control Surface     waiting for Live");
    assert!(!support::has(&h.screen(), "Live 12.4 · connected"), "{}", h.screen().join("\n"));
    h.connect();
    h.wait_for("Control Surface     Live 12.4 · connected").await;
    h.close().await;
});

case!(an_install_that_fails_says_why_and_can_be_tried_again, async {
    let live = FakeLive::new(false);
    live.installs.borrow_mut().push(Err("Kumi couldn't find Live's User Library. Open Live once so it makes one.".into()));
    let h = setup(80, FakeModels::catalog(), Some((Some(MISSING), live.clone())), |_| {});
    h.start().await;
    h.wait_for("Try again").await;
    let failed = rows(&h);
    println!("--- install failed ---\n{failed}");
    h.has("Connect to Live     didn't finish");
    h.has("Kumi couldn't find Live's User Library.");
    h.type_text("\r").await;
    h.wait_for("bridge 1.0.74 · done").await;
    assert_eq!(live.calls().iter().filter(|c| *c == "install").count(), 2);
    h.close().await;
});

case!(only_the_missing_steps_run_and_a_set_up_kumi_starts_straight_away, async {
    // Set up: no setup at all.
    let live = FakeLive::new(false);
    let h = setup(80, FakeModels::catalog(), Some((None, live.clone())), |_| {});
    h.start().await;
    h.connect();
    h.wait_for("Night Drive").await;
    assert!(!h.screen().iter().any(|l| l.contains("Sign in") || l.contains("Connect to Live")));
    assert!(live.calls().is_empty(), "no process looked at, nothing installed");
    h.close().await;
    // Only signing in is missing: the bridge shows as done, and Live connects as it stands.
    let live = FakeLive::new(false);
    let h = setup(80, unsigned(), Some((None, live.clone())), |_| {});
    h.start().await;
    h.wait_for("How do you want to sign in?").await;
    h.has("Connect to Live     bridge 1.0.74 · done");
    // Esc: later. No model picker opens; /login says how.
    h.type_text("\x1b").await;
    h.wait_for("Sign in when you're ready with /login").await;
    assert!(!h.screen().iter().any(|l| l.contains("Choose a model")));
    assert!(live.calls().is_empty());
    h.close().await;
    // Signed in, with no model chosen yet: the provider's own first choice, and no setup.
    let models = FakeModels::catalog();
    *models.model.borrow_mut() = None;
    let h = setup(80, models, Some((None, FakeLive::new(false))), |_| {});
    h.start().await;
    h.wait_for("Kumi talks to GPT-6 Astra, ChatGPT's first choice.").await;
    assert!(!h.screen().iter().any(|l| l.contains("How do you want to sign in?")));
    h.close().await;
});

case!(the_browser_sign_in_moves_the_waveform_and_narrow_or_plain_terminals_drop_it, async {
    let models = unsigned();
    let h = setup(80, models.clone(), Some((Some(MISSING), FakeLive::new(false))), |_| {});
    h.start().await;
    h.wait_for("How do you want to sign in?").await;
    h.type_text("\r").await;
    h.wait_for("Finish signing in in your browser.").await;
    h.has("Sign in             waiting for the browser");
    h.has("If it didn't open, open this link (c copies it):");
    h.has("https://auth.example.test/oauth/authorize");
    h.has("c copies the link · esc back");
    let wave = |h: &Harness| h.screen()[2].trim().to_string();
    let before = wave(&h);
    delay(250).await;
    assert_ne!(wave(&h), before, "it moves while Kumi waits for the browser");
    // Esc goes back to the choices; the waveform rests again.
    h.type_text("\x1b").await;
    h.wait_for("How do you want to sign in?").await;
    assert_eq!(wave(&h), kumi::tui::app::waveform(kumi::tui::app::WAVE_WIDTH, None));
    h.close().await;
    // Narrow: everything wraps within the window.
    let h = setup(44, unsigned(), Some((Some(MISSING), FakeLive::new(false))), |_| {});
    h.start().await;
    h.wait_for("How do you want to sign in?").await;
    let narrow = rows(&h);
    println!("--- 44 columns ---\n{narrow}");
    assert!(narrow.lines().all(|l| l.chars().count() <= 44), "{narrow}");
    h.has("Connect to Live");
    h.close().await;
    // NO_COLOR, and badges instead of glyphs: no waveform.
    for configure in [
        Box::new(|o: &mut TuiOptions| o.color_depth = Some(ColorDepth::None)) as Box<dyn FnOnce(&mut TuiOptions)>,
        Box::new(|o: &mut TuiOptions| o.icons = Some(IconStyle::Badges)),
    ] {
        let h = setup(80, unsigned(), Some((Some(MISSING), FakeLive::new(false))), configure);
        h.start().await;
        h.wait_for("How do you want to sign in?").await;
        let plain = rows(&h);
        assert!(!plain.contains('⣀'), "{plain}");
        assert_eq!(plain.lines().nth(3).unwrap().trim(), "Sign in             now", "{plain}");
        h.close().await;
    }
});

case!(a_quit_while_the_bridge_goes_in_waits_for_the_install_and_opens_live_again, async {
    let live = FakeLive::new(true);
    let gate = Signal::new();
    *live.gate.borrow_mut() = Some(gate.clone());
    let h = setup(80, FakeModels::catalog(), Some((Some(MISSING), live.clone())), |_| {});
    h.start().await;
    let mut done = h.app.run();
    h.wait_for("Restart Live now").await;
    h.type_text("\r").await;
    h.wait_for("installing…").await;
    // Ctrl-C, then Ctrl-D: the installer isn't stopped halfway.
    h.type_text("\x03").await;
    h.wait_for("Kumi quits once the bridge is in place.").await;
    h.type_text("\x04").await;
    assert!(tokio::time::timeout(std::time::Duration::from_millis(300), &mut done).await.is_err(), "Kumi waits for the install");
    assert_eq!(live.count(&format!("start {LIVE}")), 0, "Live stays closed while the bridge goes in");
    gate.cancel();
    assert_eq!(tokio::time::timeout(std::time::Duration::from_secs(5), done).await.expect("Kumi quits once it's done"), 0);
    assert_eq!(
        live.calls(),
        ["open?", "quit", "closed?", "install", "open?", format!("start {LIVE}").as_str()],
        "the Live Kumi closed opens again"
    );
});

case!(a_live_that_doesnt_quit_is_noticed_and_asked_again_or_left_for_later, async {
    // Its save dialog was cancelled: after a while Kumi says so, and asks again when told to.
    let live = FakeLive::new(true);
    live.quits.set(false);
    let h = setup(80, FakeModels::catalog(), Some((Some(MISSING), live.clone())), |_| {});
    h.start().await;
    h.wait_for("Restart Live now").await;
    h.type_text("\r").await;
    h.wait_for("waiting for Live to close").await;
    h.wait_for("Live is still open").await;
    let still = rows(&h);
    println!("--- live still open ---\n{still}");
    assert!(said(&h).contains("Live hasn't quit. If it asked about saving and you chose Cancel, it stays open."), "{still}");
    for line in ["  › Ask again         Live asks to save first", "    Later             chat without Live for now"] {
        assert!(still.lines().any(|l| l == line), "{line:?} in\n{still}");
    }
    live.quits.set(true);
    h.type_text("\r").await;
    h.wait_for("waiting for Live").await;
    assert_eq!(live.count("quit"), 2);
    assert_eq!(live.count(&format!("start {LIVE}")), 1);
    h.close().await;
    // It quits while Kumi asks: Kumi goes on by itself.
    let live = FakeLive::new(true);
    live.quits.set(false);
    let h = setup(80, FakeModels::catalog(), Some((Some(MISSING), live.clone())), |_| {});
    h.start().await;
    h.wait_for("Restart Live now").await;
    h.type_text("\r").await;
    h.wait_for("Ask again").await;
    live.quit_now();
    h.wait_for("waiting for Live").await;
    assert!(!h.screen().iter().any(|l| l.contains("Ask again")));
    assert_eq!((live.count("quit"), live.count("install")), (1, 1));
    h.close().await;
    // Later, with the request still open in Live: if Live quits on it after all, Kumi opens it again.
    let live = FakeLive::new(true);
    live.quits.set(false);
    let h = setup(80, FakeModels::catalog(), Some((Some(MISSING), live.clone())), |_| {});
    h.start().await;
    h.wait_for("Restart Live now").await;
    h.type_text("\r").await;
    h.wait_for("Ask again").await;
    h.type_text("\x1b[B\r").await;
    wait_for_said(&h, "If Live still asks about saving, Cancel keeps it open; if it quits, Kumi opens it again.").await;
    assert_eq!(live.count("install"), 0);
    live.quit_now();
    live.wait_for_call(&format!("start {LIVE}")).await;
    h.close().await;
});

case!(esc_after_kumi_asked_live_to_quit_opens_live_again_once_it_closes, async {
    let live = FakeLive::new(true);
    live.quits.set(false);
    let h = setup(80, FakeModels::catalog(), Some((Some(MISSING), live.clone())), |_| {});
    h.start().await;
    h.wait_for("Restart Live now").await;
    h.type_text("\r").await;
    h.wait_for("waiting for Live to close").await;
    h.type_text("\x1b").await;
    wait_for_said(&h, "Kumi chats without Live for now, and offers to connect it next time. If Live still asks about saving, Cancel keeps it open; if it quits, Kumi opens it again.").await;
    // The producer answers Live's save dialog after all: Live quits, and opens again.
    live.quit_now();
    live.wait_for_call(&format!("start {LIVE}")).await;
    assert_eq!(live.count("install"), 0, "nothing installed after esc");
    h.close().await;
});

case!(a_failed_install_then_later_opens_the_live_kumi_closed, async {
    let live = FakeLive::new(true);
    live.installs.borrow_mut().push(Err("The bridge's installer stopped, and put back what was there: disk full".into()));
    let h = setup(80, FakeModels::catalog(), Some((Some(MISSING), live.clone())), |_| {});
    h.start().await;
    h.wait_for("Restart Live now").await;
    h.type_text("\r").await;
    h.wait_for("Try again").await;
    h.type_text("\x1b[B\r").await;
    wait_for_said(&h, "Kumi chats without Live for now, and offers to connect it next time. Live is opening again.").await;
    assert_eq!(live.calls(), ["open?", "quit", "closed?", "install", "open?", format!("start {LIVE}").as_str()]);
    h.close().await;
    // Live wasn't open: it isn't opened for nothing.
    let live = FakeLive::new(false);
    live.installs.borrow_mut().push(Err("disk full".into()));
    let h = setup(80, FakeModels::catalog(), Some((Some(MISSING), live.clone())), |_| {});
    h.start().await;
    h.wait_for("Try again").await;
    h.type_text("\x1b").await;
    h.wait_for("Kumi chats without Live for now, and offers to connect it next time.").await;
    assert_eq!(live.calls(), ["open?", "install"]);
    h.close().await;
});

case!(esc_at_the_control_surface_says_how_kumi_connects, async {
    let live = FakeLive::new(false);
    let h = setup(80, FakeModels::catalog(), Some((Some(MISSING), live.clone())), |_| {});
    h.start().await;
    h.wait_for("waiting for Live").await;
    h.type_text("\x1b").await;
    wait_for_said(&h, "Kumi connects to Live by itself once Live answers: in Live, open Settings › Link, Tempo & MIDI and set a Control Surface to AbletonMcpBridge.").await;
    h.close().await;
});

case!(a_default_model_is_looked_for_once, async {
    // Nothing to choose from: setup asks to sign in, having looked once.
    let models = unsigned();
    let h = setup(80, models.clone(), Some((None, FakeLive::new(false))), |_| {});
    h.start().await;
    h.wait_for("How do you want to sign in?").await;
    assert_eq!(models.looked.get(), 1);
    h.close().await;
    // The bridge missing too: looked for once all the same.
    let models = unsigned();
    let h = setup(80, models.clone(), Some((Some(MISSING), FakeLive::new(false))), |_| {});
    h.start().await;
    h.wait_for("How do you want to sign in?").await;
    assert_eq!(models.looked.get(), 1);
    h.close().await;
});
