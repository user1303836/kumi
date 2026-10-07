//! Launch-button presses are fenced on the current Session object's identity.
use super::{reads::AUDITION_DEADLINE_MS, *};
use kumi_common::{abort::Signal, js::json as js_json, time::now_ms_f64};
fn target(snapshot: &LiveSnapshot, reference: &Value) -> Result<Value, LiveError> {
    let snapshot = serde_json::to_value(snapshot).unwrap();
    for track in snapshot["tracks"].as_array().into_iter().flatten() {
        for (field, kind) in [("clips", "clip"), ("clipSlots", "clip-slot")] {
            if let Some(row) =
                track[field].as_array().into_iter().flatten().filter(|v| v.is_object()).find(|v| v.get("ref") == Some(reference))
            {
                if is_non_empty_string(&row["objectIdentity"], 256) {
                    let name = if kind == "clip-slot" { &track["name"] } else { &row["name"] };
                    return Ok(
                        json!({"kind":kind,"objectIdentity":row["objectIdentity"],"name":if name.is_null() { String::new() } else { js_string(name)? }}),
                    );
                }
            }
        }
    }
    if let Some(row) = snapshot["scenes"].as_array().into_iter().flatten().find(|v| v.get("ref") == Some(reference)) {
        if is_non_empty_string(&row["objectIdentity"], 256) {
            return Ok(
                json!({"kind":"scene","objectIdentity":row["objectIdentity"],"name":if row["name"].is_null() { String::new() } else { js_string(&row["name"])? }}),
            );
        }
    }
    Err(LiveError::error("that has no launch button: name a Session clip, a clip slot or a scene"))
}
impl McpHost {
    pub async fn dispatch_fire_button_tool(&self, call: &ToolCall, signal: Option<&Signal>) -> Option<Result<Option<Value>, LiveError>> {
        let p = call.arguments.as_ref().unwrap_or(&Value::Null);
        Some(Ok(match call.name.as_str() {
            "live_fire_button_preview" => Some(self.live_fire_button_preview_async(&call.id, p).await),
            "live_fire_button_apply" => self.live_fire_button_apply_async(&call.id, p, signal).await,
            _ => return None,
        }))
    }
    pub async fn live_fire_button_preview_async(&self, id: &Value, p: &Value) -> Value {
        if !has_only(p, &["ref", "pressed", "outputSafety"]) || !is_non_empty_string(&p["ref"], 256) || !p["pressed"].is_boolean() {
            return error(id, -32602, "ref, pressed and outputSafety are required", None);
        }
        let result = async {
            let status = self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS)))).await?;
            if !status.has_operation("fire-button.set") {
                return Err(LiveError::error("launch buttons are unavailable on this Live shape"));
            }
            let snapshot = self
                .views
                .view_for(Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS))), &[p["ref"].clone()], None, &[])
                .await?;
            let target = target(&snapshot, &p["ref"])?;
            let payload = json!({"ref":p["ref"],"pressed":p["pressed"],"expectedObjectIdentity":target["objectIdentity"],"outputSafety":output_safety_of(&p["outputSafety"])});
            let t = json!({"id":tempo::transaction_id("firebutton"),"epoch":status.epoch,"kind":"fire-button","fence":js_json::stringify(&json!({"ref":p["ref"],"objectIdentity":target["objectIdentity"]})),"payload":payload,"prior":{"kind":target["kind"]},"expiresAt":now_ms_f64()+TRANSACTION_TTL_MS,"state":"previewed"});
            self.retain_bounded_transaction(&self.clip_lifecycle_transactions, t.clone(), "launch button")?;
            Ok(success_text(id, &json!({"transactionId":t["id"],"epoch":t["epoch"],"target":{"ref":p["ref"],"kind":target["kind"],"name":target["name"]},"pressed":p["pressed"],"impact":if p["pressed"]==true {"presses-launch-button-audible"} else {"releases-launch-button"},"confirmation":"apply","expiresAt":t["expiresAt"]})))
        }
        .await;
        result.unwrap_or_else(|e| {
            adapter_tool_error(id, &e, "Nothing was pressed; discover the clip, slot or scene again and give output-safety evidence.")
        })
    }
    pub async fn live_fire_button_apply_async(&self, id: &Value, p: &Value, signal: Option<&Signal>) -> Option<Value> {
        if !valid_transaction_params(p, "apply") {
            return Some(error(id, -32602, "transactionId, confirmation=apply, and idempotencyKey are required", None));
        }
        let Some(record) = self.clip_lifecycle_transactions.get(p["transactionId"].as_str().unwrap()) else {
            return Some(transaction_error(id, "Unknown or expired launch-button transaction"));
        };
        let t = record.borrow().clone();
        if t["kind"] != "fire-button" || (t["state"] == "previewed" && t["expiresAt"].as_f64().unwrap_or(f64::NAN) <= now_ms_f64()) {
            return Some(transaction_error(id, "Unknown or expired launch-button transaction"));
        }
        if t["state"] == "applied" && t["applyKey"] == p["idempotencyKey"] {
            return Some(success_text(id, &json!({"transactionId":t["id"],"state":"applied","idempotent":true})));
        }
        if t["state"] != "previewed" {
            return Some(transaction_error(id, "Transaction is no longer applicable: preview again"));
        }
        if signal.is_some_and(Signal::is_cancelled) {
            return None;
        }
        let result = async {
            let status = self.require_connected(None)?;
            if json!(status.epoch) != t["epoch"] {
                return Ok(transaction_error(id, "Live connection epoch changed; preview again"));
            }
            let context = self.transaction_context(p, signal, AUDITION_DEADLINE_MS);
            let snapshot = self.views.view_for(Some(&context), &[t["payload"]["ref"].clone()], None, &[]).await?;
            let target = target(&snapshot, &t["payload"]["ref"])?;
            if json!(js_json::stringify(&json!({"ref":t["payload"]["ref"],"objectIdentity":target["objectIdentity"]}))) != t["fence"] {
                return Ok(transaction_error(id, "the clip, slot or scene changed since the preview; preview again"));
            }
            record.borrow_mut()["state"] = json!("applying");
            record.borrow_mut()["applyKey"] = p["idempotencyKey"].clone();
            let result =
                self.async_adapter().invoke_async(&LiveInvocation::new("fire-button.set", t["payload"].clone()), Some(&context)).await?;
            if result.is_null() {
                return Err(LiveError::type_error("Cannot read properties of null (reading 'pressed')"));
            }
            if result.get("pressed") != t["payload"].get("pressed") {
                return Err(LiveError::error("the launch button wasn't confirmed"));
            }
            record.borrow_mut()["state"] = json!("applied");
            let mut body = json!({"transactionId":t["id"],"state":"applied","pressed":result["pressed"]});
            if result["pressed"] == true {
                body["held"] = json!("until it's let go (pressed: false), Kumi disconnects, or 30 s after the press");
            }
            body["idempotent"] = json!(false);
            Ok(success_text(id, &body))
        }
        .await;
        Some(result.unwrap_or_else(|e| {
            apply_failed(id, &record, &e, "Whether the button is down is uncertain: let go of it with pressed: false.")
        }))
    }
    pub fn undo_fire_button(&self, id: &Value) -> Value {
        reason_error(
            id,
            "A launch button's press changes nothing in the Set: there's nothing to undo.",
            "Let go of it with live_fire_button_preview (pressed: false), or stop what plays.",
        )
    }
}
