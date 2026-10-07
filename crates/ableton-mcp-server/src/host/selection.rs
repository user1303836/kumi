//! Selection and draw-mode transactions compensate partial combined edits before retry.
use super::{
    clip_properties::scalar_same,
    reads::AUDITION_DEADLINE_MS,
    track_view::{confirmed, digest},
    *,
};
use kumi_common::{abort::Signal, js::json as js_json, time::now_ms_f64};
const FIELDS: &[&str] = &["trackRef", "sceneRef", "slotRef", "detailClipRef", "deviceRef", "parameterRef", "chainRef"];
fn revision(snapshot: &Value) -> Result<String, LiveError> {
    let state = Value::Object(FIELDS.iter().map(|f| ((*f).into(), snapshot["selection"][*f].clone())).collect());
    digest(&state)
}
fn selection_fields(value: &Value) -> Value {
    Value::Object(
        value
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(k, _)| k.as_str() != "expectedStateRevision" && k.as_str() != "drawMode")
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
    )
}
/// Live's draw mode as a read has it: the Remote Script sends it in the Set's row, the simulator as `view.drawMode`.
fn draw_mode(snapshot: &Value) -> &Value {
    if snapshot["set"].get("drawMode").is_some() {
        &snapshot["set"]["drawMode"]
    } else {
        &snapshot["view"]["drawMode"]
    }
}
fn fence(proposed: &Value, snapshot: &Value) -> Result<String, LiveError> {
    Ok(js_json::stringify(&json!({"proposed":proposed,"selectionRevision":revision(snapshot)?,"drawMode":draw_mode(snapshot)})))
}
impl McpHost {
    async fn selection_view(&self, context: Option<&LiveOperationContext>) -> Result<Value, LiveError> {
        Ok(serde_json::to_value(
            self.views.view(context, LiveViewScope::Indices(vec![]), Some(&[LiveSnapshotPart::Set, LiveSnapshotPart::Selection])).await?,
        )
        .unwrap())
    }
    pub async fn dispatch_selection_tool(&self, call: &ToolCall, signal: Option<&Signal>) -> Option<Result<Option<Value>, LiveError>> {
        let p = call.arguments.as_ref().unwrap_or(&Value::Null);
        Some(Ok(match call.name.as_str() {
            "live_selection_preview" => Some(self.live_selection_preview_async(&call.id, p).await),
            "live_selection_apply" => self.live_selection_apply_async(&call.id, p, signal).await,
            _ => return None,
        }))
    }
    pub async fn live_selection_preview_async(&self, id: &Value, p: &Value) -> Value {
        if !has_only(p, &["trackRef", "sceneRef", "slotRef", "detailClipRef", "deviceRef", "parameterRef", "chainRef", "drawMode"]) {
            return error(id, -32602, "only selection fields and drawMode are accepted", None);
        }
        if FIELDS.iter().all(|f| p.get(*f).is_none()) && p.get("drawMode").is_none() {
            return error(id, -32602, "at least one selection field or drawMode is required", None);
        }
        if p.get("drawMode").is_some_and(|v| !v.is_boolean()) {
            return error(id, -32602, "drawMode must be boolean", None);
        }
        let result = async {
            let status = self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS)))).await?;
            if !status.connected || !status.capabilities.iter().any(|c| c.as_str() == "session.read") {
                return Err(LiveError::error("session read capability is unavailable"));
            }
            let snapshot = self.selection_view(None).await?;
            let mut proposed = json!({});
            for field in FIELDS {
                if let Some(value) = p.get(*field) {
                    if !value.is_null() && !is_non_empty_string(value, 256) {
                        return Ok(error(id, -32602, &format!("{field} is invalid"), None));
                    }
                    if !value.is_null() && !status.has_operation("selection.set") {
                        return Err(LiveError::error("selection editing is unavailable"));
                    }
                    proposed[*field] = value.clone();
                }
            }
            if p.get("drawMode").is_some() && !status.has_operation("song.view.set") {
                return Err(LiveError::error("draw-mode editing is unavailable"));
            }
            let mut payload = proposed.clone();
            payload["expectedStateRevision"] = json!(revision(&snapshot)?);
            let mut prior =
                Value::Object(proposed.as_object().unwrap().keys().map(|f| (f.clone(), snapshot["selection"][f].clone())).collect());
            // Live selects a device through its track (Song.View.select_device moves the selected track there), so the
            // track selected now goes back with it, before the device.
            if proposed.get("deviceRef").is_some_and(|v| !v.is_null()) && prior.get("trackRef").is_none() {
                prior["trackRef"] = snapshot["selection"]["trackRef"].clone();
            }
            if p.get("drawMode").is_some() {
                prior["drawMode"] = draw_mode(&snapshot).clone();
            }
            let fence = fence(&proposed, &snapshot)?;
            if let Some(v) = p.get("drawMode") {
                payload["drawMode"] = v.clone();
                proposed["drawMode"] = v.clone();
            }
            let t = json!({"id":tempo::transaction_id("selection"),"epoch":status.epoch,"kind":"selection","fence":fence,"payload":payload,"prior":prior,"expiresAt":now_ms_f64()+TRANSACTION_TTL_MS,"state":"previewed"});
            self.retain_bounded_transaction(&self.clip_lifecycle_transactions, t.clone(), "selection")?;
            Ok(success_text(id, &json!({"transactionId":t["id"],"epoch":t["epoch"],"prior":prior,"proposed":proposed,"impact":"edits-live-selection","confirmation":"apply","expiresAt":t["expiresAt"]})))
        }
        .await;
        result.unwrap_or_else(|e| adapter_tool_error(id, &e, "Selection preview requires fresh authoritative state."))
    }
    pub async fn live_selection_apply_async(&self, id: &Value, p: &Value, signal: Option<&Signal>) -> Option<Value> {
        if !valid_transaction_params(p, "apply") {
            return Some(error(id, -32602, "transactionId, confirmation=apply, and idempotencyKey are required", None));
        }
        let Some(record) = self.clip_lifecycle_transactions.get(p["transactionId"].as_str().unwrap()) else {
            return Some(transaction_error(id, "Unknown or expired selection transaction"));
        };
        let t = record.borrow().clone();
        if t["kind"] != "selection" || (t["state"] == "previewed" && t["expiresAt"].as_f64().unwrap_or(f64::NAN) <= now_ms_f64()) {
            return Some(transaction_error(id, "Unknown or expired selection transaction"));
        }
        if t["state"] == "applied" && t["applyKey"] == p["idempotencyKey"] {
            return Some(success_text(id, &json!({"transactionId":t["id"],"state":"applied","idempotent":true})));
        }
        let reconcile = t["state"] == "uncertain" && t["applyKey"] == p["idempotencyKey"];
        if t["state"] != "previewed" && !reconcile {
            return Some(transaction_error(id, "Transaction is no longer applicable"));
        }
        if signal.is_some_and(Signal::is_cancelled) {
            return None;
        }
        let payload = &t["payload"];
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
            let proposed = selection_fields(payload);
            if !reconcile {
                let snapshot = self.selection_view(Some(&context)).await?;
                if json!(fence(&proposed, &snapshot)?) != t["fence"] {
                    return Ok(transaction_error(id, "selection state changed since preview; preview again"));
                }
            }
            record.borrow_mut()["state"] = json!("applying");
            record.borrow_mut()["applyKey"] = p["idempotencyKey"].clone();
            let mut selection_applied = false;
            if !proposed.as_object().unwrap().is_empty() {
                let mut args = payload.clone();
                args.as_object_mut().unwrap().remove("drawMode");
                let result = adapter.invoke_async(&LiveInvocation::new("selection.set", args), Some(&context)).await?;
                confirmed(&result, "changed", "selection change was not confirmed")?;
                selection_applied = true;
            }
            if let Some(draw_mode) = payload.get("drawMode") {
                let draw_revision = digest(&json!({"drawMode":t["prior"]["drawMode"]}))?;
                let draw_result = async {
                    let draw = adapter
                        .invoke_async(
                            &LiveInvocation::new("song.view.set", json!({"drawMode":draw_mode,"expectedStateRevision":draw_revision})),
                            Some(&context),
                        )
                        .await?;
                    confirmed(&draw, "changed", "draw-mode change was not confirmed")
                }
                .await;
                if let Err(cause) = draw_result {
                    if selection_applied && arrangement::truthy(&t["prior"]) {
                        let compensation = async {
                            let mut args = t["prior"].clone();
                            args.as_object_mut().unwrap().remove("drawMode");
                            args["expectedStateRevision"] = json!(revision(&self.selection_view(Some(&context)).await?)?);
                            adapter.invoke_async(&LiveInvocation::new("selection.set", args), Some(&context)).await?;
                            Ok::<_, LiveError>(())
                        }
                        .await;
                        if compensation.is_err() {
                            return Err(LiveError::error("draw-mode change failed and selection compensation failed"));
                        }
                    }
                    return Err(cause);
                }
            }
            let verified = self.selection_view(Some(&context)).await?;
            for (f, value) in payload.as_object().unwrap() {
                if ["expectedStateRevision", "drawMode"].contains(&f.as_str()) {
                    continue;
                }
                if f.ends_with("Ref") && !scalar_same(Some(&verified["selection"][f]), Some(value)) {
                    return Err(LiveError::error("selection postcondition was not confirmed"));
                }
            }
            if payload.get("drawMode").is_some() && !scalar_same(Some(draw_mode(&verified)), payload.get("drawMode")) {
                return Err(LiveError::error("draw-mode postcondition was not confirmed"));
            }
            record.borrow_mut()["applyKey"] = p["idempotencyKey"].clone();
            record.borrow_mut()["state"] = json!("applied");
            Ok(success_text(id, &json!({"transactionId":t["id"],"state":"applied","idempotent":false})))
        }
        .await;
        Some(result.unwrap_or_else(|e| {
            record.borrow_mut()["state"] = json!("uncertain");
            adapter_tool_error(id, &e, "Selection state is uncertain; perform fresh discovery before retrying.")
        }))
    }
    pub async fn undo_selection_async(&self, id: &Value, p: &Value, signal: Option<&Signal>) -> Value {
        let Some(record) = self.clip_lifecycle_transactions.get(p["transactionId"].as_str().unwrap_or("")) else {
            return transaction_error(id, "Unknown or expired selection transaction");
        };
        let t = record.borrow().clone();
        if t["kind"] != "selection" {
            return transaction_error(id, "Unknown or expired selection transaction");
        }
        if t["state"] == "undone" && t["undoKey"] == p["idempotencyKey"] {
            return success_text(id, &json!({"transactionId":t["id"],"state":"undone","idempotent":true}));
        }
        let reconcile = t["state"] == "uncertain" && t["undoKey"] == p["idempotencyKey"];
        if (t["state"] != "applied" && !reconcile) || !arrangement::truthy(&t["prior"]) {
            return transaction_error(id, "Only an applied or exact-key uncertain selection transaction can be undone");
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
            let snapshot = self.selection_view(Some(&context)).await?;
            if !reconcile {
                for (f, value) in payload.as_object().unwrap() {
                    if ["expectedStateRevision", "drawMode"].contains(&f.as_str()) {
                        continue;
                    }
                    if !scalar_same(Some(&snapshot["selection"][f]), Some(value)) {
                        return Ok(transaction_error(id, "selection changed after apply; undo refused"));
                    }
                }
                if payload.get("drawMode").is_some() && !scalar_same(Some(draw_mode(&snapshot)), payload.get("drawMode")) {
                    return Ok(transaction_error(id, "draw mode changed after apply; undo refused"));
                }
            }
            record.borrow_mut()["state"] = json!("undoing");
            let mut args = t["prior"].clone();
            args.as_object_mut().unwrap().remove("drawMode");
            // A draw-mode change alone has no selection to put back (and selection.set refuses one with no fields).
            if !args.as_object().unwrap().is_empty() {
                args["expectedStateRevision"] = json!(revision(&snapshot)?);
                let result = self.invoke_undo_recovery(&record, adapter.as_ref(), "selection.set", &args, &context).await?;
                confirmed(&result, "changed", "selection restoration was not confirmed")?;
            }
            if payload.get("drawMode").is_some() {
                let snapshot = self.selection_view(Some(&context)).await?;
                let mut args = json!({});
                if let Some(v) = t["prior"].get("drawMode") {
                    args["drawMode"] = v.clone();
                }
                args["expectedStateRevision"] = json!(digest(&json!({"drawMode":draw_mode(&snapshot)}))?);
                let result = self.invoke_undo_recovery(&record, adapter.as_ref(), "song.view.set", &args, &context).await?;
                confirmed(&result, "changed", "draw-mode restoration was not confirmed")?;
                // Read back, as the apply does.
                let verified = self.selection_view(Some(&context)).await?;
                if !scalar_same(Some(draw_mode(&verified)), t["prior"].get("drawMode")) {
                    return Err(LiveError::error("draw-mode restoration was not confirmed"));
                }
            }
            record.borrow_mut()["state"] = json!("undone");
            Ok(success_text(id, &json!({"transactionId":t["id"],"state":"undone","idempotent":false})))
        }
        .await;
        result.unwrap_or_else(|e| {
            record.borrow_mut()["state"] = json!("uncertain");
            adapter_tool_error(id, &e, "Selection undo is uncertain; perform fresh discovery.")
        })
    }
}
