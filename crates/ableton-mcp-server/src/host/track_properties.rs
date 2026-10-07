//! Track colors retain exact identity, state, and applied RGB readback.
use super::{
    clip_properties::scalar_same,
    reads::AUDITION_DEADLINE_MS,
    track_view::{confirmed, digest, track},
    *,
};
use kumi_common::{abort::Signal, js::json as js_json, time::now_ms_f64};
fn revision(track: &Value) -> Result<String, LiveError> {
    digest(&json!({"colorIndex":track["colorIndex"]}))
}
fn fence(reference: &Value, track: &Value) -> String {
    js_json::stringify(&json!({"ref":reference,"objectIdentity":track["objectIdentity"],"state":[track["colorIndex"].clone()]}))
}
fn applied(id: &Value, t: &Value, revision: Option<&Value>, idempotent: bool) -> Value {
    let mut body = json!({"transactionId":t["id"],"state":"applied"});
    if let Some(revision) = revision {
        body["revision"] = revision.clone();
    }
    if let Some(color) = t.get("appliedColor") {
        body["color"] = color.clone();
    }
    body["idempotent"] = json!(idempotent);
    success_text(id, &body)
}
impl McpHost {
    pub async fn dispatch_track_properties_tool(
        &self,
        call: &ToolCall,
        signal: Option<&Signal>,
    ) -> Option<Result<Option<Value>, LiveError>> {
        let p = call.arguments.as_ref().unwrap_or(&Value::Null);
        Some(Ok(match call.name.as_str() {
            "live_track_properties_preview" => Some(self.live_track_properties_preview_async(&call.id, p).await),
            "live_track_properties_apply" => self.live_track_properties_apply_async(&call.id, p, signal).await,
            _ => return None,
        }))
    }
    pub async fn live_track_properties_preview_async(&self, id: &Value, p: &Value) -> Value {
        if !has_only(p, &["ref", "colorIndex"]) || !is_non_empty_string(&p["ref"], 256) {
            return error(id, -32602, "ref is required", None);
        }
        if p.get("colorIndex").is_none() {
            return error(id, -32602, "at least one track property is required", None);
        }
        if !is_integer_in_range(&p["colorIndex"], 0.0, 69.0) {
            return error(id, -32602, "colorIndex is out of bounds", None);
        }
        let result = async {
            let status = self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS)))).await?;
            if !status.connected || !status.capabilities.iter().any(|c| c.as_str() == "session.read") {
                return Err(LiveError::error("session read capability is unavailable"));
            }
            if !status.has_operation("track.set") {
                return Err(LiveError::error("track property editing is unavailable"));
            }
            let snapshot = self.views.view_for(None, &[p["ref"].clone()], None, &[]).await?;
            let track = track(&snapshot, &p["ref"])
                .filter(|t| is_non_empty_string(&t["objectIdentity"], 256))
                .ok_or_else(|| LiveError::error("track identity is not authoritative"))?;
            let prior = json!({"colorIndex":track["colorIndex"]});
            let payload = json!({"ref":p["ref"],"colorIndex":p["colorIndex"],"expectedObjectIdentity":track["objectIdentity"],"expectedStateRevision":revision(&track)?});
            let t = json!({"id":tempo::transaction_id("trackset"),"epoch":status.epoch,"kind":"track-set","fence":fence(&p["ref"],&track),"payload":payload,"prior":prior,"expiresAt":now_ms_f64()+TRANSACTION_TTL_MS,"state":"previewed"});
            self.retain_bounded_transaction(&self.clip_lifecycle_transactions, t.clone(), "track properties edit")?;
            Ok(success_text(id, &json!({"transactionId":t["id"],"epoch":t["epoch"],"ref":p["ref"],"prior":prior,"proposed":{"colorIndex":p["colorIndex"]},"impact":"edits-track-properties","confirmation":"apply","expiresAt":t["expiresAt"]})))
        }
        .await;
        result.unwrap_or_else(|e| adapter_tool_error(id, &e, "Track-properties preview requires fresh authoritative state."))
    }
    pub async fn live_track_properties_apply_async(&self, id: &Value, p: &Value, signal: Option<&Signal>) -> Option<Value> {
        if !valid_transaction_params(p, "apply") {
            return Some(error(id, -32602, "transactionId, confirmation=apply, and idempotencyKey are required", None));
        }
        let Some(record) = self.clip_lifecycle_transactions.get(p["transactionId"].as_str().unwrap()) else {
            return Some(transaction_error(id, "Unknown or expired track-properties transaction"));
        };
        let t = record.borrow().clone();
        if t["kind"] != "track-set" || (t["state"] == "previewed" && t["expiresAt"].as_f64().unwrap_or(f64::NAN) <= now_ms_f64()) {
            return Some(transaction_error(id, "Unknown or expired track-properties transaction"));
        }
        if t["state"] == "applied" && t["applyKey"] == p["idempotencyKey"] {
            return Some(applied(id, &t, None, true));
        }
        let reconcile = t["state"] == "uncertain" && t.get("undoKey").is_none() && t["applyKey"] == p["idempotencyKey"];
        if t["state"] != "previewed" && !reconcile {
            return Some(transaction_error(id, "Transaction is no longer applicable"));
        }
        if signal.is_some_and(Signal::is_cancelled) {
            return None;
        }
        let payload = &t["payload"];
        let reference = &payload["ref"];
        let result = async {
            if reconcile {
                self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS)))).await?;
            }
            let status = self.require_connected(Some("session.read"))?;
            if json!(status.epoch) != t["epoch"] {
                return Ok(transaction_error(id, "Live connection epoch changed; preview again"));
            }
            let adapter = self.async_adapter();
            let context = self.transaction_context(p, signal, AUDITION_DEADLINE_MS);
            if !reconcile {
                let snapshot = self.views.view_for(Some(&context), &[reference.clone()], None, &[]).await?;
                if track(&snapshot, reference).is_none_or(|r| json!(fence(reference, &r)) != t["fence"]) {
                    return Ok(transaction_error(id, "track identity or state changed since preview; preview again"));
                }
            }
            record.borrow_mut()["state"] = json!("applying");
            record.borrow_mut()["applyKey"] = p["idempotencyKey"].clone();
            let result = adapter.invoke_async(&LiveInvocation::new("track.set", payload.clone()), Some(&context)).await?;
            confirmed(&result, "changed", "track properties change was not confirmed")?;
            let snapshot = self.views.view_for(Some(&context), &[reference.clone()], None, &[]).await?;
            let verified = track(&snapshot, reference)
                .filter(|t| t.get("objectIdentity") == payload.get("expectedObjectIdentity"))
                .ok_or_else(|| LiveError::error("edited track identity changed after apply"))?;
            if !scalar_same(verified.get("colorIndex"), payload.get("colorIndex")) {
                return Err(LiveError::error("track properties postcondition was not confirmed"));
            }
            record.borrow_mut()["applyKey"] = p["idempotencyKey"].clone();
            record.borrow_mut()["state"] = json!("applied");
            if verified["color"].is_number() {
                record.borrow_mut()["appliedColor"] = verified["color"].clone();
            }
            Ok(applied(id, &record.borrow(), result.get("revision"), false))
        }
        .await;
        Some(result.unwrap_or_else(|e| {
            apply_failed(id, &record, &e, "Track properties state is uncertain; perform fresh discovery before retrying.")
        }))
    }
    pub async fn undo_track_properties_async(&self, id: &Value, p: &Value, signal: Option<&Signal>) -> Value {
        let Some(record) = self.clip_lifecycle_transactions.get(p["transactionId"].as_str().unwrap_or("")) else {
            return transaction_error(id, "Unknown or expired track-properties transaction");
        };
        let t = record.borrow().clone();
        if t["kind"] != "track-set" {
            return transaction_error(id, "Unknown or expired track-properties transaction");
        }
        if t["state"] == "undone" && t["undoKey"] == p["idempotencyKey"] {
            return success_text(id, &json!({"transactionId":t["id"],"state":"undone","idempotent":true}));
        }
        let reconcile = t["state"] == "uncertain" && t["undoKey"] == p["idempotencyKey"];
        if (t["state"] != "applied" && !reconcile) || !arrangement::truthy(&t["prior"]) {
            return transaction_error(id, "Only an applied or exact-key uncertain track-properties transaction can be undone");
        }
        let payload = &t["payload"];
        let result = async {
            self.begin_undo_recovery(&record, p["idempotencyKey"].as_str().unwrap_or(""))?;
            let status = self.require_connected(Some("session.read"))?;
            if json!(status.epoch) != t["epoch"] {
                return Ok(transaction_error(id, "Live connection epoch changed; undo refused"));
            }
            let adapter = self.async_adapter();
            let context = self.transaction_context(p, signal, AUDITION_DEADLINE_MS);
            record.borrow_mut()["undoKey"] = p["idempotencyKey"].clone();
            if reconcile {
                self.replay_undo_recovery(&record, adapter.as_ref(), &context).await?;
            }
            let snapshot = self.views.view_for(Some(&context), &[payload["ref"].clone()], None, &[]).await?;
            let current = track(&snapshot, &payload["ref"])
                .filter(|r| r.get("objectIdentity") == payload.get("expectedObjectIdentity"))
                .ok_or_else(|| LiveError::error("track identity changed after apply; undo refused"))?;
            if !reconcile && !scalar_same(Some(&current["colorIndex"]), payload.get("colorIndex")) {
                return Ok(transaction_error(id, "track changed after apply; undo refused"));
            }
            record.borrow_mut()["state"] = json!("undoing");
            let mut args = json!({"ref":payload["ref"]});
            args.as_object_mut().unwrap().extend(t["prior"].as_object().cloned().unwrap_or_default());
            args["expectedObjectIdentity"] = current["objectIdentity"].clone();
            args["expectedStateRevision"] = json!(revision(&current)?);
            let result = self.invoke_undo_recovery(&record, adapter.as_ref(), "track.set", &args, &context).await?;
            confirmed(&result, "changed", "track restoration was not confirmed")?;
            let snapshot = self.views.view_for(Some(&context), &[payload["ref"].clone()], None, &[]).await?;
            let restored = track(&snapshot, &payload["ref"])
                .filter(|r| r.get("objectIdentity") == payload.get("expectedObjectIdentity"))
                .ok_or_else(|| LiveError::error("track identity changed after undo"))?;
            for (field, value) in t["prior"].as_object().into_iter().flatten() {
                if !scalar_same(restored.get(field), Some(value)) {
                    return Err(LiveError::error("track exact prior state was not restored"));
                }
            }
            record.borrow_mut()["state"] = json!("undone");
            Ok(success_text(id, &json!({"transactionId":t["id"],"state":"undone","idempotent":false})))
        }
        .await;
        result.unwrap_or_else(|e| {
            record.borrow_mut()["state"] = json!("uncertain");
            adapter_tool_error(id, &e, "Track-properties undo is uncertain; perform fresh discovery.")
        })
    }
}
