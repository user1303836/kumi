//! Coupled Follow Action fields, targeted Session readback, and exact-key undo recovery.
use super::reads::AUDITION_DEADLINE_MS;
use super::*;
use crate::follow_actions::{validate_follow_actions, FOLLOW_ACTION_FIELDS};
use kumi_common::{abort::Signal, js::json as js_json, time::now_ms_f64};
use sha2::{Digest, Sha256};
struct FollowRead {
    snapshot: LiveSnapshot,
    clip: Value,
    arrangement: bool,
}
fn state(clip: &Value) -> Value {
    Value::Object(FOLLOW_ACTION_FIELDS.iter().map(|f| (f.clone(), clip[f].clone())).collect())
}
fn follow_fence(reference: &Value, clip: &Value) -> String {
    let mut out = json!({"ref":reference});
    if let Some(identity) = clip.get("objectIdentity") {
        out["objectIdentity"] = identity.clone();
    }
    out["fields"] = json!(FOLLOW_ACTION_FIELDS.iter().map(|f| &clip[f]).collect::<Vec<_>>());
    js_json::stringify(&out)
}
fn number(value: &Value) -> Result<f64, LiveError> {
    Ok(match value {
        Value::Null => 0.0,
        Value::Bool(v) => {
            if *v {
                1.0
            } else {
                0.0
            }
        }
        _ => kumi_common::js::number::parse(&js_string(value)?).unwrap_or(f64::NAN),
    })
}
impl McpHost {
    async fn follow_actions_view_async(&self, context: Option<&LiveOperationContext>, reference: &str) -> Result<FollowRead, LiveError> {
        let snapshot = self.views.view_for(context, &[json!(reference)], None, &[]).await?;
        let mut row = self.clip_row(&snapshot, reference)?;
        if !row.arrangement {
            let parent = reference.replacen(":clip:", ":clip_slot:", 1);
            let fields = [vec!["ref", "objectIdentity"], FOLLOW_ACTION_FIELDS.iter().map(String::as_str).collect()].concat();
            let targeted =
                self.discover_one_async(context, LiveDiscoveryKind::SessionClip, reference, Some(&fields), Some(&parent)).await?;
            let Some(targeted) = targeted.filter(|t| t.get("objectIdentity") == row.clip.get("objectIdentity")) else {
                return Err(LiveError::error("clip identity changed during Follow Action read"));
            };
            row.clip.as_object_mut().unwrap().extend(targeted.as_object().unwrap().clone());
        }
        Ok(FollowRead { snapshot, clip: row.clip, arrangement: row.arrangement })
    }
    fn follow_actions_mutation_authority(&self, read: &FollowRead, reference: &str) -> Result<Value, LiveError> {
        let mut authority = json!({});
        if let Some(identity) = read.clip.get("objectIdentity") {
            authority["expectedObjectIdentity"] = identity.clone();
        }
        authority["expectedAuthorityRevision"] = json!(self.clip_authority_digest(&read.snapshot, reference)?);
        authority["expectedStateRevision"] = json!(hex::encode(Sha256::digest(canonical_mutation_identity(&state(&read.clip))?)));
        Ok(authority)
    }
    pub async fn dispatch_follow_tool(&self, call: &ToolCall, signal: Option<&Signal>) -> Option<Result<Option<Value>, LiveError>> {
        let p = call.arguments.as_ref().unwrap_or(&Value::Null);
        Some(Ok(match call.name.as_str() {
            "live_follow_actions_preview" => Some(self.live_follow_actions_preview_async(&call.id, p).await),
            "live_follow_actions_apply" => self.live_follow_actions_apply_async(&call.id, p, signal).await,
            _ => return None,
        }))
    }
    pub async fn live_follow_actions_preview_async(&self, id: &Value, p: &Value) -> Value {
        let allowed = [vec!["clipRef"], FOLLOW_ACTION_FIELDS.iter().map(String::as_str).collect()].concat();
        if !has_only(p, &allowed) || !is_non_empty_string(&p["clipRef"], 256) || p.as_object().unwrap().len() < 2 {
            return error(id, -32602, "clipRef and at least one Follow Action field are required", None);
        }
        let result = async {
            let status = self.fresh_status(Some(&LiveOperationContext::with_deadline(now_ms_f64() + AUDITION_DEADLINE_MS))).await?;
            if !status.connected || !status.has_operation("clip.follow-actions.set") {
                return Err(LiveError::error("Willington Follow Action editing is unavailable"));
            }
            let reference = p["clipRef"].as_str().unwrap();
            let read = self.follow_actions_view_async(None, reference).await?;
            if read.arrangement {
                return Err(LiveError::error("Follow Actions require a Session clip"));
            }
            let snapshot = serde_json::to_value(&read.snapshot).unwrap();
            if snapshot["playback"]["transport"]["playing"] != false || read.clip["isRecording"] != false {
                return Err(LiveError::error("Follow Action edits require stopped transport and a non-recording clip"));
            }
            let prior =
                Value::Object(FOLLOW_ACTION_FIELDS.iter().filter_map(|f| read.clip.get(f).map(|v| (f.clone(), v.clone()))).collect());
            validate_follow_actions(prior.as_object().unwrap()).map_err(|e| LiveError::error(e.to_string()))?;
            let mut proposed = prior.clone();
            for field in FOLLOW_ACTION_FIELDS.iter() {
                if let Some(value) = p.get(field) {
                    proposed[field] = value.clone();
                }
            }
            if p.get("followActionChanceA").is_some() && p.get("followActionChanceB").is_none() {
                proposed["followActionChanceB"] = json!(100.0-number(&p["followActionChanceA"])?);
            }
            if p.get("followActionChanceB").is_some() && p.get("followActionChanceA").is_none() {
                proposed["followActionChanceA"] = json!(100.0-number(&p["followActionChanceB"])?);
            }
            validate_follow_actions(proposed.as_object().unwrap()).map_err(|e| LiveError::error(e.to_string()))?;
            let mut payload = json!({"ref":p["clipRef"]});
            payload.as_object_mut().unwrap().extend(proposed.as_object().unwrap().clone());
            payload.as_object_mut().unwrap().extend(self.follow_actions_mutation_authority(&read, reference)?.as_object().unwrap().clone());
            let t = json!({"id":tempo::transaction_id("follow"),"epoch":status.epoch,"kind":"follow-actions","fence":follow_fence(&p["clipRef"],&read.clip),"clipRef":p["clipRef"],"payload":payload,"prior":prior,"expiresAt":now_ms_f64()+TRANSACTION_TTL_MS,"state":"previewed"});
            self.retain_bounded_transaction(&self.clip_lifecycle_transactions, t.clone(), "Follow Actions")?;
            Ok(success_text(id, &json!({"transactionId":t["id"],"epoch":t["epoch"],"clipRef":p["clipRef"],"prior":prior,"proposed":proposed,"impact":"edits-clip-follow-actions","confirmation":"apply","expiresAt":t["expiresAt"]})))
        }
        .await;
        result.unwrap_or_else(|e| {
            let remediation = if e.message() == "Follow Action edits require stopped transport and a non-recording clip" {
                "Stop Live transport and clip recording, then retry the edit."
            } else {
                "Follow Action preview requires fresh authoritative state."
            };
            adapter_tool_error(id, &e, remediation)
        })
    }
    pub async fn live_follow_actions_apply_async(&self, id: &Value, p: &Value, signal: Option<&Signal>) -> Option<Value> {
        if !valid_transaction_params(p, "apply") {
            return Some(error(id, -32602, "transactionId, confirmation=apply, and idempotencyKey are required", None));
        }
        let Some(record) = self.clip_lifecycle_transactions.get(p["transactionId"].as_str().unwrap()) else {
            return Some(transaction_error(id, "Unknown or expired Follow Action transaction"));
        };
        let t = record.borrow().clone();
        if t["kind"] != "follow-actions" || (t["state"] == "previewed" && t["expiresAt"].as_f64().unwrap_or(f64::NAN) <= now_ms_f64()) {
            return Some(transaction_error(id, "Unknown or expired Follow Action transaction"));
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
                self.fresh_status(Some(&LiveOperationContext::with_deadline(now_ms_f64() + AUDITION_DEADLINE_MS))).await?;
            }
            let status = self.require_connected(Some("session.read"))?;
            if json!(status.epoch) != t["epoch"] {
                return Ok(transaction_error(id, "Live connection epoch changed; preview again"));
            }
            let adapter = self.async_adapter();
            let mut context = self.transaction_context(p, signal, AUDITION_DEADLINE_MS);
            context.deadline_ms = Some(now_ms_f64() + AUDITION_DEADLINE_MS);
            let reference = t["clipRef"].as_str().unwrap();
            if !reconcile {
                let read = self.follow_actions_view_async(Some(&context), reference).await?;
                if follow_fence(&t["clipRef"], &read.clip) != t["fence"] {
                    return Ok(transaction_error(id, "clip identity or state changed since preview; preview again"));
                }
            }
            record.borrow_mut()["state"] = json!("applying");
            record.borrow_mut()["applyKey"] = p["idempotencyKey"].clone();
            let result =
                adapter.invoke_async(&LiveInvocation::new("clip.follow-actions.set", t["payload"].clone()), Some(&context)).await?;
            if result.is_null() {
                return Err(LiveError::type_error("Cannot read properties of null (reading 'changed')"));
            }
            if result["changed"] != true {
                return Err(LiveError::error("clip change was not confirmed"));
            }
            let verified = self.follow_actions_view_async(Some(&context), reference).await?.clip;
            for field in FOLLOW_ACTION_FIELDS.iter() {
                if let Some(expected) = t["payload"].get(field) {
                    if !same_follow_action_value(field, verified.get(field), Some(expected)) {
                        return Err(LiveError::error("clip postcondition was not confirmed"));
                    }
                }
            }
            record.borrow_mut()["applyKey"] = p["idempotencyKey"].clone();
            record.borrow_mut()["state"] = json!("applied");
            let mut body = json!({"transactionId":t["id"],"state":"applied"});
            if let Some(revision) = result.get("revision") {
                body["revision"] = revision.clone();
            }
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
                    "Follow Action state is uncertain; reconcile this exact transaction and key."
                } else {
                    "Follow Action apply failed before dispatch; retry the preview before expiry."
                },
            )
        }))
    }
    pub async fn undo_follow_actions_async(&self, id: &Value, p: &Value, signal: Option<&Signal>) -> Value {
        let Some(record) = p["transactionId"].as_str().and_then(|key| self.clip_lifecycle_transactions.get(key)) else {
            return transaction_error(id, "Unknown Follow Action transaction");
        };
        let t = record.borrow().clone();
        if t["kind"] != "follow-actions" {
            return transaction_error(id, "Unknown Follow Action transaction");
        }
        if t["state"] == "undone" && t["undoKey"] == p["idempotencyKey"] {
            return success_text(id, &json!({"transactionId":t["id"],"state":"undone","idempotent":true}));
        }
        let reconcile = t["state"] == "uncertain" && t["undoKey"] == p["idempotencyKey"];
        if (t["state"] != "applied" && !reconcile) || !arrangement::truthy(&t["clipRef"]) || !arrangement::truthy(&t["prior"]) {
            return transaction_error(id, "Only an applied or exact-key uncertain Follow Action edit can be undone");
        }
        let result = async {
            let (_, steps) = self.begin_undo_recovery(&record, p["idempotencyKey"].as_str().unwrap())?;
            record.borrow_mut()["undoKey"] = p["idempotencyKey"].clone();
            let status = self.require_connected(Some("session.read"))?;
            if json!(status.epoch) != t["epoch"] {
                return Ok(transaction_error(id, "Live connection epoch changed; undo refused"));
            }
            let adapter = self.async_adapter();
            let mut context = self.transaction_context(p, signal, AUDITION_DEADLINE_MS);
            context.deadline_ms = Some(now_ms_f64() + AUDITION_DEADLINE_MS);
            record.borrow_mut()["undoKey"] = p["idempotencyKey"].clone();
            let replayed = reconcile && !steps.is_empty();
            if replayed {
                self.replay_undo_recovery(&record, &*adapter, &context).await?;
            }
            let reference = t["clipRef"].as_str().unwrap();
            let read = self.follow_actions_view_async(Some(&context), reference).await?;
            if let Some(moved) = self.undo_target_moved(
                id,
                &t,
                "clip",
                &t["clipRef"],
                read.clip.get("objectIdentity"),
                t["payload"].get("expectedObjectIdentity"),
            )? {
                return Ok(moved);
            }
            let expected = if replayed { &t["prior"] } else { &t["payload"] };
            for field in FOLLOW_ACTION_FIELDS.iter() {
                if !same_follow_action_value(field, read.clip.get(field), expected.get(field)) {
                    return Ok(transaction_error(
                        id,
                        if reconcile {
                            "Follow Action undo replay did not restore prior state"
                        } else {
                            "Clip changed after apply; undo refused"
                        },
                    ));
                }
            }
            if !replayed {
                record.borrow_mut()["state"] = json!("undoing");
                let mut args = json!({"ref":t["clipRef"]});
                args.as_object_mut().unwrap().extend(t["prior"].as_object().unwrap().clone());
                args.as_object_mut()
                    .unwrap()
                    .extend(self.follow_actions_mutation_authority(&read, reference)?.as_object().unwrap().clone());
                let result = self.invoke_undo_recovery(&record, &*adapter, "clip.follow-actions.set", &args, &context).await?;
                if result.is_null() {
                    return Err(LiveError::type_error("Cannot read properties of null (reading 'changed')"));
                }
                if result["changed"] != true {
                    return Err(LiveError::error("Follow Action restoration was not confirmed"));
                }
            }
            let restored = self.follow_actions_view_async(Some(&context), reference).await?.clip;
            for field in FOLLOW_ACTION_FIELDS.iter() {
                if !same_follow_action_value(field, restored.get(field), t["prior"].get(field)) {
                    return Err(LiveError::error("Follow Action prior state was not restored"));
                }
            }
            record.borrow_mut()["state"] = json!("undone");
            Ok(success_text(id, &json!({"transactionId":t["id"],"state":"undone","restored":t["prior"],"idempotent":false})))
        }
        .await;
        result.unwrap_or_else(|e| {
            record.borrow_mut()["state"] = json!("uncertain");
            adapter_tool_error(id, &e, "Follow Action undo is uncertain; reconcile this exact transaction and key.")
        })
    }
}
