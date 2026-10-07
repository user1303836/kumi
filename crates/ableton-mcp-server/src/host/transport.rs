//! Bounded transport writes and ownership-aware restoration of their prior fields.
use super::*;
use kumi_common::abort::{Signal, SignalExt};
use retention::TransactionRecord;
const FIELDS: &[&str] = &["position", "loopEnabled", "loopStart", "loopLength", "metronome", "punchIn", "punchOut"];
const PARTS: &[LiveSnapshotPart] = &[LiveSnapshotPart::Set, LiveSnapshotPart::Playback];
fn field_is(transport: &Value, field: &str, value: &Value) -> bool {
    if field == "position" {
        let (Some(current), Some(value)) = (transport["position"].as_f64(), value.as_f64()) else { return false };
        return if transport["playing"] == true {
            current >= value - 0.26 && current <= value + 2.5
        } else {
            (current - value).abs() <= 0.26
        };
    }
    let observed = match field {
        "loopEnabled" => &transport["loop"]["enabled"],
        "loopStart" => &transport["loop"]["start"],
        "loopLength" => &transport["loop"]["length"],
        _ => &transport[field],
    };
    if value.is_number() {
        same_live_value(Some(observed), Some(value))
    } else {
        observed == value
    }
}
fn property<'a>(result: &'a Value, key: &str) -> Result<&'a Value, LiveError> {
    if result.is_null() {
        Err(LiveError::type_error(format!("Cannot read properties of null (reading '{key}')")))
    } else {
        Ok(&result[key])
    }
}
impl McpHost {
    pub async fn dispatch_transport_tool(&self, call: &ToolCall, signal: Option<&Signal>) -> Option<Result<Option<Value>, LiveError>> {
        let p = call.arguments.as_ref().unwrap_or(&Value::Null);
        Some(Ok(match call.name.as_str() {
            "live_transport_preview" => Some(self.live_transport_preview_async(&call.id, p).await),
            "live_transport_apply" => self.live_transport_apply_async(&call.id, p, signal).await,
            _ => return None,
        }))
    }
    pub async fn live_transport_preview_async(&self, id: &Value, params: &Value) -> Value {
        if !has_only(params, FIELDS) {
            return error(id, -32602, "only bounded transport fields are accepted", None);
        }
        let mut proposed = json!({});
        for field in FIELDS {
            let Some(value) = params.get(*field) else { continue };
            if ["loopEnabled", "metronome", "punchIn", "punchOut"].contains(field) {
                if !value.is_boolean() {
                    return error(id, -32602, &format!("{field} must be boolean"), None);
                }
            } else if !value.as_f64().is_some_and(|n| n.is_finite() && n >= 0.0 && (*field != "loopLength" || n > 0.0)) {
                return error(id, -32602, &format!("{field} is out of bounds"), None);
            }
            proposed[*field] = value.clone();
        }

        if proposed.as_object().unwrap().is_empty() {
            return error(id, -32602, "at least one transport field is required", None);
        }
        let result=async{
            let status=self.require_connected(Some("transport"))?;
            let snapshot=serde_json::to_value(self.views.view(None,LiveViewScope::Indices(vec![]),Some(PARTS)).await?).unwrap();
            let transport=&snapshot["playback"]["transport"];
            if transport.is_null()||transport["loop"].is_null()||!is_non_empty_string(&snapshot["set"]["objectIdentity"],256){return Ok(transaction_error(id,"authoritative transport Set identity is unavailable"));}
            let prior=project_fields(transport,&["position","loop","punchIn","punchOut","metronome"]);
            let t=json!({
"id":tempo::transaction_id("transport"),
"epoch":status.epoch,
"setRef":snapshot["set"]["ref"],
"setIdentity":snapshot["set"]["objectIdentity"],
"prior":prior,
"proposed":proposed,
"playbackRevision":snapshot["playback"]["revision"],
"expiresAt":kumi_common::time::now_ms_f64()+TRANSACTION_TTL_MS,
"state":"previewed"}
);

            self.retain_bounded_transaction(&self.transport_transactions,t.clone(),"transport")?;
            Ok(success_text(id,&json!({"transactionId":t["id"],"epoch":t["epoch"],"prior":t["prior"],"proposed":proposed,"playbackRevision":t["playbackRevision"],"impact":"transport-state","confirmation":"apply","expiresAt":t["expiresAt"]})))
        }.await;
        result.unwrap_or_else(|e| adapter_tool_error(id, &e, "Transport preview requires fresh authoritative playback state."))
    }
    async fn confirm_transport_fields(&self, context: &LiveOperationContext, fields: &Value) -> Result<Value, LiveError> {
        loop {
            let after = serde_json::to_value(self.views.playback(Some(context)).await?).unwrap();
            if fields.as_object().unwrap().iter().all(|(field, value)| field_is(&after["transport"], field, value)) {
                return Ok(after);
            }
            if kumi_common::time::now_ms_f64() >= context.deadline_ms.unwrap() - 250.0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }

        Err(LiveError::error("transport change was not confirmed by fresh playback state"))
    }
    pub async fn live_transport_apply_async(&self, id: &Value, params: &Value, signal: Option<&Signal>) -> Option<Value> {
        if !valid_transaction_params(params, "apply") {
            return Some(error(id, -32602, "transactionId, confirmation=apply, and idempotencyKey are required", None));
        }
        let Some(record) = self.transport_transactions.get(params["transactionId"].as_str().unwrap()) else {
            return Some(transaction_error(id, "Unknown or expired transport transaction"));
        };
        let t = record.borrow().clone();
        if t["state"] == "previewed" && t["expiresAt"].as_f64().is_some_and(|n| n <= kumi_common::time::now_ms_f64()) {
            return Some(transaction_error(id, "Unknown or expired transport transaction"));
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
            let status = self.require_connected(Some("transport"))?;
            if json!(status.epoch) != t["epoch"] {
                return Ok(transaction_error(id, "Live connection epoch changed; preview again"));
            }
            let adapter = self.async_adapter();
            let context = self.transaction_context(params, signal, reads::AUDITION_DEADLINE_MS);
            let snapshot =
                serde_json::to_value(self.views.view(Some(&context), LiveViewScope::Indices(vec![]), Some(PARTS)).await?).unwrap();
            if !reconciliation
                && (snapshot["set"]["ref"] != t["setRef"]
                    || snapshot["set"]["objectIdentity"] != t["setIdentity"]
                    || snapshot["playback"]["revision"] != t["playbackRevision"])
            {
                return Ok(transaction_error(id, "transport Set identity or state changed since preview; preview again"));
            }
            let mut args = t["proposed"].clone();
            args["expectedRevision"] = t["playbackRevision"].clone();
            args["setRef"] = t["setRef"].clone();
            args["expectedObjectIdentity"] = t["setIdentity"].clone();
            let result = adapter.invoke_async(&LiveInvocation::new("transport.set", args), Some(&context)).await?;
            if property(&result, "changed")? != true || !result["revision"].is_string() {
                return Err(LiveError::error("transport change was not confirmed"));
            }
            let confirmed = self.confirm_transport_fields(&context, &t["proposed"]).await?;
            {
                let mut row = record.borrow_mut();
                row["appliedRevision"] = confirmed["revision"].clone();
                row["applyKey"] = params["idempotencyKey"].clone();
                row["state"] = json!("applied");
            }
            Ok(success_text(id, &json!({"transactionId":t["id"],"state":"applied","revision":result["revision"],"idempotent":false})))
        }
        .await;
        Some(result.unwrap_or_else(|cause| {
            if nothing_changed(&cause) && !reconciliation {
                record.borrow_mut()["state"] = json!("undone");
                adapter_tool_error(id, &cause, PREVIEW_AGAIN)
            } else {
                record.borrow_mut()["state"] = json!("uncertain");
                adapter_tool_error(id, &cause, "Transport state is uncertain; perform fresh discovery before retrying.")
            }
        }))
    }
    pub async fn undo_transport_async(&self, id: &Value, params: &Value, signal: Option<&Signal>) -> Value {
        let Some(record) = params["transactionId"].as_str().and_then(|id| self.transport_transactions.get(id)) else {
            return transaction_error(id, "Unknown or expired transport transaction");
        };
        self.live_transport_undo_async(id, &record, params, signal).await
    }
    async fn live_transport_undo_async(&self, id: &Value, record: &TransactionRecord, params: &Value, signal: Option<&Signal>) -> Value {
        let t = record.borrow().clone();
        if t["state"] == "undone" && t["undoKey"] == params["idempotencyKey"] {
            return success_text(id, &json!({"transactionId":t["id"],"state":"undone","idempotent":true}));
        }
        let reconciliation = t["state"] == "uncertain" && t["undoKey"] == params["idempotencyKey"];
        if t["state"] != "applied" && !reconciliation {
            return transaction_error(id, "Only an applied or exact-key uncertain transport transaction can be undone");
        }
        let refuse = |message: &str| {
            self.delete_undo_plan(record);
            transaction_error(id, message)
        };
        let result = async {
            self.begin_undo_recovery(record, params["idempotencyKey"].as_str().unwrap())?;
            let status = self.require_connected(Some("transport"))?;
            if json!(status.epoch) != t["epoch"] {
                return Ok(refuse("Live connection epoch changed; undo refused"));
            }
            let adapter = self.async_adapter();
            let context = self.transaction_context(params, signal, reads::AUDITION_DEADLINE_MS);
            if reconciliation {
                self.replay_undo_recovery(record, adapter.as_ref(), &context).await?;
            }
            let snapshot =
                serde_json::to_value(self.views.view(Some(&context), LiveViewScope::Indices(vec![]), Some(PARTS)).await?).unwrap();
            let prior = &t["prior"];
            let mut restore = json!({});
            for field in t["proposed"].as_object().unwrap().keys() {
                let value = match field.as_str() {
                    "loopEnabled" => &prior["loop"]["enabled"],
                    "loopStart" => &prior["loop"]["start"],
                    "loopLength" => &prior["loop"]["length"],
                    _ => &prior[field],
                };
                let boolean = ["loopEnabled", "metronome", "punchIn", "punchOut"].contains(&field.as_str());
                if (boolean && value.is_boolean()) || (!boolean && value.is_number()) {
                    restore[field] = value.clone();
                }
            }

            if snapshot["set"]["ref"] != t["setRef"] || snapshot["set"]["objectIdentity"] != t["setIdentity"] {
                return Ok(refuse("transport Set identity changed after apply; undo refused"));
            }
            let current = &snapshot["playback"]["transport"];
            if !reconciliation && current["playing"] == true {
                restore.as_object_mut().unwrap().remove("position");
            }
            if !reconciliation && restore.as_object().unwrap().is_empty() {
                return Ok(refuse("transport undo refused while playing: only the playhead changed; stop playback first"));
            }
            if reconciliation {
                self.confirm_transport_fields(&context, &restore)
                    .await
                    .map_err(|_| LiveError::error("transport undo replay did not restore the exact prior state"))?;
            } else {
                for (field, proposed) in t["proposed"].as_object().unwrap() {
                    if field != "position" && !field_is(current, field, proposed) {
                        return Ok(refuse("transport field changed after apply; undo refused"));
                    }
                }
                let mut args = restore.clone();
                args["expectedRevision"] = snapshot["playback"]["revision"].clone();
                args["setRef"] = t["setRef"].clone();
                args["expectedObjectIdentity"] = t["setIdentity"].clone();
                let result = self.invoke_undo_recovery(record, adapter.as_ref(), "transport.set", &args, &context).await?;
                if property(&result, "changed")? != true {
                    return Err(LiveError::error("transport undo was not confirmed"));
                }
            }
            self.confirm_transport_fields(&context, &restore).await?;
            {
                let mut row = record.borrow_mut();
                row["undoKey"] = params["idempotencyKey"].clone();
                row["state"] = json!("undone");
            }
            Ok(success_text(id, &json!({"transactionId":t["id"],"state":"undone","restored":restore,"idempotent":false})))
        }
        .await;
        result.unwrap_or_else(|cause| {
            record.borrow_mut()["state"] = json!("uncertain");
            adapter_tool_error(id, &cause, "Transport undo is uncertain; perform fresh discovery.")
        })
    }
}
fn project_fields(row: &Value, names: &[&str]) -> Value {
    let mut result = json!({});
    for name in names {
        if let Some(value) = row.get(*name) {
            result[*name] = value.clone();
        }
    }
    result
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn field_verification_matches_source() {
        let fixture: Value = serde_json::from_str(include_str!("../../tests/fixtures/host-transport-oracle.json")).unwrap();
        for row in fixture["fields"].as_array().unwrap() {
            assert_eq!(
                field_is(&row["transport"], row["field"].as_str().unwrap(), &row["value"]),
                row["result"].as_bool().unwrap(),
                "{row}"
            );
        }
    }
}
