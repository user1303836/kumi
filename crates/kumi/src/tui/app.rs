mod helpers;
pub use helpers::*;
mod away;
mod complete;
mod composer;
mod details;
pub use away::{AwaySettings, FrontApp, NotifierRunner};
mod draw_live;
mod draw_panel;
mod drawer;
mod drawing;
mod events;
mod help;
mod input;
mod jobs;
mod live;
mod map;
mod models;
mod music;
mod palette;
mod panels;
mod records;
mod setup;
use kumi_daw::setup::DawSetup;
pub use music::{music_lines, parse_music, MusicAsked, MUSIC_FOOTER};
pub use setup::{waveform, WAVE_WIDTH};
mod sharing;
mod shown;
mod take_back;
mod theme;
mod toast;
mod usage;

use super::{
    alerts::{self, ProgramState, StatusReport, TerminalProfile},
    editor::{Composed, Draft, Editor},
    icons::{detect_icon_style_here, IconKind, IconStyle},
    keymap::{capitalized, Chord, Keymap, Scope, AGAIN_MS},
    keys::InputEvent,
    overlay::{Area, Handled, Overlay, Overlays, Shared},
    picker::{Picker, PickerItem},
    render::Renderer,
    scheduler::FrameScheduler,
    screen::{Rect, Screen},
    style::{ColorDepth, StyleTable},
    tabs::{Hit, Tab, TabPanel, TabRow},
    transcript::{printed, AnswerStatus, Entry, EntryRef, MemoryKind, NoticeTone, StepState, Transcript, PRINTED_MOST},
    tree::TreeRow,
    tty::{Tty, TtyInput, TtyOptions, TtyOutput},
    voice::{VoiceController, VoiceHost, VoiceInput},
};
use crate::{
    config::safe_error_message,
    history::InputHistory,
    terminal::Terminal,
    text::{sanitize_text, StreamingText},
    update::UpdateControl,
};
use futures::{future::LocalBoxFuture, task::noop_waker, FutureExt};
use kumi_agent::core::contracts::*;
use kumi_agent::core::protocol::{op, Capabilities, InProcess, Request, SessionHandle};
use kumi_agent::core::{
    goal::{GoalPhase, GoalStatus},
    goal_mode::{ObjectiveState, ObjectiveStatus, Verdict},
    loop_run::{LoopState, LoopStatus, LoopStop},
    match_run::{MatchState, MatchStatus, MatchStop},
};
use kumi_ai::providers::ProviderId;
use kumi_common::{
    abort::Signal,
    js::{number, string},
    time::{now_ms_f64, perf_now},
};
use kumi_media::voice::VoiceTrouble;
use kumi_net::cause::NetCause;
use kumi_types::contracts::*;
use kumi_types::{command::KUMI_START, errors::RuntimeError};
use serde_json::Value;
use std::{
    cell::{Cell, RefCell},
    future::Future,
    rc::{Rc, Weak},
    task::Context,
    time::Duration,
};
use tokio::{sync::watch, task::JoinHandle};

pub struct PanelTab {
    pub load: Rc<dyn Fn() -> Option<String>>,
    pub save: Rc<dyn Fn(&str)>,
}
/// Whether the map's points play as the mouse comes to them, remembered between runs (h on the map changes it).
pub struct MapHover {
    pub load: Rc<dyn Fn() -> bool>,
    pub save: Rc<dyn Fn(bool)>,
}
pub struct TuiOptions {
    /// The session, which the app reaches only through its protocol.
    pub session: Rc<dyn SessionHandle>,
    pub input: Rc<dyn TtyInput>,
    pub output: Rc<dyn TtyOutput>,
    pub mode: String,
    pub startup_notice: Option<String>,
    pub secrets: Vec<String>,
    pub close_timeout_ms: Option<u64>,
    pub color_depth: Option<ColorDepth>,
    pub frame_ms: Option<f64>,
    /// How often a frame is drawn while something animates (the scheduler's ANIMATION_MS unless set).
    pub animation_ms: Option<f64>,
    /// Whether the map's zooms and moves glide (where it's drawn in cells).
    pub map_glides: bool,
    pub history: Option<Rc<RefCell<InputHistory>>>,
    pub open_browser: Option<Rc<dyn Fn(&str)>>,
    pub icons: Option<IconStyle>,
    pub tabs: Vec<Rc<dyn Tab>>,
    pub panel_tab: Option<PanelTab>,
    pub updates: Option<UpdateControl>,
    /// /daw <name>: Kumi closes and opens again with that DAW. None where Live is the only DAW here.
    pub switch_daw: Option<Rc<dyn Fn(kumi_types::daw::DawId)>>,
    /// Kumi follows the producer to a DAW they opened ([`Terminal::follow_daw`]): it closes and opens again with it.
    /// None where the producer asked for a DAW.
    pub follow_daw: Option<Rc<dyn Fn(kumi_types::daw::DawId)>>,
    /// A first run on a computer with several DAWs and no bridge yet: the DAWs setup asks the producer to choose among,
    /// and what it does with the one chosen (remembers it, and Kumi opens again with it when it isn't this run's).
    pub ask_daw: Vec<kumi_types::daw::DawId>,
    pub pick_daw: Option<Rc<dyn Fn(kumi_types::daw::DawId)>>,
    pub voice: Option<Rc<dyn VoiceController>>,
    /// Live's part of first-run setup; `None` when Kumi chats without Live by choice.
    pub connect_live: Option<ConnectDaw>,
    /// /willington, where Kumi's bridge can carry Willington.
    pub willington: Option<kumi_daw::provider::Toggle>,
    /// What plays a point of the map.
    pub player: Option<Rc<kumi_media::audio::player::Player>>,
    /// Whether the map's points play as the mouse comes to them, remembered; they do when None.
    pub map_hover: Option<MapHover>,
    /// What's new since the producer last opened Kumi, shown once as it starts.
    pub whats_new: Option<crate::whats_new::News>,
    /// How this terminal shows Kumi's status and notifications; read from the environment when None.
    pub terminal: Option<TerminalProfile>,
    /// Every cell written in each frame that changes (the `fullRepaint` setting); the terminal's own way when None.
    pub full_repaint: Option<bool>,
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
    /// Puts text on this computer's clipboard with its own tool, for terminals that don't take the terminal's way; says
    /// whether a tool took it.
    pub copy: Option<Rc<dyn Fn(&str) -> bool>>,
    /// The producer chooses a kept conversation as Kumi starts (`kumi -r`): the list opens with this typed in it.
    pub choose_conversation: Option<String>,
    /// What /export and /share take out: `kumi report`'s redactor, made at each export. None takes out what `secrets`
    /// holds, the home folder and the account name.
    pub redact: Option<crate::export::MakeRedactor>,
    /// Shows a file in the system's file manager (/share); None shows nothing.
    pub reveal: Option<Rc<dyn Fn(&std::path::Path)>>,
    /// Where /export and /share save; None saves in the folder Kumi was started in.
    pub folder: Option<std::path::PathBuf>,
    /// Text dragged across is copied as the mouse button comes up (`copyOnSelect`); otherwise Ctrl-C or right-click
    /// copies it.
    pub copy_on_select: bool,
    /// Kumi reports the mouse (`mouse`): off, the terminal keeps it, for its own selection and scrolling.
    pub mouse: bool,
    /// Quitting prints the conversation into the terminal's scrollback (`transcriptOnQuit`).
    pub transcript_on_quit: bool,
    /// The producer's editor, which ctrl+g opens the draft in ([`crate::tui::external_editor::editor_command`]); None
    /// where there's none.
    pub editor: Option<String>,
    /// Runs `editor` on the draft's file while the terminal is lent to it, to its exit code; None runs it as a program.
    /// Tests give their own.
    pub run_editor: Option<EditorRunner>,
    /// When the header says how full the model's context is (`contextMeter`).
    pub context_meter: ContextMeter,
    /// What was read from the terminal before the screen was Kumi's, while it was asked what it is (`tui::probe`): read
    /// again as the screen starts, so keys typed meanwhile go where they were typed.
    pub typeahead: Vec<u8>,
    /// What keys.json holds (other keys for Kumi's actions), or why it couldn't be read; None when there's no file.
    pub keys: Option<Result<serde_json::Value, String>>,
    /// The colours the screen is drawn in (`theme`): dark, light or colour-blind; None follows the terminal (`system`),
    /// light on a light background, and switches as the terminal says its colours changed.
    pub theme: Option<&'static super::style::Theme>,
}
/// When the header shows how full the answering model's context is: from half full (`auto`), always, or never.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ContextMeter {
    #[default]
    Auto,
    Always,
    Off,
}
/// Runs the producer's editor on a file until it closes: its exit code, 0 when it closed well.
pub type EditorRunner = Rc<dyn Fn(std::path::PathBuf) -> LocalBoxFuture<'static, std::io::Result<i32>>>;
/// The DAW's part of first-run setup: the setup shows it when Kumi's bridge isn't in the DAW yet, or is older
/// than Kumi's.
pub struct ConnectDaw {
    /// Why Live isn't connected yet, in a sentence; `None` when the bridge in Live is Kumi's own.
    pub why: Option<String>,
    /// Kumi's bridge version.
    pub bridge: String,
    /// The DAW's side of setup (named `live` as it always was).
    pub live: Rc<dyn DawSetup>,
}
/// The DAW's part of first-run setup, as the app and its tests have always named it.
pub use ConnectDaw as ConnectLive;
impl TuiOptions {
    pub fn new(controller: Rc<dyn SessionController>, input: Rc<dyn TtyInput>, output: Rc<dyn TtyOutput>, mode: impl Into<String>) -> Self {
        Self::with_session(InProcess::new(controller), input, output, mode)
    }
    /// The screen for a session reached through `session`: one on a thread of its own, say.
    pub fn with_session(
        session: Rc<dyn SessionHandle>,
        input: Rc<dyn TtyInput>,
        output: Rc<dyn TtyOutput>,
        mode: impl Into<String>,
    ) -> Self {
        Self {
            session,
            input,
            output,
            mode: mode.into(),
            startup_notice: None,
            secrets: vec![],
            close_timeout_ms: None,
            color_depth: None,
            frame_ms: None,
            animation_ms: None,
            map_glides: true,
            history: None,
            open_browser: None,
            icons: None,
            tabs: vec![],
            panel_tab: None,
            updates: None,
            switch_daw: None,
            follow_daw: None,
            ask_daw: vec![],
            pick_daw: None,
            voice: None,
            connect_live: None,
            willington: None,
            player: None,
            map_hover: None,
            whats_new: None,
            terminal: None,
            full_repaint: None,
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
            copy_on_select: true,
            mouse: true,
            transcript_on_quit: true,
            editor: None,
            run_editor: None,
            context_meter: ContextMeter::Auto,
            typeahead: Vec::new(),
            keys: None,
            theme: None,
        }
    }
}
type Action = Rc<dyn Fn() -> LocalBoxFuture<'static, Result<(), RuntimeError>>>;
type Choice = Rc<dyn Fn(PickerItem) -> LocalBoxFuture<'static, Result<(), RuntimeError>>>;
struct Aside {
    /// The session's id for it, which its words come in by.
    id: u32,
    question: String,
    answer: String,
    /// Its words made safe to show, as they come.
    stream: StreamingText,
    state: &'static str,
    /// The producer stopped it (closing its panel, or quitting).
    stopped: bool,
}
/// `@` being typed at the cursor: what follows it (`query`), the ask it was sent in, and what the session answered.
struct Naming {
    query: String,
    asked: u64,
    rows: Vec<kumi_agent::core::references::Candidate>,
}
#[derive(Clone)]
struct Held {
    /// What the session gets: each chip's whole paste in its place.
    text: String,
    /// What the conversation shows: each chip as its label.
    shown: String,
    /// The words as the box held them, to come back there as they were, chips and all.
    words: Composed,
    when: &'static str,
    taken: bool,
}
#[derive(Clone)]
struct Pin {
    pin: PinnedNode,
    kind: IconKind,
    /// The FOCUS row pinned, by its ref (the pin's own, or one FOCUS made up for a node the DAW names none for).
    row: String,
}
struct Kept {
    key: String,
    what: MemoryKind,
    title: String,
    forgotten: bool,
    forget: Forget,
}
/// What forgetting something Kumi kept asks of the session.
#[derive(Clone)]
enum Forget {
    Note(String),
    Lesson(String),
    Technique(String),
    Recipe(String),
}
struct LastAction {
    title: String,
    at: f64,
    glyph: String,
    memory: bool,
}
/// A run's latest status (a sound match, a goal, a loop, a /goal objective), and when it began on the performance
/// clock: its time counts on from there between the session's events.
#[derive(Clone)]
struct Timed<T> {
    status: T,
    since: f64,
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
    grid: Option<LauncherStrip>,
    timeline: Option<TimelineStrip>,
    key: Option<String>,
    reading: bool,
}
/// A body of a DAW's own view (a tracker's pattern or matrix, an instrument's keyzones, a track's routing), the one
/// FOCUS shows when the producer is there, with what it was read for.
#[derive(Default)]
struct OwnBody {
    body: Option<FocusBody>,
    key: Option<String>,
    reading: bool,
    again: bool,
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
    focus: Option<DawFocus>,
    catch_up: Option<CatchUp>,
    newer: Option<String>,
    library: Option<LibraryStatus>,
    told_library: bool,
    told_music: bool,
    /// How learning the producer's own music goes (/music shows it).
    music: Option<kumi_listening::music::learning::MusicStatus>,
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
    /// /map while it's open, in place of the conversation, and the key of the image it has up.
    map: Option<super::map::MapView>,
    map_image: Option<String>,
    /// /map open: the map served to the browser, until /map close or Kumi quits.
    map_page: Option<crate::map_server::MapServer>,
    /// Counts /map open and what stops one (/map close, the music changing): a map read for an older one doesn't open;
    /// and whether one is being read.
    map_page_asked: u64,
    map_page_opening: bool,
    /// Counts the map's plays, stops and closings: only the latest play plays, and says when it ended.
    map_play: u64,
    last_change: Option<(String, f64)>,
    last_action: Option<LastAction>,
    goal: Option<Timed<GoalStatus>>,
    matching: Option<Timed<MatchStatus>>,
    /// The loop under way: its status, and when it started.
    looping: Option<Timed<LoopStatus>>,
    /// The /goal objective's latest status (and, while it runs, when it started).
    objective: Option<Timed<ObjectiveStatus>>,
    watching: bool,
    undoing: bool,
    /// A key no action took that starts keys pressed one after another (esc esc), and when it came: the next, soon
    /// after, may finish them.
    pending: Option<(Chord, f64)>,
    hits: Vec<Hit>,
    tabs_area: Option<Rect>,
    selection: Option<super::selection::Selection>,
    /// The details' files opened in the producer's editor (`v`): kept until Kumi quits, as an editor that opens a window
    /// may return before it reads them.
    details_files: Vec<tempfile::TempPath>,
    /// The model that gave each answer, by its entry's id, for the details (ctrl+o).
    answer_models: std::collections::HashMap<u64, String>,
    /// Where the conversation is scrolled to.
    view: super::transcript_view::TranscriptView,
    page: i32,
    menu_index: usize,
    menu_dismissed: bool,
    /// The box's words when ↑↓ last chose in the `/` menu: a command found only by the words of what it does runs on
    /// Enter once it's chosen.
    menu_chosen: Option<String>,
    /// The words the line's command takes for the word being typed, once read (the menu asks once).
    offers: complete::Offers,
    /// Which list the menu shows (none: the commands; else the line before the word whose candidates it lists, or for
    /// `@`'s, where its `@` is): a new one starts at its first row.
    menu_list: Option<String>,
    started: bool,
    closing: bool,
    cancelling: bool,
    suppress: bool,
    failed: bool,
    bytes: usize,
    pending_turn: bool,
    held: Vec<Held>,
    last_stop: Option<StopReason>,
    asides: Vec<Rc<RefCell<Aside>>>,
    /// The id the next aside takes.
    next_aside: u32,
    /// The id the next /slots request takes.
    next_slots: u32,
    /// The id the next ChatGPT sign-in takes, and what each under way does with its address or code.
    next_sign_in: u32,
    sign_ins: std::collections::HashMap<u32, Rc<dyn Fn(Option<String>, Option<kumi_ai::auth::openai_codex::DevicePrompt>)>>,
    transport: Option<Transport>,
    activity: String,
    /// What's open over the conversation (a list, a sign-in, a side answer, a job, the help): the top one takes input.
    overlays: Overlays<TuiApp>,
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
    /// Which earlier message ↑ shows, and the draft set aside meanwhile.
    recall: Option<(usize, Draft)>,
    /// ctrl+r: an earlier message being looked for.
    search: Option<composer::Search>,
    /// ctrl+s: a draft put aside, with its files.
    stash: Option<composer::Stash>,
    /// `@` being typed at the cursor: what follows it, and what the session last said it names.
    references: Option<Naming>,
    /// How many times the session was asked what `@` names: an answer to an older ask is dropped.
    reference_asks: u64,
    last_sent: Option<String>,
    /// The last message sent, as the box held it: the session handing it back puts its chips back too.
    last_words: Option<Composed>,
    /// A held message being sent after an answer: handed back by the session meanwhile, it's left to the send's own
    /// failure to put back, so it comes back once.
    sending_held: Option<String>,
    /// The messages sent this run with chips, by their history entry, newest last: ↑ brings them back as chips.
    sent_words: Vec<(String, Composed)>,
    tree: Option<DeviceTree>,
    /// Bumped whenever `tree` is read again: its rows are made again only then, or for another focus.
    tree_revision: u64,
    tree_rows: Option<(u64, [Option<String>; 3], Rc<[TreeRow]>)>,
    tree_key: Option<String>,
    tree_reading: bool,
    tree_again: bool,
    clip: Option<NoteView>,
    clip_key: Option<String>,
    clip_reading: bool,
    clip_again: bool,
    touched: Option<Touched>,
    strip: Strip,
    own: OwnBody,
    tree_cursor: Option<usize>,
    pinned: Option<Pin>,
    /// Files the producer added, which go with the next message, and whether the clipboard is being read.
    attachments: Vec<Attachment>,
    pasting: bool,
    refreshing: Option<BodyKind>,
    tree_refresh: Option<JoinHandle<()>>,
    wake_timer: Option<JoinHandle<()>>,
    wake_time: Option<f64>,
    beat_timer: Option<JoinHandle<()>>,
    /// The redraws that end changes' flashes; quitting doesn't wait for them.
    flash_timers: Vec<JoinHandle<()>>,
    /// First-run setup, shown in place of the session until it's done or put off.
    setup: Option<setup::Setup>,
    /// A /slots swap's fetch or check under way, by its request's id: Esc stops it.
    slots_check: Option<u32>,
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
    jobs: std::collections::BTreeMap<u32, kumi_agent::core::jobs::JobStatus>,
    /// Jobs that ended whose request carries on, waiting for a moment the producer isn't in the middle of something.
    carry: Vec<u32>,
    /// Each job's record last written to the terminal's status (OSC 7501 `id=job-N`).
    job_status_sent: std::collections::BTreeMap<u32, super::alerts::StatusReport>,
    /// The session's palette as it last stood: PALETTE, and its sounds marked on the map; and PALETTE's rows for it, at
    /// the width they were made for, until it changes.
    palette: Option<kumi_agent::core::palette::PaletteView>,
    palette_rows: Option<(i32, Rc<[TabRow]>)>,
    /// The producer put the pane away (ctrl+\): no side pane at full width, no strip below 24 rows, and the drawer left
    /// folded until they open it.
    pane_away: bool,
    /// Below 100 columns: the drawer under the conversation.
    drawer: drawer::Drawer,
    /// A few words in the composer's hint row for a moment ("Copied 3 lines").
    toast: Option<toast::Toast>,
    /// The conversation rows of the frame being drawn (a copy joins the rows it wrapped and leaves out its margin), and
    /// the areas a drag selects in, each pane and the composer apart: kept with its screen once it's shown.
    text_rows: Vec<super::selection::TextRow>,
    select_areas: Vec<Rect>,
    /// The last left press: when, where, and how many came one after another on that cell (a double or triple click).
    clicks: Option<(f64, i32, i32, u8)>,
    /// What letting go of a drag copied, while its toast shows and nothing else came: the next Ctrl-C copies it again
    /// and the next Esc only takes the toast away, instead of stopping Kumi, clearing the box or quitting.
    copied: Option<(String, f64)>,
    /// Kumi is closing to open again at once (with another DAW, or updated), where the conversation carries on.
    reopening: bool,
    /// A conversation taken up is being shown again from its log: what happened then draws as it did, and nothing of
    /// the moment comes with it (no alert, sound or terminal status, no animation, no panel, nothing sent or carried on).
    replaying: bool,
    /// The model's reasoning since it last spoke or ran a step: what the headline comes from.
    reasoning: String,
    /// The last model call's tokens, with the model that answered it: how full its context is.
    last_call: Option<(Usage, Option<String>)>,
    /// The providers whose lists were read for a model's window, once each.
    windows_asked: std::collections::HashSet<String>,
    /// The plan's window, as this answer began: what share of it the answer took.
    plan_before: Option<kumi_ai::providers::usage::PlanWindow>,
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
            told_music: false,
            music: None,
            willington_off: false,
            news: None,
            news_since: None,
            records: Default::default(),
            history_rows: None,
            expanded: None,
            history_image: None,
            images: shown::Images::default(),
            map: None,
            map_image: None,
            map_page: None,
            map_page_asked: 0,
            map_page_opening: false,
            map_play: 0,
            last_change: None,
            last_action: None,
            goal: None,
            matching: None,
            looping: None,
            objective: None,
            watching: false,
            undoing: false,
            pending: None,
            hits: vec![],
            tabs_area: None,
            selection: None,
            details_files: vec![],
            answer_models: Default::default(),
            view: Default::default(),
            page: 10,
            menu_index: 0,
            menu_dismissed: false,
            menu_chosen: None,
            offers: complete::Offers::default(),
            menu_list: None,
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
            next_aside: 1,
            next_slots: 1,
            next_sign_in: 1,
            sign_ins: std::collections::HashMap::new(),
            transport: None,
            activity: format!("connecting to {}", crate::daw::words().app),
            overlays: Overlays::default(),
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
            search: None,
            stash: None,
            references: None,
            reference_asks: 0,
            last_sent: None,
            last_words: None,
            sending_held: None,
            sent_words: Vec::new(),
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
            own: OwnBody::default(),
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
            palette: None,
            palette_rows: None,
            pane_away: false,
            drawer: drawer::Drawer::default(),
            toast: None,
            text_rows: vec![],
            select_areas: vec![],
            clicks: None,
            copied: None,
            reopening: false,
            replaying: false,
            reasoning: String::new(),
            last_call: None,
            windows_asked: std::collections::HashSet::new(),
            plan_before: None,
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
    /// What that screen's frame says of its cells (its conversation rows, the areas a drag selects in): a selection
    /// starts from both.
    marks: RefCell<super::selection::Marks>,
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
    /// What each key does, keys.json's over Kumi's own.
    keymap: Keymap,
    /// What Kumi couldn't use of keys.json, said as it starts.
    keys_said: Vec<String>,
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
    fn hint(&self, parts: &[(&[&str], &str)]) -> String {
        self.0.upgrade().map(|a| TuiApp(a).keys_hint(parts)).unwrap_or_default()
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
            // The producer's theme, or the terminal's: before anything is drawn or mixed in it.
            super::style::set_theme(options.theme.unwrap_or_else(|| super::style::for_background(profile.background)));
            super::markdown::set_terminal_opens_links(profile.hyperlinks);
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
                mouse: options.mouse,
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
            let scheduler = FrameScheduler::with_intervals(
                Rc::new(move || {
                    if let Some(a) = w.upgrade() {
                        TuiApp(a).draw();
                    }
                }),
                options.frame_ms.unwrap_or(16.),
                options.animation_ms.unwrap_or(super::scheduler::ANIMATION_MS),
            );
            // What the session says between its replies (an aside's words) comes in as its other events do.
            let listener = weak.clone();
            options.session.listen(Rc::new(move |event| {
                if let Some(a) = listener.upgrade() {
                    TuiApp(a).event(event);
                }
            }));
            let (keymap, keys_said) = match &options.keys {
                Some(Ok(file)) => Keymap::new(Some(file)),
                Some(Err(why)) => (Keymap::default(), vec![why.clone()]),
                None => Keymap::new(None),
            };
            let can = options.session.capabilities();
            let mut tabs: Vec<Rc<dyn Tab>> =
                vec![Rc::new(AppTab { app: weak.clone(), id: "history", title: "HISTORY", empty: "Nothing changed yet" })];
            if can.goal {
                tabs.push(Rc::new(AppTab { app: weak.clone(), id: "goal", title: "GOAL", empty: "No goal yet: /goal and what to reach" }));
            }
            if can.jobs {
                tabs.push(Rc::new(jobs::JobsTab { app: weak.clone() }));
            }
            if can.palette {
                let gathers = keymap.key("map.gather").map(|key| format!(" and press {}", capitalized(&key))).unwrap_or_default();
                tabs.push(Rc::new(palette::PaletteTab {
                    app: weak.clone(),
                    empty: format!("Nothing in it yet: pick sounds on /map{gathers}"),
                }));
            }
            tabs.extend(options.tabs.clone());
            let active = options.panel_tab.as_ref().and_then(|p| (p.load)());
            let tabs = TabPanel::new(tabs, active.as_deref(), options.panel_tab.as_ref().map(|p| p.save.clone()));
            let voice = options.voice.as_ref().map(|v| VoiceInput::new(v.clone(), Rc::new(AppVoice(weak.clone()))));
            let (done, _) = watch::channel(None);
            // Links open where an OSC is safe to send, as the status is.
            let renderer = Renderer::new(depth)
                .full_repaint(options.full_repaint.unwrap_or(profile.repaint))
                .erasing(profile.erase)
                .linking(profile.status);
            Inner {
                state: RefCell::new(State::new(&options)),
                options,
                tty,
                renderer: RefCell::new(renderer),
                screen: RefCell::new(None),
                marks: RefCell::default(),
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
                keymap,
                keys_said,
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
    /// What's in the input box, for the menu's tests (tab puts a word in).
    #[doc(hidden)]
    pub fn draft(&self) -> String {
        self.0.state.borrow().editor.text().to_string()
    }
    /// The session, which the app reaches only through its protocol.
    fn session(&self) -> &dyn SessionHandle {
        &*self.0.options.session
    }
    /// What the session can do.
    fn can(&self) -> Capabilities {
        self.0.options.session.capabilities()
    }
    /// What this session and this app have: what the `/` menu, the help and the keys offer.
    fn has(&self) -> crate::has::Has {
        let options = &self.0.options;
        crate::has::Has {
            can: self.can(),
            fronts: kumi_agent::core::commands::Fronts {
                voice: self.0.voice.is_some(),
                updates: options.updates.is_some(),
                switch_daw: options.switch_daw.is_some(),
                willington: self.willington().is_some(),
            },
            mouse: options.mouse,
            copy_on_select: options.copy_on_select,
        }
    }
    /// A hint whose keys are as the keymap has them (`Keymap::hint`).
    fn keys_hint(&self, parts: &[(&[&str], &str)]) -> String {
        self.0.keymap.hint(parts)
    }
    /// Words that name action `id`'s key as the keymap has it (`Keymap::with_key`).
    fn key_words(&self, id: &str, words: &str, without: &str) -> String {
        self.0.keymap.with_key(id, words, without)
    }
    /// The actions an input is in `scope`, in the order they're tried: none for a paste, the mouse or a key bound to
    /// nothing there.
    fn actions(&self, scope: Scope, event: &InputEvent) -> Vec<&'static str> {
        Chord::of(event).map(|chord| self.0.keymap.actions(scope, &chord)).unwrap_or_default()
    }
    fn busy(&self) -> bool {
        matches!(self.session().status().state, TurnState::Running | TurnState::Cancelling)
    }
    fn clean(&self, text: &str, max: usize) -> String {
        head(&sanitize_text(text, &self.0.state.borrow().secrets), max)
    }
    fn notice(&self, text: &str, tone: NoticeTone) {
        let clean = self.clean(text, usize::MAX);
        self.0.state.borrow_mut().selection = None;
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
    /// A request whose answer `then` takes: at once when the session has it at once (what a session in this process
    /// does at once), or when it comes.
    fn when_answered<R>(&self, request: R, then: impl FnOnce(&TuiApp, Result<R::Reply, RuntimeError>) + 'static)
    where
        R: Request + 'static,
    {
        let app = self.clone();
        let mut asking = async move {
            let reply = app.session().op(request).await;
            then(&app, reply);
        }
        .boxed_local();
        if asking.as_mut().poll(&mut Context::from_waker(&noop_waker())).is_pending() {
            tokio::task::spawn_local(asking);
        }
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
        let printing;
        let closing;
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
            state.details_files.clear();
            state.suppress = true;
            state.stream.discard();
            // The producer quitting, not Kumi closing to open again (with another DAW, or updated) or failing to start.
            printing = self.0.options.transcript_on_quit && message.is_none() && !state.reopening;
            let flashes = std::mem::take(&mut state.flash_timers);
            let timers = [state.tree_refresh.take(), state.wake_timer.take(), state.beat_timer.take(), state.status_timer.take()];
            let alerting = [state.alert_timer.take(), state.answer_countdown.take()];
            for task in timers.into_iter().chain(alerting).flatten().chain(flashes) {
                task.abort();
            }
            closing = state.overlays.drain();
            if let Some(setup) = state.setup.as_mut() {
                setup.stop();
            }
            // The browser's map stops answering as Kumi closes.
            state.map_page = None;
            for aside in &state.asides {
                let mut aside = aside.borrow_mut();
                if aside.state == "asking" {
                    aside.stopped = true;
                    self.session().tell(op::StopAside { id: aside.id }.into());
                }
            }
        }
        // What's open lets go of what it holds: a sign-in under way stops, a key half typed goes.
        for overlay in closing {
            overlay.borrow_mut().closed(self);
        }
        if let Some(voice) = &self.0.voice {
            voice.cancel();
        }
        self.0.scheduler.dispose();
        let app = self.clone();
        tokio::task::spawn_local(async move {
            let mut code = code;
            let mut message = message;
            let result =
                tokio::time::timeout(Duration::from_millis(app.0.options.close_timeout_ms.unwrap_or(6000)), app.session().op(op::Close {}))
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
                // The conversation into the terminal's scrollback before the goodbye, with what /export takes out taken
                // out.
                let plain = app.0.icons == IconStyle::Badges;
                let text = if printing { printed(&app.0.state.borrow().transcript.entries, PRINTED_MOST, plain) } else { String::new() };
                if !text.is_empty() {
                    let secrets = app.0.state.borrow().secrets.clone();
                    let redact = crate::export::redactor_now(app.0.options.redact.as_ref(), &secrets).await;
                    app.0.options.output.write(&redact(&text));
                }
                let farewell = message.unwrap_or_else(farewell);
                app.0.options.output.write(&format!("{}\n", app.clean(&farewell, usize::MAX)));
                // The conversation this run had or took up, by the id that brings it back.
                if let Some(now) = continue_hint(app.session().op(op::ConversationNow {}).await.ok().flatten()) {
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
    /// Kumi closes to open again at once (with another DAW, or updated): the conversation carries on there, so it isn't
    /// printed into the scrollback too.
    fn reopen(&self) -> LocalBoxFuture<'static, i32> {
        self.0.state.borrow_mut().reopening = true;
        self.finish(0, None)
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
            let result = app.session().op(op::Cancel {}).await;
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
        self.0.tty.read_again(&self.0.options.typeahead);
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
        for said in &self.0.keys_said {
            self.notice(said, NoticeTone::Warn);
        }
        if let Some(news) = self.0.options.whats_new.clone() {
            let mut state = self.0.state.borrow_mut();
            state.news_since = news.since.clone();
            state.news = Some(state.transcript.add(news_entry(news)));
        }
        // Said on the welcome screen, which a notice would replace; a conversation carried on says it below.
        self.0.state.borrow_mut().willington_off = self.0.options.willington.as_ref().is_some_and(|w| (w.on)() == Some(false));
        self.0.state.borrow_mut().library = self.session().library();
        self.0.scheduler.request();
        self.task(|app| async move {
            if !app.0.state.borrow().closing {
                if let Err(error) = app.session().op(op::Start {}).await {
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
            } else if let Some(filter) = app.0.options.choose_conversation.clone().filter(|_| app.0.state.borrow().overlays.is_empty()) {
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
        // While the terminal is lent to the producer's editor, ctrl+c is the editor's.
        if self.0.state.borrow().closing || self.0.tty.lent() {
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
    fn lent(&self) -> bool {
        self.0.tty.lent()
    }
    fn follow_daw(&self, daw: kumi_types::daw::DawId) -> bool {
        let Some(follow) = self.0.options.follow_daw.clone() else { return false };
        // Nothing the producer is in the middle of: an answer, setup, a panel or question, the map, what they typed,
        // attached or put aside, a search, their editor, a background job. A question asked with a list (the technique
        // offer, the other Set's conversation) is its panel: it counts while that's open, however it closes.
        let waiting = self.0.tty.lent() || {
            let state = self.0.state.borrow();
            state.closing
                || state.setup.is_some()
                || !state.overlays.is_empty()
                || state.map.is_some()
                || !state.editor.is_empty()
                || !state.attachments.is_empty()
                || state.stash.is_some()
                || state.search.is_some()
                || state.jobs.values().any(|job| !job.state.ended())
        };
        if waiting || self.busy() {
            return false;
        }
        // Said in place of the goodbye: Kumi opens again at once.
        let app = crate::daw::provider(daw).map(|provider| provider.profile().words.app).unwrap_or_default();
        follow(daw);
        drop(self.finish(0, Some(format!("{app} is open, so Kumi opens again with it."))));
        true
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
/// The goodbye: each Set's (project's, song's) conversation continues next time.
pub(crate) fn farewell() -> String {
    format!("Kumi closed. Each {}'s conversation continues next time.", crate::daw::words().place)
}
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
/// What the conversation commands say in a chat that keeps none: without the DAW, started without -c or -r.
pub(crate) fn not_kept() -> String {
    format!("A chat without {} keeps no conversations; {k} -c or {k} -r carries on a kept one.", crate::daw::words().app, k = *KUMI_START)
}
/// The line that brings this run's conversation back: when it's kept, and this run made a request in it or took it up.
pub(crate) fn continue_hint(now: Option<ConversationNow>) -> Option<String> {
    now.filter(|now| now.kept && now.touched).map(|now| format!("Continue this conversation: {} -r {}", *KUMI_START, now.id))
}
const CHANGE_FLASH_MS: f64 = 4000.;
/// Steps the conversation doesn't show: what Kumi keeps on its own, and finding a tool that wasn't sent up front.
const QUIET_TOOLS: &[&str] = &["remember", "forget", "save_recipe", "forget_recipe", "reaction", "find_tools"];
fn head(text: &str, count: usize) -> String {
    String::from_utf16_lossy(&text.encode_utf16().take(count).collect::<Vec<_>>())
}
fn assistant(started_at: Option<f64>, status: AnswerStatus) -> Entry {
    Entry::Assistant { text: String::new(), steps: vec![], status, elapsed_ms: None, started_at, thinking: None, footer: None }
}
/// An answer's steps still running, ended as failed. `timed`: each takes how long it ran until now (an answer shown
/// again from a log has no time for a step it cut short: the log doesn't say when it ended).
fn end_steps(entry: &EntryRef, timed: bool) {
    if let Entry::Assistant { steps, .. } = &mut *entry.borrow_mut() {
        let now = perf_now();
        for step in steps {
            if step.state == StepState::Running {
                step.state = StepState::Error;
                step.ended_at = Some(now);
                if timed {
                    step.ms = step.started_at.map(|at| number::round(now - at)).or(step.ms);
                }
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
