//! Sample and parameter services used while preparing a Live change.
use super::{
    changes::{ChangeContext, ParameterRange, SampleFile, SampleSelector},
    parameters::Parameters,
    samples::{self, Sample},
};
use crate::{core::errors::RuntimeError, library::sources::homedir};
use async_trait::async_trait;
use indexmap::IndexMap;
use kumi_common::abort::Signal;
use serde_json::json;
use std::{cell::RefCell, collections::HashSet, path::Path};

#[derive(Default)]
pub struct SampleBank {
    pub samples: RefCell<IndexMap<String, Sample>>,
    pub picked: RefCell<HashSet<String>>,
}
impl SampleBank {
    pub fn sample(&self, path: &str) -> Option<SampleFile> {
        if let Some(sample) = self.samples.borrow().get(path) {
            return Some(SampleFile { path: sample.path.clone(), folder: sample.folder.clone() });
        }
        let full = if path.starts_with("~/") || path.starts_with("~\\") { format!("{}{}", homedir(), &path[1..]) } else { path.into() };
        // A share named by the model isn't looked at: opening it sends Windows' credentials to its host.
        if kumi_common::path::network_or_device(&full) {
            return None;
        }
        let path = Path::new(&full);
        if !path.is_absolute()
            || !path
                .extension()
                .and_then(|s| s.to_str())
                .is_some_and(|ext| samples::SAMPLE_EXTENSIONS.contains(&format!(".{}", ext.to_lowercase()).as_str()))
        {
            return None;
        }
        if !std::fs::metadata(path).ok()?.is_file() {
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
        let mut samples = self.samples.borrow_mut();
        samples.shift_remove(&choice.path);
        samples.insert(choice.path.clone(), choice);
        Ok(Some(file))
    }
}
pub struct PreparationContext<'a> {
    pub parameters: &'a Parameters,
    pub samples: &'a SampleBank,
    pub signal: Signal,
}
#[async_trait(?Send)]
impl ChangeContext for PreparationContext<'_> {
    fn sample(&self, path: &str) -> Option<SampleFile> {
        self.samples.sample(path)
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
