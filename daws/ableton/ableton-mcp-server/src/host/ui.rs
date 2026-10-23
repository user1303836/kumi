//! Visible Live views and locator jumps preserve their distinct preview fences.
use super::reads::AUDITION_DEADLINE_MS;
use super::*;
use kumi_common::{abort::Signal, js::json as js_json, time::now_ms_f64};
use sha2::{Digest, Sha256};
fn locators(snapshot: &Value) -> &[Value] {
    snapshot["arrangement"]["locators"].as_array().map(Vec::as_slice).unwrap_or(&[])
}
fn locator_times(snapshot: &Value) -> Vec<f64> {
    let mut times: Vec<_> = locators(snapshot).iter().filter_map(|v| v["position"].as_f64().filter(|n| n.is_finite())).collect();
    times.sort_by(f64::total_cmp);
    times
}
fn locator_ref_fence(snapshot: &Value, reference: &Value) -> String {
    let mut fence = json!({"ref":reference});
    if let Some(identity) = locators(snapshot).iter().find(|v| v["ref"] == *reference).and_then(|v| v.get("objectIdentity")) {
        fence["objectIdentity"] = identity.clone();
    }
    fence["locators"] = json!(locators(snapshot).iter().map(|v| &v["position"]).collect::<Vec<_>>());
    js_json::stringify(&fence)
}
impl McpHost {
    pub async fn dispatch_ui_tool(&self, call: &ToolCall, signal: Option<&Signal>) -> Option<Result<Option<Value>, LiveError>> {
        let p = call.arguments.as_ref().unwrap_or(&Value::Null);
        Some(Ok(match call.name.as_str() {
            "live_view_preview" => Some(self.live_view_preview_async(&call.id, p).await),
            "live_view_apply" => self.live_view_apply_async(&call.id, p, signal).await,
            "live_locator_jump_preview" => Some(self.live_locator_jump_preview_async(&call.id, p).await),
            "live_locator_jump_apply" => self.live_locator_jump_apply_async(&call.id, p, signal).await,
            _ => return None,
        }))
    }
    pub async fn live_view_preview_async(&self, id: &Value, p: &Value) -> Value {
        let actions = [
            "zoom-in",
            "zoom-out",
            "scroll-left",
            "scroll-right",
            "follow-on",
            "follow-off",
            "collapse-track",
            "expand-track",
            "hide-view",
            "focus-view",
            "browser-toggle",
        ];
        if !has_only(p, &["view", "action", "trackRef"]) {
            return error(id, -32602, "view or action is required", None);
        }
        if matches!(p["action"].as_str(), Some("hide-view" | "focus-view")) {
            if !is_non_empty_string(&p["view"], 64) {
                return error(id, -32602, "view name is required for hide/focus actions", None);
            }
        } else if p.get("view").is_none() == p.get("action").is_none() {
            return error(id, -32602, "exactly one of view or action is required", None);
        }
        if p.get("view").is_some_and(|v| !is_non_empty_string(v, 64)) {
            return error(id, -32602, "view must be a 1-64 character string", None);
        }
        if p.get("action").is_some_and(|v| !v.as_str().is_some_and(|s| actions.contains(&s))) {
            return error(id, -32602, "action is invalid", None);
        }
        let result = async {
            let status = self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS)))).await?;
            if !status.connected || !status.capabilities.iter().any(|c| c.as_str() == "session.read") {
                return Err(LiveError::error("session read capability is unavailable"));
            }
            let operation = if p.get("view").is_some() && p.get("action").is_none() { "view.set" } else { "view.control" };
            if !status.has_operation(operation) {
                return Err(LiveError::error(format!("{operation} is unavailable")));
            }
            let mut proposed = json!({});
            if matches!(p["action"].as_str(), Some("hide-view" | "focus-view")) {
                proposed["action"] = p["action"].clone();
                proposed["view"] = p["view"].clone();
            } else if let Some(view) = p.get("view") {
                proposed["view"] = view.clone();
            } else {
                proposed["action"] = p["action"].clone();
                if matches!(p["action"].as_str(), Some("collapse-track" | "expand-track")) {
                    if !is_non_empty_string(&p["trackRef"], 256) {
                        return Ok(error(id, -32602, "trackRef is required for track collapse actions", None));
                    }
                    let snapshot = serde_json::to_value(
                        self.views.view(None, LiveViewScope::Indices(vec![]), Some(&[LiveSnapshotPart::Tracks])).await?,
                    )
                    .unwrap();
                    if !snapshot["tracks"].as_array().into_iter().flatten().any(|t| t["ref"] == p["trackRef"]) {
                        return Err(LiveError::error("track reference is unknown"));
                    }
                    proposed["trackRef"] = p["trackRef"].clone();
                }
            }
            let snapshot =
                serde_json::to_value(self.views.view(None, LiveViewScope::Indices(vec![]), Some(&[LiveSnapshotPart::Set])).await?).unwrap();
            let prior = json!({"view":snapshot["view"]});
            let mut payload = json!({"operation":operation});
            payload.as_object_mut().unwrap().extend(proposed.as_object().unwrap().clone());
            let t = json!({"id":tempo::transaction_id("view"),"epoch":status.epoch,"kind":"view","fence":js_json::stringify(&json!({"operation":operation,"proposed":proposed,"epoch":status.epoch})),"payload":payload,"prior":prior,"expiresAt":now_ms_f64()+TRANSACTION_TTL_MS,"state":"previewed"});
            self.retain_bounded_transaction(&self.clip_lifecycle_transactions, t.clone(), "view")?;
            Ok(success_text(id, &json!({"transactionId":t["id"],"epoch":t["epoch"],"operation":operation,"proposed":proposed,"prior":prior,"impact":"changes-live-ui","confirmation":"apply","expiresAt":t["expiresAt"]})))
        }
        .await;
        result.unwrap_or_else(|e| adapter_tool_error(id, &e, "View preview requires a fresh connection."))
    }
    pub async fn live_view_apply_async(&self, id: &Value, p: &Value, signal: Option<&Signal>) -> Option<Value> {
        if !valid_transaction_params(p, "apply") {
            return Some(error(id, -32602, "transactionId, confirmation=apply, and idempotencyKey are required", None));
        }
        let Some(record) = self.clip_lifecycle_transactions.get(p["transactionId"].as_str().unwrap()) else {
            return Some(transaction_error(id, "Unknown or expired view transaction"));
        };
        let t = record.borrow().clone();
        if t["kind"] != "view" || (t["state"] == "previewed" && t["expiresAt"].as_f64().unwrap_or(f64::NAN) <= now_ms_f64()) {
            return Some(transaction_error(id, "Unknown or expired view transaction"));
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
            let operation = t["payload"]["operation"].as_str().unwrap_or("");
            if !["view.set", "view.control"].contains(&operation) {
                return Ok(transaction_error(id, "view transaction payload is invalid"));
            }
            let mut args = t["payload"].clone();
            args.as_object_mut().unwrap().remove("operation");
            record.borrow_mut()["state"] = json!("applying");
            record.borrow_mut()["applyKey"] = p["idempotencyKey"].clone();
            let context = self.transaction_context(p, signal, AUDITION_DEADLINE_MS);
            let result = self.async_adapter().invoke_async(&LiveInvocation::new(operation, args), Some(&context)).await?;
            let field = if operation == "view.set" { "visible" } else { "done" };
            if result.is_null() {
                return Err(LiveError::type_error(format!("Cannot read properties of null (reading '{field}')")));
            }
            if result[field] != true {
                return Err(LiveError::error(if operation == "view.set" {
                    "view change was not confirmed"
                } else {
                    "view control was not confirmed"
                }));
            }
            record.borrow_mut()["applyKey"] = p["idempotencyKey"].clone();
            record.borrow_mut()["state"] = json!("applied");
            Ok(success_text(id, &json!({"transactionId":t["id"],"state":"applied","result":result,"idempotent":false})))
        }
        .await;
        Some(
            result.unwrap_or_else(|e| {
                self.apply_failed(id, &record, &e, "View state is uncertain; check Live's visible view before retrying.")
            }),
        )
    }
    pub async fn live_locator_jump_preview_async(&self, id: &Value, p: &Value) -> Value {
        if !has_only(p, &["direction", "ref"]) {
            return error(id, -32602, "direction (next|previous) or ref is required", None);
        }
        if p.get("direction").is_none() == p.get("ref").is_none() {
            return error(id, -32602, "exactly one of direction or ref is required", None);
        }
        if p.get("direction").is_some_and(|v| !matches!(v.as_str(), Some("next" | "previous"))) {
            return error(id, -32602, "direction must be next or previous", None);
        }
        if p.get("ref").is_some_and(|v| !is_non_empty_string(v, 256)) {
            return error(id, -32602, "ref is invalid", None);
        }
        let result = async {
            let status = self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS)))).await?;
            if !status.connected || !status.capabilities.iter().any(|c| c.as_str() == "session.read") {
                return Err(LiveError::error("session read capability is unavailable"));
            }
            let by_ref = p.get("ref").is_some();
            let operation = if by_ref { "locator.jump-to" } else { "locator.jump" };
            if !status.has_operation(operation) {
                return Err(LiveError::error(if by_ref { "locator jump-to is unavailable" } else { "locator jump is unavailable" }));
            }
            let snapshot = serde_json::to_value(
                self.views
                    .view(None, LiveViewScope::Indices(vec![]), Some(&[LiveSnapshotPart::Arrangement, LiveSnapshotPart::Playback]))
                    .await?,
            )
            .unwrap();
            let position = snapshot["playback"]["transport"]["position"].as_f64().unwrap_or(0.0);
            let (payload, prior, fence, mut body) = if by_ref {
                let Some(locator) =
                    locators(&snapshot).iter().find(|l| l["ref"] == p["ref"]).filter(|l| is_non_empty_string(&l["objectIdentity"], 256))
                else {
                    return Ok(transaction_error(id, "locator reference is unknown"));
                };
                let revision = hex::encode(Sha256::digest(canonical_mutation_identity(&snapshot["arrangement"]["locators"])?));
                (json!({"jumpTo":true,"ref":p["ref"],"expectedObjectIdentity":locator["objectIdentity"],"expectedCollectionRevision":revision}), json!({"position":position,"target":locator["position"]}), locator_ref_fence(&snapshot, &p["ref"]), json!({"ref":p["ref"],"target":locator["position"]}))
            } else {
                let times = locator_times(&snapshot);
                let target = if p["direction"] == "next" {
                    times.iter().find(|time| **time > position + 1e-9)
                } else {
                    times.iter().rev().find(|time| **time < position - 1e-9)
                };
                (json!({"direction":p["direction"]}), json!({"position":position,"target":target}), js_json::stringify(&json!({"direction":p["direction"],"position":position,"locators":times})), json!({"direction":p["direction"],"current":position,"target":target}))
            };
            let t = json!({"id":tempo::transaction_id("locjump"),"epoch":status.epoch,"kind":"locator-jump","fence":fence,"payload":payload,"prior":prior,"expiresAt":now_ms_f64()+TRANSACTION_TTL_MS,"state":"previewed"});
            self.retain_bounded_transaction(&self.clip_lifecycle_transactions, t.clone(), "locator jump")?;
            body["transactionId"] = t["id"].clone();
            body["epoch"] = t["epoch"].clone();
            body["impact"] = json!("moves-playhead");
            body["confirmation"] = json!("apply");
            body["expiresAt"] = t["expiresAt"].clone();
            Ok(success_text(id, &body))
        }
        .await;
        result.unwrap_or_else(|e| adapter_tool_error(id, &e, "Locator-jump preview requires fresh authoritative state."))
    }
    pub async fn live_locator_jump_apply_async(&self, id: &Value, p: &Value, signal: Option<&Signal>) -> Option<Value> {
        if !valid_transaction_params(p, "apply") {
            return Some(error(id, -32602, "transactionId, confirmation=apply, and idempotencyKey are required", None));
        }
        let Some(record) = self.clip_lifecycle_transactions.get(p["transactionId"].as_str().unwrap()) else {
            return Some(transaction_error(id, "Unknown or expired locator-jump transaction"));
        };
        let t = record.borrow().clone();
        if t["kind"] != "locator-jump" || (t["state"] == "previewed" && t["expiresAt"].as_f64().unwrap_or(f64::NAN) <= now_ms_f64()) {
            return Some(transaction_error(id, "Unknown or expired locator-jump transaction"));
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
            let by_ref = t["payload"]["jumpTo"] == true;
            if !reconcile {
                let snapshot = serde_json::to_value(
                    self.views
                        .view(
                            Some(&context),
                            LiveViewScope::Indices(vec![]),
                            Some(&[LiveSnapshotPart::Arrangement, LiveSnapshotPart::Playback]),
                        )
                        .await?,
                )
                .unwrap();
                if by_ref {
                    if !locators(&snapshot).iter().any(|l| l["ref"] == t["payload"]["ref"])
                        || locator_ref_fence(&snapshot, &t["payload"]["ref"]) != t["fence"]
                    {
                        return Ok(transaction_error(id, "locator identity or collection changed since preview; preview again"));
                    }
                } else {
                    let position = snapshot["playback"]["transport"]["position"].as_f64().unwrap_or(0.0);
                    let times = locator_times(&snapshot);
                    if js_json::stringify(&json!({"direction":t["payload"]["direction"],"position":position,"locators":times}))
                        != t["fence"]
                    {
                        return Ok(transaction_error(id, "Playhead or locators changed since preview; preview again"));
                    }
                }
            }
            record.borrow_mut()["state"] = json!("applying");
            record.borrow_mut()["applyKey"] = p["idempotencyKey"].clone();
            let operation = if by_ref { "locator.jump-to" } else { "locator.jump" };
            let args = if by_ref {
                device_parameter::fields(&t["payload"], &["ref", "expectedObjectIdentity", "expectedCollectionRevision"])
            } else {
                device_parameter::fields(&t["payload"], &["direction"])
            };
            let result = self.async_adapter().invoke_async(&LiveInvocation::new(operation, args), Some(&context)).await?;
            if result.is_null() {
                return Err(LiveError::type_error("Cannot read properties of null (reading 'position')"));
            }
            let Some(position) = result["position"].as_f64().filter(|v| v.is_finite() && *v >= 0.0) else {
                return Err(LiveError::error("locator jump was not confirmed"));
            };
            if by_ref && (position - t["prior"]["target"].as_f64().unwrap_or(f64::NAN)).abs() > 1e-3 {
                return Err(LiveError::error("locator jump did not land on the cue within tolerance"));
            }
            record.borrow_mut()["applyKey"] = p["idempotencyKey"].clone();
            record.borrow_mut()["state"] = json!("applied");
            Ok(success_text(id, &json!({"transactionId":t["id"],"state":"applied","result":result,"idempotent":false})))
        }
        .await;
        Some(result.unwrap_or_else(|e| {
            self.apply_failed(id, &record, &e, "Playhead state is uncertain; perform fresh discovery before retrying.")
        }))
    }
}
