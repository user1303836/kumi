//! Audio clip field mutations with authority hashes, float-aware readback, and guarded undo.
use super::*;
use kumi_common::{
    abort::{Signal, SignalExt},
    js::json as js_json,
};
use sha2::{Digest, Sha256};
const FIELDS: [&str; 9] =
    ["gain", "pitchCoarse", "pitchFine", "loopStart", "loopEnd", "warpMode", "warping", "fadeInLength", "fadeOutLength"];
fn audio_fence(reference: &Value, identity: Option<&Value>, clip: &Value) -> String {
    let mut value = json!({
    "ref":reference}
    );
    if let Some(v) = identity {
        value["objectIdentity"] = v.clone();
    }
    value["fields"] = json!(FIELDS.iter().map(|f| clip[*f].clone()).collect::<Vec<_>>());
    js_json::stringify(&value)
}
impl McpHost {
    pub(super) fn audio_clip_mutation_authority(&self, snapshot: &LiveSnapshot, reference: &str) -> Result<Value, LiveError> {
        let located = self.clip_row(snapshot, reference)?;
        let authority = self.clip_authority(snapshot, reference)?;
        let mut state = json!({});
        for field in FIELDS {
            state[field] = located.clip[field].clone();
        }
        let revision = if located.arrangement {
            authority["expectedAuthorityRevision"].clone()
        } else {
            json!(hex::encode(Sha256::digest(canonical_mutation_identity(&authority)?)))
        };

        Ok(json!({
        "expectedObjectIdentity":located.clip["objectIdentity"],
        "expectedAuthorityRevision":revision,
        "expectedStateRevision":hex::encode(Sha256::digest(canonical_mutation_identity(&state)?))}
        ))
    }
    pub async fn dispatch_audio_clip_tool(&self, call: &ToolCall, signal: Option<&Signal>) -> Option<Result<Option<Value>, LiveError>> {
        let params = call.arguments.as_ref().unwrap_or(&Value::Null);
        Some(Ok(match call.name.as_str() {
            "live_audio_clip_preview" => Some(self.live_audio_clip_preview_async(&call.id, params).await),
            "live_audio_clip_apply" => self.live_audio_clip_apply_async(&call.id, params, signal).await,
            _ => return None,
        }))
    }
    pub async fn live_audio_clip_preview_async(&self, id: &Value, params: &Value) -> Value {
        if !has_only(
            params,
            &[
                "clipRef",
                "gain",
                "pitchCoarse",
                "pitchFine",
                "loopStart",
                "loopEnd",
                "warpMode",
                "warping",
                "fadeInLength",
                "fadeOutLength",
            ],
        ) || !is_non_empty_string(&params["clipRef"], 256)
        {
            return error(id, -32602, "clipRef is required", None);
        }

        let mut proposed = json!({});
        for field in FIELDS {
            let Some(value) = params.get(field) else { continue };
            if field == "warping" {
                if !value.is_boolean() {
                    return error(id, -32602, "warping must be boolean", None);
                }
                proposed[field] = value.clone();
                continue;
            }

            let valid = value.as_f64().is_some_and(|n| {
                n.is_finite()
                    && match field {
                        "gain" => n >= 0.0,
                        "pitchCoarse" => n.abs() <= 48.0 && n.fract() == 0.0,
                        "pitchFine" => n.abs() <= 50.0,
                        "loopStart" | "loopEnd" | "fadeInLength" | "fadeOutLength" => n >= 0.0,
                        "warpMode" => n.fract() == 0.0 && (0.0..=16.0).contains(&n),
                        _ => true,
                    }
            });

            if !valid {
                return error(id, -32602, &format!("{field} is out of bounds"), None);
            }
            proposed[field] = value.clone();
        }
        if proposed.as_object().unwrap().is_empty() {
            return error(id, -32602, "at least one audio clip field is required", None);
        }
        let result = async {
            let status = self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(reads::AUDITION_DEADLINE_MS)))).await?;
            if !status.connected || !status.capabilities.iter().any(|c| c.as_str() == "session.read") {
                return Err(LiveError::error("session read capability is unavailable"));
            }

            if !status.has_operation("audio.clip.set") {
                return Err(LiveError::error("audio clip editing is unavailable"));
            }
            let snapshot = self.views.view_for(None, &[params["clipRef"].clone()], None, &[]).await?;
            let reference = params["clipRef"].as_str().unwrap();
            let row = self.clip_row(&snapshot, reference)?;
            if row.clip["isAudio"] != true {
                return Ok(transaction_error(id, "audio properties require an audio clip"));
            }
            let available = row.clip["availableAudioFields"]
                .as_array()
                .cloned()
                .unwrap_or_else(|| FIELDS.iter().filter(|f| !row.clip[**f].is_null()).map(|f| json!(f)).collect());

            if proposed.as_object().unwrap().keys().any(|f| !available.contains(&json!(f))) {
                return Ok(transaction_error(id, "one or more requested audio fields are unavailable on this exact clip"));
            }

            let mut prior = json!({});
            for field in proposed.as_object().unwrap().keys() {
                prior[field] = row.clip[field].clone();
            }
            let authority = self.audio_clip_mutation_authority(&snapshot, reference)?;
            let fence = audio_fence(&params["clipRef"], authority.get("expectedObjectIdentity"), &row.clip);
            let mut payload = json!({
            "ref":params["clipRef"]}
            );
            for (k, v) in proposed.as_object().unwrap().iter().chain(authority.as_object().unwrap()) {
                payload[k] = v.clone();
            }

            let t = json!({
            "id":tempo::transaction_id("audioclip"),
            "epoch":status.epoch,
            "kind":"audio-set",
            "fence":fence,
            "clipRef":params["clipRef"],
            "payload":payload,
            "prior":prior,
            "expiresAt":kumi_common::time::now_ms_f64()+TRANSACTION_TTL_MS,
            "state":"previewed"}
            );

            self.retain_bounded_transaction(&self.clip_lifecycle_transactions, t.clone(), "audio clip")?;
            Ok(success_text(
                id,
                &json!({
                "transactionId":t["id"],
                "epoch":t["epoch"],
                "clipRef":params["clipRef"],
                "prior":prior,
                "proposed":proposed,
                "impact":"edits-audio-clip",
                "confirmation":"apply",
                "expiresAt":t["expiresAt"]}
                ),
            ))
        }
        .await;
        result.unwrap_or_else(|e| adapter_tool_error(id, &e, "Audio-clip preview requires fresh authoritative state."))
    }
    pub async fn live_audio_clip_apply_async(&self, id: &Value, params: &Value, signal: Option<&Signal>) -> Option<Value> {
        if !valid_transaction_params(params, "apply") {
            return Some(error(id, -32602, "transactionId, confirmation=apply, and idempotencyKey are required", None));
        }

        let Some(record) = self.clip_lifecycle_transactions.get(params["transactionId"].as_str().unwrap()) else {
            return Some(transaction_error(id, "Unknown or expired audio-clip transaction"));
        };
        let t = record.borrow().clone();

        if t["kind"] != "audio-set"
            || (t["state"] == "previewed" && t["expiresAt"].as_f64().is_some_and(|n| n <= kumi_common::time::now_ms_f64()))
        {
            return Some(transaction_error(id, "Unknown or expired audio-clip transaction"));
        }

        if t["state"] == "applied" && t["applyKey"] == params["idempotencyKey"] {
            return Some(success_text(
                id,
                &json!({
                "transactionId":t["id"],
                "state":"applied",
                "idempotent":true}
                ),
            ));
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
                let row = self.clip_row(&snapshot, reference)?;
                if audio_fence(&t["clipRef"], row.clip.get("objectIdentity"), &row.clip) != t["fence"] {
                    return Ok(transaction_error(id, "audio clip identity or state changed since preview; preview again"));
                }
            }

            {
                let mut row = record.borrow_mut();
                row["state"] = json!("applying");
                row["applyKey"] = params["idempotencyKey"].clone();
            }
            let result = adapter.invoke_async(&LiveInvocation::new("audio.clip.set", t["payload"].clone()), Some(&context)).await?;
            if result.is_null() {
                return Err(LiveError::type_error("Cannot read properties of null (reading 'changed')"));
            }
            if result["changed"] != true {
                return Err(LiveError::error("audio clip change was not confirmed"));
            }
            let verified = self.clip_row(&self.views.view_for(Some(&context), &[t["clipRef"].clone()], None, &[]).await?, reference)?.clip;
            for field in FIELDS {
                if let Some(value) = t["payload"].get(field) {
                    if !same_live_value(verified.get(field), Some(value)) {
                        return Err(LiveError::error("audio clip postcondition was not confirmed"));
                    }
                }
            }

            {
                let mut row = record.borrow_mut();
                row["applyKey"] = params["idempotencyKey"].clone();
                row["state"] = json!("applied");
            }
            let mut value = json!({
            "transactionId":t["id"],
            "state":"applied"}
            );
            if let Some(revision) = result.get("revision") {
                value["revision"] = revision.clone();
            }
            value["idempotent"] = json!(false);
            Ok(success_text(id, &value))
        }
        .await;
        Some(result.unwrap_or_else(|e| {
            self.apply_failed(id, &record, &e, "Audio-clip state is uncertain; perform fresh discovery before retrying.")
        }))
    }
    pub async fn undo_audio_clip_async(&self, id: &Value, params: &Value, signal: Option<&Signal>) -> Value {
        let Some(record) = params["transactionId"]
            .as_str()
            .and_then(|id| self.clip_lifecycle_transactions.get(id))
            .filter(|r| r.borrow()["kind"] == "audio-set")
        else {
            return transaction_error(id, "Unknown audio-clip transaction");
        };

        let t = record.borrow().clone();
        if t["state"] == "undone" && t["undoKey"] == params["idempotencyKey"] {
            return success_text(
                id,
                &json!({
                "transactionId":t["id"],
                "state":"undone",
                "idempotent":true}
                ),
            );
        }

        let reconciliation = t["state"] == "uncertain" && t["undoKey"] == params["idempotencyKey"];
        if (t["state"] != "applied" && !reconciliation) || !arrangement::truthy(&t["clipRef"]) || !arrangement::truthy(&t["prior"]) {
            return transaction_error(id, "Only an applied or exact-key uncertain audio-clip edit can be undone");
        }

        let result = async {
            self.begin_undo_recovery(&record, params["idempotencyKey"].as_str().unwrap())?;
            let status = self.require_connected(Some("session.read"))?;
            if json!(status.epoch) != t["epoch"] {
                return Ok(transaction_error(id, "Live connection epoch changed; undo refused"));
            }

            let adapter = self.async_adapter();
            let context = self.transaction_context(params, signal, reads::AUDITION_DEADLINE_MS);
            let reference = t["clipRef"].as_str().unwrap();

            record.borrow_mut()["undoKey"] = params["idempotencyKey"].clone();
            if reconciliation {
                self.replay_undo_recovery(&record, adapter.as_ref(), &context).await?;
            }

            let snapshot = self.views.view_for(Some(&context), &[t["clipRef"].clone()], None, &[]).await?;
            let row = self.clip_row(&snapshot, reference)?;
            if let Some(moved) = self.undo_target_moved(
                id,
                &record.borrow(),
                "clip",
                &t["clipRef"],
                row.clip.get("objectIdentity"),
                t["payload"].get("expectedObjectIdentity"),
            )? {
                return Ok(moved);
            }

            let expected = if reconciliation { &t["prior"] } else { &t["payload"] };
            for (field, value) in expected.as_object().unwrap() {
                if field != "ref" && !field.starts_with("expected") && !same_live_value(row.clip.get(field), Some(value)) {
                    return Ok(transaction_error(
                        id,
                        if reconciliation {
                            "Audio clip undo replay did not restore exact prior state"
                        } else {
                            "Audio clip changed after apply; undo refused"
                        },
                    ));
                }
            }

            if !reconciliation {
                record.borrow_mut()["state"] = json!("undoing");
                let authority = self.audio_clip_mutation_authority(&snapshot, reference)?;
                let mut args = json!({
                "ref":t["clipRef"]}
                );
                for (k, v) in t["prior"].as_object().unwrap().iter().chain(authority.as_object().unwrap()) {
                    args[k] = v.clone();
                }

                let result = self.invoke_undo_recovery(&record, adapter.as_ref(), "audio.clip.set", &args, &context).await?;
                if result.is_null() {
                    return Err(LiveError::type_error("Cannot read properties of null (reading 'changed')"));
                }
                if result["changed"] != true {
                    return Err(LiveError::error("Audio clip restoration was not confirmed"));
                }
            }
            let restored = self.clip_row(&self.views.view_for(Some(&context), &[t["clipRef"].clone()], None, &[]).await?, reference)?;
            for (field, value) in t["prior"].as_object().unwrap() {
                if !same_live_value(restored.clip.get(field), Some(value)) {
                    return Err(LiveError::error("Audio clip exact prior state was not restored"));
                }
            }

            record.borrow_mut()["state"] = json!("undone");
            Ok(success_text(
                id,
                &json!({
                "transactionId":t["id"],
                "state":"undone",
                "restored":t["prior"],
                "idempotent":false}
                ),
            ))
        }
        .await;
        result.unwrap_or_else(|e| {
            record.borrow_mut()["state"] = json!("uncertain");
            adapter_tool_error(id, &e, "Audio-clip undo is uncertain; inspect the exact clip.")
        })
    }
}
