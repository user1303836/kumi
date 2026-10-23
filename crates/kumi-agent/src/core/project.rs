//! Where Kumi keeps what it knows of each saved project: its baselines (the project as last seen, for catching up),
//! its track ids' keepers and its history, under the project's id. A project id is 32 hex digits: kept inside the
//! project by its DAW, or the one its path gives. How a DAW tells a template from a song, and a song's folder, is its
//! own (`ProjectRules`).

use crate::core::conversations::{modified_ms, remove, write_privately};
use async_trait::async_trait;
use kumi_common::js::json::stringify;
use kumi_types::{contracts::JsonObject, errors::RuntimeError};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    rc::Rc,
    sync::LazyLock,
};
use tokio::io::AsyncReadExt;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Baseline {
    pub version: u32,
    pub path: String,
    pub name: String,
    pub saved_at: i64,
    pub artifact_id: String,
    pub pages: Vec<JsonObject>,
}
#[async_trait(?Send)]
pub trait ProjectStore {
    /// The project's latest baseline, of whichever of its Sets was seen last, wherever that Set was then (a
    /// moved Set finds it here).
    async fn load(&self, project: &str) -> Result<Option<Baseline>, RuntimeError>;
    /// The baseline to compare the Set at `path` with: the last one saved for that Set (each version of a
    /// song keeps its own), or, without one, the project's latest when that Set moved here. A version first
    /// seen here has none.
    async fn load_set(&self, project: &str, path: &str) -> Result<Option<Baseline>, RuntimeError> {
        Ok(self.load(project).await?.filter(|latest| moved_to(&latest.path, path)))
    }
    async fn save(&self, project: &str, baseline: &Baseline) -> Result<(), RuntimeError>;
    /// Where the project's Set seen last was: what opens to go back to it.
    async fn set_path(&self, project: &str) -> Option<String> {
        self.load(project).await.ok().flatten().map(|baseline| baseline.path)
    }
    /// Which track keeps each of Kumi's track ids in the project (id → the track's identity in Live), so a
    /// copy's id is told from its original's after a restart and by every Kumi alike.
    async fn load_track_keepers(&self, _project: &str) -> Result<HashMap<String, String>, RuntimeError> {
        Ok(HashMap::new())
    }
    async fn save_track_keepers(&self, _project: &str, _keepers: &HashMap<String, String>) -> Result<(), RuntimeError> {
        Ok(())
    }
    /// Where the project keeps its history (`history.db`, what Kumi's undo puts back and, later, its snapshots), if it
    /// keeps one.
    fn history_path(&self, _project: &str) -> Option<PathBuf> {
        None
    }
}
const MAX_BASELINE_BYTES: usize = 8 * 1024 * 1024;
/// The most other Sets of one song (versions saved beside it) a project keeps a baseline for.
const MAX_SETS: usize = 20;
/// The most track ids a project keeps the keeper of.
const MAX_KEEPERS: usize = 10_000;
/// The project id a Set's path gave before Kumi kept one inside the Set (and still gives a Set in a
/// templates folder, which never gets one).
pub fn project_id_of(path: &str) -> String {
    hex::encode(Sha256::digest(path.as_bytes()))[..32].to_owned()
}
/// Whether `id` is a project id: 32 lowercase hex digits.
pub fn project_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
/// Whether two paths name the same file or folder: a case-only rename, or a symlink, is the same one.
pub fn same_file(a: impl AsRef<Path>, b: impl AsRef<Path>) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        matches!((std::fs::metadata(a), std::fs::metadata(b)), (Ok(a), Ok(b)) if (a.dev(), a.ino()) == (b.dev(), b.ino()))
    }
    #[cfg(not(unix))]
    {
        matches!((std::fs::canonicalize(a), std::fs::canonicalize(b)), (Ok(a), Ok(b)) if a == b)
    }
}
/// Whether the Set seen at `last` is the one at `path` now: the same file, or moved from there.
pub fn moved_to(last: &str, path: &str) -> bool {
    last == path || !Path::new(last).exists() || same_file(last, path)
}
/// How a DAW tells its templates from songs, and where a song's own folder is.
pub trait ProjectRules {
    /// Whether a project at `path` is a template, or the DAW's own default project: new songs start from these, so one
    /// never gets a project id of its own, or every song made from it would share it.
    fn template_location(&self, path: &str) -> bool;
    /// The DAW's project folder a file is in (Live's Project folder), when it keeps one: a copy in the same folder is
    /// a version of the same song.
    fn project_folder(&self, path: &str) -> Option<PathBuf>;
}
/// A saved project's id, by the DAW's `rules`, from the id kept inside it (`kept`), where it is (`path`), where its project was last
/// seen (`last`), and whether it's a song of its own here (`own`: first saved from an unsaved Set, or seen
/// here before under the id its path gives): the id, and whether to keep it inside the Set.
/// - None kept (or not an id): the id its path gave before, so what Kumi kept carries over.
/// - A song of its own: the id its path gives, whatever id it came with (from a template, or a copy whose id
///   never reached its file), so it never turns into another song when that one moves.
/// - Kept, and its project last seen in another file that's still there: a copy. In the same Live Project
///   folder it's a version of the same song; elsewhere, or copied from a template, it's a new song, with the
///   id its own path gives (the same each time, should keeping it in the Set fail).
/// - Kept otherwise (the same file, a move, or a project first seen here): the same project.
///
/// A Set in a templates folder keeps the id its path gives, and nothing is ever written into it.
pub fn decide_project_with(rules: &dyn ProjectRules, kept: Option<&str>, path: &str, last: Option<&str>, own: bool) -> (String, bool) {
    let mine = project_id_of(path);
    if rules.template_location(path) {
        return (mine, false);
    }
    let Some(kept) = kept.filter(|id| project_id(id)) else { return (mine, true) };
    if own {
        let keep = kept != mine;
        return (mine, keep);
    }
    match last.filter(|last| *last != path && Path::new(last).exists() && !same_file(last, path)) {
        Some(last)
            if rules.template_location(last)
                || !matches!((rules.project_folder(last), rules.project_folder(path)), (Some(a), Some(b)) if same_file(&a, &b)) =>
        {
            (mine, true)
        }
        _ => (kept.to_string(), false),
    }
}
fn error(e: impl std::fmt::Display) -> RuntimeError {
    RuntimeError::plain(e.to_string())
}
pub struct FileProjectStore {
    directory: PathBuf,
}
pub fn create_project_store(directory: impl Into<PathBuf>) -> Rc<FileProjectStore> {
    Rc::new(FileProjectStore { directory: directory.into() })
}
async fn read_baseline(file: &Path) -> Option<Baseline> {
    let bytes = tokio::fs::read(file).await.ok()?;
    if bytes.len() > MAX_BASELINE_BYTES {
        return None;
    }
    let value = serde_json::from_slice::<Baseline>(&bytes).ok()?;
    (value.version == 1 && !value.pages.is_empty()).then_some(value)
}
/// The Set path in a baseline file: its key `path`, which comes before the pages.
static BASELINE_PATH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"[{,]"path":("(?:[^"\\]|\\.)*")"#).unwrap());
/// Where the Set of a baseline file was, read from its start (the pages after it can be megabytes).
async fn baseline_path(file: &Path) -> Option<String> {
    let file = tokio::fs::File::open(file).await.ok()?;
    let mut start = Vec::with_capacity(16 * 1024);
    file.take(16 * 1024).read_to_end(&mut start).await.ok()?;
    let text = String::from_utf8_lossy(&start);
    serde_json::from_str::<String>(BASELINE_PATH.captures(&text)?.get(1)?.as_str()).ok()
}
/// A project's folder keeps the baseline of the Set seen last in `last-seen.json`, and the last baseline of
/// each other Set of the song that's still there (a version saved beside it) in `sets/`, by its path.
#[async_trait(?Send)]
impl ProjectStore for FileProjectStore {
    async fn load(&self, project: &str) -> Result<Option<Baseline>, RuntimeError> {
        if !project_id(project) {
            return Ok(None);
        }
        Ok(read_baseline(&self.directory.join(project).join("last-seen.json")).await)
    }
    async fn load_set(&self, project: &str, path: &str) -> Result<Option<Baseline>, RuntimeError> {
        if !project_id(project) {
            return Ok(None);
        }
        let folder = self.directory.join(project);
        let last = baseline_path(&folder.join("last-seen.json")).await;
        if last.as_deref() == Some(path) {
            return Ok(read_baseline(&folder.join("last-seen.json")).await);
        }
        // Its own first: the latest version being deleted or renamed doesn't make this one a move.
        let file = folder.join("sets").join(format!("{}.json", project_id_of(path)));
        if let Some(own) = read_baseline(&file).await.filter(|baseline| baseline.path == path) {
            return Ok(Some(own));
        }
        Ok(match last {
            Some(last) if moved_to(&last, path) => read_baseline(&folder.join("last-seen.json")).await,
            _ => None,
        })
    }
    async fn save(&self, project: &str, baseline: &Baseline) -> Result<(), RuntimeError> {
        if !project_id(project) {
            return Err(error("invalid project id"));
        }
        let text = stringify(&serde_json::to_value(baseline).map_err(error)?);
        if text.len() > MAX_BASELINE_BYTES {
            return Ok(());
        }
        let folder = self.directory.join(project);
        let sets = folder.join("sets");
        // Another Set of the song seen before, and still there, keeps its baseline under its own path; one
        // that moved here leaves nothing behind.
        if let Some(before) = baseline_path(&folder.join("last-seen.json")).await {
            if !moved_to(&before, &baseline.path) {
                write_privately(&sets, ".keep", "").await?;
                tokio::fs::rename(folder.join("last-seen.json"), sets.join(format!("{}.json", project_id_of(&before))))
                    .await
                    .map_err(error)?;
                let mut kept = vec![];
                if let Ok(mut dir) = tokio::fs::read_dir(&sets).await {
                    while let Ok(Some(entry)) = dir.next_entry().await {
                        if entry.file_name().to_str().is_some_and(|name| name.ends_with(".json")) {
                            kept.push((modified_ms(&entry.path()).await, entry.path()));
                        }
                    }
                }
                kept.sort_by(|a, b| b.0.total_cmp(&a.0));
                for (_, file) in kept.into_iter().skip(MAX_SETS) {
                    remove(file).await?;
                }
            }
        }
        write_privately(&folder, "last-seen.json", &text).await?;
        // This Set's own baseline is the latest now.
        remove(sets.join(format!("{}.json", project_id_of(&baseline.path)))).await
    }
    async fn set_path(&self, project: &str) -> Option<String> {
        if !project_id(project) {
            return None;
        }
        baseline_path(&self.directory.join(project).join("last-seen.json")).await
    }
    async fn load_track_keepers(&self, project: &str) -> Result<HashMap<String, String>, RuntimeError> {
        if !project_id(project) {
            return Ok(HashMap::new());
        }
        let Ok(bytes) = tokio::fs::read(self.directory.join(project).join("track-ids.json")).await else { return Ok(HashMap::new()) };
        let kept: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        Ok(kept["keepers"]
            .as_object()
            .into_iter()
            .flatten()
            .take(MAX_KEEPERS)
            .filter_map(|(id, identity)| Some((id.clone(), identity.as_str()?.to_owned())))
            .collect())
    }
    fn history_path(&self, project: &str) -> Option<PathBuf> {
        project_id(project).then(|| self.directory.join(project).join("history.db"))
    }
    async fn save_track_keepers(&self, project: &str, keepers: &HashMap<String, String>) -> Result<(), RuntimeError> {
        if !project_id(project) {
            return Err(error("invalid project id"));
        }
        let kept: serde_json::Map<String, Value> =
            keepers.iter().take(MAX_KEEPERS).map(|(id, identity)| (id.clone(), json!(identity))).collect();
        write_privately(&self.directory.join(project), "track-ids.json", &stringify(&json!({"version":1,"keepers":kept}))).await
    }
}
