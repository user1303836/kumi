//! Guarded Session MIDI creation, exact content verification and owned undo.
use crate::{
    live::*,
    registry::{canonical_json, UNBOUNDED_CANONICAL_LIMITS},
    transactions::batch::compensation_context,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use kumi_common::js::json as js_json;
fn now_ms() -> f64 {
    kumi_common::time::now_ms() as f64
}
use rand::RngCore;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{cell::RefCell, collections::HashMap, rc::Rc};

pub const SESSION_MIDI_TRANSACTION_TTL_MS: f64 = 30_000.0;
pub const MAX_SESSION_MIDI_NOTES: usize = 10_000_000;
const CREATE_CAPS: &[&str] = &["session.read", "session.midi_clip.create", "session.midi_clip.delete", "session.midi_note.write"];
const CREATE_OPS: &[&str] = &["clip.create", "clip.delete", "note.add-batch"];
fn fail(message: impl Into<String>) -> LiveError {
    LiveError::error(message)
}
fn array(value: &Value) -> &[Value] {
    value.as_array().map(Vec::as_slice).unwrap_or(&[])
}
fn string(value: &Value) -> &str {
    value.as_str().unwrap_or("")
}
fn number(value: &Value) -> f64 {
    value.as_f64().unwrap_or(f64::NAN)
}
fn integer(value: &Value, low: f64, high: f64) -> bool {
    value.as_f64().is_some_and(|n| n.is_finite() && n.fract() == 0.0 && n >= low && n <= high)
}
fn hash(value: &Value) -> Result<String, LiveError> {
    Ok(hex::encode(Sha256::digest(canonical_json(value, &UNBOUNDED_CANONICAL_LIMITS).map_err(|e| fail(format!("{e:?}")))?.as_bytes())))
}
fn fingerprint(value: &Value) -> Result<String, LiveError> {
    hash(&without_playback_state(value))
}
fn base_fingerprint(value: &Value) -> Result<String, LiveError> {
    let mut base = value.as_object().cloned().ok_or_else(|| fail("MIDI clip base state is unavailable"))?;
    base.remove("notes");
    base.remove("notesRevision");
    fingerprint(&Value::Object(base))
}
fn digest(value: &Value) -> bool {
    value.as_str().is_some_and(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
}
fn target_authority(snapshot: &Value, track_ref: &Value, scene_index: &Value) -> Result<Value, LiveError> {
    let track = array(&snapshot["tracks"]).iter().find(|item| item["ref"] == *track_ref);
    let slot = track.and_then(|track| array(&track["clipSlots"]).iter().find(|item| number(&item["sceneIndex"]) == number(scene_index)));
    let scene = array(&snapshot["scenes"]).iter().find(|item| number(&item["index"]) == number(scene_index));
    match (track, slot, scene) {
        (Some(track), Some(slot), Some(scene))
            if track["objectIdentity"].is_string() && slot["objectIdentity"].is_string() && scene["objectIdentity"].is_string() =>
        {
            Ok(
                json!({"trackRef":track_ref,"trackIdentity":track["objectIdentity"],"sceneIndex":scene_index,"slotRef":slot["ref"],"slotIdentity":slot["objectIdentity"],"sceneRef":scene["ref"],"sceneIdentity":scene["objectIdentity"]}),
            )
        }
        _ => Err(fail("MIDI target lacks exact track, slot, or scene identity")),
    }
}
fn deletion_authority(snapshot: &Value, reference: &Value) -> Result<Value, LiveError> {
    for track in array(&snapshot["tracks"]) {
        let clip = array(&track["clips"]).iter().find(|clip| clip["ref"] == *reference);
        let slot = array(&track["clipSlots"]).iter().find(|slot| slot["clipRef"] == *reference);
        let scene =
            slot.and_then(|slot| array(&snapshot["scenes"]).iter().find(|scene| number(&scene["index"]) == number(&slot["sceneIndex"])));
        if let (Some(clip), Some(slot), Some(scene)) = (clip, slot, scene) {
            if [clip, track, slot, scene].iter().all(|value| value["objectIdentity"].is_string()) {
                return Ok(
                    json!({"expectedObjectIdentity":clip["objectIdentity"],"expectedTrackRef":track["ref"],"expectedTrackIdentity":track["objectIdentity"],"expectedSlotRef":slot["ref"],"expectedSlotIdentity":slot["objectIdentity"],"expectedSceneRef":scene["ref"],"expectedSceneIdentity":scene["objectIdentity"]}),
                );
            }
        }
    }
    Err(fail("MIDI clip lacks exact deletion authority"))
}
fn clip_in<'a>(snapshot: &'a Value, reference: &Value) -> Option<&'a Value> {
    array(&snapshot["tracks"]).iter().flat_map(|track| array(&track["clips"])).find(|clip| clip["ref"] == *reference)
}
fn note_authority(snapshot: &Value, reference: &Value) -> Result<Value, LiveError> {
    let clip = clip_in(snapshot, reference)
        .filter(|clip| clip["notesRevision"].is_string())
        .ok_or_else(|| fail("MIDI clip notes revision is unavailable"))?;
    Ok(json!({"expectedClipAuthority":deletion_authority(snapshot,reference)?,"expectedNotesRevision":clip["notesRevision"]}))
}
fn revision(snapshot: &Value, target: &Value) -> String {
    let clip = array(&snapshot["tracks"])
        .iter()
        .find(|track| track["ref"] == target["trackRef"])
        .and_then(|track| array(&track["clips"]).iter().find(|clip| number(&clip["start"]) == number(&target["sceneIndex"]) * 4.0));
    format!(
        "{}:{}:{}:{}:{}:{}:{}:{}:{}",
        string(&target["trackRef"]),
        string(&target["trackIdentity"]),
        string(&target["slotRef"]),
        string(&target["slotIdentity"]),
        string(&target["sceneRef"]),
        string(&target["sceneIdentity"]),
        clip.map(|clip| string(&clip["ref"])).unwrap_or("empty"),
        clip.map(|clip| string(&clip["name"])).unwrap_or(""),
        clip.map(|clip| array(&clip["notes"]).len()).unwrap_or(0)
    )
}
fn validate_request(request: &mut Value) -> Result<(), LiveError> {
    if !request.is_object()
        || !request["trackRef"].is_string()
        || !integer(&request["sceneIndex"], 0.0, 100_000.0)
        || !request["name"].as_str().is_some_and(|name| (1..=256).contains(&kumi_common::js::string::utf16_len(name)))
        || !request["length"].as_f64().is_some_and(|n| n.is_finite() && n > 0.0 && n <= 1024.0)
        || !request["notes"].as_array().is_some_and(|notes| notes.len() <= MAX_SESSION_MIDI_NOTES)
    {
        return Err(fail("invalid MIDI clip request"));
    }
    let length = number(&request["length"]);
    for note in request["notes"].as_array_mut().unwrap() {
        if !note.is_object() {
            return Err(match note {
                Value::Null => LiveError::type_error("Cannot read properties of null (reading 'channel')"),
                Value::Bool(value) => LiveError::type_error(format!("Cannot create property 'channel' on boolean '{value}'")),
                Value::Number(value) => {
                    LiveError::type_error(format!("Cannot create property 'channel' on number '{}'", js_json::number(value)))
                }
                Value::String(value) => LiveError::type_error(format!("Cannot create property 'channel' on string '{value}'")),
                _ => fail("invalid MIDI note"),
            });
        }
        if note["channel"].is_null() {
            note["channel"] = json!(1);
        }
        if !integer(&note["pitch"], 0.0, 127.0)
            || !integer(&note["velocity"], 1.0, 127.0)
            || !integer(&note["channel"], 1.0, 16.0)
            || !number(&note["start"]).is_finite()
            || number(&note["start"]) < 0.0
            || !number(&note["duration"]).is_finite()
            || number(&note["duration"]) <= 0.0
            || number(&note["start"]) + number(&note["duration"]) > length
        {
            return Err(fail("invalid MIDI note"));
        }
        if !note["mute"].is_null() && !note["mute"].is_boolean() {
            return Err(fail("invalid MIDI note mute"));
        }
        for (field, low, high, message) in [
            ("probability", 0.0, 1.0, "invalid MIDI note probability"),
            ("velocityDeviation", -127.0, 127.0, "invalid MIDI velocity deviation"),
            ("releaseVelocity", 0.0, 127.0, "invalid MIDI release velocity"),
        ] {
            if !note[field].is_null() && !note[field].as_f64().is_some_and(|n| n.is_finite() && n >= low && n <= high) {
                return Err(fail(message));
            }
        }
    }
    Ok(())
}
/// Whether Live's note `found` is the `wanted` one, within the precision Live keeps notes at.
fn fits(found: &Value, wanted: &Value) -> bool {
    (number(&found["start"]) - number(&wanted["start"])).abs() < 1e-6
        && (number(&found["duration"]) - number(&wanted["duration"])).abs() < 1e-6
        && (wanted["mute"].is_null() || found["mute"] == wanted["mute"])
        && [("probability", 0.01), ("velocityDeviation", 0.51), ("releaseVelocity", 0.51)]
            .iter()
            .all(|(key, tolerance)| wanted[*key].is_null() || (number(&found[*key]) - number(&wanted[*key])).abs() <= *tolerance)
}
/// Whether Live's notes are the proposed ones, each wanted note taking the first unused one of Live's (in Live's order)
/// that fits it. Live's notes are grouped by pitch, velocity and channel (which must be equal) and sorted by start, so
/// a wanted note looks only at those of its kind starting within 1e-6 of it.
fn notes_match(actual: &Value, proposed: &Value) -> bool {
    let actual = array(actual);
    let proposed = array(proposed);
    if actual.len() != proposed.len() {
        return false;
    }
    let bits = |n: f64| if n == 0.0 { 0.0f64.to_bits() } else { n.to_bits() };
    let kind = |note: &Value| {
        let [pitch, velocity, channel] = ["pitch", "velocity", "channel"].map(|key| number(&note[key]));
        (!pitch.is_nan() && !velocity.is_nan() && !channel.is_nan()).then(|| [bits(pitch), bits(velocity), bits(channel)])
    };
    let start = |index: &usize| number(&actual[*index]["start"]);
    let mut kinds: HashMap<[u64; 3], Vec<usize>> = HashMap::new();
    for (index, found) in actual.iter().enumerate() {
        if let Some(kind) = kind(found) {
            kinds.entry(kind).or_default().push(index);
        }
    }
    for indices in kinds.values_mut() {
        indices.sort_by(|a, b| start(a).total_cmp(&start(b)));
    }
    let mut used = vec![false; actual.len()];
    for wanted in proposed {
        let Some(indices) = kind(wanted).and_then(|kind| kinds.get(&kind)) else { return false };
        let at = number(&wanted["start"]);
        let from = indices.partition_point(|index| start(index) <= at - 1e-6);
        let found = indices[from..]
            .iter()
            .take_while(|index| start(index) < at + 1e-6)
            .filter(|index| !used[**index] && fits(&actual[**index], wanted))
            .min();
        match found {
            Some(index) => used[*index] = true,
            None => return false,
        }
    }
    true
}
#[derive(Clone)]
struct Record(Rc<RefCell<Value>>);
impl Record {
    fn get(&self, key: &str) -> Value {
        self.0.borrow()[key].clone()
    }
    fn put(&self, key: &str, value: impl Into<Value>) {
        self.0.borrow_mut()[key] = value.into();
    }
    fn remove(&self, key: &str) {
        self.0.borrow_mut().as_object_mut().unwrap().remove(key);
    }
    fn is(&self, key: &str, value: &str) -> bool {
        self.0.borrow()[key] == value
    }
}
pub struct SessionMidiTransactionManager {
    adapter: Rc<dyn AsyncLiveAdapter>,
    views: Rc<LiveViews>,
    records: RefCell<Vec<(String, Record)>>,
    idempotency: RefCell<HashMap<String, (String, Value)>>,
}
impl SessionMidiTransactionManager {
    pub fn new(adapter: Rc<dyn AsyncLiveAdapter>, views: Option<Rc<LiveViews>>) -> Self {
        let view_adapter = adapter.clone();
        Self {
            adapter,
            views: views.unwrap_or_else(|| Rc::new(LiveViews::new(move || view_adapter.clone()))),
            records: RefCell::new(vec![]),
            idempotency: RefCell::new(HashMap::new()),
        }
    }
    fn record(&self, id: &str) -> Option<Record> {
        self.records.borrow().iter().find(|(key, _)| key == id).map(|(_, record)| record.clone())
    }
    fn retain(&self, value: Value) -> Result<(), LiveError> {
        let mut records = self.records.borrow_mut();
        let protected = |record: &Record| ["applying", "applied", "undoing", "uncertain"].iter().any(|state| record.is("state", state));
        let now = now_ms();
        records.retain(|(_, record)| number(&record.get("expiresAt")) > now || protected(record));
        self.idempotency.borrow_mut().retain(|_, (id, _)| records.iter().any(|(key, _)| id == key));
        while records.len() >= 512 {
            let index = records
                .iter()
                .position(|(_, record)| !protected(record))
                .or_else(|| records.iter().position(|(_, record)| record.is("state", "applied")))
                .ok_or_else(|| fail("MIDI transaction capacity is exhausted by recovery-protected work"))?;
            records.remove(index);
        }
        records.push((string(&value["transactionId"]).into(), Record(Rc::new(RefCell::new(value)))));
        Ok(())
    }
    fn require(&self, capabilities: &[&str], operations: &[&str]) -> Result<LiveStatus, LiveError> {
        let status = self.adapter.status()?;
        if !status.connected || status.epoch.is_none() {
            return Err(fail("live-capability-unavailable:connection"));
        }
        for capability in capabilities {
            if !status.capabilities.iter().any(|value| value.as_str() == *capability) {
                return Err(fail(format!("live-capability-unavailable:{capability}")));
            }
        }
        for operation in operations {
            if !status.has_operation(operation) {
                return Err(fail(format!("live-operation-unavailable:{operation}")));
            }
        }
        Ok(status)
    }
    fn preview_snapshot(&self, request: &Value, status: &LiveStatus, snapshot: &Value) -> Result<Value, LiveError> {
        let track = array(&snapshot["tracks"])
            .iter()
            .find(|track| track["ref"] == request["trackRef"] && (track["kind"] == "midi" || track["mediaKind"] == "midi"))
            .ok_or_else(|| fail("MIDI track not found"))?;
        if array(&track["clips"]).iter().any(|clip| number(&clip["start"]) == number(&request["sceneIndex"]) * 4.0) {
            return Err(fail("Session slot is occupied"));
        }
        let target = target_authority(snapshot, &request["trackRef"], &request["sceneIndex"])?;
        let mut random = [0; 18];
        rand::rng().fill_bytes(&mut random);
        let result = json!({"transactionId":format!("midi_{}",URL_SAFE_NO_PAD.encode(random)),"epoch":status.epoch,"revision":revision(snapshot,&target),"target":target,"prior":{"occupied":false},"proposed":request,"impact":"creates-session-midi-clip","confirmation":"apply","expiresAt":now_ms()+SESSION_MIDI_TRANSACTION_TTL_MS});
        let mut record = result.clone();
        record["state"] = "previewed".into();
        self.retain(record)?;
        Ok(result)
    }
    pub fn preview(&self, request: &mut Value) -> Result<Value, LiveError> {
        validate_request(request)?;
        let status = self.require(CREATE_CAPS, CREATE_OPS)?;
        self.preview_snapshot(request, &status, &serde_json::to_value(self.adapter.snapshot()?)?)
    }
    pub async fn preview_async(&self, request: &mut Value) -> Result<Value, LiveError> {
        validate_request(request)?;
        let status = self.require(CREATE_CAPS, CREATE_OPS)?;
        let snapshot = self.views.view_for(None, &[request["trackRef"].clone()], None, &[]).await?;
        self.preview_snapshot(request, &status, &serde_json::to_value(snapshot)?)
    }
    fn existing(&self, id: &str, key: &str) -> Result<Option<Value>, LiveError> {
        if let Some((prior, result)) = self.idempotency.borrow().get(key) {
            if prior != id {
                return Err(fail("idempotency key conflicts with another transaction"));
            }
            let mut result = result.clone();
            result["idempotent"] = true.into();
            return Ok(Some(result));
        }
        Ok(None)
    }
    fn applied(&self, id: &str, key: &str, record: &Record, reference: Value, verified: &Value) -> Value {
        record.put("state", "applied");
        record.put("clipRef", reference.clone());
        record.put("appliedNotes", verified.get("notes").cloned().unwrap_or(json!([])));
        record.put("applyKey", key);
        let result = json!({"transactionId":id,"state":"applied","clipRef":reference,"notes":record.get("appliedNotes"),"epoch":record.get("epoch"),"idempotent":false});
        self.idempotency.borrow_mut().insert(key.into(), (id.into(), result.clone()));
        result
    }
    pub fn apply(&self, id: &str, confirmation: &Value, key: &str) -> Result<Value, LiveError> {
        if confirmation != "apply" {
            return Err(fail("confirmation=apply is required"));
        }
        if let Some(result) = self.existing(id, key)? {
            return Ok(result);
        }
        let record = self
            .record(id)
            .filter(|record| !record.is("state", "previewed") || number(&record.get("expiresAt")) > now_ms())
            .ok_or_else(|| fail("MIDI preview expired; preview again"))?;
        if record.is("state", "applied") && record.is("applyKey", key) {
            return Ok(
                json!({"transactionId":id,"state":"applied","clipRef":record.get("clipRef"),"notes":record.get("appliedNotes"),"idempotent":true}),
            );
        }
        if !record.is("state", "previewed") {
            return Err(fail("MIDI transaction is no longer applicable"));
        }
        let status = self.require(CREATE_CAPS, CREATE_OPS)?;
        if json!(status.epoch) != record.get("epoch") {
            return Err(fail("Live connection epoch changed; preview again"));
        }
        record.put("state", "applying");
        record.put("applyKey", key);
        let mut reference = None;
        let result = (|| {
            let snapshot = serde_json::to_value(self.adapter.snapshot()?)?;
            let target = record.get("target");
            let proposed = record.get("proposed");
            if revision(&snapshot, &target_authority(&snapshot, &target["trackRef"], &target["sceneIndex"])?)
                != string(&record.get("revision"))
            {
                return Err(fail("Session target identity or slot state changed since preview"));
            }
            let created = self.adapter.invoke(&LiveInvocation::new("clip.create", create_args(&target, &proposed)))?;
            check_created(&created)?;
            let clip_ref = created["ref"].clone();
            reference = Some(clip_ref.clone());
            record.put("clipIdentity", created["objectIdentity"].clone());
            record.put("clipFingerprint", created["createdFingerprint"].clone());
            if fingerprint(
                &self
                    .adapter
                    .get(&LiveRef::from(string(&clip_ref)))?
                    .ok_or_else(|| LiveError::type_error("Cannot convert undefined or null to object"))?,
            )? != string(&record.get("clipFingerprint"))
            {
                return Err(fail("Live did not confirm the exact created clip fingerprint"));
            }
            if !array(&proposed["notes"]).is_empty() {
                let snapshot = serde_json::to_value(self.adapter.snapshot()?)?;
                let args = with_authority(json!({"ref":clip_ref,"notes":proposed["notes"]}), note_authority(&snapshot, &clip_ref)?);
                let added = self.adapter.invoke(&LiveInvocation::new("note.add-batch", args))?;
                if number(&added["added"]) != array(&proposed["notes"]).len() as f64 {
                    return Err(fail("Live did not add the complete MIDI note batch"));
                }
            }
            let verified = self.adapter.get(&LiveRef::from(string(&clip_ref)))?.unwrap_or(Value::Null);
            if !verified_content(&record, &verified) {
                return Err(fail("Live did not confirm exact MIDI clip contents"));
            }
            record.put("clipDeleteAuthority", deletion_authority(&serde_json::to_value(self.adapter.snapshot()?)?, &clip_ref)?);
            Ok(self.applied(id, key, &record, clip_ref, &verified))
        })();
        match result {
            Ok(value) => Ok(value),
            Err(cause) => {
                if let Some(reference) = reference {
                    let compensation = (|| {
                        let authority = if record.get("clipDeleteAuthority").is_null() {
                            deletion_authority(&serde_json::to_value(self.adapter.snapshot()?)?, &reference)?
                        } else {
                            record.get("clipDeleteAuthority")
                        };
                        self.adapter.invoke(&LiveInvocation::new("clip.delete", with_authority(json!({"ref":reference}), authority)))?;
                        Ok::<_, LiveError>(())
                    })();
                    if compensation.is_err() {
                        record.put("state", "uncertain");
                        return Err(fail("MIDI apply failed and compensation failed; read the target slot before retrying"));
                    }
                }
                record.put("state", "previewed");
                record.remove("applyKey");
                Err(cause)
            }
        }
    }
    pub fn undo(&self, id: &str, confirmation: &Value, key: &str) -> Result<Value, LiveError> {
        if confirmation != "undo" {
            return Err(fail("confirmation=undo is required"));
        }
        let record = self.record(id);
        if record.as_ref().is_some_and(|record| record.is("state", "undone") && record.is("undoKey", key)) {
            return Ok(json!({"transactionId":id,"state":"undone","idempotent":true}));
        }
        let record = record
            .filter(|record| record.is("state", "applied") && record.get("clipRef").is_string())
            .ok_or_else(|| fail("Only an applied MIDI transaction can be undone"))?;
        let status = self.require(&["session.read", "session.midi_clip.delete"], &["clip.delete"])?;
        if json!(status.epoch) != record.get("epoch") {
            return Err(fail("Live connection epoch changed; undo refused"));
        }
        record.put("state", "undoing");
        record.put("undoKey", key);
        let mut sent = false;
        let result = (|| {
            let reference = record.get("clipRef");
            let clip = self.adapter.get(&LiveRef::from(string(&reference)))?.unwrap_or(Value::Null);
            check_undo(&record, &clip)?;
            sent = true;
            self.adapter
                .invoke(&LiveInvocation::new("clip.delete", with_authority(json!({"ref":reference}), record.get("clipDeleteAuthority"))))?;
            record.put("state", "undone");
            Ok(json!({"transactionId":id,"state":"undone","deleted":reference,"idempotent":false}))
        })();
        if result.is_err() {
            if !sent {
                record.put("state", "applied");
                record.remove("undoKey");
            } else {
                record.put("state", "uncertain");
            }
        }
        result
    }
    pub fn is_finalizable(&self, id: &str) -> bool {
        self.record(id).is_some_and(|record| ["uncertain", "applied", "undone"].iter().any(|state| record.is("state", state)))
    }
    pub fn finalize(&self, id: &str) -> Result<Value, LiveError> {
        if !self.is_finalizable(id) {
            return Err(fail("MIDI recovery record is not finalizable"));
        }
        let prior = self.record(id).unwrap().get("state");
        self.records.borrow_mut().retain(|(key, _)| key != id);
        self.idempotency.borrow_mut().retain(|_, (key, _)| key != id);
        Ok(json!({"transactionId":id,"finalized":true,"priorState":prior}))
    }
}
fn create_args(target: &Value, proposed: &Value) -> Value {
    json!({"trackRef":target["trackRef"],"kind":"midi","name":proposed["name"],"sceneIndex":target["sceneIndex"],"length":proposed["length"],"expectedTrackIdentity":target["trackIdentity"],"expectedSlotRef":target["slotRef"],"expectedSlotIdentity":target["slotIdentity"],"expectedSceneRef":target["sceneRef"],"expectedSceneIdentity":target["sceneIdentity"]})
}
fn with_authority(mut args: Value, authority: Value) -> Value {
    if let Some(authority) = authority.as_object() {
        args.as_object_mut().unwrap().extend(authority.clone());
    }
    args
}
fn check_created(created: &Value) -> Result<(), LiveError> {
    if !created["ref"].as_str().is_some_and(|s| !s.is_empty())
        || !created["objectIdentity"].is_string()
        || !created["createdFingerprint"].is_string()
    {
        return Err(fail("Live did not return the exact created clip identity and fingerprint"));
    }
    Ok(())
}
fn verified_content(record: &Record, clip: &Value) -> bool {
    let proposed = record.get("proposed");
    !clip.is_null()
        && clip["objectIdentity"] == record.get("clipIdentity")
        && clip["name"] == proposed["name"]
        && number(&clip["length"]) == number(&proposed["length"])
        && notes_match(&clip["notes"], &proposed["notes"])
}
fn check_undo(record: &Record, clip: &Value) -> Result<(), LiveError> {
    let proposed = record.get("proposed");
    if clip.is_null()
        || clip["objectIdentity"] != record.get("clipIdentity")
        || clip["name"] != proposed["name"]
        || number(&clip["length"]) != number(&proposed["length"])
        || js_json::stringify(clip.get("notes").unwrap_or(&json!([]))) != js_json::stringify(&record.get("appliedNotes"))
    {
        return Err(fail("MIDI clip identity or content changed after apply; undo refused"));
    }
    if record.get("clipDeleteAuthority").is_null() {
        return Err(fail("MIDI clip deletion authority is unavailable"));
    }
    Ok(())
}

impl SessionMidiTransactionManager {
    async fn view_record(
        &self,
        record: &Record,
        reference: Option<&Value>,
        context: Option<&LiveOperationContext>,
    ) -> Result<Value, LiveError> {
        let mut refs = vec![record.get("target")["trackRef"].clone()];
        if let Some(reference) = reference {
            refs.push(reference.clone());
        }
        Ok(serde_json::to_value(self.views.view_for(context, &refs, None, &[]).await?)?)
    }
    async fn get_or_absent(
        &self,
        record: &Record,
        reference: &Value,
        context: Option<&LiveOperationContext>,
    ) -> Result<Option<Value>, LiveError> {
        Ok(clip_in(&self.view_record(record, None, context).await?, reference).cloned())
    }
    async fn compensate_apply(&self, record: &Record, context: Option<&LiveOperationContext>) -> Result<(), LiveError> {
        if record.get("clipRef").is_null() && record.get("compensationArgs").is_null() {
            return Ok(());
        }
        let reference = if record.get("clipRef").is_null() { record.get("compensationArgs")["ref"].clone() } else { record.get("clipRef") };
        if record.get("compensationArgs").is_null() {
            let observed = self.adapter.get_async(&LiveRef::from(string(&reference)), context).await?.unwrap_or(Value::Null);
            if !record.get("clipIdentity").is_null() && observed["objectIdentity"] != record.get("clipIdentity") {
                return Err(fail("transaction-owned MIDI clip identity changed before compensation"));
            }
            let observed_fingerprint = fingerprint(&observed)?;
            let exact_creation = observed_fingerprint == string(&record.get("clipFingerprint"));
            let exact_written = record.get("appliedNotesRevision").is_string()
                && observed["notesRevision"] == record.get("appliedNotesRevision")
                && record.get("clipBaseFingerprint").is_string()
                && base_fingerprint(&observed)? == string(&record.get("clipBaseFingerprint"));
            if !exact_creation && !exact_written {
                return Err(fail("transaction-owned MIDI clip changed before compensation"));
            }
            record.put("compensationFingerprint", observed_fingerprint);
            record.put(
                "compensationArgs",
                with_authority(
                    json!({"ref":reference}),
                    deletion_authority(&self.view_record(record, Some(&reference), context).await?, &reference)?,
                ),
            );
        } else {
            let Some(observed) = self.get_or_absent(record, &reference, context).await? else { return Ok(()) };
            if record.get("compensationFingerprint").is_null() || fingerprint(&observed)? != string(&record.get("compensationFingerprint"))
            {
                return Err(fail("transaction-owned MIDI clip changed before compensation replay"));
            }
        }
        let cleanup = LiveOperationContext {
            signal: context.and_then(|c| c.signal.clone()),
            deadline_ms: Some(context.and_then(|c| c.deadline_ms).unwrap_or_else(|| now_ms() + 5000.0)),
            transaction_id: record.get("transactionId").as_str().map(str::to_owned),
            idempotency_key: record.get("applyKey").as_str().map(str::to_owned),
        };
        self.adapter.invoke_async(&LiveInvocation::new("clip.delete", record.get("compensationArgs")), Some(&cleanup)).await?;
        if self.get_or_absent(record, &reference, context).await?.is_some() {
            return Err(fail("MIDI compensation deletion was not confirmed"));
        }
        Ok(())
    }
    pub async fn apply_async(
        &self,
        id: &str,
        confirmation: &Value,
        key: &str,
        context: Option<&LiveOperationContext>,
    ) -> Result<Value, LiveError> {
        if confirmation != "apply" {
            return Err(fail("confirmation=apply is required"));
        }
        if let Some(result) = self.existing(id, key)? {
            return Ok(result);
        }
        let started = now_ms();
        let record = self
            .record(id)
            .filter(|record| !record.is("state", "previewed") || number(&record.get("expiresAt")) > now_ms())
            .ok_or_else(|| fail("MIDI preview expired; preview again"))?;
        let reconciliation = record.is("state", "uncertain") && record.is("applyKey", key);
        if !record.is("state", "previewed") && !reconciliation {
            return Err(fail("MIDI transaction is no longer applicable"));
        }
        if reconciliation {
            self.views.view(context, LiveViewScope::Indices(vec![]), Some(&[LiveSnapshotPart::Set])).await?;
        }
        let status = self.require(CREATE_CAPS, CREATE_OPS)?;
        if json!(status.epoch) != record.get("epoch") {
            return Err(fail("Live connection epoch changed; preview again"));
        }
        if reconciliation && record.is("recoveryMode", "compensate") {
            return match self.compensate_apply(&record, context).await {
                Ok(()) => {
                    record.put("state", "undone");
                    Ok(json!({"transactionId":id,"state":"compensated","residuals":[],"idempotent":false}))
                }
                Err(cause) => {
                    record.put("state", "uncertain");
                    Err(cause)
                }
            };
        }
        record.put("state", "applying");
        record.put("recoveryMode", "apply");
        record.put("applyKey", key);
        let mut reference = None;
        let result = async {
            let snapshot = self.view_record(&record, None, context).await?;
            let target = record.get("target");
            let proposed = record.get("proposed");
            if !reconciliation
                && revision(&snapshot, &target_authority(&snapshot, &target["trackRef"], &target["sceneIndex"])?)
                    != string(&record.get("revision"))
            {
                return Err(fail("Session target identity or slot state changed since preview"));
            }
            if record.get("createArgs").is_null() {
                record.put("createArgs", create_args(&target, &proposed));
            }
            let created = self.adapter.invoke_async(&LiveInvocation::new("clip.create", record.get("createArgs")), context).await?;
            check_created(&created)?;
            let clip_ref = created["ref"].clone();
            reference = Some(clip_ref.clone());
            record.put("clipRef", clip_ref.clone());
            record.put("clipIdentity", created["objectIdentity"].clone());
            record.put("clipFingerprint", created["createdFingerprint"].clone());
            if record.get("noteArgs").is_null() {
                let creation = self
                    .adapter
                    .get_async(&LiveRef::from(string(&clip_ref)), context)
                    .await?
                    .ok_or_else(|| LiveError::type_error("Cannot convert undefined or null to object"))?;
                if fingerprint(&creation)? != string(&record.get("clipFingerprint")) {
                    return Err(fail("Live did not confirm the exact created clip fingerprint"));
                }
                record.put("clipBaseFingerprint", base_fingerprint(&creation)?);
                if array(&proposed["notes"]).is_empty() {
                    if !digest(&creation["notesRevision"]) {
                        return Err(fail("Live did not return the empty MIDI note revision"));
                    }
                    record.put("appliedNotesRevision", creation["notesRevision"].clone());
                }
            }
            if !array(&proposed["notes"]).is_empty() {
                if record.get("noteArgs").is_null() {
                    let snapshot = self.view_record(&record, Some(&clip_ref), context).await?;
                    record.put(
                        "noteArgs",
                        with_authority(json!({"ref":clip_ref,"notes":proposed["notes"]}), note_authority(&snapshot, &clip_ref)?),
                    );
                }
                let added = self.adapter.invoke_async(&LiveInvocation::new("note.add-batch", record.get("noteArgs")), context).await?;
                if !digest(&added["notesRevision"]) {
                    return Err(fail("Live did not return the fingerprinted MIDI note state"));
                }
                record.put("appliedNotesRevision", added["notesRevision"].clone());
                if number(&added["added"]) != array(&proposed["notes"]).len() as f64 {
                    return Err(fail("Live did not add the complete MIDI note batch"));
                }
            }
            let verified = self.adapter.get_async(&LiveRef::from(string(&clip_ref)), context).await?.unwrap_or(Value::Null);
            if !verified_content(&record, &verified) || verified["notesRevision"] != record.get("appliedNotesRevision") {
                return Err(fail("Live did not confirm exact MIDI clip contents"));
            }
            record.put("clipDeleteAuthority", deletion_authority(&self.view_record(&record, Some(&clip_ref), context).await?, &clip_ref)?);
            Ok(self.applied(id, key, &record, clip_ref, &verified))
        }
        .await;
        match result {
            Ok(result) => Ok(result),
            Err(cause) => {
                let message = cause.message();
                let lower = message.to_ascii_lowercase();
                if ["uncertain", "disconnect", "timeout", "cancellation"].iter().any(|word| lower.contains(word)) {
                    record.put("state", "uncertain");
                    record.put("recoveryMode", "apply");
                    return Err(cause);
                }
                if let Some(reference) = reference {
                    record.put("clipRef", reference);
                    record.put("recoveryMode", "compensate");
                    let span = context.and_then(|c| c.deadline_ms).map_or(5000.0, |deadline| deadline - started);
                    let rollback = compensation_context(&context.cloned().unwrap_or_default(), span);
                    if let Err(compensation) = self.compensate_apply(&record, Some(&rollback)).await {
                        record.put("state", "uncertain");
                        record.put("recoveryMode", "compensate");
                        return Err(fail(format!(
                            "MIDI apply failed ({}) and compensation failed ({}); retry the exact key to reconcile cleanup",
                            utf16_prefix(message, 120),
                            utf16_prefix(compensation.message(), 80)
                        )));
                    }
                }
                record.put("state", "undone");
                Err(cause)
            }
        }
    }
    pub async fn undo_async(
        &self,
        id: &str,
        confirmation: &Value,
        key: &str,
        context: Option<&LiveOperationContext>,
    ) -> Result<Value, LiveError> {
        if confirmation != "undo" {
            return Err(fail("confirmation=undo is required"));
        }
        let record = self.record(id);
        if record.as_ref().is_some_and(|record| record.is("state", "undone") && record.is("undoKey", key)) {
            return Ok(json!({"transactionId":id,"state":"undone","idempotent":true}));
        }
        let reconciliation = record.as_ref().is_some_and(|record| record.is("state", "uncertain") && record.is("undoKey", key));
        let record = record
            .filter(|record| (reconciliation || record.is("state", "applied")) && record.get("clipRef").is_string())
            .ok_or_else(|| fail("Only an applied or exact-key uncertain MIDI transaction can be undone"))?;
        let status = self.require(&["session.read", "session.midi_clip.delete"], &["clip.delete"])?;
        if json!(status.epoch) != record.get("epoch") {
            return Err(fail("Live connection epoch changed; undo refused"));
        }
        record.put("state", "undoing");
        record.put("undoKey", key);
        let mut sent = false;
        let result = async {
            let reference = record.get("clipRef");
            if !reconciliation {
                let clip = self.adapter.get_async(&LiveRef::from(string(&reference)), context).await?.unwrap_or(Value::Null);
                check_undo(&record, &clip)?;
                record.put("undoArgs", with_authority(json!({"ref":reference}), record.get("clipDeleteAuthority")));
            }
            if record.get("undoArgs").is_null() {
                return Err(fail("MIDI clip deletion replay authority is unavailable"));
            }
            sent = true;
            self.adapter.invoke_async(&LiveInvocation::new("clip.delete", record.get("undoArgs")), context).await?;
            if self.get_or_absent(&record, &reference, context).await?.is_some() {
                return Err(fail("MIDI clip deletion was not authoritatively confirmed"));
            }
            record.put("state", "undone");
            Ok(json!({"transactionId":id,"state":"undone","deleted":reference,"idempotent":false}))
        }
        .await;
        if result.is_err() {
            if !sent && !reconciliation {
                record.put("state", "applied");
                record.remove("undoKey");
                record.remove("undoArgs");
            } else {
                record.put("state", "uncertain");
            }
        }
        result
    }
}
fn utf16_prefix(text: &str, length: usize) -> String {
    String::from_utf16_lossy(&text.encode_utf16().take(length).collect::<Vec<_>>())
}

fn discovery_status(adapter: &dyn LiveAdapter, limit: f64) -> Result<LiveStatus, LiveError> {
    if !limit.is_finite() || limit.fract() != 0.0 || !(1.0..=100.0).contains(&limit) {
        return Err(fail("limit must be from 1 to 100"));
    }
    let status = adapter.status()?;
    if !status.connected || status.epoch.is_none() || !status.capabilities.contains(&LiveCapability::SessionRead) {
        return Err(fail("live-capability-unavailable:session.read"));
    }
    Ok(status)
}
fn discovery_page(snapshot: &Value, status: &LiveStatus, kind: &str, limit: f64, cursor: Option<&str>) -> Result<Value, LiveError> {
    let items = match kind {
        "track" => array(&snapshot["tracks"]).to_vec(),
        "scene" => array(&snapshot["scenes"]).to_vec(),
        "clip" => array(&snapshot["tracks"]).iter().flat_map(|track| array(&track["clips"])).cloned().collect(),
        _ => array(&snapshot["tracks"])
            .iter()
            .flat_map(|track| array(&track["clips"]))
            .flat_map(|clip| {
                array(&clip["notes"]).iter().enumerate().map(|(index, note)| {
                    let mut note = note.clone();
                    note["ref"] = format!("note:{}:{index}", string(&clip["ref"])).into();
                    note
                })
            })
            .collect(),
    };
    let offset = if let Some(cursor) = cursor.filter(|cursor| !cursor.is_empty()) {
        // Buffer.from(base64url) ignores non-alphabet bytes and accepts standard base64 too.
        let mut decoded = Vec::new();
        let mut accumulator = 0u32;
        let mut bits = 0;
        for byte in cursor.bytes() {
            let value = match byte {
                b'A'..=b'Z' => byte - b'A',
                b'a'..=b'z' => byte - b'a' + 26,
                b'0'..=b'9' => byte - b'0' + 52,
                b'-' | b'+' => 62,
                b'_' | b'/' => 63,
                b'=' => break,
                _ => continue,
            };
            accumulator = (accumulator << 6) | u32::from(value);
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                decoded.push((accumulator >> bits) as u8);
            }
        }
        let decoded = String::from_utf8_lossy(&decoded);
        let trimmed = kumi_common::js::string::trim_start(&decoded);
        let (negative, digits) =
            if let Some(rest) = trimmed.strip_prefix('-') { (true, rest) } else { (false, trimmed.strip_prefix('+').unwrap_or(trimmed)) };
        let digits: String = digits.chars().take_while(char::is_ascii_digit).collect();
        let value = digits.parse::<f64>().unwrap_or(f64::NAN) * if negative { -1.0 } else { 1.0 };
        if !value.is_finite() || value < 0.0 || value > items.len() as f64 {
            return Err(fail("invalid cursor"));
        }
        value as usize
    } else {
        0
    };
    let end = (offset + limit as usize).min(items.len());
    let mut result = json!({"epoch":status.epoch,"revision":format!("{}:{}",status.epoch.unwrap(),items.len()),"items":items[offset..end],"truncated":end<items.len()});
    if end < items.len() {
        result["nextCursor"] = URL_SAFE_NO_PAD.encode(end.to_string()).into();
    }
    Ok(result)
}
pub fn discover_session(adapter: &dyn LiveAdapter, kind: &str, limit: f64, cursor: Option<&str>) -> Result<Value, LiveError> {
    let status = discovery_status(adapter, limit)?;
    discovery_page(&serde_json::to_value(adapter.snapshot()?)?, &status, kind, limit, cursor)
}
pub async fn discover_session_async(
    adapter: Rc<dyn AsyncLiveAdapter>,
    kind: &str,
    limit: f64,
    cursor: Option<&str>,
) -> Result<Value, LiveError> {
    let status = discovery_status(&*adapter, limit)?;
    let views = LiveViews::new(move || adapter.clone());
    discovery_page(&serde_json::to_value(views.whole_set(None, None).await?)?, &status, kind, limit, cursor)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn expired_recovery_authority_is_preserved_until_terminal_and_capacity_retires_oldest_applied() {
        let manager = SessionMidiTransactionManager::new(Rc::new(DeterministicLiveSimulator::new()), None);
        for (id, state) in [("expired-preview", "previewed"), ("applied", "applied"), ("uncertain", "uncertain"), ("undone", "undone")] {
            manager
                .records
                .borrow_mut()
                .push((id.into(), Record(Rc::new(RefCell::new(json!({"transactionId":id,"state":state,"expiresAt":0}))))));
        }
        manager.idempotency.borrow_mut().insert("old-key".into(), ("expired-preview".into(), json!({})));
        manager.retain(json!({"transactionId":"new","state":"previewed","expiresAt":now_ms()+10000.0})).unwrap();
        assert!(manager.record("expired-preview").is_none());
        assert!(manager.record("undone").is_none());
        assert!(manager.record("applied").is_some());
        assert!(manager.record("uncertain").is_some());
        assert!(manager.idempotency.borrow().is_empty());
        manager.records.borrow_mut().clear();
        for index in 0..512 {
            let id = format!("applied-{index}");
            manager
                .records
                .borrow_mut()
                .push((id.clone(), Record(Rc::new(RefCell::new(json!({"transactionId":id,"state":"applied","expiresAt":0}))))));
        }
        manager.retain(json!({"transactionId":"next","state":"previewed","expiresAt":now_ms()+60000.0})).unwrap();
        assert_eq!(manager.records.borrow().len(), 512);
        assert!(manager.record("applied-0").is_none());
        assert!(manager.record("next").is_some());
        for (_, record) in manager.records.borrow().iter() {
            record.put("state", "applying");
        }
        assert!(manager
            .retain(json!({"transactionId":"another","state":"previewed","expiresAt":now_ms()+60000.0}))
            .unwrap_err()
            .message()
            .contains("capacity is exhausted"));
    }
    #[test]
    fn expressive_note_matching_is_order_independent_and_uses_live_tolerances() {
        let actual = json!([{"pitch":60,"start":1.0000001,"duration":0.5000001,"velocity":100,"channel":1,"mute":true,"probability":0.509,"velocityDeviation":8.5,"releaseVelocity":32.5},{"pitch":40,"start":0,"duration":1,"velocity":90,"channel":1}]);
        let mut wanted = json!([{"pitch":40,"start":0,"duration":1,"velocity":90,"channel":1},{"pitch":60,"start":1,"duration":0.5,"velocity":100,"channel":1,"mute":true,"probability":0.5,"velocityDeviation":8,"releaseVelocity":32}]);
        assert!(notes_match(&actual, &wanted));
        wanted[1]["probability"] = 0.49.into();
        assert!(!notes_match(&actual, &wanted));
    }
}
