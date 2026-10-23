//! Recording control fences playback state and the explicitly named armed tracks.
use super::*;
use super::{device_parameter::fields, reads::AUDITION_DEADLINE_MS};
use kumi_common::{abort::Signal, js::json as js_json, time::now_ms_f64};
fn recording_fence(transport: &Value) -> String {
    js_json::stringify(&fields(transport, &["sessionRecord", "arrangementRecord", "playing"]))
}
impl McpHost {
    pub async fn dispatch_recording_tool(&self, call: &ToolCall, signal: Option<&Signal>) -> Option<Result<Option<Value>, LiveError>> {
        let p = call.arguments.as_ref().unwrap_or(&Value::Null);
        Some(Ok(match call.name.as_str() {
            "live_recording_preview" => Some(self.live_recording_preview_async(&call.id, p).await),
            "live_recording_apply" => self.live_recording_apply_async(&call.id, p, signal).await,
            _ => return None,
        }))
    }
    pub async fn live_recording_preview_async(&self, id: &Value, p: &Value) -> Value {
        let also_valid = p.get("alsoTrackRefs").is_none_or(|also| {
            p["action"] == "start"
                && also.as_array().is_some_and(|rows| {
                    rows.len() <= 1024
                        && rows.iter().all(|r| is_non_empty_string(r, 256))
                        && rows.iter().enumerate().all(|(i, r)| r != &p["destinationTrackRef"] && !rows[..i].contains(r))
                })
        });
        if !has_only(p, &["action", "lane", "intent", "destinationTrackRef", "alsoTrackRefs", "outputSafety"])
            || !matches!(p["action"].as_str(), Some("start" | "stop"))
            || !also_valid
            || !matches!(p["lane"].as_str(), Some("session" | "arrangement"))
            || !is_non_empty_string(&p["intent"], 256)
        {
            return error(id, -32602, "action, lane, and intent are required", None);
        }
        let result = async {
            let status = self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS)))).await?;
            if !status.connected || !status.capabilities.iter().any(|c| c.as_str() == "session.read") {
                return Err(LiveError::error("session read capability is unavailable"));
            }
            let operation = if p["lane"] == "session" { "recording.session" } else { "recording.arrangement" };
            if !status.operations.iter().flatten().any(|o| o == operation) {
                return Err(LiveError::error(format!("{operation} control is unavailable")));
            }
            let snapshot = serde_json::to_value(self.views.view(None, LiveViewScope::Indices(vec![]), None).await?).unwrap();
            let transport = &snapshot["playback"]["transport"];
            if transport.is_null() {
                return Err(LiveError::error("authoritative playback state is unavailable"));
            }
            let mut identity = Value::Null;
            let mut also = vec![];
            if p["action"] == "start" {
                if !is_non_empty_string(&p["destinationTrackRef"], 256) {
                    return Err(LiveError::error("recording start requires an explicit destination track"));
                }
                let tracks = snapshot["tracks"].as_array().unwrap();
                let track = tracks
                    .iter()
                    .find(|t| t["ref"] == p["destinationTrackRef"])
                    .filter(|t| is_non_empty_string(&t["objectIdentity"], 256))
                    .ok_or_else(|| LiveError::error("destination track identity is not authoritative"))?;
                identity = track["objectIdentity"].clone();
                if track["armed"] != true {
                    return Err(LiveError::error(
                        "destination track is not armed for recording; arm it through live_routing_preview first",
                    ));
                }
                for reference in p["alsoTrackRefs"].as_array().into_iter().flatten() {
                    let track = tracks
                        .iter()
                        .find(|t| &t["ref"] == reference)
                        .filter(|t| is_non_empty_string(&t["objectIdentity"], 256))
                        .ok_or_else(|| LiveError::error("a track recorded alongside is not authoritative"))?;
                    if track["armed"] != true {
                        return Err(LiveError::error("a track recorded alongside is not armed; arm it through live_routing_preview first"));
                    }
                    also.push(json!({"ref":reference,"identity":track["objectIdentity"]}));
                }
            }
            let mut payload = json!({"action":p["action"],"lane":p["lane"],"intent":p["intent"],"outputSafety":output_safety_of(&p["outputSafety"]),"destinationTrackRef":if p["action"]=="start" {p["destinationTrackRef"].clone()}else{Value::Null},"destinationTrackIdentity":identity});
            if !also.is_empty() {
                payload["also"] = json!(also);
            }
            let t = json!({"id":tempo::transaction_id("recording"),"epoch":status.epoch,"kind":"recording","fence":recording_fence(transport),"payload":payload,"prior":fields(transport,&["sessionRecord","arrangementRecord"]),"expiresAt":now_ms_f64()+TRANSACTION_TTL_MS,"state":"previewed"});
            self.retain_bounded_transaction(&self.clip_lifecycle_transactions, t.clone(), "recording")?;
            Ok(success_text(id, &json!({"transactionId":t["id"],"epoch":t["epoch"],"action":p["action"],"lane":p["lane"],"intent":p["intent"],"prior":t["prior"],"impact":if p["action"]=="start" {"starts-recording"}else{"stops-recording"},"confirmation":"apply","expiresAt":t["expiresAt"]})))
        }
        .await;
        result.unwrap_or_else(|e| {
            adapter_tool_error(id, &e, "Nothing recorded: fix what the reason says (arm the destination first) and preview again.")
        })
    }
    pub async fn live_recording_apply_async(&self, id: &Value, p: &Value, signal: Option<&Signal>) -> Option<Value> {
        if !valid_transaction_params(p, "apply") {
            return Some(error(id, -32602, "transactionId, confirmation=apply, and idempotencyKey are required", None));
        }
        let Some(record) = self.clip_lifecycle_transactions.get(p["transactionId"].as_str().unwrap()) else {
            return Some(transaction_error(id, "Unknown or expired recording transaction"));
        };
        let t = record.borrow().clone();
        if t["kind"] != "recording" || (t["state"] == "previewed" && t["expiresAt"].as_f64().unwrap_or(f64::NAN) <= now_ms_f64()) {
            return Some(transaction_error(id, "Unknown or expired recording transaction"));
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
            let context = self.transaction_context(p, signal, AUDITION_DEADLINE_MS);
            let snapshot = serde_json::to_value(self.views.view(Some(&context), LiveViewScope::Indices(vec![]), None).await?).unwrap();
            let transport = &snapshot["playback"]["transport"];
            if !reconcile && (transport.is_null() || recording_fence(transport) != t["fence"]) {
                record.borrow_mut()["state"] = json!("uncertain");
                return Ok(transaction_error(id, "recording state changed since preview; preview again"));
            }
            if !reconcile && t["payload"]["action"] == "start" {
                let payload = &t["payload"];
                let mut expected = vec![json!({"ref":payload["destinationTrackRef"],"identity":payload["destinationTrackIdentity"]})];
                expected.extend(payload["also"].as_array().into_iter().flatten().cloned());
                let same = expected.iter().all(|e| {
                    snapshot["tracks"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .any(|track| track["armed"] == true && track["ref"] == e["ref"] && track["objectIdentity"] == e["identity"])
                });
                if !is_non_empty_string(&payload["destinationTrackRef"], 256)
                    || !is_non_empty_string(&payload["destinationTrackIdentity"], 256)
                    || !same
                {
                    record.borrow_mut()["state"] = json!("uncertain");
                    return Ok(transaction_error(id, "recording arm or destination identity changed since preview; preview again"));
                }
            }
            let payload = &t["payload"];
            let operation = if payload["lane"] == "session" { "recording.session" } else { "recording.arrangement" };
            record.borrow_mut()["state"] = json!("applying");
            record.borrow_mut()["applyKey"] = p["idempotencyKey"].clone();
            let mut args = json!({"action":payload["action"]});
            for (to, from) in [("expectedSessionRecord", "sessionRecord"), ("expectedArrangementRecord", "arrangementRecord")] {
                if let Some(v) = t["prior"].get(from) {
                    args[to] = v.clone();
                }
            }
            args["destinationTrackRef"] = payload["destinationTrackRef"].clone();
            args["destinationTrackIdentity"] = payload["destinationTrackIdentity"].clone();
            if let Some(also) = payload["also"].as_array() {
                args["alsoTrackRefs"] = json!(also.iter().map(|r|&r["ref"]).collect::<Vec<_>>());
                args["alsoTrackIdentities"] = json!(also.iter().map(|r|&r["identity"]).collect::<Vec<_>>());
            }
            args["outputSafety"] = payload["outputSafety"].clone();
            let result = self.async_adapter().invoke_async(&LiveInvocation::new(operation, args), Some(&context)).await?;
            if result.is_null() {
                return Err(LiveError::type_error("Cannot read properties of null (reading 'recording')"));
            }
            let expected = payload["action"] == "start";
            if result["recording"] != expected {
                return Err(LiveError::error("recording change was not confirmed"));
            }
            let field = if payload["lane"] == "session" { "sessionRecord" } else { "arrangementRecord" };
            let mut confirmed = false;
            while now_ms_f64() < context.deadline_ms.unwrap() - 250.0 {
                let after = serde_json::to_value(self.views.playback(Some(&context)).await?).unwrap();
                if after["transport"][field] == expected {
                    confirmed = true;
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            if !confirmed {
                return Err(LiveError::error("recording change was not confirmed by fresh playback state"));
            }
            record.borrow_mut()["applyKey"] = p["idempotencyKey"].clone();
            record.borrow_mut()["state"] = json!("applied");
            Ok(success_text(id, &json!({"transactionId":t["id"],"state":"applied","recording":result["recording"],"lane":payload["lane"],"idempotent":false})))
        }
        .await;
        Some(result.unwrap_or_else(|e| {
            let cancelled = e.message().contains("cancelled before dispatch");
            record.borrow_mut()["state"] = json!(if cancelled { "previewed" } else { "uncertain" });
            if cancelled {
                record.borrow_mut().as_object_mut().unwrap().remove("applyKey");
            }
            adapter_tool_error(id, &e, "Recording state is uncertain; perform fresh discovery and use the emergency stop path if needed.")
        }))
    }
    pub async fn undo_recording_async(&self, id: &Value, p: &Value) -> Value {
        if self.clip_lifecycle_transactions.get(p["transactionId"].as_str().unwrap_or("")).is_none_or(|r| r.borrow()["kind"] != "recording")
        {
            return transaction_error(id, "Unknown recording transaction");
        }
        transaction_error(id,"Recording start cannot be ownership-proven across later manual stop/start cycles; use a fresh live_recording_preview action=stop or emergency-stop workflow")
    }
}
