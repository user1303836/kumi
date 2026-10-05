pub mod classify;
pub mod features;
pub mod learn;
pub mod learner;
pub mod manual;
pub mod measure_worker;
pub mod plan;
pub mod presets;
pub mod search;
pub mod sets;
pub mod sources;
pub mod state;
pub mod store;
pub mod taste;
pub mod tools;
pub mod xml;

use crate::{
    core::{
        contracts::{KernelTool, LibraryState as StatusState, LibraryStatus},
        errors::RuntimeError,
    },
    web::net::WebClient,
};
use async_trait::async_trait;
pub use classify::{SoundClass, SoundKind, CLASSES};
use indexmap::IndexMap;
use kumi_common::{
    abort::{self, Signal},
    time::now_ms,
};
use learn::{learn, Counts, LearnOptions, LearnPhase, PresetEntry, ProgressCallback, SetEntry, SoundEntry, LOG_VERSION};
pub use learn::{library_logs, LearnProgress};
use learner::{LearnerMessage, LearnerOptions, LearnerReply};
pub use manual::MANUAL_TOOL;
use measure_worker::{measure_reference, worker_binary, MeasureJob};
use plan::{plan_learning, remembered_file, remembered_folders, PlanOptions};
use search::SoundIndex;
pub use sources::{library_sources, Source};
use sources::{resolve, SourceOptions, SEP};
use state::{acquire_lock, write_state};
pub use state::{read_state, LibraryState};
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    path::Path,
    process::Stdio,
    rc::{Rc, Weak},
    time::Duration,
};
use store::{read_json, write_json, LogReader};
pub use taste::TasteLine;
use taste::{taste_instructions, Taste};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command,
    sync::{mpsc, watch, Mutex},
    task::JoinHandle,
};
pub use tools::LibraryToolsOptions;
use tools::{library_tools, LearningState, LibraryAccess};
pub use tools::{FIND_PRESETS_TOOL, FIND_SOUNDS_TOOL, MY_SETS_TOOL};
#[derive(Clone)]
pub struct LibraryWebOptions {
    pub client: Rc<dyn WebClient>,
    pub base: Option<String>,
}
#[derive(Clone, Default)]
pub struct LibraryOptions {
    pub dir: String,
    pub folders: Option<Vec<String>>,
    pub projects_dir: Option<String>,
    pub sources: Option<SourceOptions>,
    pub fork: Option<bool>,
    pub workers: Option<usize>,
    pub delay_ms: Option<u64>,
    pub every_ms: Option<u64>,
    pub find_sets: Option<bool>,
    pub web: Option<LibraryWebOptions>,
}
pub struct LearnNowOptions {
    pub rebuild: bool,
    pub signal: Signal,
    pub on_progress: Option<ProgressCallback>,
}
impl LearnNowOptions {
    pub fn new(signal: Signal) -> Self {
        Self { rebuild: false, signal, on_progress: None }
    }
}
struct LearnerHandle {
    id: u64,
    input: mpsc::UnboundedSender<LearnerMessage>,
    done: watch::Receiver<bool>,
    kill: Signal,
}
struct HeldIndex {
    index: Rc<SoundIndex>,
    key: String,
    at: i64,
}
pub struct Library {
    options: LibraryOptions,
    dir: String,
    weak: Weak<Self>,
    sounds: Mutex<LogReader<SoundEntry>>,
    presets: Mutex<LogReader<PresetEntry>>,
    sets: Mutex<LogReader<SetEntry>>,
    counts: Cell<(usize, usize, usize)>,
    listeners: RefCell<IndexMap<u64, Rc<dyn Fn(LibraryStatus)>>>,
    next: Cell<u64>,
    learner: RefCell<Option<LearnerHandle>>,
    in_process: RefCell<Option<Signal>>,
    progress: RefCell<Option<LearnProgress>>,
    saved: RefCell<Option<LibraryState>>,
    paused: Cell<bool>,
    closed: Cell<bool>,
    timer: RefCell<Option<JoinHandle<()>>>,
    last_told: Cell<i64>,
    told_timer: RefCell<Option<JoinHandle<()>>>,
    held: RefCell<Option<HeldIndex>>,
    stale: Cell<bool>,
    source_cache: RefCell<Option<(i64, Vec<Source>)>>,
    remembered: RefCell<Option<Vec<String>>>,
}
/// Create inside the runtime's LocalSet, as with the session/kernel's other local callbacks.
pub fn create_library(options: LibraryOptions) -> Rc<Library> {
    let dir = resolve(&options.dir);
    let library = Rc::new_cyclic(|weak| Library {
        sounds: Mutex::new(LogReader::new(Path::new(&dir).join("sounds.jsonl"), "sounds", LOG_VERSION)),
        presets: Mutex::new(LogReader::new(Path::new(&dir).join("presets.jsonl"), "presets", LOG_VERSION)),
        sets: Mutex::new(LogReader::new(Path::new(&dir).join("sets.jsonl"), "sets", LOG_VERSION)),
        options,
        dir,
        weak: weak.clone(),
        counts: Cell::new((0, 0, 0)),
        listeners: RefCell::new(IndexMap::new()),
        next: Cell::new(0),
        learner: RefCell::new(None),
        in_process: RefCell::new(None),
        progress: RefCell::new(None),
        saved: RefCell::new(None),
        paused: Cell::new(false),
        closed: Cell::new(false),
        timer: RefCell::new(None),
        last_told: Cell::new(0),
        told_timer: RefCell::new(None),
        held: RefCell::new(None),
        stale: Cell::new(false),
        source_cache: RefCell::new(None),
        remembered: RefCell::new(None),
    });
    let weak = Rc::downgrade(&library);
    tokio::task::spawn_local(async move {
        if let Some(library) = weak.upgrade() {
            let state = read_state(&library.dir).await;
            *library.saved.borrow_mut() = state;
            library.notify();
        }
    });
    library
}
fn cancel_timer(timer: &RefCell<Option<JoinHandle<()>>>) {
    if let Some(timer) = timer.borrow_mut().take() {
        timer.abort();
    }
}
fn io_error(error: std::io::Error) -> RuntimeError {
    RuntimeError::plain(error.to_string())
}
impl Library {
    fn planning(&self) -> PlanOptions {
        PlanOptions {
            dir: self.dir.clone(),
            folders: self.options.folders.clone(),
            projects_dir: self.options.projects_dir.clone(),
            sources: self.options.sources.clone(),
            find_sets: self.options.find_sets,
            workers: self.options.workers,
        }
    }
    async fn remembering(&self) -> Vec<String> {
        let known = self.remembered.borrow().clone();
        if let Some(known) = known {
            return known;
        }
        let remembered = remembered_folders(&self.dir).await;
        *self.remembered.borrow_mut() = Some(remembered.clone());
        remembered
    }
    pub fn sources(&self) -> Vec<Source> {
        if let Some((at, sources)) = &*self.source_cache.borrow() {
            if now_ms() - at < 60000 {
                return sources.clone();
            }
        }
        let mut options = self.options.sources.clone().unwrap_or_default();
        options.folders = Some(
            self.options
                .folders
                .clone()
                .unwrap_or_default()
                .into_iter()
                .chain(self.remembered.borrow().clone().unwrap_or_default())
                .collect(),
        );
        let sources = library_sources(&options);
        *self.source_cache.borrow_mut() = Some((now_ms(), sources.clone()));
        sources
    }
    pub fn status(&self) -> LibraryStatus {
        let saved = self.saved.borrow();
        let last = saved.as_ref().and_then(|s| s.last.as_ref());
        let learned_at = last.map(|l| l.finished_at).filter(|t| *t != 0);
        let progress = self.progress.borrow();
        let (known_sounds, known_presets, known_sets) = self.counts.get();
        let counts = progress.as_ref().map(|p| (p.sounds.known, p.presets.known, p.sets.known)).unwrap_or((
            known_sounds.max(last.map(|l| l.sounds).unwrap_or(0)),
            known_presets.max(last.map(|l| l.presets).unwrap_or(0)),
            known_sets.max(last.map(|l| l.sets).unwrap_or(0)),
        ));
        let running = progress.as_ref().filter(|p| p.phase != LearnPhase::Done);
        LibraryStatus {
            state: if running.is_some() {
                if self.paused.get() {
                    StatusState::Paused
                } else {
                    StatusState::Learning
                }
            } else if learned_at.is_some() {
                StatusState::Ready
            } else {
                StatusState::New
            },
            sounds: counts.0,
            presets: counts.1,
            sets: counts.2,
            todo: running.map(|p| p.sounds.todo),
            done: running.map(|p| p.sounds.done),
            learned_at,
        }
    }
    fn tell_now(&self) {
        self.last_told.set(now_ms());
        let status = self.status();
        let mut after = None;
        loop {
            let next = self
                .listeners
                .borrow()
                .iter()
                .find(|(id, _)| after.is_none_or(|after| **id > after))
                .map(|(id, listener)| (*id, listener.clone()));
            let Some((id, listener)) = next else { break };
            after = Some(id);
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| listener(status.clone())));
        }
    }
    fn notify(&self) {
        cancel_timer(&self.told_timer);
        let wait = 400 - (now_ms() - self.last_told.get());
        if wait <= 0 {
            self.tell_now();
        } else {
            let weak = self.weak.clone();
            *self.told_timer.borrow_mut() = Some(tokio::task::spawn_local(async move {
                tokio::time::sleep(Duration::from_millis(wait as u64)).await;
                if let Some(library) = weak.upgrade() {
                    library.told_timer.borrow_mut().take();
                    library.tell_now();
                }
            }));
        }
    }
    pub fn on_status(self: &Rc<Self>, listener: Rc<dyn Fn(LibraryStatus)>) -> Box<dyn FnOnce()> {
        let existing = self.listeners.borrow().iter().find(|(_, held)| Rc::ptr_eq(held, &listener)).map(|(id, _)| *id);
        let id = existing.unwrap_or_else(|| {
            let id = self.next.get();
            self.next.set(id + 1);
            self.listeners.borrow_mut().insert(id, listener);
            id
        });
        let weak = Rc::downgrade(self);
        Box::new(move || {
            if let Some(library) = weak.upgrade() {
                library.listeners.borrow_mut().shift_remove(&id);
            }
        })
    }
    pub fn start(self: &Rc<Self>) {
        if self.closed.get() {
            return;
        }
        cancel_timer(&self.timer);
        let weak = self.weak.clone();
        let delay = self.options.delay_ms.unwrap_or(4000);
        *self.timer.borrow_mut() = Some(tokio::task::spawn_local(async move {
            tokio::time::sleep(Duration::from_millis(delay)).await;
            if let Some(library) = weak.upgrade() {
                library.timer.borrow_mut().take();
                library.start_now();
                let _ = library.sound_index().await;
            }
        }));
    }
    pub fn pause(&self) {
        if self.paused.replace(true) {
            return;
        }
        self.tell_child(LearnerMessage::Pause);
        self.notify();
    }
    pub fn resume(&self) {
        if !self.paused.replace(false) {
            return;
        }
        self.tell_child(LearnerMessage::Resume);
        self.notify();
    }
    fn tell_child(&self, message: LearnerMessage) {
        if let Some(child) = self.learner.borrow().as_ref() {
            let _ = child.input.send(message);
        }
    }
    fn start_now(&self) {
        if self.closed.get() || self.learner.borrow().is_some() || self.in_process.borrow().is_some() {
            return;
        }
        cancel_timer(&self.timer);
        *self.source_cache.borrow_mut() = None;
        let status = self.status();
        *self.progress.borrow_mut() = Some(LearnProgress {
            sounds: Counts { known: status.sounds, ..Default::default() },
            presets: Counts { known: status.presets, ..Default::default() },
            sets: Counts { known: status.sets, ..Default::default() },
            started_at: now_ms(),
            ..Default::default()
        });
        self.notify();
        if self.options.fork == Some(false) {
            let weak = self.weak.clone();
            tokio::task::spawn_local(async move {
                if let Some(library) = weak.upgrade() {
                    let _ = library.learn_now(LearnNowOptions::new(Signal::new())).await;
                    library.finished().await;
                }
            });
            return;
        }
        let spawned = Command::new(worker_binary("kumi-library-learner", "KUMI_LIBRARY_LEARNER_BIN"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn();
        let Ok(mut child) = spawned else {
            let weak = self.weak.clone();
            tokio::task::spawn_local(async move {
                if let Some(library) = weak.upgrade() {
                    library.finished().await;
                }
            });
            return;
        };
        let mut input = child.stdin.take().unwrap();
        let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
        let (tx, mut rx) = mpsc::unbounded_channel::<LearnerMessage>();
        let (done_tx, done) = watch::channel(false);
        let kill = Signal::new();
        let killed = kill.clone();
        let id = self.next.get();
        self.next.set(id + 1);
        let _ =
            tx.send(LearnerMessage::Learn { options: LearnerOptions { plan: self.planning(), paused: self.paused.get(), rebuild: false } });
        *self.learner.borrow_mut() = Some(LearnerHandle { id, input: tx, done, kill });
        let weak = self.weak.clone();
        tokio::task::spawn_local(async move {
            let writer = tokio::task::spawn_local(async move {
                while let Some(message) = rx.recv().await {
                    let text = format!("{}\n", serde_json::to_string(&message).unwrap());
                    if input.write_all(text.as_bytes()).await.is_err() || input.flush().await.is_err() {
                        break;
                    }
                }
            });
            let mut reading = true;
            let mut terminated = false;
            loop {
                tokio::select! {
                    result=lines.next_line(),if reading=>match result{Ok(Some(line))=>{if let Ok(message)=serde_json::from_str::<LearnerReply>(&line){if let Some(library)=weak.upgrade(){match message{LearnerReply::Progress{progress}=>{*library.progress.borrow_mut()=Some(progress);library.notify();},LearnerReply::Done{progress}=>*library.progress.borrow_mut()=Some(progress),_=>{}}}}},_=>reading=false},
                    _=child.wait()=>break,
                    _=killed.cancelled(),if !terminated=>{terminated=true;#[cfg(unix)]if let Some(pid)=child.id(){unsafe{libc::kill(pid as i32,libc::SIGTERM);}}#[cfg(not(unix))]{let _=child.start_kill();}},
                }
            }
            writer.abort();
            let _ = done_tx.send(true);
            if let Some(library) = weak.upgrade() {
                let current = library.learner.borrow().as_ref().is_some_and(|c| c.id == id);
                if current {
                    library.learner.borrow_mut().take();
                    library.finished().await;
                }
            }
        });
    }
    async fn finished(&self) {
        *self.saved.borrow_mut() = read_state(&self.dir).await;
        *self.progress.borrow_mut() = None;
        *self.held.borrow_mut() = None;
        self.notify();
        if !self.closed.get() {
            cancel_timer(&self.timer);
            let weak = self.weak.clone();
            let delay = self.options.every_ms.unwrap_or(30 * 60000);
            *self.timer.borrow_mut() = Some(tokio::task::spawn_local(async move {
                tokio::time::sleep(Duration::from_millis(delay)).await;
                if let Some(library) = weak.upgrade() {
                    library.timer.borrow_mut().take();
                    library.start_now();
                }
            }));
        }
    }
    pub async fn learn_now(&self, run: LearnNowOptions) -> Result<Option<LearnProgress>, RuntimeError> {
        let Some(release) = acquire_lock(&self.dir).await.map_err(io_error)? else { return Ok(None) };
        let controller = Signal::new();
        *self.in_process.borrow_mut() = Some(controller.clone());
        let signal = abort::any([controller.clone(), run.signal]);
        let latest = Rc::new(RefCell::new(None::<LearnProgress>));
        let report_latest = latest.clone();
        let weak = self.weak.clone();
        let gate_signal = signal.clone();
        let mut options = LearnOptions::new(plan_learning(self.planning()).await, signal);
        options.rebuild = run.rebuild;
        options.gate = Some(Rc::new(move || {
            let weak = weak.clone();
            let signal = gate_signal.clone();
            Box::pin(async move {
                while weak.upgrade().is_some_and(|library| library.paused.get()) && !signal.is_cancelled() {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
            })
        }));
        let weak = self.weak.clone();
        options.on_progress = Some(Rc::new(move |progress| {
            *report_latest.borrow_mut() = Some(progress.clone());
            if let Some(library) = weak.upgrade() {
                *library.progress.borrow_mut() = Some(progress.clone());
                library.notify();
            }
            if let Some(report) = &run.on_progress {
                report(progress);
            }
        }));
        let result = async {
            let progress = learn(options).await?;
            write_state(&self.dir, &progress, false).await.map_err(io_error)?;
            *self.saved.borrow_mut() = read_state(&self.dir).await;
            Ok(progress)
        }
        .await;
        if result.is_err() {
            let latest = latest.borrow().clone();
            if let Some(latest) = latest {
                let _ = write_state(&self.dir, &latest, true).await;
            }
        }
        controller.cancel();
        self.in_process.borrow_mut().take();
        self.progress.borrow_mut().take();
        self.held.borrow_mut().take();
        release.release().await.map_err(io_error)?;
        self.notify();
        result.map(Some)
    }
    pub async fn sound_index(&self) -> Result<Rc<SoundIndex>, RuntimeError> {
        self.remembering().await;
        let mut reader = self.sounds.lock().await;
        if reader.refresh().await.map_err(io_error)?.changed {
            self.stale.set(true);
        }
        let (_, presets, sets) = self.counts.get();
        self.counts.set((reader.entries.len(), presets, sets));
        let current = self.sources();
        let key = current.iter().map(|s| s.path.as_str()).collect::<Vec<_>>().join("\n");
        let rebuild =
            self.held.borrow().as_ref().is_none_or(|held| {
                held.key != key || (self.stale.get() && (self.progress.borrow().is_none() || now_ms() - held.at >= 3000))
            });
        if rebuild {
            let entries = reader.entries.values().cloned().collect::<Vec<_>>();
            drop(reader);
            let index = Rc::new(SoundIndex::build(entries, &current).await);
            *self.held.borrow_mut() = Some(HeldIndex { index, key, at: now_ms() });
            self.stale.set(false);
        }
        Ok(self.held.borrow().as_ref().unwrap().index.clone())
    }
    async fn read_taste(&self) -> (Option<Taste>, HashSet<String>) {
        let taste = read_json::<Taste>(&Path::new(&self.dir).join("taste.json")).await;
        let forgotten = read_json::<Vec<serde_json::Value>>(&Path::new(&self.dir).join("forgotten.json"))
            .await
            .unwrap_or_default()
            .into_iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect();
        (taste, forgotten)
    }
    pub async fn instructions(&self) -> Result<String, RuntimeError> {
        let (taste, forgotten) = self.read_taste().await;
        Ok(taste.map(|taste| taste_instructions(&taste, &forgotten)).unwrap_or_default())
    }
    pub async fn taste(&self) -> Result<Vec<TasteLine>, RuntimeError> {
        let (taste, forgotten) = self.read_taste().await;
        Ok(taste.map(|t| t.lines.into_iter().filter(|l| !forgotten.contains(&l.id)).collect()).unwrap_or_default())
    }
    pub async fn forget_taste(&self, id: &str) -> Result<bool, RuntimeError> {
        let (taste, forgotten) = self.read_taste().await;
        if forgotten.contains(id) || taste.is_none_or(|t| !t.lines.iter().any(|l| l.id == id)) {
            return Ok(false);
        }
        let file = Path::new(&self.dir).join("forgotten.json");
        let mut ordered = read_json::<Vec<serde_json::Value>>(&file)
            .await
            .unwrap_or_default()
            .into_iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect::<indexmap::IndexSet<_>>();
        ordered.insert(id.into());
        write_json(&file, &ordered.into_iter().collect::<Vec<_>>()).await.map_err(io_error)?;
        Ok(true)
    }
    pub fn tools(self: &Rc<Self>, options: LibraryToolsOptions) -> Vec<Rc<dyn KernelTool>> {
        let mut tools = library_tools(self.clone(), options.clone());
        tools.push(manual::manual_tool(manual::ManualOptions {
            dir: self.dir.clone(),
            client: self.options.web.as_ref().map(|w| w.client.clone()),
            base: self.options.web.as_ref().and_then(|w| w.base.clone()),
            on_event: options.on_event,
            now: None,
        }));
        tools
    }
    pub async fn close(&self) {
        self.closed.set(true);
        cancel_timer(&self.timer);
        cancel_timer(&self.told_timer);
        self.listeners.borrow_mut().clear();
        if let Some(controller) = self.in_process.borrow().as_ref() {
            controller.cancel();
        }
        let child = self.learner.borrow().as_ref().map(|c| (c.input.clone(), c.done.clone(), c.kill.clone()));
        if let Some((input, mut done, kill)) = child {
            let _ = input.send(LearnerMessage::Stop);
            let wait = async {
                while !*done.borrow() {
                    if done.changed().await.is_err() {
                        break;
                    }
                }
            };
            if tokio::time::timeout(Duration::from_millis(1500), wait).await.is_err() {
                kill.cancel();
            }
        }
    }
}
#[async_trait(?Send)]
impl LibraryAccess for Library {
    async fn sounds(&self) -> Result<Rc<SoundIndex>, RuntimeError> {
        self.sound_index().await
    }
    async fn presets(&self) -> Result<Vec<PresetEntry>, RuntimeError> {
        let mut reader = self.presets.lock().await;
        reader.refresh().await.map_err(io_error)?;
        let (sounds, _, sets) = self.counts.get();
        self.counts.set((sounds, reader.entries.len(), sets));
        Ok(reader.entries.values().cloned().collect())
    }
    async fn sets(&self) -> Result<Vec<SetEntry>, RuntimeError> {
        let mut reader = self.sets.lock().await;
        reader.refresh().await.map_err(io_error)?;
        let (sounds, presets, _) = self.counts.get();
        self.counts.set((sounds, presets, reader.entries.len()));
        Ok(reader.entries.values().cloned().collect())
    }
    fn learning(&self) -> LearningState {
        let now = self.status();
        LearningState {
            learning: matches!(now.state, StatusState::Learning | StatusState::Paused),
            first: now.learned_at.is_none(),
            sounds: now.sounds,
            todo: now.todo,
            done: now.todo.map(|_| now.done.unwrap_or(0)),
        }
    }
    fn remember(&self, folders: Vec<String>) {
        let weak = self.weak.clone();
        tokio::task::spawn_local(async move {
            if let Some(library) = weak.upgrade() {
                let known = library.remembering().await;
                let sources = library.sources();
                let added: Vec<_> = folders
                    .into_iter()
                    .filter(|f| !known.contains(f) && !sources.iter().any(|s| *f == s.path || f.starts_with(&format!("{}{SEP}", s.path))))
                    .collect();
                if added.is_empty() {
                    return;
                }
                let mut remembered = known.into_iter().chain(added).collect::<Vec<_>>();
                if remembered.len() > 32 {
                    remembered.drain(..remembered.len() - 32);
                }
                *library.remembered.borrow_mut() = Some(remembered.clone());
                if write_json(Path::new(&remembered_file(&library.dir)), &remembered).await.is_ok() {
                    library.source_cache.borrow_mut().take();
                    library.start_now();
                }
            }
        });
    }
    fn folders(&self) -> Vec<String> {
        self.sources().into_iter().map(|s| s.path).collect()
    }
    async fn measure(&self, path: String, options: features::MeasureOptions) -> Result<SoundEntry, RuntimeError> {
        let relative = path.split(['\\', '/']).next_back().unwrap_or("").to_owned();
        measure_reference(
            MeasureJob { id: 0, path, relative, size: 0, mtime: 0, start: options.start, seconds: options.seconds },
            options.signal.unwrap_or_default(),
        )
        .await
    }
}
pub fn library_dir(kumi_dir: &str, env: Option<&HashMap<String, String>>) -> String {
    let value = env.and_then(|env| env.get("KUMI_LIBRARY_DIR").cloned()).or_else(|| {
        if env.is_none() {
            std::env::var("KUMI_LIBRARY_DIR").ok()
        } else {
            None
        }
    });
    value.filter(|s| !s.is_empty()).unwrap_or_else(|| sources::join(kumi_dir, "library"))
}
