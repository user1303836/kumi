//! Clip-property transactions preserve exact clip identity and groove assignment.
use super::reads::AUDITION_DEADLINE_MS;
use super::*;
use kumi_common::{abort::Signal, js::json as js_json, time::now_ms_f64};
use sha2::{Digest, Sha256};
const FIELDS: &[&str] =
    &["muted", "colorIndex", "looping", "loopStart", "loopEnd", "launchMode", "launchQuantization", "legato", "ramMode", "velocityAmount"];
pub(super) fn scalar_same(a: Option<&Value>, b: Option<&Value>) -> bool {
    match (a, b) {
        (Some(Value::Number(a)), Some(Value::Number(b))) => a.as_f64() == b.as_f64(),
        (Some(Value::Array(_) | Value::Object(_)), _) | (_, Some(Value::Array(_) | Value::Object(_))) => false,
        _ => a == b,
    }
}
fn clip_fence(reference: &Value, clip: &Value) -> String {
    let mut v = json!({"ref":reference});
    if let Some(identity) = clip.get("objectIdentity") {
        v["objectIdentity"] = identity.clone();
    }
    v["fields"] = json!(FIELDS.iter().map(|f| &clip[*f]).collect::<Vec<_>>());
    js_json::stringify(&v)
}
impl McpHost {
    pub(super) fn clip_properties_mutation_authority(&self, snapshot: &LiveSnapshot, reference: &str) -> Result<Value, LiveError> {
        let clip = self.clip_row(snapshot, reference)?.clip;
        let fields = [
            "muted",
            "colorIndex",
            "looping",
            "loopStart",
            "loopEnd",
            "groove",
            "launchMode",
            "launchQuantization",
            "legato",
            "ramMode",
            "velocityAmount",
        ];
        let state = Value::Object(fields.iter().map(|f| ((*f).into(), clip[*f].clone())).collect());
        let mut authority = json!({});
        if let Some(identity) = clip.get("objectIdentity") {
            authority["expectedObjectIdentity"] = identity.clone();
        }
        authority["expectedAuthorityRevision"] = json!(self.clip_authority_digest(snapshot, reference)?);
        authority["expectedStateRevision"] = json!(hex::encode(Sha256::digest(canonical_mutation_identity(&state)?)));
        Ok(authority)
    }
    pub async fn dispatch_clip_properties_tool(
        &self,
        call: &ToolCall,
        signal: Option<&Signal>,
    ) -> Option<Result<Option<Value>, LiveError>> {
        let p = call.arguments.as_ref().unwrap_or(&Value::Null);
        Some(Ok(match call.name.as_str() {
            "live_clip_properties_preview" => Some(self.live_clip_properties_preview_async(&call.id, p).await),
            "live_clip_properties_apply" => self.live_clip_properties_apply_async(&call.id, p, signal).await,
            _ => return None,
        }))
    }
    pub async fn live_clip_properties_preview_async(&self, id: &Value, p: &Value) -> Value {
        let allowed = [&["clipRef", "grooveRef"][..], FIELDS].concat();
        if !has_only(p, &allowed) || !is_non_empty_string(&p["clipRef"], 256) {
            return error(id, -32602, "clipRef is required", None);
        }
        let mut proposed = json!({});
        for field in FIELDS {
            let Some(value) = p.get(*field) else {
                continue;
            };
            if matches!(*field, "muted" | "looping" | "legato" | "ramMode") {
                if !value.is_boolean() {
                    return error(id, -32602, &format!("{field} must be boolean"), None);
                }
            } else {
                let max = match *field {
                    "colorIndex" => 69.0,
                    "launchMode" => 3.0,
                    "launchQuantization" => 14.0,
                    "velocityAmount" => 1.0,
                    _ => f64::INFINITY,
                };
                let integer = matches!(*field, "colorIndex" | "launchMode" | "launchQuantization");
                if !value.as_f64().is_some_and(|n| n.is_finite() && n >= 0.0 && n <= max && (!integer || n.fract() == 0.0)) {
                    return error(id, -32602, &format!("{field} is out of bounds"), None);
                }
            }
            proposed[*field] = value.clone();
        }
        if p.get("grooveRef").is_some_and(|v| !v.is_null() && !is_non_empty_string(v, 256)) {
            return error(id, -32602, "grooveRef must be a groove reference or null", None);
        }
        if let Some(v) = p.get("grooveRef") {
            proposed["grooveRef"] = v.clone();
        }
        if proposed.as_object().unwrap().is_empty() {
            return error(id, -32602, "at least one clip field is required", None);
        }
        let result = async {
            let status = self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS)))).await?;
            if !status.connected || !status.capabilities.iter().any(|c| c.as_str() == "session.read") {
                return Err(LiveError::error("session read capability is unavailable"));
            }
            if !status.has_operation("clip.set") {
                return Err(LiveError::error("clip editing is unavailable"));
            }
            let reference = p["clipRef"].as_str().unwrap();
            let snapshot = self.views.view_for(None, &[p["clipRef"].clone()], None, &[]).await?;
            let row = self.clip_row(&snapshot, reference)?;
            let clip = &row.clip;
            if clip["isAudio"] == true && ["looping", "loopStart", "loopEnd"].iter().any(|f| proposed.get(*f).is_some()) {
                return Ok(transaction_error(id, "audio clip loop editing uses live_audio_clip_preview"));
            }
            if clip["isAudio"] == true && proposed.get("velocityAmount").is_some() {
                return Ok(transaction_error(id, "velocityAmount is only available on MIDI clips"));
            }
            if clip["isAudio"] != true && proposed.get("ramMode").is_some() {
                return Ok(transaction_error(id, "ramMode is only available on audio clips"));
            }
            if (proposed.get("launchMode").is_some() || proposed.get("launchQuantization").is_some())
                && (clip["isPlaying"] == true || clip["isTriggered"] == true)
            {
                return Ok(transaction_error(id, "launch behavior changes on a playing or triggered clip are refused"));
            }
            if FIELDS.iter().any(|f| proposed.get(*f).is_some() && clip[*f].is_null()) {
                return Ok(transaction_error(id, "one or more requested clip fields are unavailable on this exact clip"));
            }
            if !p["grooveRef"].is_null() {
                let s = serde_json::to_value(&snapshot).unwrap();
                if !s["groovePool"]["grooves"].as_array().into_iter().flatten().any(|g| g["ref"] == p["grooveRef"]) {
                    return Ok(transaction_error(id, "groove reference is unknown"));
                }
            }
            let start = proposed.get("loopStart").unwrap_or(&clip["loopStart"]);
            let end = proposed.get("loopEnd").unwrap_or(&clip["loopEnd"]);
            if start.as_f64().zip(end.as_f64()).is_some_and(|(s, e)| s > e) {
                return Ok(error(id, -32602, "loopStart must not exceed loopEnd", None));
            }
            let prior = Value::Object(
                proposed
                    .as_object()
                    .unwrap()
                    .keys()
                    .map(|f| {
                        let key = if f == "grooveRef" { "groove" } else { f.as_str() };
                        (key.into(), clip[key].clone())
                    })
                    .collect(),
            );
            let mut payload = json!({"ref":p["clipRef"]});
            payload.as_object_mut().unwrap().extend(proposed.as_object().unwrap().clone());
            payload
                .as_object_mut()
                .unwrap()
                .extend(self.clip_properties_mutation_authority(&snapshot, reference)?.as_object().unwrap().clone());
            let t = json!({"id":tempo::transaction_id("clipset"),"epoch":status.epoch,"kind":"clip-set","fence":clip_fence(&p["clipRef"],clip),"clipRef":p["clipRef"],"payload":payload,"prior":prior,"expiresAt":now_ms_f64()+TRANSACTION_TTL_MS,"state":"previewed"});
            self.retain_bounded_transaction(&self.clip_lifecycle_transactions, t.clone(), "clip properties")?;
            Ok(success_text(id, &json!({"transactionId":t["id"],"epoch":t["epoch"],"clipRef":p["clipRef"],"prior":prior,"proposed":proposed,"impact":"edits-clip","confirmation":"apply","expiresAt":t["expiresAt"]})))
        }
        .await;
        result.unwrap_or_else(|e| adapter_tool_error(id, &e, "Clip-properties preview requires fresh authoritative state."))
    }
    pub async fn live_clip_properties_apply_async(&self, id: &Value, p: &Value, signal: Option<&Signal>) -> Option<Value> {
        if !valid_transaction_params(p, "apply") {
            return Some(error(id, -32602, "transactionId, confirmation=apply, and idempotencyKey are required", None));
        }
        let Some(record) = self.clip_lifecycle_transactions.get(p["transactionId"].as_str().unwrap()) else {
            return Some(transaction_error(id, "Unknown or expired clip-properties transaction"));
        };
        let t = record.borrow().clone();
        if t["kind"] != "clip-set" || (t["state"] == "previewed" && t["expiresAt"].as_f64().unwrap_or(f64::NAN) <= now_ms_f64()) {
            return Some(transaction_error(id, "Unknown or expired clip-properties transaction"));
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
            let reference = t["clipRef"].as_str().unwrap();
            if !reconcile {
                let snapshot = self.views.view_for(Some(&context), &[t["clipRef"].clone()], None, &[]).await?;
                let row = self.clip_row(&snapshot, reference)?;
                if clip_fence(&t["clipRef"], &row.clip) != t["fence"] {
                    return Ok(transaction_error(id, "clip identity or state changed since preview; preview again"));
                }
            }
            record.borrow_mut()["state"] = json!("applying");
            record.borrow_mut()["applyKey"] = p["idempotencyKey"].clone();
            let result = adapter.invoke_async(&LiveInvocation::new("clip.set", t["payload"].clone()), Some(&context)).await?;
            if result.is_null() {
                return Err(LiveError::type_error("Cannot read properties of null (reading 'changed')"));
            }
            if result["changed"] != true {
                return Err(LiveError::error("clip change was not confirmed"));
            }
            let snapshot = self.views.view_for(Some(&context), &[t["clipRef"].clone()], None, &[]).await?;
            let verified = self.clip_row(&snapshot, reference)?.clip;
            for field in FIELDS {
                if let Some(expected) = t["payload"].get(*field) {
                    if !same_live_value(verified.get(*field), Some(expected)) {
                        return Err(LiveError::error("clip postcondition was not confirmed"));
                    }
                }
            }
            if let Some(expected) = t["payload"].get("grooveRef") {
                if !scalar_same(Some(&verified["groove"]["ref"]), Some(expected)) {
                    return Err(LiveError::error("clip groove assignment was not confirmed"));
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
        Some(
            result.unwrap_or_else(|e| {
                self.apply_failed(id, &record, &e, "Clip state is uncertain; perform fresh discovery before retrying.")
            }),
        )
    }
    pub async fn undo_clip_properties_async(&self, id: &Value, p: &Value, signal: Option<&Signal>) -> Value {
        let Some(record) = p["transactionId"].as_str().and_then(|key| self.clip_lifecycle_transactions.get(key)) else {
            return transaction_error(id, "Unknown clip-properties transaction");
        };
        let t = record.borrow().clone();
        if t["kind"] != "clip-set" {
            return transaction_error(id, "Unknown clip-properties transaction");
        }
        if t["state"] == "undone" && t["undoKey"] == p["idempotencyKey"] {
            return success_text(id, &json!({"transactionId":t["id"],"state":"undone","idempotent":true}));
        }
        let reconcile = t["state"] == "uncertain" && t["undoKey"] == p["idempotencyKey"];
        if (t["state"] != "applied" && !reconcile) || !arrangement::truthy(&t["clipRef"]) || !arrangement::truthy(&t["prior"]) {
            return transaction_error(id, "Only an applied or exact-key uncertain clip-properties edit can be undone");
        }
        let result = async {
            self.begin_undo_recovery(&record, p["idempotencyKey"].as_str().unwrap())?;
            let status = self.require_connected(Some("session.read"))?;
            if json!(status.epoch) != t["epoch"] {
                return Ok(transaction_error(id, "Live connection epoch changed; undo refused"));
            }
            let adapter = self.async_adapter();
            let context = self.transaction_context(p, signal, AUDITION_DEADLINE_MS);
            record.borrow_mut()["undoKey"] = p["idempotencyKey"].clone();
            if reconcile {
                self.replay_undo_recovery(&record, &*adapter, &context).await?;
            }
            let reference = t["clipRef"].as_str().unwrap();
            let snapshot = self.views.view_for(Some(&context), &[t["clipRef"].clone()], None, &[]).await?;
            let row = self.clip_row(&snapshot, reference)?;
            if let Some(moved) = self.undo_target_moved(
                id,
                &t,
                "clip",
                &t["clipRef"],
                row.clip.get("objectIdentity"),
                t["payload"].get("expectedObjectIdentity"),
            )? {
                return Ok(moved);
            }
            let expected = if reconcile { &t["prior"] } else { &t["payload"] };
            for (field, value) in expected.as_object().unwrap() {
                if field == "ref" || field.starts_with("expected") {
                    continue;
                }
                let same = if field == "grooveRef" {
                    scalar_same(Some(&row.clip["groove"]["ref"]), Some(value))
                } else if field == "groove" {
                    js_json::stringify(&row.clip["groove"]) == js_json::stringify(value)
                } else {
                    same_live_value(row.clip.get(field), Some(value))
                };
                if !same {
                    return Ok(transaction_error(
                        id,
                        if reconcile {
                            "Clip-properties undo replay did not restore exact prior state"
                        } else {
                            "Clip changed after apply; undo refused"
                        },
                    ));
                }
            }
            if !reconcile {
                record.borrow_mut()["state"] = json!("undoing");
                let mut prior = t["prior"].clone();
                if let Some(groove) = prior.as_object_mut().unwrap().remove("groove") {
                    prior["grooveRef"] = groove["ref"].clone();
                }
                let mut args = json!({"ref":t["clipRef"]});
                args.as_object_mut().unwrap().extend(prior.as_object().unwrap().clone());
                args.as_object_mut()
                    .unwrap()
                    .extend(self.clip_properties_mutation_authority(&snapshot, reference)?.as_object().unwrap().clone());
                let result = self.invoke_undo_recovery(&record, &*adapter, "clip.set", &args, &context).await?;
                if result.is_null() {
                    return Err(LiveError::type_error("Cannot read properties of null (reading 'changed')"));
                }
                if result["changed"] != true {
                    return Err(LiveError::error("Clip-properties restoration was not confirmed"));
                }
            }
            let snapshot = self.views.view_for(Some(&context), &[t["clipRef"].clone()], None, &[]).await?;
            let restored = self.clip_row(&snapshot, reference)?.clip;
            for (field, value) in t["prior"].as_object().unwrap() {
                if if field == "groove" {
                    js_json::stringify(&restored["groove"]) != js_json::stringify(value)
                } else {
                    !same_live_value(restored.get(field), Some(value))
                } {
                    return Err(LiveError::error("Clip exact prior state was not restored"));
                }
            }
            record.borrow_mut()["state"] = json!("undone");
            Ok(success_text(id, &json!({"transactionId":t["id"],"state":"undone","restored":t["prior"],"idempotent":false})))
        }
        .await;
        result.unwrap_or_else(|e| {
            record.borrow_mut()["state"] = json!("uncertain");
            adapter_tool_error(id, &e, "Clip-properties undo is uncertain; inspect the exact clip.")
        })
    }
}
