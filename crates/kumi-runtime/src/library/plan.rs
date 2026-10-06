use super::{
    learn::{plugin_preset_folders, set_folders, LearnPlan},
    sources::{below, current_platform, homedir, join, library_sources, recent_sets, SourceKind, SourceOptions},
    store::read_json,
};
use indexmap::IndexSet;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{path::Path, sync::LazyLock};
use tokio::io::AsyncReadExt;
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanOptions {
    pub dir: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folders: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projects_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sources: Option<SourceOptions>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub find_sets: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workers: Option<usize>,
}
pub fn remembered_file(dir: &str) -> String {
    join(dir, "folders.json")
}
pub async fn remembered_folders(dir: &str) -> Vec<String> {
    read_json::<Value>(Path::new(&remembered_file(dir)))
        .await
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect()
}
/// Only the record's first 2048 bytes are read, matching the bounded source lookup.
pub async fn kumi_sets(projects_dir: Option<&str>) -> Vec<String> {
    static ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[0-9a-f]{32}$").unwrap());
    static PATH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""path":"((?:[^"\\]|\\.)*)""#).unwrap());
    let Some(dir) = projects_dir.filter(|s| !s.is_empty()) else { return vec![] };
    let Ok(mut entries) = tokio::fs::read_dir(dir).await else { return vec![] };
    let mut names = vec![];
    while let Ok(Some(entry)) = entries.next_entry().await {
        names.push(entry.file_name().to_string_lossy().into_owned());
    }
    // Node's readdir (unlike opendir) returns names in lexical order.
    names.sort();
    let mut found = vec![];
    for name in names.into_iter().filter(|name| ID.is_match(name)) {
        let Ok(mut handle) = tokio::fs::File::open(join(&join(dir, &name), "last-seen.json")).await else { continue };
        let mut buffer = [0; 2048];
        let Ok(bytes) = handle.read(&mut buffer).await else { continue };
        let text = String::from_utf8_lossy(&buffer[..bytes]);
        let Some(path) = PATH.captures(&text).and_then(|m| m.get(1)) else { continue };
        if path.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<String>(&format!("\"{}\"", path.as_str())) else { continue };
        if value.ends_with(".als") && Path::new(&value).exists() {
            found.push(value);
        }
    }
    found
}
pub async fn plan_learning(options: PlanOptions) -> LearnPlan {
    let remembered = remembered_folders(&options.dir).await;
    let original_sources = options.sources.unwrap_or_default();
    let home = original_sources.home.clone().unwrap_or_else(homedir);
    let platform = original_sources.platform.clone().unwrap_or_else(|| current_platform().into());
    let mut source_options = original_sources.clone();
    source_options.folders = Some(options.folders.unwrap_or_default().into_iter().chain(remembered).collect());
    let sources = library_sources(&source_options);
    let others: Vec<_> = sources
        .iter()
        .filter(|s| matches!(s.kind, SourceKind::Pack | SourceKind::Core | SourceKind::Splice))
        .map(|s| s.path.clone())
        .collect();
    let find_sets = options.find_sets != Some(false);
    let recent: Vec<_> = if find_sets {
        recent_sets(&original_sources)
            .into_iter()
            .chain(kumi_sets(options.projects_dir.as_deref()).await)
            .collect::<IndexSet<_>>()
            .into_iter()
            .filter(|path| !others.iter().any(|folder| below(path, folder).is_some()))
            .collect()
    } else {
        vec![]
    };
    LearnPlan {
        dir: options.dir,
        sources,
        set_folders: if find_sets { set_folders(&recent, Some(&home), Some(&platform)) } else { vec![] },
        set_files: recent,
        plugin_presets: if find_sets {
            plugin_preset_folders(Some(&home), Some(&platform)).into_iter().filter(|folder| Path::new(folder).exists()).collect()
        } else {
            vec![]
        },
        workers: options.workers,
    }
}
