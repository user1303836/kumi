//! A session on a thread of its own, behind [`SessionHandle`]: the front end's thread draws and reads keys while the
//! session works (a model's answer, the DAW's reads, a learn). The session is built on its thread and lives there, `Rc`s
//! and all. What crosses is the protocol's: requests go down; back up, in the order they happened, come the session's
//! events, what its reads say now ([`Snapshot`]), each request's reply, and a panic. The front end's side hands a reply to
//! whoever waits for it only once every event sent before it was handled, and the reads are sent before what changed
//! them, so a front end sees what it would have with the session in its own thread ([`InProcess`]).

use crate::core::{
    contracts::*,
    jobs::{JobDetail, JobStatus},
    protocol::{Capabilities, InProcess, ModelsView, Op, Reply, SessionHandle},
    view::BodyKind,
};
use async_trait::async_trait;
use futures::future::LocalBoxFuture;
use kumi_ai::providers::ProviderId;
use kumi_types::{contracts::LibraryStatus, errors::RuntimeError};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::{Rc, Weak},
};
use tokio::sync::{mpsc, oneshot};

/// What builds the session on its thread, given where its events go: its stores, its factories, the session itself.
pub type Build =
    Box<dyn FnOnce(Rc<dyn Fn(SessionEvent)>) -> LocalBoxFuture<'static, Result<Rc<dyn SessionController>, RuntimeError>> + Send>;

/// Every kind of body FOCUS shows, for which of them the session can show.
const SHOWN: [BodyKind; 9] = [
    BodyKind::Tree,
    BodyKind::Notes,
    BodyKind::Grid,
    BodyKind::Timeline,
    BodyKind::Tracker,
    BodyKind::Matrix,
    BodyKind::Keyzones,
    BodyKind::Routing,
    BodyKind::Mixer,
];
// A kind added later goes in SHOWN too: this match stops compiling without it.
fn _shown(kind: BodyKind) {
    match kind {
        BodyKind::Tree
        | BodyKind::Notes
        | BodyKind::Grid
        | BodyKind::Timeline
        | BodyKind::Tracker
        | BodyKind::Matrix
        | BodyKind::Keyzones
        | BodyKind::Routing
        | BodyKind::Mixer => {}
    }
}

/// What the session's reads say now. The front end's side answers them from this, sent whenever it changed and before
/// the event or reply that comes with it. Only what's cheap to read: a job's details come for the one watched alone.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    status: SessionStatus,
    answering_fallback: Option<String>,
    library: Option<LibraryStatus>,
    open_set: Option<String>,
    jobs: Vec<JobStatus>,
    can_background: bool,
    shows: Vec<BodyKind>,
    models: Option<ModelsView>,
    /// The model servers on this computer, by id and name: a provider's name is a server's own.
    servers: Vec<(String, String)>,
    /// The context windows of the model chosen and of the fallback, the two that answer.
    windows: Vec<(String, f64)>,
}

impl Snapshot {
    fn read(handle: &dyn SessionHandle, session: &dyn SessionController) -> Snapshot {
        let models = handle.models();
        let controls = session.model_controls();
        let answering = models.iter().flat_map(|view| [view.current.model.clone(), view.fallback.clone()]).flatten();
        let windows = answering.filter_map(|model| Some((model.clone(), handle.context_window(&model)?))).collect();
        Snapshot {
            status: handle.status(),
            answering_fallback: handle.answering_fallback(),
            library: handle.library(),
            open_set: handle.open_set(),
            jobs: handle.jobs(),
            can_background: handle.can_background(),
            shows: SHOWN.into_iter().filter(|kind| handle.can_show(*kind)).collect(),
            models,
            servers: controls.map(|models| models.server_names()).unwrap_or_default(),
            windows,
        }
    }
}

/// What goes down to the session's thread.
enum Down {
    Op {
        id: u64,
        op: Op,
    },
    /// The job whose details the front end shows (None: none now).
    Watch {
        job: Option<u32>,
    },
}

/// What comes back up, in the order it happened.
enum Back {
    /// First, once the session is built: what it can do, and what its reads say.
    Hello {
        capabilities: Capabilities,
        snapshot: Snapshot,
    },
    Event(SessionEvent),
    Snapshot(Snapshot),
    /// The details of the job watched, when they changed.
    Job(Option<(u32, JobDetail)>),
    Reply {
        id: u64,
        reply: Result<Reply, RuntimeError>,
    },
    /// The session's thread panicked, or the session couldn't be built: what it said. Nothing comes after it.
    Fatal(String),
}

thread_local! {
    /// On the session's thread: where a panic there goes.
    static PANICS: RefCell<Option<mpsc::UnboundedSender<Back>>> = const { RefCell::new(None) };
}

/// Whether a panic is the session's own, on its thread. Then it's sent to the front end, which ends in order (the
/// terminal given back, then the message), and the front end's hook leaves the terminal alone: false elsewhere.
pub fn session_panicked(info: &std::panic::PanicHookInfo) -> bool {
    PANICS.with(|panics| {
        let Ok(panics) = panics.try_borrow() else { return false };
        let Some(back) = panics.as_ref() else { return false };
        let said =
            info.payload().downcast_ref::<&str>().map(|said| said.to_string()).or_else(|| info.payload().downcast_ref::<String>().cloned());
        let at = info.location().map(|at| format!(" ({}:{})", at.file(), at.line())).unwrap_or_default();
        let _ = back.send(Back::Fatal(format!("{}{at}", said.as_deref().unwrap_or("it stopped"))));
        true
    })
}

/// The session's side: what it sends back, and what its reads said last.
struct Core {
    back: mpsc::UnboundedSender<Back>,
    /// The session and its handle in this thread, once built (what comes before goes once it's ready).
    serving: RefCell<Option<(Rc<InProcess>, Rc<dyn SessionController>)>>,
    early: RefCell<Vec<SessionEvent>>,
    last: RefCell<Option<Snapshot>>,
    watched: Cell<Option<u32>>,
    last_job: RefCell<Option<(u32, JobDetail)>>,
    /// The job's details are read again once, after what's under way now.
    job_due: Cell<bool>,
}

impl Core {
    /// The reads, sent first when they changed.
    fn reads_changed(&self) {
        let Some((handle, session)) = self.serving.borrow().clone() else { return };
        let now = Snapshot::read(&*handle, &*session);
        if self.last.borrow().as_ref() != Some(&now) {
            *self.last.borrow_mut() = Some(now.clone());
            let _ = self.back.send(Back::Snapshot(now));
        }
    }
    fn event(self: &Rc<Self>, event: SessionEvent) {
        if self.serving.borrow().is_none() {
            self.early.borrow_mut().push(event);
            return;
        }
        self.reads_changed();
        let _ = self.back.send(Back::Event(event));
        self.job_soon();
    }
    fn reply(self: &Rc<Self>, id: u64, reply: Result<Reply, RuntimeError>) {
        self.reads_changed();
        let _ = self.back.send(Back::Reply { id, reply });
        self.job_soon();
    }
    /// The watched job's details, read once after the events and replies under way (a step's output streams by
    /// pieces), and sent when they changed.
    fn job_soon(self: &Rc<Self>) {
        if self.watched.get().is_none() || self.job_due.replace(true) {
            return;
        }
        let core = self.clone();
        tokio::task::spawn_local(async move {
            tokio::task::yield_now().await;
            core.job_due.set(false);
            core.job_now();
        });
    }
    fn job_now(&self) {
        let Some((handle, _)) = self.serving.borrow().clone() else { return };
        let now = self.watched.get().and_then(|id| Some((id, handle.job(id)?)));
        if *self.last_job.borrow() != now {
            *self.last_job.borrow_mut() = now.clone();
            let _ = self.back.send(Back::Job(now));
        }
    }
}

/// The session's thread: its own runtime, the session built, then its requests served until the handle goes.
fn session_thread(build: Build, down: mpsc::UnboundedReceiver<Down>, back: mpsc::UnboundedSender<Back>) {
    PANICS.with(|panics| *panics.borrow_mut() = Some(back.clone()));
    let _let_go = LetGo;
    serve(build, down, back);
}

/// Lets go of where a panic went while the thread is whole, as `serve` returns or a panic unwinds it: the last sender's
/// drop closes its channel, which wakes the screen's side through tokio's own thread-locals, and those are gone once
/// the thread ends.
struct LetGo;

impl Drop for LetGo {
    fn drop(&mut self) {
        PANICS.with(|panics| panics.borrow_mut().take());
    }
}

fn serve(build: Build, mut down: mpsc::UnboundedReceiver<Down>, back: mpsc::UnboundedSender<Back>) {
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(error) => {
            let _ = back.send(Back::Fatal(format!("its thread couldn't start: {error}")));
            return;
        }
    };
    let local = tokio::task::LocalSet::new();
    local.block_on(&runtime, async move {
        let core = Rc::new(Core {
            back: back.clone(),
            serving: RefCell::new(None),
            early: RefCell::new(vec![]),
            last: RefCell::new(None),
            watched: Cell::new(None),
            last_job: RefCell::new(None),
            job_due: Cell::new(false),
        });
        let emit: Rc<dyn Fn(SessionEvent)> = {
            let core = core.clone();
            Rc::new(move |event| core.event(event))
        };
        let session = match build(emit.clone()).await {
            Ok(session) => session,
            Err(error) => {
                let _ = back.send(Back::Fatal(error.message()));
                return;
            }
        };
        let handle = InProcess::new(session.clone());
        // What the handle says of its own (an aside's words, a sign-in's code) goes back as the session's events do.
        handle.listen(emit);
        let snapshot = Snapshot::read(&*handle, &*session);
        *core.last.borrow_mut() = Some(snapshot.clone());
        let _ = back.send(Back::Hello { capabilities: handle.capabilities(), snapshot });
        *core.serving.borrow_mut() = Some((handle.clone(), session));
        for event in std::mem::take(&mut *core.early.borrow_mut()) {
            core.event(event);
        }
        // Each request served on its own, in the order they came, as the front end's thread served them.
        while let Some(down) = down.recv().await {
            match down {
                Down::Op { id, op } => {
                    let (handle, core) = (handle.clone(), core.clone());
                    tokio::task::spawn_local(async move {
                        let reply = handle.send(op).await;
                        core.reply(id, reply);
                    });
                }
                Down::Watch { job } => {
                    core.watched.set(job);
                    core.job_now();
                }
            }
        }
        // The handle is gone: what's under way ends with the thread, and the session goes with it (it holds where its
        // events go, which holds this side: let go of it here, or neither would ever drop).
        core.serving.borrow_mut().take();
    });
}

/// The front end's side of a session on its own thread.
pub struct Threaded {
    down: mpsc::UnboundedSender<Down>,
    capabilities: Capabilities,
    reads: RefCell<Snapshot>,
    job: RefCell<Option<(u32, JobDetail)>>,
    watched: Cell<Option<u32>>,
    waiting: RefCell<HashMap<u64, oneshot::Sender<Result<Reply, RuntimeError>>>>,
    next: Cell<u64>,
    on_event: RefCell<Option<Rc<dyn Fn(SessionEvent)>>>,
    on_fatal: RefCell<Option<Rc<dyn Fn(String)>>>,
    /// Why the session ended, once it did on its own (a panic, its thread gone).
    ended: RefCell<Option<String>>,
}

impl Threaded {
    /// The session `build` makes, on a thread of its own, once it's ready. Why not, when it couldn't be built, or its
    /// thread panicked or ended first.
    pub async fn spawn(build: Build) -> Result<Rc<Threaded>, RuntimeError> {
        let (down, down_rx) = mpsc::unbounded_channel();
        let (back_tx, mut back) = mpsc::unbounded_channel();
        std::thread::Builder::new()
            .name("kumi-session".into())
            .spawn(move || session_thread(build, down_rx, back_tx))
            .map_err(|error| RuntimeError::plain(format!("its thread couldn't start: {error}")))?;
        let (capabilities, snapshot) = match back.recv().await {
            Some(Back::Hello { capabilities, snapshot }) => (capabilities, snapshot),
            Some(Back::Fatal(why)) => return Err(RuntimeError::plain(why)),
            _ => return Err(RuntimeError::plain("its session ended before it was ready")),
        };
        let threaded = Rc::new(Threaded {
            down,
            capabilities,
            reads: RefCell::new(snapshot),
            job: RefCell::new(None),
            watched: Cell::new(None),
            waiting: RefCell::new(HashMap::new()),
            next: Cell::new(1),
            on_event: RefCell::new(None),
            on_fatal: RefCell::new(None),
            ended: RefCell::new(None),
        });
        tokio::task::spawn_local(pump(Rc::downgrade(&threaded), back));
        Ok(threaded)
    }
    /// Where the session's ending goes, when it ends on its own (its thread panicked, or ended unasked): what it said.
    /// Kumi ends then. One that ended already is said at once.
    pub fn on_fatal(&self, fatal: Rc<dyn Fn(String)>) {
        *self.on_fatal.borrow_mut() = Some(fatal.clone());
        let ended = self.ended.borrow().clone();
        if let Some(why) = ended {
            fatal(why);
        }
    }
    fn id(&self) -> u64 {
        let id = self.next.get();
        self.next.set(id + 1);
        id
    }
    fn end(&self, why: String) {
        *self.ended.borrow_mut() = Some(why.clone());
        // (whoever still waits hears the session ended)
        self.waiting.borrow_mut().clear();
        let fatal = self.on_fatal.borrow().clone();
        if let Some(fatal) = fatal {
            fatal(why);
        }
    }
}

/// What comes back from the session's thread, taken in order: an event to the front end, the reads stored, a reply to
/// whoever waits for it. A reply is handed over only once everything sent before it was.
async fn pump(threaded: Weak<Threaded>, mut back: mpsc::UnboundedReceiver<Back>) {
    let why = loop {
        let message = back.recv().await;
        let Some(threaded) = threaded.upgrade() else { return };
        match message {
            Some(Back::Hello { .. }) => {}
            Some(Back::Event(event)) => {
                let on_event = threaded.on_event.borrow().clone();
                if let Some(on_event) = on_event {
                    on_event(event);
                }
            }
            Some(Back::Snapshot(reads)) => *threaded.reads.borrow_mut() = reads,
            Some(Back::Job(job)) => *threaded.job.borrow_mut() = job,
            Some(Back::Reply { id, reply }) => {
                let waiting = threaded.waiting.borrow_mut().remove(&id);
                if let Some(waiting) = waiting {
                    let _ = waiting.send(reply);
                }
            }
            Some(Back::Fatal(why)) => break why,
            None => break "Kumi's session ended unasked".to_string(),
        }
    };
    if let Some(threaded) = threaded.upgrade() {
        threaded.end(why);
    }
}

#[async_trait(?Send)]
impl SessionHandle for Threaded {
    fn capabilities(&self) -> Capabilities {
        self.capabilities
    }
    fn can_show(&self, kind: BodyKind) -> bool {
        self.reads.borrow().shows.contains(&kind)
    }
    fn status(&self) -> SessionStatus {
        self.reads.borrow().status.clone()
    }
    fn answering_fallback(&self) -> Option<String> {
        self.reads.borrow().answering_fallback.clone()
    }
    fn library(&self) -> Option<LibraryStatus> {
        self.reads.borrow().library.clone()
    }
    fn open_set(&self) -> Option<String> {
        self.reads.borrow().open_set.clone()
    }
    fn jobs(&self) -> Vec<JobStatus> {
        self.reads.borrow().jobs.clone()
    }
    /// The job's details, once its thread has sent them: asking for another job's watches that one instead.
    fn job(&self, id: u32) -> Option<JobDetail> {
        if self.watched.replace(Some(id)) != Some(id) {
            let _ = self.down.send(Down::Watch { job: Some(id) });
        }
        self.job.borrow().as_ref().filter(|(at, _)| *at == id).map(|(_, detail)| detail.clone())
    }
    /// Its thread stops reading them.
    fn unwatch_job(&self) {
        if self.watched.take().is_some() {
            self.job.borrow_mut().take();
            let _ = self.down.send(Down::Watch { job: None });
        }
    }
    fn can_background(&self) -> bool {
        self.reads.borrow().can_background
    }
    fn models(&self) -> Option<ModelsView> {
        self.reads.borrow().models.clone()
    }
    fn provider_name(&self, id: &str) -> String {
        let reads = self.reads.borrow();
        match reads.servers.iter().find(|(server, _)| server == id) {
            Some((_, name)) => name.clone(),
            None => ProviderId::parse(id).map_or_else(|| id.into(), |provider| provider.row().name.into()),
        }
    }
    fn cloud_provider(&self, id: &str) -> Option<ProviderId> {
        ProviderId::parse(id).filter(|_| !self.reads.borrow().servers.iter().any(|(server, _)| server == id))
    }
    fn context_window(&self, model: &str) -> Option<f64> {
        self.reads.borrow().windows.iter().find(|(at, _)| at == model).map(|(_, window)| *window)
    }
    fn listen(&self, on_event: Rc<dyn Fn(SessionEvent)>) {
        *self.on_event.borrow_mut() = Some(on_event);
    }
    async fn send(&self, op: Op) -> Result<Reply, RuntimeError> {
        if let Some(why) = self.ended.borrow().clone() {
            return Err(RuntimeError::plain(why));
        }
        let id = self.id();
        let (answered, answer) = oneshot::channel();
        self.waiting.borrow_mut().insert(id, answered);
        if self.down.send(Down::Op { id, op }).is_err() {
            self.waiting.borrow_mut().remove(&id);
            return Err(RuntimeError::plain("Kumi's session has ended"));
        }
        answer.await.map_err(|_| RuntimeError::plain("Kumi's session ended before it answered"))?
    }
    /// Sent now; nobody waits for its reply, which is dropped as it comes.
    fn tell(&self, op: Op) {
        let id = self.id();
        let _ = self.down.send(Down::Op { id, op });
    }
}

// What crosses between the threads is Send, as the channels need: said here so a type that stops being one is named.
const _: () = {
    fn sent<T: Send>() {}
    #[allow(dead_code)]
    fn crosses() {
        sent::<Op>();
        sent::<Reply>();
        sent::<SessionEvent>();
        sent::<Snapshot>();
        sent::<JobDetail>();
        sent::<RuntimeError>();
        sent::<Build>();
    }
};
