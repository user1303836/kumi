//! Learn changed sounds, presets and Sets, persisting progress as each batch finishes.
use super::{
    classify::{ClassFrom, SoundClass, SoundKind},
    presets::PresetCategory,
    sets::SetSummary,
    sources::Source,
    store::{Entry, Log, LogEntry},
};
use serde::{Deserialize, Serialize};
use std::path::Path;
pub const LOG_VERSION: u32 = 1;
macro_rules! sound_entry{($($name:ident:$type:ty),*$(,)?)=>{
    #[derive(Debug,Clone,Default,PartialEq,Serialize,Deserialize)]
    #[serde(rename_all="camelCase")]
    pub struct SoundEntry{#[serde(flatten)]pub file:Entry,$(#[serde(default,skip_serializing_if="Option::is_none")]pub $name:Option<$type>,)*}
};}
sound_entry! {seconds:f64,kind:SoundKind,r#class:SoundClass,class_from:ClassFrom,bpm:f64,key:String,note:String,loudness:f64,peak:f64,brightness:f64,flatness:f64,attack:f64,decay:f64,width:f64,onsets:f64,low:f64,high:f64,vector:String,features:u32,embedding:String,embedding_model:String,error:String}
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PresetEntry {
    #[serde(flatten)]
    pub file: Entry,
    pub name: String,
    pub format: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<PresetCategory>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inside: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub about: Option<String>,
    pub source: String,
    pub folder: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browser: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SetEntry {
    #[serde(flatten)]
    pub file: Entry,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub set: Option<SetSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
macro_rules! entry {
    ($type:ty) => {
        impl LogEntry for $type {
            fn path(&self) -> &str {
                &self.file.path
            }
        }
        impl std::ops::Deref for $type {
            type Target = Entry;
            fn deref(&self) -> &Entry {
                &self.file
            }
        }
    };
}
entry!(SoundEntry);
entry!(PresetEntry);
entry!(SetEntry);
pub struct LibraryLogs {
    pub sounds: Log<SoundEntry>,
    pub presets: Log<PresetEntry>,
    pub sets: Log<SetEntry>,
}
pub fn library_logs(dir: &str) -> LibraryLogs {
    let dir = Path::new(dir);
    LibraryLogs {
        sounds: Log::new(dir.join("sounds.jsonl"), "sounds", LOG_VERSION),
        presets: Log::new(dir.join("presets.jsonl"), "presets", LOG_VERSION),
        sets: Log::new(dir.join("sets.jsonl"), "sets", LOG_VERSION),
    }
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counts {
    pub known: usize,
    pub todo: usize,
    pub done: usize,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LearnPhase {
    #[default]
    Looking,
    Presets,
    Sets,
    Sounds,
    Tidying,
    Done,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LearnProgress {
    pub phase: LearnPhase,
    pub sounds: Counts,
    pub presets: Counts,
    pub sets: Counts,
    pub failed: usize,
    pub started_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LearnPlan {
    pub dir: String,
    pub sources: Vec<Source>,
    #[serde(default)]
    pub set_folders: Vec<String>,
    #[serde(default)]
    pub set_files: Vec<String>,
    #[serde(default)]
    pub plugin_presets: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workers: Option<usize>,
}

/// Live's usual project folders and the parent folders of recently opened projects.
pub fn set_folders(recent: &[String], home: Option<&str>, platform: Option<&str>) -> Vec<String> {
    use super::sources::{current_platform, dirname, homedir, join};
    let default_home = home.is_none().then(homedir);
    let home = home.or(default_home.as_deref()).unwrap();
    let platform = platform.unwrap_or_else(|| current_platform());
    let mut folders = indexmap::IndexSet::new();
    if platform == "win32" {
        folders.insert(join(&join(home, "Documents"), "Ableton"));
    }
    folders.insert(join(home, "Music"));
    for file in recent {
        let project = dirname(file);
        let parent = dirname(&project);
        if parent != project && parent != home && parent.encode_utf16().count() > home.encode_utf16().count() {
            folders.insert(parent);
        }
    }
    folders.into_iter().collect()
}
pub fn plugin_preset_folders(home: Option<&str>, platform: Option<&str>) -> Vec<String> {
    use super::sources::{current_platform, homedir, join};
    let default_home = home.is_none().then(homedir);
    let home = home.or(default_home.as_deref()).unwrap();
    vec![match platform.unwrap_or_else(|| current_platform()) {
        "win32" => join(&join(home, "Documents"), "VST3 Presets"),
        "darwin" => join(&join(&join(home, "Library"), "Audio"), "Presets"),
        _ => join(&join(home, ".vst3"), "presets"),
    }]
}

use super::{
    classify::{classify, name_hints},
    features::{measure_sound, MeasureOptions, FEATURES_VERSION},
    measure_worker::{MeasureJob, MeasurePool},
    presets::{plugin_preset_facts, read_live_preset, read_max_device, PresetFacts, PRESET_EXTENSIONS},
    sets::read_set,
    sources::{basename, below, browser_path, dirname, join, SourceKind},
    store::{pack_vector, write_json},
    taste::build_taste,
};
use crate::{audio::decode::AudioError, core::errors::RuntimeError};
use futures::future::LocalBoxFuture;
use indexmap::IndexMap;
use kumi_common::{
    abort::{Signal, SignalExt},
    js::string::head,
    time::now_ms,
};
use regex::Regex;
use std::{
    cell::{Cell, RefCell},
    collections::HashSet,
    io,
    rc::Rc,
    sync::LazyLock,
    time::{SystemTime, UNIX_EPOCH},
};

pub fn extension(path: &str) -> String {
    Path::new(path).extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default()
}
pub async fn learn_sound(path: &str, relative_path: &str, size: u64, mtime: i64, part: MeasureOptions) -> Result<SoundEntry, AudioError> {
    let hints = name_hints(relative_path);
    let file = Entry { path: path.into(), size, mtime, gone: None };
    if ![".wav", ".wave", ".aif", ".aiff"].contains(&extension(path).to_lowercase().as_str()) && size > 60 * 1024 * 1024 {
        return Ok(SoundEntry {
            file,
            r#class: hints.class,
            class_from: hints.class_from,
            kind: hints.kind,
            error: Some("too long to measure".into()),
            ..Default::default()
        });
    }
    let heard = measure_sound(path, part).await?;
    let found = classify(&hints, &heard.heard());
    Ok(SoundEntry {
        file,
        seconds: Some(heard.seconds),
        kind: Some(found.kind),
        r#class: found.class,
        class_from: found.class_from,
        bpm: found.bpm.filter(|n| *n != 0. && !n.is_nan()),
        key: found.key.filter(|s| !s.is_empty()),
        note: found.note.filter(|s| !s.is_empty()),
        loudness: Some(heard.loudness_db),
        peak: Some(heard.peak_db),
        brightness: Some(heard.centroid_hz),
        flatness: Some(heard.flatness),
        attack: Some(heard.attack_ms),
        decay: Some(heard.decay_ms),
        width: Some(heard.width),
        onsets: Some(heard.onsets_per_second),
        low: Some(heard.low_share),
        high: Some(heard.high_share),
        vector: Some(pack_vector(&heard.vector)),
        features: Some(FEATURES_VERSION),
        ..Default::default()
    })
}
pub type LearnGate = Rc<dyn Fn() -> LocalBoxFuture<'static, ()>>;
pub type ProgressCallback = Rc<dyn Fn(LearnProgress)>;
pub struct LearnOptions {
    pub plan: LearnPlan,
    pub signal: Signal,
    pub rebuild: bool,
    pub gate: Option<LearnGate>,
    pub on_progress: Option<ProgressCallback>,
}
impl LearnOptions {
    pub fn new(plan: LearnPlan, signal: Signal) -> Self {
        Self { plan, signal, rebuild: false, gate: None, on_progress: None }
    }
}
#[derive(Clone)]
struct Found {
    file: Entry,
    source: Source,
    relative: String,
}
#[derive(Default)]
struct Finds {
    sounds: Vec<Found>,
    presets: Vec<Found>,
    sets: Vec<Found>,
    seen: HashSet<String>,
}
#[derive(Clone, Copy)]
struct Kinds {
    sounds: bool,
    presets: bool,
    sets: bool,
}
const SKIP: &[&str] =
    &["ableton folder info", "ableton project info", "__macosx", "node_modules", "$recycle.bin", "system volume information", "defaults"];
const PROJECT_COPIES: &[&str] = &["recorded", "processed", "imported", "freeze", "consolidated"];
fn backup_set(path: &str) -> bool {
    static RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\.backup-|\[[0-9]{4}-[0-9]{2}-[0-9]{2} [0-9]{6}\]\.als$").unwrap());
    RE.is_match(path)
}
fn mtime(time: SystemTime) -> i64 {
    let ms = match time.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs_f64() * 1000.,
        Err(e) => -e.duration().as_secs_f64() * 1000.,
    };
    kumi_common::js::number::round(ms) as i64
}
fn io_error(error: io::Error) -> RuntimeError {
    RuntimeError::plain(error.to_string())
}
async fn walk(
    source: &Source,
    kinds: Kinds,
    found: &mut Finds,
    signal: &Signal,
    depth_limit: usize,
    skip: &HashSet<String>,
) -> Result<bool, RuntimeError> {
    let mut queue = vec![(source.path.clone(), 0)];
    let mut complete = true;
    while let Some((path, depth)) = queue.pop() {
        signal.check()?;
        // A folder that can't be read (a NAS's hiccup, a placeholder nothing serves now) is skipped, and the walk
        // isn't whole: what was learned in it stays. One deleted meanwhile is gone.
        let mut entries = match tokio::fs::read_dir(&path).await {
            Ok(entries) => entries,
            Err(_) if depth == 0 => return Ok(false),
            Err(error) => {
                complete &= error.kind() == io::ErrorKind::NotFound;
                continue;
            }
        };
        let parent = basename(&path).to_lowercase();
        loop {
            let entry = match entries.next_entry().await {
                Ok(Some(entry)) => entry,
                Ok(None) => break,
                Err(_) => {
                    complete = false;
                    break;
                }
            };
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let full = join(&path, &name);
            let lower = name.to_lowercase();
            let Ok(kind) = entry.file_type().await else {
                complete = false;
                continue;
            };
            if kind.is_dir() {
                if SKIP.contains(&lower.as_str())
                    || lower == "backup"
                    || lower.ends_with(".app")
                    || (parent == "samples" && PROJECT_COPIES.contains(&lower.as_str()))
                    || skip.contains(&full)
                {
                    continue;
                }
                if depth + 1 > depth_limit {
                    complete = false;
                    continue;
                }
                queue.push((full, depth + 1));
                continue;
            }
            if !kind.is_file() {
                continue;
            }
            let ext = extension(&lower);
            if ext == ".als" && backup_set(&lower) {
                continue;
            }
            // Formats Live accepts as samples, intentionally narrower than the general audio decoder.
            let list = if kinds.sounds && crate::integrations::ableton::samples::SAMPLE_EXTENSIONS.contains(&ext.as_str()) {
                &mut found.sounds
            } else if kinds.presets && PRESET_EXTENSIONS.contains(&ext.as_str()) {
                &mut found.presets
            } else if kinds.sets && ext == ".als" {
                &mut found.sets
            } else {
                continue;
            };
            if !found.seen.insert(full.clone()) {
                continue;
            }
            let info = match tokio::fs::metadata(&full).await {
                Ok(info) => info,
                Err(error) => {
                    complete &= error.kind() == io::ErrorKind::NotFound;
                    continue;
                }
            };
            if info.len() < 64 {
                continue;
            }
            let Ok(modified) = info.modified() else { continue };
            let relative =
                Path::new(&full).strip_prefix(&source.path).map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| full.clone());
            list.push(Found {
                file: Entry { path: full, size: info.len(), mtime: mtime(modified), gone: None },
                source: source.clone(),
                relative,
            });
        }
    }
    Ok(complete)
}
fn order(kind: SourceKind) -> usize {
    match kind {
        SourceKind::UserLibrary => 0,
        SourceKind::Place => 1,
        SourceKind::Folder => 2,
        SourceKind::Splice => 3,
        SourceKind::Pack => 4,
        SourceKind::Core => 5,
    }
}
fn changed(known: Option<&Entry>, file: &Found) -> bool {
    known.is_none_or(|known| known.size != file.file.size || known.mtime != file.file.mtime)
}
/// Serial appends run in the background; an unwritable batch is retried by the next learning run.
struct Writer<T> {
    log: Log<T>,
    batch: Vec<T>,
    last: i64,
    pending: Option<tokio::task::JoinHandle<()>>,
}
impl<T: LogEntry + Clone + Send + Sync + 'static> Writer<T> {
    fn new(log: Log<T>) -> Self {
        Self { log, batch: vec![], last: now_ms(), pending: None }
    }
    fn add(&mut self, entry: T) {
        self.batch.push(entry);
        if self.batch.len() >= 100 || now_ms() - self.last > 2000 {
            self.enqueue();
        }
    }
    fn enqueue(&mut self) {
        let lines = std::mem::take(&mut self.batch);
        self.last = now_ms();
        let previous = self.pending.take();
        let log = self.log.clone();
        self.pending = Some(tokio::spawn(async move {
            if let Some(previous) = previous {
                let _ = previous.await;
            }
            let _ = log.append(&lines).await;
        }));
    }
    async fn flush(&mut self) {
        self.enqueue();
        if let Some(pending) = self.pending.take() {
            let _ = pending.await;
        }
    }
}
/// Each project's newest saved version contributes once to the producer's habits.
pub fn songs_of<'a>(entries: impl IntoIterator<Item = &'a SetEntry>) -> Vec<SetSummary> {
    let mut newest: IndexMap<String, &SetEntry> = IndexMap::new();
    for entry in entries {
        if entry.set.is_none() {
            continue;
        }
        let folder = dirname(&entry.path);
        if newest.get(&folder).is_none_or(|held| held.mtime < entry.mtime) {
            newest.insert(folder, entry);
        }
    }
    newest.values().filter_map(|entry| entry.set.clone()).collect()
}
struct Progress {
    value: RefCell<LearnProgress>,
    told: Cell<i64>,
    callback: Option<ProgressCallback>,
}
impl Progress {
    fn tell(&self, force: bool) {
        if force || now_ms() - self.told.get() > 250 {
            self.told.set(now_ms());
            let value = self.value.borrow().clone();
            if let Some(callback) = &self.callback {
                callback(value);
            }
        }
    }
    fn phase(&self, phase: LearnPhase) {
        self.value.borrow_mut().phase = phase;
        self.tell(true);
    }
}
async fn gone<T: LogEntry>(
    known: &mut IndexMap<String, T>,
    seen: &[Found],
    log: &Log<T>,
    roots: &[String],
    outside: bool,
) -> Result<(), RuntimeError> {
    let present: HashSet<_> = seen.iter().map(|file| &file.file.path).collect();
    let lost: Vec<_> = known
        .keys()
        .filter(|path| {
            !present.contains(path)
                && (roots.iter().any(|root| below(path, root).is_some())
                    || (outside && ((!Path::new(path).exists() && Path::new(&dirname(&dirname(path))).exists()) || backup_set(path))))
        })
        .cloned()
        .collect();
    // In one pass: removing them one at a time moves every entry after each.
    if !lost.is_empty() {
        let lost: HashSet<&String> = lost.iter().collect();
        known.retain(|path, _| !lost.contains(path));
    }
    log.append(&lost.into_iter().map(Entry::gone).collect::<Vec<_>>()).await.map_err(io_error)
}
async fn gated(options: &LearnOptions) -> Result<(), RuntimeError> {
    if let Some(gate) = &options.gate {
        gate().await;
    }
    options.signal.check().map_err(Into::into)
}
/// Learns changed files in source order, persisting partial progress before measuring the next batch.
pub async fn learn(options: LearnOptions) -> Result<LearnProgress, RuntimeError> {
    let plan = &options.plan;
    let logs = library_logs(&plan.dir);
    if options.rebuild {
        tokio::try_join!(
            logs.sounds.write(std::iter::empty()),
            logs.presets.write(std::iter::empty()),
            logs.sets.write(std::iter::empty())
        )
        .map_err(io_error)?;
    }
    let (mut sounds, mut presets, mut sets) = tokio::join!(logs.sounds.load(), logs.presets.load(), logs.sets.load());
    let progress = Progress {
        value: RefCell::new(LearnProgress {
            sounds: Counts { known: sounds.len(), ..Default::default() },
            presets: Counts { known: presets.len(), ..Default::default() },
            sets: Counts { known: sets.len(), ..Default::default() },
            started_at: now_ms(),
            ..Default::default()
        }),
        told: Cell::new(0),
        callback: options.on_progress.clone(),
    };
    progress.tell(true);
    let mut found = Finds::default();
    let (mut walked_sounds, mut walked_presets, mut walked_sets) = (vec![], vec![], vec![]);
    let mut sources = plan.sources.clone();
    sources.sort_by_key(|s| order(s.kind));
    for source in &sources {
        progress.value.borrow_mut().at = Some(source.label.clone());
        progress.tell(false);
        let own = matches!(source.kind, SourceKind::UserLibrary | SourceKind::Place | SourceKind::Folder);
        if walk(source, Kinds { sounds: true, presets: true, sets: own }, &mut found, &options.signal, 16, &HashSet::new()).await? {
            walked_sounds.push(source.path.clone());
            walked_presets.push(source.path.clone());
            if own {
                walked_sets.push(source.path.clone());
            }
        }
    }
    let source_paths = sources.iter().map(|s| s.path.clone()).collect();
    for path in &plan.set_folders {
        if sources.iter().any(|s| *path == s.path || below(path, &s.path).is_some()) {
            continue;
        }
        let source = Source { path: path.clone(), label: basename(path), kind: SourceKind::Folder };
        if walk(&source, Kinds { sounds: false, presets: false, sets: true }, &mut found, &options.signal, 6, &source_paths).await? {
            walked_sets.push(path.clone());
        }
    }
    for path in &plan.set_files {
        if found.seen.contains(path) || backup_set(&basename(path)) {
            continue;
        }
        let Ok(info) = tokio::fs::metadata(path).await else { continue };
        let Ok(modified) = info.modified() else { continue };
        found.seen.insert(path.clone());
        found.sets.push(Found {
            file: Entry { path: path.clone(), size: info.len(), mtime: mtime(modified), gone: None },
            source: Source { path: dirname(path), label: basename(&dirname(path)), kind: SourceKind::Folder },
            relative: basename(path),
        });
    }
    for path in &plan.plugin_presets {
        let source = Source { path: path.clone(), label: "Plug-in presets".into(), kind: SourceKind::Folder };
        if walk(&source, Kinds { sounds: false, presets: true, sets: false }, &mut found, &options.signal, 6, &HashSet::new()).await? {
            walked_presets.push(path.clone());
        }
    }
    gone(&mut sounds, &found.sounds, &logs.sounds, &walked_sounds, false).await?;
    gone(&mut presets, &found.presets, &logs.presets, &walked_presets, false).await?;
    gone(&mut sets, &found.sets, &logs.sets, &walked_sets, true).await?;
    let todo_sounds: Vec<_> = found
        .sounds
        .into_iter()
        .filter(|file| {
            let known = sounds.get(&file.file.path);
            changed(known.map(|k| &k.file), file) || known.is_some_and(|k| k.vector.is_some() && k.features != Some(FEATURES_VERSION))
        })
        .collect();
    let todo_presets: Vec<_> = found.presets.into_iter().filter(|f| changed(presets.get(&f.file.path).map(|k| &k.file), f)).collect();
    let todo_sets: Vec<_> = found.sets.into_iter().filter(|f| changed(sets.get(&f.file.path).map(|k| &k.file), f)).collect();
    {
        let mut p = progress.value.borrow_mut();
        p.sounds = Counts { known: sounds.len(), todo: todo_sounds.len(), done: 0 };
        p.presets = Counts { known: presets.len(), todo: todo_presets.len(), done: 0 };
        p.sets = Counts { known: sets.len(), todo: todo_sets.len(), done: 0 };
    }
    progress.phase(LearnPhase::Presets);
    let mut preset_writer = Writer::new(logs.presets.clone());
    for file in &todo_presets {
        gated(&options).await?;
        let ext = extension(&file.file.path);
        let format = ext.to_lowercase().trim_start_matches('.').to_owned();
        let plugin = file.source.label == "Plug-in presets";
        let facts = match format.as_str() {
            "adv" | "adg" => read_live_preset(&file.file.path).await,
            "amxd" => read_max_device(&file.file.path).await,
            _ => Ok(plugin_preset_facts(&file.relative)),
        };
        let (facts, error) = match facts {
            Ok(facts) => (facts, None),
            Err(error) => {
                progress.value.borrow_mut().failed += 1;
                (PresetFacts::default(), Some(head(&error.to_string(), 120)))
            }
        };
        let folder = dirname(&file.relative);
        let name = basename(&file.file.path);
        let name = name.strip_suffix(&ext).unwrap_or(&name).to_owned();
        let entry = PresetEntry {
            file: file.file.clone(),
            name,
            format,
            device: facts.device,
            category: if plugin { Some(PresetCategory::Plugin) } else { facts.category },
            inside: facts.inside,
            about: facts.about,
            source: file.source.label.clone(),
            folder: if folder == "." { String::new() } else { folder },
            browser: if plugin { None } else { browser_path(&file.source, &file.relative).filter(|s| !s.is_empty()) },
            error,
        };
        presets.insert(file.file.path.clone(), entry.clone());
        preset_writer.add(entry);
        {
            let mut p = progress.value.borrow_mut();
            p.presets.done += 1;
            p.presets.known = presets.len();
        }
        progress.tell(false);
    }
    preset_writer.flush().await;
    progress.phase(LearnPhase::Sets);
    let mut set_writer = Writer::new(logs.sets.clone());
    for file in &todo_sets {
        gated(&options).await?;
        let entry = match read_set(&file.file.path, Some(options.signal.clone())).await {
            Ok(set) => SetEntry { file: file.file.clone(), set: Some(set), error: None },
            Err(error) => {
                options.signal.check()?;
                progress.value.borrow_mut().failed += 1;
                SetEntry { file: file.file.clone(), set: None, error: Some(head(&error.to_string(), 120)) }
            }
        };
        sets.insert(file.file.path.clone(), entry.clone());
        set_writer.add(entry);
        {
            let mut p = progress.value.borrow_mut();
            p.sets.done += 1;
            p.sets.known = sets.len();
        }
        progress.tell(false);
        tokio::task::yield_now().await;
    }
    set_writer.flush().await;
    write_json(&Path::new(&plan.dir).join("taste.json"), &build_taste(&songs_of(sets.values()), None)).await.map_err(io_error)?;
    progress.phase(LearnPhase::Sounds);
    let size = plan
        .workers
        .unwrap_or_else(|| std::thread::available_parallelism().map(usize::from).unwrap_or(1).saturating_sub(2).clamp(1, 2))
        .min(4);
    let pool = MeasurePool::new(size);
    let next = Cell::new(0);
    let sounds = RefCell::new(sounds);
    let sound_writer = RefCell::new(Writer::new(logs.sounds.clone()));
    let work = futures::future::try_join_all((0..size.max(1)).map(|slot| {
        let (pool, next, sounds, sound_writer, progress, todo, options) =
            (&pool, &next, &sounds, &sound_writer, &progress, &todo_sounds, &options);
        async move {
            while next.get() < todo.len() {
                gated(options).await?;
                // Several paused slots may resume together after another has taken the last file.
                let Some(file) = todo.get(next.get()) else { break };
                next.set(next.get() + 1);
                progress.value.borrow_mut().at = Some(file.source.label.clone());
                let entry = pool
                    .run(
                        slot,
                        MeasureJob {
                            id: 0,
                            path: file.file.path.clone(),
                            relative: file.relative.clone(),
                            size: file.file.size,
                            mtime: file.file.mtime,
                            start: None,
                            seconds: None,
                        },
                    )
                    .await;
                options.signal.check()?;
                if entry.error.is_some() && entry.r#class.is_none() && entry.kind.is_none() {
                    progress.value.borrow_mut().failed += 1;
                }
                let known = {
                    let mut sounds = sounds.borrow_mut();
                    sounds.insert(file.file.path.clone(), entry.clone());
                    sounds.len()
                };
                sound_writer.borrow_mut().add(entry);
                {
                    let mut p = progress.value.borrow_mut();
                    p.sounds.done += 1;
                    p.sounds.known = known;
                }
                progress.tell(false);
            }
            Ok::<_, RuntimeError>(())
        }
    }))
    .await;
    let mut sound_writer = sound_writer.into_inner();
    sound_writer.flush().await;
    pool.close().await;
    work?;
    let sounds = sounds.into_inner();
    progress.value.borrow_mut().at = None;
    progress.phase(LearnPhase::Tidying);
    if !todo_sounds.is_empty() {
        logs.sounds.write(sounds.values()).await.map_err(io_error)?;
    }
    if !todo_presets.is_empty() {
        logs.presets.write(presets.values()).await.map_err(io_error)?;
    }
    if !todo_sets.is_empty() {
        logs.sets.write(sets.values()).await.map_err(io_error)?;
    }
    progress.value.borrow_mut().finished_at = Some(now_ms());
    progress.phase(LearnPhase::Done);
    Ok(progress.value.into_inner())
}
