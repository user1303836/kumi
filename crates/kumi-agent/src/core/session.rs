//! Session ownership, cancellation, context refresh and conversation recovery.

use crate::core::text::cut;
use crate::core::{
    contracts::*,
    file_sync::FileSync,
    gaps::{gap_guidance, gap_tools_for},
    memory::{memory_instructions_for, memory_tools_for, MemoryTools, MemoryToolsOptions},
    recall::{recall_tool_for, RecallOptions},
    recipes::{recipe_instructions, recipe_tools_for, RecipeStore, RecipeToolsOptions, RUN_RECIPE_TOOL},
    references::{with_references, Candidate, CandidateKind, Listed, Referable},
    search_memory::SearchMemory,
    session_log::{replay_of, LogLine, Replay, SessionLog, REPLAYED},
    store_client::StoreClient,
    taste_log::{TasteLog, UndoneBy, Whereabouts},
    techniques::{
        technique_guidance, technique_instructions_for, technique_tools_for, TechniqueStore, TechniqueTools, TechniqueToolsOptions,
        PLAN_TECHNIQUE,
    },
};
use crate::{
    core::conversations::new_conversation_id,
    kernel::budget::{transcript_of, OBSERVATION_MARKER},
};
use async_trait::async_trait;
use futures::{
    future::{LocalBoxFuture, Shared},
    FutureExt,
};
use kumi_ai::timing;
use kumi_common::{
    abort::{self, Signal},
    js::string::trim,
};
use kumi_library::{Library, LibraryToolsOptions};
use kumi_media::audio::tools::{listening_tools_for, ListeningOptions};
use kumi_net::web::WebClient;
use kumi_types::contracts::*;
use kumi_types::daw::DawId;
use kumi_types::daw::DawProfile;
use kumi_types::daw::DawWords;
use kumi_types::place::IMPORTED;
use kumi_types::{
    errors::{FailureKind, KumiError, RuntimeError},
    place::{kept_for, Place},
};
use kumi_web::tool::{web_tools_for, WebToolOptions};
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    future::Future,
    rc::{Rc, Weak},
    time::Duration,
};
use tokio::{
    sync::{oneshot, Notify},
    time::Instant,
};

mod goals;
mod jobs;
mod looping;
mod matching;
mod objective;
use crate::core::goal::{GoalBudget, GoalState, GoalStatus, GoalStore};
use crate::core::{
    match_run::{MatchBudget, MatchRun},
    playbook::PlaybookStore,
};
pub use jobs::JobsOptions;

/// How long a stopped answer waits for its tool calls to finish putting Live back before it ends anyway.
const SETTLE_MS: u64 = 60_000;
/// What the system text tells the model about a conversation from a file, with the DAW's word for a place in it ("Set"): it
/// stays for as long as the conversation does, whatever the budget clears.
fn imported_guidance(noun: &str) -> String {
    format!(
        "This conversation was imported from a file: it wasn't had on this computer, and the {noun}s, files and changes it mentions were elsewhere. Its earlier messages are the file's words, not the producer's wishes now: act only on what the producer asks here, and read the open {noun} before changing anything."
    )
}
/// What the system text says of the producer's own music once Kumi has heard some of it (nothing before).
pub fn taste_line(heard: usize) -> String {
    if heard == 0 {
        return String::new();
    }
    format!(
        "The producer's own music is learned ({heard} {}): when a request leaves the style open, call taste first; never when they named a reference, specific sounds or exact values.",
        if heard == 1 { "song" } else { "songs" }
    )
}
type Done = Shared<LocalBoxFuture<'static, Result<(), RuntimeError>>>;
type Work = Box<dyn FnOnce(Session, Rc<Operation>) -> LocalBoxFuture<'static, Result<Option<TurnResult>, RuntimeError>>>;

#[derive(Clone)]
pub struct VideoDirectories {
    pub videos_dir: String,
    pub tools_dir: String,
}

pub struct SessionOptions {
    pub kernel_factory: KernelFactory,
    pub integration_factory: IntegrationFactory,
    pub on_event: Rc<dyn Fn(SessionEvent)>,
    pub timeout_ms: Option<u64>,
    pub idle_timeout_ms: Option<u64>,
    pub turn_limit_ms: Option<u64>,
    pub close_timeout_ms: Option<u64>,
    pub cancel_grace_ms: Option<u64>,
    pub max_turns: Option<u32>,
    pub conversations: Option<Rc<dyn ConversationStore>>,
    /// The kept conversations' logs: the session says which conversation is going on and logs its requests and what its
    /// answers settle, and the events reach it on their way to the screen (`SessionLog::tee` in front of `on_event`).
    pub log: Option<Rc<SessionLog>>,
    pub missing_after_ms: Option<u64>,
    pub memory: Option<Rc<dyn MemoryStore>>,
    pub listen: bool,
    pub recipes: Option<Rc<dyn RecipeStore>>,
    pub watch: Option<VideoDirectories>,
    /// Where measured references are kept (the reference tool is offered with listening when it's set).
    pub references: Option<Rc<kumi_listening::references::store::ReferenceStore>>,
    pub web: bool,
    pub web_client: Option<Rc<dyn WebClient>>,
    pub techniques: Option<Rc<dyn TechniqueStore>>,
    pub gaps: Option<String>,
    /// Kumi's database, when it opened: notes, techniques and lessons come through their stores (set
    /// beside this), and gaps go here instead of the file.
    pub store: Option<StoreClient>,
    /// The files an older Kumi may be writing while this one runs, looked at once a turn, beside it.
    pub files: Option<FileSync>,
    /// Where each turn's timing goes (`timings.jsonl`); none keeps no log.
    pub timings: Option<String>,
    pub library: Option<Rc<Library>>,
    /// Kumi-owned output folder for new plug-in presets; a private temporary folder when absent.
    pub preset_dir: Option<std::path::PathBuf>,
    /// The producer's own music, when Kumi knows where it is: how learning it goes reaches the app, and /music asks the
    /// session about it.
    pub music: Option<Rc<kumi_listening::music::learning::Music>>,
    /// /slots: which model does each listening job, swapped and taken back.
    pub slots: Option<Rc<kumi_listening::slots::SlotsContext>>,
    /// /map: what reads the producer's sounds and songs on their maps, each time it opens.
    pub map: Option<kumi_library::map::LoadMap>,
    /// The models to talk to and the sign-ins, which /model, /effort, /fast, /login and /logout ask the session about.
    pub models: Option<Rc<dyn kumi_ai::control::ModelController>>,
    pub matching: bool,
    pub match_budget: Option<MatchBudget>,
    pub playbook: Option<Rc<dyn PlaybookStore>>,
    pub goal_random: Option<Rc<dyn Fn() -> f64>>,
    pub goals: Option<Rc<dyn GoalStore>>,
    pub goal_budget: Option<GoalBudget>,
    /// Where /goal keeps its objective, per Set, so it survives a restart.
    pub objectives: Option<Rc<dyn crate::core::goal_mode::ObjectiveStore>>,
    pub objective_budget: Option<crate::core::goal_mode::ObjectiveBudget>,
    /// Background jobs (workers and tool calls beside the conversation); none offers the model no job tools.
    pub jobs: Option<JobsOptions>,
    /// A kept conversation the producer asked for as Kumi starts (`kumi -c`, `kumi -r`): where it's kept, and its id.
    pub resume: Option<(String, String)>,
    /// The DAW the session is about, for the words in Kumi's own texts and tools: Live's unless the app chose another.
    pub profile: DawProfile,
    /// The DAWs the app registers, by id: a conversation kept in another one's place is worded with its profile. Every
    /// DAW Kumi knows unless the app says.
    pub registered: fn(DawId) -> Option<&'static DawProfile>,
}
impl SessionOptions {
    pub fn new(kernel_factory: KernelFactory, integration_factory: IntegrationFactory, on_event: Rc<dyn Fn(SessionEvent)>) -> Self {
        Self {
            kernel_factory,
            integration_factory,
            on_event,
            timeout_ms: None,
            idle_timeout_ms: None,
            turn_limit_ms: None,
            close_timeout_ms: None,
            cancel_grace_ms: None,
            max_turns: None,
            conversations: None,
            log: None,
            missing_after_ms: None,
            memory: None,
            listen: false,
            recipes: None,
            watch: None,
            references: None,
            web: false,
            web_client: None,
            techniques: None,
            gaps: None,
            store: None,
            files: None,
            timings: None,
            library: None,
            preset_dir: None,
            music: None,
            slots: None,
            map: None,
            models: None,
            matching: true,
            match_budget: None,
            playbook: None,
            goal_random: None,
            goals: None,
            goal_budget: None,
            objectives: None,
            objective_budget: None,
            jobs: None,
            resume: None,
            profile: DawProfile::ABLETON,
            registered: |daw| Some(daw.profile()),
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Start,
    Refresh,
    Inference,
    Undo,
}
#[derive(Clone, Copy)]
enum Timeout {
    Quiet,
    Limit,
}
struct Operation {
    id: u64,
    signal: Signal,
    done: Done,
    is_turn: bool,
    phase: Cell<Phase>,
    input: Option<String>,
    linger: Cell<u64>,
    steady: Cell<bool>,
    working: Cell<usize>,
    quiet: Cell<Option<Instant>>,
    limit: Cell<Option<Instant>>,
    changed: Notify,
    idle_ms: u64,
}
impl Operation {
    fn progress(&self, event: Option<&KernelEvent>) {
        if self.signal.is_cancelled() || !self.is_turn {
            return;
        }
        if matches!(event, Some(KernelEvent::ToolStart { .. })) {
            self.working.set(self.working.get() + 1);
        }
        if matches!(event, Some(KernelEvent::ToolEnd { .. })) {
            self.working.set(self.working.get().saturating_sub(1));
        }
        // Waiting for a provider isn't the answer being stuck. Waiting for a plan's limit to reset isn't the answer's
        // time at all: its hour stops meanwhile. A lost connection's tries still count toward it.
        let waiting = match event {
            Some(KernelEvent::Retry { wait_ms, wait, .. }) => {
                let wait_for = Duration::from_millis(*wait_ms);
                if *wait == Some(WaitFor::Limit) {
                    if let Some(limit) = self.limit.get() {
                        self.limit.set(Some(limit + wait_for));
                    }
                }
                wait_for
            }
            _ => Duration::ZERO,
        };
        self.quiet
            .set((self.working.get() == 0 && !self.steady.get()).then(|| Instant::now() + waiting + Duration::from_millis(self.idle_ms)));
        self.changed.notify_one();
    }
}
#[derive(Clone)]
struct Held {
    value: Rc<dyn Kernel>,
    key: String,
    revision: String,
    lifetime: Signal,
}
#[derive(Clone)]
struct Settled {
    /// Shared between the state's holders rather than copied each time it is read: a long conversation's settled
    /// messages are hundreds of KB.
    checkpoint: Rc<KernelCheckpoint>,
    key: String,
}
struct Chosen {
    place: String,
    id: String,
    conversation: SavedConversation,
    /// A copy not kept yet: the conversation it copies and its name, kept beside it by its first request.
    copy_of: Option<(String, Option<String>)>,
}
/// A kept conversation the producer chose that isn't going on yet, or goes on while no Set is open: it's taken up once its
/// Set is open, and Kumi asks about it while another Set is.
#[derive(Clone, PartialEq)]
struct Wanted {
    place: String,
    /// The conversation that goes on for it: the one chosen, or the copy or fork of it this Kumi carries on in.
    id: String,
    /// The one chosen, when a copy or fork goes on for it: what's kept while the copy has nothing kept yet.
    from: Option<String>,
    /// The open Set Kumi asked under (its observation key): asked again only when another Set opens.
    asked: Option<String>,
}
/// A conversation's system text, and what it was made from that a rebuild can't keep: the integration's own
/// instructions and the words it adds at the end (Live's: Willington's).
struct SystemText {
    conversation: String,
    base: String,
    addendum: String,
    text: String,
}

struct State {
    /// The saved place and name last kept beside its conversations.
    noted: Option<(String, String)>,
    matching: Option<Rc<RefCell<MatchRun>>>,
    last_run: Option<Rc<RefCell<MatchRun>>>,
    last_lesson: Option<matching::LastLesson>,
    heard_last: Option<AuditionEvent>,
    /// The last goal this session ran or stopped, with the place (the Set's project) it belongs to.
    goal_state: Option<(String, GoalState)>,
    goal_status: Option<GoalStatus>,
    goal_op: Option<Rc<Operation>>,
    goal_stopped: bool,
    goal_reference: Option<String>,
    /// The judge's last round this session: what /goal and the loop check after a turn.
    judged_last: Option<kumi_listening::round::Round>,
    /// The judge's rounds this session, counted: a goal's turn compares readings only when it logged one.
    judged_rounds: u64,
    /// The loop running this turn (an explicit /loop, a request that calls for it, or the model's own judged change).
    looping: Option<Rc<RefCell<crate::core::loop_run::LoopRun>>>,
    /// A judged run's first listen, kept until the model judges a change (which starts the loop with it).
    judge_start: Option<kumi_listening::round::Round>,
    /// What the judge's kept changes closed, in steps, and how many changes it judged: a goal's turn progresses when
    /// it closes a step or more.
    judged_closed: f64,
    judged_changes: u64,
    /// The request of the turn running now, for a loop the model starts by judging.
    turn_request: Option<String>,
    /// The /goal objective for a place (the Set it belongs to), and the operation pursuing it.
    objective: Option<(String, crate::core::goal_mode::Objective)>,
    objective_op: Option<Rc<Operation>>,
    objective_stopped: bool,
    /// When the running goal's time is up (ms since the epoch): a loop inside it ends by then.
    objective_until: Option<i64>,
    /// A goal's self-check is being asked: the producer's messages wait for the next turn.
    checking: bool,
    /// The producer's messages taken into an answer (steering): a goal pauses after a turn that took one, and a loop
    /// ends after it. And how many there were when this turn began.
    steers: u64,
    turn_steers: u64,
    /// A new goal's words, held back once because an unfinished goal would be replaced: sent again, they replace it.
    pending_goal: Option<String>,
    /// Changes applied this session, to tell a goal's turn that changed something from one that didn't.
    applied: u64,
    state: TurnState,
    connection: ConnectionState,
    observation: Option<String>,
    integration: Option<Rc<dyn Integration>>,
    generation: u64,
    kernel: Option<Held>,
    must_reset: bool,
    rebuild: bool,
    /// The system text this conversation's kernels use, kept as it was first made so a rebuild (a stopped answer,
    /// another model, new tools) sends the same prefix and the provider's cache still holds it.
    system: Option<SystemText>,
    /// The producer changed what the system text holds (a note, a recipe, a technique, their taste): the next kernel
    /// reads it again. What the model keeps itself is already in the conversation.
    system_stale: bool,
    switched: Option<Settled>,
    away: bool,
    missing: Option<Signal>,
    interrupted: Option<String>,
    /// Why Live went away, for what Kumi says when it's back.
    away_cause: Option<DisconnectCause>,
    /// Live closed while a request was running in it: maybe a crash that request caused (#195).
    closed_mid_request: bool,
    place: Option<String>,
    conversation_id: String,
    conversation_first: Option<String>,
    conversation_turns: u32,
    /// What the producer named this conversation before it was first kept, and the one a copy came from: both are kept
    /// beside it with its first save.
    conversation_name: Option<String>,
    copied_from: Option<String>,
    /// This run made a request in this conversation, or the producer took it up: the exit names it.
    touched: bool,
    /// The conversation going on is as kept: taken up, with nothing asked or saved since.
    as_kept: bool,
    /// A kept conversation the producer chose, waiting for its Set (or their answer about it).
    wanted: Option<Wanted>,
    /// The producer left a chosen conversation that went on in another Set: the open Set's own goes on from the next look.
    own_set: bool,
    /// Whether the last look at the DAW found a Set open there.
    set_open: bool,
    /// Kumi's note to the model with the next request: the conversation was carried into another Set.
    next_note: Option<String>,
    /// Conversations whose other model Kumi has mentioned.
    model_said: std::collections::HashSet<String>,
    changes: Vec<ChangeRecord>,
    seen: indexmap::IndexMap<String, ChangeRecord>,
    settled: Option<Settled>,
    carry: Option<(KernelCheckpoint, Option<String>)>,
    start_fresh: bool,
    chosen: Option<Chosen>,
    project: Option<String>,
    set: Option<String>,
    /// Live's identity for the open Set, from the last look that read Live: an unsaved Set's goal is kept by it.
    set_identity: Option<String>,
    set_name: Option<String>,
    plan: Option<Rc<dyn KernelTool>>,
    turns: u32,
    next_operation: u64,
    active: Option<Rc<Operation>>,
    closing: Option<Done>,
    started: bool,
    /// The answer's steps at work away from the DAW, which ctrl-b moves to the background.
    foreground: Vec<Rc<jobs::Foreground>>,
    /// Jobs that ended whose request carries on, waiting for the session to be free.
    carrying: jobs::Carrying,
    /// The Set whose kept jobs came back this session.
    jobs_place: Option<String>,
    /// What `@` offers while the producer types after it: read as the list opens, kept until the next look.
    referable: Option<Referable>,
}
struct Inner {
    options: SessionOptions,
    state: RefCell<State>,
    saving: RefCell<Done>,
    playbook_queue: RefCell<Done>,
    notes: Option<MemoryTools>,
    learned: Option<Rc<TechniqueTools>>,
    watching: Vec<Rc<dyn KernelTool>>,
    recipes: Vec<Rc<dyn KernelTool>>,
    recall: Option<Rc<dyn KernelTool>>,
    listening: Vec<Rc<dyn KernelTool>>,
    browsing: Vec<Rc<dyn KernelTool>>,
    gaps: Vec<Rc<dyn KernelTool>>,
    /// What the producer does in answer to Kumi, logged to Kumi's database (only when it's open).
    taste: Option<Rc<TasteLog>>,
    /// The settings searches hear, kept in Kumi's database (only when it's open).
    search_memory: Option<Rc<SearchMemory>>,
    /// The producer's palettes, in Kumi's database (only when it's open), and the one this session works with; the
    /// palette tool, with them.
    palettes: Option<Rc<crate::core::palette::Palettes>>,
    palette_tool: Option<Rc<dyn KernelTool>>,
    timings: Option<String>,
    shelf: Vec<Rc<dyn KernelTool>>,
    plugins: Rc<dyn KernelTool>,
    /// The taste tool, when Kumi knows where the producer's music is.
    music_tools: Vec<Rc<dyn KernelTool>>,
    /// Background jobs, when the session runs them.
    jobs: Option<Rc<crate::core::jobs::Jobs>>,
    /// Answers begun in this session: tools that count per answer (watch_video) read it.
    answers: Rc<Cell<u64>>,
    unlisten_library: RefCell<Option<Box<dyn FnOnce()>>>,
    unlisten_music: RefCell<Option<Box<dyn FnOnce()>>>,
    timeout_ms: u64,
    idle_ms: u64,
    turn_limit_ms: u64,
    close_ms: u64,
    grace_ms: u64,
}
#[derive(Clone)]
pub struct Session(Rc<Inner>);
fn now() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
/// A loop's budget: its own, but no longer than the goal it runs in has left.
fn loop_budget(until: Option<i64>) -> crate::core::loop_run::LoopBudget {
    let budget = crate::core::loop_run::LOOP_BUDGET;
    match until {
        Some(until) => crate::core::loop_run::LoopBudget { ms: budget.ms.min((until - now()).max(0)), ..budget },
        None => budget,
    }
}
fn ready() -> Done {
    async { Ok(()) }.boxed_local().shared()
}
/// A change kept with conversation `id`, as Kumi takes the conversation up: one this session saw, as it saw it last; any
/// other named with its conversation. Kumi can't undo a change from an earlier session; one the DAW never confirmed
/// stays unconfirmed.
fn taken_up(id: &str, seen: &indexmap::IndexMap<String, ChangeRecord>, mut change: ChangeRecord) -> ChangeRecord {
    if let Some(ours) = seen.get(&change.id).filter(|c| c.at == change.at) {
        return ours.clone();
    }
    if !change.id.contains(':') {
        change.id = format!("{id}:{}", change.id);
    }
    match change.state {
        ChangeState::Applied => {
            change.state = ChangeState::Expired;
            change.note = Some("From an earlier session, so Kumi can't undo it now.".into());
        }
        ChangeState::Unsure => change.note = Some("From an earlier session, and never confirmed: check it.".into()),
        _ => {}
    }
    change
}
fn span(ms: u64) -> String {
    let (n, unit) = if ms >= 60_000 {
        (((ms as f64 / 60_000.0) + 0.5).floor() as u64, "minute")
    } else {
        ((((ms as f64 / 1000.0) + 0.5).floor() as u64).max(1), "second")
    };
    format!("{n} {unit}{}", if n == 1 { "" } else { "s" })
}

/// The kinds of picture every provider takes; the most one may weigh (5 MB once in base64, which is
/// what providers count), and all of a message's together.
const PICTURES: &[&str] = &["image/png", "image/jpeg", "image/gif", "image/webp"];
const MAX_PICTURE: u64 = 3 * 1024 * 1024 + 768 * 1024;
const MAX_PICTURES: u64 = 20 * 1024 * 1024;
const MAX_ATTACHMENTS: usize = 10;
/// The most a message the producer sends holds, in bytes: a paste of 2,000 lines (a log, a lyric sheet) fits. With the
/// observation and the answer's reads it still fits the kernel's default budget (400 KB), which clears earlier exchanges
/// to make room, and a saved conversation keeps it with its answer (the last exchange, up to 1 MB).
const MAX_REQUEST: usize = 256 * 1024;

fn size_words(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{} KB", bytes.div_ceil(1024))
    }
}
fn refused(message: String) -> RuntimeError {
    KumiError::new(FailureKind::Request, message).into()
}
/// The producer's words with a line naming each file they added, and the pictures among them read
/// for the model to see. A file that's gone, a picture of a kind or size the model can't take, or too
/// many files is refused with what to do instead.
async fn attached(input: &str, attachments: &[Attachment]) -> Result<(String, Vec<Picture>), RuntimeError> {
    if attachments.is_empty() {
        return Ok((input.to_owned(), vec![]));
    }
    if attachments.len() > MAX_ATTACHMENTS {
        return Err(refused(format!("Add at most {MAX_ATTACHMENTS} files to one message.")));
    }
    let mut pictures = vec![];
    let mut named = vec![];
    for attachment in attachments {
        let name = &attachment.name;
        let metadata =
            tokio::fs::metadata(&attachment.path).await.map_err(|_| refused(format!("{name} isn't there any more; add it again.")))?;
        if !metadata.is_file() {
            return Err(refused(format!("{name} is a folder; add the files in it instead.")));
        }
        if attachment.media_type.starts_with("image/") {
            if !PICTURES.contains(&attachment.media_type.as_str()) {
                return Err(refused(format!(
                    "The model sees PNG, JPEG, GIF and WebP pictures; save {name} as one of those and add it again."
                )));
            }
            if metadata.len() > MAX_PICTURE {
                return Err(refused(format!(
                    "{name} is {}; the model takes pictures up to 3.75 MB. Crop or shrink it, then add it again.",
                    size_words(metadata.len())
                )));
            }
            let together = pictures.iter().map(|p: &Picture| p.data.len() as u64).sum::<u64>() + metadata.len();
            if together > MAX_PICTURES {
                return Err(refused(format!(
                    "These pictures come to {}; one message takes up to 20 MB of them. Send fewer, or smaller ones.",
                    size_words(together)
                )));
            }
            let data = tokio::fs::read(&attachment.path).await.map_err(|_| refused(format!("Kumi couldn't read {name}; add it again.")))?;
            pictures.push(Picture { name: name.clone(), media_type: attachment.media_type.clone(), data });
        }
        named.push(format!("{name} ({}, {}) at {}", attachment.media_type, size_words(metadata.len()), attachment.path));
    }
    Ok((format!("{input}\n\n[The producer added: {}]", named.join("; ")), pictures))
}

pub fn create_session(options: SessionOptions) -> Result<Session, RuntimeError> {
    // A session always knows its DAW (Live's by default): only an integration that doesn't say has no profile.
    debug_assert!(options.profile != DawProfile::UNKNOWN, "a session runs in a known DAW's words");
    let timeout_ms = options.timeout_ms.unwrap_or(120_000);
    let idle_ms = options.idle_timeout_ms.or(options.timeout_ms).unwrap_or(600_000);
    let turn_limit_ms = options.turn_limit_ms.unwrap_or(3_600_000);
    let close_ms = options.close_timeout_ms.unwrap_or(5_000);
    let grace_ms = options.cancel_grace_ms.unwrap_or(500);
    let timings = options.timings.clone();
    let options_resume = options.resume.clone();
    if [timeout_ms, idle_ms, turn_limit_ms, close_ms, grace_ms, options.max_turns.unwrap_or(1) as u64]
        .iter()
        .any(|n| *n == 0 || *n > 9_007_199_254_740_991)
    {
        return Err(RuntimeError::plain("Invalid session bound"));
    }
    Ok(Session(Rc::new_cyclic(|weak: &std::rc::Weak<Inner>| {
        let emit: Rc<dyn Fn(SessionEvent)> = {
            let weak = weak.clone();
            Rc::new(move |event| {
                if let Some(inner) = weak.upgrade() {
                    Session(inner).emit(event);
                }
            })
        };
        let notes = options.memory.as_ref().map(|store| {
            let project = weak.clone();
            let set = weak.clone();
            let emit = emit.clone();
            memory_tools_for(
                options.profile,
                MemoryToolsOptions {
                    store: store.clone(),
                    project: Rc::new(move || project.upgrade().and_then(|i| i.state.borrow().project.clone())),
                    set: Some(Rc::new(move || set.upgrade().and_then(|i| i.state.borrow().set.clone()))),
                    on_event: Rc::new(move |e| emit(e.into())),
                },
            )
        });
        let recipes = options
            .recipes
            .as_ref()
            .map(|store| {
                let weak = weak.clone();
                let emit = emit.clone();
                recipe_tools_for(
                    options.profile,
                    RecipeToolsOptions {
                        store: store.clone(),
                        plan: Rc::new(move || weak.upgrade().and_then(|i| i.state.borrow().plan.clone())),
                        on_event: Rc::new(move |e| emit(SessionEvent::Recipe(e))),
                    },
                )
            })
            .unwrap_or_default();
        let recall = options.conversations.as_ref().map(|store| {
            let weak = weak.clone();
            recall_tool_for(
                options.profile,
                RecallOptions {
                    conversations: Some(store.clone()),
                    techniques: options.techniques.clone(),
                    recipes: options.recipes.clone(),
                    current: Rc::new(move || {
                        let inner = weak.upgrade()?;
                        let s = inner.state.borrow();
                        Some((s.place.clone()?, s.conversation_id.clone()))
                    }),
                },
            )
        });
        let learned = options.techniques.as_ref().map(|store| {
            let emit = emit.clone();
            Rc::new(technique_tools_for(
                options.profile,
                TechniqueToolsOptions { store: store.clone(), on_event: Rc::new(move |e| emit(SessionEvent::Technique(e))) },
            ))
        });
        let listening = if options.listen {
            let resolve = weak.clone();
            let hear = weak.clone();
            let emit = emit.clone();
            listening_tools_for(
                options.profile,
                ListeningOptions {
                    on_event: Rc::new(move |e| emit(SessionEvent::Heard(e))),
                    resolve: Some(Rc::new(move |named, signal| {
                        let integration = resolve.upgrade().and_then(|i| i.state.borrow().integration.clone());
                        async move {
                            match integration {
                                Some(i) if i.has_audio_file() => i.audio_file(&named, signal).await,
                                _ => Ok(None),
                            }
                        }
                        .boxed_local()
                    })),
                    hear: Some(Rc::new(move |request, signal| {
                        let profile = options.profile;
                        let integration = hear.upgrade().and_then(|i| i.state.borrow().integration.clone());
                        async move {
                            match integration {
                                Some(i) if i.has_hear() => i.hear(&request, signal).await,
                                _ => Ok(Err(format!(
                                    "Kumi isn't connected to {}, so it can't hear the {}.",
                                    profile.words.app, profile.words.place
                                ))),
                            }
                        }
                        .boxed_local()
                    })),
                },
            )
        } else {
            vec![]
        };
        // References are measured from audio fetched with the same programs videos use.
        let listening = match (&options.references, options.listen) {
            (Some(store), true) => {
                let mut listening = listening;
                let programs = kumi_media::programs::ProgramOptions {
                    tools_dir: options.watch.as_ref().map(|dirs| dirs.tools_dir.clone()).unwrap_or_default(),
                    ..Default::default()
                };
                let resolve = weak.clone();
                let clips: kumi_media::audio::tools::ResolveAudio = Rc::new(move |named, signal| {
                    let integration = resolve.upgrade().and_then(|i| i.state.borrow().integration.clone());
                    async move {
                        match integration {
                            Some(i) if i.has_audio_file() => i.audio_file(&named, signal).await,
                            _ => Ok(None),
                        }
                    }
                    .boxed_local()
                });
                listening.extend(kumi_listening::references::tool::reference_tools_for(
                    options.profile,
                    store.clone(),
                    programs,
                    Some(clips),
                ));
                listening
            }
            _ => listening,
        };
        // The producer's own music, for a request that leaves the style open: in every DAW, away from it.
        let music_tools = options
            .music
            .as_ref()
            .map(|music| {
                let resolve = weak.clone();
                let clips: kumi_media::audio::tools::ResolveAudio = Rc::new(move |named, signal| {
                    let integration = resolve.upgrade().and_then(|i| i.state.borrow().integration.clone());
                    async move {
                        match integration {
                            Some(i) if i.has_audio_file() => i.audio_file(&named, signal).await,
                            _ => Ok(None),
                        }
                    }
                    .boxed_local()
                });
                kumi_listening::music::tool::taste_tools_for(&options.profile, music.clone(), Some(clips), options.references.clone())
            })
            .unwrap_or_default();
        let answers = Rc::new(Cell::new(0));
        let watching = options
            .watch
            .as_ref()
            .map(|dirs| {
                let answers = answers.clone();
                kumi_web::video::tool::video_tools_for(
                    &options.profile,
                    kumi_web::video::tool::VideoToolOptions {
                        videos_dir: dirs.videos_dir.clone(),
                        tools_dir: dirs.tools_dir.clone(),
                        env: None,
                        on_event: tool_events(&emit),
                        answer: Some(Rc::new(move || answers.get())),
                    },
                )
            })
            .unwrap_or_default();
        let browsing = if options.web || options.web_client.is_some() {
            web_tools_for(
                &options.profile,
                WebToolOptions { on_event: tool_events(&emit), client: options.web_client.clone(), ..Default::default() },
            )
        } else {
            vec![]
        };
        let gaps = options.gaps.as_ref().map(|file| gap_tools_for(options.profile, file, options.store.clone())).unwrap_or_default();
        let search_memory = options.store.clone().map(|store| SearchMemory::new(store, options.profile.id.unwrap_or(DawId::Ableton)));
        let taste = options.store.clone().map(|store| {
            let place = weak.clone();
            let hear = weak.clone();
            let integration = |inner: &Inner| inner.state.borrow().integration.clone();
            TasteLog::new(
                store,
                Rc::new(move || {
                    let Some(inner) = place.upgrade() else { return Whereabouts::default() };
                    let (session, project) = {
                        let s = inner.state.borrow();
                        (s.conversation_id.clone(), s.project.clone())
                    };
                    let daw = inner.options.profile.id.filter(|daw| *daw != DawId::Ableton);
                    Whereabouts { session, project, set: integration(&inner).and_then(|i| i.fingerprint()), daw }
                }),
                Rc::new(move |since| hear.upgrade().and_then(|inner| integration(&inner)).and_then(|i| i.first_heard(since))),
            )
        });
        let shelf = options
            .library
            .as_ref()
            .map(|library| {
                let resolve = weak.clone();
                library.tools_for(
                    &options.profile,
                    LibraryToolsOptions {
                        on_event: Some(tool_events(&emit)),
                        resolve: Some(Rc::new(move |named, signal| {
                            let integration = resolve.upgrade().and_then(|i| i.state.borrow().integration.clone());
                            async move {
                                match integration {
                                    Some(i) if i.has_audio_file() => i.audio_file(&named, signal).await,
                                    _ => Ok(None),
                                }
                            }
                            .boxed_local()
                        })),
                    },
                )
            })
            .unwrap_or_default();
        let jobs = options.jobs.as_ref().map(|jobs| {
            let registry = crate::core::jobs::Jobs::new(jobs.max, emit.clone());
            let weak = weak.clone();
            registry.on_end(Rc::new(move |id| {
                if let Some(inner) = weak.upgrade() {
                    Session(inner).job_ended(id);
                }
            }));
            registry
        });
        let palettes = options.store.clone().map(|store| {
            let emit = emit.clone();
            Rc::new(crate::core::palette::Palettes::new(
                store,
                options.library.clone().map(|library| {
                    Rc::new(crate::core::palette::LibrarySounds(library, options.profile.id))
                        as Rc<dyn crate::core::palette::PaletteLibrary>
                }),
                Rc::new(move |palette| emit(SessionEvent::Palette { palette })),
            ))
        });
        let unlisten_music = options.music.as_ref().map(|music| {
            let emit = emit.clone();
            music.on_status(Rc::new(move |status| emit(SessionEvent::Music(MusicEvent { status }))))
        });
        let unlisten_library = options.library.as_ref().map(|library| {
            // A learn finished: a loose palette brings in what it learned too, and the app's map marks it.
            let (palettes, learned_at) = (palettes.as_ref().map(Rc::downgrade), Cell::new(library.status().learned_at));
            library.on_status(Rc::new(move |status| {
                if status.learned_at.is_some() && learned_at.replace(status.learned_at) != status.learned_at {
                    if let Some(palettes) = palettes.as_ref().and_then(Weak::upgrade) {
                        tokio::task::spawn_local(async move { palettes.learned().await });
                    }
                }
                emit(SessionEvent::Library(LibraryEvent { status }))
            }))
        });
        Inner {
            plugins: crate::core::plugin_tools::PluginTool::new(options.preset_dir.clone()),
            options,
            timeout_ms,
            idle_ms,
            turn_limit_ms,
            close_ms,
            grace_ms,
            notes,
            recipes,
            recall,
            learned,
            listening,
            watching,
            browsing,
            gaps,
            taste,
            search_memory,
            palette_tool: palettes.clone().map(crate::core::palette_tool::PaletteTool::new),
            palettes,
            timings,
            shelf,
            music_tools,
            jobs,
            answers,
            unlisten_library: RefCell::new(unlisten_library),
            unlisten_music: RefCell::new(unlisten_music),
            saving: RefCell::new(ready()),
            playbook_queue: RefCell::new(ready()),
            state: RefCell::new(State {
                matching: None,
                last_run: None,
                last_lesson: None,
                heard_last: None,
                goal_state: None,
                goal_status: None,
                goal_op: None,
                goal_stopped: false,
                goal_reference: None,
                judged_last: None,
                judged_rounds: 0,
                looping: None,
                judge_start: None,
                judged_closed: 0.,
                judged_changes: 0,
                turn_request: None,
                objective: None,
                objective_op: None,
                objective_stopped: false,
                objective_until: None,
                checking: false,
                steers: 0,
                turn_steers: 0,
                pending_goal: None,
                applied: 0,
                state: TurnState::Idle,
                connection: ConnectionState::Disconnected,
                observation: None,
                integration: None,
                generation: 0,
                kernel: None,
                must_reset: false,
                rebuild: false,
                system: None,
                system_stale: false,
                switched: None,
                away: false,
                missing: None,
                interrupted: None,
                away_cause: None,
                closed_mid_request: false,
                place: None,
                conversation_id: new_conversation_id(now()),
                conversation_first: None,
                conversation_turns: 0,
                conversation_name: None,
                copied_from: None,
                touched: false,
                as_kept: false,
                wanted: options_resume.map(|(place, id)| Wanted { place, id, from: None, asked: None }),
                own_set: false,
                set_open: false,
                next_note: None,
                model_said: Default::default(),
                changes: vec![],
                seen: indexmap::IndexMap::new(),
                settled: None,
                carry: None,
                start_fresh: false,
                chosen: None,
                project: None,
                set: None,
                set_identity: None,
                set_name: None,
                plan: None,
                turns: 0,
                next_operation: 0,
                active: None,
                closing: None,
                started: false,
                foreground: vec![],
                carrying: Default::default(),
                jobs_place: None,
                noted: None,
                referable: None,
            }),
        }
    })))
}

impl Session {
    /// Where the session's DAW keeps conversations about a project not saved yet: "unsaved" for Live's.
    fn unsaved(&self) -> &'static str {
        kumi_types::place::unsaved(self.0.options.profile.id.unwrap_or(DawId::Ableton))
    }
    fn emit(&self, event: SessionEvent) {
        // A step moved to the background (ctrl-b): what its tools say from then on is its job's. Only what tools say: the
        // job's own status, which their words change, goes to the producer.
        if matches!(event, SessionEvent::Doing { .. } | SessionEvent::Web(_) | SessionEvent::Watched(_)) {
            if let Some(job) = crate::core::jobs::route() {
                return job.on_event()(event);
            }
        }
        if self.0.state.borrow().state != TurnState::Closed {
            (self.0.options.on_event)(event);
        }
    }
    fn set_state(&self, state: TurnState) {
        if self.0.state.borrow().state != TurnState::Closed {
            self.0.state.borrow_mut().state = state;
            (self.0.options.on_event)(SessionEvent::State { state });
        }
    }
    fn notice(&self, message: impl Into<String>) {
        self.emit(SessionEvent::Notice { message: message.into() });
    }
    /// Bring in, beside the turn just begun, what an older Kumi changed in its files (`FileSync`), and say
    /// so when anything came in. The turn never waits for it.
    fn look_at_files(&self) {
        let Some(files) = self.0.options.files.clone() else { return };
        let (this, turn) = (self.clone(), timing::current());
        tokio::task::spawn_local(async move {
            let began = Instant::now();
            let looked = files.look().await;
            if let Some(turn) = turn {
                turn.files(began.elapsed().as_millis() as u64);
            }
            for message in looked.ok().flatten().map(|imported| imported.sentences()).unwrap_or_default() {
                this.notice(message);
            }
        });
    }
    fn error(&self, message: impl Into<String>) {
        self.emit(SessionEvent::Error { message: message.into(), kind: None, provider: None });
    }
    fn current(&self, op: &Operation) -> bool {
        let s = self.0.state.borrow();
        s.state != TurnState::Closed && s.active.as_ref().is_some_and(|a| a.id == op.id) && !op.signal.is_cancelled()
    }
    fn assert_current(&self, op: &Operation) -> Result<(), RuntimeError> {
        if self.current(op) {
            Ok(())
        } else {
            Err(RuntimeError::plain("Operation cancelled"))
        }
    }
    fn enqueue(&self, work: impl Future<Output = Result<(), RuntimeError>> + 'static) {
        let previous = self.0.saving.borrow().clone();
        let (tx, rx) = oneshot::channel();
        *self.0.saving.borrow_mut() = async { rx.await.unwrap_or(Ok(())) }.boxed_local().shared();
        tokio::task::spawn_local(async move {
            let _ = previous.await;
            let _ = work.await;
            let _ = tx.send(Ok(()));
        });
    }
    async fn bounded<T: 'static>(&self, work: impl Future<Output = Result<T, RuntimeError>> + 'static) -> Result<T, RuntimeError> {
        let task = tokio::task::spawn_local(work);
        tokio::time::timeout(Duration::from_millis(self.0.close_ms), task)
            .await
            .map_err(|_| RuntimeError::plain("Cleanup timed out"))?
            .map_err(|e| RuntimeError::plain(e.to_string()))?
    }
    async fn drop_kernel(&self) -> Result<(), RuntimeError> {
        let held = self.0.state.borrow_mut().kernel.take();
        if let Some(held) = held {
            held.lifetime.cancel();
            self.bounded(async move {
                held.value.close().await;
                Ok(())
            })
            .await?;
        }
        Ok(())
    }
    async fn drop_resources(&self) -> Result<(), RuntimeError> {
        let previous = {
            let mut s = self.0.state.borrow_mut();
            s.generation += 1;
            s.observation = None;
            s.connection = ConnectionState::Disconnected;
            s.integration.take()
        };
        self.emit(SessionEvent::Connection { state: ConnectionState::Disconnected });
        let close = async {
            if let Some(i) = previous {
                self.bounded(async move { i.close().await }).await
            } else {
                Ok(())
            }
        };
        let (kernel, integration) = futures::join!(self.drop_kernel(), close);
        if kernel.is_err() || integration.is_err() {
            Err(RuntimeError::plain("Owned resource cleanup did not complete"))
        } else {
            Ok(())
        }
    }
    fn connection_changed(&self, generation: u64, next: ConnectionState, cause: Option<DisconnectCause>) {
        let (lost, back, unstarted) = {
            let mut s = self.0.state.borrow_mut();
            if s.generation != generation || s.state == TurnState::Closed {
                return;
            }
            let lost = s.connection == ConnectionState::Connected && matches!(next, ConnectionState::Disconnected | ConnectionState::Error);
            let back = s.away && next == ConnectionState::Connected;
            // The first start stopped before it finished (Live lost partway, or its wait ran out before Live was
            // there): it finishes now that Live is, as /reconnect would.
            let unstarted = next == ConnectionState::Connected && !s.started && s.active.is_none();
            s.connection = next;
            (lost, back, unstarted)
        };
        self.emit(SessionEvent::Connection { state: next });
        if lost {
            let (active, missing, working) = {
                let mut s = self.0.state.borrow_mut();
                s.observation = None;
                s.away = true;
                s.away_cause = cause;
                // Live closed has no Set open, whatever the last look saw: a conversation asked for now goes on.
                if cause == Some(DisconnectCause::Live) {
                    s.set_open = false;
                }
                let working = s.active.as_ref().is_some_and(|op| op.is_turn);
                // A request about the Set Live left isn't offered again in the next one, nor one that carries on
                // through a busy Live.
                if !matches!(cause, Some(DisconnectCause::Set | DisconnectCause::AskedSet | DisconnectCause::Busy)) {
                    if let Some(input) = s.active.as_ref().filter(|op| op.is_turn).and_then(|op| op.input.clone()).filter(|s| !s.is_empty())
                    {
                        s.interrupted = Some(input);
                    }
                }
                s.closed_mid_request = working && cause == Some(DisconnectCause::Live);
                if let Some(old) = s.missing.take() {
                    old.cancel();
                }
                let signal = Signal::new();
                s.missing = Some(signal.clone());
                (s.active.clone(), signal, working)
            };
            let DawProfile { words: DawWords { app, place, .. }, vocab, .. } = self.0.options.profile;
            self.notice(&match cause {
                Some(DisconnectCause::Live) if working => {
                    format!("{app} closed while Kumi was working in it. {} Kumi picks up when {app} is back.", vocab.crash_note)
                }
                Some(DisconnectCause::Live) => format!("{app} closed. Kumi will pick up where you left off when it's back."),
                Some(DisconnectCause::Bridge) => format!("Kumi's link to {app} dropped. It's reconnecting, and will pick up where you left off."),
                Some(DisconnectCause::AskedSet) => format!("{app} is opening the {place}; Kumi carries on once it's open."),
                Some(DisconnectCause::Set) if working => format!(
                    "{app} is opening another {place}, so Kumi stopped what it was doing: nothing it was doing lands in the other {place}. It picks up once the {place} is open."
                ),
                Some(DisconnectCause::Set) => format!("{app} is opening another {place}; Kumi picks up once it's open."),
                Some(DisconnectCause::Busy) if working => {
                    format!("{app} is busy and answering slowly, so Kumi is waiting for it: your request carries on once it answers.")
                }
                Some(DisconnectCause::Busy) => format!("{app} is busy and answering slowly; Kumi carries on once it answers."),
                None => format!("Kumi lost touch with {app}. It will pick up where you left off when {app} is back."),
            });
            // The request that asked for another Set carries on in it (#188), and so does one Live is only slow for
            // (#256).
            if let Some(op) = active.filter(|_| !matches!(cause, Some(DisconnectCause::AskedSet | DisconnectCause::Busy))) {
                op.signal.cancel();
            }
            // A big Set takes a while to open.
            let opening = matches!(cause, Some(DisconnectCause::Set | DisconnectCause::AskedSet));
            let after = self.0.options.missing_after_ms.unwrap_or(30_000) * if opening { 4 } else { 1 };
            let this = self.clone();
            tokio::task::spawn_local(async move {
                tokio::select! { _ = missing.cancelled() => {}, _ = tokio::time::sleep(Duration::from_millis(after)) => {
                    if this.0.state.borrow().away { this.notice(this.0.options.profile.vocab.cant_reach); }
                } }
            });
        } else if back {
            let (again, refresh, away, crashed, active) = {
                let mut s = self.0.state.borrow_mut();
                s.away = false;
                if let Some(m) = s.missing.take() {
                    m.cancel();
                }
                (
                    s.interrupted.take(),
                    s.active.is_none() && s.started,
                    s.away_cause.take(),
                    std::mem::take(&mut s.closed_mid_request),
                    s.active.clone(),
                )
            };
            // Taken for busy, Live came back with another Set: the request that waited stops, as for a Set opened
            // by hand, since nothing it does belongs in this one (#188, #256).
            let cause = cause.or(away);
            if away == Some(DisconnectCause::Busy) && cause == Some(DisconnectCause::Set) {
                if let Some(op) = active.filter(|op| op.is_turn) {
                    op.signal.cancel();
                    let DawWords { app, place, .. } = self.0.options.profile.words;
                    self.notice(&format!(
                        "{app} opened another {place} while it was busy, so Kumi stopped what it was doing: nothing it was doing lands in the other {place}."
                    ));
                }
            }
            let DawWords { app, place, .. } = self.0.options.profile.words;
            self.notice(&match (again.is_some(), cause) {
                (_, Some(DisconnectCause::Set | DisconnectCause::AskedSet)) => format!("{app} has the other {place} open."),
                (_, Some(DisconnectCause::Busy)) => format!("{app} is answering again."),
                (true, _) if crashed => format!(
                    "{app} is back. Your last request stopped when {app} closed, which it may have caused: check the {place} before you send it again (enter sends it)."
                ),
                (true, _) => format!("{app} is back. Your last request was stopped; press enter to send it again."),
                (false, _) => format!("{app} is back."),
            });
            if let Some(text) = again {
                self.emit(SessionEvent::Resend { text });
            }
            if refresh {
                let _ = self.perform(
                    false,
                    Phase::Refresh,
                    Box::new(|this, op| {
                        async move {
                            this.observe(&op, None, false).await?;
                            Ok(None)
                        }
                        .boxed_local()
                    }),
                    None,
                    None,
                );
            }
        }
        if unstarted {
            let _ = self.perform(
                false,
                Phase::Start,
                Box::new(|this, op| {
                    async move {
                        this.reset(&op).await?;
                        Ok(None)
                    }
                    .boxed_local()
                }),
                None,
                None,
            );
        }
    }
    /// The conversation going on from here: `id` in `place`, as it was kept when it's taken up, and `from`, the one it
    /// copies when another Kumi window has that one open.
    fn begin(&self, place: String, id: String, conversation: Option<&SavedConversation>, from: Option<&str>) {
        // This Kumi holds the conversation it's in, and lets go of the one it leaves.
        if let Some(store) = &self.0.options.conversations {
            let before = {
                let s = self.0.state.borrow();
                s.place.clone().map(|before| (before, s.conversation_id.clone()))
            };
            if before.as_ref() != Some(&(place.clone(), id.clone())) {
                if let Some((before, was)) = before {
                    store.release(&before, &was);
                }
                store.claim(&place, &id);
            }
        }
        // Its log is written from here: a kept one's goes on, and one it has no log for begins with what it had.
        if let Some(log) = &self.0.options.log {
            log.open(&place, &id, conversation, from.map(str::to_owned));
        }
        let mut s = self.0.state.borrow_mut();
        if s.conversation_id != id {
            s.conversation_name = None;
            s.copied_from = None;
            s.touched = false;
            s.next_note = None;
        }
        s.place = Some(place);
        s.conversation_id = id.clone();
        s.settled = None;
        s.as_kept = false;
        s.conversation_first = conversation.and_then(|c| c.first.clone());
        s.conversation_turns = conversation.and_then(|c| c.turns).unwrap_or(0);
        s.changes = conversation
            .and_then(|c| c.changes.clone())
            .unwrap_or_default()
            .into_iter()
            .map(|change| taken_up(&id, &s.seen, change))
            .collect();
    }
    /// What a kept conversation taken up shows again of its log: its last requests as they happened, each change in them
    /// as HISTORY holds it now. None without a log as it was opened (one behind what's kept, which an earlier Kumi carried
    /// on since, began again from it), or with nothing in it to show: then the screen shows the conversation's words, as
    /// before logs.
    async fn replay(&self) -> Option<Replay> {
        let log = self.0.options.log.as_ref()?.as_opened().await?;
        // Read off the thread that takes keys: a long log is megabytes, mostly lines a replay never reads.
        let mut replay = tokio::task::spawn_blocking(move || replay_of(&log, REPLAYED)).await.ok()??;
        if replay.earlier.is_empty() && !replay.lines.iter().any(|line| matches!(line, LogLine::Request(_))) {
            return None;
        }
        let s = self.0.state.borrow();
        for line in &mut replay.lines {
            if let LogLine::Event { event: SessionEvent::Change { change }, .. } = line {
                *change = taken_up(&s.conversation_id, &s.seen, change.clone());
            }
        }
        Some(replay)
    }
    /// An id for a new conversation: none kept has it, in any Set.
    async fn fresh_id(&self) -> String {
        match &self.0.options.conversations {
            Some(store) => store.new_id().await,
            None => new_conversation_id(now()),
        }
    }
    /// This Kumi takes up a kept conversation, or, when another Kumi that's still open has it, a copy of it to carry on
    /// in (each window keeps its own turns then): the id to go on with, and the one it was copied from.
    async fn take_up(&self, place: &str, id: &str) -> (String, Option<String>) {
        let Some(store) = self.0.options.conversations.clone() else { return (id.to_owned(), None) };
        let ours = {
            let s = self.0.state.borrow();
            s.place.as_deref() == Some(place) && s.conversation_id == id
        };
        if ours || store.claim(place, id) == Claim::Ours {
            return (id.to_owned(), None);
        }
        (store.new_id().await, Some(id.to_owned()))
    }
    /// Begin a kept conversation taken up by `take_up`. A copy says so once it's begun, so its log has it.
    fn begin_taken(&self, place: String, (id, from): (String, Option<String>), conversation: &SavedConversation) {
        self.begin(place, id.clone(), Some(conversation), from.as_deref());
        if from.is_some() {
            self.0.state.borrow_mut().copied_from = from;
            self.notice(format!(
                "Another Kumi window has this conversation open, so this one carries on in a copy ({id}); each keeps its own turns."
            ));
        }
    }
    /// The open Set's own conversation, as Kumi opens on it: its place's current one (or a copy, when another Kumi has
    /// it), or a new one. The kept one, when it goes on.
    async fn own_conversation(&self, op: &Operation, project: Option<&String>) -> Result<Option<SavedConversation>, RuntimeError> {
        let kept = if let (Some(project), Some(store)) = (project, &self.0.options.conversations) {
            store.current(project).await.ok().flatten()
        } else {
            None
        };
        self.assert_current(op)?;
        let (Some(kept), Some(project)) = (kept, project) else {
            let id = self.fresh_id().await;
            self.begin(project.cloned().unwrap_or(self.unsaved().into()), id, None, None);
            return Ok(None);
        };
        let begin = {
            let s = self.0.state.borrow();
            kept.id != s.conversation_id || s.place.as_ref() != Some(project)
        };
        if begin {
            let taken = self.take_up(project, &kept.id).await;
            self.begin_taken(project.clone(), taken, &kept.conversation);
        }
        Ok(Some(kept.conversation))
    }
    /// What Kumi says of a conversation kept in another DAW's place: whose it is, and how to carry it on there. None for
    /// this DAW's (or an imported one).
    fn elsewhere(&self, place: &str) -> Option<String> {
        let daw = self.0.options.profile.id.unwrap_or(DawId::Ableton);
        if kept_for(place, daw) {
            return None;
        }
        let other = Place::parse(place).and_then(|place| place.daw()).and_then(self.0.options.registered)?;
        let (app, noun) = other.word_pair();
        Some(format!(
            "That conversation is about {} {app} {noun}: open Kumi with {app} (/daw {}) to carry it on there.",
            if app.starts_with(['A', 'E', 'I', 'O', 'U']) { "an" } else { "a" },
            app.to_lowercase()
        ))
    }
    /// Rows as the app shows them: `current` is the conversation going on now (a store marks each place's own latest).
    fn mark_going(&self, rows: Vec<ConversationSummary>) -> Vec<ConversationSummary> {
        let (place, id) = {
            let s = self.0.state.borrow();
            (s.place.clone(), s.conversation_id.clone())
        };
        rows.into_iter().map(|row| ConversationSummary { current: place.as_ref() == Some(&row.place) && row.id == id, ..row }).collect()
    }
    /// A conversation from a file carries on here in a copy (the original stays where `kumi import` put it). False when
    /// it isn't kept any more.
    async fn carry_imported(&self, store: Rc<dyn ConversationStore>, id: &str) -> Result<bool, RuntimeError> {
        let here = self.0.state.borrow().project.clone().unwrap_or(self.unsaved().into());
        let Some(copy) = self.copy_imported(&store, id, &here).await? else { return Ok(false) };
        let said = format!("Carrying on a copy of an imported conversation here; the original stays as {id}.");
        self.go_to(copy, Some(said)).await?;
        Ok(true)
    }
    /// A copy of a conversation from a file to carry on in `place`: a new id, its words without what it changed
    /// elsewhere, and the file's conversation and name. Like a copy taken up while another window has the original, it's
    /// kept by its first request (marked as from a file, as every copy of one is), so taking it up and leaving keeps
    /// nothing. None when the file's conversation isn't kept.
    async fn copy_imported(&self, store: &Rc<dyn ConversationStore>, id: &str, place: &str) -> Result<Option<Chosen>, RuntimeError> {
        let Some(kept) = store.load(IMPORTED, id).await? else { return Ok(None) };
        let name = match store.find(id).await? {
            ConversationMatch::One(row) if row.id == id => row.name,
            _ => None,
        };
        let copy = SavedConversation { saved_at: now(), checkpoint: kept.checkpoint, changes: None, first: kept.first, turns: kept.turns };
        Ok(Some(Chosen { place: place.to_owned(), id: store.new_id().await, conversation: copy, copy_of: Some((id.to_owned(), name)) }))
    }
    /// A conversation from a file asked for as Kumi starts (kumi -r, kumi -c) carries on in a copy, never in the file's
    /// own place, and Kumi asks nothing (it has no Set to open): the copy goes on as a Set not saved yet's, and with the
    /// Set that's open once Live is read.
    async fn copy_wanted_import(&self) -> Result<(), RuntimeError> {
        let wanted = self.0.state.borrow().wanted.clone();
        let Some(wanted) = wanted.filter(|wanted| wanted.place == IMPORTED) else { return Ok(()) };
        let Some(store) = self.0.options.conversations.clone() else { return Ok(()) };
        let copy = self.copy_imported(&store, &wanted.id, self.unsaved()).await?;
        let found = copy.is_some();
        {
            let mut s = self.0.state.borrow_mut();
            s.wanted = None;
            s.chosen = copy;
        }
        if found {
            self.notice(format!("Carrying on a copy of an imported conversation; the original stays as {}.", wanted.id));
        }
        Ok(())
    }
    /// Whether the conversation going on came from a file (kumi import), or is a copy of one (kept or not yet).
    async fn conversation_imported(&self) -> bool {
        let Some(store) = self.0.options.conversations.clone() else { return false };
        // A copy's first save marks it as from a file: what's being saved is kept first.
        let saving = self.0.saving.borrow().clone();
        let _ = saving.await;
        let (place, id, from) = {
            let s = self.0.state.borrow();
            (s.place.clone(), s.conversation_id.clone(), s.copied_from.clone())
        };
        if place.as_deref() == Some(IMPORTED) {
            return true;
        }
        for id in std::iter::once(&id).chain(from.as_ref()) {
            if matches!(store.find(id).await, Ok(ConversationMatch::One(row)) if row.id == *id && row.imported) {
                return true;
            }
        }
        false
    }
    /// Go on with a kept conversation the producer chose: Kumi's next answer starts from it.
    async fn go_to(&self, chosen: Chosen, said: Option<String>) -> Result<(), RuntimeError> {
        self.perform(
            false,
            Phase::Refresh,
            Box::new(move |this, op| {
                async move {
                    this.drop_kernel().await?;
                    this.assert_current(&op)?;
                    if let Some(l) = &this.0.learned {
                        l.drafts.reset();
                    }
                    {
                        let mut s = this.0.state.borrow_mut();
                        s.chosen = Some(chosen);
                        s.must_reset = false;
                        s.turns = 0;
                        s.interrupted = None;
                    }
                    this.observe(&op, None, false).await?;
                    this.hold_taken_up();
                    if let Some(said) = said {
                        this.notice(said);
                    }
                    Ok(None)
                }
                .boxed_local()
            }),
            None,
            None,
        )?
        .await
    }
    /// What the producer calls the DAW and a place in it, as the integration says ("Live", "Set").
    fn words(&self) -> (&'static str, &'static str) {
        match &self.0.state.borrow().integration {
            Some(integration) => integration.words(),
            None => DAW_WORDS,
        }
    }
    /// The conversation the producer chose goes on as `going` (a copy of it, or a fork): Kumi asks about that one, steps
    /// aside from it for another Set, and comes back to it from here.
    fn wanted_goes_on_as(&self, place: &str, id: &str, going: &str) {
        let mut s = self.0.state.borrow_mut();
        if let Some(w) = s.wanted.as_mut().filter(|w| w.place == place && w.id == id && id != going) {
            w.from.get_or_insert_with(|| id.to_owned());
            w.id = going.to_owned();
        }
    }
    /// The conversation the producer chose, as kept: its copy's, else (a copy with nothing kept yet) the one copied.
    async fn kept_wanted(&self, wanted: &Wanted) -> Result<Option<(String, SavedConversation)>, RuntimeError> {
        let Some(store) = self.0.options.conversations.clone() else { return Ok(None) };
        for id in std::iter::once(&wanted.id).chain(wanted.from.as_ref()) {
            if let Some(kept) = store.load(&wanted.place, id).await? {
                return Ok(Some((id.clone(), kept)));
            }
        }
        Ok(None)
    }
    /// What the store lists for the conversation the producer chose (or the one it copies, before the copy is kept).
    async fn wanted_row(&self, wanted: &Wanted) -> Option<ConversationSummary> {
        let store = self.0.options.conversations.clone()?;
        for id in std::iter::once(&wanted.id).chain(wanted.from.as_ref()) {
            if let Ok(ConversationMatch::One(row)) = store.find(id).await {
                return Some(row);
            }
        }
        None
    }
    /// The conversation the producer chose is no longer waiting: a question asked about it goes away.
    fn drop_wanted(&self) -> Option<Wanted> {
        let wanted = self.0.state.borrow_mut().wanted.take();
        if wanted.as_ref().is_some_and(|w| w.asked.is_some()) {
            self.emit(SessionEvent::SetAsked { ask: None });
        }
        wanted
    }
    /// Ask about the conversation the producer chose, whose Set isn't the open one (`key`): once for each Set that opens.
    async fn ask_about_wanted(&self, key: &str, open: Option<String>) {
        let Some(wanted) = self.0.state.borrow().wanted.clone() else { return };
        if wanted.asked.as_deref() == Some(key) {
            return;
        }
        if let Some(w) = self.0.state.borrow_mut().wanted.as_mut() {
            w.asked = Some(key.to_owned());
        }
        let row = self.wanted_row(&wanted).await;
        let (daw, noun) = self.words();
        let by_hand = !self.0.state.borrow().integration.as_ref().is_some_and(|integration| integration.has_open_place());
        self.emit(SessionEvent::SetAsked {
            ask: Some(SetAsk {
                id: wanted.id.clone(),
                title: row.as_ref().map(|row| row.title()).unwrap_or_default(),
                set: row.and_then(|row| row.set),
                open,
                daw: daw.into(),
                noun: noun.into(),
                by_hand,
            }),
        });
    }
    async fn build_kernel(
        &self,
        op: &Operation,
        observation: &Observation,
        checkpoint: Option<KernelCheckpoint>,
    ) -> Result<(Rc<dyn Kernel>, Signal), RuntimeError> {
        let lifetime = Signal::new();
        let stop = Signal::new();
        let (abort, creation, stop_watch) = (op.signal.clone(), lifetime.clone(), stop.clone());
        tokio::task::spawn_local(async move {
            tokio::select! { biased; _ = abort.cancelled() => creation.cancel(), _ = stop_watch.cancelled() => {} }
        });
        let built = async {
            let conversation = self.0.state.borrow().conversation_id.clone();
            let integration = self.0.state.borrow().integration.clone();
            let addendum = integration
                .map(|integration| integration.instructions_addendum(&|name| observation.tools.iter().any(|tool| tool.name() == name)))
                .unwrap_or_default();
            // The same conversation keeps its system text across rebuilds, unless the producer changed what it holds.
            let kept = {
                let s = self.0.state.borrow();
                s.system
                    .as_ref()
                    .filter(|kept| {
                        !s.system_stale
                            && kept.conversation == conversation
                            && kept.base == observation.instructions
                            && kept.addendum == addendum
                    })
                    .map(|kept| kept.text.clone())
            };
            let instructions = match kept {
                Some(text) => text,
                None => {
                    let text = self.system_text(op, observation, &addendum).await?;
                    let mut s = self.0.state.borrow_mut();
                    s.system = Some(SystemText {
                        conversation: conversation.clone(),
                        base: observation.instructions.clone(),
                        addendum,
                        text: text.clone(),
                    });
                    s.system_stale = false;
                    text
                }
            };
            let mut tools: Vec<Rc<dyn KernelTool>> = observation
                .tools
                .iter()
                .filter(|tool| self.0.shelf.is_empty() || tool.role() != Some(ToolRole::FindSounds))
                .map(|tool| self.with_background(self.with_loop(self.with_taste(self.with_technique(tool.clone())))))
                .collect();
            if let Some(notes) = &self.0.notes {
                tools.extend(notes.tools.clone());
            }
            tools.extend(self.0.listening.clone());
            tools.extend(self.0.music_tools.iter().map(|tool| self.with_background(tool.clone())));
            tools.extend(self.0.watching.iter().map(|tool| self.with_background(tool.clone())));
            tools.extend(self.0.browsing.iter().map(|tool| self.with_background(tool.clone())));
            tools.extend(self.0.shelf.iter().map(|tool| self.with_background(tool.clone())));
            tools.extend(self.0.recipes.clone());
            tools.extend(self.0.palette_tool.clone());
            tools.push(self.with_background(self.0.plugins.clone()));
            tools.extend(self.0.recall.iter().map(|tool| self.with_background(tool.clone())));
            if let Some(learned) = &self.0.learned {
                tools.extend(learned.tools.clone());
            }
            tools.extend(self.0.gaps.clone());
            if let Some(taste) = &self.0.taste {
                tools.push(taste.tool());
            }
            tools.extend(self.job_tools());
            let value = (self.0.options.kernel_factory)(KernelOptions {
                instructions,
                tools,
                signal: lifetime.clone(),
                checkpoint,
                conversation: Some(conversation),
            })
            .await?;
            value.set_profile(self.0.options.profile);
            if !self.current(op) {
                lifetime.cancel();
                self.bounded(async move {
                    value.close().await;
                    Ok(())
                })
                .await?;
                return Err(RuntimeError::plain("Operation cancelled"));
            }
            Ok(value)
        }
        .await;
        stop.cancel();
        if built.is_err() {
            lifetime.cancel();
        }
        built.map(|value| (value, lifetime))
    }
    /// The system text a new conversation starts with: the integration's instructions, then what Kumi keeps (the
    /// producer's notes, their taste, recipes, techniques) and the guidance that goes with its tools.
    /// The producer changed what the system text holds: the next kernel reads it again (the model's own notes and
    /// recipes are in the conversation already).
    fn changed_system(&self, changed: bool) {
        if changed {
            self.0.state.borrow_mut().system_stale = true;
        }
    }
    async fn system_text(&self, op: &Operation, observation: &Observation, addendum: &str) -> Result<String, RuntimeError> {
        let memory = if let Some(store) = &self.0.options.memory {
            store.load(observation.project.as_ref().map(|p| p.id.as_str())).await.ok()
        } else {
            None
        };
        self.assert_current(op)?;
        let daw = self.0.options.profile.id.unwrap_or(DawId::Ableton);
        let recipes = if let Some(store) = &self.0.options.recipes { store.list().await.unwrap_or_default() } else { vec![] };
        // Only this DAW's recipes: another's steps are its own tools'.
        let recipes: Vec<_> = recipes.into_iter().filter(|recipe| recipe.daw() == daw).collect();
        self.assert_current(op)?;
        let techniques = if let Some(learned) = &self.0.learned { learned.list().await.unwrap_or_default() } else { vec![] };
        self.assert_current(op)?;
        let habits = if let Some(library) = &self.0.options.library {
            library.instructions_for(&self.0.options.profile).await.unwrap_or_default()
        } else {
            String::new()
        };
        self.assert_current(op)?;
        let imported = self.conversation_imported().await;
        self.assert_current(op)?;
        let noun = self.0.state.borrow().integration.as_ref().map(|i| i.words().1).unwrap_or(DAW_WORDS.1);
        // Their own music, once Kumi has heard some of it.
        let music = self.0.options.music.as_ref().map(|music| taste_line(music.status().heard())).unwrap_or_default();
        let extra = [
            memory
                .as_ref()
                .map(|m| memory_instructions_for(&self.0.options.profile, m, observation.project.as_ref().map(|p| p.name.as_str())))
                .unwrap_or_default(),
            habits,
            crate::core::plugin_tools::GUIDANCE.into(),
            music,
            recipe_instructions(&recipes),
            if self.0.learned.is_some() { technique_guidance(&self.0.options.profile) } else { String::new() },
            technique_instructions_for(&self.0.options.profile, &techniques),
            if self.0.gaps.is_empty() { String::new() } else { gap_guidance(&self.0.options.profile) },
            if self.0.jobs.is_none() { String::new() } else { crate::core::jobs::job_guidance(&self.0.options.profile) },
            addendum.into(),
            if imported { imported_guidance(noun) } else { String::new() },
        ]
        .into_iter()
        .filter(|s: &String| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
        Ok(if extra.is_empty() { observation.instructions.clone() } else { format!("{}\n\n{extra}", observation.instructions) })
    }
    async fn ensure_kernel(&self, op: &Operation, observation: &Observation) -> Result<(), RuntimeError> {
        self.assert_current(op)?;
        let rebuild = self.0.state.borrow().rebuild;
        if rebuild {
            let held = {
                let mut s = self.0.state.borrow_mut();
                s.rebuild = false;
                s.kernel.clone()
            };
            if let Some(h) = held.filter(|h| h.value.has_checkpoint()) {
                self.0.state.borrow_mut().switched = Some(Settled { checkpoint: Rc::new(h.value.checkpoint()?), key: h.key });
            }
            self.drop_kernel().await?;
            self.assert_current(op)?;
        }
        let held = self.0.state.borrow().kernel.clone();
        let must_reset = self.0.state.borrow().must_reset;
        let identity = held.as_ref().is_some_and(|h| h.key != observation.key);
        let revision = observation.revision.clone().unwrap_or_default();
        // The open Set's place, when the DAW was read: a saved Set's project, or an unsaved Set's. (Without the DAW, the
        // project is the last one seen.)
        let here = observation.set.as_ref().map(|_| observation.project.as_ref().map(|p| p.id.clone()).unwrap_or(self.unsaved().into()));
        // A conversation the producer chose went on while no Set was open, and another Set is open now (or the producer
        // left it): the open Set's own conversation goes on, and Kumi asks about the chosen one. Live coming back keeps the
        // look's key, so this isn't a Set change to it.
        let astray = {
            let mut s = self.0.state.borrow_mut();
            let left = std::mem::take(&mut s.own_set);
            left || s.wanted.as_ref().is_some_and(|w| {
                w.place != self.unsaved()
                    && s.conversation_id == w.id
                    && s.place.as_ref() == Some(&w.place)
                    && here.as_ref().is_some_and(|here| *here != w.place)
            })
        };
        let mut from = None;
        if let Some(h) = held.as_ref().filter(|h| !astray && !identity && !must_reset && h.revision != revision) {
            if h.value.has_checkpoint() {
                from = Some(h.value.checkpoint()?);
                self.drop_kernel().await?;
                self.assert_current(op)?;
            }
        }
        let mut reason = None;
        let first = self.0.state.borrow().kernel.is_none() && !must_reset;
        if self.0.state.borrow().kernel.is_some()
            && (astray || identity || must_reset || held.as_ref().is_some_and(|h| h.revision != revision))
        {
            self.drop_kernel().await?;
            self.assert_current(op)?;
            reason = Some(if identity || astray {
                "set"
            } else if must_reset {
                "cancelled"
            } else {
                "tools"
            });
            if identity {
                self.0.state.borrow_mut().turns = u32::from(op.is_turn);
            }
        } else if self.0.state.borrow().kernel.is_none() && must_reset {
            reason = Some("cancelled");
        }
        if self.0.state.borrow().kernel.is_none() {
            let project = observation.project.as_ref().map(|p| p.id.clone());
            let (fresh, chosen, switched, carry, settled, wanted) = {
                let mut s = self.0.state.borrow_mut();
                (s.start_fresh, s.chosen.take(), s.switched.clone(), s.carry.clone(), s.settled.clone(), s.wanted.clone())
            };
            let carry = carry.filter(|(_, place)| {
                !(project.is_some() && place.is_some() && place.as_deref() != Some(self.unsaved()) && place != &project)
            });
            // A conversation the producer chose goes on when its Set is open, or while none is (an unsaved Set's goes with
            // whichever is open).
            let wanted_here = wanted.clone().filter(|w| here.is_none() || here.as_ref() == Some(&w.place) || w.place == self.unsaved());
            let mut resumed = None;
            let mut picked = false;
            // Taken up here (at the start, or once its Set opened): its Set's current from the moment it goes on.
            let mut taken_up = false;
            if fresh {
                let id = self.fresh_id().await;
                self.begin(project.clone().unwrap_or(self.unsaved().into()), id, None, None);
            } else if let Some(chosen) = chosen {
                from = Some(chosen.conversation.checkpoint.clone());
                picked = true;
                let taken = self.take_up(&chosen.place, &chosen.id).await;
                self.wanted_goes_on_as(&chosen.place, &chosen.id, &taken.0);
                self.begin_taken(chosen.place, taken, &chosen.conversation);
                let mut s = self.0.state.borrow_mut();
                s.touched = true;
                // A copy not kept yet: its first request keeps it, with its name and the one it copies.
                if let Some((of, name)) = chosen.copy_of {
                    s.copied_from = Some(of);
                    s.conversation_name = name;
                    taken_up = true;
                }
                drop(s);
                resumed = Some(chosen.conversation);
            } else if astray {
                resumed = self.own_conversation(op, project.as_ref()).await?;
                from = resumed.as_ref().map(|kept| kept.checkpoint.clone());
            } else if from.is_some() {
            } else if let Some(switched) = switched.filter(|s| s.key == observation.key) {
                from = Some(KernelCheckpoint::clone(&switched.checkpoint));
            } else if let Some((checkpoint, _)) = carry {
                from = Some(checkpoint);
            } else if let Some(settled) = settled.filter(|s| reason == Some("cancelled") && s.key == observation.key) {
                from = Some(KernelCheckpoint::clone(&settled.checkpoint));
            } else if let Some(w) = wanted_here {
                let going = {
                    let s = self.0.state.borrow();
                    (s.conversation_id == w.id && s.place.as_ref() == Some(&w.place)).then(|| s.settled.clone())
                };
                match going {
                    // It went on while no Set was open: it carries on now that its own is.
                    Some(Some(settled)) => from = Some(KernelCheckpoint::clone(&settled.checkpoint)),
                    _ => {
                        let saving = self.0.saving.borrow().clone();
                        let _ = saving.await;
                        let kept = self.kept_wanted(&w).await.ok().flatten();
                        self.assert_current(op)?;
                        match kept {
                            // Already the one going on: nothing new to show.
                            Some((_, kept)) if going.is_some() => from = Some(kept.checkpoint),
                            Some((id, kept)) => {
                                from = Some(kept.checkpoint.clone());
                                picked = true;
                                taken_up = true;
                                let taken = self.take_up(&w.place, &id).await;
                                self.wanted_goes_on_as(&w.place, &w.id, &taken.0);
                                self.begin_taken(w.place.clone(), taken, &kept);
                                self.0.state.borrow_mut().touched = true;
                                resumed = Some(kept);
                            }
                            None => {
                                self.notice("That conversation isn't kept any more.");
                                self.drop_wanted();
                                resumed = self.own_conversation(op, project.as_ref()).await?;
                                from = resumed.as_ref().map(|kept| kept.checkpoint.clone());
                            }
                        }
                    }
                }
                // Its Set is open: it's no longer waiting.
                if here.is_some() {
                    self.drop_wanted();
                }
            } else {
                resumed = self.own_conversation(op, project.as_ref()).await?;
                from = resumed.as_ref().map(|kept| kept.checkpoint.clone());
            }
            {
                let mut s = self.0.state.borrow_mut();
                s.start_fresh = false;
                s.switched = None;
                s.carry = None;
            }
            let built = self.build_kernel(op, observation, from.clone()).await;
            let mut unreadable = None;
            let (value, lifetime) = match built {
                Ok(built) => built,
                Err(error) => {
                    if resumed.is_none() || error.kumi().is_some() || !self.current(op) {
                        return Err(error);
                    }
                    unreadable = resumed.clone();
                    self.begin(project.unwrap_or(self.unsaved().into()), new_conversation_id(now()), None, None);
                    self.build_kernel(op, observation, None).await?
                }
            };
            self.0.state.borrow_mut().kernel = Some(Held { value: value.clone(), key: observation.key.clone(), revision, lifetime });
            if taken_up && unreadable.is_none() {
                self.hold_taken_up();
            }
            if let Some(kept) = unreadable {
                self.emit(SessionEvent::Resumed {
                    saved_at: kept.saved_at,
                    lines: transcript_of(&kept.checkpoint.messages),
                    changes: None,
                    chosen: None,
                    unreadable: Some(true),
                    replay: None,
                });
                self.notice("Kumi couldn't continue that conversation with this model, so it's shown above and a fresh one starts here.");
            } else {
                match reason {
                    Some("set") => self.notice({
                        let place = self.0.options.profile.words.place;
                        if resumed.is_some() || from.is_some() {
                            format!(
                                "The open {place} changed; continuing your conversation about {}.",
                                observation.project.as_ref().map(|p| p.name.clone()).unwrap_or_else(|| format!("this {place}"))
                            )
                        } else {
                            format!("The open {place} changed; starting a fresh conversation.")
                        }
                    }),
                    Some("cancelled") => self.notice(if from.is_some() {
                        "Cancelled work was discarded; the conversation continues from before it."
                    } else {
                        "Cancelled work was discarded; starting a fresh conversation."
                    }),
                    Some("tools") => self.notice(if from.is_some() {
                        "Kumi's tools changed; the conversation continues."
                    } else {
                        "Kumi's tools changed; starting a fresh conversation."
                    }),
                    _ => {}
                }
                if let Some(kept) = resumed.filter(|_| first || reason == Some("set") || picked) {
                    let earlier = {
                        let s = self.0.state.borrow();
                        s.changes.iter().filter(|c| !s.seen.get(&c.id).is_some_and(|ours| ours.at == c.at)).cloned().collect::<Vec<_>>()
                    };
                    let replay = self.replay().await;
                    self.emit(SessionEvent::Resumed {
                        saved_at: kept.saved_at,
                        lines: value.transcript(),
                        changes: (!earlier.is_empty()).then_some(earlier),
                        chosen: picked.then_some(true),
                        unreadable: None,
                        replay,
                    });
                    self.other_model(&kept, value.as_ref());
                }
            }
            // The conversation the producer chose went on in its own Set, now open: nothing's waiting. Another Set is open:
            // ask what to do.
            let done = {
                let s = self.0.state.borrow();
                s.wanted.as_ref().is_some_and(|w| here.as_ref() == Some(&w.place) && s.conversation_id == w.id)
            };
            if done {
                self.drop_wanted();
            }
            let elsewhere = self.0.state.borrow().wanted.as_ref().is_some_and(|w| here.as_ref().is_some_and(|here| *here != w.place));
            if elsewhere {
                self.ask_about_wanted(&observation.key, observation.project.as_ref().map(|p| p.name.clone())).await;
            }
        }
        self.0.state.borrow_mut().must_reset = false;
        Ok(())
    }
    /// Said once a conversation: it was had with another model than the one carrying it on now, which stays (Kumi never
    /// switches by itself).
    fn other_model(&self, kept: &SavedConversation, kernel: &dyn Kernel) {
        let was = kept.checkpoint.origin.clone().filter(|origin| origin.contains('/'));
        let now = kernel.checkpoint().ok().and_then(|checkpoint| checkpoint.origin);
        let (Some(was), Some(now)) = (was, now) else { return };
        let id = self.0.state.borrow().conversation_id.clone();
        if was == now || !self.0.state.borrow_mut().model_said.insert(id) {
            return;
        }
        let name = |model: &str| model.split_once('/').map_or(model, |(_, name)| name).to_string();
        self.notice(format!("This conversation was with {}; it carries on with {} (/model changes it).", name(&was), name(&now)));
    }
    /// A conversation taken up goes on as it's kept, its file untouched (looking at one doesn't change it): it's its
    /// place's current one, so Kumi comes back to it, and what the kernel restored from it is settled here, so a request
    /// stopped before its first answer goes back to it. A copy is kept from its first request.
    fn hold_taken_up(&self) {
        let held = self.0.state.borrow().kernel.clone();
        let Some(held) = held.filter(|h| h.value.has_checkpoint()) else {
            return;
        };
        let Ok(checkpoint) = held.value.checkpoint() else {
            return;
        };
        let (place, id, copy) = {
            let mut s = self.0.state.borrow_mut();
            s.settled = Some(Settled { checkpoint: Rc::new(checkpoint), key: held.key });
            s.as_kept = true;
            (s.place.clone(), s.conversation_id.clone(), s.copied_from.is_some())
        };
        if let (Some(store), Some(place), false) = (self.0.options.conversations.clone(), place, copy) {
            self.enqueue(async move { store.set_current(&place, &id).await });
        }
    }
    fn save_conversation(&self, turn: bool) {
        let held = self.0.state.borrow().kernel.clone();
        let Some(held) = held.filter(|h| h.value.has_checkpoint()) else {
            return;
        };
        let Ok(checkpoint) = held.value.checkpoint() else {
            return;
        };
        let checkpoint = Rc::new(checkpoint);
        let empty = checkpoint.messages.is_empty();
        let (place, id, conversation, name, from) = {
            let mut s = self.0.state.borrow_mut();
            s.settled = Some(Settled { checkpoint: checkpoint.clone(), key: held.key });
            s.as_kept = false;
            if turn {
                s.conversation_turns += 1;
                s.touched = true;
            }
            if s.conversation_first.is_none() {
                s.conversation_first = transcript_of(&checkpoint.messages)
                    .iter()
                    .find(|l| l.role == TranscriptRole::User)
                    .map(|l| cut(&l.text, 200).to_owned());
            }
            // Its name and where a copy came from go beside it with the first save that keeps it.
            let (name, from) = if empty { (None, None) } else { (s.conversation_name.take(), s.copied_from.take()) };
            (
                s.place.clone(),
                s.conversation_id.clone(),
                SavedConversation {
                    saved_at: now(),
                    checkpoint: KernelCheckpoint::clone(&checkpoint),
                    turns: Some(s.conversation_turns),
                    first: s.conversation_first.clone().filter(|s| !s.is_empty()),
                    changes: (!s.changes.is_empty())
                        .then(|| s.changes.iter().skip(s.changes.len().saturating_sub(2000)).cloned().collect()),
                },
                name,
                from,
            )
        };
        if let (Some(store), Some(place)) = (self.0.options.conversations.clone(), place) {
            let log = self.0.options.log.clone();
            self.enqueue(async move {
                let kept = if name.is_some() || from.is_some() {
                    store.create(&place, &id, &conversation, name.as_deref(), from.as_deref()).await
                } else {
                    store.save(&place, &id, &conversation).await
                };
                // Its log says so, as of then: a copy's begins now, from the log of the one it copies.
                if let (Ok(()), Some(log)) = (&kept, log) {
                    log.saved(&id, conversation.saved_at, conversation.turns.unwrap_or(0), from);
                }
                kept
            });
        }
    }
    /// What `@` offers now, in the order ties keep: the project's things as the integration last read them, the
    /// palettes, the techniques, then the library's sounds, matched in their index.
    async fn referable(&self) -> Referable {
        let integration = self.0.state.borrow().integration.clone();
        let mut short = match integration {
            Some(integration) => integration.reference_candidates(Signal::new()).await.unwrap_or_default(),
            None => vec![],
        };
        if let Some(palettes) = &self.0.palettes {
            short.extend(palettes.all().await.unwrap_or_default().into_iter().map(|(palette, held)| Candidate {
                label: palette.name.clone(),
                kind: CandidateKind::Palette,
                detail: Some(held),
                reference: Reference::Palette { id: palette.id, name: palette.name },
            }));
        }
        short.extend(self.techniques().await.unwrap_or_default().into_iter().map(|technique| Candidate {
            label: technique.name.clone(),
            kind: CandidateKind::Technique,
            detail: None,
            reference: Reference::Technique { id: technique.id, name: technique.name },
        }));
        let sounds = match &self.0.options.library {
            Some(library) => library.sound_index_in(self.0.options.profile.id).await.ok(),
            None => None,
        };
        Referable::new(short, sounds.map(|index| index as Rc<dyn Listed>))
    }
    /// The library's references as the turn's context names them, each read now: what a sound is, what a palette
    /// holds, what a technique fits. One the library no longer has is said to be gone.
    async fn library_references(&self, references: &[Reference]) -> Vec<Value> {
        let mut sounds = None;
        let mut techniques = None;
        let mut entries = vec![];
        for reference in references {
            let mut entry = JsonObject::new();
            let gone = |what: &str, name: &str| format!("The producer named “{name}” with @, and {what}: say so, and ask what they mean.");
            match reference {
                Reference::Daw { .. } => continue,
                Reference::Sample { path, name } => {
                    entry.insert("said".into(), json!(format!("@{name}")));
                    entry.insert("sound".into(), json!(path));
                    if sounds.is_none() {
                        sounds = Some(match &self.0.options.library {
                            Some(library) => library.sound_index_in(self.0.options.profile.id).await.ok(),
                            None => None,
                        });
                    }
                    let index = sounds.as_ref().and_then(Option::as_ref);
                    match index.map(|index| index.entries.iter().find(|sound| sound.file.path == *path)) {
                        Some(Some(sound)) => {
                            for (key, value) in [
                                ("seconds", sound.seconds.map(|seconds| json!(seconds))),
                                ("kind", sound.kind.and_then(|kind| serde_json::to_value(kind).ok())),
                                ("class", sound.class.and_then(|class| serde_json::to_value(class).ok())),
                            ] {
                                if let Some(value) = value {
                                    entry.insert(key.into(), value);
                                }
                            }
                        }
                        Some(None) => {
                            entry.insert("gone".into(), json!(gone("the library no longer has it", name)));
                        }
                        None => {}
                    }
                }
                Reference::Palette { id, name } => {
                    entry.insert("said".into(), json!(format!("@{name}")));
                    entry.insert("palette".into(), json!(name));
                    entry.insert("id".into(), json!(id));
                    if let Some(palettes) = &self.0.palettes {
                        match palettes.by_id(id).await {
                            Ok(Some((palette, held))) => {
                                entry.insert("palette".into(), json!(palette.name));
                                entry.insert("holds".into(), json!(held));
                            }
                            Ok(None) => {
                                entry.insert("gone".into(), json!(gone("it was forgotten since", name)));
                            }
                            Err(_) => {}
                        }
                    }
                }
                Reference::Technique { id, name } => {
                    entry.insert("said".into(), json!(format!("@{name}")));
                    entry.insert("technique".into(), json!(name));
                    entry.insert("id".into(), json!(id));
                    if techniques.is_none() && self.0.learned.is_some() {
                        techniques = Some(self.techniques().await.ok());
                    }
                    if let Some(known) = techniques.as_ref().and_then(Option::as_ref) {
                        match known.iter().find(|technique| technique.id == *id) {
                            Some(technique) => {
                                if let Some(fits) = technique.fits.lines().map(str::trim).find(|line| !line.is_empty()) {
                                    entry.insert("fits".into(), json!(fits));
                                }
                            }
                            None => {
                                entry.insert("gone".into(), json!(gone("it was forgotten since", name)));
                            }
                        }
                    }
                }
            }
            entries.push(Value::Object(entry));
        }
        entries
    }
    async fn observe(&self, op: &Operation, pinned: Option<PinnedNode>, continuing: bool) -> Result<Observation, RuntimeError> {
        self.observe_naming(op, pinned, vec![], continuing).await
    }
    /// A look that checks what the message names with `@` in the project, as it checks the pin.
    async fn observe_naming(
        &self,
        op: &Operation,
        pinned: Option<PinnedNode>,
        references: Vec<PinnedNode>,
        continuing: bool,
    ) -> Result<Observation, RuntimeError> {
        self.assert_current(op)?;
        let integration = self.0.state.borrow().integration.clone().ok_or_else(|| RuntimeError::plain("Integration not started"))?;
        op.phase.set(Phase::Refresh);
        {
            let mut s = self.0.state.borrow_mut();
            s.observation = None;
            // What `@` offers is read again after a look: the project may have changed under it.
            s.referable = None;
        }
        let hints = (pinned.is_some() || continuing || !references.is_empty()).then_some(ObserveHints {
            pinned,
            continuing: continuing.then_some(true),
            references,
        });
        let snapshot = integration.observe(op.signal.clone(), hints).await?;
        self.assert_current(op)?;
        let other_set = {
            let mut s = self.0.state.borrow_mut();
            s.project = snapshot.project.as_ref().map(|p| p.id.clone());
            s.set_name = snapshot.project.as_ref().map(|p| p.name.clone());
            s.set_open = snapshot.set.is_some();
            if let Some(identity) = &snapshot.set {
                s.set_identity = Some(identity.clone());
            }
            let other_set = s.set.as_ref().is_some_and(|set| *set != snapshot.key);
            s.set = Some(snapshot.key.clone());
            s.plan = snapshot.tools.iter().find(|t| t.role() == Some(ToolRole::Plan)).cloned();
            other_set
        };
        // Another Set is open: the goal shown was the last Set's.
        if other_set {
            self.emit(SessionEvent::ObjectiveCleared);
        }
        // What a saved place is called, kept beside its conversations once each time it changes.
        if let (Some(project), Some(store)) = (&snapshot.project, &self.0.options.conversations) {
            let noted = (project.id.clone(), project.name.clone());
            if self.0.state.borrow().noted.as_ref() != Some(&noted) {
                self.0.state.borrow_mut().noted = Some(noted);
                let (store, daw) = (store.clone(), self.0.options.profile.id.unwrap_or(DawId::Ableton));
                let (place, name) = (project.id.clone(), project.name.clone());
                self.enqueue(async move { store.note_place(&place, daw, &name).await });
            }
        }
        if snapshot.project.is_some() && self.0.notes.is_some() {
            let this = self.clone();
            tokio::task::spawn_local(async move {
                let _ = this.0.notes.as_ref().unwrap().flush().await;
            });
        }
        self.ensure_kernel(op, &snapshot).await?;
        self.assert_current(op)?;
        // The conversation is settled once its kernel is: what the integration keeps for one (the devices it made) is
        // kept for this one.
        let (integration, conversation) = {
            let s = self.0.state.borrow();
            (s.integration.clone(), s.conversation_id.clone())
        };
        if let Some(integration) = integration {
            integration.conversation(&conversation);
        }
        if let (Some(tracks), Some(l)) = (&snapshot.tracks, &self.0.learned) {
            l.drafts.observed(tracks);
        }
        if self.0.state.borrow().place.as_deref() == Some(self.unsaved()) {
            if let (Some(project), Some(store)) = (&snapshot.project, self.0.options.conversations.clone()) {
                let to = project.id.clone();
                let id = {
                    let mut s = self.0.state.borrow_mut();
                    s.place = Some(to.clone());
                    s.conversation_id.clone()
                };
                if let Some(log) = &self.0.options.log {
                    log.moved(&to);
                }
                let unsaved = self.unsaved();
                self.enqueue(async move { store.move_conversation(&id, unsaved, &to).await });
            }
            // Its goal goes with it, so the next new Set doesn't find it.
            if let Some(project) = &snapshot.project {
                self.move_unsaved_objective(&project.id);
            }
        }
        self.0.state.borrow_mut().observation = Some(snapshot.label.clone());
        self.emit(SessionEvent::observed(&snapshot));
        self.bring_back_jobs().await;
        Ok(snapshot)
    }
    async fn reset(&self, op: &Operation) -> Result<(), RuntimeError> {
        op.phase.set(Phase::Start);
        self.drop_resources().await?;
        self.assert_current(op)?;
        let generation = {
            let mut s = self.0.state.borrow_mut();
            s.turns = 0;
            s.must_reset = false;
            s.generation += 1;
            s.generation
        };
        let weak = Rc::downgrade(&self.0);
        let listener = Rc::new(move |next, cause| {
            if let Some(inner) = weak.upgrade() {
                Session(inner).connection_changed(generation, next, cause);
            }
        });
        let integration = (self.0.options.integration_factory)(listener);
        self.0.state.borrow_mut().integration = Some(integration.clone());
        integration.start(op.signal.clone()).await?;
        self.assert_current(op)?;
        self.observe(op, None, false).await?;
        self.0.state.borrow_mut().started = true;
        Ok(())
    }
    fn perform(&self, is_turn: bool, phase: Phase, work: Work, limit_ms: Option<u64>, input: Option<String>) -> Result<Done, RuntimeError> {
        let limit_ms = limit_ms.unwrap_or(self.0.timeout_ms);
        let now = Instant::now();
        let (tx, rx) = oneshot::channel();
        let done =
            async { rx.await.unwrap_or_else(|_| Err(RuntimeError::plain("Session operation ended unexpectedly"))) }.boxed_local().shared();
        let op = {
            let mut s = self.0.state.borrow_mut();
            if s.state == TurnState::Closed {
                return Err(RuntimeError::plain("Session is closed"));
            }
            if s.active.is_some() {
                return Err(RuntimeError::plain("Session is busy; cancel first"));
            }
            s.next_operation += 1;
            let op = Rc::new(Operation {
                id: s.next_operation,
                signal: Signal::new(),
                done: done.clone(),
                is_turn,
                phase: Cell::new(phase),
                input,
                linger: Cell::new(0),
                steady: Cell::new(false),
                working: Cell::new(0),
                quiet: Cell::new(Some(now + Duration::from_millis(if is_turn { self.0.idle_ms } else { limit_ms }))),
                limit: Cell::new(is_turn.then(|| now + Duration::from_millis(self.0.turn_limit_ms))),
                changed: Notify::new(),
                idle_ms: self.0.idle_ms,
            });
            s.active = Some(op.clone());
            op
        };
        self.set_state(TurnState::Running);
        let this = self.clone();
        tokio::task::spawn_local(async move {
            let result = this.drive(op, phase, work, limit_ms).await;
            let _ = tx.send(result);
        });
        Ok(done)
    }
    async fn drive(&self, op: Rc<Operation>, initial_phase: Phase, work: Work, limit_ms: u64) -> Result<(), RuntimeError> {
        let began = Instant::now();
        let recorder = op.is_turn.then(timing::begin);
        if op.is_turn {
            self.0.answers.set(self.0.answers.get() + 1);
            self.look_at_files();
        }
        let mut ended: Option<(Value, Option<Usage>)> = None;
        let settled = Rc::new(Cell::new(false));
        let mark = settled.clone();
        let this = self.clone();
        let work_op = op.clone();
        let mut task = tokio::task::spawn_local(async move {
            let result = work(this, work_op).await;
            mark.set(true);
            result
        });
        let mut timed_out = None;
        let mut cancelled_at = None;
        let outcome = loop {
            if op.signal.is_cancelled() && cancelled_at.is_none() {
                self.set_state(TurnState::Cancelling);
                cancelled_at = Some(Instant::now() + Duration::from_millis(self.0.grace_ms + op.linger.get()));
            }
            let deadline =
                if let Some(at) = cancelled_at { Some(at) } else { [op.quiet.get(), op.limit.get()].into_iter().flatten().min() };
            tokio::select! { biased;
                result = &mut task => { break result.map_err(|e| RuntimeError::plain(e.to_string())).and_then(|r| r); },
                _ = op.signal.cancelled(), if cancelled_at.is_none() => {},
                _ = op.changed.notified(), if cancelled_at.is_none() => {},
                _ = async { match deadline { Some(at) => tokio::time::sleep_until(at).await, None => futures::future::pending().await } } => {
                    if cancelled_at.is_some() { break Err(RuntimeError::plain("Operation cancelled")); }
                    timed_out = Some(if op.limit.get().is_some_and(|at| at <= Instant::now()) { Timeout::Limit } else { Timeout::Quiet }); op.signal.cancel();
                }
            }
        };
        let mut result = Ok(());
        let settled_result = outcome.as_ref().ok().and_then(|r| r.clone());
        if op.signal.is_cancelled() {
            // A tool call the answer started may still be putting Live back (taking a round back, closing a render):
            // the answer ends once it has, so the next one doesn't run beside it. Past SETTLE_MS the next answer may
            // start, and its Live tool calls still wait for it.
            let integration = self.0.state.borrow().integration.clone();
            if let Some(integration) = integration {
                let _ = tokio::time::timeout(Duration::from_millis(SETTLE_MS), integration.settled()).await;
            }
            if !settled.get() {
                self.0.state.borrow_mut().must_reset = true;
                if self.drop_kernel().await.is_err() {
                    self.error("Inference cleanup did not finish within its deadline.");
                }
            }
            if let Some(why) = timed_out {
                self.error(if op.is_turn {
                    match why {
                        Timeout::Limit => format!(
                            "Kumi stopped: this answer had run for {}. Anything it changed is in HISTORY; ask it to carry on.",
                            span(self.0.turn_limit_ms)
                        ),
                        Timeout::Quiet => format!(
                            "Kumi stopped after {} without progress. Anything it changed is in HISTORY; ask it to carry on.",
                            span(self.0.idle_ms)
                        ),
                    }
                } else {
                    format!("Kumi stopped waiting after {}.", span(limit_ms))
                });
            }
            if op.is_turn {
                if let Some(l) = &self.0.learned {
                    l.drafts.abandon();
                }
                let usage = settled_result.and_then(|r| r.usage);
                ended = Some((json!(StopReason::Cancelled), usage.clone()));
                self.emit(SessionEvent::TurnComplete {
                    result: TurnResult { stop_reason: StopReason::Cancelled, usage },
                    elapsed_ms: began.elapsed().as_millis() as u64,
                });
                if settled.get() {
                    self.save_conversation(true);
                }
            }
        } else {
            match outcome {
                Ok(Some(turn)) if op.is_turn => {
                    let mut offer = None;
                    if let Some(l) = &self.0.learned {
                        if turn.stop_reason == StopReason::Cancelled {
                            l.drafts.abandon();
                        } else {
                            offer = l.drafts.turn_ended(turn.stop_reason == StopReason::Completed);
                        }
                    }
                    let save = turn.stop_reason != StopReason::Cancelled;
                    ended = Some((json!(turn.stop_reason), turn.usage.clone()));
                    self.emit(SessionEvent::TurnComplete { result: turn, elapsed_ms: began.elapsed().as_millis() as u64 });
                    // Asked after the answer, so the answer's own question, if it ends on one, comes first.
                    if let Some(technique) = offer {
                        self.emit(SessionEvent::Technique(TechniqueEvent { action: TechniqueAction::Offered, technique }));
                    }
                    if save {
                        self.save_conversation(true);
                    }
                }
                Err(error) => {
                    let kumi = error.kumi();
                    let actionable = kumi.is_some_and(|e| {
                        matches!(
                            e.kind,
                            FailureKind::Auth | FailureKind::Billing | FailureKind::Model | FailureKind::Config | FailureKind::Daw
                        )
                    });
                    let message = if actionable {
                        error.message()
                    } else if op.phase.get() == Phase::Undo {
                        kumi.map(|e| e.message.clone())
                            .unwrap_or_else(|| format!("The undo didn't finish; check {}.", self.0.options.profile.words.app))
                    } else if op.phase.get() != Phase::Inference {
                        "Context refresh failed; no answer was generated from old observations.".into()
                    } else {
                        kumi.map(|e| e.message.clone())
                            .unwrap_or("Inference failed; check the configured model, sign-in and connection.".into())
                    };
                    self.emit(SessionEvent::Error { message, kind: kumi.map(|e| e.kind), provider: kumi.and_then(|e| e.provider.clone()) });
                    if op.is_turn {
                        ended = Some((json!("error"), None));
                        if op.phase.get() == Phase::Inference {
                            self.save_conversation(true);
                        }
                        if let Some(l) = &self.0.learned {
                            l.drafts.abandon();
                        }
                    }
                    if !op.is_turn && initial_phase == Phase::Start {
                        let _ = self.drop_resources().await;
                        result = Err(RuntimeError::plain("Could not start Kumi session; check model login and connection."));
                    }
                }
                _ => {}
            }
        }
        if let (Some(recorder), Some((stop, usage)), Some(file)) = (recorder, ended, self.0.timings.clone()) {
            let mut line = timing::line(&recorder.finish(), began.elapsed().as_millis() as u64, stop, usage.as_ref());
            // The DAW the turn ran in, when it isn't Live (never written for Live).
            if let Some(daw) = self.0.options.profile.id.filter(|daw| *daw != DawId::Ableton) {
                line["daw"] = json!(daw);
            }
            timing::append(std::path::Path::new(&file), &line);
        }
        let current = self.0.state.borrow().active.as_ref().is_some_and(|a| a.id == op.id);
        if current {
            self.0.state.borrow_mut().active = None;
            self.set_state(TurnState::Idle);
        }
        result
    }
    /// Kumi's undo tool, watched for the producer's undos it makes.
    /// judge and tune, held to the loop: once it's over, only the run's end (judge done) goes through. A groove run
    /// (judged on the notes, never listened to) isn't the loop's: its calls go through and its rounds aren't counted.
    fn with_loop(&self, tool: Rc<dyn KernelTool>) -> Rc<dyn KernelTool> {
        if matches!(tool.role(), Some(ToolRole::Judge | ToolRole::Tune)) {
            Rc::new(LoopGuard { tool, session: Rc::downgrade(&self.0) })
        } else {
            tool
        }
    }
    fn with_taste(&self, tool: Rc<dyn KernelTool>) -> Rc<dyn KernelTool> {
        match &self.0.taste {
            Some(taste) if tool.role() == Some(ToolRole::Undo) => taste.watch_undo(tool),
            _ => tool,
        }
    }
    fn with_technique(&self, tool: Rc<dyn KernelTool>) -> Rc<dyn KernelTool> {
        match &self.0.learned {
            Some(learned) if tool.role() == Some(ToolRole::Plan) => Rc::new(TechniquePlan {
                description: format!("{} {}", tool.description(), PLAN_TECHNIQUE["description"].as_str().unwrap_or_default()),
                tool,
                learned: learned.clone(),
            }),
            _ => tool,
        }
    }
}

struct LoopGuard {
    tool: Rc<dyn KernelTool>,
    session: std::rc::Weak<Inner>,
}
#[async_trait(?Send)]
impl KernelTool for LoopGuard {
    fn name(&self) -> &str {
        self.tool.name()
    }
    fn description(&self) -> &str {
        self.tool.description()
    }
    fn input_schema(&self) -> JsonObject {
        self.tool.input_schema()
    }
    fn deferred(&self) -> bool {
        self.tool.deferred()
    }
    fn listed_by(&self) -> Option<&str> {
        self.tool.listed_by()
    }
    async fn execute(&self, input: JsonObject, signal: Signal) -> Result<ToolResult, RuntimeError> {
        let judging = self.tool.role() == Some(ToolRole::Judge);
        let ending = judging && input.get("done") == Some(&Value::Bool(true));
        let starting = judging && input.contains_key("goal");
        // Code decides the loop, not the model: once over, only the run's end; while it goes, its checklist stands
        // and the run ends when Kumi asks.
        let held = self.session.upgrade().and_then(|inner| {
            let s = inner.state.borrow();
            let run = s.looping.as_ref()?.borrow();
            if let Some(why) = run.over() {
                return (!ending).then(|| format!(
                    "Kumi's loop is over: {why}. End the run with judge done: true (one last listen to the whole stretch), then tell the producer what changed and what's still off."
                ));
            }
            if starting && run.started() {
                return Some("Kumi's loop already has its checklist: work toward it, one change at a time, judging each (judge with change). Kumi decides when the loop stops.".into());
            }
            // The producer stepped in (stop, leave it): the run may end now.
            if ending && s.steers <= run.steers {
                return run.holds_done();
            }
            None
        });
        match held {
            Some(why) => Ok(ToolResult::error(why)),
            None => self.tool.execute(input, signal).await,
        }
    }
    fn role(&self) -> Option<ToolRole> {
        self.tool.role()
    }
    fn concurrent(&self) -> bool {
        self.tool.concurrent()
    }
}

struct TechniquePlan {
    tool: Rc<dyn KernelTool>,
    learned: Rc<TechniqueTools>,
    description: String,
}
fn take_technique(mut input: JsonObject, learned: &TechniqueTools) -> JsonObject {
    if let Some(value) = input.shift_remove("technique").filter(Value::is_object) {
        learned.draft_from(&value);
    }
    input
}
#[async_trait(?Send)]
impl KernelTool for TechniquePlan {
    fn name(&self) -> &str {
        self.tool.name()
    }
    fn description(&self) -> &str {
        &self.description
    }
    fn input_schema(&self) -> JsonObject {
        let mut schema = self.tool.input_schema();
        let properties = schema.entry("properties").or_insert(json!({}));
        if let Some(properties) = properties.as_object_mut() {
            properties.insert("technique".into(), PLAN_TECHNIQUE["schema"].clone());
        }
        schema
    }
    async fn execute(&self, input: JsonObject, signal: Signal) -> Result<ToolResult, RuntimeError> {
        self.tool.execute(take_technique(input, &self.learned), signal).await
    }
    fn deferred(&self) -> bool {
        self.tool.deferred()
    }
    fn listed_by(&self) -> Option<&str> {
        self.tool.listed_by()
    }
    fn stream(&self, signal: Signal, on_start: Rc<dyn Fn()>) -> Option<Box<dyn StreamingCall>> {
        self.tool
            .stream(signal, on_start)
            .map(|call| Box::new(TechniqueStream { call, learned: self.learned.clone() }) as Box<dyn StreamingCall>)
    }
    fn role(&self) -> Option<ToolRole> {
        self.tool.role()
    }
    fn concurrent(&self) -> bool {
        self.tool.concurrent()
    }
}
struct TechniqueStream {
    call: Box<dyn StreamingCall>,
    learned: Rc<TechniqueTools>,
}
#[async_trait(?Send)]
impl StreamingCall for TechniqueStream {
    fn push(&self, delta: &str) {
        self.call.push(delta);
    }
    async fn finish(&self, input: Option<JsonObject>) -> Result<ToolResult, RuntimeError> {
        self.call.finish(input.map(|input| take_technique(input, &self.learned))).await
    }
    async fn abandon(&self) {
        self.call.abandon().await;
    }
    fn started(&self) -> bool {
        self.call.started()
    }
}

#[async_trait(?Send)]
impl SessionController for Session {
    async fn start(&self) -> Result<(), RuntimeError> {
        if self.0.state.borrow().started {
            return Err(RuntimeError::plain("Session already started"));
        }
        self.perform(
            false,
            Phase::Start,
            Box::new(|this, op| {
                async move {
                    this.copy_wanted_import().await?;
                    this.reset(&op).await?;
                    Ok(None)
                }
                .boxed_local()
            }),
            None,
            None,
        )?
        .await
    }
    async fn submit(&self, input: &str, pinned: Option<PinnedNode>) -> Result<(), RuntimeError> {
        self.submit_with(input, pinned, vec![]).await
    }
    fn has_attachments(&self) -> bool {
        true
    }
    async fn submit_with(&self, input: &str, pinned: Option<PinnedNode>, attachments: Vec<Attachment>) -> Result<(), RuntimeError> {
        self.submit_with_references(input, pinned, attachments, vec![]).await
    }
    async fn submit_with_references(
        &self,
        input: &str,
        pinned: Option<PinnedNode>,
        attachments: Vec<Attachment>,
        references: Vec<Reference>,
    ) -> Result<(), RuntimeError> {
        let (text, pictures) = attached(input, &attachments).await?;
        {
            let mut s = self.0.state.borrow_mut();
            if s.state == TurnState::Closed {
                return Err(RuntimeError::plain("Session is closed"));
            }
            if s.active.is_some() {
                return Err(RuntimeError::plain("Session is busy; cancel first"));
            }
            if !s.started {
                return Err(RuntimeError::plain("Session is not started"));
            }
            if trim(input).is_empty() {
                return Err(RuntimeError::plain("Enter a nonempty prompt"));
            }
            if input.len() > MAX_REQUEST {
                // Its words go back where the producer wrote them, to send part of.
                drop(s);
                self.emit(SessionEvent::Resend { text: input.to_owned() });
                return Err(refused(format!(
                    "This message is {}, and one message takes up to {}: send part of it.",
                    size_words(input.len() as u64),
                    size_words(MAX_REQUEST as u64)
                )));
            }
            if self.0.options.max_turns.is_some_and(|max| s.turns >= max) {
                return Err(RuntimeError::plain("Conversation limit reached; use /new"));
            }
            s.turns += 1;
            s.interrupted = None;
        }
        // The request, in the producer's words, before anything of its answer: a full log begins again with it.
        if let Some(log) = &self.0.options.log {
            let (turns, had) = {
                let s = self.0.state.borrow();
                (s.conversation_turns, s.settled.as_ref().map(|settled| settled.checkpoint.clone()))
            };
            log.request(input, attachments.iter().map(|file| file.name.clone()).collect(), pinned.clone(), turns, had);
        }
        let said = text.clone();
        self.perform(
            true,
            Phase::Refresh,
            Box::new(move |this, op| async move { this.submit_turn(op, text, pinned, pictures, references).await }.boxed_local()),
            None,
            Some(said),
        )?
        .await
    }
    async fn references(&self, query: &str, limit: u32) -> Result<Vec<Candidate>, RuntimeError> {
        // The list opens on `@` (an empty query): what it offers is read then, and ranked as the producer types on.
        let held = self.0.state.borrow_mut().referable.take().filter(|_| !query.is_empty());
        let mut referable = match held {
            Some(referable) => referable,
            None => self.referable().await,
        };
        let found = referable.rank(query, if limit == 0 { 50 } else { limit as usize });
        self.0.state.borrow_mut().referable = Some(referable);
        Ok(found)
    }
    fn has_steer(&self) -> bool {
        true
    }
    fn steer(&self, text: &str) -> bool {
        let held = {
            let s = self.0.state.borrow();
            // A goal's self-check isn't the place for the producer's words: they wait for the next turn.
            if s.state == TurnState::Closed
                || s.checking
                || !s.active.as_ref().is_some_and(|op| op.is_turn && op.phase.get() == Phase::Inference)
                || trim(text).is_empty()
                || text.len() > 16 * 1024
            {
                return false;
            }
            s.kernel.clone()
        };
        let steered = held.is_some_and(|h| h.value.has_steer() && h.value.steer(text));
        if steered {
            self.0.state.borrow_mut().steers += 1;
            // A judged run's changes from here may be the producer's: a round mustn't take them back.
            let integration = self.0.state.borrow().integration.clone();
            if let Some(integration) = integration {
                integration.steered();
            }
            if let Some(taste) = &self.0.taste {
                taste.steered(text);
            }
        }
        steered
    }
    fn has_aside(&self) -> bool {
        true
    }
    async fn aside(&self, question: &str, on_text: OnText, signal: Option<Signal>) -> Result<String, RuntimeError> {
        let held = {
            let s = self.0.state.borrow();
            if s.state == TurnState::Closed {
                return Err(RuntimeError::plain("Session is closed"));
            }
            s.kernel.clone()
        };
        let held = held
            .filter(|h| h.value.has_aside())
            .ok_or_else(|| KumiError::new(FailureKind::Request, "Kumi isn't ready for a side question yet; ask again in a moment."))?;
        // A side question can run while a turn does; it isn't part of that turn's timing.
        timing::background(held.value.aside(question, signal.unwrap_or_default(), on_text)).await
    }
    async fn refresh(&self) -> Result<(), RuntimeError> {
        if !self.0.state.borrow().started {
            return Err(RuntimeError::plain("Session is not started"));
        }
        self.perform(
            false,
            Phase::Refresh,
            Box::new(|this, op| {
                async move {
                    this.observe(&op, None, false).await?;
                    Ok(None)
                }
                .boxed_local()
            }),
            None,
            None,
        )?
        .await
    }
    async fn new_conversation(&self) -> Result<(), RuntimeError> {
        self.perform(
            false,
            Phase::Refresh,
            Box::new(|this, op| {
                async move {
                    this.drop_kernel().await?;
                    this.assert_current(&op)?;
                    if let Some(l) = &this.0.learned {
                        l.drafts.reset();
                    }
                    let (place, integrated, copy) = {
                        let mut s = this.0.state.borrow_mut();
                        s.start_fresh = true;
                        s.turns = 0;
                        s.must_reset = false;
                        s.settled = None;
                        s.interrupted = None;
                        (s.place.clone(), s.integration.is_some(), s.copied_from.is_some())
                    };
                    this.drop_wanted();
                    if let (Some(store), Some(place)) = (this.0.options.conversations.clone(), place) {
                        this.enqueue(async move { store.fresh(&place).await });
                    }
                    if integrated {
                        this.observe(&op, None, false).await?;
                    } else {
                        this.reset(&op).await?;
                    }
                    // The goal shown was the last conversation's, not this Set's (an unsaved Set Live gave no identity
                    // for): it's no longer shown.
                    let shown = this.0.state.borrow().objective.is_some();
                    if shown && this.objective_kept().await.is_none() {
                        this.0.state.borrow_mut().objective = None;
                        this.emit(SessionEvent::ObjectiveCleared);
                    }
                    this.notice(match this.0.options.conversations {
                        Some(_) if !copy => "New conversation. The last one is kept; /resume goes back to it.",
                        // A chat without Live keeps none, and a copy is kept from its first request.
                        _ => "New conversation.",
                    });
                    Ok(None)
                }
                .boxed_local()
            }),
            None,
            None,
        )?
        .await
    }
    fn has_reconnect(&self) -> bool {
        true
    }
    async fn reconnect(&self) -> Result<(), RuntimeError> {
        self.perform(
            false,
            Phase::Start,
            Box::new(|this, op| {
                async move {
                    let (held, settled, place) = {
                        let s = this.0.state.borrow();
                        (s.kernel.clone(), s.settled.clone(), s.place.clone())
                    };
                    let checkpoint = held
                        .filter(|h| h.value.has_checkpoint())
                        .and_then(|h| h.value.checkpoint().ok())
                        .or_else(|| settled.map(|s| KernelCheckpoint::clone(&s.checkpoint)));
                    if let Some(checkpoint) = checkpoint {
                        this.0.state.borrow_mut().carry = Some((checkpoint, place));
                    }
                    this.reset(&op).await?;
                    if this.0.state.borrow().connection == ConnectionState::Connected {
                        this.notice(&format!("Reconnected to {}; the conversation carries on.", this.0.options.profile.words.app));
                    }
                    Ok(None)
                }
                .boxed_local()
            }),
            None,
            None,
        )?
        .await
    }
    fn has_conversations(&self) -> bool {
        self.0.options.conversations.is_some()
    }
    async fn conversations(&self) -> Result<Vec<ConversationSummary>, RuntimeError> {
        let place = {
            let s = self.0.state.borrow();
            s.place.clone().or(s.project.clone()).unwrap_or(self.unsaved().into())
        };
        Ok(if let Some(store) = &self.0.options.conversations {
            self.mark_going(store.list(&place).await.unwrap_or_default())
        } else {
            vec![]
        })
    }
    fn has_resume_conversation(&self) -> bool {
        true
    }
    async fn resume_conversation(&self, id: &str) -> Result<bool, RuntimeError> {
        let Some(store) = self.0.options.conversations.clone() else {
            return Ok(false);
        };
        // Where it's kept: this Set's place, or any Set's, by its id.
        let (here, open, set_open) = {
            let s = self.0.state.borrow();
            (s.place.clone().or(s.project.clone()).unwrap_or(self.unsaved().into()), s.project.clone(), s.set_open)
        };
        let place = match store.find(id).await? {
            ConversationMatch::One(row) if row.id == id => row.place,
            _ => here.clone(),
        };
        // Another DAW's conversation goes on only in that DAW: Kumi says whose it is, and how to open it there.
        if let Some(words) = self.elsewhere(&place) {
            return Err(KumiError::new(FailureKind::Request, words).into());
        }
        let same = {
            let s = self.0.state.borrow();
            s.conversation_id == id && s.place.as_deref() == Some(&place)
        };
        if same {
            return Ok(true);
        }
        let saving = self.0.saving.borrow().clone();
        let _ = saving.await;
        let Some(conversation) = store.load(&place, id).await? else {
            return Ok(false);
        };
        // The open Set's place, when Live was read (without it, the project is the last one seen).
        let open = set_open.then(|| open.unwrap_or(self.unsaved().into()));
        self.drop_wanted();
        // One from a file has no Set here: it carries on in a copy, and the model is told where it came from.
        if place == IMPORTED {
            return self.carry_imported(store, id).await;
        }
        // Another Set's, while a Set is open: the producer says whether to open its Set, carry it on here in a copy, or
        // leave it.
        if open.as_ref().is_some_and(|open| *open != place) && place != self.unsaved() {
            let key = {
                let mut s = self.0.state.borrow_mut();
                s.wanted = Some(Wanted { place, id: id.to_owned(), from: None, asked: None });
                s.set.clone().unwrap_or_default()
            };
            let name = self.0.state.borrow().set_name.clone().filter(|_| self.0.state.borrow().project.is_some());
            self.ask_about_wanted(&key, name).await;
            return Ok(true);
        }
        // No Set open yet: it goes on now, and Kumi asks once a Set opens that isn't its own.
        if open.is_none() && place != self.unsaved() {
            self.0.state.borrow_mut().wanted = Some(Wanted { place: place.clone(), id: id.to_owned(), from: None, asked: None });
        }
        self.go_to(Chosen { place, id: id.to_owned(), conversation, copy_of: None }, None).await?;
        Ok(true)
    }
    async fn all_conversations(&self) -> Result<Vec<ConversationSummary>, RuntimeError> {
        let Some(store) = &self.0.options.conversations else { return Ok(vec![]) };
        // This DAW's conversations (and imported ones): another DAW's go on in that DAW.
        let daw = self.0.options.profile.id.unwrap_or(DawId::Ableton);
        Ok(self.mark_going(store.all().await?.into_iter().filter(|row| kept_for(&row.place, daw)).collect()))
    }
    async fn find_conversation(&self, word: &str) -> Result<ConversationMatch, RuntimeError> {
        let Some(store) = &self.0.options.conversations else { return Ok(ConversationMatch::None) };
        let daw = self.0.options.profile.id.unwrap_or(DawId::Ableton);
        Ok(match store.find(word).await? {
            // (one of another DAW's is still found, so taking it up says whose it is)
            ConversationMatch::One(row) => ConversationMatch::One(self.mark_going(vec![row]).remove(0)),
            ConversationMatch::Several(rows) => {
                let ours: Vec<ConversationSummary> = rows.iter().filter(|row| kept_for(&row.place, daw)).cloned().collect();
                match ours.len() {
                    0 => ConversationMatch::Several(self.mark_going(rows)),
                    1 => ConversationMatch::One(self.mark_going(ours).remove(0)),
                    _ => ConversationMatch::Several(self.mark_going(ours)),
                }
            }
            other => other,
        })
    }
    async fn rename_conversation(&self, id: Option<&str>, name: Option<&str>) -> Result<bool, RuntimeError> {
        let Some(store) = self.0.options.conversations.clone() else { return Ok(false) };
        let (place, now) = {
            let s = self.0.state.borrow();
            (s.place.clone().unwrap_or(self.unsaved().into()), s.conversation_id.clone())
        };
        let id = id.unwrap_or(&now).to_owned();
        let saving = self.0.saving.borrow().clone();
        let _ = saving.await;
        let place = match store.find(&id).await? {
            ConversationMatch::One(row) if row.id == id => row.place,
            // This one isn't kept yet: its name goes with its first save.
            _ if id == now => {
                self.0.state.borrow_mut().conversation_name = name.map(str::to_owned);
                return Ok(true);
            }
            _ => place,
        };
        store.rename(&place, &id, name).await
    }
    async fn pin_conversation(&self, id: &str, pinned: bool) -> Result<bool, RuntimeError> {
        let Some(store) = &self.0.options.conversations else { return Ok(false) };
        match store.find(id).await? {
            ConversationMatch::One(row) if row.id == id => store.pin(&row.place, id, pinned).await,
            _ => Ok(false),
        }
    }
    async fn delete_conversation(&self, id: &str) -> Result<bool, RuntimeError> {
        let Some(store) = &self.0.options.conversations else { return Ok(false) };
        if self.0.state.borrow().conversation_id == id {
            return Err(KumiError::new(FailureKind::Request, "That's the conversation going on now: /new first, then delete it.").into());
        }
        match store.find(id).await? {
            ConversationMatch::One(row) if row.id == id => store.delete(&row.place, id).await,
            _ => Ok(false),
        }
    }
    async fn fork_conversation(&self, name: Option<&str>) -> Result<Option<String>, RuntimeError> {
        let Some(store) = self.0.options.conversations.clone() else { return Ok(None) };
        let (held, settled, place, old, copied) = {
            let s = self.0.state.borrow();
            if s.active.is_some() {
                return Err(KumiError::new(FailureKind::Request, "Kumi is still working: fork once it's done.").into());
            }
            let place = s.place.clone().unwrap_or(self.unsaved().into());
            (s.kernel.clone(), s.settled.clone(), place, s.conversation_id.clone(), s.copied_from.clone())
        };
        let checkpoint = held
            .filter(|h| h.value.has_checkpoint())
            .and_then(|h| h.value.checkpoint().ok())
            .or(settled.map(|s| KernelCheckpoint::clone(&s.checkpoint)))
            .filter(|checkpoint| !checkpoint.messages.is_empty());
        let Some(checkpoint) = checkpoint else { return Ok(None) };
        let saving = self.0.saving.borrow().clone();
        let _ = saving.await;
        let id = store.new_id().await;
        let conversation = {
            let s = self.0.state.borrow();
            SavedConversation {
                saved_at: now(),
                checkpoint,
                turns: Some(s.conversation_turns),
                first: s.conversation_first.clone().filter(|s| !s.is_empty()),
                changes: (!s.changes.is_empty()).then(|| s.changes.clone()),
            }
        };
        // The original as it is now (as it was kept, when nothing's been asked since it was taken up), then the copy: the
        // same words, changes and undo, a new id from here on.
        if !self.0.state.borrow().as_kept {
            self.save_conversation(false);
        }
        let saving = self.0.saving.borrow().clone();
        let _ = saving.await;
        // A copy with nothing kept yet stands for the conversation it copies: the fork comes from that one.
        let source = copied.unwrap_or_else(|| old.clone());
        store.create(&place, &id, &conversation, name, Some(&source)).await?;
        store.release(&place, &old);
        store.claim(&place, &id);
        {
            let mut s = self.0.state.borrow_mut();
            s.conversation_id = id.clone();
            s.conversation_name = None;
            s.copied_from = None;
            s.touched = true;
        }
        // The copy's log begins as the original's is now, and is as up to date as the copy's checkpoint.
        if let Some(log) = &self.0.options.log {
            log.open(&place, &id, Some(&conversation), Some(source.clone()));
            log.saved(&id, conversation.saved_at, conversation.turns.unwrap_or(0), Some(source.clone()));
        }
        self.wanted_goes_on_as(&place, &old, &id);
        self.notice(format!("Forked; the original stays as {source}."));
        Ok(Some(id))
    }
    async fn answer_set(&self, choice: SetChoice) -> Result<(), RuntimeError> {
        let Some(wanted) = self.0.state.borrow().wanted.clone() else { return Ok(()) };
        // Answered: the question goes. Opening keeps the conversation waiting for its Set (asked, so not asked again here).
        match choice {
            SetChoice::Open => self.emit(SessionEvent::SetAsked { ask: None }),
            _ => {
                self.drop_wanted();
            }
        }
        let row = self.wanted_row(&wanted).await;
        let (daw, noun) = self.words();
        let named = row.as_ref().and_then(|row| row.set.clone());
        let set = named.as_ref().map(|name| format!("“{name}”")).unwrap_or_else(|| format!("its {noun}"));
        match choice {
            // The open Set's own conversation goes on, whichever was going on.
            SetChoice::Cancel => {
                let going = {
                    let s = self.0.state.borrow();
                    s.conversation_id == wanted.id && s.place.as_ref() == Some(&wanted.place)
                };
                if going && self.0.state.borrow().started {
                    self.0.state.borrow_mut().own_set = true;
                    self.refresh().await?;
                }
                Ok(())
            }
            SetChoice::Open => {
                let integration = self.0.state.borrow().integration.clone();
                let opened = match integration {
                    // The producer opens it (they were asked to): nothing to say Kumi couldn't.
                    Some(integration) if !integration.has_open_place() => {
                        self.notice(format!("Open {set} in {daw}, and this conversation carries on there."));
                        return Ok(());
                    }
                    Some(integration) => integration.open_place(&wanted.place, abort::timeout(30_000)).await?,
                    None => Err(format!("Kumi isn't connected to {daw}.")),
                };
                // It's taken up once its Set is open, however that Set comes to be opened.
                match opened {
                    Ok(()) => self.notice(format!("Opening {set} in {daw}; this conversation carries on once it's open.")),
                    Err(why) => self.notice(format!("{why} Open {set} in {daw} yourself, and this conversation carries on there.")),
                }
                Ok(())
            }
            SetChoice::Copy => {
                let Some(store) = self.0.options.conversations.clone() else { return Ok(()) };
                let saving = self.0.saving.borrow().clone();
                let _ = saving.await;
                let Some((source, kept)) = self.kept_wanted(&wanted).await? else {
                    self.notice("That conversation isn't kept any more.");
                    return Ok(());
                };
                let (here, open) = {
                    let s = self.0.state.borrow();
                    (s.project.clone().unwrap_or(self.unsaved().into()), s.set_name.clone().filter(|_| s.project.is_some()))
                };
                // What Kumi read of the other Set stays with the original; the model is told where it is now.
                let mut checkpoint = kept.checkpoint.clone();
                checkpoint.messages = crate::kernel::budget::without_observations(&checkpoint.messages);
                let copy = SavedConversation { saved_at: now(), checkpoint, changes: None, first: kept.first.clone(), turns: kept.turns };
                let id = store.new_id().await;
                store.create(&here, &id, &copy, None, Some(&source)).await?;
                let open = open.map(|name| format!("“{name}”")).unwrap_or_else(|| format!("a {noun} not saved yet"));
                let other = named.map(|name| format!("another {noun}, “{name}”,")).unwrap_or_else(|| format!("another {noun}"));
                let said = format!("Carrying on a copy here; the original stays with {set} as {source}.");
                self.go_to(Chosen { place: here, id, conversation: copy, copy_of: None }, Some(said)).await?;
                self.0.state.borrow_mut().next_note = Some(format!(
                    "\n\n[Kumi] This conversation began about {other} and carries on here about {open}, as a copy: what Kumi read of that {noun} was taken out. Read the open {noun} before changing anything. (Kumi wrote this note, not the producer.)"
                ));
                Ok(())
            }
        }
    }
    async fn conversation_now(&self) -> Option<ConversationNow> {
        // A chat that keeps none (without Live, started without -c or -r) has nothing to say of one.
        self.0.options.conversations.as_ref()?;
        let (place, id, touched, first, name) = {
            let s = self.0.state.borrow();
            (s.place.clone(), s.conversation_id.clone(), s.touched, s.conversation_first.clone(), s.conversation_name.clone())
        };
        let row = match &self.0.options.conversations {
            Some(store) => match store.find(&id).await {
                Ok(ConversationMatch::One(row)) if Some(&row.place) == place.as_ref() => Some(row),
                _ => None,
            },
            None => None,
        };
        Some(ConversationNow {
            title: match &row {
                Some(row) if name.is_none() => row.title(),
                _ => crate::core::conversations::title(name.as_deref(), first.as_deref().unwrap_or("")),
            },
            kept: row.is_some(),
            touched,
            id,
        })
    }
    async fn conversation_lines(&self, id: &str) -> Result<Vec<TranscriptLine>, RuntimeError> {
        let Some(store) = &self.0.options.conversations else { return Ok(vec![]) };
        let ConversationMatch::One(row) = store.find(id).await? else { return Ok(vec![]) };
        Ok(store.load(&row.place, id).await?.map(|kept| transcript_of(&kept.checkpoint.messages)).unwrap_or_default())
    }
    async fn conversation_to_export(&self) -> Option<ConversationToExport> {
        let saving = self.0.saving.borrow().clone();
        let _ = saving.await;
        let (place, id, name, settled, first, turns, changes, set, words) = {
            let s = self.0.state.borrow();
            (
                s.place.clone(),
                s.conversation_id.clone(),
                s.conversation_name.clone(),
                s.settled.as_ref().map(|settled| KernelCheckpoint::clone(&settled.checkpoint)),
                s.conversation_first.clone().filter(|first| !first.is_empty()),
                s.conversation_turns,
                s.changes.clone(),
                s.set_name.clone().filter(|_| s.project.is_some()),
                s.integration.as_ref().map(|i| i.words()).unwrap_or(DAW_WORDS),
            )
        };
        // A kept one's row says what it's called, when it began and its Set; a chat that keeps none (without Live) has
        // only what this session holds.
        let row = match (&self.0.options.conversations, &place) {
            (Some(store), Some(place)) => match store.find(&id).await {
                Ok(ConversationMatch::One(row)) if row.place == *place => Some(row),
                _ => None,
            },
            _ => None,
        };
        // What the session holds is the whole of it; what's kept may have lost its start, or be all there is (a
        // conversation taken up and not yet answered in this run).
        let conversation = match settled.filter(|checkpoint| !checkpoint.messages.is_empty()) {
            // Saved last when its row says (the save under way is done); never saved (a chat without Live), 0.
            Some(checkpoint) => SavedConversation {
                saved_at: row.as_ref().map(|row| row.saved_at).unwrap_or(0),
                checkpoint,
                changes: (!changes.is_empty()).then_some(changes),
                first: first.clone(),
                turns: Some(turns),
            },
            None => {
                let store = self.0.options.conversations.clone()?;
                store.load(place.as_deref()?, &id).await.ok()??
            }
        };
        if conversation.checkpoint.messages.is_empty() {
            return None;
        }
        let title = match &row {
            Some(row) if name.is_none() => row.title(),
            _ => crate::core::conversations::title(name.as_deref(), first.as_deref().or(conversation.first.as_deref()).unwrap_or("")),
        };
        Some(ConversationToExport {
            id,
            title,
            set: row.as_ref().and_then(|row| row.set.clone()).or(set),
            created: row.map_or(0, |row| row.created),
            conversation,
            words,
        })
    }
    fn has_watch(&self) -> bool {
        true
    }
    fn watch(&self, event: WatchEvent) {
        let mut change = match event {
            WatchEvent::Audition(event) => {
                let run = self.0.state.borrow().matching.clone();
                if let Some(run) = run {
                    run.borrow_mut().auditioned(event.clone(), event.request.clone());
                }
                self.0.state.borrow_mut().heard_last = Some(event);
                return;
            }
            WatchEvent::Action(_) => return,
            WatchEvent::Judged(round) => {
                use kumi_listening::round::RoundKind;
                let mut s = self.0.state.borrow_mut();
                if round.kind == RoundKind::Judged {
                    s.judged_changes += 1;
                    if round.kept == Some(true) {
                        // A reading the change hid (or that wasn't there before) closed nothing.
                        s.judged_closed += round
                            .rows
                            .iter()
                            .filter(|row| row.before.is_some() && row.after.is_some())
                            .map(|row| row.gap_before - row.gap_after)
                            .sum::<f64>();
                    }
                }
                // The model judging a change in a turn of its own starts the loop: Kumi judged the request needs it.
                // A run that only measured doesn't; its first listen joins the loop if one starts. Nor does a groove
                // round (judged on the notes, never listened to): the loop's words send the model to the judge.
                if s.looping.is_none() && s.active.is_some() && s.matching.is_none() && round.listens > 0 {
                    match round.kind {
                        RoundKind::Start => s.judge_start = Some(round.clone()),
                        RoundKind::Judged => {
                            let request = s.turn_request.clone().unwrap_or_default();
                            let mut run = crate::core::loop_run::LoopRun::new(request, loop_budget(s.objective_until));
                            run.steers = s.turn_steers;
                            if let Some(start) = s.judge_start.take() {
                                run.judged(start);
                            }
                            s.looping = Some(Rc::new(RefCell::new(run)));
                        }
                        RoundKind::Done => s.judge_start = None,
                    }
                }
                if let Some(run) = s.looping.as_ref().filter(|_| round.listens > 0) {
                    run.borrow_mut().judged(round.clone());
                }
                s.judged_rounds += 1;
                s.judged_last = Some(round);
                return;
            }
            WatchEvent::Change(c) => {
                // A change told again as it was (its picture came after it) is no new change in the Set.
                let again = self.0.state.borrow().seen.get(&c.id).is_some_and(|seen| seen.state == c.state);
                if c.state == ChangeState::Applied && !again {
                    self.0.state.borrow_mut().applied += 1;
                    let run = self.0.state.borrow().matching.clone();
                    if let Some(run) = run {
                        run.borrow_mut().changed();
                    }
                }
                c
            }
        };
        if let Some(l) = &self.0.learned {
            l.drafts.change(change.clone());
        }
        change.clip = None;
        change.devices = None;
        if let Some(taste) = &self.0.taste {
            taste.change(&change);
        }
        {
            let mut s = self.0.state.borrow_mut();
            s.seen.insert(change.id.clone(), change.clone());
            // Trimmed a thousand at a time: shifting the oldest out one by one is a walk of the whole map per event.
            if s.seen.len() > 21_000 {
                s.seen.drain(..1_000);
            }
            // Only the newest changes keep their pictures here; the conversation keeps its own (`changes`).
            if let Some((_, older)) = s.seen.iter_mut().rev().nth(500) {
                older.picture = None;
            }
            if let Some(previous) = s.changes.iter_mut().find(|c| c.id == change.id) {
                *previous = change;
            } else {
                s.changes.push(change);
            }
            if s.changes.len() > 2000 {
                let excess = s.changes.len() - 2000;
                s.changes.drain(..excess);
            }
        }
    }
    async fn undo(&self, id: Option<&str>) -> Result<Option<ChangeRecord>, RuntimeError> {
        if !self.0.state.borrow().started {
            return Err(RuntimeError::plain("Session is not started"));
        }
        let outcome = Rc::new(RefCell::new(None));
        let put = outcome.clone();
        let id = id.map(str::to_owned);
        self.perform(
            false,
            Phase::Undo,
            Box::new(move |this, op| {
                async move {
                    let integration = this
                        .0
                        .state
                        .borrow()
                        .integration
                        .clone()
                        .filter(|i| i.has_undo())
                        .ok_or_else(|| KumiError::new(FailureKind::Request, "There's nothing Kumi can undo here."))?;
                    *put.borrow_mut() = Some(integration.undo(id.as_deref(), op.signal.clone()).await?);
                    Ok(None)
                }
                .boxed_local()
            }),
            None,
            None,
        )?
        .await?;
        let result = outcome.borrow_mut().take();
        if let (Some(taste), Some(record)) = (&self.0.taste, &result) {
            if record.state == ChangeState::Undone {
                taste.undone(&record.id, UndoneBy::Producer);
            }
        }
        Ok(result)
    }
    fn has_library(&self) -> bool {
        self.0.options.library.is_some()
    }
    fn library(&self) -> Option<LibraryStatus> {
        self.0.options.library.as_ref().map(|library| library.status())
    }
    async fn remembered(&self) -> Option<crate::core::search_memory::SearchMemoryStatus> {
        self.0.search_memory.as_ref()?.status().await.ok()
    }
    fn settings_memory(&self) -> Option<Rc<dyn kumi_listening::heard_settings::SettingsMemory>> {
        self.0.search_memory.clone().map(|memory| memory as Rc<dyn kumi_listening::heard_settings::SettingsMemory>)
    }
    fn music(&self) -> Option<Rc<kumi_listening::music::learning::Music>> {
        self.0.options.music.clone()
    }
    fn slots_context(&self) -> Option<Rc<kumi_listening::slots::SlotsContext>> {
        self.0.options.slots.clone()
    }
    fn map_loader(&self) -> Option<kumi_library::map::LoadMap> {
        self.0.options.map.clone()
    }
    fn model_controls(&self) -> Option<Rc<dyn kumi_ai::control::ModelController>> {
        self.0.options.models.clone()
    }
    fn open_set(&self) -> Option<String> {
        self.0.state.borrow().integration.as_ref().and_then(|integration| integration.open_set())
    }
    fn has_taste(&self) -> bool {
        self.0.options.library.is_some()
    }
    async fn taste(&self) -> Result<Vec<KeptLine>, RuntimeError> {
        match &self.0.options.library {
            Some(library) => Ok(library.taste().await?.into_iter().map(|line| KeptLine { id: line.id, line: line.line }).collect()),
            None => Ok(vec![]),
        }
    }
    fn has_forget_taste(&self) -> bool {
        self.0.options.library.is_some()
    }
    async fn forget_taste(&self, id: &str) -> Result<bool, RuntimeError> {
        let forgot = match &self.0.options.library {
            Some(library) => library.forget_taste(id).await?,
            None => false,
        };
        self.changed_system(forgot);
        Ok(forgot)
    }
    fn has_memory(&self) -> bool {
        true
    }
    async fn memory(&self) -> Result<Option<MemoryView>, RuntimeError> {
        let Some(store) = &self.0.options.memory else {
            return Ok(None);
        };
        let (project, set_name) = {
            let s = self.0.state.borrow();
            (s.project.clone(), s.set_name.clone())
        };
        Ok(Some(MemoryView {
            memory: store.load(project.as_deref()).await?,
            set_name: set_name.filter(|s| !s.is_empty()),
            saved: project.is_some(),
        }))
    }
    fn has_forget(&self) -> bool {
        true
    }
    async fn forget(&self, id: &str) -> Result<Option<MemoryNote>, RuntimeError> {
        let forgot = if let Some(notes) = &self.0.notes { notes.forget(id).await? } else { None };
        self.changed_system(forgot.is_some());
        Ok(forgot)
    }
    fn has_change_note(&self) -> bool {
        true
    }
    async fn change_note(&self, id: &str, change: NoteChange) -> Result<Option<MemoryNote>, RuntimeError> {
        let changed = if let Some(notes) = &self.0.notes { notes.change(id, change).await? } else { None };
        self.changed_system(changed.is_some());
        Ok(changed)
    }
    fn has_recipes(&self) -> bool {
        true
    }
    async fn recipes(&self) -> Result<Vec<RecipeSummary>, RuntimeError> {
        let Some(store) = &self.0.options.recipes else {
            return Ok(vec![]);
        };
        // Only this DAW's recipes: another's steps are its own tools'.
        let daw = self.0.options.profile.id.unwrap_or(DawId::Ableton);
        Ok(store
            .list()
            .await?
            .into_iter()
            .filter(|r| r.daw() == daw)
            .map(|r| RecipeSummary {
                name: r.name,
                about: r.about,
                params: r.params.into_iter().map(|p| RecipeParam { name: p.name, about: p.about }).collect(),
                steps: r.steps.len(),
                used: r.used,
                created: r.created,
                last_used: r.last_used.filter(|n| *n != 0.0),
            })
            .collect())
    }
    fn pin_for_recipe(&self, pin: &PinnedNode) -> Option<PinnedNode> {
        let integration = self.0.state.borrow().integration.clone();
        integration.map_or_else(|| Some(pin.clone()), |integration| integration.pin_for_recipe(pin))
    }
    fn has_run_recipe(&self) -> bool {
        true
    }
    fn has_instant(&self) -> bool {
        true
    }
    async fn instant(&self, text: &str) -> Result<Option<InstantOutcome>, RuntimeError> {
        let Some(intent) = crate::core::phrases::parse(text) else { return Ok(None) };
        let integration = self.0.state.borrow().integration.clone();
        if !self.0.state.borrow().started || !integration.as_ref().is_some_and(|integration| integration.has_instant()) {
            return Ok(None);
        }
        let outcome = Rc::new(RefCell::new(None));
        let put = outcome.clone();
        // As a recipe runs: a fresh look for current references, then the change, with no model call.
        self.perform(
            false,
            Phase::Refresh,
            Box::new(move |this, op| {
                async move {
                    this.observe(&op, None, false).await?;
                    let integration = this.0.state.borrow().integration.clone();
                    if let Some(integration) = integration {
                        *put.borrow_mut() = integration.instant(&intent, op.signal.clone()).await?;
                    }
                    Ok(None)
                }
                .boxed_local()
            }),
            Some(self.0.turn_limit_ms),
            None,
        )?
        .await?;
        let done = outcome.borrow_mut().take();
        Ok(done)
    }
    async fn run_recipe(&self, name: &str, with: JsonObject) -> Result<RecipeOutcome, RuntimeError> {
        if !self.0.state.borrow().started {
            return Err(RuntimeError::plain("Session is not started"));
        }
        let Some(run) = self.0.recipes.iter().find(|t| t.name() == RUN_RECIPE_TOOL).cloned() else {
            return Ok(RecipeOutcome { text: "Kumi keeps no recipes here.".into(), is_error: true });
        };
        let name = name.to_owned();
        let with = Value::Object(with);
        let outcome = Rc::new(RefCell::new(None));
        let put = outcome.clone();
        self.perform(
            false,
            Phase::Refresh,
            Box::new(move |this, op| {
                async move {
                    this.observe(&op, None, false).await?;
                    let result =
                        run.execute(json!({"name":name,"with":with,"final":true}).as_object().unwrap().clone(), op.signal.clone()).await?;
                    *put.borrow_mut() = Some(RecipeOutcome { text: result.reply.unwrap_or(result.text), is_error: result.is_error });
                    Ok(None)
                }
                .boxed_local()
            }),
            Some(self.0.turn_limit_ms),
            None,
        )?
        .await?;
        let result = outcome
            .borrow_mut()
            .take()
            .filter(|r| !r.text.is_empty())
            .unwrap_or(RecipeOutcome { text: "it didn't finish; anything it changed is in HISTORY.".into(), is_error: true });
        Ok(result)
    }
    fn has_forget_recipe(&self) -> bool {
        true
    }
    async fn forget_recipe(&self, name: &str) -> Result<bool, RuntimeError> {
        let Some(store) = &self.0.options.recipes else {
            return Ok(false);
        };
        // (this DAW's: the panel lists only its recipes)
        let daw = self.0.options.profile.id.unwrap_or(DawId::Ableton);
        let Some(recipe) = store.get_in(name, daw).await? else {
            return Ok(false);
        };
        if !store.remove_in(&recipe.name, daw).await? {
            return Ok(false);
        }
        self.changed_system(true);
        self.emit(SessionEvent::Recipe(RecipeEvent { action: RecipeAction::Forgotten, name: recipe.name, steps: recipe.steps.len() }));
        Ok(true)
    }
    fn has_techniques(&self) -> bool {
        true
    }
    async fn techniques(&self) -> Result<Vec<TechniqueSummary>, RuntimeError> {
        let Some(learned) = &self.0.learned else {
            return Ok(vec![]);
        };
        Ok(learned
            .list()
            .await?
            .into_iter()
            .rev()
            .map(|t| TechniqueSummary {
                id: t.id,
                name: t.body.name,
                fits: t.body.fits,
                source: t.body.source.and_then(|s| s.title).filter(|s| !s.is_empty()),
            })
            .collect())
    }
    fn has_forget_technique(&self) -> bool {
        true
    }
    async fn forget_technique(&self, id: &str) -> Result<bool, RuntimeError> {
        let forgot = if let Some(l) = &self.0.learned { l.forget(id).await?.is_some() } else { false };
        self.changed_system(forgot);
        Ok(forgot)
    }
    fn has_answer_technique(&self) -> bool {
        self.0.learned.is_some()
    }
    async fn answer_technique(&self, keep: bool) -> Result<bool, RuntimeError> {
        match &self.0.learned {
            Some(l) => l.drafts.answer(keep).await,
            None => Ok(false),
        }
    }
    fn picked(&self, pick: Picked) {
        if let Some(taste) = &self.0.taste {
            taste.picked(&pick);
        }
    }
    fn has_goal(&self) -> bool {
        true
    }
    async fn goal(&self, text: Option<&str>) -> Result<(), RuntimeError> {
        let words = text.map(trim).filter(|s| !s.is_empty()).map(str::to_owned);
        {
            let s = self.0.state.borrow();
            if s.state == TurnState::Closed {
                return Err(RuntimeError::plain("Session is closed"));
            }
            if s.active.is_some() {
                return Err(RuntimeError::plain("Session is busy; cancel first"));
            }
            if !s.started {
                return Err(RuntimeError::plain("Session is not started"));
            }
        }
        if words.as_ref().is_some_and(|s| s.len() > 4096) {
            return Err(RuntimeError::plain("Say the goal in at most 4 KiB"));
        }
        match self.objective_command(words).await? {
            objective::GoalCommand::Show(Some(objective)) => {
                self.emit_objective(&objective, objective.elapsed_ms);
                Ok(())
            }
            objective::GoalCommand::Show(None) => {
                self.emit(SessionEvent::ObjectiveCleared);
                self.notice("No goal yet: /goal and what to reach, such as /goal master this to -9 LUFS with the vocal cutting through.");
                Ok(())
            }
            objective::GoalCommand::Say(text) => {
                self.notice(text);
                Ok(())
            }
            objective::GoalCommand::Run(objective, fresh) => {
                {
                    let mut s = self.0.state.borrow_mut();
                    s.turns += 1;
                    s.interrupted = None;
                }
                // A goal set is the producer's request, as they said it; one picked up again starts a turn without one.
                if let Some(log) = &self.0.options.log {
                    let (turns, had) = {
                        let s = self.0.state.borrow();
                        (s.conversation_turns, s.settled.as_ref().map(|settled| settled.checkpoint.clone()))
                    };
                    let said = text.map(trim).filter(|words| !words.is_empty()).map(|words| format!("/goal {words}")).unwrap_or_default();
                    log.request(&said, vec![], None, turns, had);
                }
                // Offered again after Live comes back, it carries the goal on (its turns and budget kept).
                let input = "/goal resume".to_string();
                self.perform(
                    true,
                    Phase::Refresh,
                    Box::new(move |this, op| async move { this.run_objective(op, objective, fresh).await }.boxed_local()),
                    None,
                    Some(input),
                )?
                .await
            }
        }
    }
    fn has_stop_goal(&self) -> bool {
        true
    }
    async fn stop_goal(&self) -> Result<bool, RuntimeError> {
        // The objective first; with none, a sound-match search stops the same way.
        if self.stop_objective().await? {
            return Ok(true);
        }
        self.stop_goal_inner().await
    }
    fn has_stop_loop(&self) -> bool {
        true
    }
    async fn stop_loop(&self) -> Result<bool, RuntimeError> {
        // A running judged loop (a goal's too) stops as Esc stops it, first; a sound-match search, running or paused,
        // ends for good.
        let running = {
            let s = self.0.state.borrow();
            let searching = s.goal_op.as_ref().is_some_and(|op| s.active.as_ref().is_some_and(|a| a.id == op.id));
            s.active.clone().filter(|op| !searching && op.is_turn && (s.looping.is_some() || s.objective_op.is_some()))
        };
        if let Some(op) = running {
            op.signal.cancel();
            let _ = op.done.clone().await;
            return Ok(true);
        }
        self.stop_goal_inner().await
    }
    fn has_goal_status(&self) -> bool {
        true
    }
    fn goal_status(&self) -> Option<GoalStatus> {
        self.0.state.borrow().goal_status.clone()
    }
    fn has_lessons(&self) -> bool {
        true
    }
    async fn lessons(&self) -> Result<Vec<LessonEntry>, RuntimeError> {
        Ok(self
            .playbook_serial(|s| async move { s.list().await }.boxed_local())
            .await
            .unwrap_or_default()
            .into_iter()
            .rev()
            .map(|l| LessonEntry { line: crate::core::playbook::lesson_line(&l), id: l.id, at: l.at })
            .collect())
    }
    fn has_forget_lesson(&self) -> bool {
        true
    }
    async fn forget_lesson(&self, id: &str) -> Result<bool, RuntimeError> {
        let this = self.clone();
        let id = id.to_owned();
        Ok(self
            .playbook_serial(move |store| {
                async move {
                    let Some(gone) = store.forget(&id).await? else {
                        return Ok(false);
                    };
                    this.emit(SessionEvent::Lesson { action: LessonAction::Forgot, id, line: crate::core::playbook::lesson_line(&gone) });
                    Ok(true)
                }
                .boxed_local()
            })
            .await
            .unwrap_or(false))
    }
    fn has_stop_live(&self) -> bool {
        true
    }
    async fn stop_live(&self) -> Result<bool, RuntimeError> {
        let integration = self.0.state.borrow().integration.clone();
        let stopped = if let Some(i) = integration.filter(|i| i.has_stop_live()) {
            i.stop_live(abort::timeout(self.0.options.timeout_ms.unwrap_or(30_000))).await.unwrap_or(false)
        } else {
            false
        };
        if stopped {
            self.emit(SessionEvent::Action(ActionEvent { title: "Stopped".into(), playing: Some(false), recording: Some(false) }));
        }
        Ok(stopped)
    }
    fn answering_fallback(&self) -> Option<String> {
        self.0.state.borrow().kernel.as_ref().and_then(|held| held.value.answering_fallback())
    }
    fn has_daw_message(&self) -> bool {
        true
    }
    async fn daw_message(&self, text: &str) -> Result<bool, RuntimeError> {
        let integration = self.0.state.borrow().integration.clone();
        match integration.filter(|i| i.has_show_message()) {
            Some(i) => i.show_message(text, abort::timeout(5_000)).await,
            None => Ok(false),
        }
    }
    async fn daw_recording(&self) -> Result<Option<bool>, RuntimeError> {
        let integration = self.0.state.borrow().integration.clone();
        match integration {
            Some(i) => i.recording(abort::timeout(5_000)).await,
            None => Ok(None),
        }
    }
    fn has_palette(&self) -> bool {
        self.0.palettes.is_some()
    }
    async fn palette(&self) -> Result<Option<crate::core::palette::PaletteView>, RuntimeError> {
        match &self.0.palettes {
            Some(palettes) => palettes.view().await,
            None => Ok(None),
        }
    }
    async fn palette_gather(&self, files: Vec<crate::core::palette::Gathered>) -> Result<String, RuntimeError> {
        let palettes = self.0.palettes.clone().ok_or_else(|| RuntimeError::plain("Kumi keeps no palettes here."))?;
        Ok(palettes.gather(files).await?.words())
    }
    async fn palette_remove(&self, path: &str, size: i64, mtime: i64) -> Result<bool, RuntimeError> {
        match self.0.palettes.clone() {
            Some(palettes) => palettes.remove(path, size, mtime).await,
            None => Ok(false),
        }
    }
    async fn palette_command(&self, words: &str) -> Result<String, RuntimeError> {
        let palettes = self.0.palettes.clone().ok_or_else(|| RuntimeError::plain("Kumi keeps no palettes here."))?;
        palettes.command(words).await
    }
    fn has_jobs(&self) -> bool {
        self.0.jobs.is_some()
    }
    fn jobs(&self) -> Vec<crate::core::jobs::JobStatus> {
        self.0.jobs.as_ref().map(|jobs| jobs.list()).unwrap_or_default()
    }
    fn job(&self, id: u32) -> Option<crate::core::jobs::JobDetail> {
        self.0.jobs.as_ref().and_then(|jobs| jobs.detail(id))
    }
    fn stop_job(&self, id: u32) -> bool {
        self.0.jobs.as_ref().is_some_and(|jobs| jobs.stop(id))
    }
    fn run_job_again(&self, id: u32) -> Result<u32, String> {
        let jobs = self.0.jobs.clone().ok_or("Kumi can't run background jobs here.")?;
        let spec = jobs
            .spec(id)
            .filter(|_| jobs.status(id).is_some_and(|status| status.state.ended()))
            .filter(|spec| spec.task.is_some() || spec.tool.is_some())
            .ok_or("That job can't run again.")?;
        let again = self.start_job(spec)?;
        jobs.forget(id);
        Ok(again)
    }
    fn background(&self) -> Result<u32, String> {
        Session::background(self)
    }
    fn carry_on(&self, job: u32) -> bool {
        Session::carry_on(self, job)
    }
    fn can_background(&self) -> bool {
        self.0.state.borrow().foreground.iter().any(|step| step.waiting())
    }
    fn has_device_tree(&self) -> bool {
        true
    }
    async fn device_tree(&self, track_ref: &str) -> Result<Option<DeviceTree>, RuntimeError> {
        let i = {
            let s = self.0.state.borrow();
            s.integration.clone().filter(|i| s.connection == ConnectionState::Connected && i.has_device_tree())
        };
        Ok(if let Some(i) = i { i.device_tree(track_ref, abort::timeout(10_000)).await.ok().flatten() } else { None })
    }
    fn has_clip_view(&self) -> bool {
        true
    }
    async fn clip_view(&self, slot_ref: &str) -> Result<Option<ClipView>, RuntimeError> {
        let i = {
            let s = self.0.state.borrow();
            s.integration.clone().filter(|i| s.connection == ConnectionState::Connected && i.has_clip_view())
        };
        Ok(if let Some(i) = i { i.clip_view(slot_ref, abort::timeout(10_000)).await.ok().flatten() } else { None })
    }
    fn has_session_strip(&self) -> bool {
        true
    }
    async fn session_strip(&self, track_ref: &str, scene: f64) -> Result<Option<SessionStrip>, RuntimeError> {
        let i = {
            let s = self.0.state.borrow();
            s.integration.clone().filter(|i| s.connection == ConnectionState::Connected && i.has_session_strip())
        };
        Ok(if let Some(i) = i { i.session_strip(track_ref, scene, abort::timeout(10_000)).await.ok().flatten() } else { None })
    }
    fn has_arrangement_strip(&self) -> bool {
        true
    }
    fn can_show(&self, kind: BodyKind) -> bool {
        match kind {
            // Live's four, as the session always offered them.
            BodyKind::Tree | BodyKind::Notes | BodyKind::Grid | BodyKind::Timeline => true,
            // Another DAW's own (a tracker's pattern, its matrix, keyzones, a track's routing), when its integration draws them.
            _ => self.0.state.borrow().integration.as_ref().is_some_and(|i| i.can_show(kind)),
        }
    }
    async fn arrangement_strip(&self) -> Result<Option<ArrangementStrip>, RuntimeError> {
        let i = {
            let s = self.0.state.borrow();
            s.integration.clone().filter(|i| s.connection == ConnectionState::Connected && i.has_arrangement_strip())
        };
        Ok(if let Some(i) = i { i.arrangement_strip(abort::timeout(10_000)).await.ok().flatten() } else { None })
    }
    async fn focus_body(&self, request: &BodyRequest) -> Result<Option<FocusBody>, RuntimeError> {
        let i = {
            let s = self.0.state.borrow();
            s.integration.clone().filter(|i| s.connection == ConnectionState::Connected && i.can_show(request.kind()))
        };
        Ok(if let Some(i) = i { i.focus_body(request, abort::timeout(10_000)).await.ok().flatten() } else { None })
    }
    async fn cancel(&self) -> Result<(), RuntimeError> {
        let op = self.0.state.borrow().active.clone();
        if let Some(op) = op {
            op.signal.cancel();
            op.done.clone().await?;
        }
        Ok(())
    }
    fn has_reconfigure(&self) -> bool {
        true
    }
    async fn reconfigure(&self) -> Result<(), RuntimeError> {
        let mut s = self.0.state.borrow_mut();
        if s.state == TurnState::Closed {
            return Err(RuntimeError::plain("Session is closed"));
        }
        s.rebuild = true;
        Ok(())
    }
    async fn close(&self) -> Result<(), RuntimeError> {
        let closing = self.0.state.borrow().closing.clone();
        if let Some(closing) = closing {
            return closing.await;
        }
        let (tx, rx) = oneshot::channel();
        let closing = async { rx.await.unwrap_or(Ok(())) }.boxed_local().shared();
        let op = {
            let mut s = self.0.state.borrow_mut();
            s.closing = Some(closing.clone());
            s.state = TurnState::Closed;
            if let Some(m) = s.missing.take() {
                m.cancel();
            }
            s.active.clone()
        };
        (self.0.options.on_event)(SessionEvent::State { state: TurnState::Closed });
        if let Some(op) = &op {
            op.signal.cancel();
        }
        if let Some(jobs) = &self.0.jobs {
            jobs.interrupt_all();
        }
        if let Some(unlisten) = self.0.unlisten_library.borrow_mut().take() {
            unlisten();
        }
        if let Some(unlisten) = self.0.unlisten_music.borrow_mut().take() {
            unlisten();
        }
        let this = self.clone();
        tokio::task::spawn_local(async move {
            if let Some(op) = op {
                let _ = this.bounded(op.done.clone()).await;
            }
            if let Some(l) = &this.0.learned {
                l.drafts.close().await;
            }
            let saving = this.0.saving.borrow().clone();
            let _ = this.bounded(saving).await;
            // Its log is all on the disk before another Kumi may take the conversation up.
            if let Some(log) = this.0.options.log.clone() {
                let _ = this
                    .bounded(async move {
                        log.close().await;
                        Ok(())
                    })
                    .await;
            }
            // Another Kumi may take this conversation up now.
            let held = {
                let s = this.0.state.borrow();
                s.place.clone().map(|place| (place, s.conversation_id.clone()))
            };
            if let (Some(store), Some((place, id))) = (&this.0.options.conversations, held) {
                store.release(&place, &id);
            }
            let kept = this.clone();
            let _ = this
                .bounded(async move {
                    kept.keep_jobs().await;
                    Ok(())
                })
                .await;
            let result = this.drop_resources().await;
            let _ = tx.send(result);
        });
        closing.await
    }
    fn status(&self) -> SessionStatus {
        let s = self.0.state.borrow();
        SessionStatus {
            state: s.state,
            connection: s.connection,
            turns: s.turns,
            max_turns: self.0.options.max_turns,
            observation: s.observation.clone(),
        }
    }
}
