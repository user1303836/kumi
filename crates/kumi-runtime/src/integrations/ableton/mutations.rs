//! Preview/apply changes and retire the references whose positions they moved.
use super::{
    change_context::{PreparationContext, SampleBank},
    changes::{laid_over, new_record, ChangeKind},
    connection::{ReadError, NO_CURRENT_LIVE},
    context::{self, ObservationError},
    history::{uncertain, Restore},
    observation::Observer,
    options::AbletonOptions,
    parameters::{ChangeOutcome, Parameters},
    views::ViewHost,
};
use crate::core::{contracts::*, errors::RuntimeError};
use indexmap::IndexSet;
use kumi_common::{
    abort::{self, Signal, SignalExt},
    js::{
        json::stringify,
        number,
        string::{head, utf16_len},
    },
};
use regex::Regex;
use serde_json::{json, Value};
use std::{cell::RefCell, collections::HashSet, rc::Rc, sync::LazyLock};

pub struct Mutations {
    pub parameters: Rc<Parameters>,
    pub observer: Rc<Observer>,
    pub options: Rc<AbletonOptions>,
    pub samples: SampleBank,
    pub(crate) copied: RefCell<HashSet<String>>,
}
impl Mutations {
    pub fn new(parameters: Rc<Parameters>, observer: Rc<Observer>, options: Rc<AbletonOptions>) -> Self {
        Self { parameters, observer, options, samples: SampleBank::default(), copied: RefCell::new(HashSet::new()) }
    }
    pub fn supported(&self, since: Option<&str>) -> bool {
        since.is_none_or(|since| super::bridge_version::at_least(self.parameters.history.connection.version().as_deref(), since))
    }
    pub fn too_old(&self, since: Option<&str>) -> String {
        format!(
            "That needs the Ableton bridge {} or later; this one is {}. Tell the producer to update it (kumi doctor says how).",
            since.unwrap_or("undefined"),
            self.parameters.history.connection.version().as_deref().unwrap_or("older")
        )
    }
    pub async fn change(&self, kind: &ChangeKind, named: JsonObject, original: Signal, settled: bool) -> ChangeOutcome {
        // A change acting on a track's devices reads them again first, so what Kumi holds of them is current where it
        // acts, and is refused if a device it names isn't the one this turn showed. One that changes the devices
        // themselves (or renames one) has them read again on the next turn too, since Live tells of no device renamed.
        let acts = matches!(kind.family, ChangeFamily::Device | ChangeFamily::Parameter) || kind.tool == "set_chain_mixer";
        let changes = matches!(kind.family, ChangeFamily::Device | ChangeFamily::Rename);
        let (tracks, devices) =
            if acts || changes { self.observer.devices_named(&named, kind.family == ChangeFamily::Device) } else { (vec![], vec![]) };
        if acts {
            if let Err(text) = self.observer.refresh_devices(&tracks, &devices, original.clone()).await {
                return ChangeOutcome::error(text);
            }
        }
        let outcome = match self.try_change(kind, named, original, settled).await {
            Ok(result) => result,
            Err(ReadError::Observation(error)) => ChangeOutcome::error(error.0),
            Err(ReadError::Other(_)) => {
                ChangeOutcome::error("The change failed before anything happened in Live; discover again, then retry.")
            }
        };
        if changes {
            self.observer.devices_changed(&tracks);
        }
        outcome
    }
    async fn try_change(&self, kind: &ChangeKind, named: JsonObject, original: Signal, settled: bool) -> Result<ChangeOutcome, ReadError> {
        let history = &self.parameters.history;
        let connection = &history.connection;
        let signal = abort::any([original, connection.lifetime.clone()]);
        let input = context::object(&connection.references.borrow().lengthen(&json!(named)))?;
        let lease = connection.lease.get();
        signal.check()?;
        if !connection.available.get() || connection.lost.get() || connection.epoch.get().is_none() || connection.tools().is_none() {
            return Err(observation(NO_CURRENT_LIVE));
        }
        connection.ensure_catalog(signal.clone()).await?;
        connection.assert_lease(lease, &signal)?;
        if !connection.has(&kind.preview) || !connection.has(&kind.apply) {
            connection.guard_epoch(signal.clone(), connection.epoch.get().unwrap(), lease).await?;
            connection.tools().unwrap().refresh(signal.clone()).await?;
            connection.assert_lease(lease, &signal)?;
        }
        if !connection.has(&kind.preview) || !connection.has(&kind.apply) {
            return Err(observation(kind.unavailable.as_deref().unwrap_or("That change isn't available for the open Set right now")));
        }
        if !self.supported(kind.since.as_deref()) {
            return Err(observation(&self.too_old(kind.since.as_deref())));
        }
        if history.changes_this_turn.get() >= 5_000 {
            return Err(observation("That's 5000 changes in one answer; carry on in the next one"));
        }
        connection.references.borrow().require_fresh_references(&input)?;
        // Notes written in Kumi's notation become the notes Live takes; a mistake in it comes back as the change's error.
        let input = match super::notes::expand(&kind.tool, input, connection, self.observer.tempo.get(), &signal).await {
            Ok(input) => input,
            Err(text) => return Ok(ChangeOutcome::error(text)),
        };
        if kind.family == ChangeFamily::Parameter && self.parameters.fast_on() {
            return Ok(self.parameters.fast_parameters(kind, &input, signal).await?);
        }
        let prepared = if kind.has("prepare") {
            match kind
                .prepare(&input, &PreparationContext { parameters: &self.parameters, samples: &self.samples, signal: signal.clone() })
                .await?
            {
                Ok(args) => args,
                Err(text) => return Ok(ChangeOutcome::error(text)),
            }
        } else {
            input
        };
        connection.assert_lease(lease, &signal)?;
        let epoch = connection.epoch.get().ok_or_else(|| observation(NO_CURRENT_LIVE))?;
        if !settled {
            connection.guard_epoch(signal.clone(), epoch, lease).await?;
        }
        let args = self.append_at_end(kind, prepared, signal.clone()).await?;
        connection.assert_lease(lease, &signal)?;
        let previewed = connection.call(&kind.preview, args.clone(), signal.clone()).await?;
        connection.assert_lease(lease, &signal)?;
        if previewed.is_error == Some(true) {
            let text = stringify(&serde_json::to_value(&previewed).unwrap());
            let more = if kind.has("explain") {
                kind.explain(&text, &args, &PreparationContext { parameters: &self.parameters, samples: &self.samples, signal })
                    .await
                    .ok()
                    .flatten()
                    .filter(|s| !s.is_empty())
            } else {
                None
            };
            return Ok(ChangeOutcome::error(format!("{text}{}", more.map(|s| format!(" {s}")).unwrap_or_default())));
        }
        let mut preview = context::payload(&previewed)?;
        if let Some(given) = preview.get("epoch") {
            connection.assert_epoch(Some(given), epoch)?;
        }
        let transaction = preview.get("transactionId").and_then(Value::as_str).filter(|s| !s.is_empty() && utf16_len(s) <= 256);
        let confirmation = preview.get("confirmation").and_then(Value::as_str).filter(|s| !s.is_empty() && utf16_len(s) <= 512);
        let (Some(transaction), Some(confirmation)) = (transaction, confirmation) else {
            return Err(observation("The bridge's preview was malformed; nothing was changed"));
        };
        let (transaction, confirmation) = (transaction.to_owned(), confirmation.to_owned());
        let known = |reference: &Value| reference.as_str().and_then(|r| connection.references.borrow().known.get(r).cloned());
        // A new Arrangement clip is laid over the clips it lands on, as Live does: they're read first, to say which (an
        // audio clip's own length shows once it's made).
        let under = if kind.tool == "add_arrangement_clip" { self.arrangement_clips_of(&args, &signal).await } else { None };
        if let (Some(under), Some(start), Some(length)) = (&under, beats(args.get("position")), beats(args.get("length"))) {
            preview.insert("replaces".into(), json!(laid_over(under, start, start + length)));
        }
        let summary = kind.summarize(&preview, &args, &known, None);
        signal.check()?;
        history.changes_this_turn.set(history.changes_this_turn.get() + 1);
        let applied = match connection
            .call(
                &kind.apply,
                object(json!({"transactionId":transaction,"confirmation":confirmation,"idempotencyKey":uuid::Uuid::new_v4().to_string()})),
                history.change_signal(),
            )
            .await
        {
            Ok(result) => result,
            Err(_) => {
                history.remember(
                    new_record(kind, summary, ChangeState::Unsure, connection.now().timestamp_millis()),
                    transaction.into(),
                    None,
                );
                return Ok(ChangeOutcome::error("Live didn't confirm this change, so it may or may not have happened. Tell the producer to check Live; discover again before more changes."));
            }
        };
        if applied.is_error == Some(true) {
            let text = stringify(&serde_json::to_value(&applied).unwrap());
            if !uncertain(&applied) {
                return Ok(ChangeOutcome::error(text));
            }
            history.remember(new_record(kind, summary, ChangeState::Unsure, connection.now().timestamp_millis()), transaction.into(), None);
            return Ok(ChangeOutcome::error(format!("Live couldn't confirm this change: {text}")));
        }
        let result = match context::payload(&applied) {
            Ok(value) => value,
            Err(_) => {
                history.remember(
                    new_record(kind, summary, ChangeState::Unsure, connection.now().timestamp_millis()),
                    transaction.into(),
                    None,
                );
                return Ok(ChangeOutcome::error("Kumi couldn't read Live's answer to this change, so it can't confirm whether it happened. Tell the producer to check Live; discover again before more changes."));
            }
        };
        if kind.tool == "add_arrangement_clip" {
            // Whether Kumi's own undo can take the new clip back hangs on what it cut: the track's clips are read again
            // and compared with the read before. A read that fails, or a clip cut past what that read explains, leaves
            // the undo to Live's (deleting the new clip wouldn't bring back what it cut).
            let after = self.arrangement_clips_of(&args, &signal).await;
            let made = result.get("result");
            match (&under, &after, beats(made.and_then(|m| m.get("start"))), beats(made.and_then(|m| m.get("length")))) {
                (Some(under), Some(after), Some(start), Some(length)) => {
                    let replaces = laid_over(under, start, start + length);
                    if replaces.is_empty() && cut(under, after) {
                        preview.insert("replacesUnknown".into(), json!(true));
                    }
                    preview.insert("replaces".into(), json!(replaces));
                }
                _ => {
                    preview.insert("replacesUnknown".into(), json!(true));
                }
            }
        }
        let final_summary = kind.summarize(&preview, &args, &known, Some(&result));
        let applied = result.get("state").and_then(Value::as_str) == Some("applied");
        let permanent = if applied { kind.permanent(&args).or_else(|| kind.replaced(&preview)).filter(|s| !s.is_empty()) } else { None };
        let mut record = new_record(
            kind,
            final_summary.clone(),
            if applied {
                if permanent.is_some() {
                    ChangeState::Kept
                } else {
                    ChangeState::Applied
                }
            } else {
                ChangeState::Unsure
            },
            connection.now().timestamp_millis(),
        );
        record.note = permanent.clone();
        let field = match kind.family {
            ChangeFamily::Rename => Some("name"),
            ChangeFamily::Color => Some("color"),
            _ => None,
        };
        let restore = field.and_then(|field| {
            args.get("ref").and_then(Value::as_str).and_then(|reference| {
                connection.references.borrow().known.get(reference).map(|track| Restore {
                    reference: reference.into(),
                    field: field.into(),
                    value: if field == "name" { Some(track.name.clone()) } else { track.color.clone() },
                })
            })
        });
        history.remember(record.clone(), transaction.into(), restore);
        if kind.tool == "set_tempo" && record.state == ChangeState::Applied {
            if let Some(tempo) = args.get("tempo").and_then(Value::as_f64) {
                self.observer.tempo.set(Some(tempo));
            }
        }
        if let Some(reference) = args.get("ref").and_then(Value::as_str) {
            if let Some(track) = connection.references.borrow_mut().known.get_mut(reference) {
                if kind.family == ChangeFamily::Rename {
                    if let Some(renamed) = summary.track {
                        track.name = renamed.name;
                    }
                }
                if kind.family == ChangeFamily::Color {
                    if let Some(colors) = &record.colors {
                        track.color = Some(colors.to.clone());
                    }
                }
            }
        }
        if kind.restructures == Some(true) {
            let created: Vec<_> = result
                .get("created")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|v| v.as_object().cloned().unwrap_or_default())
                .collect();
            let mut book = connection.references.borrow_mut();
            book.cursors.clear();
            static TRACK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r":track:([0-9]+)$").unwrap());
            static SCENE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r":scene:([0-9]+)$").unwrap());
            let positions = |pattern: &Regex| {
                created
                    .iter()
                    .filter_map(|row| row.get("ref").and_then(Value::as_str))
                    .filter_map(|reference| pattern.captures(reference).and_then(|m| number::parse(&m[1])))
                    .collect::<Vec<_>>()
            };
            let tracks = positions(&TRACK);
            let scenes = positions(&SCENE);
            if kind.tool == "add_tracks_and_scenes" && !created.is_empty() && tracks.len() + scenes.len() == created.len() {
                let first_track = tracks.into_iter().fold(f64::INFINITY, f64::min);
                let first_scene = scenes.into_iter().fold(f64::INFINITY, f64::min);
                let references: IndexSet<_> = book.refs.keys().cloned().chain(book.named_references()).collect();
                for reference in references {
                    if track_index_of(&reference).is_some_and(|i| i >= first_track)
                        || scene_index_of(&reference).is_some_and(|i| i >= first_scene)
                    {
                        book.retire(&reference);
                    }
                }
            } else {
                book.refs.clear();
                book.known.clear();
                book.clear_names();
            }
            for row in &created {
                if let Some(reference) = row.get("ref").and_then(Value::as_str) {
                    book.retire(reference);
                }
            }
            for row in &created {
                if let Some(reference) = row.get("ref").and_then(Value::as_str).filter(|s| utf16_len(s) <= 256) {
                    if let Some(kind) = row.get("kind").and_then(Value::as_str).filter(|s| matches!(*s, "track" | "scene")) {
                        book.refs.insert(reference.into(), kind.into());
                        if kind == "track" {
                            if let Some(name) = row.get("name").and_then(Value::as_str) {
                                book.known.insert(reference.into(), TrackChip { name: head(name, 256), color: None });
                            }
                        }
                    }
                }
            }
        }
        let shifted = matches!(kind.tool.as_str(), "move_device" | "move_device_to" | "delete_device");
        if shifted || kind.restructures == Some(true) {
            self.parameters.fast_found.borrow_mut().clear();
        }
        let mut devices_now = None;
        if shifted {
            let mut tracks = Vec::new();
            for key in ["deviceRef", "ref", "targetTrackRef", "targetChainRef"] {
                if let Some(index) = args.get(key).and_then(Value::as_str).and_then(track_index_of) {
                    if !tracks.contains(&index) {
                        tracks.push(index);
                    }
                }
            }
            {
                let mut book = connection.references.borrow_mut();
                let references: IndexSet<_> = book.refs.keys().cloned().chain(book.named_references()).collect();
                static DEVICE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r":(?:device|parameter|chain|drum_pad):").unwrap());
                for reference in references {
                    if DEVICE.is_match(&reference)
                        && !reference.contains(":mixer:")
                        && track_index_of(&reference).is_some_and(|index| tracks.contains(&index))
                    {
                        book.retire(&reference);
                    }
                }
            }
            devices_now = self.devices_of(&tracks, signal).await.ok().flatten();
        }
        let produced = kind.produces(&result);
        if let Some(produced) = &produced {
            if utf16_len(&produced.reference) <= 256 {
                let mut book = connection.references.borrow_mut();
                if kind.restructures != Some(true) {
                    book.unname(&produced.reference);
                }
                book.refs.insert(produced.reference.clone(), produced.kind.clone());
            }
        }
        let mut reply = object(json!({"changed":record.title,"change":record.id,"state":record.state}));
        if let Some(produced) = produced {
            reply.insert("ref".into(), json!(connection.references.borrow_mut().short_ref(&produced.reference)));
        }
        if let Some(lines) = final_summary.lines.filter(|lines| !lines.is_empty()) {
            reply.insert("lines".into(), json!(lines));
        }
        if kind.restructures == Some(true) {
            reply.insert("note".into(),json!(if kind.tool=="add_tracks_and_scenes"{"Tracks and scenes after the new ones moved (return tracks among them): discover those again; earlier references still work, and the new ones in live.created are current."}else{"Track and scene positions moved; discover again before using earlier references (the new ones in live.created are current)."}));
        }
        if shifted {
            if let Some(devices) = devices_now {
                reply.insert("devicesNow".into(), json!(devices));
                reply.insert("note".into(),json!("Devices on the tracks involved moved along their chains: devicesNow has each track's devices as they are now, with current references; earlier device references on those tracks are retired (discover inside racks again)."));
            } else {
                reply.insert("note".into(),json!("Devices on the tracks involved moved along their chains: discover them (and their parameters) again before using earlier references."));
            }
        }
        let mut full = reply.clone();
        full.insert("live".into(), connection.references.borrow_mut().shorten(&json!(result)));
        let text = stringify(&json!(full));
        Ok(ChangeOutcome {
            text: if text.len() <= 16 * 1024 { text } else { stringify(&json!(reply)) },
            is_error: record.state != ChangeState::Applied && permanent.is_none(),
            missed: None,
        })
    }
    async fn append_at_end(&self, kind: &ChangeKind, input: JsonObject, signal: Signal) -> Result<JsonObject, ReadError> {
        let lacks = |key: &str| {
            input
                .get(key)
                .and_then(Value::as_array)
                .is_some_and(|items| items.iter().any(|item| item.is_object() && !item.as_object().unwrap().contains_key("index")))
        };
        if kind.family != ChangeFamily::Structure || (!lacks("tracks") && !lacks("scenes")) {
            return Ok(input);
        }
        let probe = self.parameters.history.connection.call(&kind.preview, input.clone(), signal).await?;
        if probe.is_error == Some(true) {
            return Ok(input);
        }
        let prior = context::object(context::payload(&probe)?.get("prior").unwrap_or(&Value::Null))?;
        let mut out = input;
        for key in ["tracks", "scenes"] {
            let count = prior.get(key).and_then(Value::as_array).map_or(0, Vec::len);
            if let Some(Value::Array(items)) = out.get_mut(key) {
                for (index, item) in items.iter_mut().enumerate() {
                    if let Some(row) = item.as_object_mut() {
                        if !row.contains_key("index") {
                            row.insert("index".into(), json!(count + index));
                        }
                    }
                }
            }
        }
        Ok(out)
    }
    /// The Arrangement clips on the track a change names (name and span), or None when they can't be read.
    async fn arrangement_clips_of(&self, args: &JsonObject, signal: &Signal) -> Option<Vec<JsonObject>> {
        let track = args.get("trackRef").and_then(Value::as_str)?;
        let read = object(json!({"parent":track,"fields":["name","start","endTime","length","objectIdentity"]}));
        self.parameters.history.connection.rows("arrangement-clip", read, signal.clone()).await.ok()
    }
    async fn devices_of(&self, tracks: &[f64], signal: Signal) -> Result<Option<JsonObject>, RuntimeError> {
        let connection = &self.parameters.history.connection;
        let Some(epoch) = connection.epoch.get() else { return Ok(None) };
        if tracks.is_empty() || tracks.len() > 4 {
            return Ok(None);
        }
        let mut now = JsonObject::new();
        for index in tracks {
            let reference = format!("{}:track:{}", number::to_string(epoch), number::to_string(*index));
            let read = connection
                .invoke(
                    "live_discover",
                    object(json!({"kind":"device","parent":reference,"fields":["ref","name","className"]})),
                    signal.clone(),
                )
                .await;
            if read.is_error {
                return Ok(None);
            }
            let parsed: Value = serde_json::from_str(&read.text).map_err(|e| RuntimeError::plain(e.to_string()))?;
            let live = context::object(parsed.get("live").unwrap_or(&Value::Null))?;
            let mut row = JsonObject::new();
            if let Some(name) =
                connection.references.borrow().known.get(&reference).map(|track| track.name.clone()).filter(|s| !s.is_empty())
            {
                row.insert("track".into(), json!(name));
            }
            row.insert("devices".into(), live.get("items").filter(|v| v.is_array()).cloned().unwrap_or(json!([])));
            let short = connection.references.borrow_mut().short_ref(&reference);
            now.insert(short, json!(row));
        }
        Ok(Some(now))
    }
}
/// Whether a clip read before a change is cut or gone in the read after (each told by its identity).
fn cut(before: &[JsonObject], after: &[JsonObject]) -> bool {
    let span = |clip: &JsonObject| (beats(clip.get("start")), beats(clip.get("endTime")));
    before.iter().any(|clip| {
        let identity = clip.get("objectIdentity").filter(|identity| identity.is_string());
        identity.is_none() || !after.iter().any(|other| other.get("objectIdentity") == identity && span(other) == span(clip))
    })
}
/// A finite number of beats, if the value is one.
fn beats(value: Option<&Value>) -> Option<f64> {
    value.and_then(Value::as_f64).filter(|n| n.is_finite())
}
pub fn track_index_of(reference: &str) -> Option<f64> {
    static MATCH: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r":(?:track|clip_slot|clip|arrangement_clip|device|chain|drum_pad|routing_choice|take_lane|mixer):([0-9]+)").unwrap()
    });
    MATCH.captures(reference).and_then(|m| number::parse(&m[1]))
}
pub fn scene_index_of(reference: &str) -> Option<f64> {
    static MATCH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r":scene:([0-9]+)|:(?:clip_slot|clip):[0-9]+:([0-9]+)").unwrap());
    MATCH.captures(reference).and_then(|m| m.get(1).or_else(|| m.get(2)).and_then(|m| number::parse(m.as_str())))
}
fn observation(message: &str) -> ReadError {
    ReadError::Observation(ObservationError(message.into()))
}
fn object(value: Value) -> JsonObject {
    value.as_object().cloned().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clip_cut_or_gone_since_the_read_before_is_seen() {
        let clip = |identity: &str, start: f64, end: f64| object(json!({"objectIdentity":identity,"start":start,"endTime":end}));
        let before = [clip("a", 0., 8.), clip("b", 16., 24.)];
        // The same clips, and a new one: nothing cut.
        assert!(!cut(&before, &[clip("a", 0., 8.), clip("b", 16., 24.), clip("new", 8., 12.)]));
        // A clip cut back, or gone: cut.
        assert!(cut(&before, &[clip("a", 0., 8.), clip("b", 16., 20.)]));
        assert!(cut(&before, &[clip("a", 0., 8.)]));
        // A clip read without its identity can't be told: taken as cut.
        assert!(cut(&[object(json!({"start":0.,"endTime":8.}))], &[clip("a", 0., 8.)]));
    }
}
