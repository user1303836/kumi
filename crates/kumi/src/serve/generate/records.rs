//! A small in-memory index of the runtime's retained billing receipts. It is rebuilt once per
//! runtime; the ordered job snapshots remain the durable authority for whether a POST could occur.

use super::{refused, OpError};
use crate::serve::artifacts::open_regular;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::io::{BufReader, Read};
use std::path::Path;

#[derive(Clone)]
pub(super) struct Record {
    pub job_id: String,
    pub kind: String,
    pub reserved: u64,
    pub actual: Option<u64>,
}

pub(super) type Records = HashMap<String, Record>;

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

pub(super) fn remember(records: &mut Records, id: &str, receipt: &Value) -> Result<(), OpError> {
    let quote = receipt["quote_id"].as_str().ok_or_else(unavailable)?;
    let kind = receipt["kind"].as_str().ok_or_else(unavailable)?;
    let reserved = receipt["credits"]["reserved"].as_u64().ok_or_else(unavailable)?;
    let actual = receipt["credits"]["actual"].as_u64();
    if let Some(record) = records.get_mut(quote) {
        if record.job_id != id
            || record.kind != kind
            || record.reserved != reserved
            || record.actual.is_some_and(|known| actual.is_some_and(|actual| actual != known))
        {
            return Err(unavailable());
        }
        if actual.is_some() {
            record.actual = actual;
        }
    } else {
        records.insert(quote.to_owned(), Record { job_id: id.to_owned(), kind: kind.to_owned(), reserved, actual });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

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
