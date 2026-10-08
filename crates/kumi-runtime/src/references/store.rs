//! Measured references, kept: each reference is measured once, then read back by what was asked for or by its name.
//! One file per reference (`<key>.json`), and the downloaded audio beside them.

use crate::listening::checklist::Profile;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;

/// A track a reference was measured from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KeptTrack {
    pub artist: String,
    pub title: String,
    /// The link or file its audio came from.
    pub source: String,
}

/// A measured reference.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeptReference {
    pub version: u32,
    /// What was asked for, folded: how it's found again.
    pub key: String,
    pub name: String,
    pub kind: String,
    pub tracks: Vec<KeptTrack>,
    pub profile: Profile,
    pub at: i64,
}

pub struct ReferenceStore {
    folder: PathBuf,
}

impl ReferenceStore {
    pub fn new(folder: impl Into<PathBuf>) -> Self {
        Self { folder: folder.into() }
    }
    pub fn folder(&self) -> &Path {
        &self.folder
    }
    /// Where fetched audio is kept.
    pub fn audio_folder(&self) -> PathBuf {
        self.folder.join("audio")
    }
    /// What was asked for, as a key: words without case or punctuation, a link or path as it is (made safe).
    pub fn key(what: &str) -> String {
        let what = what.trim();
        let folded: String = if what.contains("://") || what.contains('/') || what.contains('\\') {
            what.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
        } else {
            what.to_lowercase().chars().map(|c| if c.is_alphanumeric() { c } else { '-' }).collect()
        };
        let mut key = String::new();
        for c in folded.chars() {
            if c == '-' && (key.is_empty() || key.ends_with('-')) {
                continue;
            }
            key.push(c);
        }
        let key = key.trim_end_matches('-').to_string();
        // Long links keep their ends, where the ids are.
        if key.chars().count() > 120 {
            key.chars().skip(key.chars().count() - 120).collect()
        } else if key.is_empty() {
            "reference".into()
        } else {
            key
        }
    }
    fn file(&self, key: &str) -> PathBuf {
        self.folder.join(format!("{key}.json"))
    }
    /// A kept reference: by what was asked for, else by its name.
    pub async fn load(&self, what: &str) -> Option<KeptReference> {
        let key = Self::key(what);
        if let Some(kept) = read(&self.file(&key)).await {
            return Some(kept);
        }
        let wanted = super::sources::folded(what);
        let mut entries = tokio::fs::read_dir(&self.folder).await.ok()?;
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "json") {
                if let Some(kept) = read(&path).await.filter(|kept| super::sources::folded(&kept.name) == wanted) {
                    return Some(kept);
                }
            }
        }
        None
    }
    pub async fn save(&self, kept: &KeptReference) -> Result<(), String> {
        let mut builder = tokio::fs::DirBuilder::new();
        builder.recursive(true);
        builder.create(&self.folder).await.map_err(|error| error.to_string())?;
        let temporary = self.folder.join(format!(".reference-{}", uuid::Uuid::new_v4()));
        let written = async {
            let mut file = tokio::fs::File::create(&temporary).await?;
            file.write_all(&serde_json::to_vec_pretty(kept).expect("a reference serializes")).await?;
            file.flush().await?;
            drop(file);
            tokio::fs::rename(&temporary, self.file(&kept.key)).await
        }
        .await;
        if let Err(error) = written {
            let _ = tokio::fs::remove_file(&temporary).await;
            return Err(error.to_string());
        }
        Ok(())
    }
}

async fn read(path: &Path) -> Option<KeptReference> {
    let bytes = tokio::fs::read(path).await.ok()?;
    serde_json::from_slice::<KeptReference>(&bytes).ok().filter(|kept| kept.version == 1)
}
