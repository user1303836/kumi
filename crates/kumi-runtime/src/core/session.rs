//! Session ownership, cancellation, context refresh and conversation recovery.

use super::{
    contracts::*,
    errors::{FailureKind, KumiError, RuntimeError},
    file_sync::FileSync,
    gaps::{gap_tools, GAP_GUIDANCE},
    memory::{memory_instructions, memory_tools, MemoryTools, MemoryToolsOptions},
    recall::{recall_tool, RecallOptions},
    recipes::{recipe_instructions, recipe_tools, RecipeStore, RecipeToolsOptions, RUN_RECIPE_TOOL},
    store_client::StoreClient,
    taste_log::{TasteLog, UndoneBy, Whereabouts},
    techniques::{
        technique_instructions, technique_tools, TechniqueStore, TechniqueTools, TechniqueToolsOptions, PLAN_TECHNIQUE, TECHNIQUE_GUIDANCE,
    },
    timing,
};
use crate::{
    audio::tools::{listening_tools, ListeningOptions},
    integrations::ableton::{
        project::new_conversation_id,
        willington::{willington_instructions, WillingtonSwitch},
    },
    kernel::budget::{transcript_of, OBSERVATION_MARKER},
    library::{Library, LibraryToolsOptions, FIND_SOUNDS_TOOL},
    web::{
        net::WebClient,
        tool::{web_tools, WebToolOptions},
    },
};
use async_trait::async_trait;
use futures::{
    future::{LocalBoxFuture, Shared},
    FutureExt,
};
use kumi_common::{
    abort::{self, Signal},
    js::string::{head, trim},
};
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    future::Future,
    rc::Rc,
    time::Duration,
};
use tokio::{
    sync::{oneshot, Notify},
    time::Instant,
};

mod goals;
mod matching;
use super::goal::{GoalBudget, GoalState, GoalStatus, GoalStore};
use super::{
    match_run::{MatchBudget, MatchRun},
    playbook::PlaybookStore,
};

const UNSAVED: &str = "unsaved";
const STILL_MISSING: &str =
    "Kumi can't reach Live. Is it open, with AbletonMcpBridge chosen as a Control Surface (Settings → Link, Tempo & MIDI)?";
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
    pub missing_after_ms: Option<u64>,
    pub memory: Option<Rc<dyn MemoryStore>>,
    pub listen: bool,
    pub recipes: Option<Rc<dyn RecipeStore>>,
    pub watch: Option<VideoDirectories>,
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
    pub matching: bool,
    pub match_budget: Option<MatchBudget>,
    pub playbook: Option<Rc<dyn PlaybookStore>>,
    pub goal_random: Option<Rc<dyn Fn() -> f64>>,
    pub goals: Option<Rc<dyn GoalStore>>,
    pub goal_budget: Option<GoalBudget>,
    /// How Willington's bindings stand, None while Kumi's bridge in Live doesn't carry them: asked each time
    /// the model's session is made, since /willington switches them while Kumi runs.
    pub willington: Option<Rc<dyn Fn() -> Option<WillingtonSwitch>>>,
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
            missing_after_ms: None,
            memory: None,
            listen: false,
            recipes: None,
            watch: None,
            web: false,
            web_client: None,
            techniques: None,
            gaps: None,
            store: None,
            files: None,
            timings: None,
            library: None,
            matching: true,
            match_budget: None,
            playbook: None,
            goal_random: None,
            goals: None,
            goal_budget: None,
            willington: None,
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
        self.quiet.set((self.working.get() == 0 && !self.steady.get()).then(|| Instant::now() + Duration::from_millis(self.idle_ms)));
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
    checkpoint: KernelCheckpoint,
    key: String,
}
struct Chosen {
    place: String,
    id: String,
    conversation: SavedConversation,
}
struct State {
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
    state: TurnState,
    connection: ConnectionState,
    observation: Option<String>,
    integration: Option<Rc<dyn Integration>>,
    generation: u64,
    kernel: Option<Held>,
    must_reset: bool,
    rebuild: bool,
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
    changes: Vec<ChangeRecord>,
    seen: indexmap::IndexMap<String, ChangeRecord>,
    settled: Option<Settled>,
    carry: Option<(KernelCheckpoint, Option<String>)>,
    start_fresh: bool,
    chosen: Option<Chosen>,
    project: Option<String>,
    set: Option<String>,
    set_name: Option<String>,
    plan: Option<Rc<dyn KernelTool>>,
    turns: u32,
    next_operation: u64,
    active: Option<Rc<Operation>>,
    closing: Option<Done>,
    started: bool,
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
    timings: Option<String>,
    shelf: Vec<Rc<dyn KernelTool>>,
    /// Answers begun in this session: tools that count per answer (watch_video) read it.
    answers: Rc<Cell<u64>>,
    unlisten_library: RefCell<Option<Box<dyn FnOnce()>>>,
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
fn ready() -> Done {
    async { Ok(()) }.boxed_local().shared()
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
    let timeout_ms = options.timeout_ms.unwrap_or(120_000);
    let idle_ms = options.idle_timeout_ms.or(options.timeout_ms).unwrap_or(600_000);
    let turn_limit_ms = options.turn_limit_ms.unwrap_or(3_600_000);
    let close_ms = options.close_timeout_ms.unwrap_or(5_000);
    let grace_ms = options.cancel_grace_ms.unwrap_or(500);
    let timings = options.timings.clone();
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
            memory_tools(MemoryToolsOptions {
                store: store.clone(),
                project: Rc::new(move || project.upgrade().and_then(|i| i.state.borrow().project.clone())),
                set: Some(Rc::new(move || set.upgrade().and_then(|i| i.state.borrow().set.clone()))),
                on_event: Rc::new(move |e| emit(e.into())),
            })
        });
        let recipes = options
            .recipes
            .as_ref()
            .map(|store| {
                let weak = weak.clone();
                let emit = emit.clone();
                recipe_tools(RecipeToolsOptions {
                    store: store.clone(),
                    plan: Rc::new(move || weak.upgrade().and_then(|i| i.state.borrow().plan.clone())),
                    on_event: Rc::new(move |e| emit(SessionEvent::Recipe(e))),
                })
            })
            .unwrap_or_default();
        let recall = options.conversations.as_ref().map(|store| {
            let weak = weak.clone();
            recall_tool(RecallOptions {
                conversations: Some(store.clone()),
                techniques: options.techniques.clone(),
                recipes: options.recipes.clone(),
                current: Rc::new(move || {
                    let inner = weak.upgrade()?;
                    let s = inner.state.borrow();
                    Some((s.place.clone()?, s.conversation_id.clone()))
                }),
            })
        });
        let learned = options.techniques.as_ref().map(|store| {
            let emit = emit.clone();
            Rc::new(technique_tools(TechniqueToolsOptions {
                store: store.clone(),
                on_event: Rc::new(move |e| emit(SessionEvent::Technique(e))),
            }))
        });
        let listening = if options.listen {
            let resolve = weak.clone();
            let hear = weak.clone();
            let emit = emit.clone();
            listening_tools(ListeningOptions {
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
                    let integration = hear.upgrade().and_then(|i| i.state.borrow().integration.clone());
                    async move {
                        match integration {
                            Some(i) if i.has_hear() => i.hear(&request, signal).await,
                            _ => Ok(Err("Kumi isn't connected to Live, so it can't hear the Set.".into())),
                        }
                    }
                    .boxed_local()
                })),
            })
        } else {
            vec![]
        };
        let answers = Rc::new(Cell::new(0));
        let watching = options
            .watch
            .as_ref()
            .map(|dirs| {
                let answers = answers.clone();
                crate::video::tool::video_tools(crate::video::tool::VideoToolOptions {
                    videos_dir: dirs.videos_dir.clone(),
                    tools_dir: dirs.tools_dir.clone(),
                    env: None,
                    on_event: emit.clone(),
                    answer: Some(Rc::new(move || answers.get())),
                })
            })
            .unwrap_or_default();
        let browsing = if options.web || options.web_client.is_some() {
            web_tools(WebToolOptions { on_event: emit.clone(), client: options.web_client.clone(), ..Default::default() })
        } else {
            vec![]
        };
        let gaps = options.gaps.as_ref().map(|file| gap_tools(file, options.store.clone())).unwrap_or_default();
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
                    Whereabouts { session, project, set: integration(&inner).and_then(|i| i.fingerprint()) }
                }),
                Rc::new(move |since| hear.upgrade().and_then(|inner| integration(&inner)).and_then(|i| i.first_heard(since))),
            )
        });
        let shelf = options
            .library
            .as_ref()
            .map(|library| {
                let resolve = weak.clone();
                library.tools(LibraryToolsOptions {
                    on_event: Some(emit.clone()),
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
                })
            })
            .unwrap_or_default();
        let unlisten_library = options
            .library
            .as_ref()
            .map(|library| library.on_status(Rc::new(move |status| emit(SessionEvent::Library(LibraryEvent { status })))));
        Inner {
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
            timings,
            shelf,
            answers,
            unlisten_library: RefCell::new(unlisten_library),
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
                state: TurnState::Idle,
                connection: ConnectionState::Disconnected,
                observation: None,
                integration: None,
                generation: 0,
                kernel: None,
                must_reset: false,
                rebuild: false,
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
                changes: vec![],
                seen: indexmap::IndexMap::new(),
                settled: None,
                carry: None,
                start_fresh: false,
                chosen: None,
                project: None,
                set: None,
                set_name: None,
                plan: None,
                turns: 0,
                next_operation: 0,
                active: None,
                closing: None,
                started: false,
            }),
        }
    })))
}

impl Session {
    fn emit(&self, event: SessionEvent) {
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
            self.notice(match cause {
                Some(DisconnectCause::Live) if working => "Live closed while Kumi was working in it. If it crashed, your unsaved changes may be lost: Live offers to recover them when it opens. Kumi picks up when Live is back.",
                Some(DisconnectCause::Live) => "Live closed. Kumi will pick up where you left off when it's back.",
                Some(DisconnectCause::Bridge) => "Kumi's link to Live dropped. It's reconnecting, and will pick up where you left off.",
                Some(DisconnectCause::AskedSet) => "Live is opening the Set; Kumi carries on once it's open.",
                Some(DisconnectCause::Set) if working => "Live is opening another Set, so Kumi stopped what it was doing: nothing it was doing lands in the other Set. It picks up once the Set is open.",
                Some(DisconnectCause::Set) => "Live is opening another Set; Kumi picks up once it's open.",
                Some(DisconnectCause::Busy) if working => "Live is busy and answering slowly, so Kumi is waiting for it: your request carries on once it answers.",
                Some(DisconnectCause::Busy) => "Live is busy and answering slowly; Kumi carries on once it answers.",
                None => "Kumi lost touch with Live. It will pick up where you left off when Live is back.",
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
                    if this.0.state.borrow().away { this.notice(STILL_MISSING); }
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
                    self.notice("Live opened another Set while it was busy, so Kumi stopped what it was doing: nothing it was doing lands in the other Set.");
                }
            }
            self.notice(match (again.is_some(), cause) {
                (_, Some(DisconnectCause::Set | DisconnectCause::AskedSet)) => "Live has the other Set open.",
                (_, Some(DisconnectCause::Busy)) => "Live is answering again.",
                (true, _) if crashed => "Live is back. Your last request stopped when Live closed, which it may have caused: check the Set before you send it again (enter sends it).",
                (true, _) => "Live is back. Your last request was stopped; press enter to send it again.",
                (false, _) => "Live is back.",
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
    fn begin(&self, place: String, id: String, conversation: Option<&SavedConversation>) {
        let mut s = self.0.state.borrow_mut();
        s.place = Some(place);
        s.conversation_id = id.clone();
        s.settled = None;
        s.conversation_first = conversation.and_then(|c| c.first.clone());
        s.conversation_turns = conversation.and_then(|c| c.turns).unwrap_or(0);
        s.changes = conversation
            .and_then(|c| c.changes.clone())
            .unwrap_or_default()
            .into_iter()
            .map(|mut change| {
                if let Some(ours) = s.seen.get(&change.id).filter(|c| c.at == change.at) {
                    return ours.clone();
                }
                if !change.id.contains(':') {
                    change.id = format!("{id}:{}", change.id);
                }
                if matches!(change.state, ChangeState::Applied | ChangeState::Unsure) {
                    change.state = ChangeState::Expired;
                    change.note = Some("From an earlier session, so Kumi can't undo it now.".into());
                }
                change
            })
            .collect();
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
            let memory = if let Some(store) = &self.0.options.memory {
                store.load(observation.project.as_ref().map(|p| p.id.as_str())).await.ok()
            } else {
                None
            };
            self.assert_current(op)?;
            let recipes = if let Some(store) = &self.0.options.recipes { store.list().await.unwrap_or_default() } else { vec![] };
            self.assert_current(op)?;
            let techniques = if let Some(learned) = &self.0.learned { learned.list().await.unwrap_or_default() } else { vec![] };
            self.assert_current(op)?;
            let habits =
                if let Some(library) = &self.0.options.library { library.instructions().await.unwrap_or_default() } else { String::new() };
            self.assert_current(op)?;
            let extra = [
                memory.as_ref().map(|m| memory_instructions(m, observation.project.as_ref().map(|p| p.name.as_str()))).unwrap_or_default(),
                habits,
                recipe_instructions(&recipes),
                if self.0.learned.is_some() { TECHNIQUE_GUIDANCE.into() } else { String::new() },
                technique_instructions(&techniques),
                if self.0.gaps.is_empty() { String::new() } else { GAP_GUIDANCE.into() },
                willington_instructions(self.0.options.willington.as_ref().and_then(|on| on()), |name| {
                    observation.tools.iter().any(|tool| tool.name() == name)
                })
                .into(),
            ]
            .into_iter()
            .filter(|s: &String| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");
            let mut tools: Vec<Rc<dyn KernelTool>> = observation
                .tools
                .iter()
                .filter(|tool| self.0.shelf.is_empty() || tool.name() != FIND_SOUNDS_TOOL)
                .map(|tool| self.with_taste(self.with_technique(tool.clone())))
                .collect();
            if let Some(notes) = &self.0.notes {
                tools.extend(notes.tools.clone());
            }
            tools.extend(self.0.listening.clone());
            tools.extend(self.0.watching.clone());
            tools.extend(self.0.browsing.clone());
            tools.extend(self.0.shelf.clone());
            tools.extend(self.0.recipes.clone());
            tools.extend(self.0.recall.clone());
            if let Some(learned) = &self.0.learned {
                tools.extend(learned.tools.clone());
            }
            tools.extend(self.0.gaps.clone());
            if let Some(taste) = &self.0.taste {
                tools.push(taste.tool());
            }
            let conversation = self.0.state.borrow().conversation_id.clone();
            let value = (self.0.options.kernel_factory)(KernelOptions {
                instructions: if extra.is_empty() {
                    observation.instructions.clone()
                } else {
                    format!("{}\n\n{extra}", observation.instructions)
                },
                tools,
                signal: lifetime.clone(),
                checkpoint,
                conversation: Some(conversation),
            })
            .await?;
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
                self.0.state.borrow_mut().switched = Some(Settled { checkpoint: h.value.checkpoint()?, key: h.key });
            }
            self.drop_kernel().await?;
            self.assert_current(op)?;
        }
        let held = self.0.state.borrow().kernel.clone();
        let must_reset = self.0.state.borrow().must_reset;
        let identity = held.as_ref().is_some_and(|h| h.key != observation.key);
        let revision = observation.revision.clone().unwrap_or_default();
        let mut from = None;
        if let Some(h) = held.as_ref().filter(|h| !identity && !must_reset && h.revision != revision) {
            if h.value.has_checkpoint() {
                from = Some(h.value.checkpoint()?);
                self.drop_kernel().await?;
                self.assert_current(op)?;
            }
        }
        let mut reason = None;
        let first = self.0.state.borrow().kernel.is_none() && !must_reset;
        if self.0.state.borrow().kernel.is_some() && (identity || must_reset || held.as_ref().is_some_and(|h| h.revision != revision)) {
            self.drop_kernel().await?;
            self.assert_current(op)?;
            reason = Some(if identity {
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
            let (fresh, chosen, switched, carry, settled) = {
                let mut s = self.0.state.borrow_mut();
                (s.start_fresh, s.chosen.take(), s.switched.clone(), s.carry.clone(), s.settled.clone())
            };
            let carry = carry
                .filter(|(_, place)| !(project.is_some() && place.is_some() && place.as_deref() != Some(UNSAVED) && place != &project));
            let mut resumed = None;
            let mut picked = false;
            if fresh {
                self.begin(project.clone().unwrap_or(UNSAVED.into()), new_conversation_id(now()), None);
            } else if let Some(chosen) = chosen {
                from = Some(chosen.conversation.checkpoint.clone());
                picked = true;
                self.begin(chosen.place, chosen.id, Some(&chosen.conversation));
                resumed = Some(chosen.conversation);
            } else if from.is_some() {
            } else if let Some(switched) = switched.filter(|s| s.key == observation.key) {
                from = Some(switched.checkpoint);
            } else if let Some((checkpoint, _)) = carry {
                from = Some(checkpoint);
            } else if let Some(settled) = settled.filter(|s| reason == Some("cancelled") && s.key == observation.key) {
                from = Some(settled.checkpoint);
            } else {
                let kept = if let (Some(project), Some(store)) = (&project, &self.0.options.conversations) {
                    store.current(project).await.ok().flatten()
                } else {
                    None
                };
                self.assert_current(op)?;
                if let Some(kept) = kept {
                    from = Some(kept.conversation.checkpoint.clone());
                    let begin = {
                        let s = self.0.state.borrow();
                        kept.id != s.conversation_id || s.place != project
                    };
                    if begin {
                        self.begin(project.clone().unwrap(), kept.id, Some(&kept.conversation));
                    }
                    resumed = Some(kept.conversation);
                } else {
                    self.begin(project.clone().unwrap_or(UNSAVED.into()), new_conversation_id(now()), None);
                }
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
                    self.begin(project.unwrap_or(UNSAVED.into()), new_conversation_id(now()), None);
                    self.build_kernel(op, observation, None).await?
                }
            };
            self.0.state.borrow_mut().kernel = Some(Held { value: value.clone(), key: observation.key.clone(), revision, lifetime });
            if let Some(kept) = unreadable {
                self.emit(SessionEvent::Resumed {
                    saved_at: kept.saved_at,
                    lines: transcript_of(&kept.checkpoint.messages),
                    changes: None,
                    chosen: None,
                    unreadable: Some(true),
                });
                self.notice("Kumi couldn't continue that conversation with this model, so it's shown above and a fresh one starts here.");
            } else {
                match reason {
                    Some("set") => self.notice(if resumed.is_some() {
                        format!(
                            "The open Set changed; continuing your conversation about {}.",
                            observation.project.as_ref().map(|p| p.name.as_str()).unwrap_or("this Set")
                        )
                    } else {
                        "The open Set changed; starting a fresh conversation.".into()
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
                    self.emit(SessionEvent::Resumed {
                        saved_at: kept.saved_at,
                        lines: value.transcript(),
                        changes: (!earlier.is_empty()).then_some(earlier),
                        chosen: picked.then_some(true),
                        unreadable: None,
                    });
                }
            }
        }
        self.0.state.borrow_mut().must_reset = false;
        Ok(())
    }
    fn save_conversation(&self, turn: bool) {
        let held = self.0.state.borrow().kernel.clone();
        let Some(held) = held.filter(|h| h.value.has_checkpoint()) else {
            return;
        };
        let Ok(checkpoint) = held.value.checkpoint() else {
            return;
        };
        let (place, id, conversation) = {
            let mut s = self.0.state.borrow_mut();
            s.settled = Some(Settled { checkpoint: checkpoint.clone(), key: held.key });
            if turn {
                s.conversation_turns += 1;
            }
            if s.conversation_first.is_none() {
                s.conversation_first =
                    transcript_of(&checkpoint.messages).iter().find(|l| l.role == TranscriptRole::User).map(|l| head(&l.text, 200));
            }
            (
                s.place.clone(),
                s.conversation_id.clone(),
                SavedConversation {
                    saved_at: now(),
                    checkpoint,
                    turns: Some(s.conversation_turns),
                    first: s.conversation_first.clone().filter(|s| !s.is_empty()),
                    changes: (!s.changes.is_empty())
                        .then(|| s.changes.iter().skip(s.changes.len().saturating_sub(2000)).cloned().collect()),
                },
            )
        };
        if let (Some(store), Some(place)) = (self.0.options.conversations.clone(), place) {
            self.enqueue(async move { store.save(&place, &id, &conversation).await });
        }
    }
    async fn observe(&self, op: &Operation, pinned: Option<PinnedNode>, continuing: bool) -> Result<Observation, RuntimeError> {
        self.assert_current(op)?;
        let integration = self.0.state.borrow().integration.clone().ok_or_else(|| RuntimeError::plain("Integration not started"))?;
        op.phase.set(Phase::Refresh);
        self.0.state.borrow_mut().observation = None;
        let hints = (pinned.is_some() || continuing).then_some(ObserveHints { pinned, continuing: continuing.then_some(true) });
        let snapshot = integration.observe(op.signal.clone(), hints).await?;
        self.assert_current(op)?;
        {
            let mut s = self.0.state.borrow_mut();
            s.project = snapshot.project.as_ref().map(|p| p.id.clone());
            s.set_name = snapshot.project.as_ref().map(|p| p.name.clone());
            s.set = Some(snapshot.key.clone());
            s.plan = snapshot.tools.iter().find(|t| t.name() == "make_changes").cloned();
        }
        if snapshot.project.is_some() && self.0.notes.is_some() {
            let this = self.clone();
            tokio::task::spawn_local(async move {
                let _ = this.0.notes.as_ref().unwrap().flush().await;
            });
        }
        self.ensure_kernel(op, &snapshot).await?;
        self.assert_current(op)?;
        if let (Some(tracks), Some(l)) = (&snapshot.tracks, &self.0.learned) {
            l.drafts.observed(tracks);
        }
        if self.0.state.borrow().place.as_deref() == Some(UNSAVED) {
            if let (Some(project), Some(store)) = (&snapshot.project, self.0.options.conversations.clone()) {
                let to = project.id.clone();
                let id = {
                    let mut s = self.0.state.borrow_mut();
                    s.place = Some(to.clone());
                    s.conversation_id.clone()
                };
                self.enqueue(async move { store.move_conversation(&id, UNSAVED, &to).await });
            }
        }
        self.0.state.borrow_mut().observation = Some(snapshot.label.clone());
        self.emit(SessionEvent::Observation { label: snapshot.label.clone() });
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
                            FailureKind::Auth | FailureKind::Billing | FailureKind::Model | FailureKind::Config | FailureKind::Live
                        )
                    });
                    let message = if actionable {
                        error.message()
                    } else if op.phase.get() == Phase::Undo {
                        kumi.map(|e| e.message.clone()).unwrap_or("The undo didn't finish; check Live.".into())
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
            let line = timing::line(&recorder.finish(), began.elapsed().as_millis() as u64, stop, usage.as_ref());
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
    fn with_taste(&self, tool: Rc<dyn KernelTool>) -> Rc<dyn KernelTool> {
        match &self.0.taste {
            Some(taste) if tool.name() == "undo_change" => taste.watch_undo(tool),
            _ => tool,
        }
    }
    fn with_technique(&self, tool: Rc<dyn KernelTool>) -> Rc<dyn KernelTool> {
        match &self.0.learned {
            Some(learned) if tool.name() == "make_changes" => Rc::new(TechniquePlan {
                description: format!("{} {}", tool.description(), PLAN_TECHNIQUE["description"].as_str().unwrap_or_default()),
                tool,
                learned: learned.clone(),
            }),
            _ => tool,
        }
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
    fn stream(&self, signal: Signal, on_start: Rc<dyn Fn()>) -> Option<Box<dyn StreamingCall>> {
        self.tool
            .stream(signal, on_start)
            .map(|call| Box::new(TechniqueStream { call, learned: self.learned.clone() }) as Box<dyn StreamingCall>)
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
            if trim(input).is_empty() || input.len() > 16 * 1024 {
                return Err(RuntimeError::plain("Enter a nonempty prompt of at most 16 KiB"));
            }
            if self.0.options.max_turns.is_some_and(|max| s.turns >= max) {
                return Err(RuntimeError::plain("Conversation limit reached; use /new"));
            }
            s.turns += 1;
            s.interrupted = None;
        }
        let said = text.clone();
        self.perform(
            true,
            Phase::Refresh,
            Box::new(move |this, op| async move { this.submit_turn(op, text, pinned, pictures).await }.boxed_local()),
            None,
            Some(said),
        )?
        .await
    }
    fn has_steer(&self) -> bool {
        true
    }
    fn steer(&self, text: &str) -> bool {
        let held = {
            let s = self.0.state.borrow();
            if s.state == TurnState::Closed
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
                    let (place, integrated) = {
                        let mut s = this.0.state.borrow_mut();
                        s.start_fresh = true;
                        s.turns = 0;
                        s.must_reset = false;
                        s.settled = None;
                        s.interrupted = None;
                        (s.place.clone(), s.integration.is_some())
                    };
                    if let (Some(store), Some(place)) = (this.0.options.conversations.clone(), place) {
                        this.enqueue(async move { store.fresh(&place).await });
                    }
                    if integrated {
                        this.observe(&op, None, false).await?;
                    } else {
                        this.reset(&op).await?;
                    }
                    this.notice("New conversation. The last one is kept; /conversations goes back to it.");
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
                        .or_else(|| settled.map(|s| s.checkpoint));
                    if let Some(checkpoint) = checkpoint {
                        this.0.state.borrow_mut().carry = Some((checkpoint, place));
                    }
                    this.reset(&op).await?;
                    if this.0.state.borrow().connection == ConnectionState::Connected {
                        this.notice("Reconnected to Live; the conversation carries on.");
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
        true
    }
    async fn conversations(&self) -> Result<Vec<ConversationSummary>, RuntimeError> {
        let place = {
            let s = self.0.state.borrow();
            s.place.clone().or(s.project.clone()).unwrap_or(UNSAVED.into())
        };
        Ok(if let Some(store) = &self.0.options.conversations { store.list(&place).await.unwrap_or_default() } else { vec![] })
    }
    fn has_resume_conversation(&self) -> bool {
        true
    }
    async fn resume_conversation(&self, id: &str) -> Result<bool, RuntimeError> {
        let Some(store) = &self.0.options.conversations else {
            return Ok(false);
        };
        let (place, same) = {
            let s = self.0.state.borrow();
            let place = s.place.clone().or(s.project.clone()).unwrap_or(UNSAVED.into());
            let same = s.conversation_id == id && s.place.as_deref() == Some(&place);
            (place, same)
        };
        if same {
            return Ok(true);
        }
        let saving = self.0.saving.borrow().clone();
        let _ = saving.await;
        let Some(conversation) = store.load(&place, id).await? else {
            return Ok(false);
        };
        let id = id.to_owned();
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
                        s.chosen = Some(Chosen { place, id, conversation });
                        s.must_reset = false;
                        s.turns = 0;
                        s.interrupted = None;
                    }
                    this.observe(&op, None, false).await?;
                    this.save_conversation(false);
                    Ok(None)
                }
                .boxed_local()
            }),
            None,
            None,
        )?
        .await?;
        Ok(true)
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
            WatchEvent::Change(c) => {
                if c.state == ChangeState::Applied {
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
            if s.seen.len() > 20_000 {
                s.seen.shift_remove_index(0);
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
        match &self.0.options.library {
            Some(library) => library.forget_taste(id).await,
            None => Ok(false),
        }
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
        if let Some(notes) = &self.0.notes {
            notes.forget(id).await
        } else {
            Ok(None)
        }
    }
    fn has_change_note(&self) -> bool {
        true
    }
    async fn change_note(&self, id: &str, change: NoteChange) -> Result<Option<MemoryNote>, RuntimeError> {
        if let Some(notes) = &self.0.notes {
            notes.change(id, change).await
        } else {
            Ok(None)
        }
    }
    fn has_recipes(&self) -> bool {
        true
    }
    async fn recipes(&self) -> Result<Vec<RecipeSummary>, RuntimeError> {
        let Some(store) = &self.0.options.recipes else {
            return Ok(vec![]);
        };
        Ok(store
            .list()
            .await?
            .into_iter()
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
    fn has_run_recipe(&self) -> bool {
        true
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
        let Some(recipe) = store.get(name).await? else {
            return Ok(false);
        };
        if !store.remove(&recipe.name).await? {
            return Ok(false);
        }
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
        if let Some(l) = &self.0.learned {
            Ok(l.forget(id).await?.is_some())
        } else {
            Ok(false)
        }
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
        let goal = text.map(trim).filter(|s| !s.is_empty()).map(str::to_owned);
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
            if goal.as_ref().is_some_and(|s| s.len() > 4096) {
                return Err(RuntimeError::plain("Say the goal in at most 4 KiB"));
            }
            s.turns += 1;
            s.interrupted = None;
        }
        let input = goal.as_ref().map(|s| format!("/goal {s}")).unwrap_or("/goal".into());
        self.perform(
            true,
            Phase::Refresh,
            Box::new(move |this, op| async move { this.run_goal(op, goal).await }.boxed_local()),
            None,
            Some(input),
        )?
        .await
    }
    fn has_stop_goal(&self) -> bool {
        true
    }
    async fn stop_goal(&self) -> Result<bool, RuntimeError> {
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
            .map(|l| LessonEntry { line: super::playbook::lesson_line(&l), id: l.id, at: l.at })
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
                    this.emit(SessionEvent::Lesson { action: LessonAction::Forgot, id, line: super::playbook::lesson_line(&gone) });
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
    async fn arrangement_strip(&self) -> Result<Option<ArrangementStrip>, RuntimeError> {
        let i = {
            let s = self.0.state.borrow();
            s.integration.clone().filter(|i| s.connection == ConnectionState::Connected && i.has_arrangement_strip())
        };
        Ok(if let Some(i) = i { i.arrangement_strip(abort::timeout(10_000)).await.ok().flatten() } else { None })
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
        if let Some(unlisten) = self.0.unlisten_library.borrow_mut().take() {
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
