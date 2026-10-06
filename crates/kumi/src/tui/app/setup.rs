//! First-run setup: what Kumi still needs before a session can see Live — signing in, its bridge in
//! Live, and the Control Surface Live connects through — run in the app, in place of the session,
//! until each is done or put off (esc). Already set up, Kumi starts straight into the session.

use super::super::{
    icons::IconStyle,
    render::Cursor,
    screen::Screen,
    style::Style,
    width::{text_width, truncate},
    wrap::wrap,
};
use super::drawing::{sp, st};
use super::*;
use async_trait::async_trait;
use kumi_common::abort;
use kumi_runtime::{
    providers::{provider_info, SignIn},
    KUMI_VERSION,
};
use tokio::sync::oneshot;

/// Live's side of setup, which the app can't do on its own: Live the app, and the bridge in it.
#[async_trait(?Send)]
pub trait LiveSetup {
    /// The open Live's app (a `.app` on macOS, an `.exe` on Windows), if Live is open.
    async fn open_live(&self) -> Option<String>;
    /// Ask Live to quit as its own menu does, so unsaved work makes it ask to save first. Returns at once.
    async fn ask_to_quit(&self);
    /// Wait until Live has closed: true once it has, false when `stop` comes first.
    async fn closed(&self, stop: &Signal) -> bool;
    /// Put Kumi's bridge in place, with Live closed: the bridge's version, or what went wrong, in words.
    async fn install(&self) -> Result<String, String>;
    /// Open Live: `app`, the one that was open, or else the newest one installed.
    async fn start(&self, app: Option<String>) -> bool;
    /// Connect the session to Live through the bridge just put in place.
    async fn connect(&self);
    /// Live's version ("12.4"), when it can be read from its app.
    async fn version(&self, app: Option<String>) -> Option<String>;
    /// How long Live gets to quit once asked, before Kumi says it's still open (its save dialog
    /// cancelled, say) and offers to ask again.
    fn quit_wait(&self) -> Duration {
        Duration::from_secs(15)
    }
}

/// How long Kumi keeps watching, after the producer put the Live step off, for Live to quit on the
/// request Kumi already sent (its save dialog still up), so it can open Live again.
const STILL_ASKED_MS: u64 = 120_000;

impl Setup {
    /// The app is closing: nothing more is waited for.
    pub(super) fn stop(&mut self) {
        self.answer.take();
        self.esc.cancel();
        if let Some(ticker) = self.ticker.take() {
            ticker.abort();
        }
    }

    /// Quitting while Kumi's bridge goes into Live, or while Kumi has Live closed (or closing): put off
    /// until the installer is done and Live is open again, since stopping the installer partway can
    /// leave Live's Remote Script half switched. True when the quit was put off.
    pub(super) fn defer_quit(&mut self, code: i32, message: Option<String>) -> bool {
        if !(self.installing || self.holding) {
            return false;
        }
        if self.quit.is_none() {
            self.quit = Some((code, message));
            if self.installing {
                self.problem = Some("Kumi quits once the bridge is in place.".into());
            }
        }
        // Waits and questions end at once; the installer runs to its end.
        self.esc.cancel();
        if let Some(answer) = self.answer.take() {
            let _ = answer.send(None);
        }
        true
    }
}

/// What became of the Live step.
enum LiveStep {
    /// The bridge is in place and Live is opening: the app it opens from, when known.
    Done(Option<String>),
    /// Put off, with a sentence for the farewell when Live is involved.
    Later(Option<String>),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Step {
    Sign,
    Live,
    Surface,
    Set,
}

pub(super) struct Choice {
    label: String,
    detail: String,
    value: String,
}
fn choice(label: &str, detail: &str, value: &str) -> Choice {
    Choice { label: label.into(), detail: detail.into(), value: value.into() }
}

pub(super) struct Setup {
    step: Step,
    /// Shown beside each step once it's done: who Kumi talks to, the bridge's version, Live's.
    signed: Option<String>,
    bridge: Option<String>,
    live: Option<String>,
    status: String,
    warn: bool,
    say: Vec<String>,
    problem: Option<String>,
    choices: Vec<Choice>,
    chosen: usize,
    hint: String,
    /// The waveform moves while Kumi waits for something outside it: the browser, the install, Live.
    waiting: bool,
    since: f64,
    ticker: Option<JoinHandle<()>>,
    answer: Option<oneshot::Sender<Option<String>>>,
    /// Fired by esc while Kumi waits.
    esc: Signal,
    /// The bridge's installer is running, and Kumi has Live closed (or is closing it): a quit then waits.
    installing: bool,
    holding: bool,
    /// A quit put off until then: its code and farewell.
    quit: Option<(i32, Option<String>)>,
}

/// The waveform's levels, low to high.
const LEVELS: [char; 4] = ['⣀', '⣤', '⣶', '⣿'];
/// How wide the waveform is, and its shape when nothing is waited for.
pub const WAVE_WIDTH: usize = 26;
const RESTING: &str = "⣀⣀⣤⣶⣿⣶⣤⣀⣀⣀⣤⣶⣿⣿⣶⣤⣀⣀⣀⣀⣤⣤⣶⣤⣀⣀";

/// One line of braille bars, `width` cells: the resting shape when `phase` is `None`, else a wave
/// moving along it (`phase` in seconds).
pub fn waveform(width: usize, phase: Option<f64>) -> String {
    let Some(t) = phase else {
        return RESTING.chars().cycle().take(width).collect();
    };
    (0..width)
        .map(|i| {
            let x = i as f64;
            let v = ((x * 0.55 - t * 5.0).sin() + 0.6 * (x * 0.23 + t * 3.1).sin() + 1.6) / 3.2;
            LEVELS[((v * 4.0).floor() as i32).clamp(0, 3) as usize]
        })
        .collect()
}

/// How the producer can sign in, as the setup offers it.
fn sign_in_choices() -> Vec<Choice> {
    vec![
        choice("ChatGPT", "your ChatGPT plan · opens your browser", "openai-codex"),
        choice("Anthropic", "paste an API key", "anthropic"),
        choice("OpenAI", "paste an API key", "openai"),
        choice("OpenCode", "paste an API key", "opencode"),
    ]
}

const CHOOSE_HINT: &str = "↑↓ choose · enter select · esc later";
const SURFACE: &str = "In Live, open Settings › Link, Tempo & MIDI and set a Control Surface to AbletonMcpBridge. Kumi notices by itself.";

impl TuiApp {
    pub(super) fn setup_active(&self) -> bool {
        self.0.state.borrow().setup.is_some()
    }

    /// At start, when Kumi can reach Live: the setup shows at once when Live's bridge is missing or older
    /// than Kumi's (local reads only). Chatting without Live by choice, Kumi signs in as it always has.
    pub(super) fn begin_setup(&self) -> bool {
        if self.0.options.connect_live.as_ref().is_none_or(|l| l.why.is_none()) {
            return false;
        }
        let unsigned = self.0.options.models.as_ref().is_some_and(|m| m.current().model.is_none());
        self.open_setup(if unsigned { Step::Sign } else { Step::Live }, None);
        true
    }

    /// Without a model: the provider's own first choice when Kumi is signed in somewhere (or a model on
    /// this computer is running), as always; otherwise signing in comes first, in the setup.
    pub(super) async fn needs_sign_in(&self) -> bool {
        let (Some(models), Some(live)) = (self.0.options.models.clone(), self.0.options.connect_live.as_ref()) else { return false };
        if models.current().model.is_some() {
            return false;
        }
        let bridge = format!("bridge {}", live.bridge);
        match models.choose_default().await {
            Ok(Some(chosen)) => {
                self.told_default(chosen);
                false
            }
            // A failure to look is the session's to say; the setup offers to sign in either way.
            _ if self.closing() => false,
            _ => {
                self.open_setup(Step::Sign, Some(bridge));
                true
            }
        }
    }

    fn open_setup(&self, step: Step, bridge: Option<String>) {
        let setup = Setup {
            step,
            signed: None,
            bridge,
            live: None,
            status: "now".into(),
            warn: false,
            say: vec![],
            problem: None,
            choices: vec![],
            chosen: 0,
            hint: String::new(),
            waiting: false,
            since: perf_now(),
            ticker: None,
            answer: None,
            esc: Signal::new(),
            installing: false,
            holding: false,
            quit: None,
        };
        self.0.state.borrow_mut().setup = Some(setup);
        self.0.scheduler.request();
    }

    /// The missing steps, in order, then the session. `looked`: Kumi already looked for a default model.
    pub(super) async fn run_setup(&self, looked: bool) {
        let unsigned = self.0.state.borrow().setup.as_ref().is_some_and(|s| s.step == Step::Sign);
        if !unsigned || (!looked && self.default_found().await) {
            self.note_signed();
        } else if !self.setup_sign_in().await {
            return self.end_setup(Some(Step::Sign), None).await;
        }
        let live = self.0.options.connect_live.as_ref().map(|l| (l.live.clone(), l.why.clone()));
        if let Some((live, Some(why))) = live {
            let app = match self.setup_live(live.clone(), why).await {
                LiveStep::Done(app) => app,
                LiveStep::Later(also) => return self.end_setup(Some(Step::Live), also).await,
            };
            if !self.setup_surface(live, app).await {
                return self.end_setup(Some(Step::Surface), None).await;
            }
        } else if let Some((live, None)) = live {
            // The bridge was already in place: Live connects as it stands, and later runs need nothing more.
            if self.connected() {
                let version = live.version(None).await;
                self.setup_show(|s| s.live = Some(version.map(|v| format!("Live {v}")).unwrap_or_else(|| "Live".into())));
            }
        }
        self.end_setup(None, None).await;
    }

    /// No model yet, but signed in somewhere: the provider's own first choice, as Kumi always chose.
    async fn default_found(&self) -> bool {
        let Some(models) = self.0.options.models.clone() else { return false };
        if models.current().model.is_some() {
            return true;
        }
        match models.choose_default().await {
            Ok(Some(chosen)) => {
                self.told_default(chosen);
                true
            }
            _ => false,
        }
    }

    fn connected(&self) -> bool {
        self.0.state.borrow().connection == ConnectionState::Connected
    }

    fn closing(&self) -> bool {
        self.0.state.borrow().closing
    }

    /// Who Kumi talks to, for the Sign in step: the chosen model's provider.
    fn note_signed(&self) {
        let Some(models) = &self.0.options.models else { return };
        let current = models.current();
        let name = current.provider.as_deref().map(|p| models.provider_name(p));
        self.setup_show(|s| s.signed = name);
    }

    /// Change the setup and draw it; the waveform moves while it waits.
    fn setup_show(&self, change: impl FnOnce(&mut Setup)) {
        let start = {
            let mut state = self.0.state.borrow_mut();
            let Some(setup) = state.setup.as_mut() else { return };
            let was = setup.waiting;
            change(setup);
            if setup.waiting && !was {
                setup.since = perf_now();
            }
            if !setup.waiting {
                if let Some(ticker) = setup.ticker.take() {
                    ticker.abort();
                }
            }
            setup.waiting && setup.ticker.is_none()
        };
        if start {
            let weak = Rc::downgrade(&self.0);
            let ticker = tokio::task::spawn_local(async move {
                loop {
                    tokio::time::sleep(Duration::from_millis(80)).await;
                    let Some(a) = weak.upgrade() else { break };
                    a.scheduler.request();
                }
            });
            if let Some(setup) = self.0.state.borrow_mut().setup.as_mut() {
                setup.ticker = Some(ticker);
            }
        }
        self.0.scheduler.request();
    }

    /// A quit was put off until Live and its bridge were done with.
    fn quitting(&self) -> bool {
        self.0.state.borrow().setup.as_ref().is_some_and(|s| s.quit.is_some())
    }

    /// Show `choices` for the current step and wait for one: its value, or `None` for esc (later).
    async fn setup_choose(&self, status: &str, say: Vec<String>, choices: Vec<Choice>) -> Option<String> {
        if self.quitting() {
            return None;
        }
        let (tx, rx) = oneshot::channel();
        let status = status.to_string();
        self.setup_show(move |s| {
            s.status = status;
            s.say = say;
            s.choices = choices;
            s.chosen = 0;
            s.hint = CHOOSE_HINT.into();
            s.waiting = false;
            s.answer = Some(tx);
        });
        rx.await.ok().flatten()
    }

    /// A signal esc fires while Kumi waits (already fired when a quit is waiting).
    fn setup_wait(&self) -> Signal {
        let signal = Signal::new();
        if self.quitting() {
            signal.cancel();
        }
        let copy = signal.clone();
        self.setup_show(move |s| s.esc = copy);
        signal
    }

    /// A sign-in that didn't finish, said on the setup.
    pub(super) fn setup_failed(&self, message: &str) {
        let message = message.to_string();
        self.setup_show(move |s| s.problem = Some(message));
    }

    async fn setup_sign_in(&self) -> bool {
        let Some(models) = self.0.options.models.clone() else { return true };
        loop {
            self.setup_show(|s| s.step = Step::Sign);
            let Some(value) = self.setup_choose("now", vec!["How do you want to sign in?".into()], sign_in_choices()).await else {
                return false;
            };
            let Some(provider) = ProviderId::parse(&value) else { continue };
            self.setup_show(|s| s.problem = None);
            if self.setup_signing(provider).await {
                break;
            }
            if self.closing() {
                return false;
            }
        }
        if models.current().model.is_none() {
            if let Ok(Some(chosen)) = models.choose_default().await {
                self.told_default(chosen);
            }
        }
        self.note_signed();
        true
    }

    /// One sign-in, through the session's own panels: true once signed in, false when it went back
    /// (esc, or it didn't finish).
    async fn setup_signing(&self, provider: ProviderId) -> bool {
        let (tx, mut rx) = oneshot::channel::<()>();
        let tx = Rc::new(RefCell::new(Some(tx)));
        let then = self.action(move |_| {
            let tx = tx.clone();
            async move {
                if let Some(tx) = tx.borrow_mut().take() {
                    let _ = tx.send(());
                }
                Ok(())
            }
        });
        let browser = provider_info(provider).sign_in != SignIn::ApiKey;
        self.setup_show(move |s| {
            s.choices.clear();
            s.say.clear();
            s.status = if browser { "waiting for the browser" } else { "paste a key" }.into();
            s.waiting = browser;
            s.hint = if browser { "c copies the link · esc back" } else { "enter check · esc back" }.into();
        });
        self.sign_in(provider, Some(then));
        let signed = loop {
            tokio::select! {
                biased;
                done = &mut rx => break done.is_ok(),
                _ = tokio::time::sleep(Duration::from_millis(50)) => {
                    // A sign-in's `then` runs before anything else once its panel closes.
                    if self.0.state.borrow().panel.is_none() || self.closing() {
                        break rx.try_recv().is_ok();
                    }
                }
            }
        };
        self.setup_show(|s| s.waiting = false);
        signed
    }

    /// The bridge into Live: Live quits first when it's open (it asks to save), then the bridge goes in
    /// place and Live opens again. Once Kumi has asked Live to quit, or waits for the producer to quit
    /// it, every way out of this step leaves Live open again.
    async fn setup_live(&self, live: Rc<dyn LiveSetup>, why: String) -> LiveStep {
        self.setup_show(|s| {
            s.step = Step::Live;
            s.problem = None;
        });
        // The Live Kumi has closed (or is closing).
        let mut held: Option<String> = None;
        loop {
            let said = why.clone();
            self.setup_show(move |s| {
                s.status = "checking Live…".into();
                s.say = vec![said];
                s.choices.clear();
                s.hint.clear();
            });
            let open = live.open_live().await;
            if self.closing() {
                return LiveStep::Later(None);
            }
            if open.is_some() {
                // Open (again): nothing is held until the producer says how it closes.
                self.setup_show(|s| s.holding = false);
                let say = vec!["Live loads the bridge when it starts.".to_string(), "Live will ask to save first.".to_string()];
                let restart =
                    vec![choice("Restart Live now", "", "restart"), choice("I'll quit it", "Kumi waits, then opens Live again", "wait")];
                let Some(answer) = self.setup_choose("Live is open · restart needed", say, restart).await else {
                    return self.leave_live(&live, None, false).await;
                };
                held = open.clone();
                let asked = answer == "restart";
                self.setup_show(|s| s.holding = true);
                if asked {
                    live.ask_to_quit().await;
                }
                if !self.wait_closed(&live, asked).await {
                    return self.leave_live(&live, held, asked).await;
                }
                // The Live Kumi was connected to has gone: Control Surface waits for the one that opens next.
                self.0.state.borrow_mut().connection = ConnectionState::Disconnected;
            }
            let said = why.clone();
            self.setup_show(move |s| {
                s.status = "installing…".into();
                s.say = vec![said];
                s.hint.clear();
                s.waiting = true;
                s.installing = true;
            });
            let installed = live.install().await;
            self.setup_show(|s| s.installing = false);
            if self.quitting() {
                return self.leave_live(&live, held, false).await;
            }
            match installed {
                Ok(version) => {
                    self.setup_show(|s| {
                        s.bridge = Some(format!("bridge {version}"));
                        s.status = "opening Live…".into();
                        s.say.clear();
                    });
                    // The Live Kumi closed opens again; one that wasn't open opens as the newest installed.
                    let app = open.or(held);
                    let opened = live.start(app.clone()).await;
                    self.setup_show(|s| s.holding = false);
                    let connecting = live.clone();
                    tokio::task::spawn_local(async move { connecting.connect().await });
                    if !opened {
                        self.setup_failed("Kumi couldn't open Live; open it yourself.");
                    }
                    self.setup_show(|s| s.waiting = false);
                    return LiveStep::Done(app);
                }
                Err(problem) => {
                    let again = vec![choice("Try again", "", "again"), choice("Later", "chat without Live for now", "later")];
                    self.setup_show(|s| s.warn = true);
                    let answer = self.setup_choose("didn't finish", vec![problem], again).await;
                    self.setup_show(|s| s.warn = false);
                    if answer.as_deref() != Some("again") {
                        return self.leave_live(&live, held, false).await;
                    }
                }
            }
        }
    }

    /// Wait for Live to close: true once it has, false for later (esc, or a quit put off). When Kumi
    /// asked it to quit and it's still open a while later (its save dialog cancelled, say), Kumi says
    /// so and offers to ask again, still watching for it to close meanwhile.
    async fn wait_closed(&self, live: &Rc<dyn LiveSetup>, asked: bool) -> bool {
        let stop = self.setup_wait();
        loop {
            self.setup_show(move |s| {
                s.status = if asked { "waiting for Live to close" } else { "waiting for you to quit Live" }.into();
                s.say = vec![if asked {
                    "Answer Live if it asks about saving.".into()
                } else {
                    "Quit Live when you're ready. Kumi then puts its bridge in place and opens Live again.".into()
                }];
                s.choices.clear();
                s.hint = "esc later".into();
                s.waiting = true;
            });
            if !asked {
                return live.closed(&stop).await;
            }
            tokio::select! {
                closed = live.closed(&stop) => return closed,
                _ = tokio::time::sleep(live.quit_wait()) => {}
            }
            let say = vec!["Live hasn't quit. If it asked about saving and you chose Cancel, it stays open.".to_string()];
            let again =
                vec![choice("Ask again", "Live asks to save first", "again"), choice("Later", "chat without Live for now", "later")];
            let answer = tokio::select! {
                closed = live.closed(&stop) => {
                    // It closed after all, or esc: the question no longer applies.
                    self.setup_show(|s| {
                        s.answer = None;
                        s.choices.clear();
                    });
                    return closed;
                }
                answer = self.setup_choose("Live is still open", say, again) => answer,
            };
            if answer.as_deref() != Some("again") {
                return false;
            }
            live.ask_to_quit().await;
        }
    }

    /// Out of the Live step without the bridge in place. When Kumi had Live closed (or closing), Live
    /// opens again: now, if it has closed, or, when a quit Kumi asked for is still being decided (Live's
    /// save dialog up), once it closes in the next couple of minutes.
    async fn leave_live(&self, live: &Rc<dyn LiveSetup>, held: Option<String>, asked: bool) -> LiveStep {
        self.setup_show(|s| {
            s.holding = false;
            s.waiting = false;
        });
        let Some(app) = held else { return LiveStep::Later(None) };
        if live.open_live().await.is_none() {
            let opened = live.start(Some(app)).await;
            return LiveStep::Later(Some(
                if opened { "Live is opening again." } else { "Kumi couldn't open Live again; open it yourself." }.into(),
            ));
        }
        if !asked {
            return LiveStep::Later(None);
        }
        let watching = live.clone();
        tokio::task::spawn_local(async move {
            if watching.closed(&abort::timeout(STILL_ASKED_MS)).await {
                watching.start(Some(app)).await;
            }
        });
        LiveStep::Later(Some("If Live still asks about saving, Cancel keeps it open; if it quits, Kumi opens it again.".into()))
    }

    /// Live connects through the bridge once it has it as a Control Surface: Kumi watches for that.
    async fn setup_surface(&self, live: Rc<dyn LiveSetup>, app: Option<String>) -> bool {
        let stop = self.setup_wait();
        self.setup_show(|s| {
            s.step = Step::Surface;
            s.status = "waiting for Live".into();
            s.say = vec![SURFACE.into()];
            s.hint = "esc later".into();
            s.waiting = true;
        });
        while !self.connected() {
            if self.closing() {
                return false;
            }
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(200)) => {}
                _ = stop.cancelled() => return false,
            }
        }
        let version = live.version(app).await;
        self.setup_show(|s| {
            s.problem = None;
            s.live = Some(version.map(|v| format!("Live {v}")).unwrap_or_else(|| "Live".into()));
        });
        true
    }

    /// Setup ends: done ("You're set.", then the session), or put off at `later` (with `also` to say), or
    /// the quit put off while Live and its bridge were busy.
    async fn end_setup(&self, later: Option<Step>, also: Option<String>) {
        if self.closing() {
            return;
        }
        let quit = self.0.state.borrow_mut().setup.as_mut().and_then(|s| s.quit.take());
        if later.is_none() && quit.is_none() {
            let stop = self.setup_wait();
            self.setup_show(|s| {
                s.step = Step::Set;
                s.say.clear();
                s.choices.clear();
                s.hint.clear();
                s.problem = None;
                s.waiting = false;
            });
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(1500)) => {}
                _ = stop.cancelled() => {}
            }
        }
        {
            let mut state = self.0.state.borrow_mut();
            if let Some(ticker) = state.setup.take().and_then(|mut s| s.ticker.take()) {
                ticker.abort();
            }
        }
        self.0.renderer.borrow_mut().invalidate();
        self.0.scheduler.request();
        if let Some((code, message)) = quit {
            drop(self.finish(code, message));
            return;
        }
        match later {
            Some(Step::Sign) => self.notice(
                "Sign in when you're ready with /login (ChatGPT with your plan, or others with an API key), or choose a model on this computer with /model.",
                NoticeTone::Info,
            ),
            Some(Step::Live) => self.notice(
                &format!(
                    "Kumi chats without Live for now, and offers to connect it next time.{}",
                    also.map(|also| format!(" {also}")).unwrap_or_default()
                ),
                NoticeTone::Info,
            ),
            // The bridge is current, so no setup shows next time: the session connects as soon as Live answers.
            Some(Step::Surface) => self.notice(
                "Kumi connects to Live by itself once Live answers: in Live, open Settings › Link, Tempo & MIDI and set a Control Surface to AbletonMcpBridge.",
                NoticeTone::Info,
            ),
            _ => {}
        }
        if later != Some(Step::Sign) {
            if let Err(error) = self.check_model().await {
                self.panel_failed(&error);
            }
        }
    }

    pub(super) fn setup_key(&self, name: &str, mods: super::super::keys::Modifiers) {
        if mods.ctrl && matches!(name, "c" | "d") {
            drop(self.finish(0, None));
            return;
        }
        let mut state = self.0.state.borrow_mut();
        let Some(setup) = state.setup.as_mut() else { return };
        if setup.answer.is_some() {
            match name {
                "up" => setup.chosen = setup.chosen.saturating_sub(1),
                "down" | "tab" => setup.chosen = (setup.chosen + 1).min(setup.choices.len().saturating_sub(1)),
                "enter" => {
                    let value = setup.choices.get(setup.chosen).map(|c| c.value.clone());
                    if let (Some(value), Some(answer)) = (value, setup.answer.take()) {
                        setup.choices.clear();
                        let _ = answer.send(Some(value));
                    }
                }
                "escape" => {
                    if let Some(answer) = setup.answer.take() {
                        let _ = answer.send(None);
                    }
                }
                _ => {}
            }
        } else if name == "escape" || (name == "enter" && setup.step == Step::Set) {
            setup.esc.cancel();
        }
        drop(state);
        self.0.scheduler.request();
    }

    /// The setup, full screen: the wordmark, the waveform, the steps, then what the current one asks.
    pub(super) fn draw_setup(&self, screen: &mut Screen, columns: i32, rows: i32) -> Option<Cursor> {
        let x = 2;
        let width = (columns - 4).min(60);
        let state = self.0.state.borrow();
        let setup = state.setup.as_ref()?;
        let panel = state.panel.clone();
        let mut y = 1;
        screen.put(x, y, "kumi", &st::TITLE);
        let version = KUMI_VERSION;
        if width > text_width(version) + 6 {
            screen.put(x + width - text_width(version), y, version, &st::FAINT);
        }
        y += 1;
        if self.0.icons != IconStyle::Badges && self.0.depth != ColorDepth::None {
            let phase = setup.waiting.then(|| (perf_now() - setup.since) / 1000.);
            screen.put(x, y, &waveform(WAVE_WIDTH.min(width.max(0) as usize), phase), &st::ACCENT);
            y += 1;
        }
        y += 1;
        let names = [(Step::Sign, "Sign in"), (Step::Live, "Connect to Live"), (Step::Surface, "Control Surface")];
        let column = if width >= 40 { 20 } else { 17 };
        let signing = panel.as_ref().and_then(|p| match &*p.borrow() {
            Panel::Key { checking, .. } => Some(if *checking { "checking the key…" } else { "paste a key" }),
            Panel::ChatGpt { .. } => Some("waiting for the browser"),
            _ => None,
        });
        for (step, name) in &names {
            let done = match step {
                Step::Sign => setup.signed.clone(),
                Step::Live => setup.bridge.clone(),
                Step::Surface => setup.live.clone().map(|l| format!("{l} · connected")),
                Step::Set => None,
            };
            let order = |s: Step| [Step::Sign, Step::Live, Step::Surface, Step::Set].iter().position(|t| *t == s).unwrap_or(0);
            let (name_style, status, status_style) = if *step == setup.step {
                let status = if *step == Step::Sign {
                    signing.map(str::to_string).unwrap_or_else(|| setup.status.clone())
                } else {
                    setup.status.clone()
                };
                (st::BRIGHT, status, if setup.warn { st::WARN } else { st::TEXT })
            } else if order(*step) < order(setup.step) || done.is_some() {
                let status = match (step, done) {
                    (Step::Surface, Some(done)) => done,
                    (_, Some(done)) => format!("{done} · done"),
                    (Step::Surface, None) => "when Live opens".into(),
                    (_, None) => "later".into(),
                };
                let style = if status == "later" || status == "when Live opens" { st::FAINT } else { st::ACCENT };
                (style, status, style)
            } else {
                (st::FAINT, String::new(), st::FAINT)
            };
            screen.put(x, y, &truncate(name, width), &name_style);
            if !status.is_empty() && width > column + 4 {
                screen.put(x + column, y, &truncate(&status, width - column), &status_style);
            }
            y += 1;
        }
        y += 1;
        let say = |screen: &mut Screen, y: &mut i32, text: &str, style: Style| {
            for row in wrap(&[sp(text, style)], width.max(1)) {
                if *y < rows - 1 {
                    let line: String = row.into_iter().map(|s| s.text).collect();
                    screen.put(x, *y, string::trim_end(&line), &style);
                }
                *y += 1;
            }
        };
        let mut cursor = None;
        if setup.step == Step::Set {
            say(screen, &mut y, "You're set.", st::BRIGHT);
            return None;
        }
        let shown = setup.step == Step::Sign && signing.is_some();
        if let Some(panel) = panel.filter(|_| shown) {
            match &*panel.borrow() {
                Panel::Key { provider, secret, checking, status, .. } => {
                    let info = provider_info(*provider);
                    say(screen, &mut y, &format!("Paste your {} API key. It stays hidden, even here.", info.name), st::TEXT);
                    y += 1;
                    let count = secret.encode_utf16().count();
                    let dots = "•".repeat(count.min((width - 16).max(8) as usize));
                    screen.put(x, y, &dots, &st::BRIGHT);
                    if count > 0 {
                        let tally = format!("{count} characters");
                        if width > text_width(&dots) + text_width(&tally) + 2 {
                            screen.put(x + width - text_width(&tally), y, &tally, &st::FAINT);
                        }
                    }
                    if !*checking {
                        cursor = Some(Cursor { x: x + text_width(&dots), y });
                    }
                    y += 2;
                    if let Some((text, tone)) = status {
                        say(screen, &mut y, text, if *tone == NoticeTone::Warn { st::WARN } else { st::DIM });
                    } else if let Some(page) = info.key_page {
                        say(screen, &mut y, &format!("Make one at {page}."), st::DIM);
                    }
                    say(
                        screen,
                        &mut y,
                        &format!("Kumi checks it with {}, then keeps it in ~/.kumi, readable only by you.", info.name),
                        st::FAINT,
                    );
                }
                Panel::ChatGpt { url, .. } => {
                    say(screen, &mut y, "Finish signing in in your browser.", st::TEXT);
                    if let Some(url) = url {
                        y += 1;
                        say(screen, &mut y, "If it didn't open, open this link (c copies it):", st::DIM);
                        for part in helpers::chunk(url, width.max(1) as usize).into_iter().take(4) {
                            if y < rows - 1 {
                                screen.put(x, y, &part, &st::ACCENT);
                            }
                            y += 1;
                        }
                    }
                }
                _ => {}
            }
        } else {
            for text in &setup.say {
                say(screen, &mut y, text, if setup.warn { st::WARN } else { st::TEXT });
            }
            if !setup.choices.is_empty() {
                if !setup.say.is_empty() {
                    y += 1;
                }
                let label_width = setup.choices.iter().map(|c| text_width(&c.label)).max().unwrap_or(0).max(16) + 2;
                for (index, item) in setup.choices.iter().enumerate() {
                    if y >= rows - 1 {
                        break;
                    }
                    let chosen = index == setup.chosen;
                    if chosen {
                        screen.put(x, y, "›", &st::ACCENT);
                    }
                    screen.put(x + 2, y, &truncate(&item.label, width - 2), &if chosen { st::ACCENT } else { st::TEXT });
                    let at = x + 2 + label_width;
                    if !item.detail.is_empty() && at + 8 < x + width {
                        screen.put(at, y, &truncate(&item.detail, x + width - at), &st::DIM);
                    }
                    y += 1;
                }
            }
        }
        if let Some(problem) = &setup.problem {
            y += 1;
            say(screen, &mut y, problem, st::WARN);
        }
        if !setup.hint.is_empty() && y + 1 < rows {
            screen.put(x, y + 1, &truncate(&setup.hint, width), &st::FAINT);
        }
        cursor
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_waveform_rests_until_kumi_waits_then_moves() {
        let still = waveform(WAVE_WIDTH, None);
        assert_eq!(still, RESTING);
        assert_eq!(still.chars().count(), WAVE_WIDTH);
        assert_eq!(waveform(10, None).chars().count(), 10);
        let moving = |t: f64| waveform(WAVE_WIDTH, Some(t));
        assert_eq!(moving(0.3).chars().count(), WAVE_WIDTH);
        assert!(moving(0.3).chars().all(|c| LEVELS.contains(&c)));
        assert_ne!(moving(0.0), moving(0.3), "it moves");
        assert_eq!(moving(1.25), moving(1.25), "the same moment, the same picture");
    }
}
