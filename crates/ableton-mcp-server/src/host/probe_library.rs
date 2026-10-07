//! Owner-allowlisted offline library discovery, including the source WAL refusal.
use super::*;
use crate::{library_search::*, sqlite_reader::SqliteReader};
use kumi_common::js::json as js_json;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime},
};
fn unavailable(message: impl Into<String>) -> LibrarySearchError {
    LibraryUnavailable::new(message, json!({})).into()
}
fn allowlisted_file(file: &Value, root: &Value, label: &str) -> Result<PathBuf, LibrarySearchError> {
    if !is_non_empty_string(file, 4096) || !file.as_str().is_some_and(|s| Path::new(s).is_absolute() && !s.contains('\0')) {
        return Err(unavailable(format!("an explicit absolute {label} path is required")));
    }
    if !is_non_empty_string(root, 4096) || !root.as_str().is_some_and(|s| Path::new(s).is_absolute() && !s.contains('\0')) {
        return Err(unavailable("an explicit absolute allowlist root is required"));
    }
    let root = crate::command::resolve(root.as_str().unwrap()).map_err(|_| unavailable("the allowlist root cannot be resolved"))?;
    let root = Path::new(&root);
    let stat = fs::symlink_metadata(root).map_err(|_| unavailable("the allowlist root does not exist"))?;
    if !stat.is_dir() || stat.file_type().is_symlink() {
        return Err(unavailable("the allowlist root must be a real directory"));
    }
    let real_root = fs::canonicalize(root).map_err(|_| unavailable("the allowlist root cannot be resolved"))?;
    let file = crate::command::resolve(file.as_str().unwrap()).map_err(|_| unavailable(format!("the {label} cannot be resolved")))?;
    let file = Path::new(&file);
    let stat = fs::symlink_metadata(file).map_err(|_| unavailable(format!("the {label} does not exist")))?;
    if !stat.is_file() || stat.file_type().is_symlink() {
        return Err(unavailable(format!("the {label} must be a real regular file")));
    }
    if stat.len() > 128 * 1024 * 1024 {
        return Err(unavailable(format!("the {label} exceeds its 128 MiB bound")));
    }
    let real = fs::canonicalize(file).map_err(|_| unavailable(format!("the {label} cannot be resolved")))?;
    if !real.starts_with(real_root) {
        return Err(unavailable(format!("the {label} is outside the owner allowlist root")));
    }
    Ok(real)
}
/// A database read, by its path and what its file was then (size, mtime and on unix its inode, which a file put in its
/// place by a rename changes): each page of a search, and the next search, read and parse the file (up to 128 MiB)
/// again only once it changed. At most two are kept (the files and the plug-ins database), each
/// for two minutes after its last use.
pub(super) struct Read {
    path: PathBuf,
    file: FileStamp,
    reader: Arc<SqliteReader>,
    used: Instant,
}
#[derive(PartialEq)]
struct FileStamp {
    len: u64,
    modified: Option<SystemTime>,
    #[cfg(unix)]
    inode: u64,
}
impl FileStamp {
    fn of(stat: &fs::Metadata) -> Self {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Self {
            len: stat.len(),
            modified: stat.modified().ok(),
            #[cfg(unix)]
            inode: stat.ino(),
        }
    }
}
/// The databases this host's library searches read lately.
pub(super) type LibraryReads = Arc<Mutex<Vec<Read>>>;
const KEPT_FOR: Duration = Duration::from_secs(120);
fn forget_idle(read: &mut Vec<Read>) {
    read.retain(|kept| kept.used.elapsed() < KEPT_FOR);
}
async fn read_database(reads: &LibraryReads, path: &Path) -> Result<Arc<SqliteReader>, LibrarySearchError> {
    let unreadable =
        |e: std::io::Error| unavailable(format!("the library database is unreadable ({})", crate::delivery::io_error(&e, "open", &[path])));
    let file = FileStamp::of(&fs::metadata(path).map_err(unreadable)?);
    let cached = {
        let mut read = reads.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        forget_idle(&mut read);
        read.iter_mut().find(|kept| kept.path == path && kept.file == file).map(|kept| {
            kept.used = Instant::now();
            kept.reader.clone()
        })
    };
    let reader = match cached {
        Some(reader) => reader,
        None => {
            // Off the bridge's only thread: reading and parsing it takes a while.
            let owned = path.to_path_buf();
            let parsed = tokio::task::spawn_blocking(move || -> Result<SqliteReader, LibrarySearchError> {
                let bytes = fs::read(&owned).map_err(|e| {
                    unavailable(format!("the library database is unreadable ({})", crate::delivery::io_error(&e, "open", &[&owned])))
                })?;
                SqliteReader::new(bytes).map_err(|e| unavailable(format!("the library database is unreadable ({e})")))
            })
            .await
            .map_err(|e| unavailable(format!("the library database is unreadable ({e})")))??;
            let reader = Arc::new(parsed);
            let mut read = reads.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            read.retain(|kept| kept.path != path);
            read.push(Read { path: path.to_path_buf(), file, reader: reader.clone(), used: Instant::now() });
            while read.len() > 2 {
                read.remove(0);
            }
            drop(read);
            // Let it go once it's idle, even if no other search comes to notice.
            let reads = reads.clone();
            tokio::spawn(async move {
                tokio::time::sleep(KEPT_FOR).await;
                forget_idle(&mut reads.lock().unwrap_or_else(|poisoned| poisoned.into_inner()));
            });
            reader
        }
    };
    // The WAL is checked each time: it changes while the database file doesn't.
    if reader.wal_mode {
        let wal = PathBuf::from(format!("{}-wal", path.to_string_lossy()));
        if wal.exists() && fs::symlink_metadata(wal).is_ok_and(|m| m.len() > 0) {
            return Err(unavailable("the library database has uncheckpointed WAL frames; close Live and retry after a checkpoint rather than guessing at partial content"));
        }
    }
    Ok(reader)
}
/// `query` on a blocking thread: scanning, decoding, filtering and sorting a library's tables takes a while, and the
/// bridge's only thread answers everything else meanwhile.
async fn off_thread<T: Send + 'static>(
    query: impl FnOnce() -> Result<T, LibrarySearchError> + Send + 'static,
) -> Result<T, LibrarySearchError> {
    tokio::task::spawn_blocking(query).await.map_err(|e| unavailable(format!("the library search failed ({e})")))?
}
fn strings(params: &Value, key: &str) -> Option<Vec<String>> {
    params[key].as_array().map(|a| a.iter().filter_map(Value::as_str).map(str::to_owned).collect())
}
impl McpHost {
    pub async fn live_library_search_async(&self, id: &Value, params: &Value) -> Result<Value, LiveError> {
        let bounded = |v: &Value, max: usize| v.as_array().is_some_and(|a| a.len() <= max && a.iter().all(|v| is_non_empty_string(v, 256)));
        fn enum_list(v: &Value, allowed: &[&str]) -> Result<bool, LiveError> {
            let Some(a) = v.as_array().filter(|a| a.len() <= allowed.len()) else { return Ok(false) };
            for v in a {
                if !allowed.contains(&js_string(v)?.as_str()) {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        let valid = has_only(
            params,
            &[
                "database",
                "allowlistRoot",
                "pluginsDatabase",
                "mode",
                "query",
                "tags",
                "kinds",
                "sources",
                "vendors",
                "formats",
                "sort",
                "limit",
                "cursor",
            ],
        ) && match params.get("mode") {
            Some(v) => ["files", "plugins", "tags"].contains(&js_string(v)?.as_str()),
            None => true,
        } && params.get("query").is_none_or(|v| is_non_empty_string(v, 256) || v == "")
            && params.get("tags").is_none_or(|v| bounded(v, 8))
            && match params.get("kinds") {
                Some(v) => enum_list(v, LIBRARY_KINDS)?,
                None => true,
            }
            && params.get("sources").is_none_or(|v| bounded(v, 8))
            && params.get("vendors").is_none_or(|v| bounded(v, 8))
            && match params.get("formats") {
                Some(v) => enum_list(v, &["vst3", "vst2", "au", "clap", "unknown"])?,
                None => true,
            }
            && match params.get("sort") {
                Some(v) => ["useCount", "modified", "name"].contains(&js_string(v)?.as_str()),
                None => true,
            }
            && params.get("limit").is_none_or(|v| is_integer_in_range(v, 1.0, 100.0))
            && params.get("cursor").is_none_or(|v| is_non_empty_string(v, 4096));
        if !valid {
            return Ok(error(id, -32602, "database, allowlistRoot, and bounded query fields are invalid", None));
        }
        let mode = params.get("mode").filter(|v| !v.is_null()).cloned().unwrap_or(json!("files"));
        let result: Result<Value, LibrarySearchError> = async {
            let database = allowlisted_file(&params["database"], &params["allowlistRoot"], "library database")?;
            let kind = if mode == "plugins" {
                LibraryMode::Plugins
            } else if mode == "tags" {
                LibraryMode::Tags
            } else {
                LibraryMode::Files
            };
            let mut query = LibraryQuery::new(kind, params["limit"].as_f64().unwrap_or(50.0) as usize);
            query.host_values = Some(params.clone());
            query.query = params["query"].as_str().map(str::to_owned);
            query.tags = strings(params, "tags");
            query.sources = strings(params, "sources");
            query.vendors = strings(params, "vendors");
            query.cursor = params["cursor"].as_str().map(str::to_owned);
            query.kinds = params["kinds"].as_array().map(|a| a.iter().filter_map(|v| serde_json::from_value(v.clone()).ok()).collect());
            query.formats = params["formats"].as_array().map(|a| a.iter().filter_map(|v| serde_json::from_value(v.clone()).ok()).collect());
            query.sort = params.get("sort").map(|v| serde_json::from_value(v.clone()).unwrap_or(LibrarySort::Name));
            let (version, supported, page) = if mode == "plugins" {
                if params.get("pluginsDatabase").is_none() {
                    return Err(unavailable(
                        "plug-in inventory requires an explicit pluginsDatabase path (Live-plugins-*.db) inside the same allowlist root",
                    ));
                }
                let path = allowlisted_file(&params["pluginsDatabase"], &params["allowlistRoot"], "plug-in database")?;
                let reader = read_database(&self.library_reads, &path).await?;
                off_thread(move || {
                    Ok((
                        assert_supported_plugins_schema(&reader)?,
                        SUPPORTED_PLUGINS_SCHEMA_VERSIONS,
                        serde_json::to_value(query_library_plugins(&reader, &query)?).unwrap(),
                    ))
                })
                .await?
            } else {
                let reader = read_database(&self.library_reads, &database).await?;
                let tags = mode == "tags";
                off_thread(move || {
                    let version = assert_supported_files_schema(&reader)?;
                    let page = if tags {
                        serde_json::to_value(query_library_tag_vocabulary(&reader, &query)?).unwrap()
                    } else {
                        serde_json::to_value(query_library_files(&reader, &query)?).unwrap()
                    };
                    Ok((version, SUPPORTED_FILES_SCHEMA_VERSIONS, page))
                })
                .await?
            };
            let mut out = json!({"schema":LIBRARY_SEARCH_SCHEMA,"mode":mode,"databaseVersion":version,"supportedVersions":supported,"items":page["items"],"paging":page["paging"],"unavailable":{"similarity":"audio-similarity queries are unavailable in this build: the fe_values record schema is not enumerated (presence is noted, semantics are never guessed)","duplicates":"duplicate-sample queries are unavailable in this build: no duplicate-identity evidence is enumerated"},"privacy":{"note":"the database path, allowlist root, and raw filesystem paths are redacted from results; usage counts are opaque numbers; only module basenames are reported for plug-ins","redacted":["database","allowlistRoot","pluginsDatabase","plugin_modules.path"]},"provenance":{"bindingEvidence":"shape-probed first-hand on Live 12.4.5 (files database version 12300, macOS platform 2; plug-ins database version 1); unofficial undocumented schema, version-specific; results are discovery evidence and loadability still requires live_browser_inspect","supportedFilesVersions":SUPPORTED_FILES_SCHEMA_VERSIONS,"supportedPluginsVersions":SUPPORTED_PLUGINS_SCHEMA_VERSIONS}});
            if mode == "files" {
                if let Some(note) = page.get("tagVocabularyNote") {
                    out["tagVocabularyNote"] = note.clone();
                }
            }
            Ok(out)
        }
        .await;
        Ok(match result {
            Ok(v) => success_text(id, &v),
            Err(LibrarySearchError::Unavailable(e)) => {
                let mut out = json!({"unavailable":true,"reason":e.message});
                out.as_object_mut().unwrap().extend(e.details);
                response(id, json!({"content":[text_content(&js_json::stringify(&out))],"isError":true}))
            }
            Err(e) => adapter_tool_error(
                id,
                &LiveError::error(e.to_string()),
                "Library search is read-only and fail-closed; verify the database path and allowlist root.",
            ),
        })
    }
}
