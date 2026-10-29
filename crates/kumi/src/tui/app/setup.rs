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
use super::panels::ChatGptPanel;
use super::*;
use kumi_ai::providers::{table, SignIn};
use kumi_common::abort;
use kumi_daw::{
    provider::{older_than, EnableStep, Installed as LiveInstalled},
    setup::DawSetup,
};
use kumi_types::version::KUMI_VERSION;
use tokio::sync::oneshot;

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
    /// Where the Set is that Kumi's restart of Live closed, when Live didn't open with it again.
    set_note: Option<String>,
    /// Why this computer can't reach ChatGPT's sign-in, said under the sign-in choices.
    unreachable: Option<String>,
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
    table::first_choices()
        .into_iter()
        .map(|(provider, label)| {
            let how =
                if provider.row().sign_in == SignIn::Chatgpt { "your ChatGPT plan · opens your browser" } else { "paste an API key" };
            choice(label, how, provider.as_str())
        })
        .collect()
}

/// The model servers running (on this computer, or one named in settings.json elsewhere), offered beside the providers.
fn local_choice(running: &[(String, String)]) -> Choice {
    let here = running.iter().all(|(_, place)| place == "on this computer");
    let named: Vec<String> = running
        .iter()
        .map(|(name, place)| if here || place == "on this computer" { name.clone() } else { format!("{name} {place}") })
        .collect();
    choice(if here { "On this computer" } else { "Your model servers" }, &format!("{} · no sign-in", named.join(" or ")), "local")
}

const CHOOSE_HINT: &str = "↑↓ choose · enter select · esc later";
/// The browser's sign-in link, copied whole, or a code for a browser on another device.
const BROWSER_HINT: &str = "c copies the link · d uses a code · esc back";
const CODE_HINT: &str = "c copies the code · b uses the browser · esc back";
/// How often the sign-in step looks again for models on this computer while the producer chooses.
const LOCAL_LOOK: Duration = Duration::from_secs(3);
/// Where the Set is that Kumi's restart closed, when Kumi didn't know it to open it again.
fn open_recent() -> String {
    let words = crate::daw::words();
    if crate::daw::current().id() == kumi_types::daw::DawId::Ableton {
        "If you saved your Set, it's in Live's File › Open Recent Set.".into()
    } else {
        format!("If you saved your {}, {} lists it with its recent {}.", words.place, words.app, words.places)
    }
}

/// Open Live again, with the Set it had open when Kumi knows it. A Set that won't open (moved, or on a drive that's
/// gone) doesn't keep Live closed: Live opens without it. Whether Live opened, and whether with its Set.
async fn reopen(live: &Rc<dyn DawSetup>, app: Option<String>, set: Option<String>) -> (bool, bool) {
    if set.is_some() && live.start(app.clone(), set).await {
        return (true, true);
    }
    (live.start(app, None).await, false)
}

/// macOS didn't let Kumi ask Live to quit: the producer quits it, and how to let Kumi next time.
fn refused_words(terminal: Option<&str>) -> String {
    format!(
        "macOS didn't let Kumi ask Live to quit, so quit it yourself (save your work first); Kumi then puts its bridge in place and opens Live again. To let Kumi restart Live next time, turn on Live under {} in System Settings › Privacy & Security › Automation.",
        terminal.unwrap_or("your terminal app")
    )
}

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
            set_note: None,
            unreachable: None,
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
        // Several DAWs here and none set up or chosen yet: the producer says which one Kumi works with.
        if let Some(daw) = self.ask_daw().await {
            if daw != crate::daw::current().id() {
                // Chosen and remembered: Kumi opens again with it, and its own setup goes on there.
                self.setup_show(|s| s.quit = Some((0, None)));
                return self.end_setup(None, None).await;
            }
        }
        let live = self.0.options.connect_live.as_ref().map(|l| (l.live.clone(), l.why.clone()));
        if let Some((live, Some(why))) = live {
            let app = match self.setup_live(live.clone(), why).await {
                LiveStep::Done(app) => app,
                LiveStep::Later(also) => return self.end_setup(Some(Step::Live), also).await,
            };
            // The DAW told to use Kumi's bridge, by the producer, when it needs telling (Live's Control Surface).
            if let Some(enable) = crate::daw::current().enable_step() {
                if !self.setup_surface(live, app, enable).await {
                    return self.end_setup(Some(Step::Surface), None).await;
                }
            }
        } else if let Some((live, None)) = live {
            // The bridge was already in place: Live connects as it stands, and later runs need nothing more.
            if self.connected() {
                let version = live.version(None).await;
                self.setup_show(|s| {
                    s.live =
                        Some(version.map(|v| format!("{} {v}", crate::daw::words().app)).unwrap_or_else(|| crate::daw::words().app.into()))
                });
            }
        }
        self.end_setup(None, None).await;
    }

    /// "Which DAW do you work in?", among the DAWs here (`ask_daw`): the one chosen, given to `pick_daw`; None when
    /// there's nothing to ask, or the producer put it off (this run's DAW goes on).
    async fn ask_daw(&self) -> Option<kumi_types::daw::DawId> {
        let (asked, pick) = (self.0.options.ask_daw.clone(), self.0.options.pick_daw.clone()?);
        if asked.len() < 2 {
            return None;
        }
        let app = |daw| crate::daw::provider(daw).map(|provider| provider.profile().words.app).unwrap_or_default();
        let choices = asked.iter().map(|daw| choice(app(*daw), "", daw.as_str())).collect();
        let said = vec!["Which DAW do you work in? Kumi sets itself up for it.".to_string()];
        let picked = self.setup_choose("now", said, choices).await?;
        let daw = kumi_types::daw::DawId::parse(&picked)?;
        pick(daw);
        Some(daw)
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
        // Whether this computer reaches the sign-in at all: looked at once, beside the choices, and said under them
        // (the cause, and the proxy settings) when it doesn't.
        if let Some(reach) = self.0.options.reach.clone() {
            let weak = Rc::downgrade(&self.0);
            tokio::task::spawn_local(async move {
                let problem = reach().await;
                if let (Some(problem), Some(app)) = (problem, weak.upgrade()) {
                    TuiApp(app).setup_show(move |s| s.unreachable = Some(problem));
                }
            });
        }
        loop {
            self.setup_show(|s| s.step = Step::Sign);
            let Some((value, running)) = self.sign_in_choice(&models).await else {
                return false;
            };
            self.setup_show(|s| s.problem = None);
            if value == "local" {
                if self.default_found().await {
                    break;
                }
                let names: Vec<_> = running.iter().map(|(name, _)| name.as_str()).collect();
                self.setup_failed(&format!("{} has no model to talk to yet: add one there, then choose this again.", names.join(" or ")));
                continue;
            }
            let Some(provider) = ProviderId::parse(&value) else { continue };
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

    /// How to sign in, with models on this computer offered too once one of their servers runs: looked for only
    /// here, now and every few seconds while the producer chooses. The choice, and what runs here.
    async fn sign_in_choice(&self, models: &Rc<dyn ModelController>) -> Option<(String, Vec<(String, String)>)> {
        let chosen = self.setup_choose(
            "now",
            vec![
                "How do you want to sign in?".into(),
                "Other providers (OpenRouter, Gemini, DeepSeek and more): /login, once Kumi's open.".into(),
            ],
            sign_in_choices(),
        );
        tokio::pin!(chosen);
        let looking = async {
            loop {
                let running: Vec<(String, String)> =
                    models.local().await.into_iter().filter(|server| server.running).map(|server| (server.name, server.r#where)).collect();
                if !running.is_empty() {
                    return running;
                }
                tokio::time::sleep(LOCAL_LOOK).await;
            }
        };
        tokio::pin!(looking);
        let mut running = vec![];
        loop {
            tokio::select! {
                // The choices are shown before anything is added to them.
                biased;
                value = &mut chosen => return value.map(|value| (value, running)),
                found = &mut looking, if running.is_empty() => {
                    running = found;
                    let offer = local_choice(&running);
                    self.setup_show(move |s| {
                        if s.answer.is_some() {
                            s.choices.push(offer);
                        }
                    });
                }
            }
        }
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
        let browser = provider.row().sign_in != SignIn::ApiKey;
        self.setup_show(move |s| {
            s.choices.clear();
            s.say.clear();
            s.status = if browser { "waiting for the browser" } else { "paste a key" }.into();
            s.waiting = browser;
            s.hint = if browser { BROWSER_HINT } else { "enter check · esc back" }.into();
        });
        self.sign_in(provider, Some(then));
        let signed = loop {
            tokio::select! {
                biased;
                done = &mut rx => break done.is_ok(),
                _ = tokio::time::sleep(Duration::from_millis(50)) => {
                    // A sign-in's `then` runs before anything else once its panel closes.
                    if self.0.state.borrow().overlays.is_empty() || self.closing() {
                        break rx.try_recv().is_ok();
                    }
                }
            }
        };
        self.setup_show(|s| s.waiting = false);
        signed
    }

    /// The bridge into Live: Live quits first when it's open (it asks to save), then the bridge goes in
    /// place and Live opens again, with the Set it had open when Kumi knows it. Once Kumi has asked Live to
    /// quit, or waits for the producer to quit it, every way out of this step leaves Live open again. A Live
    /// that isn't open is looked for first: Live 12 has to be here, and opened once, to take the bridge.
    async fn setup_live(&self, live: Rc<dyn DawSetup>, why: String) -> LiveStep {
        let min = crate::daw::current().min_version();
        let (app, site) = (crate::daw::words().app, crate::daw::site());
        let later = || choice("Later", &format!("chat without {app} for now"), "later");
        self.setup_show(|s| {
            s.step = Step::Live;
            s.problem = None;
        });
        // The Live Kumi has closed (or is closing), and the Set it had open, when Kumi knows it.
        let mut held: Option<String> = None;
        let mut set: Option<String> = None;
        loop {
            let said = why.clone();
            self.setup_show(move |s| {
                s.status = format!("checking {app}…");
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
                // A Live older than 12 can't take Kumi's bridge: said before it's asked to quit, and it stays open.
                let older = live.version(open.clone()).await.filter(|version| older_than(version, min));
                if let Some(version) = older {
                    let say = vec![format!(
                        "Kumi works with {app} {min}: the {app} that's open is {version}. Open {app} {min} instead ({site} has it), then choose Look again."
                    )];
                    let again = vec![choice("Look again", "", "again"), later()];
                    if self.setup_choose(&format!("{app} {min} needed"), say, again).await.as_deref() != Some("again") {
                        return LiveStep::Later(None);
                    }
                    continue;
                }
                let mut say = vec![format!("{app} loads the bridge when it starts."), format!("{app} will ask to save first.")];
                // Asking Live to quit is an Apple Event: the first time, macOS asks whether this terminal may send it.
                let permission = live.asks_permission();
                if let Some(terminal) = &permission {
                    say.push(format!("If macOS asks whether {terminal} can control {app}, choose OK."));
                }
                let wait = choice("I'll quit it", &format!("Kumi waits, then opens {app} again"), "wait");
                // A DAW Kumi can't ask to quit is quit by the producer.
                let restart =
                    if live.can_ask_to_quit() { vec![choice(&format!("Restart {app} now"), "", "restart"), wait] } else { vec![wait] };
                let Some(answer) = self.setup_choose(&format!("{app} is open · restart needed"), say, restart).await else {
                    return self.leave_live(&live, None, None, false).await;
                };
                held = open.clone();
                set = live.open_set();
                let mut asked = answer == "restart";
                self.setup_show(|s| s.holding = true);
                let mut refused = None;
                if asked {
                    self.setup_show(|s| s.status = format!("asking {app} to quit…"));
                    if !live.ask_to_quit().await {
                        // Turned down: asking again can't help, so the producer quits Live.
                        asked = false;
                        refused = Some(refused_words(permission.as_deref()));
                    }
                }
                if !self.wait_closed(&live, asked, refused).await {
                    return self.leave_live(&live, held, set, asked).await;
                }
                // The Live Kumi was connected to has gone: Control Surface waits for the one that opens next.
                self.0.state.borrow_mut().connection = ConnectionState::Disconnected;
            } else if held.is_none() {
                // Not open, and not because Kumi closed it: is there a Live here that can take the bridge?
                match live.installed().await {
                    LiveInstalled::Ready => {}
                    LiveInstalled::NeverOpened(app) => {
                        if !self.first_open(&live, app).await {
                            return LiveStep::Later(None);
                        }
                        continue;
                    }
                    missing => {
                        let say = vec![match missing {
                            LiveInstalled::Older(version) => format!(
                                "Kumi works with {app} {min}: this computer has {app} {version}. Install {app} {min} ({site}) and open it once, then choose Look again."
                            ),
                            _ => format!(
                                "Kumi works with {app} {min}: install it ({site}), or open it once if it's here, then choose Look again."
                            ),
                        }];
                        let again = vec![choice("Look again", "", "again"), later()];
                        if self.setup_choose(&format!("{app} {min} not found"), say, again).await.as_deref() != Some("again") {
                            return LiveStep::Later(None);
                        }
                        continue;
                    }
                }
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
                return self.leave_live(&live, held, set, false).await;
            }
            match installed {
                Ok(version) => {
                    self.setup_show(|s| {
                        s.bridge = Some(format!("bridge {version}"));
                        s.status = format!("opening {app}…");
                        s.say.clear();
                    });
                    // The Live Kumi closed opens again, with its Set when Kumi knows it (else the producer finds it in
                    // Open Recent); one that wasn't open opens as the newest installed.
                    let closed_by_kumi = held.is_some();
                    let app = open.or(held);
                    let (opened, with_set) = reopen(&live, app.clone(), set).await;
                    if closed_by_kumi && !with_set {
                        self.setup_show(|s| s.set_note = Some(open_recent()));
                    }
                    self.setup_show(|s| s.holding = false);
                    let connecting = live.clone();
                    tokio::task::spawn_local(async move { connecting.connect().await });
                    if !opened {
                        self.setup_failed(&format!("Kumi couldn't open {}; open it yourself.", crate::daw::words().app));
                    }
                    self.setup_show(|s| s.waiting = false);
                    return LiveStep::Done(app);
                }
                Err(problem) => {
                    let again = vec![choice("Try again", "", "again"), later()];
                    self.setup_show(|s| s.warn = true);
                    let answer = self.setup_choose("didn't finish", vec![problem], again).await;
                    self.setup_show(|s| s.warn = false);
                    if answer.as_deref() != Some("again") {
                        return self.leave_live(&live, held, set, false).await;
                    }
                }
            }
        }
    }

    /// Live 12 (`app`) is here but has never been opened, so it has no User Library for the bridge yet: Kumi opens it
    /// once and waits for it to make one. True once it has: Live is open then, and the step restarts it with the bridge.
    async fn first_open(&self, live: &Rc<dyn DawSetup>, app: String) -> bool {
        let min = crate::daw::current().min_version();
        let daw = crate::daw::words().app;
        // Live keeps the bridge in its User Library; another DAW in a folder of its own it makes on its first run.
        let (folder, made) = if crate::daw::current().id() == kumi_types::daw::DawId::Ableton {
            ("no User Library for the bridge", "Live makes its User Library as it starts.")
        } else {
            ("no folder for Kumi's bridge yet", "")
        };
        let say = vec![format!(
            "{daw} {min} hasn't been opened here yet, so it has {folder}. Kumi opens it once, then restarts it with the bridge."
        )];
        let open = vec![choice(&format!("Open {daw}"), "", "open"), choice("Later", &format!("chat without {daw} for now"), "later")];
        if self.setup_choose(&format!("{daw} hasn't been opened yet"), say, open).await.as_deref() != Some("open") {
            return false;
        }
        let stop = self.setup_wait();
        self.setup_show(|s| {
            s.status = format!("opening {daw}…");
            s.say = vec![if made.is_empty() {
                "If it shows a dialog, answer it.".into()
            } else {
                format!("{made} If it shows a dialog, answer it.")
            }];
            s.hint = "esc later".into();
            s.waiting = true;
        });
        if !live.start(Some(app), None).await {
            self.setup_failed(&format!("Kumi couldn't open {daw}; open it yourself."));
        }
        loop {
            if live.installed().await == LiveInstalled::Ready {
                return true;
            }
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(1)) => {}
                _ = stop.cancelled() => return false,
            }
        }
    }

    /// Wait for Live to close: true once it has, false for later (esc, or a quit put off). When Kumi
    /// asked it to quit and it's still open a while later (its save dialog cancelled, say), Kumi says
    /// so and offers to ask again, still watching for it to close meanwhile. `refused`: the system didn't
    /// let Kumi ask, said in place of the usual words while the producer quits Live.
    async fn wait_closed(&self, live: &Rc<dyn DawSetup>, mut asked: bool, mut refused: Option<String>) -> bool {
        let stop = self.setup_wait();
        let app = crate::daw::words().app;
        loop {
            let yourself = refused
                .clone()
                .unwrap_or_else(|| format!("Quit {app} when you're ready. Kumi then puts its bridge in place and opens {app} again."));
            self.setup_show(move |s| {
                s.status = if asked { format!("waiting for {app} to close") } else { format!("waiting for you to quit {app}") };
                s.say = vec![if asked { format!("Answer {app} if it asks about saving.") } else { yourself }];
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
            let say = vec![format!("{app} hasn't quit. If it asked about saving and you chose Cancel, it stays open.")];
            let again = vec![
                choice("Ask again", &format!("{app} asks to save first"), "again"),
                choice("Later", &format!("chat without {app} for now"), "later"),
            ];
            let still_open = format!("{app} is still open");
            let answer = tokio::select! {
                closed = live.closed(&stop) => {
                    // It closed after all, or esc: the question no longer applies.
                    self.setup_show(|s| {
                        s.answer = None;
                        s.choices.clear();
                    });
                    return closed;
                }
                answer = self.setup_choose(&still_open, say, again) => answer,
            };
            if answer.as_deref() != Some("again") {
                return false;
            }
            if !live.ask_to_quit().await {
                asked = false;
                refused = Some(refused_words(live.asks_permission().as_deref()));
            }
        }
    }

    /// Out of the Live step without the bridge in place. When Kumi had Live closed (or closing), Live
    /// opens again, with `set` when Kumi knows it: now, if it has closed, or, when a quit Kumi asked for is
    /// still being decided (Live's save dialog up), once it closes in the next couple of minutes.
    async fn leave_live(&self, live: &Rc<dyn DawSetup>, held: Option<String>, set: Option<String>, asked: bool) -> LiveStep {
        self.setup_show(|s| {
            s.holding = false;
            s.waiting = false;
        });
        let Some(app) = held else { return LiveStep::Later(None) };
        if live.open_live().await.is_none() {
            let (opened, with_set) = reopen(live, Some(app), set).await;
            let recent = if with_set { String::new() } else { format!(" {}", open_recent()) };
            let app = crate::daw::words().app;
            return LiveStep::Later(Some(if opened {
                format!("{app} is opening again.{recent}")
            } else {
                format!("Kumi couldn't open {app} again; open it yourself.{recent}")
            }));
        }
        if !asked {
            return LiveStep::Later(None);
        }
        let watching = live.clone();
        tokio::task::spawn_local(async move {
            if watching.closed(&abort::timeout(STILL_ASKED_MS)).await {
                reopen(&watching, Some(app), set).await;
            }
        });
        LiveStep::Later(Some(format!(
            "If {} still asks about saving, Cancel keeps it open; if it quits, Kumi opens it again.",
            crate::daw::words().app
        )))
    }

    /// Live connects through the bridge once it has it as a Control Surface: Kumi watches for that. Still not
    /// chosen a while later, Kumi adds where Live's Settings are, and that a dialog in Live comes first: still
    /// one instruction, carried out in Live by the producer, never by Kumi.
    async fn setup_surface(&self, live: Rc<dyn DawSetup>, app: Option<String>, enable: EnableStep) -> bool {
        let stop = self.setup_wait();
        let note = self.0.state.borrow().setup.as_ref().and_then(|s| s.set_note.clone());
        self.setup_show(move |s| {
            s.step = Step::Surface;
            s.status = enable.waiting.into();
            s.say = std::iter::once(enable.say.to_string()).chain(note).collect();
            s.hint = "esc later".into();
            s.waiting = true;
        });
        let mut more = live.settings_menu().map(|menu| format!("{menu} If {} shows a dialog, answer it first.", crate::daw::words().app));
        let more_at = std::time::Instant::now() + live.surface_wait();
        while !self.connected() {
            if self.closing() {
                return false;
            }
            if let Some(more) = more.take_if(|_| std::time::Instant::now() >= more_at) {
                self.setup_show(move |s| s.say.insert(1, more));
            }
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(200)) => {}
                _ = stop.cancelled() => return false,
            }
        }
        let version = live.version(app).await;
        self.setup_show(|s| {
            s.problem = None;
            s.live = Some(version.map(|v| format!("{} {v}", crate::daw::words().app)).unwrap_or_else(|| crate::daw::words().app.into()));
        });
        true
    }

    /// Setup ends: done ("You're set.", then the session), or put off at `later` (with `also` to say), or
    /// the quit put off while Live and its bridge were busy.
    async fn end_setup(&self, later: Option<Step>, also: Option<String>) {
        if self.closing() {
            return;
        }
        let (quit, set_note) = {
            let mut state = self.0.state.borrow_mut();
            let setup = state.setup.as_mut();
            setup.map(|s| (s.quit.take(), s.set_note.take())).unwrap_or_default()
        };
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
        if let Some(note) = set_note {
            self.notice(&note, NoticeTone::Info);
        }
        match later {
            Some(Step::Sign) => self.notice(
                "Sign in when you're ready with /login (ChatGPT with your plan, or others with an API key), or choose a model on this computer with /model.",
                NoticeTone::Info,
            ),
            Some(Step::Live) => self.notice(
                &format!(
                    "Kumi chats without {} for now, and offers to connect it next time.{}",
                    crate::daw::words().app,
                    also.map(|also| format!(" {also}")).unwrap_or_default()
                ),
                NoticeTone::Info,
            ),
            // The bridge is current, so no setup shows next time: the session connects as soon as Live answers.
            Some(Step::Surface) => {
                if let Some(enable) = crate::daw::current().enable_step() {
                    self.notice(enable.put_off, NoticeTone::Info);
                }
            }
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
        // A sign-in open over the setup shows in it, in its words.
        let top = state.overlays.top();
        let top = top.as_ref().map(|top| top.borrow());
        let key = top.as_ref().and_then(|top| top.downcast_ref::<Panel>());
        let chatgpt = top.as_ref().and_then(|top| top.downcast_ref::<ChatGptPanel>());
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
        // Where Live doesn't run (Linux), signing in is all there is.
        let live_here = self.0.options.connect_live.as_ref().is_some_and(|l| l.live.here());
        // The steps in the DAW's words; the enable step only for a DAW that needs one.
        let daw = crate::daw::current();
        let enable = daw.enable_step();
        let connect = format!("Connect to {}", daw.profile().words.app);
        let mut names = vec![(Step::Sign, "Sign in")];
        if live_here {
            names.push((Step::Live, connect.as_str()));
            names.extend(enable.map(|enable| (Step::Surface, enable.label)));
        }
        let column = if width >= 40 { 20 } else { 17 };
        let signing = match (key, chatgpt) {
            (Some(Panel::Key { checking, .. }), _) => Some(if *checking { "checking the key…" } else { "paste a key" }),
            (_, Some(chatgpt)) => Some(if chatgpt.by_code { "waiting for the code" } else { "waiting for the browser" }),
            _ => None,
        };
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
                    (Step::Surface, None) => enable.map(|enable| enable.later).unwrap_or("later").into(),
                    (_, None) => "later".into(),
                };
                let style = if status == "later" || enable.is_some_and(|enable| status == enable.later) { st::FAINT } else { st::ACCENT };
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
        let panel_kind = chatgpt.filter(|_| shown).map(|chatgpt| chatgpt.by_code);
        if let Some(panel) = key.filter(|_| shown) {
            match panel {
                Panel::Key { provider, secret, checking, status, .. } => {
                    let info = provider.row();
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
                _ => {}
            }
        } else if let Some(chatgpt) = chatgpt.filter(|_| shown) {
            if chatgpt.by_code {
                match &chatgpt.code {
                    Some(code) => {
                        say(screen, &mut y, &format!("In a browser on any device, open {} and enter this code:", code.url), st::TEXT);
                        y += 1;
                        if y < rows - 1 {
                            screen.put(x, y, &code.code, &st::BRIGHT);
                        }
                        y += 2;
                        say(screen, &mut y, "Kumi signs in once it's entered. No code? b uses the browser instead.", st::DIM);
                    }
                    None => say(screen, &mut y, "Asking ChatGPT for a sign-in code…", st::TEXT),
                }
            } else {
                say(screen, &mut y, "Finish signing in in your browser.", st::TEXT);
                if let Some(url) = &chatgpt.url {
                    y += 1;
                    say(screen, &mut y, "If it didn't open, open this link (c copies it):", st::DIM);
                    // All of it, edge to edge with nothing beside it, so a selection copies the whole link.
                    for part in helpers::chunk(url, columns.max(1) as usize) {
                        if y < rows - 2 {
                            screen.put(0, y, &part, &st::ACCENT);
                        }
                        y += 1;
                    }
                    y += 1;
                    say(screen, &mut y, "No browser here? d uses a code.", st::DIM);
                }
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
        // Only while the sign-in choices are up: it's about ChatGPT's sign-in, not a key's check or Live.
        if let Some(unreachable) = setup.unreachable.as_ref().filter(|_| setup.step == Step::Sign && setup.answer.is_some()) {
            y += 1;
            say(screen, &mut y, unreachable, st::WARN);
        }
        // A ChatGPT sign-in's keys follow how it signs in, which d changes.
        let hint = match panel_kind {
            Some(true) => CODE_HINT,
            Some(false) => BROWSER_HINT,
            None => setup.hint.as_str(),
        };
        if !hint.is_empty() && y + 1 < rows {
            screen.put(x, y + 1, &truncate(hint, width), &st::FAINT);
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
