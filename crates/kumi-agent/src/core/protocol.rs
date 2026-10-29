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
use futures::{task::noop_waker, FutureExt};
use kumi_common::abort::Signal;
use serde::{Deserialize, Serialize};

use crate::core::contracts::*;
use crate::core::jobs::{JobDetail, JobStatus};
use crate::core::palette::{Gathered, PaletteView};
use crate::core::search_memory::SearchMemoryStatus;
use kumi_types::contracts::*;
use kumi_types::errors::RuntimeError;

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
        }
    }
}

/// A request a front end makes, by its struct in [`op`]: it travels as an [`Op`], and its answer is a [`Reply`].
pub trait Request: Into<Op> {
    type Reply;
    /// This request's own answer, out of the session's; None when the session answered another request.
    fn answer(reply: Reply) -> Option<Self::Reply>;
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
    } -> ();
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
    /// A copy of this conversation to carry on in: its id, or None with nothing to copy yet.
    ForkConversation {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
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
    /// Whether ctrl-b has a step to move right now.
    fn can_background(&self) -> bool;
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

/// A session in this process, behind [`SessionController`]: each request goes to the method it was before the
/// protocol, and the state it shows is read from it as it's asked for.
pub struct InProcess {
    this: Weak<InProcess>,
    session: Rc<dyn SessionController>,
    on_event: RefCell<Option<Rc<dyn Fn(SessionEvent)>>>,
    /// The asides under way, by id, and what stops each.
    asides: RefCell<HashMap<u32, Signal>>,
}

impl InProcess {
    pub fn new(session: Rc<dyn SessionController>) -> Rc<InProcess> {
        Rc::new_cyclic(|this| InProcess { this: this.clone(), session, on_event: RefCell::new(None), asides: RefCell::new(HashMap::new()) })
    }
    fn emit(&self, event: SessionEvent) {
        let on_event = self.on_event.borrow().clone();
        if let Some(on_event) = on_event {
            on_event(event);
        }
    }
    async fn serve(&self, op: Op) -> Result<Reply, RuntimeError> {
        let session = &self.session;
        Ok(match op {
            Op::Start(op::Start {}) => Reply::Start(session.start().await?),
            Op::Submit(op::Submit { input, pinned, attachments }) => Reply::Submit(if attachments.is_empty() {
                session.submit(&input, pinned).await?
            } else {
                session.submit_with(&input, pinned, attachments).await?
            }),
            Op::Steer(op::Steer { text }) => Reply::Steer(session.steer(&text)),
            Op::Aside(op::Aside { id, question }) => {
                // A stop that came first left its signal stopped: the aside ends at once.
                let signal = self.asides.borrow_mut().entry(id).or_insert_with(Signal::new).clone();
                let this = self.this.clone();
                let on_text: OnText = Rc::new(move |text: &str| {
                    if let Some(this) = this.upgrade() {
                        this.emit(SessionEvent::Aside { id, text: text.into() });
                    }
                });
                let answer = session.aside(&question, on_text, Some(signal)).await;
                self.asides.borrow_mut().remove(&id);
                Reply::Aside(answer?)
            }
            Op::StopAside(op::StopAside { id }) => {
                // Before its aside starts too: the aside takes the stopped signal up as it starts (an id is never used
                // again, so what a late stop leaves is harmless).
                let signal = self.asides.borrow_mut().entry(id).or_insert_with(Signal::new).clone();
                signal.cancel();
                Reply::StopAside(())
            }
            Op::Cancel(op::Cancel {}) => Reply::Cancel(session.cancel().await?),
            Op::Close(op::Close {}) => Reply::Close(session.close().await?),
            Op::Undo(op::Undo { id }) => Reply::Undo(session.undo(id.as_deref()).await?),
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
            Op::ForkConversation(op::ForkConversation { name }) => {
                Reply::ForkConversation(session.fork_conversation(name.as_deref()).await?)
            }
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
    fn listen(&self, on_event: Rc<dyn Fn(SessionEvent)>) {
        *self.on_event.borrow_mut() = Some(on_event);
    }
    async fn send(&self, op: Op) -> Result<Reply, RuntimeError> {
        self.serve(op).await
    }
    fn tell(&self, op: Op) {
        let Some(this) = self.this.upgrade() else { return };
        // As far as it goes at once (all of it, for what the controller does at once), then the rest in a task of its own.
        let mut serving = async move {
            let _ = this.serve(op).await;
        }
        .boxed_local();
        if serving.as_mut().poll(&mut Context::from_waker(&noop_waker())).is_pending() {
            tokio::task::spawn_local(serving);
        }
    }
}
