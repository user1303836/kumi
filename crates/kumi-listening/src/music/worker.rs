//! The scan as a process of its own: a job kind of Kumi's library learner (`kumi __library-learner --music`), at its
//! lowest priority, pausing whenever the library learner does (while Kumi listens or renders) and while the computer
//! runs on its battery. It's told what to learn, and when to pause, over JSON lines on stdin, and says how it's going
//! on stdout.

use super::{
    database,
    hear::{hear, restyle, Clap, Ears, Heard, Style},
    map,
    scan::{clustered, next as next_song, recluster, spread, tags_pass, Aside, Next, TagsPass, STYLE_WORDS},
    store::{Counts, MusicError, MusicStore, Stage, LIVE},
    vectors::{VectorReader, VectorWriter},
};
use futures::stream::{FuturesUnordered, StreamExt};
use kumi_common::{
    abort::Signal,
    js::{json::stringify, string::head},
    time::now_ms,
};
use kumi_layout::Layout;
use kumi_library::{
    daws::Daws,
    sources::{library_sources, SourceOptions},
    state::acquire_lock,
};
use serde::{Deserialize, Serialize};
use std::{
    cell::Cell,
    collections::{HashSet, VecDeque},
    io,
    path::PathBuf,
    rc::Rc,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    sync::{mpsc, Notify},
};

/// Songs heard at once: one decoding and measuring while the other is with the style model.
const AT_ONCE: usize = 2;
/// How often the battery is looked at.
const BATTERY_EVERY: Duration = Duration::from_secs(60);
/// How often the counts are read again from music.db while hearing (a song the app heard in full meanwhile, for a
/// reference): between, the scan counts what it hears itself.
const COUNTED_EVERY: Duration = Duration::from_secs(60);
/// How often the scan says again that it holds the music (a scan runs for days; a lock left a day counts as left).
const HELD_EVERY: Duration = Duration::from_secs(5 * 60);

/// What the scan is told to learn.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanOptions {
    /// Kumi's music folder: music.db and the vector files.
    pub dir: String,
    /// The producer's music folders.
    pub folders: Vec<String>,
    /// Their sample folders (libraryFolders): left out of the walk, with the library's own places.
    #[serde(default)]
    pub library_folders: Vec<String>,
    #[serde(default)]
    pub paused: bool,
    /// Learn while the computer runs on its battery too (tests, which a laptop on its battery would hold).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub on_battery: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ScanMessage {
    Learn { options: ScanOptions },
    Pause,
    Resume,
    Stop,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScanPhase {
    /// The walk and the tags pass.
    #[default]
    Looking,
    Hearing,
    Done,
}

/// How the scan is going.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanProgress {
    pub phase: ScanPhase,
    pub songs: usize,
    pub light: usize,
    pub full: usize,
    pub skipped: usize,
    /// Songs not heard yet that are only in the cloud: heard once they're on this computer.
    #[serde(default)]
    pub cloud: usize,
    /// Songs heard (or skipped) in this run.
    pub heard: usize,
    pub started_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<i64>,
    /// Why the style model couldn't hear, when it couldn't (the songs are heard without it, and again later).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unstyled: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ScanReply {
    Progress {
        progress: ScanProgress,
    },
    Done {
        progress: ScanProgress,
    },
    /// Another Kumi is learning the music now.
    Busy,
    Failed {
        message: String,
    },
    /// What a model fetch says ("Kumi is fetching its style model…").
    Said {
        text: String,
    },
}

/// How the scan is held and stopped: paused (it waits), woken when that changes, stopped for good; and whether it
/// waits while the computer runs on its battery.
#[derive(Clone)]
pub struct Control {
    pub paused: Rc<Cell<bool>>,
    /// The same, for the tags pass's threads.
    pub held: Arc<AtomicBool>,
    pub wake: Rc<Notify>,
    pub stop: Signal,
    pub battery: bool,
}

impl Control {
    pub fn new(battery: bool) -> Control {
        Control {
            paused: Rc::new(Cell::new(false)),
            held: Arc::new(AtomicBool::new(false)),
            wake: Rc::new(Notify::new()),
            stop: Signal::new(),
            battery,
        }
    }
    pub fn pause(&self, paused: bool) {
        self.paused.set(paused);
        self.held.store(paused, Ordering::Relaxed);
        self.wake.notify_waiters();
    }
    pub fn stop(&self) {
        self.stop.cancel();
        self.wake.notify_waiters();
    }
}

/// Whether the computer runs on its battery, where that's cheap to know: macOS's pmset, Linux's power supplies,
/// Windows' power status. Not known: no.
pub fn on_battery() -> bool {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("pmset")
            .args(["-g", "ps"])
            .stderr(std::process::Stdio::null())
            .output()
            .is_ok_and(|out| String::from_utf8_lossy(&out.stdout).contains("'Battery Power'"))
    }
    #[cfg(target_os = "linux")]
    {
        let Ok(entries) = std::fs::read_dir("/sys/class/power_supply") else { return false };
        let read = |path: std::path::PathBuf| std::fs::read_to_string(path).unwrap_or_default().trim().to_string();
        let mains: Vec<String> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| read(path.join("type")) == "Mains")
            .map(|path| read(path.join("online")))
            .collect();
        !mains.is_empty() && mains.iter().all(|online| online == "0")
    }
    #[cfg(windows)]
    {
        #[repr(C)]
        #[derive(Default)]
        struct PowerStatus {
            ac_line: u8,
            battery_flag: u8,
            battery_percent: u8,
            saver: u8,
            battery_seconds: u32,
            battery_full_seconds: u32,
        }
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetSystemPowerStatus(status: *mut PowerStatus) -> i32;
        }
        let mut status = PowerStatus::default();
        unsafe { GetSystemPowerStatus(&mut status) != 0 && status.ac_line == 0 }
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    {
        false
    }
}

/// Whether the scan should wait now: it's paused, or the computer is on its battery (looked at once a minute).
fn waits(control: &Control, battery: &mut (Option<Instant>, bool)) -> bool {
    if control.stop.is_cancelled() {
        return false;
    }
    if control.paused.get() {
        return true;
    }
    if control.battery && battery.0.is_none_or(|at| at.elapsed() >= BATTERY_EVERY) {
        *battery = (Some(Instant::now()), on_battery());
    }
    control.battery && battery.1
}

/// Wait while the scan is paused, or the computer is on its battery; at once when it's stopped.
async fn gate(control: &Control, battery: &mut (Option<Instant>, bool)) {
    loop {
        let woken = control.wake.notified();
        if !waits(control, battery) {
            return;
        }
        match control.paused.get() {
            true => tokio::select! { _ = woken => {}, _ = control.stop.cancelled() => return },
            false => {
                tokio::select! { _ = woken => {}, _ = control.stop.cancelled() => return, _ = tokio::time::sleep(BATTERY_EVERY) => {} }
            }
        }
    }
}

/// The tags pass's own gate, for its threads: between one file and the next it waits while the scan is paused or the
/// computer is on its battery (looked at once a minute), and returns at once when it's stopped.
fn holder(control: &Control, battery: (Option<Instant>, bool)) -> impl Fn() + Send + Sync + 'static {
    let (held, stop, watch) = (control.held.clone(), control.stop.clone(), control.battery);
    let battery = Mutex::new((battery.0.unwrap_or_else(Instant::now), battery.1));
    move || loop {
        if stop.is_cancelled() {
            return;
        }
        let on = watch && {
            let mut battery = battery.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            if battery.0.elapsed() >= BATTERY_EVERY {
                *battery = (Instant::now(), on_battery());
            }
            battery.1
        };
        if !held.load(Ordering::Relaxed) && !on {
            return;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// What a layout asks as it works: it waits while the scan should (paused, on its battery), and goes on unless the scan
/// was stopped.
fn going(control: &Control, battery: (Option<Instant>, bool)) -> impl Fn() -> bool + Send + Sync + 'static {
    let (hold, stop) = (holder(control, battery), control.stop.clone());
    move || {
        hold();
        !stop.is_cancelled()
    }
}

/// The library's places and the producer's sample folders: the walk leaves them out (and the offer's count).
pub fn skipped_folders(daws: Daws, library_folders: &[String]) -> Vec<String> {
    let mut skip: Vec<String> = library_folders.to_vec();
    if !daws.0.is_empty() {
        let options = SourceOptions { folders: Some(library_folders.to_vec()), ..Default::default() };
        skip.extend(library_sources(daws, &options).into_iter().map(|source| source.path));
    }
    skip
}

/// What the style words mean to a style model: kept in music.db for it (and this list, and this words model), else
/// heard now (and kept). None when it has no words (its text side can't be had now, or the slot runs another model).
async fn style_words(
    store: &MusicStore,
    style: &dyn Style,
    say: crate::embed::Say<'_>,
    signal: &kumi_common::abort::Signal,
) -> Result<Option<Vec<(String, Vec<f32>)>>, MusicError> {
    let list: Vec<&str> = STYLE_WORDS.lines().map(str::trim).filter(|word| !word.is_empty()).collect();
    let key = {
        use sha2::{Digest, Sha256};
        let words_model = kumi_ai::models::pinned::CLAP_TEXT.sha256;
        hex::encode(Sha256::digest(format!("{words_model}\n{STYLE_WORDS}").as_bytes()))[..16].to_string()
    };
    let kept = store.meta("style_words")?.and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok());
    if let Some(kept) = kept.filter(|kept| kept["model"] == style.model() && kept["list"] == key.as_str()) {
        let vectors: Vec<Vec<f32>> = serde_json::from_value(kept["vectors"].clone()).unwrap_or_default();
        if vectors.len() == list.len() {
            return Ok(Some(list.iter().map(|word| word.to_string()).zip(vectors).collect()));
        }
    }
    let mut named = Vec::with_capacity(list.len());
    for word in &list {
        match style.words(word, say, signal).await {
            Ok(vector) => named.push((word.to_string(), vector)),
            Err(_) => return Ok(None),
        }
    }
    let vectors: Vec<&Vec<f32>> = named.iter().map(|(_, vector)| vector).collect();
    store.set_meta("style_words", &serde_json::json!({"model": style.model(), "list": key, "vectors": vectors}).to_string())?;
    Ok(Some(named))
}

/// The heard songs this style model hasn't heard, those taste used first, then the newest (none only in the cloud).
fn missing(store: &MusicStore, reader: &VectorReader) -> Result<VecDeque<i64>, MusicError> {
    let mut statement = store.connection().prepare(&format!(
        "SELECT id FROM songs WHERE {LIVE} AND stage >= 1 AND cloud = 0 ORDER BY used_at IS NULL, used_at DESC, mtime DESC, id"
    ))?;
    let ids: Vec<i64> = statement.query_map([], |row| row.get(0))?.collect::<Result<_, _>>()?;
    Ok(ids.into_iter().filter(|id| !reader.has(*id)).collect())
}

/// Off this thread with the store: the tags pass and the clusters take seconds on a big library.
async fn blocking<T: Send + 'static>(
    store: MusicStore,
    job: impl FnOnce(&mut MusicStore) -> Result<T, MusicError> + Send + 'static,
) -> Result<(MusicStore, T), MusicError> {
    let (store, result) = tokio::task::spawn_blocking(move || {
        let mut store = store;
        let result = job(&mut store);
        (store, result)
    })
    .await
    .map_err(|error| MusicError(error.to_string()))?;
    Ok((store, result?))
}

/// A song heard, or not.
type Took = (Next, Result<Heard, Result<(Vec<f32>, String), String>>);

/// What the hearing loop keeps as it goes.
struct Hearing {
    /// Songs under way.
    busy: HashSet<i64>,
    /// Songs that can't be heard this run (their files weren't there).
    later: HashSet<i64>,
    /// The producer's folders not there now: their songs wait.
    away: Vec<String>,
    /// Songs the style model heard (for the clusters and the map).
    styled: usize,
    progress: ScanProgress,
}

/// How hearing a song left the loop.
enum Flow {
    Go,
    /// Stopped, or nothing can be heard now (the reason, when it's not a stop).
    End(Option<String>),
}

/// A song heard, kept in music.db (and its vector in its file), and counted as it changed the counts.
fn took(
    hearing: &mut Hearing,
    store: &MusicStore,
    writer: &mut VectorWriter,
    model: &str,
    stop: &Signal,
    took: Took,
) -> Result<Flow, MusicError> {
    let (next, heard) = took;
    let song = next.song();
    let was = (song.stage == Stage::Light as i64, song.stage == Stage::Full as i64);
    hearing.busy.remove(&song.id);
    let progress = &mut hearing.progress;
    let unheard = |progress: &mut ScanProgress| {
        progress.light -= usize::from(was.0).min(progress.light);
        progress.full -= usize::from(was.1).min(progress.full);
    };
    match heard {
        Ok(Heard::Done { profile, vector, unstyled, .. }) => {
            let stage = match next {
                Next::Hear(_, stage) => stage,
                Next::Restyle(_, stage) => stage,
            };
            let profile = serde_json::to_string(&profile).map_err(|error| MusicError(error.to_string()))?;
            store.heard(song.id, stage, &profile, now_ms())?;
            unheard(progress);
            match stage {
                Stage::Full => progress.full += 1,
                _ => progress.light += 1,
            }
            match vector.filter(|(_, by)| by == model) {
                Some((vector, _)) => {
                    writer.put(store, song.id, &vector)?;
                    hearing.styled += usize::from(song.stage == 0);
                }
                None => progress.unstyled = progress.unstyled.take().or(unstyled),
            }
            progress.heard += 1;
        }
        Ok(Heard::Skipped(why)) => {
            store.skip(song.id, &why, now_ms())?;
            unheard(progress);
            progress.skipped += 1;
            progress.heard += 1;
        }
        Ok(Heard::Later { away, .. }) => {
            hearing.later.insert(song.id);
            if away && !hearing.away.contains(&song.folder) {
                hearing.away.push(song.folder.clone());
            }
        }
        Ok(Heard::Stop(why)) => return Ok(Flow::End((!stop.is_cancelled()).then_some(why))),
        Err(Ok((vector, by))) if by == model => {
            writer.put(store, song.id, &vector)?;
            hearing.styled += 1;
        }
        // Heard by another model meanwhile, or not at all: a later run hears it again.
        Err(Ok(_)) => {}
        Err(Err(why)) => {
            if stop.is_cancelled() {
                return Ok(Flow::End(None));
            }
            progress.unstyled = progress.unstyled.take().or(Some(why));
        }
    }
    Ok(Flow::Go)
}

fn counted(progress: &mut ScanProgress, counts: Counts) {
    (progress.songs, progress.light, progress.full, progress.skipped, progress.cloud) =
        (counts.songs, counts.light, counts.full, counts.skipped, counts.cloud);
}

/// One run of the scan: the tags pass, then hearing until there's nothing left to hear (or it's stopped). `style` is the
/// style model to hear with: None, CLAP as the embeddings slot runs it (or none, when the slot is off).
pub async fn run(
    options: &ScanOptions,
    daws: Daws,
    control: &Control,
    style: Option<Rc<dyn Style>>,
    report: &dyn Fn(ScanReply),
) -> Result<ScanProgress, MusicError> {
    let dir = PathBuf::from(&options.dir);
    let mut progress = ScanProgress { started_at: now_ms(), ..Default::default() };
    report(ScanReply::Progress { progress: progress.clone() });
    let mut battery = (None, false);
    gate(control, &mut battery).await;
    if control.stop.is_cancelled() {
        return Ok(progress);
    }
    let store = MusicStore::open(&database(&dir))?;
    let (folders, skip, stop) = (options.folders.clone(), skipped_folders(daws, &options.library_folders), control.stop.clone());
    let hold = holder(control, battery);
    let (mut store, (_, away)) = blocking(store, move |store| {
        let pass = TagsPass { folders: &folders, skip: &skip, signal: &stop, readers: 4, progress: &|_| {}, hold: &hold };
        tags_pass(store, &pass)
    })
    .await?;
    let say = |text: &str| report(ScanReply::Said { text: text.to_string() });
    for folder in &away {
        say(&format!("Kumi can't read {folder} now: its songs wait for it."));
    }
    progress.phase = ScanPhase::Hearing;
    counted(&mut progress, store.counts()?);
    report(ScanReply::Progress { progress: progress.clone() });

    // The style model and its vectors (none when the embeddings slot is off).
    let style: Option<Rc<dyn Style>> = match style {
        Some(style) => Some(style),
        None => Clap::from_slot(&control.stop).await.map(|clap| Rc::new(clap) as Rc<dyn Style>),
    };
    let model = style.as_ref().map(|style| style.model().to_string()).unwrap_or_default();
    let mut writer = VectorWriter::open(&dir, &model)?;
    // Vectors a scan that ended early left waiting beside the file: into it.
    writer.settle_left(&store)?;
    let mut missing = match style.is_some() {
        true => missing(&store, &VectorReader::open(&dir, &store, &model)?)?,
        false => VecDeque::new(),
    };
    let mut clusters = clustered(&store, &model)?;
    let mut mapped = map::mapped(&store, &model)?;
    let spread = spread(&store)?;
    let mut spread_at = 0;
    // What the style words mean to the style model, to name its styles by (none: it has no words).
    let named: Option<std::sync::Arc<Vec<(String, Vec<f32>)>>> = match &style {
        Some(style) => style_words(&store, style.as_ref(), &say, &control.stop).await?.map(std::sync::Arc::new),
        None => None,
    };
    let ears = Ears { style: style.as_deref(), say: &say, signal: &control.stop };
    let styled = store.heard_ids()?.len().saturating_sub(missing.len());
    let mut hearing = Hearing { busy: HashSet::new(), later: HashSet::new(), away, styled, progress };

    let mut running = FuturesUnordered::new();
    let (mut told, mut counted_at) = (Instant::now(), Instant::now());
    let mut ended: Option<String> = None;
    // The folders found away so far, said once each.
    let mut said_away = hearing.away.len();
    let mut say_away = |hearing: &Hearing| {
        for folder in &hearing.away[said_away..] {
            say(&format!("Kumi can't read {folder} now: its songs wait for it."));
        }
        said_away = hearing.away.len();
    };
    'hearing: loop {
        if control.stop.is_cancelled() {
            break;
        }
        // The songs under way finish before the scan waits (paused, on its battery): none sits unheard past its
        // converter's deadline, to be kept as one that won't decode.
        if waits(control, &mut battery) {
            while let Some(done) = running.next().await {
                if let Flow::End(why) = took(&mut hearing, &store, &mut writer, &model, &control.stop, done)? {
                    ended = why;
                    break 'hearing;
                }
            }
            say_away(&hearing);
            gate(control, &mut battery).await;
            continue;
        }
        while running.len() < AT_ONCE {
            let aside: HashSet<i64> = hearing.busy.union(&hearing.later).copied().collect();
            let around = Aside { songs: &aside, folders: &hearing.away, since: hearing.progress.started_at };
            let Some(next) = next_song(&store, &around, &mut missing, &spread, &mut spread_at)? else { break };
            hearing.busy.insert(next.song().id);
            let ears = &ears;
            running.push(async move {
                let heard = match &next {
                    Next::Hear(song, stage) => Ok(hear(song, *stage, ears).await),
                    Next::Restyle(song, stage) => Err(restyle(song, *stage, ears).await),
                };
                (next, heard)
            });
        }
        let Some(done) = running.next().await else { break };
        if let Flow::End(why) = took(&mut hearing, &store, &mut writer, &model, &control.stop, done)? {
            ended = why;
            break;
        }
        // The clusters again once the songs the style model heard grow by a tenth (ten at least), and the map by the
        // same rule; the songs under way finish first.
        let regroup = style.is_some() && map::due(Some(clusters), hearing.styled, false);
        let remap = style.is_some() && map::due(mapped, hearing.styled, false);
        if regroup || remap {
            while let Some(done) = running.next().await {
                if let Flow::End(why) = took(&mut hearing, &store, &mut writer, &model, &control.stop, done)? {
                    ended = why;
                    break 'hearing;
                }
            }
        }
        say_away(&hearing);
        if regroup {
            let (back, made) = blocking(store, {
                let (dir, model) = (dir.clone(), model.clone());
                let named = named.clone();
                move |store| recluster(store, &dir, &model, named.as_deref().map(Vec::as_slice))
            })
            .await?;
            (store, clusters) = (back, made);
        }
        if remap {
            // It waits while Kumi listens, as hearing does; a stop ends it, and the loop with it.
            let (back, laid) = blocking(store, {
                let (dir, model, go_on) = (dir.clone(), model.clone(), going(control, battery));
                move |store| map::lay_out_until(store, &dir, &model, &Layout::default(), &go_on)
            })
            .await?;
            store = back;
            mapped = laid.or(mapped);
        }
        if counted_at.elapsed() >= COUNTED_EVERY {
            counted_at = Instant::now();
            counted(&mut hearing.progress, store.counts()?);
        }
        if told.elapsed() >= Duration::from_secs(1) {
            told = Instant::now();
            report(ScanReply::Progress { progress: hearing.progress.clone() });
        }
    }
    // Stopped: the songs under way are left for next time.
    drop(running);
    let mut progress = hearing.progress;
    if let Some(why) = ended {
        return Err(MusicError(why));
    }
    if !control.stop.is_cancelled() {
        writer.settle(&store)?;
        // The clusters and the map at a scan's end when the songs on them changed at all: more or fewer, or others
        // (a folder moved, a file changed).
        if style.is_some() {
            let changed = {
                let reader = VectorReader::open(&dir, &store, &model)?;
                map::changed(&store, &reader)?
            };
            if changed.clusters || hearing.styled != clusters {
                let (back, _) = blocking(store, {
                    let (dir, model) = (dir.clone(), model.clone());
                    move |store| recluster(store, &dir, &model, named.as_deref().map(Vec::as_slice))
                })
                .await?;
                store = back;
            }
            if changed.places || map::due(mapped, hearing.styled, true) {
                let go_on = going(control, battery);
                let (back, _) = blocking(store, move |store| map::lay_out_until(store, &dir, &model, &Layout::default(), &go_on)).await?;
                store = back;
            }
        }
    }
    // Stopped while the map was made: what was heard stays, and the next run makes it.
    if !control.stop.is_cancelled() {
        progress.phase = ScanPhase::Done;
        progress.finished_at = Some(now_ms());
        store.set_meta("learned_at", &now_ms().to_string())?;
    }
    counted(&mut progress, store.counts()?);
    Ok(progress)
}

/// The worker process: one run, as it's told, then it ends (Kumi starts it again later to look for new songs). `tools`
/// is where Kumi keeps the programs it fetches (its own ffmpeg, read or fetched there as the app does).
pub async fn main(daws: Daws, tools: Option<String>) -> io::Result<()> {
    kumi_library::learner::low_priority();
    // Its models (CLAP for every song) run as the background work it is: their threads don't spin between steps.
    kumi_ai::models::in_background();
    let control = Control::new(true);
    let (jobs, mut incoming) = mpsc::unbounded_channel();
    let (output, mut outgoing) = mpsc::unbounded_channel::<ScanReply>();
    // A program fetched to hear the songs (ffmpeg on Windows and Linux) is said as a model's fetch is. Weakly: the
    // replies end, and the process with them, once the run's done.
    let fetching = output.downgrade();
    kumi_media::programs::configure_programs(kumi_media::programs::ProgramDefaults {
        tools_dir: tools,
        on_fetch: Some(Arc::new(move |message: &str| {
            if let Some(output) = fetching.upgrade() {
                let _ = output.send(ScanReply::Said { text: message.to_string() });
            }
        })),
    });
    let write = tokio::task::spawn_local(async move {
        let mut stdout = tokio::io::stdout();
        while let Some(reply) = outgoing.recv().await {
            let line = serde_json::to_value(&reply).map(|value| stringify(&value)).unwrap_or_default();
            stdout.write_all(format!("{line}\n").as_bytes()).await?;
            stdout.flush().await?;
        }
        Ok::<_, io::Error>(())
    });
    let listening = control.clone();
    let input = tokio::task::spawn_local(async move {
        let mut lines = BufReader::new(tokio::io::stdin()).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            match serde_json::from_str::<ScanMessage>(&line) {
                Ok(ScanMessage::Pause) => listening.pause(true),
                Ok(ScanMessage::Resume) => listening.pause(false),
                Ok(ScanMessage::Stop) => listening.stop(),
                Ok(ScanMessage::Learn { options }) => {
                    listening.pause(options.paused);
                    let _ = jobs.send(options);
                }
                Err(_) => {}
            }
        }
        // Kumi has gone: stop, and don't wait long for a decoder that won't.
        listening.stop();
        tokio::time::sleep(Duration::from_secs(2)).await;
        std::process::exit(0);
    });
    if let Some(options) = incoming.recv().await {
        match acquire_lock(&options.dir).await? {
            Some(lock) => {
                let report = |reply: ScanReply| {
                    let _ = output.send(reply);
                };
                // Held for as long as the scan runs: another Kumi takes over only a lock that's stopped saying so.
                let holding = {
                    let lock = lock.holding();
                    tokio::task::spawn_local(async move {
                        loop {
                            tokio::time::sleep(HELD_EVERY).await;
                            let _ = lock.refresh().await;
                        }
                    })
                };
                let control = Control { battery: !options.on_battery, ..control.clone() };
                match run(&options, daws, &control, None, &report).await {
                    Ok(progress) if progress.phase == ScanPhase::Done => report(ScanReply::Done { progress }),
                    Ok(progress) => report(ScanReply::Progress { progress }),
                    Err(error) if !control.stop.is_cancelled() => report(ScanReply::Failed { message: head(&error.to_string(), 300) }),
                    Err(_) => {}
                }
                holding.abort();
                lock.release().await?;
            }
            None => {
                let _ = output.send(ScanReply::Busy);
            }
        }
    }
    drop(output);
    input.abort();
    write.await.map_err(io::Error::other)??;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_layout_waits_while_the_scan_is_paused_and_ends_when_its_stopped() {
        let control = Control::new(false);
        let go_on = going(&control, (None, false));
        assert!(go_on(), "neither paused nor stopped: it goes on");
        control.pause(true);
        let done = AtomicBool::new(false);
        std::thread::scope(|scope| {
            let waiting = scope.spawn(|| {
                let went = go_on();
                done.store(true, Ordering::Relaxed);
                went
            });
            std::thread::sleep(Duration::from_millis(400));
            assert!(!done.load(Ordering::Relaxed), "paused: it waits");
            control.stop();
            assert!(!waiting.join().unwrap(), "stopped: it ends");
        });
    }
}
