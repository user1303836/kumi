//! What a front end and the session say to each other. A front end makes requests: each is a struct in [`op`], which
//! travels as an [`Op`] and is answered by its own [`Reply`] (`session.op(op::Undo { id: None })` gives back the
//! change undone). The session sends its events ([`SessionEvent`]), and says what it can do ([`Capabilities`]). An `Op`
//! is written as the session's events are, a field left out when it's None: `{"type":"undo"}`.
//!
//! A front end holds a [`SessionHandle`]. Today's is [`InProcess`]: the session in this process, behind
//! [`SessionController`], whose methods its requests go to. A front end that asks only this way can have its session
//! anywhere else.

use std::{
    cell::RefCell,
    collections::HashMap,
    rc::{Rc, Weak},
    task::Context,
};

use async_trait::async_trait;
use futures::{future::LocalBoxFuture, task::noop_waker, FutureExt};
use kumi_common::abort::Signal;
use serde::{Deserialize, Serialize};

use crate::core::contracts::*;
use crate::core::jobs::{JobDetail, JobStatus};
use crate::core::palette::{Gathered, PaletteListed, PaletteView};
use crate::core::search_memory::SearchMemoryStatus;
use kumi_ai::{
    auth::openai_codex::DevicePrompt,
    control::{ChatGptSignIn, CurrentModel, DefaultModel, LocalStatus, ModelController, ProviderStatus},
    providers::{
        models::{ApiKeyCheck, ModelInfo, ServiceTier},
        Effort, ProviderId,
    },
};
use kumi_library::map::MapData;
use kumi_listening::{
    music::learning::Music,
    slots::{Said, SlotsContext},
};
use kumi_types::contracts::*;
use kumi_types::errors::RuntimeError;

/// The most a message the producer sends holds, in bytes: a paste of 2,000 lines (a log, a lyric sheet) fits. With the
/// observation and the answer's reads it still fits the kernel's default budget (400 KB), which clears earlier exchanges
/// to make room, and a saved conversation keeps it with its answer (the last exchange, up to 1 MB). A front end says so
/// before sending more, and the session refuses it.
pub const MAX_MESSAGE: usize = 256 * 1024;

/// A size as a message's limit is said: KB, or MB from one.
pub fn size_words(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{} KB", bytes.div_ceil(1024))
    }
}

/// What a message past [`MAX_MESSAGE`] is told.
pub fn too_long_words(bytes: usize) -> String {
    format!(
        "This message is {}, and one message takes up to {}: send part of it.",
        size_words(bytes as u64),
        size_words(MAX_MESSAGE as u64)
    )
}

/// What a session can do, so a front end offers only that. A session's stay as they are while it runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    pub attachments: bool,
    pub steer: bool,
    pub aside: bool,
    pub reconnect: bool,
    pub conversations: bool,
    pub resume_conversation: bool,
    pub watch: bool,
    pub memory: bool,
    pub library: bool,
    pub taste: bool,
    pub forget_taste: bool,
    pub forget: bool,
    pub change_note: bool,
    pub recipes: bool,
    pub instant: bool,
    pub run_recipe: bool,
    pub forget_recipe: bool,
    pub techniques: bool,
    pub forget_technique: bool,
    pub answer_technique: bool,
    pub goal: bool,
    pub stop_goal: bool,
    pub stop_loop: bool,
    pub goal_status: bool,
    pub lessons: bool,
    pub forget_lesson: bool,
    pub stop_daw: bool,
    pub daw_message: bool,
    pub jobs: bool,
    pub palette: bool,
    pub device_tree: bool,
    pub clip_view: bool,
    pub session_strip: bool,
    pub arrangement_strip: bool,
    pub reconfigure: bool,
    pub music: bool,
    pub map: bool,
    pub slots: bool,
    /// Choosing a model and signing in (/model, /effort, /fast, /login, /logout).
    pub models: bool,
    pub take_back: bool,
}

impl Capabilities {
    /// A controller's, from its `has_…` flags.
    pub fn of(session: &dyn SessionController) -> Capabilities {
        Capabilities {
            attachments: session.has_attachments(),
            steer: session.has_steer(),
            aside: session.has_aside(),
            reconnect: session.has_reconnect(),
            conversations: session.has_conversations(),
            resume_conversation: session.has_resume_conversation(),
            watch: session.has_watch(),
            memory: session.has_memory(),
            library: session.has_library(),
            taste: session.has_taste(),
            forget_taste: session.has_forget_taste(),
            forget: session.has_forget(),
            change_note: session.has_change_note(),
            recipes: session.has_recipes(),
            instant: session.has_instant(),
            run_recipe: session.has_run_recipe(),
            forget_recipe: session.has_forget_recipe(),
            techniques: session.has_techniques(),
            forget_technique: session.has_forget_technique(),
            answer_technique: session.has_answer_technique(),
            goal: session.has_goal(),
            stop_goal: session.has_stop_goal(),
            stop_loop: session.has_stop_loop(),
            goal_status: session.has_goal_status(),
            lessons: session.has_lessons(),
            forget_lesson: session.has_forget_lesson(),
            stop_daw: session.has_stop_live(),
            daw_message: session.has_daw_message(),
            jobs: session.has_jobs(),
            palette: session.has_palette(),
            device_tree: session.has_device_tree(),
            clip_view: session.has_clip_view(),
            session_strip: session.has_session_strip(),
            arrangement_strip: session.has_arrangement_strip(),
            reconfigure: session.has_reconfigure(),
            music: session.music().is_some(),
            map: session.map_loader().is_some(),
            slots: session.slots_context().is_some(),
            models: session.model_controls().is_some(),
            take_back: session.has_take_back(),
        }
    }
}

/// A request a front end makes, by its struct in [`op`]: it travels as an [`Op`], and its answer is a [`Reply`].
pub trait Request: Into<Op> {
    type Reply;
    /// This request's own answer, out of the session's; None when the session answered another request.
    fn answer(reply: Reply) -> Option<Self::Reply>;
}

/// Words that never show: a key. Its Debug says only that it's hidden; on the wire it's itself, as its request needs.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Secret(pub String);

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret([redacted])")
    }
}

/// The models as a front end shows them, read at once: the one chosen now, whether `/fast` is on, and the fallback.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelsView {
    pub current: CurrentModel,
    /// `/fast` is on (the setting, whether or not this model has a faster tier).
    pub fast: bool,
    /// The model that carries on while the chosen one's plan limit lasts or its provider is down.
    pub fallback: Option<String>,
}

/// Each request: its struct in [`op`] (whose doc is the request's), its [`Op`] and [`Reply`] variants, and its
/// [`Request`] impl, from one table.
macro_rules! requests {
    ($($(#[doc = $doc:literal])* $name:ident { $($(#[$attr:meta])* $field:ident: $ty:ty),* $(,)? } -> $reply:ty;)*) => {
        /// The requests a front end makes, each by itself.
        pub mod op {
            use super::*;
            $(
                $(#[doc = $doc])*
                #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
                #[serde(rename_all = "camelCase")]
                pub struct $name {
                    $($(#[$attr])* pub $field: $ty,)*
                }
            )*
        }
        /// A request as it travels.
        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
        #[serde(tag = "type", rename_all = "kebab-case")]
        pub enum Op {
            $($name(op::$name),)*
        }
        /// The session's answer to a request, by the request's name.
        #[derive(Debug)]
        pub enum Reply {
            $($name($reply),)*
        }
        impl Op {
            /// Every request's name, as its struct in [`op`] is called.
            pub const NAMES: &'static [&'static str] = &[$(stringify!($name)),*];
            /// This request's name, as its struct in [`op`] is called.
            pub fn name(&self) -> &'static str {
                match self {
                    $(Op::$name(_) => stringify!($name),)*
                }
            }
        }
        $(
            impl From<op::$name> for Op {
                fn from(request: op::$name) -> Op {
                    Op::$name(request)
                }
            }
            impl Request for op::$name {
                type Reply = $reply;
                fn answer(reply: Reply) -> Option<$reply> {
                    match reply {
                        Reply::$name(reply) => Some(reply),
                        _ => None,
                    }
                }
            }
        )*
    };
}

requests! {
    /// Start: reach the DAW and read the project open in it.
    Start {} -> ();
    /// A message for the model: what the producer pinned is "this" in it, and the files they added go with it.
    Submit {
        input: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pinned: Option<PinnedNode>,
        attachments: Vec<Attachment>,
        /// What the message names with `@`, in the order it names them.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        references: Vec<Reference>,
    } -> ();
    /// What the producer can name with `@`, best first for `query` (what they've typed after it), at most `limit`.
    References { query: String, limit: u32 } -> Vec<crate::core::references::Candidate>;
    /// More from the producer for the answer under way, entering it at its next step (a "steer" event says when):
    /// false when no answer is at a point to take it.
    Steer { text: String } -> bool;
    /// A side question about the conversation (/btw), answered without tools and never kept in it: the answer. Its words
    /// come as they're written, as `aside` events with its `id`, and [`op::StopAside`] stops it.
    Aside { id: u32, question: String } -> String;
    StopAside { id: u32 } -> ();
    /// Stop the answer under way.
    Cancel {} -> ();
    /// End the session.
    Close {} -> ();
    /// Undo one of Kumi's changes (the latest undoable one without `id`); not during a turn. The change as it now
    /// stands, or None with nothing to undo (an error event says why).
    Undo {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
    } -> Option<ChangeRecord>;
    /// The conversation's requests that can be taken back, newest first, each with its changes as HISTORY has them.
    TakeBackPlan {} -> Vec<Requested>;
    /// Take back the request asked at `to` (its `at`) and each after it, undoing their changes newest first until an
    /// undo fails; not during a turn.
    TakeBack { to: i64 } -> TakenBack;
    Refresh {} -> ();
    /// Start afresh; this conversation stays kept.
    NewConversation {} -> ();
    /// A fresh bridge to the DAW, carrying the conversation over.
    Reconnect {} -> ();
    /// The model changed: the next turn or refresh builds the kernel afresh, carrying the conversation on.
    Reconfigure {} -> ();
    /// A fixed phrase ("tempo 124"), run with no model call: what changed, or None when the message isn't one.
    Instant { text: String } -> Option<InstantOutcome>;
    /// The producer picked one of the options Kumi offered, with a key.
    Pick { pick: Picked } -> ();
    /// The open project's kept conversations, newest first.
    Conversations {} -> Vec<ConversationSummary>;
    /// Every project's kept conversations, newest first.
    AllConversations {} -> Vec<ConversationSummary>;
    /// What the producer's word names among the kept conversations: an id, the start of one, or a name.
    FindConversation { word: String } -> ConversationMatch;
    /// Carry on a kept conversation instead of this one: false when it's gone.
    ResumeConversation { id: String } -> bool;
    /// The producer's name for a kept conversation, or for this one without `id`; None takes it away.
    RenameConversation {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    } -> bool;
    PinConversation { id: String, pinned: bool } -> bool;
    /// A kept conversation goes for good (never this one): false when it wasn't kept.
    DeleteConversation { id: String } -> bool;
    /// A copy of this conversation to carry on in: its id, or None with nothing to copy yet. With `before` (a request's
    /// `at`, from `TakeBackPlan`), the copy is the conversation as it stood just before that request.
    ForkConversation {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        before: Option<i64>,
    } -> Option<String>;
    /// The producer's answer to `set-asked`.
    AnswerSet { choice: SetChoice } -> ();
    ConversationNow {} -> Option<ConversationNow>;
    /// A kept conversation's words, for a look before going back to it.
    ConversationLines { id: String } -> Vec<TranscriptLine>;
    /// This conversation, to export: None before its first answer.
    ConversationToExport {} -> Option<ConversationToExport>;
    /// What Kumi remembers now: about the producer, and about the open project once it's saved.
    Memory {} -> Option<MemoryView>;
    /// What Kumi's searches remember: None without its database.
    Remembered {} -> Option<SearchMemoryStatus>;
    /// What Kumi learned from the producer's own projects, a line each.
    Taste {} -> Vec<KeptLine>;
    ForgetTaste { id: String } -> bool;
    /// Forget a note: the note, or None when there's none by that id.
    Forget { id: String } -> Option<MemoryNote>;
    /// Change a note's words, or pin it: the note as changed, or None when there's none by that id.
    ChangeNote { id: String, change: NoteChange } -> Option<MemoryNote>;
    /// The saved recipes, most recently used first.
    Recipes {} -> Vec<RecipeSummary>;
    /// A pin as a recipe's blanks take it now.
    PinForRecipe { pin: PinnedNode } -> Option<PinnedNode>;
    /// Run a recipe now, with no model, `with` a value for each blank: what it did.
    RunRecipe { name: String, with: JsonObject } -> RecipeOutcome;
    ForgetRecipe { name: String } -> bool;
    Techniques {} -> Vec<TechniqueSummary>;
    ForgetTechnique { id: String } -> bool;
    /// The producer's answer to the technique Kumi offered: false when none was waiting.
    AnswerTechnique { keep: bool } -> bool;
    /// What Kumi learned matching sounds, newest first.
    Lessons {} -> Vec<LessonEntry>;
    ForgetLesson { id: String } -> bool;
    /// /goal: pursue one, with what to reach, or pick a paused one up without.
    Goal {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
    } -> ();
    StopGoal {} -> bool;
    /// End a sound-match search, running or paused, keeping its best.
    StopLoop {} -> bool;
    /// Stop the DAW playing and recording, any time, even during a turn: false when it couldn't.
    StopDaw {} -> bool;
    /// A short line in the DAW's own window, any time: false when it couldn't.
    DawMessage { text: String } -> bool;
    /// Whether the DAW is recording: None when it can't say.
    DawRecording {} -> Option<bool>;
    /// Stop a running job: false when it isn't running.
    StopJob { id: u32 } -> bool;
    /// Run a job that ended again: the new job's number, or why it can't.
    RunJobAgain { id: u32 } -> Result<u32, String>;
    /// The answer's steps at work away from the DAW go on as a job: its number, or why nothing moved.
    Background {} -> Result<u32, String>;
    /// Carry on with a job that ended whose request needs it: false when it can't now.
    CarryOn { job: u32 } -> bool;
    /// The session's palette, with its files: None before one is made or chosen.
    Palette {} -> Option<PaletteView>;
    /// Sounds or songs gathered into the palette: what happened, in words.
    PaletteGather { files: Vec<Gathered> } -> String;
    /// A file out of the palette, by its path, size and modification time: whether it had it.
    PaletteRemove { path: String, size: i64, mtime: i64 } -> bool;
    /// /palette and its words: its answer, in words.
    PaletteCommand { words: String } -> String;
    /// Every palette, the latest changed first: what `/palette use` offers.
    PaletteNames {} -> Vec<PaletteListed>;
    /// /music: what Kumi knows of the producer's music and how learning it goes, and (before they add a folder) the
    /// usual place it found songs in.
    Music {} -> MusicView;
    /// The folders Kumi learns the producer's music from.
    MusicFolders {} -> Vec<String>;
    /// /music add: learn from these folders from now on, the producer having agreed; or why it can't.
    LearnMusic { folders: Vec<String> } -> Result<(), String>;
    /// /music remove: learn from these folders alone from now on; or why it can't.
    SetMusicFolders { folders: Vec<String> } -> Result<(), String>;
    /// /music pause (`held`) and resume.
    HoldMusic { held: bool } -> ();
    /// /music forget: everything Kumi learned of the producer's music; or why it couldn't.
    ForgetMusic {} -> Result<(), String>;
    /// /map: the producer's sounds and songs on their maps, read afresh.
    Map {} -> MapData;
    /// /slots and its words: what it says. What's under way comes as `slots-progress` events with its `id`, and
    /// [`op::StopSlots`] stops it.
    Slots { id: u32, words: String } -> Said;
    StopSlots { id: u32 } -> ();
    /// The providers Kumi can sign in to, and how each is signed in now.
    Providers {} -> Vec<ProviderStatus>;
    /// The model servers on this computer, and whether each runs.
    LocalModels {} -> Vec<LocalStatus>;
    /// With no model chosen, the first a signed-in provider or a running server offers, chosen: None with one chosen
    /// already, or none to choose.
    ChooseDefault {} -> Option<DefaultModel>;
    /// A provider's models (`refresh`: read afresh, not what this session listed).
    Models { provider: String, refresh: bool } -> Vec<ModelInfo>;
    /// The model to talk to from the next message: what to say of it (one that can only talk, say).
    ChooseModel { model: String } -> Option<String>;
    /// How hard the model thinks: None, its own default.
    SetEffort {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        effort: Option<Effort>,
    } -> ();
    /// `/fast`: the model's faster tier on or off; None when turning it on and the model has none.
    SetFast { on: bool } -> Option<ServiceTier>;
    /// The model that carries on while the chosen one can't (None: none), from the next message.
    SetFallback {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
    } -> ();
    /// A provider's key, checked, and kept when it works: what the check said.
    SaveKey { provider: ProviderId, key: Secret } -> ApiKeyCheck;
    /// Signed out of a provider: whether there was a sign-in to take away.
    SignOut { provider: ProviderId } -> bool;
    /// Sign in to ChatGPT, in the browser or (`by_code`) with a code to type. Its address or code comes as a `sign-in`
    /// event with its `id`, and [`op::StopSignIn`] stops it.
    SignInChatgpt { id: u32, by_code: bool } -> ();
    StopSignIn { id: u32 } -> ();
    /// The saved model, when its provider's own list no longer shows it: what to say ("ChatGPT no longer offers
    /// gpt-5.5 to your sign-in"), or None.
    MissingModel {} -> Option<String>;
    /// A body for FOCUS (while connected).
    FocusBody { request: BodyRequest } -> Option<FocusBody>;
}

/// The session as a front end has it: the state it shows (answered at once), and its requests.
#[async_trait(?Send)]
pub trait SessionHandle {
    fn capabilities(&self) -> Capabilities;
    /// Whether FOCUS can show this kind of body (while connected).
    fn can_show(&self, kind: BodyKind) -> bool;
    fn status(&self) -> SessionStatus;
    /// The fallback model answering for the producer's now (`<provider>/<model>`); None while theirs does.
    fn answering_fallback(&self) -> Option<String>;
    fn library(&self) -> Option<LibraryStatus>;
    /// The saved project open in the DAW, by its file, once the session has read it.
    fn open_set(&self) -> Option<String>;
    /// The background jobs, oldest first, and one with its steps and report.
    fn jobs(&self) -> Vec<JobStatus>;
    fn job(&self, id: u32) -> Option<JobDetail>;
    /// The job asked for last isn't shown any more (its panel closed).
    fn unwatch_job(&self) {}
    /// Whether ctrl-b has a step to move right now.
    fn can_background(&self) -> bool;
    /// The models as the screen shows them; None where there's no model to choose (`kumi -p`'s).
    fn models(&self) -> Option<ModelsView>;
    /// A provider's name as the producer knows it: a model server's own name for one.
    fn provider_name(&self, id: &str) -> String;
    /// The cloud provider `id` names (an error's or a model's provider), unless it's a model server's.
    fn cloud_provider(&self, id: &str) -> Option<ProviderId>;
    /// The tokens `model` reads at once, as its provider's list (or its server) says: the context meter's whole.
    fn context_window(&self, model: &str) -> Option<f64>;
    /// Where what the session says between its replies goes (an aside's words).
    fn listen(&self, on_event: Rc<dyn Fn(SessionEvent)>);
    /// A request, answered.
    async fn send(&self, op: Op) -> Result<Reply, RuntimeError>;
    /// A request whose answer nobody waits for: sent now, its answer dropped (and any error with it).
    fn tell(&self, op: Op);
}

impl<'a> dyn SessionHandle + 'a {
    /// A request, answered with its own reply.
    pub async fn op<R: Request>(&self, request: R) -> Result<R::Reply, RuntimeError> {
        R::answer(self.send(request.into()).await?).ok_or_else(|| RuntimeError::plain("The session answered another request."))
    }
}

/// A part of the session a request is answered from (its music, its map, its slots), or why it can't be.
pub fn part<T>(part: Option<T>) -> Result<T, RuntimeError> {
    part.ok_or_else(|| RuntimeError::plain("Not available"))
}

/// What /music shows, read from the producer's music: before they add a folder, the usual place songs were found in,
/// looked for off Kumi's thread.
pub async fn music_view(music: &Rc<Music>) -> MusicView {
    let found = match music.folders().is_empty() {
        true => {
            let (places, skip) = (music.places(), music.skipping());
            tokio::task::spawn_blocking(move || kumi_listening::music::folders::found_in(places, &skip())).await.ok().flatten()
        }
        false => None,
    };
    MusicView { status: music.status(), agreed: music.agreed(), held: music.held(), found }
}

/// A session in this process, behind [`SessionController`]: each request goes to the method it was before the
/// protocol, and the state it shows is read from it as it's asked for.
pub struct InProcess {
    this: Weak<InProcess>,
    session: Rc<dyn SessionController>,
    side: Rc<HandleSide>,
}

/// The handle's side of a session in this process, whatever answers its requests (`InProcess` a session's, a test its
/// own): who hears what's said between replies, and what stops each aside, /slots request and ChatGPT sign-in under way,
/// by its id.
pub struct HandleSide {
    this: Weak<HandleSide>,
    on_event: RefCell<Option<Rc<dyn Fn(SessionEvent)>>>,
    /// The asides under way, by id, and what stops each.
    asides: RefCell<HashMap<u32, Signal>>,
    /// The same for /slots' fetches and checks.
    slots: RefCell<HashMap<u32, Signal>>,
    /// And for ChatGPT's sign-ins.
    sign_ins: RefCell<HashMap<u32, Signal>>,
}

impl HandleSide {
    pub fn new() -> Rc<HandleSide> {
        Rc::new_cyclic(|this| HandleSide {
            this: this.clone(),
            on_event: RefCell::new(None),
            asides: RefCell::new(HashMap::new()),
            slots: RefCell::new(HashMap::new()),
            sign_ins: RefCell::new(HashMap::new()),
        })
    }
    /// Where what's said between replies goes: [`SessionHandle::listen`]'s.
    pub fn listen(&self, on_event: Rc<dyn Fn(SessionEvent)>) {
        *self.on_event.borrow_mut() = Some(on_event);
    }
    /// Says `event` to whoever listens.
    pub fn emit(&self, event: SessionEvent) {
        let on_event = self.on_event.borrow().clone();
        if let Some(on_event) = on_event {
            on_event(event);
        }
    }
    /// [`op::Aside`], which `ask` asks given where its words go (`aside` events with `id`) and what stops it.
    pub async fn aside<F>(&self, id: u32, ask: impl FnOnce(OnText, Signal) -> F) -> Result<String, RuntimeError>
    where
        F: std::future::Future<Output = Result<String, RuntimeError>>,
    {
        // A stop that came first left its signal stopped: the aside ends at once.
        let signal = under_way(&self.asides, id);
        let this = self.this.clone();
        let on_text: OnText = Rc::new(move |text: &str| {
            if let Some(this) = this.upgrade() {
                this.emit(SessionEvent::Aside { id, text: text.into() });
            }
        });
        let answer = ask(on_text, signal).await;
        self.asides.borrow_mut().remove(&id);
        answer
    }
    /// [`op::StopAside`]: before its aside starts too, which takes the stopped signal up as it starts (an id is never used
    /// again, so what a late stop leaves is harmless).
    pub fn stop_aside(&self, id: u32) {
        under_way(&self.asides, id).cancel();
    }
    /// [`op::Slots`] from the session's slots: what's under way comes as `slots-progress` events with `id`.
    pub async fn slots(&self, id: u32, words: &str, context: &SlotsContext) -> Said {
        // As an aside's: a stop that came first left its signal stopped.
        let signal = under_way(&self.slots, id);
        let this = self.this.clone();
        let progress = move |line: String| {
            if let Some(this) = this.upgrade() {
                this.emit(SessionEvent::SlotsProgress { id, line });
            }
        };
        let said = kumi_listening::slots::command(words, context, &progress, signal).await;
        self.slots.borrow_mut().remove(&id);
        said
    }
    /// [`op::StopSlots`].
    pub fn stop_slots(&self, id: u32) {
        under_way(&self.slots, id).cancel();
    }
    /// [`op::SignInChatgpt`] through the session's model controls: its address or code comes as a `sign-in` event with
    /// `id`.
    pub async fn sign_in_chatgpt(&self, id: u32, by_code: bool, models: &dyn ModelController) -> Result<(), RuntimeError> {
        // As an aside's: a stop that came first left its signal stopped.
        let signal = under_way(&self.sign_ins, id);
        let this = self.this.clone();
        let said = move |url: Option<String>, code: Option<DevicePrompt>| {
            if let Some(this) = this.upgrade() {
                this.emit(SessionEvent::SignIn { id, url, code });
            }
        };
        let how = if by_code {
            ChatGptSignIn::Device { signal, on_code: Rc::new(move |code| said(None, Some(code))) }
        } else {
            ChatGptSignIn::Browser { signal, on_url: Rc::new(move |url| said(Some(url), None)) }
        };
        let signed = models.sign_in_chatgpt(how).await;
        self.sign_ins.borrow_mut().remove(&id);
        signed
    }
    /// [`op::StopSignIn`].
    pub fn stop_sign_in(&self, id: u32) {
        under_way(&self.sign_ins, id).cancel();
    }
}

/// What stops the request `id`: made as it starts, or as a stop comes before it.
fn under_way(signals: &RefCell<HashMap<u32, Signal>>, id: u32) -> Signal {
    signals.borrow_mut().entry(id).or_insert_with(Signal::new).clone()
}

/// [`SessionHandle::tell`]'s timing, for any handle in this process: `answering` goes as far as it goes at once (all of
/// it, for what a session does at once), then the rest in a task of its own.
pub fn tell_now(mut answering: LocalBoxFuture<'static, ()>) {
    if answering.as_mut().poll(&mut Context::from_waker(&noop_waker())).is_pending() {
        tokio::task::spawn_local(answering);
    }
}

/// The models as a front end shows them, from the session's model controls: None where there's no model to choose.
pub fn models_view(models: Option<Rc<dyn ModelController>>) -> Option<ModelsView> {
    let models = models?;
    Some(ModelsView { current: models.current(), fast: models.fast_enabled(), fallback: models.fallback() })
}

/// A provider's name as the producer knows it: the session's model controls know a model server's own name; without
/// them, the providers' table's.
pub fn provider_name(models: Option<Rc<dyn ModelController>>, id: &str) -> String {
    match models {
        Some(models) => models.provider_name(id),
        None => ProviderId::parse(id).map_or_else(|| id.into(), |provider| provider.row().name.into()),
    }
}

/// The cloud provider `id` names, unless the session's model controls know it as a model server's.
pub fn cloud_provider(models: Option<Rc<dyn ModelController>>, id: &str) -> Option<ProviderId> {
    match models {
        Some(models) => models.cloud_provider(id),
        None => ProviderId::parse(id),
    }
}

/// The model requests (signing in among them), answered from a session's model controls as `InProcess` answers them:
/// "Not available" for a session with none. None for any other request.
pub async fn models_answer(op: &Op, models: Option<Rc<dyn ModelController>>, side: &HandleSide) -> Option<Result<Reply, RuntimeError>> {
    let controls = || part(models.clone());
    Some(match op {
        Op::Providers(_) => async { Ok(Reply::Providers(controls()?.providers().await?)) }.await,
        Op::LocalModels(_) => async { Ok(Reply::LocalModels(controls()?.local().await)) }.await,
        Op::ChooseDefault(_) => async { Ok(Reply::ChooseDefault(controls()?.choose_default().await?)) }.await,
        Op::Models(op::Models { provider, refresh }) => async { Ok(Reply::Models(controls()?.models(provider, *refresh).await?)) }.await,
        Op::ChooseModel(op::ChooseModel { model }) => async { Ok(Reply::ChooseModel(controls()?.choose(model).await?)) }.await,
        Op::SetEffort(op::SetEffort { effort }) => async { Ok(Reply::SetEffort(controls()?.set_effort(*effort).await?)) }.await,
        Op::SetFast(op::SetFast { on }) => async { Ok(Reply::SetFast(controls()?.set_fast(*on).await?)) }.await,
        Op::SetFallback(op::SetFallback { model }) => {
            async { Ok(Reply::SetFallback(controls()?.set_fallback(model.as_deref()).await?)) }.await
        }
        Op::SaveKey(op::SaveKey { provider, key }) => {
            async { Ok(Reply::SaveKey(controls()?.save_key(*provider, &key.0, None).await?)) }.await
        }
        Op::SignOut(op::SignOut { provider }) => async { Ok(Reply::SignOut(controls()?.sign_out(*provider).await?)) }.await,
        Op::SignInChatgpt(op::SignInChatgpt { id, by_code }) => {
            async { Ok(Reply::SignInChatgpt(side.sign_in_chatgpt(*id, *by_code, &*controls()?).await?)) }.await
        }
        Op::MissingModel(_) => async { Ok(Reply::MissingModel(kumi_ai::control::missing_model(&*controls()?).await)) }.await,
        _ => return None,
    })
}

/// The tokens `model` reads at once, as the session's model controls say.
pub fn context_window(models: Option<Rc<dyn ModelController>>, model: &str) -> Option<f64> {
    models?.context_window(model)
}

impl InProcess {
    pub fn new(session: Rc<dyn SessionController>) -> Rc<InProcess> {
        Rc::new_cyclic(|this| InProcess { this: this.clone(), session, side: HandleSide::new() })
    }
    async fn serve(&self, op: Op) -> Result<Reply, RuntimeError> {
        let session = &self.session;
        Ok(match op {
            Op::Start(op::Start {}) => Reply::Start(session.start().await?),
            // Only a message with references takes the controller's way for them, so a session's calls stay as they were.
            Op::Submit(op::Submit { input, pinned, attachments, references }) => Reply::Submit(if !references.is_empty() {
                session.submit_with_references(&input, pinned, attachments, references).await?
            } else if attachments.is_empty() {
                session.submit(&input, pinned).await?
            } else {
                session.submit_with(&input, pinned, attachments).await?
            }),
            Op::References(op::References { query, limit }) => Reply::References(session.references(&query, limit).await?),
            Op::Steer(op::Steer { text }) => Reply::Steer(session.steer(&text)),
            Op::Aside(op::Aside { id, question }) => {
                Reply::Aside(self.side.aside(id, |on_text, signal| session.aside(&question, on_text, Some(signal))).await?)
            }
            Op::StopAside(op::StopAside { id }) => Reply::StopAside(self.side.stop_aside(id)),
            Op::Cancel(op::Cancel {}) => Reply::Cancel(session.cancel().await?),
            Op::Close(op::Close {}) => Reply::Close(session.close().await?),
            Op::Undo(op::Undo { id }) => Reply::Undo(session.undo(id.as_deref()).await?),
            Op::TakeBackPlan(op::TakeBackPlan {}) => Reply::TakeBackPlan(session.take_back_plan().await?),
            Op::TakeBack(op::TakeBack { to }) => Reply::TakeBack(session.take_back(to).await?),
            Op::Refresh(op::Refresh {}) => Reply::Refresh(session.refresh().await?),
            Op::NewConversation(op::NewConversation {}) => Reply::NewConversation(session.new_conversation().await?),
            Op::Reconnect(op::Reconnect {}) => Reply::Reconnect(session.reconnect().await?),
            Op::Reconfigure(op::Reconfigure {}) => Reply::Reconfigure(session.reconfigure().await?),
            Op::Instant(op::Instant { text }) => Reply::Instant(session.instant(&text).await?),
            Op::Pick(op::Pick { pick }) => Reply::Pick(session.picked(pick)),
            Op::Conversations(op::Conversations {}) => Reply::Conversations(session.conversations().await?),
            Op::AllConversations(op::AllConversations {}) => Reply::AllConversations(session.all_conversations().await?),
            Op::FindConversation(op::FindConversation { word }) => Reply::FindConversation(session.find_conversation(&word).await?),
            Op::ResumeConversation(op::ResumeConversation { id }) => Reply::ResumeConversation(session.resume_conversation(&id).await?),
            Op::RenameConversation(op::RenameConversation { id, name }) => {
                Reply::RenameConversation(session.rename_conversation(id.as_deref(), name.as_deref()).await?)
            }
            Op::PinConversation(op::PinConversation { id, pinned }) => Reply::PinConversation(session.pin_conversation(&id, pinned).await?),
            Op::DeleteConversation(op::DeleteConversation { id }) => Reply::DeleteConversation(session.delete_conversation(&id).await?),
            Op::ForkConversation(op::ForkConversation { name, before }) => Reply::ForkConversation(match before {
                Some(before) => session.fork_conversation_before(name.as_deref(), before).await?,
                None => session.fork_conversation(name.as_deref()).await?,
            }),
            Op::AnswerSet(op::AnswerSet { choice }) => Reply::AnswerSet(session.answer_set(choice).await?),
            Op::ConversationNow(op::ConversationNow {}) => Reply::ConversationNow(session.conversation_now().await),
            Op::ConversationLines(op::ConversationLines { id }) => Reply::ConversationLines(session.conversation_lines(&id).await?),
            Op::ConversationToExport(op::ConversationToExport {}) => Reply::ConversationToExport(session.conversation_to_export().await),
            Op::Memory(op::Memory {}) => Reply::Memory(session.memory().await?),
            Op::Remembered(op::Remembered {}) => Reply::Remembered(session.remembered().await),
            Op::Taste(op::Taste {}) => Reply::Taste(session.taste().await?),
            Op::ForgetTaste(op::ForgetTaste { id }) => Reply::ForgetTaste(session.forget_taste(&id).await?),
            Op::Forget(op::Forget { id }) => Reply::Forget(session.forget(&id).await?),
            Op::ChangeNote(op::ChangeNote { id, change }) => Reply::ChangeNote(session.change_note(&id, change).await?),
            Op::Recipes(op::Recipes {}) => Reply::Recipes(session.recipes().await?),
            Op::PinForRecipe(op::PinForRecipe { pin }) => Reply::PinForRecipe(session.pin_for_recipe(&pin)),
            Op::RunRecipe(op::RunRecipe { name, with }) => Reply::RunRecipe(session.run_recipe(&name, with).await?),
            Op::ForgetRecipe(op::ForgetRecipe { name }) => Reply::ForgetRecipe(session.forget_recipe(&name).await?),
            Op::Techniques(op::Techniques {}) => Reply::Techniques(session.techniques().await?),
            Op::ForgetTechnique(op::ForgetTechnique { id }) => Reply::ForgetTechnique(session.forget_technique(&id).await?),
            Op::AnswerTechnique(op::AnswerTechnique { keep }) => Reply::AnswerTechnique(session.answer_technique(keep).await?),
            Op::Lessons(op::Lessons {}) => Reply::Lessons(session.lessons().await?),
            Op::ForgetLesson(op::ForgetLesson { id }) => Reply::ForgetLesson(session.forget_lesson(&id).await?),
            Op::Goal(op::Goal { text }) => Reply::Goal(session.goal(text.as_deref()).await?),
            Op::StopGoal(op::StopGoal {}) => Reply::StopGoal(session.stop_goal().await?),
            Op::StopLoop(op::StopLoop {}) => Reply::StopLoop(session.stop_loop().await?),
            Op::StopDaw(op::StopDaw {}) => Reply::StopDaw(session.stop_live().await?),
            Op::DawMessage(op::DawMessage { text }) => Reply::DawMessage(session.daw_message(&text).await?),
            Op::DawRecording(op::DawRecording {}) => Reply::DawRecording(session.daw_recording().await?),
            Op::StopJob(op::StopJob { id }) => Reply::StopJob(session.stop_job(id)),
            Op::RunJobAgain(op::RunJobAgain { id }) => Reply::RunJobAgain(session.run_job_again(id)),
            Op::Background(op::Background {}) => Reply::Background(session.background()),
            Op::CarryOn(op::CarryOn { job }) => Reply::CarryOn(session.carry_on(job)),
            Op::Palette(op::Palette {}) => Reply::Palette(session.palette().await?),
            Op::PaletteGather(op::PaletteGather { files }) => Reply::PaletteGather(session.palette_gather(files).await?),
            Op::PaletteRemove(op::PaletteRemove { path, size, mtime }) => {
                Reply::PaletteRemove(session.palette_remove(&path, size, mtime).await?)
            }
            Op::PaletteCommand(op::PaletteCommand { words }) => Reply::PaletteCommand(session.palette_command(&words).await?),
            Op::PaletteNames(op::PaletteNames {}) => Reply::PaletteNames(session.palette_names().await?),
            Op::Music(op::Music {}) => Reply::Music(music_view(&part(session.music())?).await),
            Op::MusicFolders(op::MusicFolders {}) => Reply::MusicFolders(part(session.music())?.folders()),
            Op::LearnMusic(op::LearnMusic { folders }) => Reply::LearnMusic(part(session.music())?.learn(folders)),
            Op::SetMusicFolders(op::SetMusicFolders { folders }) => {
                Reply::SetMusicFolders(part(session.music())?.set_folders(Some(folders)))
            }
            Op::HoldMusic(op::HoldMusic { held }) => Reply::HoldMusic(part(session.music())?.hold(held)),
            Op::ForgetMusic(op::ForgetMusic {}) => {
                let music = part(session.music())?;
                Reply::ForgetMusic(music.forget().await)
            }
            Op::Map(op::Map {}) => {
                let load = part(session.map_loader())?;
                Reply::Map(load().await)
            }
            Op::Slots(op::Slots { id, words }) => Reply::Slots(self.side.slots(id, &words, &*part(session.slots_context())?).await),
            Op::StopSlots(op::StopSlots { id }) => Reply::StopSlots(self.side.stop_slots(id)),
            op @ (Op::Providers(_)
            | Op::LocalModels(_)
            | Op::ChooseDefault(_)
            | Op::Models(_)
            | Op::ChooseModel(_)
            | Op::SetEffort(_)
            | Op::SetFast(_)
            | Op::SetFallback(_)
            | Op::SaveKey(_)
            | Op::SignOut(_)
            | Op::SignInChatgpt(_)
            | Op::MissingModel(_)) => {
                // If this list and models_answer's ever part ways, a request the controls don't answer is "Not available", not a
                // panic.
                return models_answer(&op, session.model_controls(), &self.side)
                    .await
                    .unwrap_or_else(|| Err(RuntimeError::plain("Not available")));
            }
            Op::StopSignIn(op::StopSignIn { id }) => Reply::StopSignIn(self.side.stop_sign_in(id)),
            Op::FocusBody(op::FocusBody { request }) => Reply::FocusBody(session.focus_body(&request).await?),
        })
    }
}

#[async_trait(?Send)]
impl SessionHandle for InProcess {
    fn capabilities(&self) -> Capabilities {
        Capabilities::of(&*self.session)
    }
    fn can_show(&self, kind: BodyKind) -> bool {
        self.session.can_show(kind)
    }
    fn status(&self) -> SessionStatus {
        self.session.status()
    }
    fn answering_fallback(&self) -> Option<String> {
        self.session.answering_fallback()
    }
    fn library(&self) -> Option<LibraryStatus> {
        self.session.library()
    }
    fn open_set(&self) -> Option<String> {
        self.session.open_set()
    }
    fn jobs(&self) -> Vec<JobStatus> {
        self.session.jobs()
    }
    fn job(&self, id: u32) -> Option<JobDetail> {
        self.session.job(id)
    }
    fn can_background(&self) -> bool {
        self.session.can_background()
    }
    fn models(&self) -> Option<ModelsView> {
        models_view(self.session.model_controls())
    }
    fn provider_name(&self, id: &str) -> String {
        provider_name(self.session.model_controls(), id)
    }
    fn cloud_provider(&self, id: &str) -> Option<ProviderId> {
        cloud_provider(self.session.model_controls(), id)
    }
    fn context_window(&self, model: &str) -> Option<f64> {
        context_window(self.session.model_controls(), model)
    }
    fn listen(&self, on_event: Rc<dyn Fn(SessionEvent)>) {
        self.side.listen(on_event);
    }
    async fn send(&self, op: Op) -> Result<Reply, RuntimeError> {
        self.serve(op).await
    }
    fn tell(&self, op: Op) {
        let Some(this) = self.this.upgrade() else { return };
        tell_now(
            async move {
                let _ = this.serve(op).await;
            }
            .boxed_local(),
        );
    }
}
