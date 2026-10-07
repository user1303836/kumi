mod helpers;
pub use helpers::*;
mod draw_live;
mod draw_panel;
mod drawing;
mod events;
mod input;
mod live;
mod panels;
mod setup;
pub use setup::{waveform, LiveSetup, WAVE_WIDTH};

use super::{
    editor::Editor,
    icons::{detect_icon_style_here, IconKind, IconStyle},
    keys::InputEvent,
    picker::{Picker, PickerItem},
    render::Renderer,
    scheduler::FrameScheduler,
    screen::Rect,
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
    /// What's new since the producer last opened Kumi, shown once as it starts.
    pub whats_new: Option<crate::whats_new::News>,
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
            whats_new: None,
        }
    }
}
type Action = Rc<dyn Fn() -> LocalBoxFuture<'static, Result<(), RuntimeError>>>;
type Choice = Rc<dyn Fn(PickerItem) -> LocalBoxFuture<'static, Result<(), RuntimeError>>>;
type PanelRef = Rc<RefCell<Panel>>;
enum Panel {
    Pick { picker: Rc<RefCell<Picker>>, choose: Choice, choosing: Rc<Cell<bool>> },
    Key { provider: ProviderId, secret: String, checking: bool, status: Option<(String, NoticeTone)>, then: Option<Action> },
    ChatGpt { url: Option<String>, abort: Signal, then: Option<Action> },
    Btw { at: usize, scroll: i32 },
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
    changes: Vec<ChangeRecord>,
    last_change: Option<(String, f64)>,
    last_action: Option<LastAction>,
    goal: Option<Value>,
    matching: Option<Value>,
    kept: Vec<Rc<RefCell<Kept>>>,
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
    planning: Option<String>,
    planning_since: f64,
    /// A model call being tried again: why, and when (perf time) the next try starts.
    retry: Option<(String, f64)>,
    busy_since: f64,
    turn_changes: usize,
    recall: Option<(usize, String)>,
    last_sent: Option<String>,
    tree: Option<DeviceTree>,
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
}
impl State {
    fn new(options: &TuiOptions) -> Self {
        Self {
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
            changes: vec![],
            last_change: None,
            last_action: None,
            goal: None,
            matching: None,
            kept: vec![],
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
            planning: None,
            planning_since: 0.,
            retry: None,
            busy_since: 0.,
            turn_changes: 0,
            recall: None,
            last_sent: None,
            tree: None,
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
        }
    }
}
struct Inner {
    options: TuiOptions,
    state: RefCell<State>,
    tty: Tty,
    renderer: RefCell<Renderer>,
    scheduler: FrameScheduler,
    table: Rc<StyleTable>,
    icons: IconStyle,
    depth: ColorDepth,
    tabs: TabPanel,
    voice: Option<VoiceInput>,
    done: watch::Sender<Option<i32>>,
    output_failed: Cell<bool>,
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
            let n = a.state.borrow().changes.len();
            (self.id == "history" && n > 0).then_some(n as i64)
        })
    }
    fn rows(&self, width: i32) -> Vec<TabRow> {
        self.app
            .upgrade()
            .map(|a| {
                let app = TuiApp(a);
                if self.id == "history" {
                    app.history_rows(width)
                } else {
                    app.goal_rows(width)
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
            let tty = Tty::new(TtyOptions {
                input: options.input.clone(),
                output: options.output.clone(),
                mouse: true,
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
                scheduler,
                table: Rc::new(StyleTable::new()),
                icons,
                depth,
                tabs,
                voice,
                done,
                output_failed: Cell::new(false),
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
            for task in [state.tree_refresh.take(), state.wake_timer.take(), state.beat_timer.take()].into_iter().flatten().chain(flashes) {
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
            app.0.tty.restore(false);
            if !app.0.output_failed.get() {
                app.0.options.output.write(&format!("{}\n", app.clean(message.as_deref().unwrap_or(FAREWELL), usize::MAX)));
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
const CHANGE_FLASH_MS: f64 = 4000.;
const QUIET_TOOLS: &[&str] = &["remember", "forget", "save_recipe", "forget_recipe", "reaction"];
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
