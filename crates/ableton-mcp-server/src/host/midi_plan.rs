//! Exact, indexed MIDI note plans and interim-content recovery fences.
use super::*;
use crate::{
    midi_transforms::{note_content_digest, note_identity_digest},
    registry::{canonical_json, CanonicalError},
};
use kumi_common::js::json as js_json;
use note_edit::normalized_note;
use retention::TransactionRecord;
use sha2::{Digest, Sha256};

pub(super) fn transform_patch(note: &Value) -> Value {
    let mut patch = json!({});
    for field in ["id", "pitch", "start", "duration", "velocity", "mute", "probability", "velocityDeviation", "releaseVelocity"] {
        if let Some(v) = note.get(field) {
            patch[field] = v.clone();
        }
    }
    patch
}
pub(super) fn note_digest(notes: &[Value], id_bound: bool) -> Result<String, LiveError> {
    (if id_bound { note_identity_digest(notes) } else { note_content_digest(notes) }).map_err(|e| LiveError::error(e.to_string()))
}
pub(super) fn capture_bounded_fingerprint(value: &Value) -> Result<String, LiveError> {
    let canonical = canonical_json(&without_playback_state(value), &MUTATION_CANONICAL_LIMITS).map_err(|e| {
        LiveError::error(match e {
            CanonicalError::TooDeep => "clip content is too deeply nested",
            CanonicalError::StringTooLarge => "clip content string is too large",
            CanonicalError::ArrayTooLarge => "clip content array exceeds its authoritative bound",
            CanonicalError::ObjectTooLarge => "clip content object is too large",
        })
    })?;

    Ok(hex::encode(Sha256::digest(canonical)))
}
pub(super) fn note_plan_interim_digest(initial: &[Value], steps: &[Value], completed: usize, id_bound: bool) -> Result<String, LiveError> {
    let mut rows: Vec<Option<Value>> = vec![];
    let mut by_id = std::collections::HashMap::<String, usize>::new();
    let mut anonymous = vec![];
    for note in initial {
        if note["id"].is_number() {
            let key = js_json::stringify(&note["id"]);
            if let Some(index) = by_id.get(&key) {
                rows[*index] = Some(note.clone());
            } else {
                by_id.insert(key, rows.len());
                rows.push(Some(note.clone()));
            }
        } else {
            anonymous.push(note.clone());
        }
    }
    for step in steps.iter().take(completed) {
        for item in step["items"].as_array().into_iter().flatten() {
            match step["operation"].as_str().unwrap_or("") {
                "note.delete" => {
                    if let Some(index) = by_id.remove(&js_json::stringify(item)) {
                        rows[index] = None;
                    }
                }
                "note.update" => {
                    if let Some(index) = by_id.get(&js_json::stringify(&item["id"])) {
                        if let Some(note) = &mut rows[*index] {
                            for (k, v) in item.as_object().unwrap() {
                                note[k] = v.clone();
                            }
                        }
                    }
                }
                _ => anonymous.push(item.clone()),
            }
        }
    }
    let notes: Vec<_> = rows.into_iter().flatten().chain(anonymous).collect();
    note_digest(&notes, id_bound)
}
pub(super) fn build_note_plan(diff: &Value) -> Vec<Value> {
    let mut steps = vec![];
    let deletes = diff["delete"].as_array().cloned().unwrap_or_default();
    for chunk in deletes.chunks(512) {
        steps.push(json!({
        "operation":"note.delete",
        "items":chunk}
        ));
    }

    let patches: Vec<_> = diff["update"].as_array().into_iter().flatten().map(transform_patch).collect();
    for chunk in patches.chunks(512) {
        steps.push(json!({
        "operation":"note.update",
        "items":chunk}
        ));
    }

    let adds: Vec<_> = diff["add"].as_array().into_iter().flatten().map(|n| normalized_note(n, true, false, true)).collect();
    for chunk in adds.chunks(512) {
        steps.push(json!({
        "operation":"note.add-batch",
        "items":chunk}
        ));
    }
    steps
}
impl McpHost {
    pub(super) async fn execute_note_plan(
        &self,
        record: Option<&TransactionRecord>,
        adapter: &dyn AsyncLiveAdapter,
        context: &LiveOperationContext,
        reference: &str,
        steps: &[Value],
        initial: &[Value],
        id_bound: bool,
        clip_identity: Option<&str>,
    ) -> Result<(), LiveError> {
        let mut plan = if let Some(record) = record {
            Some(self.begin_undo_recovery(record, context.idempotency_key.as_deref().unwrap_or(""))?.1)
        } else {
            None
        };

        for (index, step) in steps.iter().enumerate() {
            let operation = step["operation"].as_str().unwrap();
            let field = if operation == "note.delete" { "noteIds" } else { "notes" };
            let recorded = plan
                .as_ref()
                .and_then(|p| p.get(index))
                .filter(|r| {
                    let r = r.borrow();
                    r["operation"] == operation && js_json::stringify(&r["args"][field]) == js_json::stringify(&step["items"])
                })
                .cloned();

            if recorded.as_ref().is_some_and(|r| r.borrow()["completed"] == true) {
                continue;
            }
            let fresh = self
                .note_clip(&self.views.view_for(Some(context), &[json!(reference)], None, &[]).await?, Some(context), reference)
                .await?;
            if let Some(identity) = clip_identity {
                if let Some(moved) =
                    Self::moved_target("clip", &json!(reference), fresh.authority.get("expectedObjectIdentity"), Some(&json!(identity)))?
                {
                    return Err(LiveError::error(moved));
                }
            }

            let current = note_digest(&fresh.notes, id_bound)?;
            if current == note_plan_interim_digest(initial, steps, index + 1, id_bound)? {
                if let Some(recorded) = recorded {
                    recorded.borrow_mut()["completed"] = json!(true);
                }
                continue;
            }

            if current != note_plan_interim_digest(initial, steps, index, id_bound)? {
                return Err(LiveError::error("notes changed during the note plan; refusing to overwrite external edits"));
            }

            let args = json!({
            "ref":reference,
            field:step["items"],
            "expectedClipAuthority":fresh.authority,
            "expectedNotesRevision":fresh.notes_revision}
            );

            let result = if let Some(recorded) = recorded {
                recorded.borrow_mut()["args"] = args.clone();
                let result = adapter.invoke_async(&LiveInvocation::new(operation, args), Some(context)).await?;
                {
                    let mut r = recorded.borrow_mut();
                    r["result"] = result.clone();
                    r["completed"] = json!(true);
                }
                result
            } else if let Some(plan) = &mut plan {
                let created = self.push_undo_recovery_step(
                    record.unwrap(),
                    json!({
                    "operation":operation,
                    "args":args,
                    "completed":false}
                    ),
                )?;
                plan.push(created.clone());

                let result = adapter.invoke_async(&LiveInvocation::new(operation, args), Some(context)).await?;
                {
                    let mut r = created.borrow_mut();
                    r["result"] = result.clone();
                    r["completed"] = json!(true);
                }
                result
            } else {
                adapter.invoke_async(&LiveInvocation::new(operation, args), Some(context)).await?
            };
            let confirmation = match operation {
                "note.delete" => "deleted",
                "note.add-batch" => "added",
                _ => "updated",
            };
            if result.is_null() {
                return Err(LiveError::type_error(format!("Cannot read properties of null (reading '{confirmation}')")));
            }
            if result[confirmation].as_f64() != Some(step["items"].as_array().unwrap().len() as f64) {
                return Err(LiveError::error(format!("{operation} did not confirm the exact chunk")));
            }
        }
        Ok(())
    }
}
