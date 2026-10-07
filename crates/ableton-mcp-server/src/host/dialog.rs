//! Dialog presses require a complete observation and the exact same dialog at apply time.
use super::{reads::AUDITION_DEADLINE_MS, track_view::confirmed, *};
use kumi_common::{abort::Signal, js::json as js_json, time::now_ms_f64};
fn dialog_state(read: &Value) -> Result<Value, LiveError> {
    if read.is_null() {
        return Err(LiveError::type_error("Cannot read properties of null (reading 'buttonCount')"));
    }
    Ok(json!({"buttonCount":read["buttonCount"],"message":read["message"],"openDialogCount":read["openDialogCount"]}))
}
fn done(read: &Value) -> Result<bool, LiveError> {
    if read.is_null() {
        return Err(LiveError::type_error("Cannot read properties of null (reading 'done')"));
    }
    Ok(read.get("done").is_none_or(|v| v == true))
}
fn fence(button: &Value, state: &Value) -> String {
    let mut value = json!({"button":button});
    value.as_object_mut().unwrap().extend(state.as_object().unwrap().clone());
    js_json::stringify(&value)
}
impl McpHost {
    pub async fn dispatch_dialog_tool(&self, call: &ToolCall, signal: Option<&Signal>) -> Option<Result<Option<Value>, LiveError>> {
        let p = call.arguments.as_ref().unwrap_or(&Value::Null);
        Some(Ok(match call.name.as_str() {
            "live_application_dialog_preview" => Some(self.live_application_dialog_preview_async(&call.id, p).await),
            "live_application_dialog_apply" => self.live_application_dialog_apply_async(&call.id, p, signal).await,
            _ => return None,
        }))
    }
    pub async fn live_application_dialog_preview_async(&self, id: &Value, p: &Value) -> Value {
        if !has_only(p, &["button"]) {
            return error(id, -32602, "button is optional; omit for a read-only dialog state check", None);
        }
        if p.get("button").is_some_and(|v| !is_integer_in_range(v, 0.0, 16.0)) {
            return error(id, -32602, "button is invalid", None);
        }
        let result = async {
            let status = self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS)))).await?;
            if !status.connected || !status.capabilities.iter().any(|c| c.as_str() == "session.read") {
                return Err(LiveError::error("session read capability is unavailable"));
            }
            if !status.has_operation("application.dialog") {
                return Err(LiveError::error("application dialog surface is unavailable"));
            }
            let read = self
                .async_adapter()
                .invoke_async(
                    &LiveInvocation::new("application.dialog", json!({"action":"read"})),
                    Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS))),
                )
                .await?;
            let state = dialog_state(&read)?;
            let done = done(&read)?;
            let Some(button) = p.get("button") else {
                let mut body = state;
                body["done"] = json!(done);
                return Ok(success_text(id, &body));
            };
            if !done {
                return Ok(adapter_tool_error(
                    id,
                    &LiveError::error("dialog observation is incomplete; guarded presses are refused"),
                    "Wait for a complete dialog observation before requesting a guarded press.",
                ));
            }
            if !state["buttonCount"].is_number() || !state["openDialogCount"].is_number() {
                return Err(LiveError::error("dialog shape is not observable; guarded presses are refused"));
            }
            if button.as_f64().unwrap() >= state["buttonCount"].as_f64().unwrap() {
                return Ok(error(id, -32602, "button is not present in the current dialog", None));
            }
            let payload = json!({"action":"press","button":button,"expectedMessage":state["message"],"expectedButtonCount":state["buttonCount"],"expectedOpenDialogCount":state["openDialogCount"]});
            let t = json!({"id":tempo::transaction_id("dialog"),"epoch":status.epoch,"kind":"dialog","fence":fence(button,&state),"payload":payload,"prior":state,"expiresAt":now_ms_f64()+TRANSACTION_TTL_MS,"state":"previewed"});
            self.retain_bounded_transaction(&self.clip_lifecycle_transactions, t.clone(), "dialog")?;
            let mut body = json!({"transactionId":t["id"],"epoch":t["epoch"]});
            body.as_object_mut().unwrap().extend(state.as_object().unwrap().clone());
            body["button"] = button.clone();
            body["impact"] = json!("presses-dialog-button-potentially-destructive");
            body["confirmation"] = json!("apply");
            body["expiresAt"] = t["expiresAt"].clone();
            Ok(success_text(id, &body))
        }
        .await;
        result.unwrap_or_else(|e| adapter_tool_error(id, &e, "Dialog preview requires a fresh connection."))
    }
    pub async fn live_application_dialog_apply_async(&self, id: &Value, p: &Value, signal: Option<&Signal>) -> Option<Value> {
        if !valid_transaction_params(p, "apply") {
            return Some(error(id, -32602, "transactionId, confirmation=apply, and idempotencyKey are required", None));
        }
        let Some(record) = self.clip_lifecycle_transactions.get(p["transactionId"].as_str().unwrap()) else {
            return Some(transaction_error(id, "Unknown or expired dialog transaction"));
        };
        let t = record.borrow().clone();
        if t["kind"] != "dialog" || (t["state"] == "previewed" && t["expiresAt"].as_f64().unwrap_or(f64::NAN) <= now_ms_f64()) {
            return Some(transaction_error(id, "Unknown or expired dialog transaction"));
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
            let read = adapter.invoke_async(&LiveInvocation::new("application.dialog", json!({"action":"read"})), Some(&context)).await?;
            if !done(&read)? {
                return Ok(adapter_tool_error(
                    id,
                    &LiveError::error("dialog observation is incomplete; the press was refused"),
                    "Wait for a complete dialog observation, then request a fresh preview.",
                ));
            }
            if json!(fence(&t["payload"]["button"], &dialog_state(&read)?)) != t["fence"] {
                return Ok(transaction_error(id, "dialog state changed since preview; the press was refused"));
            }
            record.borrow_mut()["state"] = json!("applying");
            record.borrow_mut()["applyKey"] = p["idempotencyKey"].clone();
            let result = adapter.invoke_async(&LiveInvocation::new("application.dialog", t["payload"].clone()), Some(&context)).await?;
            confirmed(&result, "done", "dialog press was not confirmed")?;
            record.borrow_mut()["applyKey"] = p["idempotencyKey"].clone();
            record.borrow_mut()["state"] = json!("applied");
            Ok(success_text(id, &json!({"transactionId":t["id"],"state":"applied","stateAfter":dialog_state(&result)?,"idempotent":false})))
        }
        .await;
        Some(result.unwrap_or_else(|e| apply_failed(id, &record, &e, "Dialog state is uncertain; inspect Live before retrying.")))
    }
}
