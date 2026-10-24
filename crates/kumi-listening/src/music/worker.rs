//! The scan as a process of its own: a job kind of Kumi's library learner (`kumi-library-learner --music`), at its
//! lowest priority, pausing whenever the library learner does (while Kumi listens or renders) and while the computer
//! runs on its battery. It's told what to learn, and when to pause, over JSON lines on stdin, and says how it's going
//! on stdout.

use super::{
    database,
    hear::{hear, restyle, Clap, Ears, Heard, Style},
    map,
    scan::{clustered, next as next_song, recluster, spread, tags_pass, Next, TagsPass, STYLE_WORDS},
    store::{MusicError, MusicStore, LIVE},
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
    pub wake: Rc<Notify>,
    pub stop: Signal,
    pub battery: bool,
}

impl Control {
    pub fn new(battery: bool) -> Control {
        Control { paused: Rc::new(Cell::new(false)), wake: Rc::new(Notify::new()), stop: Signal::new(), battery }
    }
    pub fn pause(&self, paused: bool) {
        self.paused.set(paused);
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

/// Wait while the scan is paused, or the computer is on its battery; at once when it's stopped.
async fn gate(control: &Control, battery: &mut (Option<Instant>, bool)) {
    loop {
        if control.stop.is_cancelled() {
            return;
        }
        let woken = control.wake.notified();
        if control.paused.get() {
            tokio::select! { _ = woken => {}, _ = control.stop.cancelled() => return }
            continue;
        }
        if control.battery {
            if battery.0.is_none_or(|at| at.elapsed() >= BATTERY_EVERY) {
                *battery = (Some(Instant::now()), on_battery());
            }
            if battery.1 {
                tokio::select! { _ = woken => {}, _ = control.stop.cancelled() => return, _ = tokio::time::sleep(BATTERY_EVERY) => {} }
                continue;
            }
        }
        return;
    }
}

/// The library's places and the producer's sample folders: the walk leaves them out.
fn skipped_folders(daws: Daws, library_folders: &[String]) -> Vec<String> {
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

/// The heard songs this style model hasn't heard, those taste used first, then the newest.
fn missing(store: &MusicStore, reader: &VectorReader) -> Result<VecDeque<i64>, MusicError> {
    let mut statement = store
        .connection()
        .prepare(&format!("SELECT id FROM songs WHERE {LIVE} AND stage >= 1 ORDER BY used_at IS NULL, used_at DESC, mtime DESC, id"))?;
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
    let (mut store, _) = blocking(store, move |store| {
        let pass = TagsPass { folders: &folders, skip: &skip, signal: &stop, readers: 4, progress: &|_| {} };
        tags_pass(store, &pass)
    })
    .await?;
    let counts = |store: &MusicStore, progress: &mut ScanProgress| -> Result<(), MusicError> {
        let counts = store.counts()?;
        (progress.songs, progress.light, progress.full, progress.skipped) = (counts.songs, counts.light, counts.full, counts.skipped);
        Ok(())
    };
    progress.phase = ScanPhase::Hearing;
    counts(&store, &mut progress)?;
    report(ScanReply::Progress { progress: progress.clone() });

    // The style model and its vectors (none when the embeddings slot is off).
    let style: Option<Rc<dyn Style>> = match style {
        Some(style) => Some(style),
        None => Clap::from_slot(&control.stop).await.map(|clap| Rc::new(clap) as Rc<dyn Style>),
    };
    let model = style.as_ref().map(|style| style.model().to_string()).unwrap_or_default();
    let mut writer = VectorWriter::open(&dir, &model)?;
    let mut missing = match style.is_some() {
        true => missing(&store, &VectorReader::open(&dir, &store, &model)?)?,
        false => VecDeque::new(),
    };
    let mut styled = store.heard_ids()?.len().saturating_sub(missing.len());
    let mut clusters = clustered(&store, &model)?;
    let mut mapped = map::mapped(&store, &model)?;
    let spread = spread(&store)?;
    let mut spread_at = 0;
    let say = |text: &str| report(ScanReply::Said { text: text.to_string() });
    // What the style words mean to the style model, to name its styles by (none: it has no words).
    let named: Option<std::sync::Arc<Vec<(String, Vec<f32>)>>> = match &style {
        Some(style) => style_words(&store, style.as_ref(), &say, &control.stop).await?.map(std::sync::Arc::new),
        None => None,
    };
    let ears = Ears { style: style.as_deref(), say: &say, signal: &control.stop };

    let mut busy: HashSet<i64> = HashSet::new();
    let mut failed: HashSet<i64> = HashSet::new();
    let mut running = FuturesUnordered::new();
    let mut told = Instant::now();
    loop {
        gate(control, &mut battery).await;
        if control.stop.is_cancelled() {
            break;
        }
        while running.len() < AT_ONCE {
            let mut skipping: HashSet<i64> = busy.union(&failed).copied().collect();
            skipping.extend(missing.iter().filter(|id| failed.contains(id)));
            let Some(next) = next_song(&store, &skipping, &mut missing, &spread, &mut spread_at)? else { break };
            busy.insert(next.song().id);
            let ears = &ears;
            running.push(async move {
                let heard = match &next {
                    Next::Hear(song, stage) => Ok(hear(song, *stage, ears).await),
                    Next::Restyle(song, stage) => Err(restyle(song, *stage, ears).await),
                };
                (next, heard)
            });
        }
        let Some((next, heard)) = running.next().await else { break };
        let id = next.song().id;
        busy.remove(&id);
        match heard {
            Ok(Heard::Done { profile, vector, unstyled, .. }) => {
                let stage = match next {
                    Next::Hear(_, stage) => stage,
                    Next::Restyle(_, stage) => stage,
                };
                let profile = serde_json::to_string(&profile).map_err(|error| MusicError(error.to_string()))?;
                store.heard(id, stage, &profile, now_ms())?;
                match vector.filter(|(_, by)| *by == model) {
                    Some((vector, _)) => {
                        writer.put(&store, id, &vector)?;
                        styled += usize::from(next.song().stage == 0);
                    }
                    None => {
                        failed.insert(id);
                        progress.unstyled = progress.unstyled.or(unstyled);
                    }
                }
                progress.heard += 1;
            }
            Ok(Heard::Skipped(why)) => {
                store.skip(id, &why, now_ms())?;
                progress.heard += 1;
            }
            Ok(Heard::Stop(why)) => {
                if control.stop.is_cancelled() {
                    break;
                }
                return Err(MusicError(why));
            }
            Err(Ok((vector, by))) if by == model => {
                writer.put(&store, id, &vector)?;
                styled += 1;
            }
            Err(Ok(_)) => {
                failed.insert(id);
            }
            Err(Err(why)) => {
                if control.stop.is_cancelled() {
                    break;
                }
                failed.insert(id);
                progress.unstyled = progress.unstyled.or(Some(why));
            }
        }
        // The clusters again once the songs the style model heard grow by a tenth (ten at least).
        if styled >= clusters + (clusters / 10).max(10) {
            let (back, made) = blocking(store, {
                let (dir, model) = (dir.clone(), model.clone());
                let named = named.clone();
                move |store| recluster(store, &dir, &model, named.as_deref().map(Vec::as_slice))
            })
            .await?;
            (store, clusters) = (back, made);
        }
        // The map again too, by the same rule.
        if style.is_some() && map::due(mapped, styled, false) {
            let (back, laid) = blocking(store, {
                let (dir, model) = (dir.clone(), model.clone());
                move |store| map::lay_out(store, &dir, &model, &Layout::default())
            })
            .await?;
            (store, mapped) = (back, Some(laid));
        }
        if told.elapsed() >= Duration::from_secs(1) {
            told = Instant::now();
            counts(&store, &mut progress)?;
            report(ScanReply::Progress { progress: progress.clone() });
        }
    }
    // Stopped: the songs under way are left for next time.
    drop(running);
    if !control.stop.is_cancelled() {
        writer.settle(&store)?;
        if style.is_some() && styled != clusters {
            let (back, _) = blocking(store, {
                let (dir, model) = (dir.clone(), model.clone());
                move |store| recluster(store, &dir, &model, named.as_deref().map(Vec::as_slice))
            })
            .await?;
            store = back;
        }
        if style.is_some() && map::due(mapped, styled, true) {
            let (back, _) = blocking(store, move |store| map::lay_out(store, &dir, &model, &Layout::default())).await?;
            store = back;
        }
        progress.phase = ScanPhase::Done;
        progress.finished_at = Some(now_ms());
        store.set_meta("learned_at", &now_ms().to_string())?;
    }
    counts(&store, &mut progress)?;
    Ok(progress)
}

/// The worker process: one run, as it's told, then it ends (Kumi starts it again later to look for new songs).
pub async fn main(daws: Daws) -> io::Result<()> {
    kumi_library::learner::low_priority();
    let control = Control::new(true);
    let (jobs, mut incoming) = mpsc::unbounded_channel();
    let (output, mut outgoing) = mpsc::unbounded_channel::<ScanReply>();
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
                match run(&options, daws, &control, None, &report).await {
                    Ok(progress) if progress.phase == ScanPhase::Done => report(ScanReply::Done { progress }),
                    Ok(progress) => report(ScanReply::Progress { progress }),
                    Err(error) if !control.stop.is_cancelled() => report(ScanReply::Failed { message: head(&error.to_string(), 300) }),
                    Err(_) => {}
                }
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
