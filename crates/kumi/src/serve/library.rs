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
    description: "Show Kumi's approved sample folders and learned sounds, add one explicitly approved local folder, or start a scan job for all approved folders. Never discovers DAW libraries, projects or other folders. Scanning reads and measures audio locally; it does not download programs or models.",
    schema,
    effect,
    run,
};

fn schema() -> Value {
    json!({"type":"object","additionalProperties":false,"required":["action"],"properties":{
        "action":{"type":"string","enum":["status","add_folder","scan"]},
        "folder":{"type":"string","minLength":1,"maxLength":32768,"description":"Absolute local folder, required only for add_folder. Scan covers all currently approved folders."}
    }})
}
fn effect(args: &JsonObject) -> Effect {
    if args.get("action").and_then(Value::as_str) == Some("status") {
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
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Args {
    action: Action,
    folder: Option<String>,
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
        let args: Args =
            serde_json::from_value(Value::Object(call.args)).map_err(|error| OpError::Invalid { reason: error.to_string() })?;
        check(&call.signal)?;
        if !matches!(args.action, Action::AddFolder) && args.folder.is_some() {
            return Err(OpError::Invalid { reason: "folder is only for add_folder; scan reads all currently approved folders.".into() });
        }
        match args.action {
            Action::Status => status(ctx).await,
            Action::AddFolder => {
                let folder = args.folder.ok_or_else(|| OpError::Invalid { reason: "add_folder needs an absolute local folder.".into() })?;
                let _changing = tokio::select! {
                    guard = ctx.library_state().changing.lock() => guard,
                    _ = call.signal.cancelled() => return Err(cancelled()),
                };
                idle(ctx)?;
                // This approved action can grant a new folder; it cannot follow a network namespace or a replaced link.
                let folder = canonical_local_path(&folder).filter(|folder| folder.is_dir()).ok_or_else(|| OpError::Invalid {
                    reason:
                        "Choose an existing local folder by its canonical absolute path, without symlinks or network/device namespaces."
                            .into(),
                })?;
                check(&call.signal)?;
                ctx.add_library_root(&folder).map_err(failed)?;
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
                                        _ => (0., None, scanning.as_str()),
                                    };
                                    progress_job.report(done, total, Some(message));
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
    })
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
        assert!(!error.reason().contains(root.to_str().unwrap()));
        assert!(!error.reason().contains(ctx.paths().state_dir.to_str().unwrap()));
        assert!(error.reason().contains("Drums"));
        assert!(error.reason().contains("sample.wav"));
        assert_eq!(folder_label(&ctx), "Drums");
    }
}
