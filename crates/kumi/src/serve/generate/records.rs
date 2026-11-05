//! A small in-memory index of the runtime's retained billing receipts. New creates require a
//! complete scan; GET-only recovery remembers live billing without reading unrelated jobs.
//! Late scan results merge monotonically, never replacing live charges with older snapshots.

use super::{refused, OpError};
use crate::serve::artifacts::open_regular;
use serde::Deserialize;
use serde_json::Value;
use std::collections::{hash_map::Entry, HashMap};
use std::io::{BufReader, Read};
use std::path::Path;

pub(super) struct Record {
    pub job_id: String,
    pub kind: String,
    pub reserved: u64,
    pub actual: Option<u64>,
}

impl Record {
    fn update(&mut self, id: &str, kind: &str, reserved: u64, actual: Option<u64>) -> Result<(), OpError> {
        if self.job_id != id
            || self.kind != kind
            || self.reserved != reserved
            || self.actual.is_some_and(|known| actual.is_some_and(|actual| actual != known))
        {
            return Err(unavailable());
        }
        if actual.is_some() {
            self.actual = actual;
        }
        Ok(())
    }
}

pub(super) type Records = HashMap<String, Record>;

#[derive(Default)]
pub(super) struct Index {
    loaded: bool,
    conflicted: bool,
    receipts: Records,
}

impl Index {
    pub(super) fn loaded(&self) -> Result<bool, OpError> {
        if self.conflicted {
            return Err(unavailable());
        }
        Ok(self.loaded)
    }

    /// Only a complete, consistent index proves absence or the lack of an unsettled overrun.
    pub(super) fn complete(&self) -> Result<&Records, OpError> {
        if !self.loaded()? {
            return Err(unavailable());
        }
        Ok(&self.receipts)
    }

    pub(super) fn merge(&mut self, loaded: Records) -> Result<(), OpError> {
        for (quote, record) in loaded {
            match self.receipts.entry(quote) {
                Entry::Occupied(mut entry) => {
                    if let Err(error) = entry.get_mut().update(&record.job_id, &record.kind, record.reserved, record.actual) {
                        // An inconsistent late scan must also block future creates, not only its caller.
                        self.conflicted = true;
                        return Err(error);
                    }
                }
                Entry::Vacant(entry) => {
                    entry.insert(record);
                }
            }
        }
        self.loaded = true;
        self.loaded().map(|_| ())
    }

    pub(super) fn remember(&mut self, id: &str, receipt: &Value) -> Result<(), OpError> {
        let result = remember(&mut self.receipts, id, receipt);
        self.conflicted |= result.is_err();
        result
    }
}

#[derive(Deserialize)]
#[serde(tag = "kind")]
enum Saved {
    #[serde(rename = "generate")]
    Generate { job_id: String, result: Option<Receipt> },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct Receipt {
    quote_id: String,
    kind: String,
    provider: String,
    submission: String,
    credits: Credits,
}

#[derive(Deserialize)]
struct Credits {
    reserved: u64,
    actual: Option<u64>,
}

fn unavailable() -> OpError {
    refused("ledger_unavailable", "A retained generation receipt could not be read safely; no new generation was authorized.")
}

pub(super) fn job_id(id: &str) -> bool {
    id.len() <= 64
        && id.strip_prefix('j').and_then(|id| id.split_once('-')).is_some_and(|(time, number)| {
            !time.is_empty() && !number.is_empty() && time.bytes().chain(number.bytes()).all(|byte| byte.is_ascii_digit())
        })
}

pub(super) fn load(state: &Path) -> Result<Records, OpError> {
    let folder = state.join("jobs");
    let entries = match std::fs::read_dir(&folder) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(HashMap::new()),
        Err(_) => return Err(unavailable()),
    };
    let mut records = HashMap::new();
    for entry in entries {
        let entry = entry.map_err(|_| unavailable())?;
        let path = entry.path();
        let Some(id) = path.file_stem().and_then(|name| name.to_str()).filter(|id| job_id(id)) else { continue };
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let file = open_regular(&path).map_err(|_| unavailable())?;
        let mut input = BufReader::new(file.take(16 * 1024 * 1024 + 1));
        let saved: Saved = serde_json::from_reader(&mut input).map_err(|_| unavailable())?;
        if input.into_inner().limit() == 0 {
            return Err(unavailable());
        }
        let Saved::Generate { job_id: saved_id, result: Some(receipt) } = saved else { continue };
        if saved_id != id
            || receipt.provider != "mirelo"
            || !matches!(receipt.kind.as_str(), "sfx" | "audio_to_midi")
            || !matches!(receipt.submission.as_str(), "unknown" | "submitted" | "not_submitted")
            || (receipt.credits.actual.is_some() && receipt.submission != "submitted")
        {
            return Err(unavailable());
        }
        let previous = records.insert(
            receipt.quote_id,
            Record { job_id: saved_id, kind: receipt.kind, reserved: receipt.credits.reserved, actual: receipt.credits.actual },
        );
        if previous.is_some() {
            return Err(unavailable());
        }
    }
    Ok(records)
}

fn remember(records: &mut Records, id: &str, receipt: &Value) -> Result<(), OpError> {
    let quote = receipt["quote_id"].as_str().ok_or_else(unavailable)?;
    let kind = receipt["kind"].as_str().ok_or_else(unavailable)?;
    let reserved = receipt["credits"]["reserved"].as_u64().ok_or_else(unavailable)?;
    let actual = receipt["credits"]["actual"].as_u64();
    if let Some(record) = records.get_mut(quote) {
        record.update(id, kind, reserved, actual)?;
    } else {
        records.insert(quote.to_owned(), Record { job_id: id.to_owned(), kind: kind.to_owned(), reserved, actual });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn receipt(quote: &str, actual: Option<u64>) -> Value {
        serde_json::json!({"quote_id":quote, "kind":"sfx", "credits":{"reserved":15, "actual":actual}})
    }

    fn record(id: &str, actual: Option<u64>) -> Record {
        Record { job_id: id.into(), kind: "sfx".into(), reserved: 15, actual }
    }

    #[test]
    fn live_charges_survive_initial_and_late_scan_completion() {
        let mut index = Index::default();
        // The blocking scan captured these before live GET/checkpoint updates, but returns later.
        let scanned = Records::from([("q-live".into(), record("j1-1", None)), ("q-disk".into(), record("j1-2", Some(20)))]);
        let late_scan = Records::from([("q-live".into(), record("j1-1", None))]);
        index.remember("j1-1", &receipt("q-live", Some(16))).unwrap();
        index.remember("j1-3", &receipt("q-new", Some(12))).unwrap();
        assert!(!index.loaded().unwrap());
        assert_eq!(index.complete().err().unwrap().code(), "ledger_unavailable");

        index.merge(scanned).unwrap();
        assert!(index.loaded().unwrap());
        assert_eq!(index.complete().unwrap()["q-live"].actual, Some(16));
        assert_eq!(index.complete().unwrap()["q-new"].actual, Some(12));
        assert_eq!(index.complete().unwrap()["q-disk"].actual, Some(20));
        index.remember("j1-1", &receipt("q-live", None)).unwrap();
        index.merge(late_scan).unwrap();
        assert_eq!(index.complete().unwrap()["q-live"].actual, Some(16));
        assert_eq!(index.complete().unwrap().len(), 3);
    }

    #[test]
    fn a_scan_can_add_a_charge_to_an_unknown_live_receipt() {
        let mut index = Index::default();
        index.remember("j1-1", &receipt("q-one", None)).unwrap();
        index.merge(Records::from([("q-one".into(), record("j1-1", Some(16)))])).unwrap();
        assert_eq!(index.complete().unwrap()["q-one"].actual, Some(16));
    }

    #[test]
    fn conflicting_initial_or_late_scans_keep_future_creates_closed() {
        for initialized in [false, true] {
            for field in ["actual", "job_id", "kind", "reserved"] {
                let mut index = Index::default();
                if initialized {
                    index.merge(Records::new()).unwrap();
                }
                index.remember("j1-1", &receipt("q-one", Some(16))).unwrap();
                let mut conflicting = record("j1-1", Some(16));
                match field {
                    "actual" => conflicting.actual = Some(12),
                    "job_id" => conflicting.job_id = "j1-2".into(),
                    "kind" => conflicting.kind = "audio_to_midi".into(),
                    "reserved" => conflicting.reserved = 20,
                    _ => unreachable!(),
                }
                assert_eq!(
                    index.merge(Records::from([("q-one".into(), conflicting)])).unwrap_err().code(),
                    "ledger_unavailable",
                    "{initialized} {field}"
                );
                assert_eq!(index.receipts["q-one"].actual, Some(16));
                assert_eq!(index.loaded().unwrap_err().code(), "ledger_unavailable");
                assert_eq!(index.complete().err().unwrap().code(), "ledger_unavailable");
                // A retry or another scan finishing successfully cannot clear contradictory evidence.
                assert_eq!(index.merge(Records::new()).unwrap_err().code(), "ledger_unavailable");
            }
        }
    }

    #[test]
    fn conflicting_live_billing_cannot_be_hidden_by_a_later_scan() {
        let mut index = Index::default();
        index.merge(Records::new()).unwrap();
        index.remember("j1-1", &receipt("q-one", Some(12))).unwrap();
        assert_eq!(index.remember("j1-1", &receipt("q-one", Some(16))).unwrap_err().code(), "ledger_unavailable");
        assert_eq!(index.merge(Records::new()).unwrap_err().code(), "ledger_unavailable");
        assert_eq!(index.complete().err().unwrap().code(), "ledger_unavailable");
    }

    #[test]
    fn a_valid_json_prefix_cannot_hide_a_receipt_larger_than_the_read_bound() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let jobs = root.join("jobs");
        std::fs::create_dir(&jobs).unwrap();
        let mut file = std::fs::File::create(jobs.join("j1-1.json")).unwrap();
        file.write_all(br#"{"kind":"listen"}"#).unwrap();
        for _ in 0..2048 {
            file.write_all(&[b' '; 8192]).unwrap();
        }
        drop(file);
        assert_eq!(load(&root).err().unwrap().code(), "ledger_unavailable");
    }
}
