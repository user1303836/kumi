//! Sample and parameter services used while preparing a Live change.
use super::{
    changes::{ChangeContext, ParameterRange, SampleFile, SampleSelector},
    parameters::Parameters,
    samples::{self, Sample},
};
use crate::{
    core::errors::RuntimeError,
    library::sources::{below, homedir, library_sources, SourceKind, SourceOptions},
};
use async_trait::async_trait;
use indexmap::IndexMap;
use kumi_common::{abort::Signal, path::network_or_device};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    cell::RefCell,
    collections::HashSet,
    path::{Path, PathBuf},
};

#[derive(Default)]
pub struct SampleBank {
    pub samples: RefCell<IndexMap<String, Sample>>,
    pub picked: RefCell<HashSet<String>>,
    /// Live's own Places (the User Library and the folders added in Live's Browser), when a test names them; read
    /// from Live's Library.cfg otherwise.
    pub places: RefCell<Option<Vec<String>>>,
    /// Where copies of Places' sounds on a share go; Kumi's own cache otherwise.
    pub cache: RefCell<Option<PathBuf>>,
}
/// Live's own Places: the User Library and the folders the producer added in Live, as Library.cfg (and Live's
/// indexer) list them.
fn live_places() -> Vec<String> {
    library_sources(&SourceOptions::default())
        .into_iter()
        .filter(|source| matches!(source.kind, SourceKind::Place | SourceKind::UserLibrary))
        .map(|source| source.path)
        .collect()
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
impl SampleBank {
    /// The file a change imports for `path`: one find_samples gave, or an audio file on this computer. A sound on a
    /// network share is imported only from inside one of Live's own Places, and from a copy here: the bridge refuses
    /// shares (opening one sends Windows' credentials to its host), and Live opens its Places itself. Any other share
    /// is never opened.
    pub async fn sample(&self, path: &str) -> Option<SampleFile> {
        let file = self.located(path)?;
        self.deliverable(file).await
    }
    async fn deliverable(&self, file: SampleFile) -> Option<SampleFile> {
        if !network_or_device(&file.path) {
            return Some(file);
        }
        let places = self.places.borrow().clone().unwrap_or_else(live_places);
        if !places.iter().any(|place| below(&file.path, place).is_some()) {
            return None;
        }
        self.local_copy(&file.path).await
    }
    /// A copy of a Place's sound on a share, kept in the cache while it's the same file (size and mtime).
    async fn local_copy(&self, source: &str) -> Option<SampleFile> {
        let name = Path::new(source).file_name()?.to_owned();
        let cache = self.cache.borrow().clone().unwrap_or_else(kumi_cache);
        let folder = cache.join(&hex::encode(Sha256::digest(source.as_bytes()))[..16]);
        let copy = folder.join(&name);
        let (from, into, to) = (PathBuf::from(source), folder.clone(), copy.clone());
        // Off Kumi's thread: it's read over the network.
        tokio::task::spawn_blocking(move || -> std::io::Result<()> {
            let stat = std::fs::metadata(&from)?;
            if !stat.is_file() {
                return Err(std::io::Error::other("not a file"));
            }
            if std::fs::metadata(&to).is_ok_and(|kept| kept.len() == stat.len() && kept.modified().ok() == stat.modified().ok()) {
                return Ok(());
            }
            std::fs::create_dir_all(&into)?;
            let partial = into.join(format!(".{}.part", std::process::id()));
            std::fs::copy(&from, &partial)?;
            if let Ok(modified) = stat.modified() {
                std::fs::File::options().write(true).open(&partial)?.set_modified(modified)?;
            }
            std::fs::rename(&partial, &to)
        })
        .await
        .ok()?
        .ok()?;
        Some(SampleFile { path: copy.to_string_lossy().into_owned(), folder: folder.to_string_lossy().into_owned() })
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
    pub async fn pick(&self, selector: SampleSelector, signal: Signal) -> Result<Option<SampleFile>, RuntimeError> {
        let named: Vec<_> = selector.folders.iter().filter_map(|folder| samples::folder_path(folder, None)).collect();
        let found = samples::find_samples(samples::FindSamplesOptions {
            folders: if named.is_empty() { samples::default_sample_folders(None, None, None) } else { named },
            random: selector.random || selector.words.is_empty(),
            words: selector.words,
            limit: 50,
            signal: Some(signal),
        })
        .await?;
        let choice = found.samples.into_iter().find(|sample| !self.picked.borrow().contains(&sample.path));
        let Some(choice) = choice else { return Ok(None) };
        self.picked.borrow_mut().insert(choice.path.clone());
        let file = SampleFile { path: choice.path.clone(), folder: choice.folder.clone() };
        {
            let mut samples = self.samples.borrow_mut();
            samples.shift_remove(&choice.path);
            samples.insert(choice.path.clone(), choice);
        }
        Ok(self.deliverable(file).await)
    }
}
pub struct PreparationContext<'a> {
    pub parameters: &'a Parameters,
    pub samples: &'a SampleBank,
    pub signal: Signal,
}
#[async_trait(?Send)]
impl ChangeContext for PreparationContext<'_> {
    async fn sample(&self, path: &str) -> Option<SampleFile> {
        self.samples.sample(path).await
    }
    async fn parameters(&self, device_ref: &str) -> Result<Vec<ParameterRange>, RuntimeError> {
        self.read(device_ref, false).await
    }
    async fn ranges(&self, device_ref: &str) -> Result<Vec<ParameterRange>, RuntimeError> {
        self.read(device_ref, true).await
    }
    async fn pick(&self, selector: SampleSelector) -> Result<Option<SampleFile>, RuntimeError> {
        self.samples.pick(selector, self.signal.clone()).await
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
