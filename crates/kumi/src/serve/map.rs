//! C3's current learned sound catalog and C1's path-free search. A revision includes every approved entry, with
//! points only for sounds the existing layout positioned. Only explicit refresh invalidates a live revision.

use super::artifacts::FileIdentity;
use super::ctx::{reads, Call, Ctx, OpError, OpFuture, Operation};
use super::snapshots;
use kumi_common::abort::Signal;
use kumi_library::{
    features::VECTOR_LENGTH,
    learn::SoundEntry,
    map::{read_sounds_map, sounds_on_map_by_turns, SoundsMap},
    search::{LikeSound, SoundIndex, SoundQuery},
    store::unpack_vector,
    Library, SoundClass, SoundKind, Source,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
    rc::Rc,
};
use tokio::sync::{mpsc, oneshot, Mutex};

/// The Python snapshot reader refuses larger files, including the snapshot envelope.
const MAX_SNAPSHOT_BYTES: usize = 64 * 1024 * 1024;

pub static SOUNDS: Operation = Operation {
    name: "sounds",
    description: "Search the current approved sound catalog by words, kind (kick, snare, pad, loop, one-shot…), or a current sound_id's measured similarity. Includes sounds not yet positioned on the map. Returns revision-scoped IDs, never file paths. Does not scan or remap the library.",
    schema: schema,
    effect: reads,
    run: run,
};

fn schema() -> Value {
    json!({"type":"object", "additionalProperties":false, "required":["query"], "properties":{
        "query":{"type":"string", "maxLength":4096},
        "near":{"type":"string", "minLength":1, "maxLength":128},
        "kind":{"type":"string", "minLength":1, "maxLength":64},
        "limit":{"type":"integer", "minimum":1, "maximum":100, "default":20}
    }})
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Args {
    query: String,
    near: Option<String>,
    kind: Option<String>,
    #[serde(default = "default_limit")]
    limit: usize,
}

fn default_limit() -> usize {
    20
}

impl Args {
    fn search(&self) -> Result<SoundQuery, OpError> {
        if !(1..=100).contains(&self.limit) || self.query.len() > 4096 {
            return Err(OpError::Invalid { reason: "limit must be 1..100 and query at most 4096 bytes".into() });
        }
        let mut query = SoundQuery { words: kumi_library::search::tokens(&self.query), limit: self.limit, ..Default::default() };
        if let Some(kind) = &self.kind {
            let kind = kind.trim().to_lowercase();
            if let Some(class) = SoundClass::parse(&kind) {
                query.classes.push(class);
            } else {
                query.kind = Some(serde_json::from_value::<SoundKind>(json!(kind)).map_err(|_| OpError::Invalid {
                    reason: "kind must name a sound class (such as kick, snare or pad), loop, or one-shot".into(),
                })?);
            }
        }
        Ok(query)
    }
}

fn run(ctx: &Ctx, call: Call) -> OpFuture<'_> {
    Box::pin(async move {
        let args: Args =
            serde_json::from_value(Value::Object(call.args)).map_err(|error| OpError::Invalid { reason: error.to_string() })?;
        ctx.maps().search(ctx, args, &call.signal).await
    })
}

#[derive(Clone, Serialize)]
struct Sound {
    name: String,
    kind: Option<SoundKind>,
    class: Option<SoundClass>,
    folder: String,
    duration: Option<f64>,
    path: PathBuf,
}

impl Sound {
    fn public(&self, id: &str) -> Value {
        json!({"sound_id":id, "name":self.name, "kind":self.kind, "class":self.class,
            "folder":self.folder, "duration":self.duration})
    }
}

#[derive(Clone)]
struct File {
    indexed: PathBuf,
    canonical: PathBuf,
    identity: FileIdentity,
}

// A cached index is Rc-based, so it lives on its own thread rather than being copied for every query. The queue is
// bounded, and callers can cancel both queueing and waiting. Dropping a revision closes its worker's channel.
struct Request {
    query: SoundQuery,
    near: Option<String>,
    reply: oneshot::Sender<Result<Vec<String>, OpError>>,
}

struct Current {
    revision: String,
    sounds: BTreeMap<String, Sound>,
    files: HashMap<String, File>,
    ids: HashMap<String, String>,
    search: mpsc::Sender<Request>,
}

#[cfg(test)]
struct CommitGate {
    entered: oneshot::Sender<()>,
    release: std::sync::mpsc::Receiver<()>,
}

pub struct MapStore {
    pointer: PathBuf,
    current: RefCell<Option<Rc<Current>>>,
    refreshing: Mutex<()>,
    #[cfg(test)]
    commit_gate: RefCell<Option<CommitGate>>,
}

impl MapStore {
    pub fn new(state_dir: &Path) -> Result<Self, OpError> {
        let pointer = state_dir.join("map/current");
        // Persisted snapshots are not grants. Before this run can advertise anything, revoke the last run's pointer.
        match std::fs::remove_file(&pointer) {
            Ok(()) => (),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(failed(error)),
        }
        Ok(Self {
            pointer,
            current: RefCell::new(None),
            refreshing: Mutex::new(()),
            #[cfg(test)]
            commit_gate: RefCell::new(None),
        })
    }

    /// Pause this store's next pointer worker only after it has crossed the cancellation boundary.
    #[cfg(test)]
    pub(super) fn gate_next_commit(&self) -> (oneshot::Receiver<()>, std::sync::mpsc::Sender<()>) {
        let (entered, reached) = oneshot::channel();
        let (release, waiting) = std::sync::mpsc::channel();
        assert!(self.commit_gate.replace(Some(CommitGate { entered, release: waiting })).is_none());
        (reached, release)
    }

    pub fn revision(&self) -> Option<String> {
        self.current.borrow().as_ref().map(|current| current.revision.clone())
    }

    /// Startup must not replace a map already published by a tool or a completed scan.
    pub(crate) async fn initialize(&self, ctx: &Ctx, signal: &Signal) -> Result<(), OpError> {
        self.ensure(ctx, signal).await.map(|_| ())
    }

    pub async fn refresh(&self, ctx: &Ctx, library: &Library, signal: &Signal) -> Result<String, OpError> {
        let _guard = tokio::select! {
            _ = signal.cancelled() => return Err(cancelled()),
            guard = self.refreshing.lock() => guard,
        };
        self.refresh_locked(ctx, library, signal).await
    }

    async fn refresh_locked(&self, ctx: &Ctx, library: &Library, signal: &Signal) -> Result<String, OpError> {
        check(signal)?;
        let dir = ctx.paths().state_dir.join("library");
        let layout = tokio::task::spawn_blocking(move || read_sounds_map(&dir)).await.map_err(failed)?;
        check(signal)?;
        let index = library.sound_index_in(Some(ctx.daw())).await.map_err(failed)?;
        let sources = library.sources().await;
        self.project(ctx, layout, &index, sources, signal).await
    }

    async fn ensure(&self, ctx: &Ctx, signal: &Signal) -> Result<Rc<Current>, OpError> {
        check(signal)?;
        if let Some(current) = self.current.borrow().clone() {
            return Ok(current);
        }
        let _guard = tokio::select! {
            _ = signal.cancelled() => return Err(cancelled()),
            guard = self.refreshing.lock() => guard,
        };
        if let Some(current) = self.current.borrow().clone() {
            return Ok(current);
        }
        let library = super::library::get(ctx).await?;
        self.refresh_locked(ctx, &library, signal).await?;
        self.current.borrow().clone().ok_or_else(|| failed("map was not published"))
    }

    async fn project(
        &self,
        ctx: &Ctx,
        layout: Option<SoundsMap>,
        index: &SoundIndex,
        sources: Vec<Source>,
        signal: &Signal,
    ) -> Result<String, OpError> {
        let made = layout.as_ref().map(|map| map.made);
        let mapped = match layout {
            Some(map) => sounds_on_map_by_turns(&map, index).await,
            None => Vec::new(),
        };
        check(signal)?;
        // A layout can intentionally lag a small scan, or omit an unmeasured sound entirely. It is not the
        // catalog: validate every learned entry, once, off the current-thread runtime.
        let mut candidates = Vec::with_capacity(index.entries.len());
        for (row, entry) in index.entries.iter().enumerate() {
            if entry.gone != Some(true) {
                candidates.push((row, SoundEntry::clone(entry)));
            }
            if row % 1024 == 0 {
                tokio::task::yield_now().await;
                check(signal)?;
            }
        }
        let roots = ctx.library_roots();
        let checking = signal.clone();
        let approved = tokio::task::spawn_blocking(move || {
            let mut seen = HashSet::new();
            let mut approved = Vec::with_capacity(candidates.len());
            for (row, entry) in candidates {
                check(&checking)?;
                if let Ok(file) = checked_file(Path::new(&entry.path), &roots) {
                    if seen.insert(file.canonical.clone()) {
                        approved.push((row, entry, file));
                    }
                }
            }
            Ok::<_, OpError>(approved)
        })
        .await
        .map_err(failed)??;
        check(signal)?;
        let revision = format!("r{:032x}", rand::random::<u128>());
        let mut sounds = BTreeMap::new();
        let mut files = HashMap::new();
        let mut ids = HashMap::new();
        let mut entries = Vec::with_capacity(approved.len());
        for (at, (row, entry, file)) in approved.into_iter().enumerate() {
            let id = format!("s-{revision}-{at:x}");
            sounds.insert(
                id.clone(),
                Sound {
                    name: index.name(row).to_owned(),
                    kind: entry.kind,
                    class: entry.r#class,
                    folder: index.place(row).to_owned(),
                    duration: entry.seconds.filter(|seconds| seconds.is_finite() && *seconds >= 0.0),
                    path: file.canonical.clone(),
                },
            );
            ids.insert(entry.path.clone(), id.clone());
            files.insert(id, file);
            entries.push(entry);
            if at % 1024 == 0 {
                tokio::task::yield_now().await;
                check(signal)?;
            }
        }
        // Preserve the layout's positions and grouping semantics; never invent coordinates for catalog-only rows.
        let layer_sounds = mapped
            .into_iter()
            .filter(|sound| ids.contains_key(&sound.path) && (0.0..=1.0).contains(&sound.x) && (0.0..=1.0).contains(&sound.y))
            .collect();
        let layer = tokio::task::spawn_blocking(move || crate::map::sounds_layer(layer_sounds)).await.map_err(failed)?;
        let points: Vec<_> = layer.points.iter().map(|point| json!([ids[&point.path], point.x, point.y, point.group])).collect();
        let groups: BTreeMap<_, _> = layer
            .groups
            .iter()
            .enumerate()
            .map(|(at, group)| (at.to_string(), json!({"name":group.name,"color":group.color,"count":group.count})))
            .collect();
        let body = json!({"revision":revision, "made":made, "points":points, "groups":groups, "sounds":sounds});
        let search = worker(entries, sources).await?;
        check(signal)?;
        self.publish(ctx, Current { revision, sounds, files, ids, search }, body, MAX_SNAPSHOT_BYTES, signal).await
    }

    async fn publish(&self, ctx: &Ctx, current: Current, body: Value, max_bytes: usize, signal: &Signal) -> Result<String, OpError> {
        check(signal)?;
        let revision = current.revision.clone();
        // Hold the refresh mutex through both durable writes and pruning. A refused oversized snapshot must not
        // replace either the in-memory catalog or its durable pointer.
        ctx.snapshots().write_durably_bounded(&format!("map/{revision}"), body, max_bytes).await.map_err(|error| {
            if error.kind() == std::io::ErrorKind::InvalidData {
                OpError::Refused {
                    code: "map_too_large",
                    reason: "The sound map exceeds the 64 MiB reader limit; the previous map remains available.".into(),
                    owner: None,
                }
            } else {
                failed(error)
            }
        })?;
        // Cancellation while the writer serialized or flushed leaves the previous revision advertised. Never drop
        // an in-flight durable write; wait for it, then refuse before committing the pointer.
        if signal.is_cancelled() {
            let pointer = self.pointer.clone();
            let unpublished = revision.clone();
            let _ = tokio::task::spawn_blocking(move || {
                let published = std::fs::read_to_string(&pointer);
                if published.as_ref().is_ok_and(|text| text.trim() != unpublished)
                    || published.as_ref().is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
                {
                    if let Some(dir) = pointer.parent() {
                        let _ = std::fs::remove_file(dir.join(format!("{unpublished}.json")));
                    }
                }
            })
            .await;
            return Err(cancelled());
        }
        let pointer = self.pointer.clone();
        let contents = format!("{revision}\n");
        #[cfg(test)]
        let gate = self.commit_gate.borrow_mut().take();
        tokio::task::spawn_blocking(move || {
            #[cfg(test)]
            if let Some(gate) = gate {
                let _ = gate.entered.send(());
                gate.release.recv_timeout(std::time::Duration::from_secs(10)).map_err(std::io::Error::other)?;
            }
            snapshots::replace(&pointer, contents.as_bytes())
        })
        .await
        .map_err(failed)?
        .map_err(failed)?;
        let previous = self.current.replace(Some(Rc::new(current))).map(|before| before.revision.clone());
        let pointer = self.pointer.clone();
        let published = revision.clone();
        // Publication has succeeded even if a held-open old file cannot yet be removed (Windows). Retry it on the
        // next publication, without reporting a successful new map as failed.
        let _ = tokio::task::spawn_blocking(move || prune(&pointer, &published, previous.as_deref())).await;
        Ok(revision)
    }

    pub async fn resolve(&self, ctx: &Ctx, sound_id: &str, revision: &str, signal: &Signal) -> Result<PathBuf, OpError> {
        let current = self.ensure(ctx, signal).await?;
        if current.revision != revision {
            return Err(stale());
        }
        let path = self.validate(ctx, &current, sound_id, signal).await?;
        self.still_current(&current)?;
        Ok(path)
    }

    /// Staging pins the catalog's recorded identity through the actual open/copy, not merely this preflight check.
    pub(crate) async fn stage(
        &self,
        ctx: &Ctx,
        sound_id: &str,
        revision: &str,
        signal: &Signal,
    ) -> Result<super::artifacts::StagedArtifact, OpError> {
        let current = self.ensure(ctx, signal).await?;
        if current.revision != revision {
            return Err(stale());
        }
        let path = self.validate(ctx, &current, sound_id, signal).await?;
        let identity = current.files.get(sound_id).ok_or_else(stale)?.identity.value();
        let staged = ctx.artifacts().stage(ctx, &path, Some(&identity), signal).await?;
        self.still_current(&current)?;
        Ok(staged)
    }

    fn still_current(&self, current: &Current) -> Result<(), OpError> {
        if self.current.borrow().as_ref().is_some_and(|now| now.revision == current.revision) {
            Ok(())
        } else {
            Err(stale())
        }
    }

    async fn validate(&self, ctx: &Ctx, current: &Current, id: &str, signal: &Signal) -> Result<PathBuf, OpError> {
        let file = current.files.get(id).ok_or_else(stale)?.clone();
        let roots = ctx.library_roots();
        let checked = tokio::select! {
            _ = signal.cancelled() => return Err(cancelled()),
            checked = tokio::task::spawn_blocking(move || {
                let now = checked_file(&file.indexed, &roots)?;
                if now.canonical != file.canonical || now.identity != file.identity { return Err(stale()); }
                Ok(now.canonical)
            }) => checked.map_err(failed)??,
        };
        // The worker performed no-follow canonical validation. Recheck only the live grant here, with no second
        // synchronous filesystem traversal (which could resolve a replacement differently).
        if !ctx.library_roots().iter().any(|root| checked.starts_with(root)) {
            return Err(stale());
        }
        check(signal)?;
        Ok(checked)
    }

    async fn search(&self, ctx: &Ctx, args: Args, signal: &Signal) -> Result<Value, OpError> {
        let query = args.search()?;
        let current = self.ensure(ctx, signal).await?;
        let near = if let Some(id) = args.near.as_deref() {
            self.validate(ctx, &current, id, signal).await?;
            Some(current.files.get(id).ok_or_else(stale)?.indexed.to_string_lossy().into_owned())
        } else {
            None
        };
        let (reply, receive) = oneshot::channel();
        let request = Request { query, near, reply };
        tokio::select! {
            _ = signal.cancelled() => return Err(cancelled()),
            sent = current.search.send(request) => sent.map_err(|_| failed("sound search worker stopped"))?,
        }
        let paths = tokio::select! {
            _ = signal.cancelled() => return Err(cancelled()),
            result = receive => result.map_err(|_| failed("sound search worker stopped"))??,
        };
        let candidates: Vec<_> = paths
            .into_iter()
            .filter_map(|path| {
                let id = current.ids.get(&path)?;
                Some((id.clone(), current.files.get(id)?.clone()))
            })
            .collect();
        let roots = ctx.library_roots();
        let checking = signal.clone();
        let valid = tokio::select! {
            _ = signal.cancelled() => return Err(cancelled()),
            valid = tokio::task::spawn_blocking(move || {
                let mut valid = Vec::with_capacity(candidates.len());
                for (id, file) in candidates {
                    check(&checking)?;
                    // Deleted, replaced or no-longer-approved hits are omitted, never reinterpreted by name.
                    if checked_file(&file.indexed, &roots)
                        .is_ok_and(|now| now.canonical == file.canonical && now.identity == file.identity)
                    {
                        valid.push(id);
                    }
                }
                Ok::<_, OpError>(valid)
            }) => valid.map_err(failed)??,
        };
        let roots = ctx.library_roots();
        let items: Vec<_> = valid
            .into_iter()
            .filter(|id| roots.iter().any(|root| current.files[id].canonical.starts_with(root)))
            .map(|id| current.sounds[&id].public(&id))
            .collect();
        self.still_current(&current)?;
        Ok(json!({"map_revision":current.revision,"items":items}))
    }
}

fn prune(pointer: &Path, current: &str, previous: Option<&str>) {
    let Some(dir) = pointer.parent() else { return };
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name();
        let Some(revision) = name.to_str().and_then(|name| name.strip_suffix(".json")) else { continue };
        if revision == current
            || Some(revision) == previous
            || revision.len() != 33
            || !revision.starts_with('r')
            || !revision[1..].bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            continue;
        }
        // Never remove a published revision, even if a pointer has changed unexpectedly outside this store.
        let Ok(published) = std::fs::read_to_string(pointer) else { return };
        if published.trim() != current {
            return;
        }
        let _ = std::fs::remove_file(entry.path());
    }
}

fn checked_file(path: &Path, roots: &[PathBuf]) -> Result<File, OpError> {
    // Share the restricted learner's pre-filesystem namespace and no-follow checks. In particular, canonical
    // Windows VerbatimDisk paths are local; a junction must not trigger UNC authentication during validation.
    let canonical = path.to_str().and_then(kumi_library::sources::canonical_local_path).ok_or_else(stale)?;
    if !roots.iter().any(|root| canonical.starts_with(root)) {
        return Err(stale());
    }
    let identity = FileIdentity::at(&canonical).map_err(|_| stale())?;
    Ok(File { indexed: path.to_path_buf(), canonical, identity })
}

async fn worker(entries: Vec<SoundEntry>, sources: Vec<Source>) -> Result<mpsc::Sender<Request>, OpError> {
    let (send, mut receive) = mpsc::channel::<Request>(8);
    let (ready, started) = oneshot::channel();
    std::thread::Builder::new()
        .name("kumi-sound-search".into())
        .spawn(move || {
            let index = SoundIndex::of_every_source(entries, &sources);
            let _ = ready.send(());
            while let Some(mut request) = receive.blocking_recv() {
                if request.reply.is_closed() {
                    continue;
                }
                let result = (|| {
                    if let Some(path) = request.near {
                        let entry = index.vector_of(&path, None).ok_or_else(stale)?;
                        let vector = entry.vector.as_deref().map(unpack_vector).unwrap_or_default();
                        if vector.len() != VECTOR_LENGTH || vector.iter().any(|v| !v.is_finite()) {
                            return Err(OpError::Refused {
                                code: "unmeasured",
                                reason: "that sound has no learned similarity features".into(),
                                owner: None,
                            });
                        }
                        request.query.like = Some(LikeSound {
                            vector: vector.into_iter().map(f64::from).collect(),
                            name: String::new(),
                            path: Some(path),
                            brightness: entry.brightness.unwrap_or_default(),
                            attack: entry.attack.unwrap_or_default(),
                            seconds: entry.seconds.unwrap_or_default(),
                        });
                    }
                    Ok(index.search(&request.query).hits.into_iter().map(|hit| hit.entry.path.clone()).collect())
                })();
                let _ = request.reply.send(result);
            }
        })
        .map_err(failed)?;
    started.await.map_err(|_| failed("sound search worker stopped"))?;
    Ok(send)
}

fn stale() -> OpError {
    OpError::Refused {
        code: "stale",
        reason: "that sound is not available in the current map and approved library; search again".into(),
        owner: None,
    }
}
fn cancelled() -> OpError {
    OpError::Refused { code: "cancelled", reason: "sound map request cancelled".into(), owner: None }
}
fn check(signal: &Signal) -> Result<(), OpError> {
    if signal.is_cancelled() {
        Err(cancelled())
    } else {
        Ok(())
    }
}
fn failed(error: impl std::fmt::Display) -> OpError {
    OpError::Failed { reason: error.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kumi_library::{
        map::SOUNDS_MAP_VERSION,
        sources::SourceKind,
        store::{pack_vector, Entry},
    };

    struct Fixture {
        _dir: tempfile::TempDir,
        ctx: Rc<Ctx>,
        root: PathBuf,
        sources: Vec<Source>,
        index: SoundIndex,
        layout: SoundsMap,
    }

    fn context(state: PathBuf, roots: Vec<PathBuf>) -> Rc<Ctx> {
        Ctx::new(
            super::super::Args {
                kumi_home: state.join("home"),
                state_dir: state,
                daw: kumi_types::daw::DawId::Ableton,
                library_roots: roots,
                media_roots: vec![],
                tools: BTreeMap::new(),
            },
            Default::default(),
            Default::default(),
        )
        .unwrap()
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path().join("Samples");
            std::fs::create_dir(&root).unwrap();
            let root = root.canonicalize().unwrap();
            let sources = vec![Source { path: root.to_string_lossy().into_owned(), label: "Samples".into(), kind: SourceKind::Folder }];
            let mut entries = Vec::new();
            let mut points = Vec::new();
            for (at, (name, number, class, kind)) in [
                ("Kick seed.wav", Some(0.0), SoundClass::Kick, SoundKind::OneShot),
                ("Kick close.wav", Some(0.1), SoundClass::Kick, SoundKind::OneShot),
                ("Pad far.wav", Some(10.0), SoundClass::Pad, SoundKind::Loop),
                ("Kick unknown.wav", None, SoundClass::Kick, SoundKind::OneShot),
            ]
            .into_iter()
            .enumerate()
            {
                let path = root.join(name);
                std::fs::write(&path, b"learned fixture").unwrap();
                let path = path.to_string_lossy().into_owned();
                entries.push(SoundEntry {
                    file: Entry { path: path.clone(), size: 15, mtime: 1, gone: None },
                    vector: number.map(|number| pack_vector(&[number; VECTOR_LENGTH])),
                    kind: Some(kind),
                    class: Some(class),
                    seconds: number.map(|_| 1.25),
                    ..Default::default()
                });
                // Deliberately unrelated to feature distance: near must not use screen distance.
                if number.is_some() {
                    points.push((path, [0.0, 0.9, 0.01, 0.5][at], 0.5));
                }
            }
            let index = SoundIndex::new(entries, &sources);
            let layout = SoundsMap { version: SOUNDS_MAP_VERSION, made: 1234, shape: 2, points };
            let ctx = context(dir.path().join("state"), vec![root.clone()]);
            Self { _dir: dir, ctx, root, sources, index, layout }
        }

        async fn publish(&self) -> String {
            self.ctx.maps().project(&self.ctx, Some(self.layout.clone()), &self.index, self.sources.clone(), &Signal::new()).await.unwrap()
        }

        async fn search(&self, query: &str, near: Option<String>, kind: Option<&str>) -> Result<Value, OpError> {
            self.ctx
                .maps()
                .search(&self.ctx, Args { query: query.into(), near, kind: kind.map(str::to_owned), limit: 20 }, &Signal::new())
                .await
        }

        fn snapshot(&self, revision: &str) -> Value {
            serde_json::from_slice(&std::fs::read(self.ctx.paths().state_dir.join(format!("map/{revision}.json"))).unwrap()).unwrap()
        }

        fn id(&self, name: &str) -> String {
            let current = self.ctx.maps().current.borrow();
            current.as_ref().unwrap().sounds.iter().find(|(_, sound)| sound.name == name).unwrap().0.clone()
        }
    }

    #[tokio::test]
    async fn search_dtos_and_map_metadata_correspond_without_public_paths() {
        let fixture = Fixture::new();
        let revision = fixture.publish().await;
        let result = fixture.search("", None, None).await.unwrap();
        let snapshot = fixture.snapshot(&revision);
        assert_eq!(result["map_revision"], revision);
        assert_eq!(snapshot["revision"], revision);
        assert_eq!(snapshot["made"], 1234);
        assert_eq!(snapshot["schema"], super::super::snapshots::SCHEMA);
        assert!(snapshot["seq"].is_u64());
        assert!(snapshot["owner"]["pid"].is_u64());
        assert_eq!(std::fs::read_to_string(fixture.ctx.paths().state_dir.join("map/current")).unwrap(), format!("{revision}\n"));
        assert_eq!(snapshot["points"].as_array().unwrap().len(), 3);
        for item in result["items"].as_array().unwrap() {
            assert_eq!(item.as_object().unwrap().len(), 6);
            assert!(item.get("path").is_none());
            let id = item["sound_id"].as_str().unwrap();
            assert!(id.contains(&revision));
            let sound = &snapshot["sounds"][id];
            if let Some(point) = snapshot["points"].as_array().unwrap().iter().find(|point| point[0] == id) {
                let group = &snapshot["groups"][point[3].as_u64().unwrap().to_string()];
                let members = snapshot["points"].as_array().unwrap().iter().filter(|member| member[3] == point[3]).count();
                assert_eq!(group["count"].as_u64().unwrap() as usize, members);
            }
            let resolved = fixture.ctx.maps().resolve(&fixture.ctx, id, &revision, &Signal::new()).await.unwrap();
            assert_eq!(sound["path"], resolved.to_string_lossy().as_ref());
            assert!(resolved.starts_with(&fixture.root));
        }
        assert!(!serde_json::to_string(&result).unwrap().contains(fixture.root.to_str().unwrap()));
        let unknown = fixture.id("Kick unknown");
        assert!(snapshot["sounds"][unknown]["duration"].is_null());
        assert_eq!(fixture.search("", None, None).await.unwrap()["map_revision"], revision);
    }

    #[tokio::test]
    async fn explicit_refresh_invalidates_ids_even_for_the_same_file() {
        let fixture = Fixture::new();
        let old = fixture.publish().await;
        let id = fixture.id("Kick seed");
        let new = fixture.publish().await;
        assert_ne!(old, new);
        for revision in [&old, &new] {
            assert_eq!(fixture.ctx.maps().resolve(&fixture.ctx, &id, revision, &Signal::new()).await.unwrap_err().code(), "stale");
        }
        assert_eq!(fixture.search("", Some(id), None).await.unwrap_err().code(), "stale");
    }

    #[tokio::test]
    async fn similarity_uses_learned_features_excludes_self_and_keeps_kind_filters() {
        let fixture = Fixture::new();
        fixture.publish().await;
        let id = fixture.id("Kick seed");
        let result = fixture.search("", Some(id.clone()), None).await.unwrap();
        let names: Vec<_> = result["items"].as_array().unwrap().iter().map(|item| item["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["Kick close", "Pad far"]);
        let pad = fixture.search("", Some(id), Some("loop")).await.unwrap();
        assert_eq!(pad["items"].as_array().unwrap().len(), 1);
        assert_eq!(pad["items"][0]["name"], "Pad far");
        let kicks = fixture.search("", None, Some("kick")).await.unwrap();
        assert_eq!(kicks["items"].as_array().unwrap().len(), 3);
        assert!(kicks["items"].as_array().unwrap().iter().all(|item| item["class"] == "kick"));
        assert_eq!(fixture.search("", Some(fixture.id("Kick unknown")), None).await.unwrap_err().code(), "unmeasured");
        assert_eq!(fixture.search("", None, Some("unsupported-kind")).await.unwrap_err().code(), "invalid");
    }

    #[tokio::test]
    async fn word_results_keep_existing_search_order() {
        let fixture = Fixture::new();
        fixture.publish().await;
        let args = Args { query: "kick".into(), near: None, kind: None, limit: 20 };
        let expected: Vec<_> = fixture.index.search(&args.search().unwrap()).hits.into_iter().map(|hit| hit.name).collect();
        let actual = fixture.search("kick", None, None).await.unwrap();
        let names: Vec<_> = actual["items"].as_array().unwrap().iter().map(|item| item["name"].as_str().unwrap()).collect();
        assert_eq!(names, expected);
    }

    #[tokio::test]
    async fn current_grants_files_and_file_identity_are_required() {
        let fixture = Fixture::new();
        let revision = fixture.publish().await;
        let id = fixture.id("Kick seed");
        let revoked = context(fixture._dir.path().join("revoked"), vec![]);
        assert_eq!(fixture.ctx.maps().resolve(&revoked, &id, &revision, &Signal::new()).await.unwrap_err().code(), "stale");
        let original = fixture.root.join("Kick seed.wav");
        std::fs::rename(&original, fixture.root.join("old.wav")).unwrap();
        std::fs::write(&original, b"different file").unwrap();
        assert_eq!(fixture.ctx.maps().resolve(&fixture.ctx, &id, &revision, &Signal::new()).await.unwrap_err().code(), "stale");
        let result = fixture.search("seed", None, None).await.unwrap();
        assert!(result["items"].as_array().unwrap().is_empty());
        assert!(checked_file(fixture._dir.path(), &[fixture.root.clone()]).is_err());
        assert!(checked_file(&fixture.root.join("../elsewhere.wav"), &[fixture.root.clone()]).is_err());
        assert!(checked_file(&fixture.root, &[fixture.root.clone()]).is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlink_escape_and_retargeted_paths_never_resolve() {
        use std::os::unix::fs::symlink;
        let fixture = Fixture::new();
        let revision = fixture.publish().await;
        let id = fixture.id("Kick seed");
        let original = fixture.root.join("Kick seed.wav");
        let outside = fixture._dir.path().join("outside.wav");
        std::fs::write(&outside, b"outside").unwrap();
        std::fs::remove_file(&original).unwrap();
        symlink(&outside, &original).unwrap();
        assert!(checked_file(&original, &[fixture.root.clone()]).is_err());
        assert_eq!(fixture.ctx.maps().resolve(&fixture.ctx, &id, &revision, &Signal::new()).await.unwrap_err().code(), "stale");
        // Even a retarget to an approved file must not reinterpret the old ID.
        std::fs::remove_file(&original).unwrap();
        symlink(fixture.root.join("Pad far.wav"), &original).unwrap();
        assert_eq!(fixture.ctx.maps().resolve(&fixture.ctx, &id, &revision, &Signal::new()).await.unwrap_err().code(), "stale");
        let refreshed = fixture.publish().await;
        let snapshot = fixture.snapshot(&refreshed);
        let names: HashSet<_> = snapshot["sounds"].as_object().unwrap().values().map(|sound| sound["name"].as_str().unwrap()).collect();
        assert!(!names.contains("Kick seed"));
        assert!(names.contains("Pad far"), "an alias must not displace the real approved file");
    }

    #[tokio::test]
    async fn unlaid_out_catalog_remains_searchable_and_revoked_roots_are_not_advertised() {
        let fixture = Fixture::new();
        fixture.publish().await;
        let store = MapStore::new(&fixture.ctx.paths().state_dir).unwrap();
        assert!(!fixture.ctx.paths().state_dir.join("map/current").exists());
        assert!(store.current.borrow().is_none());
        let revision = store.project(&fixture.ctx, None, &fixture.index, fixture.sources.clone(), &Signal::new()).await.unwrap();
        let snapshot = fixture.snapshot(&revision);
        assert!(snapshot["made"].is_null());
        assert!(snapshot["points"].as_array().unwrap().is_empty());
        assert_eq!(snapshot["sounds"].as_object().unwrap().len(), 4);
        let result =
            store.search(&fixture.ctx, Args { query: "".into(), near: None, kind: None, limit: 20 }, &Signal::new()).await.unwrap();
        assert_eq!(result["items"].as_array().unwrap().len(), 4);
        let unmeasured =
            store.search(&fixture.ctx, Args { query: "unknown".into(), near: None, kind: None, limit: 20 }, &Signal::new()).await.unwrap();
        assert_eq!(unmeasured["items"].as_array().unwrap().len(), 1);
        let revoked = context(fixture._dir.path().join("revoked"), vec![]);
        revoked.maps().project(&revoked, None, &fixture.index, fixture.sources.clone(), &Signal::new()).await.unwrap();
        let result =
            revoked.maps().search(&revoked, Args { query: "".into(), near: None, kind: None, limit: 20 }, &Signal::new()).await.unwrap();
        assert!(result["items"].as_array().unwrap().is_empty());
        let signal = Signal::new();
        signal.cancel();
        assert_eq!(store.resolve(&fixture.ctx, "anything", &revision, &signal).await.unwrap_err().code(), "cancelled");
    }

    #[tokio::test]
    async fn delayed_startup_keeps_ids_already_published_for_a_tool() {
        tokio::task::LocalSet::new()
            .run_until(async {
                let fixture = Fixture::new();
                let revision = fixture.publish().await;
                let id = fixture.id("Kick seed");
                fixture.ctx.maps().initialize(&fixture.ctx, &Signal::new()).await.unwrap();
                assert_eq!(fixture.ctx.maps().revision().as_deref(), Some(revision.as_str()));
                let resolved = fixture.ctx.maps().resolve(&fixture.ctx, &id, &revision, &Signal::new()).await.unwrap();
                assert_eq!(resolved, fixture.root.join("Kick seed.wav"));
            })
            .await;
    }

    #[tokio::test]
    async fn durable_publication_keeps_only_current_and_previous_revisions() {
        let fixture = Fixture::new();
        let first = fixture.publish().await;
        let second = fixture.publish().await;
        let third = fixture.publish().await;
        let dir = fixture.ctx.paths().state_dir.join("map");
        assert!(!dir.join(format!("{first}.json")).exists());
        assert!(dir.join(format!("{second}.json")).exists());
        assert!(dir.join(format!("{third}.json")).exists());
        assert_eq!(std::fs::read_to_string(dir.join("current")).unwrap(), format!("{third}\n"));
        // Even an outdated pruner may not delete the revision a later publication now advertises.
        prune(&dir.join("current"), &first, None);
        assert!(dir.join(format!("{third}.json")).exists());
    }

    #[tokio::test]
    async fn oversized_publication_keeps_the_previous_pointer_catalog_and_ids() {
        let fixture = Fixture::new();
        let revision = fixture.publish().await;
        let id = fixture.id("Kick seed");
        let current = fixture.ctx.maps().current.borrow().clone().unwrap();
        let next = format!("r{:032x}", 7);
        let candidate = Current {
            revision: next.clone(),
            sounds: current.sounds.clone(),
            files: current.files.clone(),
            ids: current.ids.clone(),
            search: current.search.clone(),
        };
        // A small bound exercises exactly the production publication path without allocating 64 MiB per test.
        let result = fixture
            .ctx
            .maps()
            .publish(&fixture.ctx, candidate, json!({"revision":next, "sounds":"x".repeat(1024)}), 512, &Signal::new())
            .await;
        assert_eq!(result.unwrap_err().code(), "map_too_large");
        assert_eq!(fixture.ctx.maps().revision().as_deref(), Some(revision.as_str()));
        assert_eq!(std::fs::read_to_string(fixture.ctx.paths().state_dir.join("map/current")).unwrap(), format!("{revision}\n"));
        assert!(!fixture.ctx.paths().state_dir.join(format!("map/{next}.json")).exists());
        assert_eq!(fixture.search("seed", None, None).await.unwrap()["items"][0]["sound_id"], id);
        assert_eq!(
            fixture.ctx.maps().resolve(&fixture.ctx, &id, &revision, &Signal::new()).await.unwrap(),
            fixture.root.join("Kick seed.wav")
        );
    }

    #[tokio::test]
    async fn staging_copies_the_recorded_file_and_refuses_stale_sources_and_revisions() {
        let fixture = Fixture::new();
        let revision = fixture.publish().await;
        let id = fixture.id("Kick seed");
        let source = fixture.root.join("Kick seed.wav");
        let staged = fixture.ctx.maps().stage(&fixture.ctx, &id, &revision, &Signal::new()).await.unwrap();
        let copy = staged.path().to_path_buf();
        assert_ne!(copy, source);
        assert_eq!(std::fs::read(&copy).unwrap(), std::fs::read(&source).unwrap());
        drop(staged);
        assert!(!copy.exists());
        assert_eq!(std::fs::read(&source).unwrap(), b"learned fixture");
        std::fs::rename(&source, fixture.root.join("preserved.wav")).unwrap();
        std::fs::write(&source, b"replacement").unwrap();
        assert_eq!(fixture.ctx.maps().stage(&fixture.ctx, &id, &revision, &Signal::new()).await.err().unwrap().code(), "stale");
        let held = fixture.ctx.maps().current.borrow().clone().unwrap();
        fixture.publish().await;
        assert_eq!(fixture.ctx.maps().still_current(&held).unwrap_err().code(), "stale");
        assert_eq!(fixture.ctx.maps().stage(&fixture.ctx, &id, &revision, &Signal::new()).await.err().unwrap().code(), "stale");
    }
}
