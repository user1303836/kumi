//! The producer's music as the app holds it: whether Kumi learns it (their music folders), how the scan is going (a
//! `kumi-library-learner --music` process, started again every half hour to find new songs), and pausing it while
//! Kumi listens or renders, or when the producer asks.

use super::{
    database,
    hear::Style,
    store::MusicStore,
    worker::{run, Control, ScanMessage, ScanOptions, ScanPhase, ScanProgress, ScanReply},
};
use indexmap::IndexMap;
use kumi_common::{abort::Signal, time::now_ms};
use kumi_library::{daws::Daws, measure_worker::worker_binary};
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

#[derive(Clone)]
pub struct MusicOptions {
    /// Kumi's music folder.
    pub dir: PathBuf,
    /// The producer's music folders (`musicFolders`): none, and Kumi doesn't learn their music.
    pub folders: Vec<String>,
    /// Their sample folders (`libraryFolders`), which the walk leaves out with the library's places.
    pub library_folders: Vec<String>,
    /// false: the scan runs in this process (tests), with `style` as its style model when there's one.
    pub fork: bool,
    pub style: Option<Rc<dyn Style>>,
    /// How long after Kumi starts the first scan begins, and how long between scans.
    pub delay_ms: u64,
    pub every_ms: u64,
    /// What the DAWs the app registers tell the library (their places, which the walk leaves out).
    pub daws: Daws,
}

impl MusicOptions {
    pub fn new(dir: PathBuf, folders: Vec<String>, daws: Daws) -> MusicOptions {
        MusicOptions { dir, folders, library_folders: vec![], fork: true, style: None, delay_ms: 4_000, every_ms: 30 * 60_000, daws }
    }
}

/// The scan under way: a child process told over its stdin, or one in this process.
enum Running {
    Child { id: u64, input: mpsc::UnboundedSender<ScanMessage>, done: watch::Receiver<bool>, kill: Signal },
    Here { id: u64, control: Control },
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
    timer: RefCell<Option<JoinHandle<()>>>,
}

fn cancel(timer: &RefCell<Option<JoinHandle<()>>>) {
    if let Some(timer) = timer.borrow_mut().take() {
        timer.abort();
    }
}

/// Create inside the runtime's LocalSet: its scan reports through local tasks.
pub fn create_music(options: MusicOptions) -> Rc<Music> {
    let state = if options.folders.is_empty() { MusicState::Off } else { MusicState::Done };
    let status =
        MusicStatus { state, folders: options.folders.clone(), songs: 0, light: 0, full: 0, skipped: 0, learned_at: None, said: None };
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
        timer: RefCell::new(None),
    });
    music.read_counts();
    music
}

impl Music {
    pub fn dir(&self) -> PathBuf {
        self.options.borrow().dir.clone()
    }

    pub fn folders(&self) -> Vec<String> {
        self.options.borrow().folders.clone()
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
        let (weak, file) = (self.weak.clone(), database(&self.dir()));
        tokio::task::spawn_local(async move {
            let read = tokio::task::spawn_blocking(move || {
                let store = MusicStore::open_read(&file).ok().flatten()?;
                let learned_at = store.meta("learned_at").ok().flatten().and_then(|at| at.parse::<i64>().ok());
                Some((store.counts().ok()?, learned_at))
            })
            .await
            .ok()
            .flatten();
            if let (Some(music), Some((counts, learned_at))) = (weak.upgrade(), read) {
                music.update(|status| {
                    (status.songs, status.light, status.full, status.skipped) = (counts.songs, counts.light, counts.full, counts.skipped);
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

    /// Learn from these folders from now on: the scan under way stops, and one over the new folders starts (none: Kumi
    /// stops learning).
    pub fn set_folders(self: &Rc<Self>, folders: Vec<String>) {
        self.options.borrow_mut().folders = folders.clone();
        self.update(|status| status.folders = folders.clone());
        if folders.is_empty() {
            cancel(&self.timer);
            self.send(ScanMessage::Stop);
            self.update(|status| status.state = MusicState::Off);
        } else {
            self.learn_now();
        }
    }

    /// Start learning a moment after Kumi starts (nothing, without music folders).
    pub fn start(self: &Rc<Self>) {
        if self.closed.get() || self.options.borrow().folders.is_empty() {
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
        let progress_of = |status: &mut MusicStatus, progress: &ScanProgress| {
            (status.songs, status.light, status.full, status.skipped) = (progress.songs, progress.light, progress.full, progress.skipped);
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
        if self.closed.get() || options.folders.is_empty() || self.running.borrow().is_some() {
            return;
        }
        cancel(&self.timer);
        let id = self.next.get();
        self.next.set(id + 1);
        let paused = self.paused();
        self.update(|status| status.state = if paused { MusicState::Paused } else { MusicState::Learning });
        let scan = ScanOptions {
            dir: options.dir.to_string_lossy().into_owned(),
            folders: options.folders.clone(),
            library_folders: options.library_folders.clone(),
            paused,
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
        let spawned = Command::new(worker_binary("kumi-library-learner", "KUMI_LIBRARY_LEARNER_BIN"))
            .arg("--music")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn();
        let Ok(mut child) = spawned else {
            self.update(|status| status.said = Some("Kumi's learner couldn't start; it tries again later.".into()));
            self.finished(id);
            return;
        };
        let mut input = child.stdin.take().unwrap();
        let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
        let (sender, mut messages) = mpsc::unbounded_channel::<ScanMessage>();
        let (done_sender, done) = watch::channel(false);
        let kill = Signal::new();
        let killed = kill.clone();
        let _ = sender.send(ScanMessage::Learn { options: scan });
        *self.running.borrow_mut() = Some(Running::Child { id, input: sender, done, kill });
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
        if self.closed.get() || options.folders.is_empty() {
            return;
        }
        let paused = self.paused();
        self.update(|status| {
            if matches!(status.state, MusicState::Learning | MusicState::Paused) {
                status.state = if paused { MusicState::Paused } else { MusicState::Done };
            }
        });
        let weak = self.weak.clone();
        let delay = if self.again.replace(false) { 0 } else { options.every_ms };
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
            Some(Running::Child { input, done, kill, .. }) => Some((input.clone(), done.clone(), kill.clone())),
            Some(Running::Here { control, .. }) => {
                control.stop();
                None
            }
            None => None,
        };
        if let Some((input, mut done, kill)) = child {
            let _ = input.send(ScanMessage::Stop);
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
