//! Exact MIDI note update/delete transactions and content-verified undo.
use super::*;
use kumi_common::{
    abort::{Signal, SignalExt},
    js::json as js_json,
};
use retention::TransactionRecord;
use std::collections::HashMap;
const NOTE_FIELDS: &[&str] =
    &["id", "pitch", "start", "duration", "velocity", "mute", "probability", "velocityDeviation", "releaseVelocity"];
const MAX_NOTES: usize = 10_000_000;
pub(super) struct NoteClip {
    pub notes: Vec<Value>,
    pub notes_revision: String,
    pub authority: Value,
    /// Where its notes can be, in its own time: up to the later of its loop end and end marker. (Live's length is
    /// end minus start, which the notes of a split or left-trimmed clip, its start marker past 0, run past.)
    pub end: f64,
    pub name: String,
    /// It's in the Arrangement (not a Session slot).
    pub arrangement: bool,
}
/// A MIDI clip's own-time end (see `NoteClip::end`), from its row: its length where the row has no loop or markers.
fn own_time_end(clip: &Value) -> Option<f64> {
    let length = finite(&clip["length"])?;
    Some(["loopEnd", "endMarker"].iter().filter_map(|key| finite(&clip[*key])).fold(length, f64::max))
}
/// Why an audio clip takes no note edit.
fn audio_clip(name: &Value) -> LiveError {
    LiveError::error(format!("\"{}\" is an audio clip; notes are only in MIDI clips", name.as_str().unwrap_or_default()))
}
/// Why a note can't go where it was asked to: the clip's own-time span, as read_notes' JSON gives its notes.
pub(super) fn past_the_clip(clip: &NoteClip) -> String {
    format!("\"{}\" holds notes from beat 0 to {} in its own time", clip.name, helpers::js_string(&json!(clip.end)).unwrap_or_default())
}
fn key(note: &Value) -> String {
    note.get("id").map(js_json::stringify).unwrap_or_else(|| "$undefined".into())
}
fn finite(value: &Value) -> Option<f64> {
    value.as_f64().filter(|n| n.is_finite())
}
fn integer(value: &Value) -> bool {
    finite(value).is_some_and(|n| n >= 0.0 && n.fract() == 0.0)
}
fn selected(note: &Value, names: &[&str]) -> Value {
    let mut out = json!({});
    for name in names {
        if let Some(v) = note.get(*name) {
            out[*name] = v.clone();
        }
    }
    out
}
pub(super) fn normalized_note(note: &Value, content: bool, include_id: bool, include_channel: bool) -> Value {
    let mut out = json!({});
    if include_id {
        out["id"] = note.get("id").cloned().unwrap_or(Value::Null);
    }
    for field in ["pitch", "start", "duration", "velocity"] {
        if let Some(value) = note.get(field) {
            out[field] = value.clone();
        }
    }
    if include_channel {
        out["channel"] = note.get("channel").filter(|v| !v.is_null()).cloned().unwrap_or(json!(1));
    }
    for (field, default) in
        [("mute", json!(false)), ("probability", json!(1)), ("velocityDeviation", json!(0)), ("releaseVelocity", json!(64))]
    {
        out[field] = note.get(field).filter(|v| !v.is_null()).cloned().unwrap_or(if content { default } else { Value::Null });
    }
    out
}
pub(super) fn note_fence(notes: &[Value]) -> String {
    fence(notes, false)
}
pub(super) fn note_content_fence(notes: &[Value]) -> String {
    fence(notes, true)
}
fn fence(notes: &[Value], content: bool) -> String {
    let mut rows: Vec<_> =
        notes.iter().map(|note| normalized_note(note, content, !content, true)).map(|note| (js_json::stringify(&note), note)).collect();
    rows.sort_by(|a, b| crate::midi_transforms::locale_compare(&a.0, &b.0));
    js_json::stringify(&Value::Array(rows.into_iter().map(|(_, row)| row).collect()))
}
impl McpHost {
    /// The clip a note edit names, with its notes and their revision, fenced as the Remote Script fences it: a Session
    /// clip by its track, slot and scene, from the view's track row; an Arrangement clip by its track, read on its own
    /// with its notes (a view lists Arrangement clips without them), under that track.
    pub(super) async fn note_clip(
        &self,
        snapshot: &LiveSnapshot,
        context: Option<&LiveOperationContext>,
        reference: &str,
    ) -> Result<NoteClip, LiveError> {
        let value = serde_json::to_value(snapshot).unwrap();
        if let Some(row) = value["arrangement"]["clips"].as_array().into_iter().flatten().find(|clip| clip["ref"] == reference) {
            let track =
                value["tracks"].as_array().into_iter().flatten().find(|track| track["ref"] == row["trackRef"]).cloned().unwrap_or_default();
            if row["isAudio"] == true || row["kind"] == "audio" {
                return Err(audio_clip(&row["name"]));
            }
            let fields =
                ["ref", "objectIdentity", "kind", "name", "length", "loopEnd", "endMarker", "notes", "notesRevision"];
            let clip = match track["ref"].as_str() {
                Some(parent) if is_non_empty_string(&track["objectIdentity"], 256) => {
                    self.discover_one_async(context, LiveDiscoveryKind::ArrangementClip, reference, Some(&fields), Some(parent)).await?
                }
                _ => None,
            };
            let clip = clip.filter(|clip| {
                clip["kind"] == "midi"
                    && clip["notes"].is_array()
                    && is_non_empty_string(&clip["notesRevision"], 64)
                    && own_time_end(clip).is_some()
                    && is_non_empty_string(&clip["objectIdentity"], 256)
            });
            if let Some(clip) = clip {
                return Ok(NoteClip {
                    notes: clip["notes"].as_array().unwrap().clone(),
                    notes_revision: clip["notesRevision"].as_str().unwrap().into(),
                    authority: json!({"expectedObjectIdentity":clip["objectIdentity"],"expectedTrackRef":track["ref"],"expectedTrackIdentity":track["objectIdentity"]}),
                    end: own_time_end(&clip).unwrap(),
                    name: clip["name"].as_str().or(row["name"].as_str()).unwrap_or_default().into(),
                    arrangement: true,
                });
            }
            return Err(LiveError::error("MIDI clip reference lacks exact identity or notes revision"));
        }
        for track in snapshot.tracks.as_deref().unwrap_or(&[]) {
            if let Some(clip) = track.clips.iter().find(|clip| clip.ref_.0 == reference) {
                let clip = serde_json::to_value(clip).unwrap();
                if clip["kind"] == "audio" {
                    return Err(audio_clip(&clip["name"]));
                }
                if clip["kind"] == "midi"
                    && clip["notes"].is_array()
                    && is_non_empty_string(&clip["notesRevision"], 64)
                    && own_time_end(&clip).is_some()
                {
                    return Ok(NoteClip {
                        notes: clip["notes"].as_array().unwrap().clone(),
                        notes_revision: clip["notesRevision"].as_str().unwrap().into(),
                        authority: self.clip_authority(snapshot, reference)?,
                        end: own_time_end(&clip).unwrap(),
                        name: clip["name"].as_str().unwrap_or_default().into(),
                        arrangement: false,
                    });
                }
            }
        }

        Err(LiveError::error("MIDI clip reference lacks exact identity or notes revision"))
    }
    pub async fn dispatch_note_edit_tool(&self, call: &ToolCall, signal: Option<&Signal>) -> Option<Result<Option<Value>, LiveError>> {
        let p = call.arguments.as_ref().unwrap_or(&Value::Null);
        Some(Ok(match call.name.as_str() {
            "live_note_update_preview" => Some(self.live_note_edit_preview_async(&call.id, p, "update").await),
            "live_note_delete_preview" => Some(self.live_note_edit_preview_async(&call.id, p, "delete").await),
            "live_note_update_apply" => self.live_note_edit_apply_async(&call.id, p, "update", signal).await,
            "live_note_delete_apply" => self.live_note_edit_apply_async(&call.id, p, "delete", signal).await,
            _ => return None,
        }))
    }
    pub async fn live_note_edit_preview_async(&self, id: &Value, params: &Value, kind: &str) -> Value {
        let required = if kind == "update" { ["clipRef", "notes"] } else { ["clipRef", "noteIds"] };
        if !has_only(params, &required) || !is_non_empty_string(&params["clipRef"], 256) {
            return error(id, -32602, &format!("{} are required", required.join(" and ")), None);
        }
        let result = async {
            let status = self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(reads::AUDITION_DEADLINE_MS)))).await?;
            if !status.connected || !status.capabilities.iter().any(|c| c.as_str() == "session.read") {
                return Err(LiveError::error("session read capability is unavailable"));
            }
            let operation = if kind == "update" { "note.update" } else { "note.delete" };
            if !status.has_operation(operation) {
                return Err(LiveError::error(format!("{operation} is unavailable")));
            }
            let snapshot = self.views.view_for(None, &[params["clipRef"].clone()], None, &[]).await?;
            let clip = self.note_clip(&snapshot, None, params["clipRef"].as_str().unwrap()).await?;
            let fence = note_fence(&clip.notes);
            let present: HashSet<_> = clip.notes.iter().filter(|n| n["id"].is_number()).map(key).collect();
            let mut note_ids = vec![];
            let mut patches = None;
            if kind == "update" {
                let Some(rows) = params["notes"].as_array().filter(|rows| !rows.is_empty() && rows.len() <= MAX_NOTES) else {
                    return Ok(error(id, -32602, "notes must be one or more patch objects", None));
                };
                let mut seen = HashSet::new();
                for patch in rows {
                    if !patch.as_object().is_some_and(|o| o.len() >= 2) || !integer(&patch["id"]) || !has_only(patch, NOTE_FIELDS) {
                        return Ok(error(id, -32602, "note patches require an id, at least one edit, and only supported fields", None));
                    }
                    if !seen.insert(key(patch)) {
                        return Ok(error(id, -32602, "duplicate note patch id", None));
                    }
                    note_ids.push(patch["id"].clone());
                    for field in &NOTE_FIELDS[1..] {
                        if let Some(value) = patch.get(*field) {
                            let valid = match *field {
                                "pitch" => integer(value) && value.as_f64().unwrap() <= 127.0,
                                "start" => finite(value).is_some_and(|n| n >= 0.0),
                                "duration" => finite(value).is_some_and(|n| n > 0.0),
                                "velocity" | "releaseVelocity" => finite(value).is_some_and(|n| (0.0..=127.0).contains(&n)),
                                "mute" => value.is_boolean(),
                                "probability" => finite(value).is_some_and(|n| (0.0..=1.0).contains(&n)),
                                "velocityDeviation" => finite(value).is_some_and(|n| n.abs() <= 127.0),
                                _ => false,
                            };
                            if !valid {
                                return Ok(error(
                                    id,
                                    -32602,
                                    &if *field == "mute" { "mute must be boolean".into() } else { format!("{field} is out of bounds") },
                                    None,
                                ));
                            }
                        }
                    }
                }
                if seen.iter().any(|id| !present.contains(id)) {
                    return Ok(transaction_error(id, "note id is not present in the clip"));
                }
                patches = Some(rows.clone());
            } else {
                let Some(rows) =
                    params["noteIds"].as_array().filter(|rows| !rows.is_empty() && rows.len() <= MAX_NOTES && rows.iter().all(integer))
                else {
                    return Ok(error(id, -32602, "noteIds must be one or more non-negative integers", None));
                };
                let seen: HashSet<_> = rows.iter().map(js_json::stringify).collect();
                if seen.len() != rows.len() {
                    return Ok(error(id, -32602, "duplicate note id", None));
                }
                if seen.iter().any(|id| !present.contains(id)) {
                    return Ok(transaction_error(id, "note id is not present in the clip"));
                }
                note_ids = rows.clone();
            }
            let selected_ids: HashSet<_> = note_ids.iter().map(js_json::stringify).collect();
            let prior_notes: Vec<_> = clip.notes.iter().filter(|note| selected_ids.contains(&key(note))).cloned().collect();
            let mut expected_notes = clip.notes.clone();
            if let Some(patches) = &patches {
                for patch in patches {
                    let Some(note) = expected_notes.iter_mut().find(|note| key(note) == key(patch)) else {
                        return Err(LiveError::error("note patch target disappeared"));
                    };
                    for (field, value) in patch.as_object().unwrap() {
                        note[field] = value.clone();
                    }
                    if !note["start"]
                        .as_f64()
                        .zip(note["duration"].as_f64())
                        .is_some_and(|(start, duration)| start >= 0.0 && duration > 0.0 && start + duration <= clip.end)
                    {
                        return Ok(error(id, -32602, &format!("note patch runs past the clip: {}", past_the_clip(&clip)), None));
                    }
                }
            } else {
                expected_notes.retain(|note| !selected_ids.contains(&key(note)));
            }

            let expected_applied_fence = note_fence(&expected_notes);
            let mut t = json!({
            "id":tempo::transaction_id(&format!("note{kind}")),
            "epoch":status.epoch,
            "kind":kind,
            "clipRef":params["clipRef"],
            "authority":clip.authority,
            "notesRevision":clip.notes_revision,
            "fence":fence,
            "expectedAppliedFence":expected_applied_fence}
            );
            if let Some(patches) = &patches {
                t["patches"] = json!(patches);
            }
            t["noteIds"] = json!(note_ids);
            t["priorNotes"] = json!(prior_notes);
            t["priorAllNotes"] = json!(clip.notes);
            t["expiresAt"] = json!(kumi_common::time::now_ms_f64() + TRANSACTION_TTL_MS);
            t["state"] = json!("previewed");

            self.retain_bounded_transaction(&self.note_edit_transactions, t.clone(), "note edit")?;
            let mut outcome = json!({
            "transactionId":t["id"],
            "epoch":t["epoch"],
            "clipRef":t["clipRef"]}
            );
            outcome[if kind == "update" { "patches" } else { "noteIds" }] =
                if kind == "update" { t["patches"].clone() } else { t["noteIds"].clone() };
            outcome["priorNotes"] = t["priorNotes"].clone();
            outcome["impact"] = json!(if kind == "update" { "edits-midi-notes" } else { "deletes-midi-notes" });
            outcome["confirmation"] = json!("apply");
            outcome["expiresAt"] = t["expiresAt"].clone();
            Ok(success_text(id, &outcome))
        }
        .await;
        result.unwrap_or_else(|e| adapter_tool_error(id, &e, "Note-edit preview requires fresh authoritative clip state."))
    }
    pub async fn live_note_edit_apply_async(&self, id: &Value, params: &Value, kind: &str, signal: Option<&Signal>) -> Option<Value> {
        if !valid_transaction_params(params, "apply") {
            return Some(error(id, -32602, "transactionId, confirmation=apply, and idempotencyKey are required", None));
        }
        let Some(record) = self.note_edit_transactions.get(params["transactionId"].as_str().unwrap()) else {
            return Some(transaction_error(id, "Unknown or expired note-edit transaction"));
        };
        let t = record.borrow().clone();
        if t["kind"] != kind || (t["state"] == "previewed" && t["expiresAt"].as_f64().is_some_and(|n| n <= kumi_common::time::now_ms_f64()))
        {
            return Some(transaction_error(id, "Unknown or expired note-edit transaction"));
        }
        if t["state"] == "applied" && t["applyKey"] == params["idempotencyKey"] {
            return Some(success_text(id, &json!({"transactionId":t["id"],"state":"applied","idempotent":true})));
        }
        let reconciliation = t["state"] == "uncertain" && t["applyKey"] == params["idempotencyKey"];
        if t["state"] != "previewed" && !reconciliation {
            return Some(transaction_error(id, "Transaction is no longer applicable"));
        }
        if signal.is_some_and(Signal::aborted) {
            return None;
        }
        let result = async {
            if reconciliation {
                self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(reads::AUDITION_DEADLINE_MS)))).await?;
            }
            let status = self.require_connected(Some("session.read"))?;
            if json!(status.epoch) != t["epoch"] {
                return Ok(transaction_error(id, "Live connection epoch changed; preview again"));
            }
            let adapter = self.async_adapter();
            let context = self.transaction_context(params, signal, reads::AUDITION_DEADLINE_MS);
            let reference = t["clipRef"].as_str().unwrap();
            if !reconciliation {
                let snapshot = self.views.view_for(Some(&context), &[t["clipRef"].clone()], None, &[]).await?;
                let current = self.note_clip(&snapshot, Some(&context), reference).await?;
                if note_fence(&current.notes) != t["fence"]
                    || current.notes_revision != t["notesRevision"]
                    || js_json::stringify(&current.authority) != js_json::stringify(&t["authority"])
                {
                    return Ok(transaction_error(id, "clip identity or notes changed since preview; preview again"));
                }
            }

            let mut args = json!({
            "ref":t["clipRef"]}
            );
            args[if kind == "update" { "notes" } else { "noteIds" }] = t[if kind == "update" { "patches" } else { "noteIds" }].clone();
            args["expectedClipAuthority"] = t["authority"].clone();
            args["expectedNotesRevision"] = t["notesRevision"].clone();
            {
                let mut row = record.borrow_mut();
                row["state"] = json!("applying");
                row["applyKey"] = params["idempotencyKey"].clone();
            }

            let result = adapter
                .invoke_async(&LiveInvocation::new(if kind == "update" { "note.update" } else { "note.delete" }, args), Some(&context))
                .await?;
            let count_key = if kind == "update" { "updated" } else { "deleted" };
            if result.is_null() {
                return Err(LiveError::type_error(format!("Cannot read properties of null (reading '{count_key}')")));
            }
            let count = t[if kind == "update" { "patches" } else { "noteIds" }].as_array().unwrap().len();
            if result[count_key].as_f64() != Some(count as f64) {
                return Err(LiveError::error("Live did not confirm the complete note edit"));
            }

            let applied = self
                .note_clip(&self.views.view_for(Some(&context), &[t["clipRef"].clone()], None, &[]).await?, Some(&context), reference)
                .await?;
            let applied_fence = note_fence(&applied.notes);
            record.borrow_mut()["appliedFence"] = json!(applied_fence);
            if applied_fence != t["expectedAppliedFence"] {
                return Err(LiveError::error("Live note edit changed, clamped, or omitted unexpected note state"));
            }

            {
                let mut row = record.borrow_mut();
                row["applyKey"] = params["idempotencyKey"].clone();
                row["state"] = json!("applied");
            }
            let mut outcome = json!({
            "transactionId":t["id"],
            "state":"applied"}
            );
            outcome[count_key] = result[count_key].clone();
            outcome["idempotent"] = json!(false);
            Ok(success_text(id, &outcome))
        }
        .await;
        Some(result.unwrap_or_else(|e| {
            record.borrow_mut()["state"] = json!("uncertain");
            adapter_tool_error(id, &e, "Note-edit state is uncertain; perform fresh discovery before retrying.")
        }))
    }
    pub async fn undo_note_edit_async(&self, id: &Value, params: &Value, signal: Option<&Signal>) -> Value {
        let Some(record) = params["transactionId"].as_str().and_then(|id| self.note_edit_transactions.get(id)) else {
            return transaction_error(id, "Unknown or expired note-edit transaction");
        };
        self.note_edit_undo(id, &record, params, signal).await
    }
    async fn note_edit_undo(&self, id: &Value, record: &TransactionRecord, params: &Value, signal: Option<&Signal>) -> Value {
        let t = record.borrow().clone();
        if t["state"] == "undone" && t["undoKey"] == params["idempotencyKey"] {
            return success_text(id, &json!({"transactionId":t["id"],"state":"undone","idempotent":true}));
        }
        let reconciliation = t["state"] == "uncertain" && t["undoKey"] == params["idempotencyKey"];
        let apply_recovery = t["state"] == "uncertain" && t.get("undoKey").is_none() && t["kind"] == "delete";
        if t["state"] != "applied" && !reconciliation && !apply_recovery {
            return transaction_error(
                id,
                "Only an applied, recoverable uncertain delete, or exact-key uncertain note-edit transaction can be undone",
            );
        }
        let result = async {
            self.begin_undo_recovery(record, params["idempotencyKey"].as_str().unwrap())?;
            let status = self.require_connected(Some("session.read"))?;
            if json!(status.epoch) != t["epoch"] {
                return Ok(transaction_error(id, "Live connection epoch changed; undo refused"));
            }
            let adapter = self.async_adapter();
            let context = self.transaction_context(params, signal, reads::AUDITION_DEADLINE_MS);
            let reference = t["clipRef"].as_str().unwrap();
            if reconciliation {
                self.replay_undo_recovery(record, adapter.as_ref(), &context).await?;
            }
            let current = self
                .note_clip(&self.views.view_for(Some(&context), &[t["clipRef"].clone()], None, &[]).await?, Some(&context), reference)
                .await?;
            let prior_notes = t["priorNotes"].as_array().unwrap();
            if apply_recovery {
                if js_json::stringify(&current.authority) != js_json::stringify(&t["authority"]) {
                    return Err(LiveError::error("uncertain note deletion clip hierarchy changed"));
                }
                let prior_all = t["priorAllNotes"].as_array().unwrap();
                let original: HashMap<_, _> = prior_all.iter().map(|note| (key(note), note)).collect();
                let current_by_id: HashMap<_, _> = current.notes.iter().map(|note| (key(note), note)).collect();
                for (note_id, note) in &current_by_id {
                    if original.get(note_id).is_none_or(|prior| note_fence(&[(*note).clone()]) != note_fence(&[(*prior).clone()])) {
                        return Err(LiveError::error("uncertain note deletion conflicts with external note changes"));
                    }
                }
                let missing: Vec<_> = prior_all.iter().filter(|note| !current_by_id.contains_key(&key(note))).cloned().collect();
                let expected = note_content_fence(prior_all);
                {
                    let mut row = record.borrow_mut();
                    row["undoExpectedFence"] = json!(expected);
                    row["undoKey"] = params["idempotencyKey"].clone();
                }
                if !missing.is_empty() {
                    let notes: Vec<_> = missing.iter().map(|note| normalized_note(note, true, false, true)).collect();
                    self.invoke_undo_recovery(
                        record,
                        adapter.as_ref(),
                        "note.add-batch",
                        &json!({
                        "ref":t["clipRef"],
                        "notes":notes,
                        "expectedClipAuthority":current.authority,
                        "expectedNotesRevision":current.notes_revision}
                        ),
                        &context,
                    )
                    .await?;
                }

                let verified = self
                    .note_clip(&self.views.view_for(Some(&context), &[t["clipRef"].clone()], None, &[]).await?, Some(&context), reference)
                    .await?;
                if note_content_fence(&verified.notes) != expected {
                    return Err(LiveError::error("uncertain note deletion recovery did not restore exact prior content"));
                }
                record.borrow_mut()["state"] = json!("undone");
                return Ok(success_text(
                    id,
                    &json!({
                    "transactionId":t["id"],
                    "state":"undone",
                    "restored":missing.len(),
                    "recoveredFromUncertainApply":true,
                    "idempotent":false}
                    ),
                ));
            }
            if reconciliation {
                let restored_fence = if t["kind"] == "update" { note_fence(&current.notes) } else { note_content_fence(&current.notes) };
                if !arrangement::truthy(&t["undoExpectedFence"]) || restored_fence != t["undoExpectedFence"] {
                    return Err(LiveError::error("note-edit undo replay did not restore the exact prior content"));
                }
                record.borrow_mut()["state"] = json!("undone");
                return Ok(success_text(
                    id,
                    &json!({
                    "transactionId":t["id"],
                    "state":"undone",
                    "restored":prior_notes.len(),
                    "idempotent":false}
                    ),
                ));
            }

            if !arrangement::truthy(&t["appliedFence"])
                || note_fence(&current.notes) != t["appliedFence"]
                || js_json::stringify(&current.authority) != js_json::stringify(&t["authority"])
            {
                return Ok(transaction_error(id, "clip identity or notes changed after apply; undo refused"));
            }
            {
                let mut row = record.borrow_mut();
                row["state"] = json!("undoing");
                row["undoKey"] = params["idempotencyKey"].clone();
            }
            if t["kind"] == "update" {
                let restore: Vec<_> = prior_notes.iter().map(|note| normalized_note(note, true, true, false)).collect();
                record.borrow_mut()["undoExpectedFence"] = t["fence"].clone();
                self.invoke_undo_recovery(
                    record,
                    adapter.as_ref(),
                    "note.update",
                    &json!({
                    "ref":t["clipRef"],
                    "notes":restore,
                    "expectedClipAuthority":current.authority,
                    "expectedNotesRevision":current.notes_revision}
                    ),
                    &context,
                )
                .await?;

                let verified = self
                    .note_clip(&self.views.view_for(Some(&context), &[t["clipRef"].clone()], None, &[]).await?, Some(&context), reference)
                    .await?;
                if note_fence(&verified.notes) != t["fence"] {
                    return Err(LiveError::error("note update undo did not restore exact prior notes"));
                }
                {
                    let mut row = record.borrow_mut();
                    row["state"] = json!("undone");
                    row["undoKey"] = params["idempotencyKey"].clone();
                }
                return Ok(success_text(
                    id,
                    &json!({
                    "transactionId":t["id"],
                    "state":"undone",
                    "restored":restore.len(),
                    "idempotent":false}
                    ),
                ));
            }
            let notes: Vec<_> = prior_notes.iter().map(|note| normalized_note(note, true, false, true)).collect();
            let mut expected = current.notes.clone();
            expected.extend(prior_notes.clone());
            let expected = note_content_fence(&expected);
            record.borrow_mut()["undoExpectedFence"] = json!(expected);
            let result = self
                .invoke_undo_recovery(
                    record,
                    adapter.as_ref(),
                    "note.add-batch",
                    &json!({
                    "ref":t["clipRef"],
                    "notes":notes,
                    "expectedClipAuthority":current.authority,
                    "expectedNotesRevision":current.notes_revision}
                    ),
                    &context,
                )
                .await?;
            if result.is_null() {
                return Err(LiveError::type_error("Cannot read properties of null (reading 'added')"));
            }
            if result["added"].as_f64() != Some(notes.len() as f64)
                || !result["noteIds"].as_array().is_some_and(|ids| ids.len() == notes.len())
            {
                return Err(LiveError::error("note delete undo did not re-add the complete batch"));
            }

            let verified = self
                .note_clip(&self.views.view_for(Some(&context), &[t["clipRef"].clone()], None, &[]).await?, Some(&context), reference)
                .await?;
            if note_content_fence(&verified.notes) != expected {
                return Err(LiveError::error("note delete undo content verification failed"));
            }
            {
                let mut row = record.borrow_mut();
                row["state"] = json!("undone");
                row["undoKey"] = params["idempotencyKey"].clone();
            }

            let re_added: Vec<_> = result["noteIds"]
                .as_array()
                .unwrap()
                .iter()
                .enumerate()
                .map(|(index, id)| {
                    let mut row = selected(&prior_notes[index], &["id"]);
                    let prior = row.as_object_mut().unwrap().remove("id");
                    if let Some(prior) = prior {
                        row["priorId"] = prior;
                    }
                    row["noteId"] = id.clone();
                    row
                })
                .collect();
            Ok(success_text(
                id,
                &json!({
                "transactionId":t["id"],
                "state":"undone",
                "restored":notes.len(),
                "reAdded":re_added,
                "note":"re-added notes receive new note ids",
                "idempotent":false}
                ),
            ))
        }
        .await;
        result.unwrap_or_else(|e| {
            record.borrow_mut()["state"] = json!("uncertain");
            adapter_tool_error(id, &e, "Note-edit undo is uncertain; perform fresh discovery.")
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn note_fences_match_source() {
        let data: Value = serde_json::from_str(include_str!("../../tests/fixtures/host-note-edit-oracle.json")).unwrap();
        for row in data["fences"].as_array().unwrap() {
            let notes = row["notes"].as_array().unwrap();
            assert_eq!(note_fence(notes), row["fence"].as_str().unwrap());
            assert_eq!(note_content_fence(notes), row["content"].as_str().unwrap());
        }
    }
}
