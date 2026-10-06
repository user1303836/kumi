//! Kumi's own id for each track of a Set (`kumi.track` in the track's Live data). It's saved with the Set and
//! kept through renames and moves, and the producer never sees it: Live marks no change and keeps no undo step
//! for it. A duplicate of a track carries its id, so a copy is given one of its own. Ids count within a project:
//! a track dragged in from another Set brings that Set's id, which matters only if it meets that id here.
use super::{
    connection::LiveConnection,
    context,
    views::{self, ViewHost},
};
use crate::core::{contracts::JsonObject, errors::RuntimeError};
use kumi_common::abort::Signal;
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
};

/// The key a track keeps its id under, with the track.
pub const TRACK_KEY: &str = "kumi.track";
/// The tracks that get ids: the Set's own (groups included), its returns and its main track.
const KINDS: [&str; 3] = ["track", "return-track", "main-track"];
/// The most tracks one write gives ids to.
const BATCH: usize = 1024;

/// Whether `id` is one Kumi gives: a ULID, 26 Crockford base-32 digits.
pub fn track_id(id: &str) -> bool {
    id.len() == 26 && id.bytes().all(|b| b"0123456789ABCDEFGHJKMNPQRSTVWXYZ".contains(&b))
}

/// A track as a pass reads it: its ref, its identity in Live, and the id it holds (if any).
#[derive(Debug, Clone, PartialEq)]
pub struct Seen {
    pub reference: String,
    pub identity: String,
    pub id: Option<String>,
}

/// A track's new id, with what it holds now (Live writes only if that's still so).
#[derive(Debug, Clone, PartialEq)]
pub struct Write {
    pub reference: String,
    pub identity: String,
    pub prior: Option<String>,
    pub id: String,
}

/// The new ids, in track order. A track without an id of Kumi's gets one. Of tracks sharing an id, the one
/// `known` to hold it (id → identity) keeps it, or else the first (Live places a duplicate right after its
/// original), and each other gets its own.
pub fn ids_to_write(tracks: &[Seen], known: &HashMap<String, String>, mut new_id: impl FnMut() -> String) -> Vec<Write> {
    fn valid(track: &Seen) -> Option<&str> {
        track.id.as_deref().filter(|id| track_id(id))
    }
    let mut keepers: HashMap<&str, &str> = HashMap::new();
    for track in tracks {
        if let Some(id) = valid(track) {
            let keeper = keepers.entry(id).or_insert(&track.identity);
            if known.get(id) == Some(&track.identity) {
                *keeper = &track.identity;
            }
        }
    }
    tracks
        .iter()
        .filter(|track| valid(track).is_none_or(|id| keepers[id] != track.identity))
        .map(|track| Write { reference: track.reference.clone(), identity: track.identity.clone(), prior: track.id.clone(), id: new_id() })
        .collect()
}

/// Whether rows read with `kumiTrack` show a track without an id of Kumi's, or two sharing one.
pub fn gaps(rows: &[JsonObject]) -> bool {
    let mut ids = std::collections::HashSet::new();
    rows.iter().any(|row| match row.get("kumiTrack").and_then(Value::as_str).filter(|id| track_id(id)) {
        Some(id) => !ids.insert(id),
        None => row.contains_key("kumiTrack"),
    })
}

/// A project's (or an unsaved Set's) tracks as the last pass left them.
#[derive(Default)]
struct Scope {
    /// The track list's revision when its ids were last made whole.
    revision: Option<String>,
    /// Each id, and the identity of the track that keeps it.
    known: HashMap<String, String>,
}

/// Keeps a Set's track ids whole, in the background.
#[derive(Default)]
pub struct TrackIds {
    /// Whether Live reports track ids, and the connection epoch that was asked in.
    reported: Cell<Option<(f64, bool)>>,
    scopes: RefCell<HashMap<String, Scope>>,
    running: Cell<bool>,
}

/// Clears `running` however a pass ends.
struct Running<'a>(&'a Cell<bool>);
impl Drop for Running<'_> {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

impl TrackIds {
    /// Whether this connection's Live reports track ids (asked once per connection epoch).
    pub fn reported(&self, epoch: Option<f64>) -> bool {
        matches!((self.reported.get(), epoch), (Some((asked, true)), Some(epoch)) if asked == epoch)
    }
    /// Whether a pass is due for `scope`: its track list changed since the last one (`revision`), or the
    /// tracks read show an id missing or shared (`gaps`).
    pub fn due(&self, scope: &str, revision: Option<&str>, gaps: bool) -> bool {
        !self.running.get()
            && (gaps || self.scopes.borrow().get(scope).is_none_or(|s| s.revision.is_none() || s.revision.as_deref() != revision))
    }
    /// Makes the Set's track ids whole. Asks once per connection whether Live reports them, reads every track's,
    /// and gives each track without one, and each copy, a new id. The ids go in one write (per 1,024 tracks),
    /// only when `writable` (never into a template). Nothing is read or written while another pass runs.
    pub async fn pass(&self, connection: &LiveConnection, scope: &str, writable: bool, signal: Signal) -> Result<(), RuntimeError> {
        if self.running.replace(true) {
            return Ok(());
        }
        let _running = Running(&self.running);
        let epoch = connection.epoch.get();
        if self.reported.get().map(|(asked, _)| asked) != epoch {
            // One track, asked for the id alone: a Live that doesn't report ids reads that one track whole, once.
            let probe =
                connection.call("live_discover", object(json!({"kind":"track","fields":["kumiTrack"],"limit":1})), signal.clone()).await?;
            let page = context::payload(&probe)?;
            let Some(row) = page.get("items").and_then(Value::as_array).and_then(|items| items.first()) else { return Ok(()) };
            let Some(epoch) = epoch else { return Ok(()) };
            self.reported.set(Some((epoch, row.get("kumiTrack").is_some())));
        }
        if !self.reported(epoch) {
            return Ok(());
        }
        let mut seen = vec![];
        let mut revision = None;
        for kind in KINDS {
            let read = views::pages(
                connection,
                object(json!({"kind":kind,"fields":["objectIdentity","kumiTrack"],"limit":100_000,"budget":connection.whole_budget()})),
                signal.clone(),
            )
            .await?;
            let page = context::payload(&read)?;
            if read.is_error == Some(true) || page.get("truncated") == Some(&Value::Bool(true)) {
                return Ok(());
            }
            if kind == "track" {
                revision = page.get("revision").and_then(Value::as_str).map(str::to_owned);
            }
            for row in page.get("items").and_then(Value::as_array).into_iter().flatten() {
                let (Some(reference), Some(identity)) = (row["ref"].as_str(), row["objectIdentity"].as_str()) else { continue };
                seen.push(Seen {
                    reference: reference.into(),
                    identity: identity.into(),
                    id: row["kumiTrack"].as_str().map(str::to_owned),
                });
            }
        }
        let writes = {
            let scopes = self.scopes.borrow();
            ids_to_write(&seen, scopes.get(scope).map(|s| &s.known).unwrap_or(&HashMap::new()), kumi_store::ids::new_id)
        };
        if writable && !writes.is_empty() {
            if !connection.has("live_data_preview") || !connection.has("live_data_apply") {
                return Ok(());
            }
            for batch in writes.chunks(BATCH) {
                write(connection, batch, signal.clone()).await?;
            }
        }
        let mut scopes = self.scopes.borrow_mut();
        let kept = scopes.entry(scope.into()).or_default();
        kept.revision = revision;
        let given: HashMap<&str, &str> = writes.iter().map(|w| (w.identity.as_str(), w.id.as_str())).collect();
        for track in &seen {
            let id = if writable { given.get(track.identity.as_str()).copied() } else { None }
                .or_else(|| track.id.as_deref().filter(|id| track_id(id) && !given.contains_key(track.identity.as_str())));
            if let Some(id) = id {
                kept.known.insert(id.into(), track.identity.clone());
            }
        }
        Ok(())
    }
}

/// One write of track ids: the first track in the call's own place, the rest as its entries. Live checks every
/// track's identity and what it holds before writing any.
async fn write(connection: &LiveConnection, batch: &[Write], signal: Signal) -> Result<(), RuntimeError> {
    let place = |w: &Write| json!({"trackRef":w.reference,"value":w.id,"expectedValue":w.prior,"expectedIdentity":w.identity});
    let mut args = place(&batch[0]);
    args["key"] = json!(TRACK_KEY);
    if batch.len() > 1 {
        args["entries"] = batch[1..].iter().map(place).collect();
    }
    let preview = context::payload(&connection.call("live_data_preview", object(args), signal.clone()).await?)?;
    let Some(transaction) = preview.get("transactionId").cloned() else {
        return Err(RuntimeError::plain("Live didn't take the track ids"));
    };
    let apply = json!({"transactionId":transaction,"confirmation":"apply","idempotencyKey":format!("track-ids-{}", batch[0].id)});
    let applied = context::payload(&connection.call("live_data_apply", object(apply), signal).await?)?;
    if applied.get("state") != Some(&json!("applied")) {
        return Err(RuntimeError::plain("Live didn't take the track ids"));
    }
    Ok(())
}

fn object(value: Value) -> JsonObject {
    match value {
        Value::Object(map) => map,
        _ => JsonObject::new(),
    }
}
