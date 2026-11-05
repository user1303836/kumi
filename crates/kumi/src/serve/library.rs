//! The plugin's library is an exclusive set of grants, not the standalone app's source discovery.
//! Reads load its saved index; only the approved `scan` action starts learning and layout.

use super::ctx::{Call, Ctx, Effect, JsonObject, OpError, OpFuture, Operation};
use kumi_common::abort::Signal;
use kumi_library::{
    create_library,
    learn::{LearnPhase, LearnProgress},
    sources::{canonical_local_path, SourceOptions},
    LearnNowOptions, Library, LibraryOptions,
};
use kumi_media::audio::DecodeOptions;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
};
use tokio::sync::Mutex;

pub static LIBRARY: Operation = Operation {
    name: "library",
    description: "Show Kumi's approved sample folders and learned sounds, add one explicitly approved local folder, start a scan job, or request cancellation of the current scan by job_id. Cancellation acknowledges the request; the job reports cancelled after settling. Never discovers other folders or downloads programs or models.",
    schema,
    effect,
    run,
};

fn schema() -> Value {
    json!({"type":"object","additionalProperties":false,"required":["action"],"properties":{
        "action":{"type":"string","enum":["status","add_folder","scan","cancel"]},
        "folder":{"type":"string","minLength":1,"maxLength":32768,"description":"Absolute local folder, required only for add_folder. Scan covers all currently approved folders."},
        "job_id":{"type":"string","minLength":4,"maxLength":128,"pattern":"^j[0-9]+-[0-9]+$","description":"The current scan's job_id, required only for cancel."}
    }})
}
fn effect(args: &JsonObject) -> Effect {
    if matches!(args.get("action").and_then(Value::as_str), Some("status" | "cancel")) {
        Effect::Read
    } else {
        Effect::FolderRead
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Action {
    Status,
    AddFolder,
    Scan,
    Cancel,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Args {
    action: Action,
    folder: Option<String>,
    job_id: Option<String>,
}

struct Cached {
    roots: Vec<PathBuf>,
    library: Rc<Library>,
}
#[derive(Default)]
pub struct LibraryState {
    cache: Mutex<Option<Cached>>,
    changing: Mutex<()>,
    scan_job: RefCell<Option<String>>,
    #[cfg(test)]
    progress_observer: RefCell<Option<Rc<dyn Fn(&LearnProgress)>>>,
}

/// Loading saved, already-approved measurements is a read. No `Library::start` means no implicit learner.
pub async fn get(ctx: &Ctx) -> Result<Rc<Library>, OpError> {
    let mut cached = ctx.library_state().cache.lock().await;
    let roots = ctx.library_roots();
    if let Some(held) = cached.as_ref().filter(|held| held.roots == roots) {
        return Ok(held.library.clone());
    }
    if let Some(previous) = cached.take() {
        previous.library.close().await;
    }
    let folders = roots.iter().map(|root| text(root).map(str::to_owned)).collect::<Result<Vec<_>, _>>()?;
    let library = create_library(LibraryOptions {
        dir: text(&ctx.paths().state_dir.join("library"))?.to_owned(),
        sources: Some(SourceOptions { approved_folders: Some(folders), ..Default::default() }),
        decode: DecodeOptions { installed_only: true, ffmpeg: ctx.media().tool("ffmpeg").map(Path::to_path_buf) },
        fork: Some(false),
        workers: Some(1),
        find_sets: Some(false),
        ..Default::default()
    });
    *cached = Some(Cached { roots, library: library.clone() });
    Ok(library)
}

/// Re-project the saved library under this run's grants, without scanning a folder or doing DSP.
pub fn start(ctx: Rc<Ctx>) {
    tokio::task::spawn_local(async move {
        let result = ctx.maps().initialize(&ctx, ctx.shutdown()).await;
        if let Err(error) = result {
            if !ctx.shutdown().is_cancelled() {
                super::say(&format!("kumi serve: saved map unavailable: {}", error.reason()));
            }
        }
    });
}

pub async fn status(ctx: &Ctx) -> Result<Value, OpError> {
    let library = get(ctx).await?;
    let index = library.sound_index().await.map_err(failed)?;
    let job = ctx.library_state().scan_job.borrow().clone();
    let learned_at = library.status().learned_at;
    Ok(json!({
        "state": if job.is_some() { "scanning" } else if learned_at.is_some() { "ready" } else { "not_scanned" },
        "folders":ctx.library_roots(), "sounds":index.entries.len(), "measured":index.compared().count(),
        "learned_at":learned_at, "job_id":job, "map_revision":ctx.maps().revision()
    }))
}

fn run(ctx: &Ctx, call: Call) -> OpFuture<'_> {
    Box::pin(async move {
        let result = async {
            let args: Args =
                serde_json::from_value(Value::Object(call.args)).map_err(|error| OpError::Invalid { reason: error.to_string() })?;
            check(&call.signal)?;
            if !matches!(args.action, Action::AddFolder) && args.folder.is_some() {
                return Err(OpError::Invalid {
                    reason: "folder is only for add_folder; scan reads all currently approved folders.".into(),
                });
            }
            if !matches!(args.action, Action::Cancel) && args.job_id.is_some() {
                return Err(OpError::Invalid { reason: "job_id is only for cancel.".into() });
            }
            match args.action {
                Action::Status => status(ctx).await,
                Action::Cancel => {
                    let id = args.job_id.filter(|id| scan_id(id)).ok_or_else(|| OpError::Invalid {
                        reason: "cancel needs a runtime scan job_id (j<digits>-<digits>, at most 128 bytes).".into(),
                    })?;
                    if ctx.library_state().scan_job.borrow().as_deref() != Some(id.as_str()) || !ctx.jobs().cancel(&id, "scan") {
                        return Err(OpError::Refused {
                            code: "not_running",
                            reason: "That job is not this runtime's current running library scan.".into(),
                            owner: None,
                        });
                    }
                    Ok(json!({"job_id":id,"cancel_requested":true}))
                }
                Action::AddFolder => {
                    let folder =
                        args.folder.ok_or_else(|| OpError::Invalid { reason: "add_folder needs an absolute local folder.".into() })?;
                    let _changing = tokio::select! {
                        guard = ctx.library_state().changing.lock() => guard,
                        _ = call.signal.cancelled() => return Err(cancelled()),
                    };
                    idle(ctx)?;
                    // This approved action can grant a new folder; it cannot follow a network namespace or a replaced link.
                    let folder = tokio::task::spawn_blocking(move || canonical_local_path(&folder).filter(|folder| folder.is_dir()))
                        .await
                        .map_err(failed)?
                        .ok_or_else(|| {
                            OpError::Invalid {
                        reason:
                            "Choose an existing local folder by its canonical absolute path, without symlinks or network/device namespaces."
                                .into(),
                    }
                        })?;
                    check(&call.signal)?;
                    // The checked path is the grant. Resolving it again here could silently grant a replacement link.
                    ctx.add_checked_library_root(folder)?;
                    let library = get(ctx).await?;
                    ctx.maps().refresh(ctx, &library, &call.signal).await?;
                    status(ctx).await
                }
                Action::Scan => {
                    let _changing = tokio::select! {
                        guard = ctx.library_state().changing.lock() => guard,
                        _ = call.signal.cancelled() => return Err(cancelled()),
                    };
                    idle(ctx)?;
                    if ctx.library_roots().is_empty() {
                        return Err(OpError::Refused {
                            code: "no_folders",
                            reason: "Approve a local sample folder with add_folder before scanning.".into(),
                            owner: None,
                        });
                    }
                    check(&call.signal)?;
                    let id = ctx.start_job("scan", |ctx, job| async move {
                        let _scanning = ScanGuard(ctx.clone());
                        let job = Rc::new(job);
                        let progress_job = job.clone();
                        #[cfg(test)]
                        let observer = ctx.library_state().progress_observer.borrow().clone();
                        let label = folder_label(&ctx);
                        let scanning = format!("Scanning {label}");
                        let measuring = format!("Measuring {label}");
                        let saving = format!("Saving sounds from {label}");
                        let result = async {
                            let library = get(&ctx).await?;
                            let progress = library
                                .learn_now(LearnNowOptions {
                                    rebuild: false,
                                    signal: job.signal.clone(),
                                    on_progress: Some(Rc::new(move |progress: LearnProgress| {
                                        let (done, total, message) = match progress.phase {
                                            LearnPhase::Sounds => {
                                                (progress.sounds.done as f64, Some(progress.sounds.todo as f64), measuring.as_str())
                                            }
                                            LearnPhase::Tidying | LearnPhase::Done => {
                                                (progress.sounds.done as f64, Some(progress.sounds.todo as f64), saving.as_str())
                                            }
                                            LearnPhase::Looking => (
                                                (progress.sounds.todo + progress.presets.todo + progress.sets.todo) as f64,
                                                None,
                                                scanning.as_str(),
                                            ),
                                            LearnPhase::Presets => {
                                                (progress.presets.done as f64, Some(progress.presets.todo as f64), scanning.as_str())
                                            }
                                            LearnPhase::Sets => {
                                                (progress.sets.done as f64, Some(progress.sets.todo as f64), scanning.as_str())
                                            }
                                        };
                                        progress_job.report(done, total, Some(message));
                                        #[cfg(test)]
                                        if let Some(observer) = &observer {
                                            observer(&progress);
                                        }
                                    })),
                                })
                                .await
                                .map_err(failed)?
                                .ok_or_else(|| OpError::Refused {
                                    code: "busy",
                                    reason: "Another scan holds this library's lock.".into(),
                                    owner: None,
                                })?;
                            check(&job.signal)?;
                            job.report(progress.sounds.done as f64, None, Some(&format!("Laying out sounds from {label}")));
                            let signal = job.signal.clone();
                            kumi_library::sounds_map::lay_out_sounds(
                                &ctx.paths().state_dir.join("library"),
                                &library.sources().await,
                                &Default::default(),
                                Arc::new(move || !signal.is_cancelled()),
                            )
                            .await
                            .map_err(failed)?;
                            check(&job.signal)?;
                            let revision = ctx.maps().refresh(&ctx, &library, &job.signal).await?;
                            let index = library.sound_index().await.map_err(failed)?;
                            Ok(json!({"map_revision":revision, "sounds":index.entries.len(),
                        "measured":index.compared().count(), "failed":progress.failed}))
                        }
                        .await;
                        result.map_err(|error| display_error(&ctx, error))
                    })?;
                    *ctx.library_state().scan_job.borrow_mut() = Some(id.clone());
                    Ok(json!({"job_id":id}))
                }
            }
        }
        .await;
        result.map_err(|error| display_error(ctx, error))
    })
}

fn scan_id(id: &str) -> bool {
    if id.len() > 128 {
        return false;
    }
    let Some((at, serial)) = id.strip_prefix('j').and_then(|id| id.split_once('-')) else { return false };
    !at.is_empty() && !serial.is_empty() && at.bytes().all(|byte| byte.is_ascii_digit()) && serial.bytes().all(|byte| byte.is_ascii_digit())
}

struct ScanGuard(Rc<Ctx>);
impl Drop for ScanGuard {
    fn drop(&mut self) {
        self.0.library_state().scan_job.borrow_mut().take();
    }
}
fn idle(ctx: &Ctx) -> Result<(), OpError> {
    if ctx.library_state().scan_job.borrow().is_some() {
        return Err(OpError::Refused {
            code: "busy",
            reason: "A library scan is running; wait for that job before changing folders or scanning again.".into(),
            owner: None,
        });
    }
    Ok(())
}
fn text(path: &Path) -> Result<&str, OpError> {
    path.to_str().ok_or_else(|| OpError::Invalid { reason: "This path is not valid UTF-8.".into() })
}
fn check(signal: &Signal) -> Result<(), OpError> {
    if signal.is_cancelled() {
        Err(cancelled())
    } else {
        Ok(())
    }
}
fn cancelled() -> OpError {
    OpError::Refused { code: "cancelled", reason: "The library operation was cancelled.".into(), owner: None }
}
fn failed(error: impl std::fmt::Display) -> OpError {
    OpError::Failed { reason: format!("Could not read the approved library: {error}") }
}

fn folder_label(ctx: &Ctx) -> String {
    let roots = ctx.library_roots();
    let mut label = roots.iter().take(3).map(|root| folder_name(root)).collect::<Vec<_>>().join(", ");
    if roots.len() > 3 {
        label.push_str(&format!(" and {} more folders", roots.len() - 3));
    }
    label
}

fn folder_name(path: &Path) -> std::borrow::Cow<'_, str> {
    path.file_name().map(|name| name.to_string_lossy()).unwrap_or_else(|| "approved folder".into())
}

fn display_error(ctx: &Ctx, mut error: OpError) -> OpError {
    let reason = match &mut error {
        OpError::Invalid { reason } | OpError::NotConnected { reason } | OpError::Refused { reason, .. } | OpError::Failed { reason } => {
            reason
        }
    };
    for root in ctx.library_roots() {
        redact_path(reason, &root, &folder_name(&root));
    }
    redact_path(reason, &ctx.paths().state_dir, "library state");
    error
}

fn redact_path(reason: &mut String, path: &Path, label: &str) {
    let path = path.to_string_lossy();
    for named in [Some(path.as_ref()), path.strip_prefix(r"\\?\")].into_iter().flatten() {
        if reason.contains(named) {
            *reason = reason.replace(named, label);
        }
        if named.contains('\\') {
            for spelling in [named.replace('\\', "/"), named.replace('\\', "\\\\")] {
                if reason.contains(&spelling) {
                    *reason = reason.replace(&spelling, label);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kumi_types::{daw::DawId, system::Env};
    use std::collections::BTreeMap;

    fn context(dir: &Path, roots: Vec<PathBuf>) -> Rc<Ctx> {
        context_with_tools(dir, roots, BTreeMap::new())
    }

    fn context_with_tools(dir: &Path, roots: Vec<PathBuf>, tools: BTreeMap<String, PathBuf>) -> Rc<Ctx> {
        Ctx::new(
            super::super::Args {
                state_dir: dir.join("state"),
                kumi_home: dir.join("home"),
                hermes_home: None,
                hermes_root: None,
                daw: DawId::Reaper,
                library_roots: roots,
                media_roots: Vec::new(),
                tools,
            },
            Env::new(),
            Default::default(),
        )
        .unwrap()
    }

    fn call(args: Value) -> Call {
        Call {
            args: args.as_object().unwrap().clone(),
            host: Default::default(),
            signal: Signal::new(),
            progress: super::super::ctx::Progress::none(),
        }
    }

    async fn scan(ctx: &Ctx) -> Value {
        let started = (LIBRARY.run)(ctx, call(json!({"action":"scan"}))).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(120), ctx.jobs().all_settled()).await.unwrap();
        let job = ctx.jobs().get(started["job_id"].as_str().unwrap(), &ctx.paths().state_dir).unwrap();
        assert_eq!(job["state"], "done", "{job}");
        job["result"].clone()
    }

    fn wav(path: &Path, hz: f64) {
        let frames = 4096u32;
        let rate = 8000u32;
        let size = frames * 2;
        let mut bytes = Vec::with_capacity(size as usize + 44);
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + size).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&rate.to_le_bytes());
        bytes.extend_from_slice(&(rate * 2).to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&size.to_le_bytes());
        for frame in 0..frames {
            let sample = ((frame as f64 * hz * std::f64::consts::TAU / rate as f64).sin() * 12000.) as i16;
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn operation_effects_require_folder_approval_but_never_a_daw_write_lease() {
        for (action, expected) in
            [("status", Effect::Read), ("cancel", Effect::Read), ("add_folder", Effect::FolderRead), ("scan", Effect::FolderRead)]
        {
            let args = json!({"action":action}).as_object().unwrap().clone();
            let effect = (LIBRARY.effect)(&args);
            assert_eq!(effect, expected);
            assert!(!effect.writes());
        }
    }

    #[tokio::test]
    async fn invalid_folder_grants_and_missing_grants_never_start_scanning() {
        tokio::task::LocalSet::new()
            .run_until(async {
                let dir = tempfile::tempdir().unwrap();
                let ctx = context(dir.path(), vec![]);
                let file = dir.path().join("not-a-folder.wav");
                std::fs::write(&file, b"original").unwrap();
                for folder in [
                    "relative".to_owned(),
                    "//server/share".to_owned(),
                    dir.path().join("missing").to_string_lossy().into_owned(),
                    file.to_string_lossy().into_owned(),
                ] {
                    let error = (LIBRARY.run)(&ctx, call(json!({"action":"add_folder","folder":folder}))).await.unwrap_err();
                    assert_eq!(error.code(), "invalid");
                    assert!(!error.reason().contains(dir.path().to_str().unwrap()));
                }
                for args in [
                    json!({"action":"add_folder"}),
                    json!({"action":"scan","folder":dir.path()}),
                    json!({"action":"status","folder":dir.path()}),
                ] {
                    assert_eq!((LIBRARY.run)(&ctx, call(args)).await.unwrap_err().code(), "invalid");
                }
                assert_eq!((LIBRARY.run)(&ctx, call(json!({"action":"scan"}))).await.unwrap_err().code(), "no_folders");
                assert!(ctx.library_roots().is_empty());
                assert_eq!(ctx.jobs().running(), 0);
                assert_eq!(std::fs::read(file).unwrap(), b"original");
            })
            .await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlink_folder_grants_are_refused_without_changing_roots() {
        tokio::task::LocalSet::new()
            .run_until(async {
                let dir = tempfile::tempdir().unwrap();
                let real = dir.path().join("Samples");
                std::fs::create_dir(&real).unwrap();
                let alias = dir.path().join("Alias");
                std::os::unix::fs::symlink(&real, &alias).unwrap();
                let ctx = context(dir.path(), vec![]);
                assert_eq!((LIBRARY.run)(&ctx, call(json!({"action":"add_folder","folder":alias}))).await.unwrap_err().code(), "invalid");
                assert!(ctx.library_roots().is_empty());
                assert!(real.is_dir());
            })
            .await;
    }

    #[tokio::test]
    async fn a_running_scan_refuses_another_scan_and_folder_changes() {
        tokio::task::LocalSet::new()
            .run_until(async {
                let dir = tempfile::tempdir().unwrap();
                let root = dir.path().join("Samples");
                let other = dir.path().join("Other");
                std::fs::create_dir(&root).unwrap();
                std::fs::create_dir(&other).unwrap();
                let ctx = context(dir.path(), vec![root.canonicalize().unwrap()]);
                let started = (LIBRARY.run)(&ctx, call(json!({"action":"scan"}))).await.unwrap();
                assert_eq!((LIBRARY.run)(&ctx, call(json!({"action":"cancel","job_id":"j0-0"}))).await.unwrap_err().code(), "not_running");
                for args in [json!({"action":"scan"}), json!({"action":"add_folder","folder":other})] {
                    assert_eq!((LIBRARY.run)(&ctx, call(args)).await.unwrap_err().code(), "busy");
                }
                tokio::time::timeout(std::time::Duration::from_secs(10), ctx.jobs().all_settled()).await.unwrap();
                let job = ctx.jobs().get(started["job_id"].as_str().unwrap(), &ctx.paths().state_dir).unwrap();
                assert_eq!(job["state"], "done", "{job}");
                assert_eq!(ctx.library_roots(), vec![root.canonicalize().unwrap()]);
                assert_eq!((LIBRARY.run)(&ctx, call(json!({"action":"status"}))).await.unwrap()["state"], "ready");
                get(&ctx).await.unwrap().close().await;
            })
            .await;
    }

    /// Uses the real __library-measure child: build the kumi binary beside the test executable first.
    #[tokio::test]
    async fn a_small_second_scan_is_searchable_without_redrawing_and_unmeasured_sounds_remain_visible() {
        tokio::task::LocalSet::new()
            .run_until(async {
                let dir = tempfile::tempdir().unwrap();
                let root = dir.path().join("Samples");
                std::fs::create_dir(&root).unwrap();
                let root = root.canonicalize().unwrap();
                for at in 0..20 {
                    wav(&root.join(format!("tone-{at:02}.wav")), 120. + at as f64 * 25.);
                }
                let broken = root.join("Brokenprobe.wav");
                std::fs::write(&broken, [0u8; 128]).unwrap();
                let original = std::fs::read(root.join("tone-00.wav")).unwrap();
                let ctx = context(dir.path(), vec![]);
                let added = (LIBRARY.run)(&ctx, call(json!({"action":"add_folder","folder":root}))).await.unwrap();
                assert_eq!(added["sounds"], 0, "granting a folder must not scan it");
                let first = scan(&ctx).await;
                assert_eq!(first["sounds"], 21);
                assert_eq!(first["measured"], 20);
                let library_dir = ctx.paths().state_dir.join("library");
                let layout = kumi_library::map::read_sounds_map(&library_dir).unwrap();
                assert_eq!(layout.points.len(), 20);
                let before = (super::super::map::SOUNDS.run)(&ctx, call(json!({"query":"tone-00"}))).await.unwrap();
                let old_id = before["items"][0]["sound_id"].as_str().unwrap();
                wav(&root.join("Freshaddition.wav"), 735.);
                let second = scan(&ctx).await;
                assert_eq!(second["sounds"], 22);
                assert_eq!(second["measured"], 21);
                assert_ne!(first["map_revision"], second["map_revision"]);
                assert_eq!(
                    kumi_library::map::read_sounds_map(&library_dir).unwrap(),
                    layout,
                    "one addition is below the two-sound threshold"
                );
                let revision = second["map_revision"].as_str().unwrap();
                let snapshot: Value =
                    serde_json::from_slice(&std::fs::read(ctx.paths().state_dir.join(format!("map/{revision}.json"))).unwrap()).unwrap();
                assert_eq!(snapshot["points"].as_array().unwrap().len(), 20);
                assert_eq!(snapshot["sounds"].as_object().unwrap().len(), 22);
                for query in ["Freshaddition", "Brokenprobe"] {
                    let found = (super::super::map::SOUNDS.run)(&ctx, call(json!({"query":query}))).await.unwrap();
                    assert_eq!(found["map_revision"], revision);
                    assert_eq!(found["items"].as_array().unwrap().len(), 1);
                    let id = found["items"][0]["sound_id"].as_str().unwrap();
                    assert!(!snapshot["points"].as_array().unwrap().iter().any(|point| point[0] == id));
                    assert_eq!(ctx.maps().resolve(&ctx, id, revision, &Signal::new()).await.unwrap(), root.join(format!("{query}.wav")));
                }
                assert_eq!(ctx.maps().resolve(&ctx, old_id, revision, &Signal::new()).await.unwrap_err().code(), "stale");
                assert_eq!(std::fs::read(root.join("tone-00.wav")).unwrap(), original);
                assert_eq!(std::fs::read(broken).unwrap(), [0u8; 128]);
                get(&ctx).await.unwrap().close().await;
            })
            .await;
    }

    #[tokio::test]
    async fn cancel_rejects_invalid_arguments_and_missing_or_settled_jobs() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(dir.path(), vec![]);
        for args in [
            json!({"action":"cancel"}),
            json!({"action":"cancel","job_id":""}),
            json!({"action":"cancel","job_id":"../current"}),
            json!({"action":"cancel","job_id":"j1-2-extra"}),
            json!({"action":"cancel","job_id":"j1-"}),
            json!({"action":"cancel","job_id":format!("j{}-1", "1".repeat(128))}),
            json!({"action":"cancel","job_id":"j1-1","folder":dir.path()}),
            json!({"action":"scan","job_id":"j1-1"}),
            json!({"action":"status","job_id":"j1-1"}),
            json!({"action":"add_folder","job_id":"j1-1","folder":dir.path()}),
        ] {
            assert_eq!((LIBRARY.run)(&ctx, call(args)).await.unwrap_err().code(), "invalid");
        }
        assert_eq!((LIBRARY.run)(&ctx, call(json!({"action":"cancel","job_id":"j1-1"}))).await.unwrap_err().code(), "not_running");
        assert_eq!(ctx.jobs().running(), 0);
    }

    fn map_files(ctx: &Ctx) -> BTreeMap<std::ffi::OsString, Vec<u8>> {
        std::fs::read_dir(ctx.paths().state_dir.join("map"))
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                (entry.file_name(), std::fs::read(entry.path()).unwrap())
            })
            .collect()
    }

    async fn request_cancel_while_running(ctx: &Ctx, id: &str) {
        assert_eq!((LIBRARY.run)(ctx, call(json!({"action":"cancel","job_id":"j0-0"}))).await.unwrap_err().code(), "not_running");
        assert_eq!(ctx.jobs().get(id, &ctx.paths().state_dir).unwrap()["state"], "running");
        for _ in 0..2 {
            assert_eq!(
                (LIBRARY.run)(ctx, call(json!({"action":"cancel","job_id":id}))).await.unwrap(),
                json!({"job_id":id,"cancel_requested":true})
            );
            assert_eq!(ctx.jobs().get(id, &ctx.paths().state_dir).unwrap()["state"], "running");
            assert_eq!((LIBRARY.run)(ctx, call(json!({"action":"scan"}))).await.unwrap_err().code(), "busy");
            assert_eq!(
                (LIBRARY.run)(ctx, call(json!({"action":"add_folder","folder":ctx.library_roots()[0]}))).await.unwrap_err().code(),
                "busy"
            );
        }
    }

    async fn settled_cancel(ctx: &Ctx, id: &str, revision: &str, previous: &BTreeMap<std::ffi::OsString, Vec<u8>>) {
        tokio::time::timeout(std::time::Duration::from_secs(10), ctx.jobs().all_settled()).await.unwrap();
        assert_eq!(ctx.jobs().get(id, &ctx.paths().state_dir).unwrap()["state"], "cancelled");
        assert_eq!(ctx.maps().revision().as_deref(), Some(revision));
        assert_eq!(&map_files(ctx), previous);
        assert_eq!((LIBRARY.run)(ctx, call(json!({"action":"cancel","job_id":id}))).await.unwrap_err().code(), "not_running");
    }

    #[tokio::test]
    async fn cancelling_during_the_walk_preserves_the_map_and_releases_busy_only_after_settlement() {
        use futures::FutureExt;
        tokio::task::LocalSet::new()
            .run_until(async {
                let dir = tempfile::tempdir().unwrap();
                let root = dir.path().join("Samples");
                std::fs::create_dir(&root).unwrap();
                let root = root.canonicalize().unwrap();
                wav(&root.join("Original.wav"), 220.);
                let ctx = context(dir.path(), vec![root.clone()]);
                let baseline = scan(&ctx).await;
                let previous = map_files(&ctx);
                wav(&root.join("Next.wav"), 330.);
                wav(&root.join("Later.wav"), 440.);
                let observed = Rc::new(std::cell::Cell::new(false));
                let reached = observed.clone();
                let weak = Rc::downgrade(&ctx);
                *ctx.library_state().progress_observer.borrow_mut() = Some(Rc::new(move |progress| {
                    if progress.phase == LearnPhase::Looking && progress.sounds.todo == 1 && !reached.replace(true) {
                        // Run at the first real directory discovery, before the next walk iteration can continue.
                        // Cancellation, idempotent acknowledgement and busy refusals must not need the worker to yield.
                        let ctx = weak.upgrade().unwrap();
                        let id = ctx.library_state().scan_job.borrow().clone().unwrap();
                        request_cancel_while_running(&ctx, &id).now_or_never().expect("cancellation must not wait for a scan");
                    }
                }));
                let started = (LIBRARY.run)(&ctx, call(json!({"action":"scan"}))).await.unwrap();
                settled_cancel(&ctx, started["job_id"].as_str().unwrap(), baseline["map_revision"].as_str().unwrap(), &previous).await;
                assert!(observed.get(), "the scan must actually reach the walk");
                ctx.library_state().progress_observer.borrow_mut().take();
                let next = scan(&ctx).await;
                assert_eq!(next["sounds"], 3);
                assert_ne!(next["map_revision"], baseline["map_revision"]);
                get(&ctx).await.unwrap().close().await;
            })
            .await;
    }

    /// The decoder handshake proves the real measurement child is working, not merely queued or entering its phase.
    #[cfg(unix)]
    #[tokio::test]
    async fn cancelling_active_measurement_stops_the_worker_and_decoder_before_rescanning() {
        use std::os::unix::fs::PermissionsExt;
        tokio::task::LocalSet::new()
            .run_until(async {
                let dir = tempfile::tempdir().unwrap();
                let base = dir.path().canonicalize().unwrap();
                let root = base.join("Samples");
                std::fs::create_dir(&root).unwrap();
                wav(&root.join("Original.wav"), 220.);
                let decoder = base.join("decoder");
                let marker = base.join("decoder-active");
                std::fs::write(
                    &decoder,
                    format!("#!/bin/sh\nprintf '%s %s\\n' \"$$\" \"$PPID\" > '{}'\nexec /bin/sleep 120\n", marker.display()),
                )
                .unwrap();
                std::fs::set_permissions(&decoder, std::fs::Permissions::from_mode(0o700)).unwrap();
                let ctx = context_with_tools(&base, vec![root.clone()], BTreeMap::from([("ffmpeg".into(), decoder.clone())]));
                let baseline = scan(&ctx).await;
                let previous = map_files(&ctx);
                let compressed = root.join("Pending.mp3");
                std::fs::write(&compressed, [0u8; 128]).unwrap();
                let started = (LIBRARY.run)(&ctx, call(json!({"action":"scan"}))).await.unwrap();
                let pids = tokio::time::timeout(std::time::Duration::from_secs(10), async {
                    loop {
                        if let Ok(text) = tokio::fs::read_to_string(&marker).await {
                            if let Ok(pids) = text.split_whitespace().map(str::parse::<libc::pid_t>).collect::<Result<Vec<_>, _>>() {
                                if pids.len() == 2 && pids.iter().all(|pid| *pid > 1) {
                                    break pids;
                                }
                            }
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                    }
                })
                .await
                .expect("the real measurement worker must start the approved decoder");
                assert!(pids.iter().all(|pid| unsafe { libc::kill(*pid, 0) } == 0));
                request_cancel_while_running(&ctx, started["job_id"].as_str().unwrap()).await;
                settled_cancel(&ctx, started["job_id"].as_str().unwrap(), baseline["map_revision"].as_str().unwrap(), &previous).await;
                // Worker::stop awaits the worker itself; its child can need a moment for its new parent to reap it.
                tokio::time::timeout(std::time::Duration::from_secs(5), async {
                    while pids.iter().any(|pid| unsafe { libc::kill(*pid, 0) } == 0) {
                        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                    }
                })
                .await
                .expect("cancellation must end both the worker and the active decoder");
                assert_eq!(std::fs::read(&compressed).unwrap(), [0u8; 128]);
                // The next scan may record this fixture's decode failure, but must not inherit a cancelled worker.
                std::fs::write(&decoder, "#!/bin/sh\nexit 1\n").unwrap();
                let next = scan(&ctx).await;
                assert_eq!(next["sounds"], 2);
                assert_ne!(next["map_revision"], baseline["map_revision"]);
                get(&ctx).await.unwrap().close().await;
            })
            .await;
    }

    #[tokio::test]
    async fn cancellation_after_pointer_commit_starts_finishes_done_without_rollback() {
        tokio::task::LocalSet::new()
            .run_until(async {
                let dir = tempfile::tempdir().unwrap();
                let root = dir.path().join("Samples");
                std::fs::create_dir(&root).unwrap();
                let root = root.canonicalize().unwrap();
                wav(&root.join("Original.wav"), 220.);
                let ctx = context(dir.path(), vec![root.clone()]);
                let baseline = scan(&ctx).await;
                let old_revision = baseline["map_revision"].as_str().unwrap();
                let map_dir = ctx.paths().state_dir.join("map");
                let old_snapshot = std::fs::read(map_dir.join(format!("{old_revision}.json"))).unwrap();
                let old_pointer = std::fs::read(map_dir.join("current")).unwrap();
                wav(&root.join("Committed.wav"), 330.);
                let (entered, release) = ctx.maps().gate_next_commit();
                let started = (LIBRARY.run)(&ctx, call(json!({"action":"scan"}))).await.unwrap();
                tokio::time::timeout(std::time::Duration::from_secs(10), entered)
                    .await
                    .expect("the scan must enter its pointer commit")
                    .expect("the commit worker must reach the gate");
                // The actual pointer worker has begun and cannot observe cancellation now. Hold it here so the
                // native cancel call is guaranteed to land after that boundary, not merely after a timer expires.
                request_cancel_while_running(&ctx, started["job_id"].as_str().unwrap()).await;
                assert_eq!(ctx.maps().revision().as_deref(), Some(old_revision));
                assert_eq!(std::fs::read(map_dir.join("current")).unwrap(), old_pointer);
                release.send(()).unwrap();
                tokio::time::timeout(std::time::Duration::from_secs(10), ctx.jobs().all_settled()).await.unwrap();
                let job = ctx.jobs().get(started["job_id"].as_str().unwrap(), &ctx.paths().state_dir).unwrap();
                assert_eq!(job["state"], "done", "{job}");
                let revision = job["result"]["map_revision"].as_str().unwrap();
                assert_ne!(revision, old_revision);
                assert_eq!(ctx.maps().revision().as_deref(), Some(revision));
                assert_eq!(std::fs::read_to_string(map_dir.join("current")).unwrap(), format!("{revision}\n"));
                let snapshot: Value = serde_json::from_slice(&std::fs::read(map_dir.join(format!("{revision}.json"))).unwrap()).unwrap();
                assert_eq!(snapshot["revision"], revision);
                assert_eq!(snapshot["sounds"].as_object().unwrap().len(), 2);
                assert_eq!(snapshot["points"].as_array().unwrap().len(), 2);
                assert_eq!(std::fs::read(map_dir.join(format!("{old_revision}.json"))).unwrap(), old_snapshot);
                let found = (super::super::map::SOUNDS.run)(&ctx, call(json!({"query":"Committed"}))).await.unwrap();
                assert_eq!(found["map_revision"], revision);
                assert_eq!(found["items"].as_array().unwrap().len(), 1);
                let id = found["items"][0]["sound_id"].as_str().unwrap();
                assert!(snapshot["points"].as_array().unwrap().iter().any(|point| point[0] == id));
                assert_eq!(ctx.maps().resolve(&ctx, id, revision, &Signal::new()).await.unwrap(), root.join("Committed.wav"));
                assert_eq!(
                    (LIBRARY.run)(&ctx, call(json!({"action":"cancel","job_id":started["job_id"]}))).await.unwrap_err().code(),
                    "not_running"
                );
                get(&ctx).await.unwrap().close().await;
            })
            .await;
    }

    #[test]
    fn scan_messages_name_folders_without_disclosing_absolute_paths() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("Drums");
        std::fs::create_dir(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let ctx = Ctx::new(
            super::super::Args {
                state_dir: dir.path().join("state"),
                kumi_home: dir.path().join("home"),
                hermes_home: None,
                hermes_root: None,
                daw: DawId::Reaper,
                library_roots: vec![root.clone()],
                media_roots: Vec::new(),
                tools: BTreeMap::new(),
            },
            Env::new(),
            Default::default(),
        )
        .unwrap();
        let reason = format!("{}: sample.wav failed; index {}", root.display(), ctx.paths().state_dir.join("library").display());
        let error = display_error(&ctx, OpError::Refused { code: "locked", reason, owner: None });
        assert_eq!(error.code(), "locked");
        assert!(!error.reason().contains(root.to_str().unwrap()), "{error:?}; granted roots: {:?}", ctx.library_roots());
        assert!(!error.reason().contains(ctx.paths().state_dir.to_str().unwrap()));
        assert!(error.reason().contains("Drums"));
        assert!(error.reason().contains("sample.wav"));
        assert_eq!(folder_label(&ctx), "Drums");
    }
}
