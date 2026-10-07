//! Track presentation edits and momentary instrument selection share authoritative view fences.
use super::{clip_properties::scalar_same, reads::AUDITION_DEADLINE_MS, *};
use kumi_common::{abort::Signal, js::json as js_json, time::now_ms_f64};
use sha2::{Digest, Sha256};
const FIELDS: &[(&str, &str)] =
    &[("collapsed", "isCollapsed"), ("deviceInsertMode", "deviceInsertMode"), ("showChains", "isShowingChains")];
pub(super) fn track(snapshot: &LiveSnapshot, reference: &Value) -> Option<Value> {
    snapshot.tracks.iter().flatten().find_map(|track| {
        let row = serde_json::to_value(track).unwrap();
        (row.get("ref") == Some(reference)).then_some(row)
    })
}
fn state(track: Option<&Value>) -> Value {
    let mut result = json!({});
    for (field, row) in FIELDS {
        result[*field] = track.and_then(|t| t.get("view")).and_then(|v| v.get(*row)).cloned().unwrap_or(Value::Null);
    }
    result
}
pub(super) fn digest(value: &Value) -> Result<String, LiveError> {
    Ok(hex::encode(Sha256::digest(canonical_mutation_identity(value)?.as_bytes())))
}
pub(super) fn confirmed(result: &Value, field: &str, message: &str) -> Result<(), LiveError> {
    if result.is_null() {
        return Err(LiveError::type_error(format!("Cannot read properties of null (reading '{field}')")));
    }
    if result[field] != true {
        return Err(LiveError::error(message));
    }
    Ok(())
}
fn fence(reference: &Value, track: &Value) -> String {
    js_json::stringify(&json!({"ref":reference,"objectIdentity":track["objectIdentity"],"viewState":state(Some(track))}))
}
impl McpHost {
    pub async fn dispatch_track_view_tool(&self, call: &ToolCall, signal: Option<&Signal>) -> Option<Result<Option<Value>, LiveError>> {
        let p = call.arguments.as_ref().unwrap_or(&Value::Null);
        Some(Ok(match call.name.as_str() {
            "live_track_view_preview" => Some(self.live_track_view_preview_async(&call.id, p).await),
            "live_track_view_apply" => self.live_track_view_apply_async(&call.id, p, signal).await,
            _ => return None,
        }))
    }
    pub async fn live_track_view_preview_async(&self, id: &Value, p: &Value) -> Value {
        if !has_only(p, &["ref", "collapsed", "deviceInsertMode", "showChains", "selectInstrument"]) || !is_non_empty_string(&p["ref"], 256)
        {
            return error(id, -32602, "ref is required", None);
        }
        if FIELDS.iter().all(|(f, _)| p.get(*f).is_none()) && p["selectInstrument"] != true {
            return error(id, -32602, "at least one view field or selectInstrument is required", None);
        }
        let result = async {
            let status = self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS)))).await?;
            if !status.connected || !status.capabilities.iter().any(|c| c.as_str() == "session.read") {
                return Err(LiveError::error("session read capability is unavailable"));
            }
            let snapshot = self.views.view_for(None, &[p["ref"].clone()], None, &[]).await?;
            let track = track(&snapshot, &p["ref"])
                .filter(|t| is_non_empty_string(&t["objectIdentity"], 256))
                .ok_or_else(|| LiveError::error("track identity is not authoritative"))?;
            let view_state = state(Some(&track));
            let revision = digest(&view_state)?;
            let mut proposed = json!({});
            for (field, _) in FIELDS {
                if let Some(v) = p.get(*field) {
                    if *field == "deviceInsertMode" {
                        if !is_integer_in_range(v, 0.0, 8.0) {
                            return Ok(error(id, -32602, "deviceInsertMode is invalid", None));
                        }
                    } else if !v.is_boolean() {
                        return Ok(error(id, -32602, &format!("{field} must be boolean"), None));
                    }
                    proposed[*field] = v.clone();
                }
            }
            if !proposed.as_object().unwrap().is_empty() && !status.has_operation("track.view.set") {
                return Err(LiveError::error("track view editing is unavailable"));
            }
            if p["selectInstrument"] == true && !status.has_operation("track.select-instrument") {
                return Err(LiveError::error("instrument selection is unavailable"));
            }
            let mut payload = json!({"ref":p["ref"]});
            payload.as_object_mut().unwrap().extend(proposed.as_object().unwrap().clone());
            payload["selectInstrument"] = json!(p["selectInstrument"]==true);
            payload["expectedObjectIdentity"] = track["objectIdentity"].clone();
            payload["expectedStateRevision"] = json!(revision);
            let t = json!({"id":tempo::transaction_id("trackview"),"epoch":status.epoch,"kind":"track-view","fence":fence(&p["ref"],&track),"clipRef":p["ref"],"payload":payload,"prior":view_state,"expiresAt":now_ms_f64()+TRANSACTION_TTL_MS,"state":"previewed"});
            self.retain_bounded_transaction(&self.clip_lifecycle_transactions, t.clone(), "track view")?;
            Ok(success_text(id, &json!({"transactionId":t["id"],"epoch":t["epoch"],"ref":p["ref"],"prior":view_state,"proposed":proposed,"selectInstrument":p["selectInstrument"]==true,"impact":"edits-track-view","confirmation":"apply","expiresAt":t["expiresAt"]})))
        }
        .await;
        result.unwrap_or_else(|e| adapter_tool_error(id, &e, "Track-view preview requires fresh authoritative state."))
    }
    pub async fn live_track_view_apply_async(&self, id: &Value, p: &Value, signal: Option<&Signal>) -> Option<Value> {
        if !valid_transaction_params(p, "apply") {
            return Some(error(id, -32602, "transactionId, confirmation=apply, and idempotencyKey are required", None));
        }
        let Some(record) = self.clip_lifecycle_transactions.get(p["transactionId"].as_str().unwrap()) else {
            return Some(transaction_error(id, "Unknown or expired track-view transaction"));
        };
        let t = record.borrow().clone();
        if t["kind"] != "track-view" || (t["state"] == "previewed" && t["expiresAt"].as_f64().unwrap_or(f64::NAN) <= now_ms_f64()) {
            return Some(transaction_error(id, "Unknown or expired track-view transaction"));
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
                    return Ok(transaction_error(id, "track identity or view state changed since preview; preview again"));
                }
            }
            record.borrow_mut()["state"] = json!("applying");
            record.borrow_mut()["applyKey"] = p["idempotencyKey"].clone();
            let edits = FIELDS.iter().any(|(f, _)| payload.get(*f).is_some());
            if edits {
                let args = device_parameter::fields(
                    payload,
                    &["ref", "collapsed", "deviceInsertMode", "showChains", "expectedObjectIdentity", "expectedStateRevision"],
                );
                let result = adapter.invoke_async(&LiveInvocation::new("track.view.set", args), Some(&context)).await?;
                confirmed(&result, "changed", "track view change was not confirmed")?;
            }
            if payload["selectInstrument"] == true {
                let snapshot = self.views.view_for(Some(&context), &[reference.clone()], None, &[]).await?;
                let track = track(&snapshot, reference);
                let mut args = json!({"ref":reference});
                if let Some(identity) = track.as_ref().and_then(|t| t.get("objectIdentity")) {
                    args["expectedObjectIdentity"] = identity.clone();
                }
                args["expectedStateRevision"] = json!(digest(&state(track.as_ref()))?);
                let result = adapter.invoke_async(&LiveInvocation::new("track.select-instrument", args), Some(&context)).await?;
                confirmed(&result, "done", "instrument selection was not confirmed")?;
            }
            if edits {
                let snapshot = self.views.view_for(Some(&context), &[reference.clone()], None, &[]).await?;
                let verified = track(&snapshot, reference);
                for (f, row) in FIELDS {
                    if let Some(v) = payload.get(*f) {
                        if !scalar_same(verified.as_ref().and_then(|t| t.get("view")).and_then(|v| v.get(*row)), Some(v)) {
                            return Err(LiveError::error("track view postcondition was not confirmed"));
                        }
                    }
                }
            }
            record.borrow_mut()["applyKey"] = p["idempotencyKey"].clone();
            record.borrow_mut()["state"] = json!("applied");
            Ok(success_text(id, &json!({"transactionId":t["id"],"state":"applied","idempotent":false})))
        }
        .await;
        Some(result.unwrap_or_else(|e| {
            self.apply_failed(id, &record, &e, "Track-view state is uncertain; perform fresh discovery before retrying.")
        }))
    }
    pub async fn undo_track_view_async(&self, id: &Value, p: &Value, signal: Option<&Signal>) -> Value {
        let Some(record) = self.clip_lifecycle_transactions.get(p["transactionId"].as_str().unwrap_or("")) else {
            return transaction_error(id, "Unknown or expired track-view transaction");
        };
        let t = record.borrow().clone();
        if t["kind"] != "track-view" {
            return transaction_error(id, "Unknown or expired track-view transaction");
        }
        if t["state"] == "undone" && t["undoKey"] == p["idempotencyKey"] {
            return success_text(id, &json!({"transactionId":t["id"],"state":"undone","idempotent":true}));
        }
        let reconcile = t["state"] == "uncertain" && t["undoKey"] == p["idempotencyKey"];
        if (t["state"] != "applied" && !reconcile) || !arrangement::truthy(&t["prior"]) {
            return transaction_error(id, "Only an applied or exact-key uncertain track-view transaction can be undone");
        }
        let payload = &t["payload"];
        if FIELDS.iter().all(|(f, _)| payload.get(*f).is_none()) {
            return transaction_error(id, "Instrument selection is momentary and not undoable");
        }
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
            let track = track(&snapshot, &payload["ref"])
                .filter(|r| is_non_empty_string(&r["objectIdentity"], 256))
                .ok_or_else(|| LiveError::error("track identity is unavailable"))?;
            if let Some(moved) = self.undo_target_moved(
                id,
                &t,
                "track",
                &payload["ref"],
                track.get("objectIdentity"),
                payload.get("expectedObjectIdentity"),
            )? {
                return Ok(moved);
            }
            if !reconcile {
                for (f, row) in FIELDS {
                    if payload.get(*f).is_some() && !scalar_same(track["view"].get(*row), payload.get(*f)) {
                        return Ok(transaction_error(id, "track view changed after apply; undo refused"));
                    }
                }
            }
            let revision = digest(&state(Some(&track)))?;
            record.borrow_mut()["state"] = json!("undoing");
            let mut args = json!({"ref":payload["ref"],"expectedObjectIdentity":track["objectIdentity"],"expectedStateRevision":revision});
            for (f, _) in FIELDS {
                if payload.get(*f).is_some()
                    && if *f == "deviceInsertMode" { t["prior"][*f].is_number() } else { t["prior"][*f].is_boolean() }
                {
                    args[*f] = t["prior"][*f].clone();
                }
            }
            let result = self.invoke_undo_recovery(&record, adapter.as_ref(), "track.view.set", &args, &context).await?;
            confirmed(&result, "changed", "track view restoration was not confirmed")?;
            record.borrow_mut()["state"] = json!("undone");
            Ok(success_text(id, &json!({"transactionId":t["id"],"state":"undone","idempotent":false})))
        }
        .await;
        result.unwrap_or_else(|e| {
            record.borrow_mut()["state"] = json!("uncertain");
            adapter_tool_error(id, &e, "Track-view undo is uncertain; perform fresh discovery.")
        })
    }
}
