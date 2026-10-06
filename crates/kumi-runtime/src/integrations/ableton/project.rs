//! Saved Set baselines, bounded conversation history, and semantic catch-up descriptions.

use crate::{
    core::{
        contracts::{
            CatchUp, ChangeState, ConversationStore, ConversationSummary, CurrentConversation, FoundExchange, JsonObject,
            SavedConversation, TranscriptRole,
        },
        errors::RuntimeError,
    },
    kernel::budget::transcript_of,
};
use async_trait::async_trait;
use indexmap::{IndexMap, IndexSet};
use kumi_common::js::{
    json::stringify,
    number::{round, to_string},
    string::{head, trim, utf16_len},
};
use rand::TryRngCore;
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
use tokio::io::{AsyncReadExt, AsyncWriteExt};

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
const MAX_CONVERSATION_BYTES: usize = 256 * 1024;
const MAX_KEPT_CHANGES: usize = 100;
const MAX_KEPT: usize = 20;
/// The most other Sets of one song (versions saved beside it) a project keeps a baseline for.
const MAX_SETS: usize = 20;
/// The most track ids a project keeps the keeper of.
const MAX_KEEPERS: usize = 10_000;
/// The project id a Set's path gave before Kumi kept one inside the Set (and still gives a Set in a
/// templates folder, which never gets one).
pub fn project_id_of(path: &str) -> String {
    hex::encode(Sha256::digest(path.as_bytes()))[..32].to_owned()
}
/// The key a Set keeps its project's id under (`Song.set_data`), with the Set: Save As and moving the
/// folder keep it.
pub const PROJECT_KEY: &str = "kumi.project";
/// Whether `id` is a project id: 32 lowercase hex digits.
pub fn project_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
/// Whether a Set at `path` is a template, or Live's own default Set: new songs start from these, so one
/// never gets a project id of its own, or every song made from it would share it.
pub fn template_location(path: &str) -> bool {
    let parts: Vec<String> = Path::new(path).components().map(|part| part.as_os_str().to_string_lossy().to_lowercase()).collect();
    let has = |name: &str| parts.iter().any(|part| part == name);
    let in_row = |names: &[&str]| parts.windows(names.len()).any(|row| row.iter().zip(names).all(|(part, name)| part == name));
    in_row(&["user library", "templates"])
        // Live's own settings, where its default Set is kept: ~/Library/Preferences/Ableton on a Mac,
        // %APPDATA%\Ableton on Windows.
        || in_row(&["library", "preferences", "ableton"])
        || in_row(&["appdata", "roaming", "ableton"])
        || (has("templates") && parts.iter().any(|part| part.ends_with(".app") || part == "resources"))
}
/// The Live Project folder a Set is in: the nearest folder above it with an "Ableton Project Info".
pub fn project_folder(path: &str) -> Option<PathBuf> {
    Path::new(path).ancestors().skip(1).find(|folder| folder.join("Ableton Project Info").is_dir()).map(Path::to_path_buf)
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
/// A Set's project, from the id kept inside it (`kept`), where it is (`path`), where its project was last
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
pub fn decide_project(kept: Option<&str>, path: &str, last: Option<&str>, own: bool) -> (String, bool) {
    let mine = project_id_of(path);
    if template_location(path) {
        return (mine, false);
    }
    let Some(kept) = kept.filter(|id| project_id(id)) else { return (mine, true) };
    if own {
        let keep = kept != mine;
        return (mine, keep);
    }
    match last.filter(|last| *last != path && Path::new(last).exists() && !same_file(last, path)) {
        Some(last)
            if template_location(last)
                || !matches!((project_folder(last), project_folder(path)), (Some(a), Some(b)) if same_file(&a, &b)) =>
        {
            (mine, true)
        }
        _ => (kept.to_string(), false),
    }
}
fn error(e: impl std::fmt::Display) -> RuntimeError {
    RuntimeError::plain(e.to_string())
}
async fn write_privately(folder: &Path, name: &str, text: &str) -> Result<(), RuntimeError> {
    let mut builder = tokio::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    builder.mode(0o700);
    builder.create(folder).await.map_err(error)?;
    let temporary = folder.join(format!(".{name}-{}", uuid::Uuid::new_v4()));
    let result = async {
        let mut options = tokio::fs::OpenOptions::new();
        options.create(true).truncate(true).write(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options.open(&temporary).await?;
        file.write_all(text.as_bytes()).await?;
        file.flush().await?;
        drop(file);
        tokio::fs::rename(&temporary, folder.join(name)).await
    }
    .await;
    if let Err(e) = result {
        let _ = tokio::fs::remove_file(&temporary).await;
        return Err(error(e));
    }
    Ok(())
}
async fn remove(file: impl AsRef<Path>) -> Result<(), RuntimeError> {
    match tokio::fs::remove_file(file).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(error(e)),
    }
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
pub fn new_conversation_id(at: i64) -> String {
    let mut number = at.unsigned_abs();
    let mut digits = Vec::new();
    loop {
        digits.push(b"0123456789abcdefghijklmnopqrstuvwxyz"[(number % 36) as usize] as char);
        number /= 36;
        if number == 0 {
            break;
        }
    }
    if at < 0 {
        digits.push('-')
    }
    let mut bytes = [0; 3];
    rand::rngs::OsRng.try_fill_bytes(&mut bytes).expect("OS randomness");
    format!("{}{}", digits.into_iter().rev().collect::<String>(), hex::encode(bytes))
}
static PLACE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^([0-9a-f]{32}|unsaved)$").unwrap());
static CONVERSATION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[0-9a-z]{6,24}$").unwrap());
/// The Set's name in a baseline file: its first key `name`, which comes before the pages.
static BASELINE_NAME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"[{,]"name":("(?:[^"\\]|\\.)*")"#).unwrap());
/// The most kept conversations a search reads, newest first.
const MAX_SEARCHED: usize = 400;
async fn modified_ms(path: &Path) -> f64 {
    tokio::fs::metadata(path)
        .await
        .ok()
        .and_then(|s| s.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64() * 1000.0)
        .unwrap_or(0.0)
}
pub struct FileConversationStore {
    directory: PathBuf,
}
pub fn create_conversation_store(directory: impl Into<PathBuf>) -> Rc<FileConversationStore> {
    Rc::new(FileConversationStore { directory: directory.into() })
}
impl FileConversationStore {
    fn folder(&self, place: &str) -> Result<PathBuf, RuntimeError> {
        if !PLACE.is_match(place) {
            return Err(error("invalid place"));
        }
        Ok(self.directory.join(place))
    }
    fn kept(&self, place: &str) -> Result<PathBuf, RuntimeError> {
        Ok(self.folder(place)?.join("conversations"))
    }
    fn file(&self, place: &str, id: &str) -> Result<PathBuf, RuntimeError> {
        if !CONVERSATION.is_match(id) {
            return Err(error("invalid conversation id"));
        }
        Ok(self.kept(place)?.join(format!("{id}.json")))
    }
    async fn read(file: &Path) -> Option<SavedConversation> {
        let bytes = tokio::fs::read(file).await.ok()?;
        let value: Value = serde_json::from_slice(&bytes).ok()?;
        if !value["savedAt"].is_number()
            || value["checkpoint"]["version"].as_f64() != Some(1.0)
            || !value["checkpoint"]["messages"].is_array()
        {
            return None;
        }
        let mut kept = json!({"savedAt":value["savedAt"],"checkpoint":value["checkpoint"]});
        for key in ["changes", "first", "turns"] {
            let allowed = match key {
                "changes" => value[key].is_array(),
                "first" => value[key].is_string(),
                _ => value[key].is_number(),
            };
            if allowed {
                kept[key] = value[key].clone()
            }
        }
        serde_json::from_value(kept).ok()
    }
    async fn current_id(&self, place: &str) -> Option<String> {
        let folder = self.folder(place).ok()?;
        let text = tokio::fs::read_to_string(folder.join("current")).await.ok()?;
        let id = trim(&text);
        CONVERSATION.is_match(id).then(|| id.to_owned())
    }
    async fn migrate(&self, place: &str) -> Result<(), RuntimeError> {
        let legacy = self.folder(place)?.join("conversation.json");
        let Some(value) = Self::read(&legacy).await else { return Ok(()) };
        let id = new_conversation_id(value.saved_at);
        write_privately(&self.kept(place)?, &format!("{id}.json"), &stringify(&serde_json::to_value(&value).map_err(error)?)).await?;
        if self.current_id(place).await.is_none() {
            write_privately(&self.folder(place)?, "current", &id).await?;
        }
        remove(&legacy).await
    }
    async fn names(&self, place: &str) -> Vec<String> {
        let Ok(folder) = self.kept(place) else { return vec![] };
        let Ok(mut dir) = tokio::fs::read_dir(folder).await else { return vec![] };
        let mut names = vec![];
        while let Ok(Some(entry)) = dir.next_entry().await {
            if let Some(name) = entry.file_name().to_str() {
                if name.strip_suffix(".json").is_some_and(|id| CONVERSATION.is_match(id)) {
                    names.push(name.to_owned());
                }
            }
        }
        names.sort();
        names
    }
    /// A saved Set's name, read from the start of its baseline (the pages after it can be megabytes).
    async fn set_name(&self, place: &str) -> Option<String> {
        if place == "unsaved" {
            return None;
        }
        let file = tokio::fs::File::open(self.folder(place).ok()?.join("last-seen.json")).await.ok()?;
        let mut start = Vec::with_capacity(16 * 1024);
        file.take(16 * 1024).read_to_end(&mut start).await.ok()?;
        let text = String::from_utf8_lossy(&start);
        let quoted = BASELINE_NAME.captures(&text)?.get(1)?.as_str();
        serde_json::from_str::<String>(quoted).ok().filter(|name| !trim(name).is_empty())
    }
    async fn trim(&self, place: &str) -> Result<(), RuntimeError> {
        let current = format!("{}.json", self.current_id(place).await.as_deref().unwrap_or("undefined"));
        let names = self.names(place).await;
        let mut others = vec![];
        for name in names.iter().filter(|n| **n != current) {
            let at = tokio::fs::metadata(self.kept(place)?.join(name))
                .await
                .ok()
                .and_then(|s| s.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs_f64() * 1000.0)
                .unwrap_or(0.0);
            others.push((name, at));
        }
        others.sort_by(|a, b| b.1.total_cmp(&a.1));
        for (name, _) in others.into_iter().skip(MAX_KEPT - usize::from(names.contains(&current))) {
            remove(self.kept(place)?.join(name)).await?
        }
        Ok(())
    }
}
/// A message as kept on disk: a picture the producer added is named, not kept, so one screenshot
/// can't crowd out the conversation.
fn without_pictures(message: &Value) -> Value {
    let Some(parts) = message["content"].as_array().filter(|_| message["role"] == "user") else { return message.clone() };
    if !parts.iter().any(|part| part["type"] == "file") {
        return message.clone();
    }
    let mut kept = message.clone();
    kept["content"] = parts
        .iter()
        .map(|part| {
            if part["type"] != "file" {
                return part.clone();
            }
            let name = part["filename"].as_str().unwrap_or("a picture");
            json!({"type":"text","text":format!("[The producer showed {name} here; pictures aren't kept with saved conversations.]")})
        })
        .collect();
    kept
}
// Disk checkpoints also contain older string-form messages; keep those forms intact.
fn bounded_messages(all: &[Value]) -> Vec<Value> {
    let sizes: Vec<_> = all.iter().map(|m| stringify(m).len() + 1).collect();
    let mut total = sizes.iter().sum::<usize>() + 1;
    let mut start = 0;
    while start < all.len() && total > MAX_CONVERSATION_BYTES {
        loop {
            total -= sizes[start];
            start += 1;
            if start >= all.len() || all[start]["role"] == "user" {
                break;
            }
        }
    }
    let mut messages = all[start..].to_vec();
    if messages.is_empty() {
        if let Some(last) = all.iter().rposition(|m| m["role"] == "user") {
            if stringify(&json!(&all[last..])).len() <= 4 * MAX_CONVERSATION_BYTES {
                messages = all[last..].to_vec();
            }
        }
    }
    if !messages.is_empty() && messages.len() < all.len() {
        const SHORT: &str = "[Kumi removed the earlier part of this conversation to save room.]\n\n";
        if messages[0]["role"] == "user" {
            if let Some(content) = messages[0]["content"].as_array_mut() {
                if let Some(part) = content.iter_mut().find(|p| p["type"] == "text") {
                    if let Some(text) = part["text"].as_str() {
                        if !text.starts_with(SHORT) {
                            part["text"] = json!(format!("{SHORT}{text}"))
                        }
                    }
                } else {
                    content.insert(0, json!({"type":"text","text":trim(SHORT)}));
                }
            }
        }
    }
    messages
}
#[async_trait(?Send)]
impl ConversationStore for FileConversationStore {
    async fn current(&self, place: &str) -> Result<Option<CurrentConversation>, RuntimeError> {
        if self.migrate(place).await.is_err() {
            return Ok(None);
        }
        let Some(id) = self.current_id(place).await else { return Ok(None) };
        let Some(conversation) = Self::read(&self.file(place, &id)?).await else { return Ok(None) };
        Ok(Some(CurrentConversation { id, conversation }))
    }
    async fn load(&self, place: &str, id: &str) -> Result<Option<SavedConversation>, RuntimeError> {
        if self.migrate(place).await.is_err() {
            return Ok(None);
        }
        let Ok(file) = self.file(place, id) else { return Ok(None) };
        Ok(Self::read(&file).await)
    }
    async fn save(&self, place: &str, id: &str, conversation: &SavedConversation) -> Result<(), RuntimeError> {
        let all: &Vec<Value> = &conversation.checkpoint.messages.iter().map(without_pictures).collect();
        let messages = bounded_messages(all);
        if messages.is_empty() {
            return Ok(());
        }
        let said: Vec<_> = transcript_of(all).into_iter().filter(|l| l.role == TranscriptRole::User).collect();
        let mut checkpoint = conversation.checkpoint.clone();
        checkpoint.messages = messages;
        let saved = SavedConversation {
            saved_at: conversation.saved_at,
            checkpoint,
            changes: conversation
                .changes
                .as_ref()
                .filter(|c| !c.is_empty())
                .map(|c| c[c.len().saturating_sub(MAX_KEPT_CHANGES)..].to_vec()),
            first: Some(head(conversation.first.as_deref().or_else(|| said.first().map(|l| l.text.as_str())).unwrap_or(""), 200)),
            turns: Some(conversation.turns.unwrap_or(0).max(said.len() as u32)),
        };
        self.file(place, id)?;
        write_privately(&self.kept(place)?, &format!("{id}.json"), &stringify(&serde_json::to_value(saved).map_err(error)?)).await?;
        write_privately(&self.folder(place)?, "current", id).await?;
        self.trim(place).await
    }
    async fn fresh(&self, place: &str) -> Result<(), RuntimeError> {
        let folder = self.folder(place)?;
        if self.current_id(place).await.is_some() {
            write_privately(&folder, "current", "").await?
        }
        Ok(())
    }
    async fn list(&self, place: &str) -> Result<Vec<ConversationSummary>, RuntimeError> {
        if self.migrate(place).await.is_err() {
            return Ok(vec![]);
        }
        let current = self.current_id(place).await;
        let mut rows = vec![];
        for name in self.names(place).await {
            let Some(value) = Self::read(&self.kept(place)?.join(&name)).await else { continue };
            let id = name[..name.len() - 5].to_owned();
            let said: Vec<_> = transcript_of(&value.checkpoint.messages).into_iter().filter(|l| l.role == TranscriptRole::User).collect();
            rows.push(ConversationSummary {
                id: id.clone(),
                saved_at: value.saved_at,
                first: value.first.unwrap_or_else(|| said.first().map(|l| head(&l.text, 200)).unwrap_or_default()),
                turns: value.turns.unwrap_or(said.len() as u32),
                current: Some(&id) == current.as_ref(),
            });
        }
        rows.sort_by(|a, b| b.saved_at.cmp(&a.saved_at));
        Ok(rows)
    }
    async fn move_conversation(&self, id: &str, from: &str, to: &str) -> Result<(), RuntimeError> {
        let file = self.file(from, id)?;
        let Some(value) = Self::read(&file).await else { return Ok(()) };
        self.file(to, id)?;
        write_privately(&self.kept(to)?, &format!("{id}.json"), &stringify(&serde_json::to_value(value).map_err(error)?)).await?;
        write_privately(&self.folder(to)?, "current", id).await?;
        remove(&file).await?;
        if self.current_id(from).await.as_deref() == Some(id) {
            write_privately(&self.folder(from)?, "current", "").await?
        }
        self.trim(to).await
    }
    async fn search(
        &self,
        words: &[String],
        needed: usize,
        limit: usize,
        skip: Option<(&str, &str)>,
    ) -> Result<Vec<FoundExchange>, RuntimeError> {
        if words.is_empty() || limit == 0 {
            return Ok(vec![]);
        }
        let mut files = vec![];
        if let Ok(mut places) = tokio::fs::read_dir(&self.directory).await {
            while let Ok(Some(entry)) = places.next_entry().await {
                let Some(place) = entry.file_name().to_str().filter(|p| PLACE.is_match(p)).map(str::to_owned) else { continue };
                for name in self.names(&place).await {
                    let path = self.kept(&place)?.join(&name);
                    let at = modified_ms(&path).await;
                    files.push((place.clone(), name, path, at));
                }
            }
        }
        files.sort_by(|a, b| b.3.total_cmp(&a.3));
        files.truncate(MAX_SEARCHED);
        let mut found = vec![];
        for (place, name, path, _) in files {
            let id = name[..name.len() - 5].to_owned();
            if skip == Some((place.as_str(), id.as_str())) {
                continue;
            }
            // Reading and scoring hundreds of files: the app keeps drawing between them.
            tokio::task::yield_now().await;
            let Some(conversation) = Self::read(&path).await else { continue };
            let mut exchange = |said: String, answer: String, tools: Vec<String>| {
                let text = format!("{said}\n{answer}\n{}", tools.join(" ")).to_lowercase();
                let matched = words.iter().filter(|word| text.contains(word.as_str())).count();
                if matched >= needed {
                    found.push(FoundExchange {
                        place: place.clone(),
                        set: None,
                        conversation: id.clone(),
                        saved_at: conversation.saved_at,
                        said,
                        answer,
                        tools,
                        matched,
                    });
                }
            };
            // An exchange is a request and the answers to it, with the tools they used.
            let mut current: Option<(String, Vec<String>, Vec<String>)> = None;
            for line in transcript_of(&conversation.checkpoint.messages) {
                match line.role {
                    TranscriptRole::User => {
                        if let Some((said, answers, tools)) = current.take() {
                            exchange(said, answers.join("\n"), tools);
                        }
                        current = Some((line.text, vec![], vec![]));
                    }
                    TranscriptRole::Assistant => {
                        if let Some((_, answers, tools)) = current.as_mut() {
                            if !line.text.is_empty() {
                                answers.push(line.text);
                            }
                            tools.extend(line.tools.unwrap_or_default());
                        }
                    }
                }
            }
            if let Some((said, answers, tools)) = current.take() {
                exchange(said, answers.join("\n"), tools);
            }
            // What the conversation changed in Live, in plain words.
            let titles: Vec<_> =
                conversation.changes.iter().flatten().filter(|c| c.state != ChangeState::Undone).map(|c| c.title.as_str()).collect();
            if !titles.is_empty() {
                exchange(conversation.first.clone().unwrap_or_default(), format!("Changed: {}", titles.join("; ")), vec![]);
            }
        }
        found.sort_by(|a, b| b.matched.cmp(&a.matched).then(b.saved_at.cmp(&a.saved_at)));
        found.truncate(limit);
        let mut names: Vec<(String, Option<String>)> = vec![];
        for hit in &mut found {
            if let Some((_, name)) = names.iter().find(|(place, _)| *place == hit.place) {
                hit.set = name.clone();
            } else {
                let name = self.set_name(&hit.place).await;
                names.push((hit.place.clone(), name.clone()));
                hit.set = name;
            }
        }
        Ok(found)
    }
}

pub fn since(saved_at: f64, now: f64) -> String {
    let minutes = round((now - saved_at) / 60_000.0).max(0.0);
    if minutes < 2.0 {
        return "just now".into();
    }
    if minutes < 60.0 {
        return format!("{} minutes ago", to_string(minutes));
    }
    let hours = round(minutes / 60.0);
    if hours < 24.0 {
        return format!("{} {} ago", to_string(hours), if hours == 1.0 { "hour" } else { "hours" });
    }
    let days = round(hours / 24.0);
    format!("{} {} ago", to_string(days), if days == 1.0 { "day" } else { "days" })
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DescribedDiff {
    pub lines: Vec<String>,
    pub more: usize,
}
pub fn catch_up_from(name: &str, baseline: &Baseline, described: DescribedDiff) -> CatchUp {
    CatchUp { set: name.into(), last_seen_at: baseline.saved_at, lines: described.lines, more: described.more, after_reconnect: None }
}

const KINDS: &[(&str, &str)] =
    &[("track", "tracks"), ("scene", "scenes"), ("clip", "clips"), ("device", "devices"), ("locator", "locators")];
fn plural(kind: &str) -> String {
    KINDS.iter().find(|(one, _)| *one == kind).map(|(_, many)| many.to_string()).unwrap_or_else(|| format!("{kind}s"))
}
fn known(kind: &str) -> bool {
    KINDS.iter().any(|(one, _)| *one == kind)
}
fn records(pages: &[JsonObject]) -> IndexMap<String, Value> {
    let mut rows = IndexMap::new();
    for page in pages {
        for item in page.get("records").and_then(Value::as_array).into_iter().flatten() {
            if let Some(id) = item["snapshotId"].as_str() {
                rows.insert(id.to_owned(), item.clone());
            }
        }
    }
    rows
}
fn js_text(value: Option<&Value>) -> String {
    match value {
        None => "undefined".into(),
        Some(Value::Null) => "null".into(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(items)) => {
            items.iter().map(|v| if v.is_null() { String::new() } else { js_text(Some(v)) }).collect::<Vec<_>>().join(",")
        }
        Some(Value::Object(_)) => "[object Object]".into(),
        Some(v) => stringify(v),
    }
}
fn quoted(value: &Value, fallback: &str) -> String {
    value.as_str().map(trim).filter(|s| !s.is_empty()).map(|s| format!("“{}”", head(s, 60))).unwrap_or_else(|| fallback.into())
}
static DERIVED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"/(parentSnapshotId|structureHash|trackCount|sceneCount|clipCount|deviceCount)$|/hash$").unwrap());
fn named(kind: &str, items: &[Value], verb: &str) -> String {
    let names: Vec<_> = items.iter().map(|item| quoted(&item["name"], "")).filter(|s| !s.is_empty()).collect();
    if items.len() == 1 {
        return trim(&format!("{verb} {kind} {}", names.first().map(String::as_str).unwrap_or(""))).into();
    }
    if names.len() == items.len() && items.len() <= 3 {
        format!("{verb} {} {}", plural(kind), names.join(", "))
    } else {
        format!("{verb} {} {}", items.len(), plural(kind))
    }
}
fn pick(ids: &Value, from: &IndexMap<String, Value>) -> Vec<Value> {
    ids.as_array().into_iter().flatten().filter_map(|id| from.get(&js_text(Some(id))).cloned()).collect()
}
fn resolve_ambiguity(
    item: &Value,
    was: &IndexMap<String, Value>,
    now: &IndexMap<String, Value>,
    added: &mut IndexMap<String, Vec<Value>>,
    removed: &mut IndexMap<String, Vec<Value>>,
    lines: &mut Vec<String>,
) {
    let kind = js_text(item.get("kind"));
    if !known(&kind) {
        return;
    }
    let mut olds = pick(&item["beforeSnapshotIds"], was);
    let mut news = pick(&item["afterSnapshotIds"], now);
    olds.retain(|old| {
        if let Some(i) = news.iter().position(|row| row.get("name") == old.get("name")) {
            news.remove(i);
            false
        } else {
            true
        }
    });
    olds.retain(|old| {
        if let Some(i) = news.iter().position(|row| row.get("order") == old.get("order")) {
            let same = news.remove(i);
            lines.push(format!("Renamed {kind} {} → {}", quoted(&old["name"], "(unnamed)"), quoted(&same["name"], "(unnamed)")));
            false
        } else {
            true
        }
    });
    if !news.is_empty() {
        added.entry(kind.clone()).or_default().extend(news);
    }
    if !olds.is_empty() {
        removed.entry(kind).or_default().extend(olds);
    }
}
pub fn describe_diff(diff: &JsonObject, before: &[JsonObject], after: &[JsonObject], limit: Option<usize>) -> DescribedDiff {
    let was = records(before);
    let now = records(after);
    let mut lines = vec![];
    let mut added: IndexMap<String, Vec<Value>> = IndexMap::new();
    let mut removed: IndexMap<String, Vec<Value>> = IndexMap::new();
    let mut edited: IndexMap<String, Vec<(Value, IndexSet<String>)>> = IndexMap::new();
    for item in diff.get("items").and_then(Value::as_array).into_iter().flatten() {
        if item["type"] == "ambiguity" {
            resolve_ambiguity(item, &was, &now, &mut added, &mut removed, &mut lines);
            continue;
        }
        if item["type"] != "change" {
            continue;
        }
        let Some(facets) = item["facets"].as_array() else { continue };
        let kind = js_text(item.get("kind"));
        let old = item["beforeSnapshotId"].as_str().and_then(|id| was.get(id));
        let current = item["afterSnapshotId"].as_str().and_then(|id| now.get(id));
        if facets.iter().any(|v| v == "added") {
            if let Some(current) = current {
                added.entry(kind).or_default().push(current.clone());
                continue;
            }
        }
        if facets.iter().any(|v| v == "removed") {
            if let Some(old) = old {
                removed.entry(kind).or_default().push(old.clone());
                continue;
            }
        }
        let details: Vec<_> =
            item["details"].as_array().into_iter().flatten().filter(|d| d["path"].as_str().is_some_and(|p| !DERIVED.is_match(p))).collect();
        if kind == "set" {
            for d in details {
                if d["path"] == "/data/tempo" && d["before"].is_number() && d["after"].is_number() {
                    lines.push(format!("Tempo {} → {} BPM", js_text(d.get("before")), js_text(d.get("after"))));
                }
            }
            continue;
        }
        if facets.iter().any(|v| v == "renamed") {
            if let (Some(old), Some(current)) = (old, current) {
                lines.push(format!("Renamed {kind} {} → {}", quoted(&old["name"], "(unnamed)"), quoted(&current["name"], "(unnamed)")));
            }
        }
        let what: IndexSet<_> =
            details.iter().filter_map(|d| d["path"].as_str()?.split('/').nth(2)).filter(|p| !p.is_empty()).map(str::to_owned).collect();
        if !what.is_empty() {
            if let Some(row) = current.or(old) {
                edited.entry(kind).or_default().push((row.clone(), what));
            }
        }
    }
    for (kind, gone) in &mut removed {
        let Some(fresh) = added.get_mut(kind) else { continue };
        gone.retain(|old| {
            if let Some(i) = fresh.iter().position(|item| item.get("order") == old.get("order")) {
                let matched = fresh.remove(i);
                lines.push(format!(
                    "{} → {} (renamed and changed)",
                    quoted(&old["name"], &format!("An unnamed {kind}")),
                    quoted(&matched["name"], "(unnamed)")
                ));
                false
            } else {
                true
            }
        });
    }
    for (kind, _) in KINDS {
        if let Some(items) = added.get(*kind).filter(|items| !items.is_empty()) {
            lines.push(named(kind, items, "Added"))
        }
        if let Some(items) = removed.get(*kind).filter(|items| !items.is_empty()) {
            lines.push(named(kind, items, "Removed"))
        }
        if let Some(changes) = edited.get(*kind) {
            if changes.len() == 1 {
                let (record, what) = &changes[0];
                let what = what.iter().map(String::as_str).collect::<Vec<_>>().join(", ");
                lines.push(format!(
                    "Changed {kind} {}{}",
                    quoted(&record["name"], "(unnamed)"),
                    if what.is_empty() { String::new() } else { format!(" ({what})") }
                ));
            } else if changes.len() > 1 {
                lines.push(format!("Changed {} {}", changes.len(), plural(kind)));
            }
        }
    }
    let limit = limit.unwrap_or(8);
    let more = lines.len().saturating_sub(limit);
    lines.truncate(limit);
    DescribedDiff { lines, more }
}

fn numeric(value: Option<&Value>) -> f64 {
    match value {
        Some(Value::Null) => 0.0,
        Some(Value::Number(n)) => n.as_f64().unwrap_or(f64::NAN),
        Some(Value::Bool(v)) => f64::from(*v),
        Some(Value::String(s)) => kumi_common::js::number::parse(s).unwrap_or(f64::NAN),
        _ => f64::NAN,
    }
}
fn track_coordinates(pages: &[JsonObject]) -> IndexMap<String, String> {
    let rows = records(pages);
    let mut tracks: Vec<_> = rows.values().filter(|r| r["kind"] == "track").collect();
    tracks.sort_by(|a, b| (numeric(a.get("order")) - numeric(b.get("order"))).partial_cmp(&0.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut counts: IndexMap<String, usize> = IndexMap::new();
    let mut names = IndexMap::new();
    for row in tracks {
        let data = &row["data"];
        let canonical = json!({"kind":data["kind"],"name":row["name"],"structureHash":data["structureHash"]});
        let base = format!("track-snapshot:{}", &hex::encode(Sha256::digest(stringify(&canonical)))[..20]);
        let count = counts.entry(base.clone()).or_default();
        *count += 1;
        names.insert(format!("{base}-{count}"), row["name"].as_str().unwrap_or("(unnamed)").to_owned());
    }
    names
}
fn small(value: Option<&Value>) -> Option<Value> {
    let value = value?;
    Some(match value {
        Value::String(s) => json!(head(s, 120)),
        Value::Array(_) | Value::Object(_) => {
            let text = stringify(value);
            if utf16_len(&text) <= 240 {
                value.clone()
            } else {
                json!(format!("{}…", head(&text, 237)))
            }
        }
        _ => value.clone(),
    })
}
fn made_of(kind: &str, row: &Value) -> JsonObject {
    let data = &row["data"];
    let location = &data["location"];
    let name = row["name"].as_str().filter(|n| !n.is_empty() && *n != "unavailable");
    let mut made = JsonObject::new();
    if let Some(name) = name {
        made.insert("name".into(), json!(name));
    }
    match kind {
        "track" => {
            made.insert("trackKind".into(), data["kind"].clone());
            if let Some(routing) = small(data.get("routing")) {
                made.insert("routing".into(), routing);
            }
            made.insert("armed".into(), data["armed"].clone());
            made.insert("monitoring".into(), data["monitoring"].clone());
            let mut mixer = JsonObject::new();
            for key in ["volume", "pan", "sends", "mute", "solo"] {
                if let Some(value) = data["mixer"].get(key).filter(|v| !v.is_null()) {
                    mixer.insert(key.into(), value.clone());
                }
            }
            made.insert("mixer".into(), json!(mixer));
        }
        "device" => {
            made.insert("className".into(), data["className"].clone());
            made.insert("position".into(), data["siblingOrder"].clone());
            if data["depth"].as_f64().is_some_and(|n| n > 0.0) {
                made.insert("insideRack".into(), json!(true));
            }
        }
        "clip" => {
            made.insert("clipKind".into(), data["clipKind"].clone());
            made.insert("lane".into(), location["lane"].clone());
            if location["sceneOrder"].is_number() {
                made.insert("scene".into(), location["sceneOrder"].clone());
            }
            made.insert("start".into(), data["start"].clone());
            made.insert("length".into(), data["length"].clone());
        }
        _ => {}
    }
    made
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DescribedWatch {
    pub changes: Vec<JsonObject>,
    pub more: usize,
}
pub fn describe_watch(diff: &JsonObject, before: &[JsonObject], after: &[JsonObject], limit: Option<usize>) -> DescribedWatch {
    let was = records(before);
    let now = records(after);
    let mut tracks = track_coordinates(before);
    tracks.extend(track_coordinates(after));
    let on = |row: Option<&Value>| {
        row.and_then(|r| r["data"]["parentSnapshotId"].as_str())
            .and_then(|p| tracks.get(p))
            .map(|n| json!({"on":n}).as_object().unwrap().clone())
            .unwrap_or_default()
    };
    let mut changes: Vec<(usize, JsonObject)> = vec![];
    for (item_index, item) in diff.get("items").and_then(Value::as_array).into_iter().flatten().enumerate() {
        let kind = js_text(item.get("kind"));
        if kind != "set" && !known(&kind) {
            continue;
        }
        let change = if item["type"] == "ambiguity" {
            let names = |ids: &Value, from: &IndexMap<String, Value>| {
                let mut counted: IndexMap<String, usize> = IndexMap::new();
                for id in ids.as_array().into_iter().flatten() {
                    if let Some(name) = from.get(&js_text(Some(id))).and_then(|r| r["name"].as_str()) {
                        *counted.entry(name.into()).or_default() += 1;
                    }
                }
                let mut listed: Vec<_> = counted
                    .iter()
                    .take(20)
                    .map(|(name, count)| if *count > 1 { format!("{name} ×{count}") } else { name.clone() })
                    .collect();
                if counted.len() > 20 {
                    listed.push(format!("+{} more names", counted.len() - 20))
                }
                listed
            };
            json!({"unclear":kind,"before":names(&item["beforeSnapshotIds"],&was),"after":names(&item["afterSnapshotIds"],&now)})
                .as_object()
                .unwrap()
                .clone()
        } else {
            if item["type"] != "change" {
                continue;
            }
            let Some(facets) = item["facets"].as_array() else { continue };
            let old = item["beforeSnapshotId"].as_str().and_then(|id| was.get(id));
            let current = item["afterSnapshotId"].as_str().and_then(|id| now.get(id));
            if facets.iter().any(|v| v == "added") && current.is_some() {
                let current = current.unwrap();
                let mut c = json!({"added":kind}).as_object().unwrap().clone();
                c.extend(made_of(&kind, current));
                c.extend(on(Some(current)));
                c.insert("order".into(), current["order"].clone());
                c
            } else if facets.iter().any(|v| v == "removed") && old.is_some() {
                let old = old.unwrap();
                let mut c = json!({"removed":kind,"name":old["name"]}).as_object().unwrap().clone();
                c.extend(on(Some(old)));
                c.insert("order".into(), old["order"].clone());
                c
            } else {
                let what:Vec<_>=item["details"].as_array().into_iter().flatten().filter_map(|d|{let path=d["path"].as_str()?;if DERIVED.is_match(path)||path.ends_with("Hash")||path.ends_with("Fingerprint")||path.ends_with("/hash"){return None}Some(json!({"what":path.strip_prefix("/data/").unwrap_or(path).replace('/',"."),"from":small(d.get("before")).unwrap_or(Value::Null),"to":small(d.get("after")).unwrap_or(Value::Null)}))}).collect();
                let renamed = facets.iter().any(|v| v == "renamed")
                    && old.is_some()
                    && current.is_some()
                    && old.unwrap().get("name") != current.unwrap().get("name");
                let row = current.or(old);
                if what.is_empty() && !renamed {
                    if kind == "device" && row.is_some() {
                        let mut c = json!({"changed":kind,"name":row.unwrap()["name"]}).as_object().unwrap().clone();
                        c.extend(on(row));
                        c.insert("what".into(), json!("its settings"));
                        c
                    } else {
                        continue;
                    }
                } else {
                    let mut c =
                        json!({"changed":kind,"name":row.map(|r|r["name"].clone()).unwrap_or(Value::Null)}).as_object().unwrap().clone();
                    c.extend(on(row));
                    if renamed {
                        c.insert("renamedFrom".into(), old.unwrap()["name"].clone());
                    }
                    if !what.is_empty() {
                        c.insert("what".into(), json!(&what[..what.len().min(12)]));
                    }
                    c
                }
            }
        };
        changes.push((item_index, change));
    }
    let gone: Vec<_> = changes.iter().filter(|(_, c)| c.contains_key("removed")).cloned().collect();
    for (id, gone) in gone {
        if let Some(fresh) = changes.iter().position(|(_, c)| {
            c.get("added") == gone.get("removed") && c.get("order") == gone.get("order") && c.get("on") == gone.get("on")
        }) {
            let mut rest = changes[fresh].1.clone();
            let added = rest.shift_remove("added").unwrap();
            rest.shift_remove("order");
            let mut replacement = json!({"changed":added}).as_object().unwrap().clone();
            replacement.extend(rest);
            replacement.insert("renamedFrom".into(), gone.get("name").cloned().unwrap_or(Value::Null));
            replacement.insert("note".into(), json!("renamed and changed"));
            changes[fresh].1 = replacement;
            changes.retain(|(old, _)| *old != id);
        }
    }
    let rank = |c: &JsonObject| {
        let kind = c.get("added").or(c.get("removed")).or(c.get("changed")).or(c.get("unclear")).and_then(Value::as_str).unwrap_or("");
        ["set", "track", "scene", "device", "clip", "locator"].iter().position(|v| *v == kind).unwrap_or(usize::MAX)
    };
    changes.sort_by_key(|(_, c)| rank(c));
    let mut changes: Vec<_> = changes
        .into_iter()
        .map(|(_, mut c)| {
            c.shift_remove("order");
            c
        })
        .collect();
    let limit = limit.unwrap_or(40);
    let more = changes.len().saturating_sub(limit);
    changes.truncate(limit);
    DescribedWatch { changes, more }
}
