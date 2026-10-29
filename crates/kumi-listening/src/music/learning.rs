//! The producer's music as the app holds it: whether Kumi learns it (their music folders), how the scan is going (a
//! `kumi __library-learner --music` process, started again every half hour to find new songs), and pausing it while
//! Kumi listens or renders, or when the producer asks.

use super::{
    database,
    hear::Style,
    store::MusicStore,
    worker::{run, Control, ScanMessage, ScanOptions, ScanPhase, ScanProgress, ScanReply},
};
use indexmap::IndexMap;
use kumi_common::{abort::Signal, time::now_ms};
use kumi_library::{daws::Daws, measure_worker::worker_program, state::release_ended};
use serde::{Deserialize, Serialize};
use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    process::Stdio,
    rc::{Rc, Weak},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command,
    sync::{mpsc, watch},
    task::JoinHandle,
};

/// How learning the producer's music goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MusicState {
    /// No music folders: Kumi doesn't learn it.
    Off,
    Learning,
    Paused,
    /// Every song heard that can be, for now.
    Done,
    /// Another Kumi is learning it now.
    Elsewhere,
}

/// What Kumi knows of the producer's music, and how learning it is going (a `{ type: "music" }` session event).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MusicStatus {
    pub state: MusicState,
    pub folders: Vec<String>,
    pub songs: usize,
    pub light: usize,
    pub full: usize,
    pub skipped: usize,
    /// Songs not heard yet because they're only in the cloud: heard once they're on this computer.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub cloud: usize,
    /// When a scan last heard everything it could.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub learned_at: Option<i64>,
    /// The scan's latest word: a model it's fetching, or why it stopped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub said: Option<String>,
}

impl MusicStatus {
    /// Songs heard, lightly or in full.
    pub fn heard(&self) -> usize {
        self.light + self.full
    }
}

fn is_zero(count: &usize) -> bool {
    *count == 0
}

/// Why a folder can't be added while Kumi forgets the producer's music.
pub const FORGETTING: &str = "Kumi is still forgetting your music: add the folder again in a moment.";

/// Keeps the producer's answer (settings.json's `musicFolders` and `musicOffered`): the folders they agreed Kumi may
/// learn from (none until they agree), and whether Kumi offered.
pub type SaveMusic = Rc<dyn Fn(Option<&[String]>, bool) -> Result<(), String>>;

#[derive(Clone)]
pub struct MusicOptions {
    /// Kumi's music folder.
    pub dir: PathBuf,
    /// The producer's music folders (`musicFolders`): none until they agree, and Kumi doesn't learn their music without
    /// any.
    pub folders: Option<Vec<String>>,
    /// Kumi offered to learn their music (`musicOffered`).
    pub offered: bool,
    /// Whether another Kumi has offered since this one read `offered` (none: nothing else to ask).
    pub offered_since: Option<Rc<dyn Fn() -> bool>>,
    /// Where their answer is kept (none: nowhere, as in tests).
    pub save: Option<SaveMusic>,
    /// Their sample folders (`libraryFolders`), which the walk leaves out with the library's places.
    pub library_folders: Vec<String>,
    /// false: the scan runs in this process (tests), with `style` as its style model when there's one (taste hears
    /// with it too).
    pub fork: bool,
    pub style: Option<Rc<dyn Style>>,
    /// Where to look for music before the producer names a folder (tests); none, the usual places.
    pub places: Option<Vec<PathBuf>>,
    /// How long after Kumi starts the first scan begins, and how long between scans.
    pub delay_ms: u64,
    pub every_ms: u64,
    /// What the DAWs the app registers tell the library (their places, which the walk leaves out).
    pub daws: Daws,
    /// Where taste keeps the references it makes from their songs (none: nowhere, as in tests), which a forget takes
    /// too.
    pub references: Option<PathBuf>,
}

impl MusicOptions {
    pub fn new(dir: PathBuf, folders: Option<Vec<String>>, daws: Daws) -> MusicOptions {
        MusicOptions {
            dir,
            folders,
            offered: false,
            offered_since: None,
            save: None,
            library_folders: vec![],
            fork: true,
            style: None,
            places: None,
            delay_ms: 4_000,
            every_ms: 30 * 60_000,
            daws,
            references: None,
        }
    }
    /// The folders a scan walks: none until the producer agreed.
    fn walked(&self) -> Vec<String> {
        self.folders.clone().unwrap_or_default()
    }
}

/// The scan under way: a child process told over its stdin (`heard`: whether it has said anything yet; `pid`: its
/// process, whose lock is let go of for it when it's ended), or one in this process.
enum Running {
    Child {
        id: u64,
        input: mpsc::UnboundedSender<ScanMessage>,
        done: watch::Receiver<bool>,
        kill: Signal,
        heard: Rc<Cell<bool>>,
        pid: Option<u32>,
    },
    Here {
        id: u64,
        control: Control,
    },
}

impl Running {
    fn id(&self) -> u64 {
        match self {
            Running::Child { id, .. } | Running::Here { id, .. } => *id,
        }
    }
}

/// Who hears how learning goes.
type Listener = Rc<dyn Fn(MusicStatus)>;

pub struct Music {
    options: RefCell<MusicOptions>,
    weak: Weak<Music>,
    status: RefCell<MusicStatus>,
    listeners: RefCell<IndexMap<u64, Listener>>,
    next: Cell<u64>,
    running: RefCell<Option<Running>>,
    /// Paused while Kumi listens or renders (the transport plays), and by the producer.
    playing: Cell<bool>,
    held: Cell<bool>,
    /// Learn again as soon as the scan under way has stopped (the folders changed, or the producer asked).
    again: Cell<bool>,
    closed: Cell<bool>,
    /// Counts forgets: music.db's counts read before one are out of date after it.
    forgets: Cell<u64>,
    /// A forget under way: no folder is added (and nothing written) until it's done.
    forgetting: Cell<bool>,
    timer: RefCell<Option<JoinHandle<()>>>,
}

fn cancel(timer: &RefCell<Option<JoinHandle<()>>>) {
    if let Some(timer) = timer.borrow_mut().take() {
        timer.abort();
    }
}

/// Create inside the runtime's LocalSet: its scan reports through local tasks.
pub fn create_music(options: MusicOptions) -> Rc<Music> {
    let state = if options.walked().is_empty() { MusicState::Off } else { MusicState::Done };
    let status =
        MusicStatus { state, folders: options.walked(), songs: 0, light: 0, full: 0, skipped: 0, cloud: 0, learned_at: None, said: None };
    let music = Rc::new_cyclic(|weak| Music {
        options: RefCell::new(options),
        weak: weak.clone(),
        status: RefCell::new(status),
        listeners: RefCell::new(IndexMap::new()),
        next: Cell::new(0),
        running: RefCell::new(None),
        playing: Cell::new(false),
        held: Cell::new(false),
        again: Cell::new(false),
        closed: Cell::new(false),
        forgets: Cell::new(0),
        forgetting: Cell::new(false),
        timer: RefCell::new(None),
    });
    music.read_counts();
    music
}

impl Music {
    pub fn dir(&self) -> PathBuf {
        self.options.borrow().dir.clone()
    }

    /// The folders Kumi learns from (none: it doesn't).
    pub fn folders(&self) -> Vec<String> {
        self.options.borrow().walked()
    }

    /// Whether the producer agreed Kumi may learn their music (they may have taken every folder away since).
    pub fn agreed(&self) -> bool {
        self.options.borrow().folders.is_some()
    }

    /// Whether Kumi offered to learn it: this Kumi, or another since this one started.
    pub fn offered(&self) -> bool {
        let options = self.options.borrow();
        options.offered || options.offered_since.as_ref().is_some_and(|offered| offered())
    }

    /// The style model a test gave (none: CLAP, as the embeddings slot runs it).
    pub fn style(&self) -> Option<Rc<dyn Style>> {
        self.options.borrow().style.clone()
    }

    /// Where to look for their music before they name a folder.
    pub fn places(&self) -> Vec<PathBuf> {
        self.options.borrow().places.clone().unwrap_or_else(super::folders::usual_places)
    }

    /// The producer's sample folders, which a walk (and the count for the offer) leaves out.
    pub fn library_folders(&self) -> Vec<String> {
        self.options.borrow().library_folders.clone()
    }

    /// What every walk leaves out, so the offer counts the songs Kumi would learn: their sample folders and the library's
    /// places (Live's User Library and Packs, under ~/Music on a Mac). Found where it's called (it looks at the DAWs'
    /// own settings): off the app's thread.
    pub fn skipping(&self) -> impl FnOnce() -> Vec<String> + Send + 'static {
        let (daws, folders) = {
            let options = self.options.borrow();
            (options.daws, options.library_folders.clone())
        };
        move || super::worker::skipped_folders(daws, &folders)
    }

    /// Whether a forget is under way.
    pub fn forgetting(&self) -> bool {
        self.forgetting.get()
    }

    fn save(&self) -> Result<(), String> {
        let options = self.options.borrow().clone();
        match &options.save {
            Some(save) => save(options.folders.as_deref(), options.offered),
            None => Ok(()),
        }
    }

    /// Kumi offered to learn their music: it's kept, so it's offered once.
    pub fn offer_made(&self) -> Result<(), String> {
        self.options.borrow_mut().offered = true;
        self.save()
    }

    /// The producer said yes: these folders are kept as theirs, and learning starts now.
    pub fn learn(self: &Rc<Self>, folders: Vec<String>) -> Result<(), String> {
        self.options.borrow_mut().offered = true;
        self.set_folders(Some(folders))
    }

    /// Forget what Kumi learned of their music (music.db, the style vectors, the map, and the references taste made from
    /// their songs): the scan stops, the folder goes, and Kumi learns nothing more until they add a folder again (it
    /// doesn't offer again). All of it goes even when their settings can't be written; then that's said.
    pub async fn forget(self: &Rc<Self>) -> Result<(), String> {
        self.forgetting.set(true);
        let saved = self.set_folders(None);
        self.stopped().await;
        self.forgets.set(self.forgets.get() + 1);
        let dir = self.dir();
        let mut removed = tokio::fs::remove_dir_all(&dir).await;
        // A file the scan held a moment longer (Windows): once more, shortly.
        if removed.as_ref().is_err_and(|error| error.kind() != std::io::ErrorKind::NotFound) {
            tokio::time::sleep(Duration::from_millis(500)).await;
            removed = tokio::fs::remove_dir_all(&dir).await;
        }
        let references = self.options.borrow().references.clone();
        let references = match references {
            Some(folder) => crate::references::store::ReferenceStore::new(folder).forget_kind("music").await,
            None => Ok(()),
        };
        self.update(|status| {
            (status.songs, status.light, status.full, status.skipped, status.cloud, status.learned_at) = (0, 0, 0, 0, 0, None)
        });
        self.forgetting.set(false);
        match (removed, references, saved) {
            (Err(error), ..) if error.kind() != std::io::ErrorKind::NotFound => Err(format!("Kumi couldn't delete {}: {error}", dir.display())),
            (_, Err(why), _) => Err(format!("Kumi couldn't delete the references it kept from your songs: {why}")),
            (_, _, Err(why)) => Err(format!(
                "Kumi forgot your music, but couldn't take your folders out of its settings ({why}): it learns from them again when it starts, unless you /music remove them then."
            )),
            _ => Ok(()),
        }
    }

    /// Waits (a moment and a half at most, then ends it) for the scan under way to stop.
    async fn stopped(&self) {
        let child = match self.running.borrow().as_ref() {
            Some(Running::Child { done, kill, .. }) => Some((done.clone(), kill.clone())),
            _ => None,
        };
        if let Some((mut done, kill)) = child {
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
        for _ in 0..100 {
            if self.running.borrow().is_none() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    pub fn status(&self) -> MusicStatus {
        self.status.borrow().clone()
    }

    pub fn on_status(self: &Rc<Self>, listener: Listener) -> Box<dyn FnOnce()> {
        let id = self.next.get();
        self.next.set(id + 1);
        self.listeners.borrow_mut().insert(id, listener);
        let weak = Rc::downgrade(self);
        Box::new(move || {
            if let Some(music) = weak.upgrade() {
                music.listeners.borrow_mut().shift_remove(&id);
            }
        })
    }

    fn tell(&self) {
        let status = self.status();
        let listeners: Vec<_> = self.listeners.borrow().values().cloned().collect();
        for listener in listeners {
            listener(status.clone());
        }
    }

    fn update(&self, change: impl FnOnce(&mut MusicStatus)) {
        let before = self.status();
        change(&mut self.status.borrow_mut());
        if self.status() != before {
            self.tell();
        }
    }

    /// The counts music.db holds now, read off the app's thread.
    fn read_counts(&self) {
        let (weak, file, forgets) = (self.weak.clone(), database(&self.dir()), self.forgets.get());
        tokio::task::spawn_local(async move {
            let read = tokio::task::spawn_blocking(move || {
                let store = MusicStore::open_read(&file).ok().flatten()?;
                let learned_at = store.meta("learned_at").ok().flatten().and_then(|at| at.parse::<i64>().ok());
                Some((store.counts().ok()?, learned_at))
            })
            .await
            .ok()
            .flatten();
            // Read before a forget: music.db went since, and its counts with it.
            let music = weak.upgrade().filter(|music| music.forgets.get() == forgets);
            if let (Some(music), Some((counts, learned_at))) = (music, read) {
                music.update(|status| {
                    (status.songs, status.light, status.full, status.skipped, status.cloud) =
                        (counts.songs, counts.light, counts.full, counts.skipped, counts.cloud);
                    status.learned_at = status.learned_at.max(learned_at);
                });
            }
        });
    }

    fn paused(&self) -> bool {
        self.playing.get() || self.held.get()
    }

    fn send(&self, message: ScanMessage) {
        match self.running.borrow().as_ref() {
            Some(Running::Child { input, .. }) => {
                let _ = input.send(message);
            }
            Some(Running::Here { control, .. }) => match message {
                ScanMessage::Pause => control.pause(true),
                ScanMessage::Resume => control.pause(false),
                ScanMessage::Stop => control.stop(),
                ScanMessage::Learn { .. } => {}
            },
            None => {}
        }
    }

    fn paused_changed(&self, was: bool) {
        if self.paused() == was {
            return;
        }
        self.send(if self.paused() { ScanMessage::Pause } else { ScanMessage::Resume });
        self.update(|status| match status.state {
            MusicState::Learning if self.paused() => status.state = MusicState::Paused,
            MusicState::Paused if !self.paused() => status.state = MusicState::Learning,
            _ => {}
        });
    }

    /// Kumi is listening or rendering (the transport plays): the scan waits, as the library's learner does.
    pub fn set_playing(&self, playing: bool) {
        let was = self.paused();
        self.playing.set(playing);
        self.paused_changed(was);
    }

    /// The producer paused learning (or let it go on).
    pub fn hold(&self, held: bool) {
        let was = self.paused();
        self.held.set(held);
        self.paused_changed(was);
    }

    pub fn held(&self) -> bool {
        self.held.get()
    }

    /// Learn from these folders from now on (kept as the producer's): the scan under way stops, and one over the new
    /// folders starts. None, or none left: Kumi stops learning; with none left, what it heard is set aside as gone (a
    /// folder added again brings back the songs found as they were).
    pub fn set_folders(self: &Rc<Self>, folders: Option<Vec<String>>) -> Result<(), String> {
        if self.forgetting.get() && folders.as_ref().is_some_and(|folders| !folders.is_empty()) {
            return Err(FORGETTING.into());
        }
        self.options.borrow_mut().folders = folders;
        let walked = self.folders();
        self.update(|status| status.folders = walked.clone());
        if walked.is_empty() {
            cancel(&self.timer);
            self.send(ScanMessage::Stop);
            self.update(|status| status.state = MusicState::Off);
            if self.agreed() {
                self.set_aside();
            }
        } else {
            self.learn_now();
        }
        self.save()
    }

    /// Every song set aside as gone once the scan under way has stopped, while no folder is left.
    fn set_aside(&self) {
        let weak = self.weak.clone();
        tokio::task::spawn_local(async move {
            let Some(music) = weak.upgrade() else { return };
            music.stopped().await;
            if !music.folders().is_empty() || !music.agreed() || music.forgetting.get() {
                return;
            }
            let file = database(&music.dir());
            let _ = tokio::task::spawn_blocking(move || MusicStore::open_existing(&file).ok().flatten().map(|store| store.set_all_gone()))
                .await;
            music.read_counts();
        });
    }

    /// Start learning a moment after Kumi starts (nothing, without music folders).
    pub fn start(self: &Rc<Self>) {
        if self.closed.get() || self.folders().is_empty() {
            return;
        }
        cancel(&self.timer);
        let weak = self.weak.clone();
        let delay = self.options.borrow().delay_ms;
        *self.timer.borrow_mut() = Some(tokio::task::spawn_local(async move {
            tokio::time::sleep(Duration::from_millis(delay)).await;
            if let Some(music) = weak.upgrade() {
                music.timer.borrow_mut().take();
                music.start_now();
            }
        }));
    }

    fn heard(&self, reply: ScanReply) {
        // Kumi stopped learning (forgotten, or every folder taken away) while a scan still ran: what it says as it stops
        // changes nothing.
        if self.folders().is_empty() {
            return;
        }
        let progress_of = |status: &mut MusicStatus, progress: &ScanProgress| {
            (status.songs, status.light, status.full, status.skipped, status.cloud) =
                (progress.songs, progress.light, progress.full, progress.skipped, progress.cloud);
            if let Some(why) = &progress.unstyled {
                status.said = Some(format!("The style model couldn't hear: {why}"));
            }
        };
        let paused = self.paused();
        self.update(|status| match reply {
            ScanReply::Progress { progress } => {
                progress_of(status, &progress);
                status.state = if paused { MusicState::Paused } else { MusicState::Learning };
            }
            ScanReply::Done { progress } => {
                progress_of(status, &progress);
                status.state = MusicState::Done;
                status.learned_at = progress.finished_at.or(Some(now_ms()));
            }
            ScanReply::Busy => status.state = MusicState::Elsewhere,
            ScanReply::Failed { message } => status.said = Some(message),
            ScanReply::Said { text } => status.said = Some(text),
        });
    }

    fn start_now(&self) {
        let options = self.options.borrow().clone();
        if self.closed.get() || options.walked().is_empty() || self.running.borrow().is_some() {
            return;
        }
        cancel(&self.timer);
        let id = self.next.get();
        self.next.set(id + 1);
        let paused = self.paused();
        self.update(|status| status.state = if paused { MusicState::Paused } else { MusicState::Learning });
        let scan = ScanOptions {
            dir: options.dir.to_string_lossy().into_owned(),
            folders: options.walked(),
            library_folders: options.library_folders.clone(),
            paused,
            on_battery: false,
        };
        let weak = self.weak.clone();
        if !options.fork {
            let control = Control::new(false);
            control.pause(paused);
            *self.running.borrow_mut() = Some(Running::Here { id, control: control.clone() });
            tokio::task::spawn_local(async move {
                let report = |reply: ScanReply| {
                    if let Some(music) = weak.upgrade() {
                        music.heard(reply);
                    }
                };
                match run(&scan, options.daws, &control, options.style.clone(), &report).await {
                    Ok(progress) if progress.phase == ScanPhase::Done => report(ScanReply::Done { progress }),
                    Ok(progress) => report(ScanReply::Progress { progress }),
                    Err(error) => report(ScanReply::Failed { message: error.to_string() }),
                }
                if let Some(music) = weak.upgrade() {
                    music.finished(id);
                }
            });
            return;
        }
        let spawned = worker_program("library-learner", "KUMI_LIBRARY_LEARNER_BIN")
            .command()
            .arg("--music")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn();
        let Ok(mut child) = spawned else {
            self.update(|status| {
                status.said = Some("Kumi's learner couldn't start; it tries again later.".into());
                status.state = MusicState::Done;
            });
            self.next_scan(options.every_ms);
            return;
        };
        let pid = child.id();
        let mut input = child.stdin.take().unwrap();
        let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
        let (sender, mut messages) = mpsc::unbounded_channel::<ScanMessage>();
        let (done_sender, done) = watch::channel(false);
        let kill = Signal::new();
        let killed = kill.clone();
        let heard = Rc::new(Cell::new(false));
        let hearing = heard.clone();
        let _ = sender.send(ScanMessage::Learn { options: scan });
        *self.running.borrow_mut() = Some(Running::Child { id, input: sender, done, kill, heard, pid });
        tokio::task::spawn_local(async move {
            let writer = tokio::task::spawn_local(async move {
                while let Some(message) = messages.recv().await {
                    let line = format!("{}\n", serde_json::to_string(&message).unwrap_or_default());
                    if input.write_all(line.as_bytes()).await.is_err() || input.flush().await.is_err() {
                        break;
                    }
                }
            });
            let (mut reading, mut ended) = (true, false);
            loop {
                tokio::select! {
                    line = lines.next_line(), if reading => match line {
                        Ok(Some(line)) => {
                            hearing.set(true);
                            if let (Ok(reply), Some(music)) = (serde_json::from_str::<ScanReply>(&line), weak.upgrade()) {
                                music.heard(reply);
                            }
                        }
                        _ => reading = false,
                    },
                    _ = child.wait() => break,
                    _ = killed.cancelled(), if !ended => {
                        ended = true;
                        let _ = child.start_kill();
                    }
                }
            }
            writer.abort();
            let _ = done_sender.send(true);
            if let Some(music) = weak.upgrade() {
                music.finished(id);
            }
        });
    }

    /// A scan ended: the next looks for new songs after a while, or at once when the folders changed meanwhile.
    fn finished(&self, id: u64) {
        if self.running.borrow().as_ref().is_none_or(|running| running.id() != id) {
            return;
        }
        self.running.borrow_mut().take();
        self.read_counts();
        let options = self.options.borrow().clone();
        if self.closed.get() || options.walked().is_empty() {
            return;
        }
        let paused = self.paused();
        self.update(|status| {
            if matches!(status.state, MusicState::Learning | MusicState::Paused) {
                status.state = if paused { MusicState::Paused } else { MusicState::Done };
            }
        });
        self.next_scan(if self.again.replace(false) { 0 } else { options.every_ms });
    }

    /// The next scan, `delay` ms from now.
    fn next_scan(&self, delay: u64) {
        let weak = self.weak.clone();
        cancel(&self.timer);
        *self.timer.borrow_mut() = Some(tokio::task::spawn_local(async move {
            tokio::time::sleep(Duration::from_millis(delay)).await;
            if let Some(music) = weak.upgrade() {
                music.timer.borrow_mut().take();
                music.start_now();
            }
        }));
    }

    /// Learn again now (after the folders changed, or as the producer asks): the scan under way ends first.
    pub fn learn_now(self: &Rc<Self>) {
        cancel(&self.timer);
        if self.running.borrow().is_some() {
            self.again.set(true);
            self.send(ScanMessage::Stop);
        } else {
            self.start_now();
        }
    }

    /// Stop the scan (its child within a moment and a half, else ended), and start none again.
    pub async fn close(&self) {
        self.closed.set(true);
        cancel(&self.timer);
        self.listeners.borrow_mut().clear();
        let child = match self.running.borrow().as_ref() {
            Some(Running::Child { input, done, kill, heard, pid, .. }) => {
                Some((input.clone(), done.clone(), kill.clone(), heard.get(), *pid))
            }
            Some(Running::Here { control, .. }) => {
                control.stop();
                None
            }
            None => None,
        };
        if let Some((input, mut done, kill, heard, pid)) = child {
            // A scan that hasn't said a word is still starting (the program, its models and programs, its lock), and reads
            // no stop till then: nothing of the music's is its own yet, so it's ended at once rather than waited for.
            if heard {
                let _ = input.send(ScanMessage::Stop);
            } else {
                kill.cancel();
            }
            let wait = async {
                while !*done.borrow() {
                    if done.changed().await.is_err() {
                        break;
                    }
                }
            };
            let gone = tokio::time::timeout(Duration::from_millis(1500), wait).await.is_ok();
            if !gone {
                kill.cancel();
            }
            // Ended rather than stopped, it can't let go of a lock it took: that's done for it, so a quit leaves nothing
            // behind.
            if let (false, Some(pid)) = (heard && gone, pid) {
                let _ = release_ended(&self.dir().to_string_lossy(), pid).await;
            }
        }
    }
}
