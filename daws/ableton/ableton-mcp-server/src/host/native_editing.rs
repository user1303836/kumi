//! Native editing transactions keep opaque prior state off model-facing receipts.
use super::reads::AUDITION_DEADLINE_MS;
use super::*;
use kumi_common::{abort::Signal, time::now_ms_f64};
use sha2::{Digest, Sha256};
const SELECTOR: &[&str] = &["kind", "ref", "targetRef", "noteId", "dimension", "trackRefs"];
fn property<'a>(value: Option<&'a Value>, key: &str) -> Result<Option<&'a Value>, LiveError> {
    value.map(|v| v.get(key)).ok_or_else(|| LiveError::error("Missing native editing result"))
}
fn put(out: &mut Value, key: &str, value: Option<&Value>) {
    if let Some(value) = value {
        out[key] = value.clone();
    }
}
fn validate_readback(read: &Value) -> Result<Value, LiveError> {
    let state = read["state"]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 1048576)
        .ok_or_else(|| LiveError::error("Incomplete native editing state"))?;
    if read["stateRevision"].as_str() != Some(hex::encode(Sha256::digest(state.as_bytes())).as_str()) {
        return Err(LiveError::error("Invalid native editing revision"));
    }
    let summary = read["summary"].as_str().filter(|s| s.len() <= 8192).ok_or_else(|| LiveError::error("Missing native editing summary"))?;
    let summary: Value = serde_json::from_str(summary).map_err(|_| LiveError::error("Invalid native editing summary"))?;
    if !summary.is_object() {
        return Err(LiveError::error("Invalid native editing summary"));
    }
    Ok(summary)
}
impl McpHost {
    pub async fn dispatch_native_editing_tool(&self, call: &ToolCall, signal: Option<&Signal>) -> Option<Result<Option<Value>, LiveError>> {
        let p = call.arguments.as_ref().unwrap_or(&Value::Null);
        Some(match call.name.as_str() {
            "live_native_editing_preview" => self.live_native_editing_preview_async(&call.id, p).await.map(Some),
            "live_native_editing_apply" => Ok(self.live_native_editing_apply_async(&call.id, p, signal).await),
            _ => return None,
        })
    }
    pub async fn live_native_editing_preview_async(&self, id: &Value, p: &Value) -> Result<Value, LiveError> {
        if !has_only(p, &["kind", "ref", "targetRef", "noteId", "dimension", "trackRefs", "edit"])
            || !p["edit"].is_object()
            || !["group-tracks", "scene-follow", "global-follow", "note-expression", "arrangement-automation"]
                .contains(&p["kind"].as_str().unwrap_or(""))
        {
            return Ok(error(id, -32602, "an explicit native editing kind, target and edit are required", None));
        }
        let result = async {
            let status = self.fresh_status(Some(&LiveOperationContext::with_deadline(now_ms_f64() + AUDITION_DEADLINE_MS))).await?;
            if !status.connected || !status.has_operation("willington.editing.read") || !status.has_operation("willington.editing.set")
                || !status.extra.get("nativeEditingKinds").and_then(Value::as_array).is_some_and(|k| k.contains(&p["kind"])) {
                return Err(LiveError::error("Native editing kind unavailable"));
            }
            let read = self.async_adapter().invoke_async(&LiveInvocation::new("willington.editing.read", p.clone()), None).await?;
            let summary = validate_readback(&read)?;
            if !read["next"].as_str().is_some_and(|s| !s.is_empty() && s.len() <= 1048576) {
                return Err(LiveError::error("Missing native editing proposal"));
            }
            let proposed: Value = serde_json::from_str(read["next"].as_str().unwrap()).map_err(|_| LiveError::error("Invalid native editing proposal"))?;
            let mut payload = device_parameter::fields(p, SELECTOR);
            payload["next"] = read["next"].clone();
            payload["expectedStateRevision"] = read["stateRevision"].clone();
            let t = json!({"id":tempo::transaction_id("nativeedit"),"epoch":status.epoch,"kind":"native-editing",
                "fence":read["stateRevision"],"payload":payload,"prior":read["state"],"state":"previewed","expiresAt":now_ms_f64()+TRANSACTION_TTL_MS});
            self.retain_bounded_transaction(&self.clip_lifecycle_transactions, t.clone(), "native editing")?;
            Ok(success_text(id, &json!({"transactionId":t["id"],"epoch":t["epoch"],"prior":summary,"proposed":proposed,
                "undoable":p["kind"] != "group-tracks","confirmation":"apply","expiresAt":t["expiresAt"]})))
        }.await;
        Ok(result.unwrap_or_else(|e| adapter_tool_error(id, &e, "Native preview requires complete current readback and stopped playback.")))
    }
    pub async fn live_native_editing_apply_async(&self, id: &Value, p: &Value, signal: Option<&Signal>) -> Option<Value> {
        if !valid_transaction_params(p, "apply") {
            return Some(error(id, -32602, "transactionId, confirmation=apply and idempotencyKey are required", None));
        }
        let Some(record) = self.clip_lifecycle_transactions.get(p["transactionId"].as_str().unwrap()) else {
            return Some(transaction_error(id, "Unknown or expired Native editing preview"));
        };
        let t = record.borrow().clone();
        if t["kind"] != "native-editing" || (t["state"] == "previewed" && t["expiresAt"].as_f64().unwrap_or(f64::NAN) <= now_ms_f64()) {
            return Some(transaction_error(id, "Unknown or expired Native editing preview"));
        }
        if t["state"] == "applied" && t["applyKey"] == p["idempotencyKey"] {
            return Some(success_text(id, &json!({"transactionId":t["id"],"state":"applied","idempotent":true})));
        }
        let reconcile = t["state"] == "uncertain" && t["applyKey"] == p["idempotencyKey"];
        if t["state"] != "previewed" && !reconcile {
            return Some(transaction_error(id, "Native editing transaction is no longer applicable"));
        }
        if signal.is_some_and(Signal::is_cancelled) {
            return None;
        }
        let result = async {
            let status = self.fresh_status(Some(&LiveOperationContext::with_deadline(now_ms_f64() + AUDITION_DEADLINE_MS))).await?;
            if !status.connected
                || json!(status.epoch) != t["epoch"]
                || !status.has_operation("willington.editing.set")
                || !status.extra.get("nativeEditingKinds").and_then(Value::as_array).is_some_and(|k| k.contains(&t["payload"]["kind"]))
            {
                return Err(LiveError::error("Native editing epoch or capability changed"));
            }
            let context = LiveOperationContext {
                signal: signal.cloned(),
                deadline_ms: Some(now_ms_f64() + AUDITION_DEADLINE_MS),
                transaction_id: t["id"].as_str().map(str::to_owned),
                idempotency_key: p["idempotencyKey"].as_str().map(str::to_owned),
            };
            record.borrow_mut()["state"] = json!("applying");
            record.borrow_mut()["applyKey"] = p["idempotencyKey"].clone();
            let result = self
                .async_adapter()
                .invoke_async(&LiveInvocation::new("willington.editing.set", t["payload"].clone()), Some(&context))
                .await?;
            if property(Some(&result), "changed")? != Some(&Value::Bool(true)) {
                return Err(LiveError::error("Native editing write was not confirmed"));
            }
            let summary = validate_readback(&result)?;
            record.borrow_mut()["created"] = result.clone();
            record.borrow_mut()["state"] = json!("applied");
            let mut body = json!({"transactionId":t["id"],"state":"applied"});
            if t["payload"]["kind"] == "group-tracks" {
                body["created"] = json!([{"ref":summary["createdRef"],"kind":"track"}]);
            }
            body["after"] = summary;
            body["undoable"] = json!(t["payload"]["kind"] != "group-tracks");
            body["idempotent"] = json!(false);
            Ok(success_text(id, &body))
        }
        .await;
        Some(result.unwrap_or_else(|e| {
            if record.borrow()["state"] == "applying" {
                record.borrow_mut()["state"] = json!("uncertain");
            }
            adapter_tool_error(
                id,
                &e,
                if record.borrow()["state"] == "uncertain" {
                    "Native editing state is uncertain; reconcile this exact transaction and key."
                } else {
                    "Native editing apply failed before dispatch; retry the preview before expiry."
                },
            )
        }))
    }
    pub async fn undo_native_editing_async(&self, id: &Value, p: &Value, signal: Option<&Signal>) -> Value {
        let Some(record) = p["transactionId"].as_str().and_then(|key| self.clip_lifecycle_transactions.get(key)) else {
            return transaction_error(id, "Unknown Native editing transaction");
        };
        let t = record.borrow().clone();
        if t["kind"] != "native-editing" || !arrangement::truthy(&t["prior"]) {
            return transaction_error(id, "Unknown Native editing transaction");
        }
        if t["payload"]["kind"] == "group-tracks" {
            return transaction_error(id, "Group creation is not undoable through Kumi history; use Live's own undo if appropriate");
        }
        if t["state"] == "undone" && t["undoKey"] == p["idempotencyKey"] {
            return success_text(id, &json!({"transactionId":t["id"],"state":"undone","idempotent":true}));
        }
        let reconcile = t["state"] == "uncertain" && t["undoKey"] == p["idempotencyKey"];
        if t["state"] != "applied" && !reconcile {
            return transaction_error(id, "Only applied or exact-key uncertain Native editing undo is allowed");
        }
        let result = async {
            let (_, steps) = self.begin_undo_recovery(&record, p["idempotencyKey"].as_str().unwrap())?;
            record.borrow_mut()["undoKey"] = p["idempotencyKey"].clone();
            let status = self.fresh_status(Some(&LiveOperationContext::with_deadline(now_ms_f64() + AUDITION_DEADLINE_MS))).await?;
            if !status.connected
                || json!(status.epoch) != t["epoch"]
                || !status.has_operation("willington.editing.set")
                || !status.extra.get("nativeEditingKinds").and_then(Value::as_array).is_some_and(|k| k.contains(&t["payload"]["kind"]))
            {
                return Err(LiveError::error("Native editing epoch or capability changed"));
            }
            let adapter = self.async_adapter();
            let context = LiveOperationContext {
                signal: signal.cloned(),
                deadline_ms: Some(now_ms_f64() + AUDITION_DEADLINE_MS),
                transaction_id: t["id"].as_str().map(str::to_owned),
                idempotency_key: p["idempotencyKey"].as_str().map(str::to_owned),
            };
            let selector = device_parameter::fields(&t["payload"], SELECTOR);
            if reconcile && !steps.is_empty() {
                self.replay_undo_recovery(&record, &*adapter, &context).await?;
                let restored = adapter.invoke_async(&LiveInvocation::new("willington.editing.read", selector), Some(&context)).await?;
                if !super::clip_properties::scalar_same(property(Some(&restored), "stateRevision")?, t.get("fence")) {
                    return Err(LiveError::error("Native editing prior state was not restored exactly"));
                }
            } else {
                let prior = &t["prior"];
                let next = Value::String(json!({"restore":prior}).to_string());
                record.borrow_mut()["state"] = json!("undoing");
                let mut args = selector;
                args["next"] = next;
                put(&mut args, "expectedStateRevision", t["created"].get("stateRevision"));
                let restored = self.invoke_undo_recovery(&record, &*adapter, "willington.editing.set", &args, &context).await?;
                if property(Some(&restored), "changed")? != Some(&Value::Bool(true))
                    || !super::clip_properties::scalar_same(property(Some(&restored), "stateRevision")?, t.get("fence"))
                {
                    return Err(LiveError::error("Native editing prior state was not restored exactly"));
                }
            }
            record.borrow_mut()["state"] = json!("undone");
            Ok(success_text(id, &json!({"transactionId":t["id"],"state":"undone","idempotent":false})))
        }
        .await;
        result.unwrap_or_else(|e| {
            record.borrow_mut()["state"] = json!("uncertain");
            adapter_tool_error(id, &e, "Native editing undo is uncertain; reconcile this exact transaction and key.")
        })
    }
}
