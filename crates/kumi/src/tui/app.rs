mod helpers;
pub use helpers::*;
mod away;
pub use away::{AwaySettings, FrontApp, NotifierRunner};
mod draw_live;
mod draw_panel;
mod drawing;
mod events;
mod input;
mod jobs;
mod live;
mod panels;
mod records;
mod setup;
pub use setup::{waveform, LiveInstalled, LiveSetup, WAVE_WIDTH};
mod sharing;
mod shown;

use super::{
    alerts::{self, ProgramState, StatusReport, TerminalProfile},
    editor::Editor,
    icons::{detect_icon_style_here, IconKind, IconStyle},
    keys::InputEvent,
    picker::{Picker, PickerItem},
    render::Renderer,
    scheduler::FrameScheduler,
    screen::{Rect, Screen},
    style::{ColorDepth, StyleTable},
    tabs::{Hit, Tab, TabPanel, TabRow},
    transcript::{AnswerStatus, Entry, EntryRef, MemoryKind, NoticeTone, StepState, Transcript},
    tree::TreeRow,
    tty::{Tty, TtyInput, TtyOptions, TtyOutput},
    voice::{VoiceController, VoiceHost, VoiceInput},
};
use crate::{
    config::safe_error_message,
    history::InputHistory,
    models::ModelController,
    terminal::Terminal,
    text::{sanitize_text, StreamingText},
    update::UpdateControl,
    willington::WillingtonControl,
};
use futures::{future::LocalBoxFuture, FutureExt};
use kumi_common::{
    abort::Signal,
    js::{number, string},
    time::{now_ms_f64, perf_now},
};
use kumi_runtime::{
    core::{contracts::*, errors::RuntimeError},
    providers::ProviderId,
    voice::VoiceTrouble,
    web::cause::NetCause,
    KUMI_START,
};
use serde_json::Value;
use std::{
    cell::{Cell, RefCell},
    future::Future,
    rc::{Rc, Weak},
    time::Duration,
};
use tokio::{sync::watch, task::JoinHandle};

pub struct PanelTab {
    pub load: Rc<dyn Fn() -> Option<String>>,
    pub save: Rc<dyn Fn(&str)>,
}
pub struct TuiOptions {
    pub controller: Rc<dyn SessionController>,
    pub input: Rc<dyn TtyInput>,
    pub output: Rc<dyn TtyOutput>,
    pub models: Option<Rc<dyn ModelController>>,
    pub mode: String,
    pub startup_notice: Option<String>,
    pub secrets: Vec<String>,
    pub close_timeout_ms: Option<u64>,
    pub color_depth: Option<ColorDepth>,
    pub frame_ms: Option<f64>,
    pub history: Option<Rc<RefCell<InputHistory>>>,
    pub open_browser: Option<Rc<dyn Fn(&str)>>,
    pub icons: Option<IconStyle>,
    pub tabs: Vec<Rc<dyn Tab>>,
    pub panel_tab: Option<PanelTab>,
    pub updates: Option<UpdateControl>,
    pub voice: Option<Rc<dyn VoiceController>>,
    /// Live's part of first-run setup; `None` when Kumi chats without Live by choice.
    pub connect_live: Option<ConnectLive>,
    /// /willington, where Kumi's bridge can carry Willington.
    pub willington: Option<WillingtonControl>,
    /// /slots: which model does each listening job, swapped and taken back.
    pub slots: Option<Rc<kumi_runtime::slots::SlotsContext>>,
    /// What's new since the producer last opened Kumi, shown once as it starts.
    pub whats_new: Option<crate::whats_new::News>,
    /// How this terminal shows Kumi's status and notifications; read from the environment when None.
    pub terminal: Option<TerminalProfile>,
    /// What the producer chose about being told while away from Kumi's window.
    pub away: AwaySettings,
    /// Runs the program that shows a desktop notification, for a terminal without its own; None runs it.
    pub run_notifier: Option<NotifierRunner>,
    /// The shortest time between two alerts, and between checks whether the DAW still records; tests shorten it.
    pub alert_gap_ms: Option<f64>,
    /// Whether the terminal's app is in front, for a terminal that says nothing of focus; None asks the system.
    pub front_app: Option<FrontApp>,
    /// Fixed phrases ("tempo 124", "mute drums") change Live at once, with no model call (`instantPhrases`).
    pub instant: bool,
    /// How changes' pictures show: read from the terminal when None.
    pub graphics: Option<super::pictures::Graphics>,
    /// How to start Kumi next time, said after the farewell when this window can't yet ([`crate::install::next_time`]).
    pub next_time: Option<String>,
    /// Before signing in: whether this computer reaches the sign-in, and if not, why and what to do, in a sentence.
    pub reach: Option<Rc<dyn Fn() -> LocalBoxFuture<'static, Option<String>>>>,
    /// Kumi runs on another computer (over SSH), whose browser isn't the producer's: ChatGPT signs in with a code.
    pub remote: bool,
    /// How the producer runs Kumi from a shell ("kumi"), where Kumi's words name one of its commands.
    pub command: String,
    /// Puts text on this computer's clipboard with its own tool, for terminals that don't take the terminal's way.
    pub copy: Option<Rc<dyn Fn(&str)>>,
    /// The producer chooses a kept conversation as Kumi starts (`kumi -r`): the list opens with this typed in it.
    pub choose_conversation: Option<String>,
    /// What /export and /share take out: `kumi report`'s redactor, made at each export. None takes out what `secrets`
    /// holds, the home folder and the account name.
    pub redact: Option<crate::export::MakeRedactor>,
    /// Shows a file in the system's file manager (/share); None shows nothing.
    pub reveal: Option<Rc<dyn Fn(&std::path::Path)>>,
    /// Where /export and /share save; None saves in the folder Kumi was started in.
    pub folder: Option<std::path::PathBuf>,
}
/// Live's part of first-run setup: the setup shows it when Kumi's bridge isn't in Live yet, or is older
/// than Kumi's.
pub struct ConnectLive {
    /// Why Live isn't connected yet, in a sentence; `None` when the bridge in Live is Kumi's own.
    pub why: Option<String>,
    /// Kumi's bridge version.
    pub bridge: String,
    pub live: Rc<dyn LiveSetup>,
}
impl TuiOptions {
    pub fn new(controller: Rc<dyn SessionController>, input: Rc<dyn TtyInput>, output: Rc<dyn TtyOutput>, mode: impl Into<String>) -> Self {
        Self {
            controller,
            input,
            output,
            mode: mode.into(),
            models: None,
            startup_notice: None,
            secrets: vec![],
            close_timeout_ms: None,
            color_depth: None,
            frame_ms: None,
            history: None,
            open_browser: None,
            icons: None,
            tabs: vec![],
            panel_tab: None,
            updates: None,
            voice: None,
            connect_live: None,
            willington: None,
            slots: None,
            whats_new: None,
            terminal: None,
            away: AwaySettings::default(),
            run_notifier: None,
            alert_gap_ms: None,
            front_app: None,
            instant: true,
            graphics: None,
            next_time: None,
            reach: None,
            remote: false,
            command: "kumi".into(),
            copy: None,
            choose_conversation: None,
            redact: None,
            reveal: None,
            folder: None,
        }
    }
}
type Action = Rc<dyn Fn() -> LocalBoxFuture<'static, Result<(), RuntimeError>>>;
type Choice = Rc<dyn Fn(PickerItem) -> LocalBoxFuture<'static, Result<(), RuntimeError>>>;
type PanelRef = Rc<RefCell<Panel>>;
enum Panel {
    Pick {
        picker: Rc<RefCell<Picker>>,
        choose: Choice,
        choosing: Rc<Cell<bool>>,
    },
    Key {
        provider: ProviderId,
        secret: String,
        checking: bool,
        status: Option<(String, NoticeTone)>,
        then: Option<Action>,
    },
    /// ChatGPT's sign-in: in this computer's browser (`url`, once it's known), or `by_code`, with a code entered in
    /// a browser anywhere.
    ChatGpt {
        url: Option<String>,
        code: Option<kumi_runtime::auth::openai_codex::DevicePrompt>,
        by_code: bool,
        abort: Signal,
        then: Option<Action>,
    },
    Btw {
        at: usize,
        scroll: i32,
    },
    /// A background job's steps and report.
    Job {
        id: u32,
        scroll: i32,
    },
}
struct Aside {
    question: String,
    answer: String,
    state: &'static str,
    abort: Signal,
}
#[derive(Clone)]
struct Held {
    text: String,
    when: &'static str,
    taken: bool,
}
#[derive(Clone)]
struct Pin {
    pin: PinnedNode,
    kind: IconKind,
}
struct Kept {
    key: String,
    what: MemoryKind,
    title: String,
    forgotten: bool,
    forget: Rc<dyn Fn() -> LocalBoxFuture<'static, Result<bool, RuntimeError>>>,
}
struct LastAction {
    title: String,
    at: f64,
    glyph: String,
    memory: bool,
}
#[derive(Default)]
struct Used {
    input: f64,
    output: f64,
    cached: f64,
    answers: usize,
}
#[derive(Default)]
struct Strip {
    session: Option<SessionStrip>,
    arrangement: Option<ArrangementStrip>,
    key: Option<String>,
    reading: bool,
}
struct State {
    /// The web page saved last for this conversation (by /share or /export), with its key: /share shows it again while
    /// nothing in the conversation changed.
    shared: Option<(u64, std::path::PathBuf)>,
    editor: Editor,
    transcript: Transcript,
    stream: StreamingText,
    secrets: Vec<String>,
    current: Option<EntryRef>,
    used: Used,
    connection: ConnectionState,
    set_name: Option<String>,
    focus: Option<LiveFocus>,
    catch_up: Option<CatchUp>,
    newer: Option<String>,
    library: Option<LibraryStatus>,
    told_library: bool,
    /// Willington's bindings are off in the bridge that carries them, and the producer has yet to be told.
    willington_off: bool,
    /// What's new, shown as Kumi started: a conversation carried on at the start comes above it.
    news: Option<EntryRef>,
    /// The version that news is since, where /changelog starts.
    news_since: Option<String>,
    /// Kumi's changes and what it kept, which HISTORY shows: changed only through its methods, which say so.
    records: records::Records,
    /// HISTORY's rows, made again when the records change, at another width, or when a row opens or closes.
    history_rows: Option<(u64, i32, Option<String>, Rc<[TabRow]>)>,
    /// The change whose picture is open in HISTORY.
    expanded: Option<String>,
    /// The image the open row shows (on a terminal with graphics), kept up while it's open.
    history_image: Option<String>,
    /// Pictures drawn, and images uploaded to the terminal.
    images: shown::Images,
    last_change: Option<(String, f64)>,
    last_action: Option<LastAction>,
    goal: Option<Value>,
    matching: Option<Value>,
    /// The loop under way: its status, and when it started.
    looping: Option<Value>,
    /// The /goal objective's latest status (and, while it runs, when it started).
    objective: Option<Value>,
    watching: bool,
    undoing: bool,
    hits: Vec<Hit>,
    tabs_area: Option<Rect>,
    scroll: i32,
    last_total: i32,
    page: i32,
    menu_index: usize,
    menu_dismissed: bool,
    started: bool,
    closing: bool,
    cancelling: bool,
    suppress: bool,
    failed: bool,
    bytes: usize,
    pending_turn: bool,
    held: Vec<Held>,
    last_stop: Option<String>,
    asides: Vec<Rc<RefCell<Aside>>>,
    transport: Option<LiveTransport>,
    activity: String,
    panel: Option<PanelRef>,
    /// The picker asking whether to keep a technique: closing it is a no.
    offer: Option<Rc<RefCell<Picker>>>,
    /// The picker asking about another Set's conversation: closing it leaves that conversation.
    set_ask: Option<Rc<RefCell<Picker>>>,
    planning: Option<String>,
    planning_since: f64,
    /// A model call being tried again: why, and when (perf time) the next try starts.
    retry: Option<(String, f64)>,
    /// What a long wait for the model is for (a plan's limit, the connection), and when it ends (epoch ms).
    waiting_for: Option<(WaitFor, f64)>,
    /// What refused the call on the way, for a wait for the connection: a certificate (TLS) or a proxy.
    waiting_cause: Option<NetCause>,
    /// That proxy isn't answering, rather than answering no.
    waiting_proxy_down: bool,
    /// A fixed phrase is changing Live: its changes say "instant" in NOW.
    instant_running: bool,
    instant_changes: Vec<String>,
    busy_since: f64,
    turn_changes: usize,
    recall: Option<(usize, String)>,
    last_sent: Option<String>,
    tree: Option<DeviceTree>,
    /// Bumped whenever `tree` is read again: its rows are made again only then, or for another focus.
    tree_revision: u64,
    tree_rows: Option<(u64, [Option<String>; 3], Rc<[TreeRow]>)>,
    tree_key: Option<String>,
    tree_reading: bool,
    tree_again: bool,
    clip: Option<ClipView>,
    clip_key: Option<String>,
    clip_reading: bool,
    clip_again: bool,
    touched: Option<Touched>,
    strip: Strip,
    tree_cursor: Option<usize>,
    pinned: Option<Pin>,
    /// Files the producer added, which go with the next message, and whether the clipboard is being read.
    attachments: Vec<Attachment>,
    pasting: bool,
    refreshing: Option<&'static str>,
    tree_refresh: Option<JoinHandle<()>>,
    wake_timer: Option<JoinHandle<()>>,
    wake_time: Option<f64>,
    beat_timer: Option<JoinHandle<()>>,
    /// The redraws that end changes' flashes; quitting doesn't wait for them.
    flash_timers: Vec<JoinHandle<()>>,
    /// First-run setup, shown in place of the session until it's done or put off.
    setup: Option<setup::Setup>,
    /// A /slots swap's fetch or check under way: Esc stops it.
    slots_check: Option<Rc<kumi_common::abort::Controller>>,
    /// Whether Kumi's window is in front, as the terminal says when that changes (assumed until it does), and
    /// since when it isn't.
    window_focused: bool,
    unfocused_since: Option<f64>,
    /// The terminal has said something of focus since Kumi started (some never do).
    focus_reported: bool,
    /// The status last written to the terminal and when; a write waiting for the gap after it.
    status_sent: Option<(StatusReport, f64)>,
    status_timer: Option<JoinHandle<()>>,
    /// How the status rests between answers: done or error with its words, or idle.
    resting: Option<(ProgramState, String)>,
    /// The newest alert waiting for the gap after the last, or for the DAW to stop recording; when the last went.
    alert_waiting: Option<away::Alert>,
    alert_sent_at: f64,
    alert_timer: Option<JoinHandle<()>>,
    /// This answer's run (a /goal, /loop or sound match) said how it ended: its own "done" isn't told too.
    run_told: bool,
    /// A /goal, /loop or sound match ran in this answer: nobody may be there to pick.
    run_active: bool,
    /// Counting down to Kumi taking its recommended answer; any key stops it.
    answer_countdown: Option<JoinHandle<()>>,
    /// Background jobs as they last stood, by id: JOBS, the header's count and the cards.
    jobs: std::collections::BTreeMap<u32, kumi_runtime::core::jobs::JobStatus>,
    /// Jobs that ended whose request carries on, waiting for a moment the producer isn't in the middle of something.
    carry: Vec<u32>,
    /// Each job's record last written to the terminal's status (OSC 7501 `id=job-N`).
    job_status_sent: std::collections::BTreeMap<u32, super::alerts::StatusReport>,
}
impl State {
    fn new(options: &TuiOptions) -> Self {
        Self {
            shared: None,
            editor: Editor::new(),
            transcript: Transcript::default(),
            stream: StreamingText::new(&options.secrets),
            secrets: options.secrets.clone(),
            current: None,
            used: Used::default(),
            connection: if options.mode == "inference-only" { ConnectionState::Disconnected } else { ConnectionState::Connecting },
            set_name: None,
            focus: None,
            catch_up: None,
            newer: None,
            library: None,
            told_library: false,
            willington_off: false,
            news: None,
            news_since: None,
            records: Default::default(),
            history_rows: None,
            expanded: None,
            history_image: None,
            images: shown::Images::default(),
            last_change: None,
            last_action: None,
            goal: None,
            matching: None,
            looping: None,
            objective: None,
            watching: false,
            undoing: false,
            hits: vec![],
            tabs_area: None,
            scroll: 0,
            last_total: 0,
            page: 10,
            menu_index: 0,
            menu_dismissed: false,
            started: false,
            closing: false,
            cancelling: false,
            suppress: false,
            failed: false,
            bytes: 0,
            pending_turn: false,
            held: vec![],
            last_stop: None,
            asides: vec![],
            transport: None,
            activity: "connecting to Live".into(),
            panel: None,
            offer: None,
            set_ask: None,
            planning: None,
            planning_since: 0.,
            retry: None,
            waiting_for: None,
            waiting_cause: None,
            waiting_proxy_down: false,
            instant_running: false,
            instant_changes: Vec::new(),
            busy_since: 0.,
            turn_changes: 0,
            recall: None,
            last_sent: None,
            tree: None,
            tree_revision: 0,
            tree_rows: None,
            tree_key: None,
            tree_reading: false,
            tree_again: false,
            clip: None,
            clip_key: None,
            clip_reading: false,
            clip_again: false,
            touched: None,
            strip: Strip::default(),
            tree_cursor: None,
            pinned: None,
            attachments: vec![],
            pasting: false,
            refreshing: None,
            tree_refresh: None,
            wake_timer: None,
            wake_time: None,
            beat_timer: None,
            flash_timers: Vec::new(),
            setup: None,
            slots_check: None,
            window_focused: true,
            unfocused_since: None,
            focus_reported: false,
            status_sent: None,
            status_timer: None,
            resting: None,
            alert_waiting: None,
            alert_sent_at: f64::NEG_INFINITY,
            alert_timer: None,
            run_told: false,
            run_active: false,
            answer_countdown: None,
            jobs: std::collections::BTreeMap::new(),
            carry: vec![],
            job_status_sent: std::collections::BTreeMap::new(),
        }
    }
}
struct Inner {
    options: TuiOptions,
    state: RefCell<State>,
    tty: Tty,
    renderer: RefCell<Renderer>,
    /// The last frame's screen, drawn over again when the window keeps its size: no new cells each frame.
    screen: RefCell<Option<Screen>>,
    scheduler: FrameScheduler,
    table: Rc<StyleTable>,
    icons: IconStyle,
    depth: ColorDepth,
    tabs: TabPanel,
    voice: Option<VoiceInput>,
    done: watch::Sender<Option<i32>>,
    output_failed: Cell<bool>,
    /// How this terminal shows Kumi's status and notifications.
    profile: TerminalProfile,
    /// How changes' pictures show on this terminal.
    graphics: super::pictures::Graphics,
}
#[derive(Clone)]
pub struct TuiApp(Rc<Inner>);
pub fn create_tui(options: TuiOptions) -> TuiApp {
    TuiApp::new(options)
}
struct AppTab {
    app: Weak<Inner>,
    id: &'static str,
    title: &'static str,
    empty: &'static str,
}
impl Tab for AppTab {
    fn id(&self) -> &str {
        self.id
    }
    fn title(&self) -> &str {
        self.title
    }
    fn empty(&self) -> Option<&str> {
        Some(self.empty)
    }
    fn badge(&self) -> Option<i64> {
        self.app.upgrade().and_then(|a| {
            let n = a.state.borrow().records.changes().len();
            (self.id == "history" && n > 0).then_some(n as i64)
        })
    }
    fn rows(&self, width: i32) -> Rc<[TabRow]> {
        self.app
            .upgrade()
            .map(|a| {
                let app = TuiApp(a);
                if self.id == "history" {
                    app.history_rows(width)
                } else {
                    app.goal_rows(width).into()
                }
            })
            .unwrap_or_default()
    }
}
struct AppVoice(Weak<Inner>);
impl VoiceHost for AppVoice {
    fn insert(&self, text: &str) {
        if let Some(a) = self.0.upgrade() {
            TuiApp(a).insert_spoken(text);
        }
    }
    fn send(&self) {
        if let Some(a) = self.0.upgrade() {
            TuiApp(a).submit_task();
        }
    }
    fn notice(&self, text: &str, tone: NoticeTone) {
        if let Some(a) = self.0.upgrade() {
            TuiApp(a).notice(text, tone);
        }
    }
    fn offer(&self, trouble: VoiceTrouble) {
        if let Some(a) = self.0.upgrade() {
            TuiApp(a).offer_voice_fix(trouble);
        }
    }
    fn names(&self) -> Vec<String> {
        self.0.upgrade().map(|a| TuiApp(a).spoken_names()).unwrap_or_default()
    }
    fn redraw(&self) {
        if let Some(a) = self.0.upgrade() {
            a.scheduler.request();
        }
    }
    fn animate(&self, on: bool) {
        if let Some(a) = self.0.upgrade() {
            let busy = TuiApp(a.clone()).busy();
            a.scheduler.set_animating(on || busy);
        }
    }
}
impl TuiApp {
    pub fn new(options: TuiOptions) -> Self {
        Self(Rc::new_cyclic(|weak: &Weak<Inner>| {
            let depth = options.color_depth.unwrap_or_else(super::style::detect_color_depth_here);
            let icons = options.icons.unwrap_or_else(detect_icon_style_here);
            let w = weak.clone();
            let resize = weak.clone();
            let profile = options.terminal.clone().unwrap_or_else(away::detect_profile_here);
            let graphics = options.graphics.unwrap_or_else(|| {
                let cells_only = std::env::var("KUMI_PICTURES").is_ok_and(|value| value == "cells");
                super::pictures::graphics_for(&profile, depth, icons == IconStyle::Glyphs, cells_only)
            });
            // Giving the terminal back takes Kumi's status with it, the crash path's way too.
            let mut farewell = Vec::new();
            if profile.status {
                farewell.push(alerts::clear_sequence(None));
            }
            if profile.progress {
                farewell.push(alerts::PROGRESS_CLEAR.to_string());
            }
            // Pictures Kumi put up as images, by the range of ids Kumi gives them: the crash path takes them down too.
            if graphics == super::pictures::Graphics::Kitty {
                farewell.push(super::pictures::kitty::DELETE_ALL.to_string());
            }
            let farewell = farewell.iter().map(|s| if profile.tmux { alerts::tmux_wrap(s) } else { s.clone() }).collect();
            let tty = Tty::new(TtyOptions {
                input: options.input.clone(),
                output: options.output.clone(),
                mouse: true,
                farewell,
                on_input: Rc::new(move |event| {
                    if let Some(a) = w.upgrade() {
                        TuiApp(a).on_input(event);
                    }
                }),
                on_resize: Rc::new(move || {
                    if let Some(a) = resize.upgrade() {
                        a.renderer.borrow_mut().invalidate();
                        a.scheduler.request();
                    }
                }),
            });
            let w = weak.clone();
            let scheduler = FrameScheduler::new(
                Rc::new(move || {
                    if let Some(a) = w.upgrade() {
                        TuiApp(a).draw();
                    }
                }),
                options.frame_ms.unwrap_or(16.),
            );
            let mut tabs: Vec<Rc<dyn Tab>> =
                vec![Rc::new(AppTab { app: weak.clone(), id: "history", title: "HISTORY", empty: "Nothing changed yet" })];
            if options.controller.has_goal() {
                tabs.push(Rc::new(AppTab { app: weak.clone(), id: "goal", title: "GOAL", empty: "No goal yet: /goal and what to reach" }));
            }
            if options.controller.has_jobs() {
                tabs.push(Rc::new(jobs::JobsTab { app: weak.clone() }));
            }
            tabs.extend(options.tabs.clone());
            let active = options.panel_tab.as_ref().and_then(|p| (p.load)());
            let tabs = TabPanel::new(tabs, active.as_deref(), options.panel_tab.as_ref().map(|p| p.save.clone()));
            let voice = options.voice.as_ref().map(|v| VoiceInput::new(v.clone(), Rc::new(AppVoice(weak.clone()))));
            let (done, _) = watch::channel(None);
            Inner {
                state: RefCell::new(State::new(&options)),
                options,
                tty,
                renderer: RefCell::new(Renderer::new(depth)),
                screen: RefCell::new(None),
                scheduler,
                table: Rc::new(StyleTable::new()),
                icons,
                depth,
                tabs,
                voice,
                done,
                output_failed: Cell::new(false),
                profile,
                graphics,
            }
        }))
    }
    pub fn flush(&self) {
        self.0.scheduler.flush();
    }
    /// Number of transcript entries laid out, for the source latency-budget check.
    #[doc(hidden)]
    pub fn transcript_layout_count(&self) -> usize {
        self.0.state.borrow().transcript.laid_out
    }
    fn busy(&self) -> bool {
        matches!(self.0.options.controller.status().state, TurnState::Running | TurnState::Cancelling)
    }
    fn clean(&self, text: &str, max: usize) -> String {
        head(&sanitize_text(text, &self.0.state.borrow().secrets), max)
    }
    fn notice(&self, text: &str, tone: NoticeTone) {
        let clean = self.clean(text, usize::MAX);
        static LINES: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
            regex::Regex::new(r"[\t-\r \u{00a0}\u{1680}\u{2000}-\u{200a}\u{2028}\u{2029}\u{202f}\u{205f}\u{3000}\u{feff}]*\n[\t-\r \u{00a0}\u{1680}\u{2000}-\u{200a}\u{2028}\u{2029}\u{202f}\u{205f}\u{3000}\u{feff}]*").unwrap()
        });
        let text = LINES.replace_all(&clean, " ");
        self.0.state.borrow_mut().transcript.add(Entry::Notice { text: head(&text, 2048), tone });
        self.0.scheduler.request();
    }
    fn error(&self, error: &RuntimeError) {
        let message = safe_error_message(Some(&error.message()), &self.0.state.borrow().secrets);
        self.notice(&message, NoticeTone::Warn);
    }
    fn task<F, Fut>(&self, f: F)
    where
        F: FnOnce(TuiApp) -> Fut + 'static,
        Fut: Future<Output = Result<(), RuntimeError>> + 'static,
    {
        let app = self.clone();
        tokio::task::spawn_local(async move {
            if let Err(error) = f(app.clone()).await {
                app.error(&error);
            }
        });
    }
    fn done(&self) -> LocalBoxFuture<'static, i32> {
        let mut receiver = self.0.done.subscribe();
        async move {
            loop {
                if let Some(code) = *receiver.borrow_and_update() {
                    return code;
                }
                if receiver.changed().await.is_err() {
                    return 1;
                }
            }
        }
        .boxed_local()
    }
    fn finish(&self, code: i32, message: Option<String>) -> LocalBoxFuture<'static, i32> {
        {
            let mut state = self.0.state.borrow_mut();
            if state.closing {
                return self.done();
            }
            // Kumi's bridge going into Live, or Live closed by Kumi: the quit waits until both are done with.
            if state.setup.as_mut().is_some_and(|setup| setup.defer_quit(code, message.clone())) {
                drop(state);
                self.0.scheduler.request();
                return self.done();
            }
            state.closing = true;
            state.suppress = true;
            state.stream.discard();
            let flashes = std::mem::take(&mut state.flash_timers);
            let timers = [state.tree_refresh.take(), state.wake_timer.take(), state.beat_timer.take(), state.status_timer.take()];
            let alerting = [state.alert_timer.take(), state.answer_countdown.take()];
            for task in timers.into_iter().chain(alerting).flatten().chain(flashes) {
                task.abort();
            }
            if let Some(panel) = state.panel.take() {
                match &mut *panel.borrow_mut() {
                    Panel::ChatGpt { abort, .. } => abort.cancel(),
                    Panel::Key { secret, .. } => secret.clear(),
                    _ => {}
                }
            }
            if let Some(setup) = state.setup.as_mut() {
                setup.stop();
            }
            for aside in &state.asides {
                if aside.borrow().state == "asking" {
                    aside.borrow().abort.cancel();
                }
            }
        }
        if let Some(voice) = &self.0.voice {
            voice.cancel();
        }
        self.0.scheduler.dispose();
        let app = self.clone();
        tokio::task::spawn_local(async move {
            let mut code = code;
            let mut message = message;
            let result = tokio::time::timeout(
                Duration::from_millis(app.0.options.close_timeout_ms.unwrap_or(6000)),
                app.0.options.controller.close(),
            )
            .await;
            let error = match result {
                Ok(Ok(())) => None,
                Ok(Err(e)) => Some(e),
                Err(_) => Some(RuntimeError::plain("closing took too long")),
            };
            if let Some(error) = error {
                code = 1;
                if message.is_none() {
                    message = Some(format!(
                        "Kumi didn't close cleanly: {}",
                        safe_error_message(Some(&error.message()), &app.0.state.borrow().secrets)
                    ));
                }
            }
            // Images Kumi put up go with it.
            let images = app.0.state.borrow_mut().images.clear();
            app.0.tty.write(&images);
            app.0.tty.restore(false);
            if !app.0.output_failed.get() {
                app.0.options.output.write(&format!("{}\n", app.clean(message.as_deref().unwrap_or(FAREWELL), usize::MAX)));
                // The conversation this run had or took up, by the id that brings it back.
                if let Some(now) = continue_hint(app.0.options.controller.conversation_now().await) {
                    app.0.options.output.write(&format!("\x1b[2m{now}\x1b[22m\n"));
                }
                if let Some(next) = &app.0.options.next_time {
                    app.0.options.output.write(&format!("{next}\n"));
                }
            }
            app.0.done.send_replace(Some(code));
        });
        self.done()
    }
    fn cancel(&self) {
        {
            let mut state = self.0.state.borrow_mut();
            if state.cancelling || state.closing {
                return;
            }
            state.cancelling = true;
            state.suppress = true;
            state.stream.discard();
        }
        self.task(|app| async move {
            let result = app.0.options.controller.cancel().await;
            app.0.state.borrow_mut().cancelling = false;
            let running = app.running_jobs();
            if running > 0 {
                app.notice(
                    &format!(
                        "{} still running in the background: /jobs to see or stop {}.",
                        if running == 1 { "A job is" } else { "Jobs are" },
                        if running == 1 { "it" } else { "them" }
                    ),
                    NoticeTone::Info,
                );
            }
            app.0.scheduler.request();
            result
        });
    }
}
impl Terminal for TuiApp {
    fn run(&self) -> LocalBoxFuture<'static, i32> {
        {
            let mut state = self.0.state.borrow_mut();
            if state.started || state.closing {
                return self.done();
            }
            state.started = true;
        }
        if let Err(error) = self.0.tty.start() {
            return self.finish(1, Some(format!("Kumi couldn't start: {error}")));
        }
        let weak = Rc::downgrade(&self.0);
        self.0.options.input.on_end(Rc::new(move || {
            if let Some(a) = weak.upgrade() {
                drop(TuiApp(a).finish(0, None));
            }
        }));
        let weak = Rc::downgrade(&self.0);
        self.0.options.output.on_error(Rc::new(move |_| {
            if let Some(a) = weak.upgrade() {
                a.output_failed.set(true);
                drop(TuiApp(a).finish(1, None));
            }
        }));
        if let Some(notice) = &self.0.options.startup_notice {
            self.notice(notice, NoticeTone::Info);
        }
        if let Some(news) = self.0.options.whats_new.clone() {
            let mut state = self.0.state.borrow_mut();
            state.news_since = news.since.clone();
            state.news = Some(state.transcript.add(news_entry(news)));
        }
        // Said on the welcome screen, which a notice would replace; a conversation carried on says it below.
        self.0.state.borrow_mut().willington_off = self.0.options.willington.as_ref().is_some_and(|w| (w.on)() == Some(false));
        self.0.state.borrow_mut().library = self.0.options.controller.library();
        self.0.scheduler.request();
        self.task(|app| async move {
            if !app.0.state.borrow().closing {
                if let Err(error) = app.0.options.controller.start().await {
                    if !app.0.state.borrow().closing {
                        let message =
                            format!("Kumi couldn't start: {}", safe_error_message(Some(&error.message()), &app.0.state.borrow().secrets));
                        app.finish(1, Some(message)).await;
                    }
                }
            }
            Ok(())
        });
        // A step missing (signing in, Live's bridge): the setup runs first, in place of the session.
        let setup = self.begin_setup();
        self.task(move |app| async move {
            if setup {
                app.run_setup(false).await;
            } else if app.needs_sign_in().await {
                // It looked for a default model already.
                app.run_setup(true).await;
            } else if let Err(error) = app.check_model().await {
                app.panel_failed(&error);
            } else if let Some(filter) = app.0.options.choose_conversation.clone().filter(|_| app.0.state.borrow().panel.is_none()) {
                // `kumi -r`: the producer chooses the conversation as Kumi starts.
                if let Err(error) = app.open_conversations(Some(filter).filter(|f| !f.is_empty())).await {
                    app.panel_failed(&error);
                }
            }
            Ok(())
        });
        self.done()
    }
    fn handle_event(&self, event: SessionEvent) {
        self.event(event);
    }
    fn offer_update(&self, latest: &str) {
        let empty = {
            let mut state = self.0.state.borrow_mut();
            if state.closing || state.newer.as_deref() == Some(latest) {
                return;
            }
            state.newer = Some(latest.into());
            state.transcript.is_empty()
        };
        if !empty {
            self.notice(&format!("Kumi {latest} is out: /update gets it."), NoticeTone::Info);
        }
        self.0.scheduler.request();
    }
    fn interrupt(&self) {
        if self.0.state.borrow().closing {
            return;
        }
        if self.busy() {
            self.cancel();
        } else {
            drop(self.finish(0, None));
        }
    }
    fn close(&self) -> LocalBoxFuture<'static, i32> {
        self.finish(0, None)
    }
}
impl Drop for Inner {
    fn drop(&mut self) {
        self.scheduler.dispose();
        let state = self.state.get_mut();
        for timer in [state.tree_refresh.take(), state.wake_timer.take(), state.beat_timer.take()].into_iter().flatten() {
            timer.abort();
        }
    }
}
const FAREWELL: &str = "Kumi closed. Each Set's conversation continues next time.";
/// The conversation going on, for /status: what it's called and the id that brings it back once it's kept.
pub(crate) fn conversation_line(now: &ConversationNow, clean: impl Fn(&str) -> String) -> String {
    let title = clean(&now.title);
    let title = if title.is_empty() { "new".to_string() } else { title };
    if now.kept {
        format!("Conversation: {title} · {}", now.id)
    } else {
        format!("Conversation: {title} (kept once you ask something)")
    }
}
/// What the conversation commands say in a chat that keeps none: without Live, started without -c or -r.
pub(crate) fn not_kept() -> String {
    format!("A chat without Live keeps no conversations; {k} -c or {k} -r carries on a kept one.", k = *KUMI_START)
}
/// The line that brings this run's conversation back: when it's kept, and this run made a request in it or took it up.
pub(crate) fn continue_hint(now: Option<ConversationNow>) -> Option<String> {
    now.filter(|now| now.kept && now.touched).map(|now| format!("Continue this conversation: {} -r {}", *KUMI_START, now.id))
}
const CHANGE_FLASH_MS: f64 = 4000.;
/// Steps the conversation doesn't show: what Kumi keeps on its own, and finding a tool that wasn't sent up front.
const QUIET_TOOLS: &[&str] = &["remember", "forget", "save_recipe", "forget_recipe", "reaction", "find_tools"];
const ACTION_TOOLS: &[&str] = &["play", "fire_scene", "launch_clip", "record", "jump_to_locator", "select", "show"];
fn head(text: &str, count: usize) -> String {
    String::from_utf16_lossy(&text.encode_utf16().take(count).collect::<Vec<_>>())
}
fn assistant(started_at: Option<f64>, status: AnswerStatus) -> Entry {
    Entry::Assistant { text: String::new(), steps: vec![], status, elapsed_ms: None, started_at }
}
fn end_steps(entry: &EntryRef) {
    if let Entry::Assistant { steps, .. } = &mut *entry.borrow_mut() {
        let now = perf_now();
        for step in steps {
            if step.state == StepState::Running {
                step.state = StepState::Error;
                step.ended_at = Some(now);
                step.ms = step.started_at.map(|at| number::round(now - at)).or(step.ms);
                step.doing = None;
            }
        }
    }
}

/// What's new as a transcript entry: shown to the producer, never to the model.
pub(super) fn news_entry(news: crate::whats_new::News) -> Entry {
    let footer = crate::whats_new::more_line(&news);
    Entry::News { title: news.title, items: news.items, footer }
}
