//! Sample and parameter services used while preparing a Live change.
use super::{
    changes::{ChangeContext, NoSample, ParameterRange, SampleFile, SampleSelector},
    parameters::Parameters,
    samples::{self, Sample},
};
use crate::{
    core::errors::RuntimeError,
    library::sources::{
        below, homedir, library_sources, live_preference_folders, read_indexer_log, read_library_config, SourceKind, SourceOptions,
    },
};
use async_trait::async_trait;
use indexmap::IndexMap;
use kumi_common::{
    abort::{Signal, SignalExt},
    path::network_or_device,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    cell::RefCell,
    collections::HashSet,
    fs::File,
    io::{ErrorKind, Read, Write},
    path::{Path, PathBuf},
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// The bridge's bound on an import (`MAX_BYTES` in its import_files.rs): a bigger sound isn't copied here for it.
const MAX_IMPORT_BYTES: u64 = 512 * 1024 * 1024;
/// How long Live's Places, once read, are taken as they were.
const PLACES_FOR: Duration = Duration::from_secs(60);
/// A copy no Arrangement clip plays goes once it hasn't been used for this long: Live's other imports play the
/// bridge's own copy of it.
const UNUSED_FOR: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// In a copy's folder: touched each time it's used, and there for good once an Arrangement clip plays it.
const USED: &str = ".used";
const KEPT: &str = ".kept";

#[derive(Default)]
pub struct SampleBank {
    pub samples: RefCell<IndexMap<String, Sample>>,
    pub picked: RefCell<HashSet<String>>,
    /// Live's own Places (the User Library and the folders added in Live's Browser), when a test names them; read
    /// from Live's Library.cfg otherwise. A named one that isn't a folder here is a share that's off.
    pub places: RefCell<Option<Vec<String>>>,
    /// Where copies of Places' sounds on a share go; Kumi's own cache otherwise.
    pub cache: RefCell<Option<PathBuf>>,
    /// Live's Places as last read, and when.
    read: RefCell<Option<(Instant, Rc<Places>)>>,
}
/// Live's Places: the ones it reads now, and every one it names, a share that's off included.
#[derive(Default)]
struct Places {
    readable: Vec<String>,
    named: Vec<String>,
}
/// Live's own Places: the User Library and the folders the producer added in Live, as Library.cfg (and Live's
/// indexer) list them. It opens each, so it's read off Kumi's thread.
fn live_places() -> Places {
    let options = SourceOptions::default();
    let readable: Vec<String> = library_sources(&options)
        .into_iter()
        .filter(|source| matches!(source.kind, SourceKind::Place | SourceKind::UserLibrary))
        .map(|source| source.path)
        .collect();
    let mut named = readable.clone();
    if let Some(preferences) = live_preference_folders(&options).into_iter().next() {
        let text = |name: &str| std::fs::read(Path::new(&preferences).join(name)).map(|text| String::from_utf8_lossy(&text).into_owned());
        let config = read_library_config(&text("Library.cfg").unwrap_or_default());
        let indexer = read_indexer_log(&text("Indexer.txt").unwrap_or_default());
        named.extend(config.user_library.into_iter().chain(config.places).chain(indexer.places.into_iter().map(|place| place.path)));
    }
    Places { readable, named }
}
fn kumi_cache() -> PathBuf {
    std::env::var("KUMI_HOME")
        .ok()
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home::home_dir().unwrap_or_default().join(".kumi"))
        .join("cache")
        .join("places")
}
/// Whether `path` is inside `place`: on Windows in any ASCII case, as its file systems and shares read names (ASCII
/// only, so no other folding widens it).
fn inside(path: &str, place: &str) -> bool {
    if cfg!(windows) {
        below(&path.to_ascii_lowercase(), &place.to_ascii_lowercase()).is_some()
    } else {
        below(path, place).is_some()
    }
}
/// A path part that climbs (".", "..", or one Windows may take for one): it would leave a Place on its share.
fn climbs(path: &str) -> bool {
    path.split(['/', '\\']).any(|part| !part.is_empty() && part.chars().all(|c| c == '.' || c == ' '))
}
/// Copies `from`, a sound in the Place `place` on a share, into a folder of `cache` named for the file as it is (path,
/// size and mtime), unless a copy of it is there already. A copy that fails or is stopped leaves nothing.
fn copy_from_place(from: &Path, place: &Path, cache: &Path, signal: &Signal) -> Result<SampleFile, NoSample> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let name = from.file_name().ok_or(NoSample::NotThere)?;
    let stat = match std::fs::metadata(from) {
        Ok(stat) => stat,
        // The Place is there without it: there's no such sound. Otherwise its share couldn't be read.
        Err(error) if error.kind() == ErrorKind::NotFound && place.is_dir() => return Err(NoSample::NotThere),
        Err(_) => return Err(NoSample::ShareUnread),
    };
    if !stat.is_file() {
        return Err(NoSample::NotThere);
    }
    if stat.len() == 0 || stat.len() > MAX_IMPORT_BYTES {
        return Err(NoSample::OutOfBounds);
    }
    let modified = stat.modified().ok().and_then(|at| at.duration_since(UNIX_EPOCH).ok()).map_or(0, |at| at.as_nanos());
    let key = hex::encode(Sha256::digest(format!("{}\n{}\n{modified}", from.to_string_lossy(), stat.len())));
    let folder = cache.join(&key[..16]);
    let copy = folder.join(name);
    let file = SampleFile { path: copy.to_string_lossy().into_owned(), folder: folder.to_string_lossy().into_owned() };
    // A copy is renamed in whole, so one that's there is complete. A changed file gets a folder of its own, and the
    // copy a Set may play is never written over.
    if copy.is_file() {
        used(&folder);
        return Ok(file);
    }
    trim(cache);
    let partial = folder.join(format!(".{}.{}.part", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    let copied = File::open(from).map_err(|_| NoSample::ShareUnread).and_then(|source| {
        std::fs::create_dir_all(&folder)
            .and_then(|()| copy_stoppable(source, &partial, signal))
            .and_then(|()| std::fs::rename(&partial, &copy))
            .map_err(|_| NoSample::ShareUnread)
    });
    if copied.is_err() {
        let _ = std::fs::remove_file(&partial);
        // Another copy of this same file got there first (Windows doesn't rename over a file that's open).
        if !copy.is_file() {
            return Err(NoSample::ShareUnread);
        }
    }
    used(&folder);
    Ok(file)
}
/// `source` into a new file at `partial`, checking `signal` between chunks, up to the import bound.
fn copy_stoppable(source: File, partial: &Path, signal: &Signal) -> std::io::Result<()> {
    let mut source = source.take(MAX_IMPORT_BYTES);
    let mut copy = File::options().write(true).create_new(true).open(partial)?;
    let mut chunk = vec![0; 1 << 20];
    loop {
        if signal.aborted() {
            return Err(std::io::Error::other("stopped"));
        }
        match source.read(&mut chunk) {
            Ok(0) => return Ok(()),
            Ok(read) => copy.write_all(&chunk[..read])?,
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
}
/// Notes that a copy's folder was just used.
fn used(folder: &Path) {
    let _ = File::create(folder.join(USED)).and_then(|stamp| stamp.set_modified(SystemTime::now()));
}
/// Drops the copies no Arrangement clip plays that haven't been used for `UNUSED_FOR`.
fn trim(cache: &Path) {
    let Ok(entries) = std::fs::read_dir(cache) else { return };
    for entry in entries.flatten() {
        let folder = entry.path();
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) || folder.join(KEPT).exists() {
            continue;
        }
        let last = std::fs::metadata(folder.join(USED)).or_else(|_| entry.metadata()).and_then(|stat| stat.modified());
        if last.is_ok_and(|last| last.elapsed().is_ok_and(|idle| idle > UNUSED_FOR)) {
            let _ = std::fs::remove_dir_all(&folder);
        }
    }
}
impl SampleBank {
    /// The file a change imports for `path`: one find_samples gave, or an audio file on this computer. A sound on a
    /// network share is imported only from inside one of Live's own Places, and from a copy here: the bridge refuses
    /// shares (opening one sends Windows' credentials to its host), and Live opens its Places itself. Any other share
    /// is never opened.
    pub async fn sample(&self, path: &str, signal: &Signal) -> Result<Result<SampleFile, NoSample>, RuntimeError> {
        let Some(file) = self.located(path) else { return Ok(Err(NoSample::NotThere)) };
        self.deliverable(file, signal).await
    }
    async fn deliverable(&self, file: SampleFile, signal: &Signal) -> Result<Result<SampleFile, NoSample>, RuntimeError> {
        if !network_or_device(&file.path) {
            return Ok(Ok(file));
        }
        if climbs(&file.path) {
            return Ok(Err(NoSample::NotThere));
        }
        // Reading them can wait out a share that's off: a stop doesn't, and the read finishes on its own.
        let places = tokio::select! {
            biased;
            () = signal.cancelled() => return Err(RuntimeError::Aborted),
            places = self.places() => places,
        };
        let Some(place) = places.readable.iter().find(|place| inside(&file.path, place)) else {
            // A Place Live names that it can't read now is a share that's off: that, rather than a wrong path.
            let off = places.named.iter().any(|place| inside(&file.path, place));
            return Ok(Err(if off { NoSample::ShareUnread } else { NoSample::NotThere }));
        };
        let (from, place, cache) =
            (PathBuf::from(&file.path), PathBuf::from(place), self.cache.borrow().clone().unwrap_or_else(kumi_cache));
        let stop = signal.clone();
        // Off Kumi's thread: it's read over the network. Stopping doesn't wait for a read the network holds; the copy
        // then ends at its next chunk and removes what it wrote.
        let copying = tokio::task::spawn_blocking(move || copy_from_place(&from, &place, &cache, &stop));
        tokio::select! {
            copied = copying => Ok(copied.unwrap_or(Err(NoSample::ShareUnread))),
            () = signal.cancelled() => Err(RuntimeError::Aborted),
        }
    }
    /// Live's Places, read off Kumi's thread at most once a minute (a test's are taken as they are each time).
    async fn places(&self) -> Rc<Places> {
        let named = self.places.borrow().clone();
        if named.is_none() {
            if let Some((at, places)) = self.read.borrow().as_ref() {
                if at.elapsed() < PLACES_FOR {
                    return places.clone();
                }
            }
        }
        let tested = named.is_some();
        let places = Rc::new(
            tokio::task::spawn_blocking(move || match named {
                Some(named) => Places { readable: named.iter().filter(|place| Path::new(place).is_dir()).cloned().collect(), named },
                None => live_places(),
            })
            .await
            .unwrap_or_default(),
        );
        if !tested {
            *self.read.borrow_mut() = Some((Instant::now(), places.clone()));
        }
        places
    }
    /// Keeps a copy here for good once an Arrangement clip plays it (Live opens that where it is).
    pub async fn keep(&self, file: &SampleFile) {
        let cache = self.cache.borrow().clone().unwrap_or_else(kumi_cache);
        let folder = PathBuf::from(&file.folder);
        if folder.parent() == Some(cache.as_path()) {
            let _ = tokio::task::spawn_blocking(move || File::create(folder.join(KEPT))).await;
        }
    }
    fn located(&self, path: &str) -> Option<SampleFile> {
        if let Some(sample) = self.samples.borrow().get(path) {
            return Some(SampleFile { path: sample.path.clone(), folder: sample.folder.clone() });
        }
        let full = if path.starts_with("~/") || path.starts_with("~\\") { format!("{}{}", homedir(), &path[1..]) } else { path.into() };
        let path = Path::new(&full);
        // A share isn't looked at here: `deliverable` opens it only once it's known to be inside a Place.
        let share = network_or_device(&full);
        if !path.is_absolute()
            || !path
                .extension()
                .and_then(|s| s.to_str())
                .is_some_and(|ext| samples::SAMPLE_EXTENSIONS.contains(&format!(".{}", ext.to_lowercase()).as_str()))
        {
            return None;
        }
        if !share && !std::fs::metadata(path).ok()?.is_file() {
            return None;
        }
        Some(SampleFile { folder: path.parent()?.to_string_lossy().into_owned(), path: full })
    }
    pub async fn pick(&self, selector: SampleSelector, signal: Signal) -> Result<Result<SampleFile, NoSample>, RuntimeError> {
        let named: Vec<_> = selector.folders.iter().filter_map(|folder| samples::folder_path(folder, None)).collect();
        let found = samples::find_samples(samples::FindSamplesOptions {
            folders: if named.is_empty() { samples::default_sample_folders(None, None, None) } else { named },
            random: selector.random || selector.words.is_empty(),
            words: selector.words,
            limit: 50,
            signal: Some(signal.clone()),
        })
        .await?;
        let choice = found.samples.into_iter().find(|sample| !self.picked.borrow().contains(&sample.path));
        let Some(choice) = choice else { return Ok(Err(NoSample::NotThere)) };
        self.picked.borrow_mut().insert(choice.path.clone());
        let file = SampleFile { path: choice.path.clone(), folder: choice.folder.clone() };
        {
            let mut samples = self.samples.borrow_mut();
            samples.shift_remove(&choice.path);
            samples.insert(choice.path.clone(), choice);
        }
        self.deliverable(file, &signal).await
    }
}
pub struct PreparationContext<'a> {
    pub parameters: &'a Parameters,
    pub samples: &'a SampleBank,
    pub signal: Signal,
}
#[async_trait(?Send)]
impl ChangeContext for PreparationContext<'_> {
    async fn sample(&self, path: &str) -> Result<Result<SampleFile, NoSample>, RuntimeError> {
        self.samples.sample(path, &self.signal).await
    }
    async fn parameters(&self, device_ref: &str) -> Result<Vec<ParameterRange>, RuntimeError> {
        self.read(device_ref, false).await
    }
    async fn ranges(&self, device_ref: &str) -> Result<Vec<ParameterRange>, RuntimeError> {
        self.read(device_ref, true).await
    }
    async fn pick(&self, selector: SampleSelector) -> Result<Result<SampleFile, NoSample>, RuntimeError> {
        self.samples.pick(selector, self.signal.clone()).await
    }
    async fn keep(&self, file: &SampleFile) {
        self.samples.keep(file).await;
    }
    fn has_value_for(&self) -> bool {
        true
    }
    async fn value_for(&self, parameter_ref: &str, text: &str) -> Result<Result<f64, String>, RuntimeError> {
        self.parameters.value_for_text(parameter_ref, text, self.signal.clone()).await
    }
}
impl PreparationContext<'_> {
    async fn read(&self, device_ref: &str, ranges: bool) -> Result<Vec<ParameterRange>, RuntimeError> {
        let fields = if ranges { vec!["ref", "name", "min", "max", "value", "displayValue"] } else { vec!["ref", "name"] };
        let rows = self
            .parameters
            .device_parameters(json!(device_ref), fields.into_iter().map(str::to_owned).collect(), self.signal.clone())
            .await?;
        Ok(rows
            .into_iter()
            .filter_map(|row| {
                Some(ParameterRange {
                    reference: row.get("ref")?.as_str()?.into(),
                    name: row.get("name")?.as_str()?.into(),
                    min: row.get("min").and_then(|v| v.as_f64()).filter(|_| ranges),
                    max: row.get("max").and_then(|v| v.as_f64()).filter(|_| ranges),
                    value: row.get("value").and_then(|v| v.as_f64()).filter(|_| ranges),
                    display: row.get("displayValue").and_then(|v| v.as_str()).filter(|_| ranges).map(str::to_owned),
                })
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place_with(names: &[&str]) -> (tempfile::TempDir, PathBuf) {
        let folder = tempfile::tempdir().unwrap();
        let place = folder.path().join("Place");
        std::fs::create_dir(&place).unwrap();
        for name in names {
            std::fs::write(place.join(name), format!("RIFF {name}")).unwrap();
        }
        (folder, place)
    }

    #[test]
    fn a_copy_stopped_partway_leaves_nothing_behind() {
        let (folder, place) = place_with(&["kick.wav"]);
        let cache = folder.path().join("cache");
        let stopped = Signal::new();
        stopped.cancel();
        assert_eq!(copy_from_place(&place.join("kick.wav"), &place, &cache, &stopped), Err(NoSample::ShareUnread));
        for copy in std::fs::read_dir(&cache).unwrap().flatten() {
            assert_eq!(std::fs::read_dir(copy.path()).unwrap().count(), 0, "{copy:?}");
        }
        let copied = copy_from_place(&place.join("kick.wav"), &place, &cache, &Signal::new()).unwrap();
        assert_eq!(std::fs::read(&copied.path).unwrap(), b"RIFF kick.wav");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn copies_unused_for_a_week_go_unless_an_arrangement_clip_plays_them() {
        let (folder, place) = place_with(&["a.wav", "b.wav", "c.wav", "local.wav"]);
        let cache = folder.path().join("cache");
        let bank = SampleBank { cache: RefCell::new(Some(cache.clone())), ..SampleBank::default() };
        let copy = |name: &str| copy_from_place(&place.join(name), &place, &cache, &Signal::new()).unwrap();
        let (played, unplayed) = (copy("a.wav"), copy("b.wav"));
        bank.keep(&played).await;
        let week_ago = SystemTime::now() - UNUSED_FOR - Duration::from_secs(60);
        for copy in [&played, &unplayed] {
            File::options().write(true).open(Path::new(&copy.folder).join(USED)).unwrap().set_modified(week_ago).unwrap();
        }
        let fresh = copy("c.wav");
        assert!(Path::new(&played.path).is_file() && Path::new(&fresh.path).is_file());
        assert!(!Path::new(&unplayed.folder).exists());
        // Only Kumi's own copies are kept: a sound's own folder isn't touched.
        let local =
            SampleFile { path: place.join("local.wav").to_string_lossy().into_owned(), folder: place.to_string_lossy().into_owned() };
        bank.keep(&local).await;
        assert!(!place.join(KEPT).exists());
    }
}
