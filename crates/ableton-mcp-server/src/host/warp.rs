//! Warp marker edits with exact authority fences and retained inverse operations.
use super::*;
use kumi_common::{
    abort::{Signal, SignalExt},
    js::json as js_json,
};
/// The same beat time, within floating point's rounding: an undo moves a marker back by the distance it moved,
/// and `(b + d) - d` can come back a few ulps from `b`.
fn same(a: &Value, b: &Value) -> bool {
    match (a.as_f64(), b.as_f64()) {
        (Some(a), Some(b)) => (a - b).abs() <= 1e-9 * 1.0_f64.max(a.abs()).max(b.abs()),
        _ => a == b,
    }
}
fn beats(markers: &[Value]) -> Result<Vec<Value>, LiveError> {
    let mut values = vec![];
    for m in markers {
        let beat = tuning::field(m, "beatTime")?.cloned().unwrap_or(Value::Null);
        if !values.iter().any(|v| same(v, &beat)) {
            values.push(beat);
        }
    }
    Ok(values)
}

fn equal_beats(a: &[Value], b: &[Value]) -> bool {
    a.len() == b.len() && a.iter().all(|a| b.iter().any(|b| same(a, b)))
}
fn applied_beats(markers: &[Value], payload: &Value) -> Result<Vec<Value>, LiveError> {
    let mut values = beats(markers)?;
    let beat = &payload["beatTime"];
    let action = payload["action"].as_str().unwrap_or("");
    if action != "add" {
        values.retain(|v| !same(v, beat));
    }
    if action != "delete" {
        let target = if action == "add" { beat.clone() } else { json!(beat.as_f64().unwrap() + payload["distance"].as_f64().unwrap()) };
        if !values.iter().any(|v| same(v, &target)) {
            values.push(target);
        }
    }
    Ok(values)
}

fn markers<'a>(read: &'a Value, label: &str) -> Result<&'a [Value], LiveError> {
    match tuning::field(read, "markers")? {
        None | Some(Value::Null) => Ok(&[]),
        Some(Value::Array(rows)) => Ok(rows),
        Some(Value::String(_)) if label == "before" => Err(LiveError::error("mutation authority contains an unsupported value")),
        Some(_) if label == "before" => Err(LiveError::type_error("markers is not iterable")),
        Some(_) => Err(LiveError::type_error(format!("({label}.markers ?? []).map is not a function"))),
    }
}

fn operation(action: &Value) -> &'static str {
    if action == "add" {
        "audio.warp-marker.add"
    } else if action == "move" {
        "audio.warp-marker.move"
    } else {
        "audio.warp-marker.delete"
    }
}

impl McpHost {
    pub async fn dispatch_warp_marker_tool(&self, call: &ToolCall, signal: Option<&Signal>) -> Option<Result<Option<Value>, LiveError>> {
        let p = call.arguments.as_ref().unwrap_or(&Value::Null);
        Some(Ok(match call.name.as_str() {
            "live_warp_marker_preview" => Some(self.live_warp_marker_preview_async(&call.id, p).await),
            "live_warp_marker_apply" => self.live_warp_marker_apply_async(&call.id, p, signal).await,
            _ => return None,
        }))
    }

    pub async fn live_warp_marker_preview_async(&self, id: &Value, params: &Value) -> Value {
        if !has_only(params, &["clipRef", "action", "beatTime", "distance"]) || !is_non_empty_string(&params["clipRef"], 256) {
            return error(id, -32602, "clipRef is required", None);
        }
        if !["add", "move", "delete"].iter().any(|a| params["action"] == *a) {
            return error(id, -32602, "action must be add, move, or delete", None);
        }
        if !params["beatTime"].as_f64().is_some_and(f64::is_finite) {
            return error(id, -32602, "beatTime is invalid", None);
        }
        if params["action"] == "move" && !params["distance"].as_f64().is_some_and(f64::is_finite) {
            return error(id, -32602, "distance is required for move", None);
        }
        if params["action"] == "move" && params["distance"].as_f64() == Some(0.0) {
            return error(id, -32602, "distance must move the marker: 0 leaves it where it is", None);
        }
        let result = async {
            let status = self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(reads::AUDITION_DEADLINE_MS)))).await?;
            if !status.connected || !status.capabilities.iter().any(|c| c.as_str() == "session.read") {
                return Err(LiveError::error("session read capability is unavailable"));
            }
            let op = operation(&params["action"]);
            if !status.has_operation(op) {
                return Err(LiveError::error(format!("{op} is unavailable")));
            }
            let adapter = self.async_adapter();
            let reference = params["clipRef"].as_str().unwrap();
            let snapshot = self.views.view_for(None, &[params["clipRef"].clone()], None, &[]).await?;
            let row = self.clip_row(&snapshot, reference)?;
            if row.clip["kind"] != "audio" && row.clip["isAudio"] != true {
                return Ok(transaction_error(id, "warp markers require an audio clip"));
            }
            let context = LiveOperationContext::with_deadline(self.deadline(reads::AUDITION_DEADLINE_MS));
            let read = adapter
                .invoke_async(
                    &LiveInvocation::new(
                        "audio.warp-marker.read",
                        json!({
                        "ref":params["clipRef"]}
                        ),
                    ),
                    Some(&context),
                )
                .await?;
            let markers = tuning::field(&read, "markers")?.and_then(Value::as_array).cloned().unwrap_or_default();
            let beats = beats(&markers)?;
            let beat = params["beatTime"].as_f64().unwrap();
            let exists = beats.iter().any(|v| same(v, &params["beatTime"]));
            if params["action"] == "add" && (beat < 0.0 || exists) {
                return Ok(error(id, -32602, "a warp marker already exists at that beat time", None));
            }
            if params["action"] != "add" && !exists {
                return Ok(error(id, -32602, "no warp marker exists at that beat time", None));
            }
            if params["action"] == "move" {
                let target = beat + params["distance"].as_f64().unwrap();
                if target < 0.0 || (beats.iter().any(|v| v.as_f64() == Some(target)) && target != beat) {
                    return Ok(error(id, -32602, "warp-marker move target collides with an existing marker", None));
                }
            }
            let collection = self.warp_marker_collection_revision(&markers)?;
            if arrangement::truthy(&read["revision"]) && read["revision"] != collection {
                return Err(LiveError::error("warp-marker revision disagreement between adapter and host"));
            }
            let authority = self.clip_authority_digest(&snapshot, reference)?;
            let fence = js_json::stringify(&json!({
            "ref":params["clipRef"],
            "markers":markers,
            "authorityDigest":authority,
            "collectionRevision":collection}
            ));
            let mut payload = json!({
            "action":params["action"],
            "ref":params["clipRef"],
            "beatTime":params["beatTime"],
            "expectedClipAuthorityDigest":authority,
            "expectedMarkerCollectionRevision":collection}
            );
            if params["action"] == "move" {
                payload["distance"] = params["distance"].clone();
            }
            let mut t = json!({
            "id":tempo::transaction_id("warp"),
            "epoch":status.epoch,
            "kind":"warp-marker",
            "fence":fence,
            "clipRef":params["clipRef"],
            "payload":payload,
            "prior":{
            "markers":markers}
            }
            );
            if is_non_empty_string(&row.clip["objectIdentity"], 256) {
                t["targetIdentity"] = row.clip["objectIdentity"].clone();
            }
            t["expiresAt"] = json!(kumi_common::time::now_ms_f64() + TRANSACTION_TTL_MS);
            t["state"] = json!("previewed");
            self.retain_bounded_transaction(&self.clip_lifecycle_transactions, t.clone(), "warp marker")?;
            Ok(success_text(
                id,
                &json!({
                "transactionId":t["id"],
                "epoch":t["epoch"],
                "action":params["action"],
                "clipRef":params["clipRef"],
                "markers":markers,
                "impact":"edits-warp-markers",
                "confirmation":"apply",
                "expiresAt":t["expiresAt"]}
                ),
            ))
        }
        .await;
        result.unwrap_or_else(|e| adapter_tool_error(id, &e, "Warp-marker preview requires fresh authoritative state."))
    }

    pub async fn live_warp_marker_apply_async(&self, id: &Value, params: &Value, signal: Option<&Signal>) -> Option<Value> {
        if !valid_transaction_params(params, "apply") {
            return Some(error(id, -32602, "transactionId, confirmation=apply, and idempotencyKey are required", None));
        }
        let Some(record) = self.clip_lifecycle_transactions.get(params["transactionId"].as_str().unwrap()) else {
            return Some(transaction_error(id, "Unknown or expired warp-marker transaction"));
        };
        let t = record.borrow().clone();
        if t["kind"] != "warp-marker"
            || (t["state"] == "previewed" && t["expiresAt"].as_f64().is_some_and(|n| n <= kumi_common::time::now_ms_f64()))
        {
            return Some(transaction_error(id, "Unknown or expired warp-marker transaction"));
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
            let payload = &t["payload"];
            if !reconciliation {
                let before = adapter
                    .invoke_async(
                        &LiveInvocation::new(
                            "audio.warp-marker.read",
                            json!({
                            "ref":t["clipRef"]}
                            ),
                        ),
                        Some(&context),
                    )
                    .await?;
                let snapshot = self.views.view_for(Some(&context), &[t["clipRef"].clone()], None, &[]).await?;
                let markers = markers(&before, "before")?;
                if js_json::stringify(&json!({
                "ref":t["clipRef"],
                "markers":markers,
                "authorityDigest":self.clip_authority_digest(&snapshot,
                reference)?,
                "collectionRevision":self.warp_marker_collection_revision(markers)?}
                )) != t["fence"]
                {
                    return Ok(transaction_error(id, "warp markers or clip hierarchy changed since preview; preview again"));
                }
            }
            {
                let mut r = record.borrow_mut();
                r["state"] = json!("applying");
                r["applyKey"] = params["idempotencyKey"].clone();
            }
            let mut args = payload.clone();
            args.as_object_mut().unwrap().remove("action");
            let result = adapter.invoke_async(&LiveInvocation::new(operation(&payload["action"]), args), Some(&context)).await?;
            if tuning::field(&result, "changed")? != Some(&json!(true)) {
                return Err(LiveError::error("warp-marker change was not confirmed"));
            }
            let after = adapter
                .invoke_async(
                    &LiveInvocation::new(
                        "audio.warp-marker.read",
                        json!({
                        "ref":t["clipRef"]}
                        ),
                    ),
                    Some(&context),
                )
                .await?;
            let expected = applied_beats(t["prior"]["markers"].as_array().unwrap(), payload)?;
            let after_beats = beats(markers(&after, "after")?)?;
            if !equal_beats(&expected, &after_beats) {
                return Err(LiveError::error("warp-marker postcondition was not confirmed"));
            }
            {
                let mut r = record.borrow_mut();
                r["applyKey"] = params["idempotencyKey"].clone();
                r["state"] = json!("applied");
            }
            let mut reply = json!({
            "transactionId":t["id"],
            "state":"applied"}
            );
            if let Some(revision) = result.get("revision") {
                reply["revision"] = revision.clone();
            }
            if let Some(markers) = after.get("markers") {
                reply["markers"] = markers.clone();
            }
            reply["idempotent"] = json!(false);
            Ok(success_text(id, &reply))
        }
        .await;
        Some(result.unwrap_or_else(|e| {
            record.borrow_mut()["state"] = json!("uncertain");
            adapter_tool_error(id, &e, "Warp-marker state is uncertain; perform fresh discovery before retrying.")
        }))
    }

    pub async fn undo_warp_marker_async(&self, id: &Value, params: &Value, signal: Option<&Signal>) -> Value {
        let Some(record) = params["transactionId"]
            .as_str()
            .and_then(|id| self.clip_lifecycle_transactions.get(id))
            .filter(|r| r.borrow()["kind"] == "warp-marker")
        else {
            return transaction_error(id, "Unknown or expired warp-marker transaction");
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
        if (t["state"] != "applied" && !reconciliation) || !arrangement::truthy(&t["clipRef"]) {
            return transaction_error(id, "Only an applied or exact-key uncertain warp-marker transaction can be undone");
        }
        let result = async {
            self.begin_undo_recovery(&record, params["idempotencyKey"].as_str().unwrap())?;
            let status = self.require_connected(Some("session.read"))?;
            if json!(status.epoch) != t["epoch"] {
                return Ok(transaction_error(id, "Live connection epoch changed; undo refused"));
            }
            let adapter = self.async_adapter();
            let context = self.transaction_context(params, signal, reads::AUDITION_DEADLINE_MS);
            record.borrow_mut()["undoKey"] = params["idempotencyKey"].clone();
            if reconciliation {
                self.replay_undo_recovery(&record, adapter.as_ref(), &context).await?;
            }
            let reference = t["clipRef"].as_str().unwrap();
            let snapshot = self.views.view_for(Some(&context), &[t["clipRef"].clone()], None, &[]).await?;
            let read = adapter
                .invoke_async(
                    &LiveInvocation::new(
                        "audio.warp-marker.read",
                        json!({
                        "ref":t["clipRef"]}
                        ),
                    ),
                    Some(&context),
                )
                .await?;
            let row = self.clip_row(&snapshot, reference)?;
            if let Some(moved) =
                self.undo_target_moved(id, &t, "clip", &t["clipRef"], row.clip.get("objectIdentity"), t.get("targetIdentity"))?
            {
                return Ok(moved);
            }
            let payload = &t["payload"];
            let action = &payload["action"];
            let mut inverse = json!({
            "ref":t["clipRef"],
            "beatTime":payload["beatTime"]}
            );
            let op = if action == "add" {
                "audio.warp-marker.delete"
            } else if action == "delete" {
                "audio.warp-marker.add"
            } else {
                inverse["beatTime"] = json!(payload["beatTime"].as_f64().unwrap() + payload["distance"].as_f64().unwrap());
                inverse["distance"] = json!(-payload["distance"].as_f64().unwrap());
                "audio.warp-marker.move"
            };
            let prior = t["prior"]["markers"].as_array().unwrap();
            let current = markers(&read, "read")?;
            if !reconciliation && !equal_beats(&applied_beats(prior, payload)?, &beats(current)?) {
                return Ok(transaction_error(id, "warp markers changed after apply; undo refused"));
            }
            record.borrow_mut()["state"] = json!("undoing");
            inverse["expectedClipAuthorityDigest"] = json!(self.clip_authority_digest(&snapshot, reference)?);
            inverse["expectedMarkerCollectionRevision"] = json!(self.warp_marker_collection_revision(current)?);
            let result = self.invoke_undo_recovery(&record, adapter.as_ref(), op, &inverse, &context).await?;
            if tuning::field(&result, "changed")? != Some(&json!(true)) {
                return Err(LiveError::error("warp-marker undo was not confirmed"));
            }
            let restored = adapter
                .invoke_async(
                    &LiveInvocation::new(
                        "audio.warp-marker.read",
                        json!({
                        "ref":t["clipRef"]}
                        ),
                    ),
                    Some(&context),
                )
                .await?;
            if !equal_beats(&beats(prior)?, &beats(markers(&restored, "restored")?)?) {
                return Err(LiveError::error("warp-marker undo did not restore the exact prior collection"));
            }
            record.borrow_mut()["state"] = json!("undone");
            Ok(success_text(
                id,
                &json!({
                "transactionId":t["id"],
                "state":"undone",
                "idempotent":false}
                ),
            ))
        }
        .await;
        result.unwrap_or_else(|e| {
            record.borrow_mut()["state"] = json!("uncertain");
            adapter_tool_error(id, &e, "Warp-marker undo is uncertain; perform fresh discovery.")
        })
    }
}
