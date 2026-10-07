//! Song settings bind their Set identity and preserve Live's numeric readback precision.
use super::{
    reads::AUDITION_DEADLINE_MS,
    track_view::{confirmed, digest},
    *,
};
use kumi_common::{abort::Signal, js::json as js_json, time::now_ms_f64};
const FIELDS: &[&str] = &[
    "signatureNumerator",
    "signatureDenominator",
    "swingAmount",
    "clipTriggerQuantization",
    "midiRecordingQuantization",
    "selectOnLaunch",
];
fn settings(song: &Value) -> Result<Value, LiveError> {
    if song.is_null() {
        return Err(LiveError::type_error("Cannot read properties of null (reading 'selectOnLaunch')"));
    }
    Ok(
        json!({"selectOnLaunch":song["selectOnLaunch"],"signatureNumerator":song["signatureNumerator"],"signatureDenominator":song["signatureDenominator"],"swingAmount":song["swingAmount"],"clipTriggerQuantization":song["clipTriggerQuantization"]["value"],"midiRecordingQuantization":song["midiRecordingQuantization"]["value"]}),
    )
}
fn fence(settings: &Value) -> String {
    js_json::stringify(&json!({"state":FIELDS.iter().map(|f|&settings[*f]).collect::<Vec<_>>()}))
}
fn same_set(set: &Value, payload: &Value) -> bool {
    set.get("ref") == payload.get("setRef") && set.get("objectIdentity") == payload.get("expectedObjectIdentity")
}
impl McpHost {
    pub async fn dispatch_song_settings_tool(&self, call: &ToolCall, signal: Option<&Signal>) -> Option<Result<Option<Value>, LiveError>> {
        let p = call.arguments.as_ref().unwrap_or(&Value::Null);
        Some(Ok(match call.name.as_str() {
            "live_song_settings_preview" => Some(self.live_song_settings_preview_async(&call.id, p).await),
            "live_song_settings_apply" => self.live_song_settings_apply_async(&call.id, p, signal).await,
            _ => return None,
        }))
    }
    async fn settings_set(&self, context: Option<&LiveOperationContext>) -> Result<Value, LiveError> {
        let snapshot = self.views.view(context, LiveViewScope::Indices(vec![]), Some(&[LiveSnapshotPart::Set])).await?;
        Ok(serde_json::to_value(snapshot).unwrap()["set"].clone())
    }
    pub async fn live_song_settings_preview_async(&self, id: &Value, p: &Value) -> Value {
        if !has_only(p, FIELDS) {
            return error(id, -32602, "song settings arguments are invalid", None);
        }
        let mut proposed = json!({});
        for field in FIELDS {
            if let Some(value) = p.get(*field) {
                let valid = match *field {
                    "signatureNumerator" | "signatureDenominator" => is_integer_in_range(value, 1.0, 99.0),
                    "swingAmount" => value.as_f64().is_some_and(|n| n.is_finite() && (0.0..=1.0).contains(&n)),
                    "clipTriggerQuantization" => is_integer_in_range(value, 0.0, 13.0),
                    "selectOnLaunch" => value.is_boolean(),
                    _ => is_integer_in_range(value, 0.0, 8.0),
                };
                if !valid {
                    return error(
                        id,
                        -32602,
                        &format!("{field} {}", if *field == "selectOnLaunch" { "must be boolean" } else { "is out of bounds" }),
                        None,
                    );
                }
                proposed[*field] = value.clone();
            }
        }
        if proposed.as_object().unwrap().is_empty() {
            return error(id, -32602, "at least one song settings field is required", None);
        }
        let result = async {
            let status = self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS)))).await?;
            if !status.connected || !status.capabilities.iter().any(|c| c.as_str() == "session.read") {
                return Err(LiveError::error("session read capability is unavailable"));
            }
            if !status.has_operation("song.set") {
                return Err(LiveError::error("song settings editing is unavailable"));
            }
            let adapter = self.async_adapter();
            let set = self.settings_set(None).await?;
            let song = adapter.invoke_async(&LiveInvocation::new("song.read", json!({"setRef":set["ref"]})), None).await?;
            let settings = settings(&song)?;
            if proposed.as_object().unwrap().keys().any(|f| settings[f].is_null()) {
                return Ok(transaction_error(id, "one or more requested song settings are unavailable on this shape"));
            }
            let prior = Value::Object(proposed.as_object().unwrap().keys().map(|f| (f.clone(), settings[f].clone())).collect());
            if !is_non_empty_string(&set["objectIdentity"], 256) {
                return Err(LiveError::error("song settings require exact Set identity"));
            }
            let mut payload = proposed.clone();
            payload["setRef"] = set["ref"].clone();
            payload["expectedObjectIdentity"] = set["objectIdentity"].clone();
            payload["expectedStateRevision"] = json!(digest(&settings)?);
            let t = json!({"id":tempo::transaction_id("songset"),"epoch":status.epoch,"kind":"song-set","fence":fence(&settings),"payload":payload,"prior":prior,"expiresAt":now_ms_f64()+TRANSACTION_TTL_MS,"state":"previewed"});
            self.retain_bounded_transaction(&self.clip_lifecycle_transactions, t.clone(), "song settings edit")?;
            Ok(success_text(id, &json!({"transactionId":t["id"],"epoch":t["epoch"],"prior":prior,"proposed":proposed,"impact":"edits-song-settings-playback-feel","confirmation":"apply","expiresAt":t["expiresAt"]})))
        }
        .await;
        result.unwrap_or_else(|e| adapter_tool_error(id, &e, "Song-settings preview requires fresh authoritative state."))
    }
    pub async fn live_song_settings_apply_async(&self, id: &Value, p: &Value, signal: Option<&Signal>) -> Option<Value> {
        if !valid_transaction_params(p, "apply") {
            return Some(error(id, -32602, "transactionId, confirmation=apply, and idempotencyKey are required", None));
        }
        let Some(record) = self.clip_lifecycle_transactions.get(p["transactionId"].as_str().unwrap()) else {
            return Some(transaction_error(id, "Unknown or expired song-settings transaction"));
        };
        let t = record.borrow().clone();
        if t["kind"] != "song-set" || (t["state"] == "previewed" && t["expiresAt"].as_f64().unwrap_or(f64::NAN) <= now_ms_f64()) {
            return Some(transaction_error(id, "Unknown or expired song-settings transaction"));
        }
        if t["state"] == "applied" && t["applyKey"] == p["idempotencyKey"] {
            return Some(success_text(id, &json!({"transactionId":t["id"],"state":"applied","idempotent":true})));
        }
        let reconcile = t["state"] == "uncertain" && t.get("undoKey").is_none() && t["applyKey"] == p["idempotencyKey"];
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
            let set = self.settings_set(Some(&context)).await?;
            if !same_set(&set, payload) {
                return Err(LiveError::error("song settings Set identity changed since preview"));
            }
            let read = LiveInvocation::new("song.read", json!({"setRef":set["ref"]}));
            if !reconcile {
                let song = adapter.invoke_async(&read, Some(&context)).await?;
                if json!(fence(&settings(&song)?)) != t["fence"] {
                    return Ok(transaction_error(id, "song settings changed since preview; preview again"));
                }
            }
            record.borrow_mut()["state"] = json!("applying");
            record.borrow_mut()["applyKey"] = p["idempotencyKey"].clone();
            let result = adapter.invoke_async(&LiveInvocation::new("song.set", payload.clone()), Some(&context)).await?;
            confirmed(&result, "changed", "song settings change was not confirmed")?;
            let after = self.settings_set(Some(&context)).await?;
            if !same_set(&after, payload) {
                return Err(LiveError::error("song settings Set identity changed after apply"));
            }
            let verified = settings(&adapter.invoke_async(&read, Some(&context)).await?)?;
            for f in FIELDS {
                if payload.get(*f).is_some() && !same_live_value(verified.get(*f), payload.get(*f)) {
                    return Err(LiveError::error("song settings postcondition was not confirmed"));
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
            self.apply_failed(id, &record, &e, "Song settings state is uncertain; perform fresh discovery before retrying.")
        }))
    }
    pub async fn undo_song_settings_async(&self, id: &Value, p: &Value, signal: Option<&Signal>) -> Value {
        let Some(record) = self.clip_lifecycle_transactions.get(p["transactionId"].as_str().unwrap_or("")) else {
            return transaction_error(id, "Unknown or expired song-settings transaction");
        };
        let t = record.borrow().clone();
        if t["kind"] != "song-set" {
            return transaction_error(id, "Unknown or expired song-settings transaction");
        }
        if t["state"] == "undone" && t["undoKey"] == p["idempotencyKey"] {
            return success_text(id, &json!({"transactionId":t["id"],"state":"undone","idempotent":true}));
        }
        let reconcile = t["state"] == "uncertain" && t["undoKey"] == p["idempotencyKey"];
        if (t["state"] != "applied" && !reconcile) || !arrangement::truthy(&t["prior"]) {
            return transaction_error(id, "Only an applied or exact-key uncertain song-settings transaction can be undone");
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
            let set = self.settings_set(Some(&context)).await?;
            if !same_set(&set, payload) {
                return Err(LiveError::error("song settings Set identity changed after apply; undo refused"));
            }
            let read = LiveInvocation::new("song.read", json!({"setRef":set["ref"]}));
            let raw_song = adapter.invoke_async(&read, Some(&context)).await?;
            let song = settings(&raw_song)?;
            if !reconcile {
                for (f, v) in payload.as_object().into_iter().flatten() {
                    if ["setRef", "expectedObjectIdentity", "expectedStateRevision"].contains(&f.as_str()) {
                        continue;
                    }
                    if !same_live_value(song.get(f), Some(v)) {
                        return Ok(transaction_error(id, "song settings changed after apply; undo refused"));
                    }
                }
            }
            record.borrow_mut()["state"] = json!("undoing");
            let mut args = t["prior"].clone();
            args["setRef"] = payload["setRef"].clone();
            args["expectedObjectIdentity"] = payload["expectedObjectIdentity"].clone();
            args["expectedStateRevision"] = json!(digest(&settings(&raw_song)?)?);
            let result = self.invoke_undo_recovery(&record, adapter.as_ref(), "song.set", &args, &context).await?;
            confirmed(&result, "changed", "song settings restoration was not confirmed")?;
            let after = self.settings_set(Some(&context)).await?;
            if !same_set(&after, payload) {
                return Err(LiveError::error("song settings Set identity changed after undo"));
            }
            let restored = settings(&adapter.invoke_async(&read, Some(&context)).await?)?;
            for (f, v) in t["prior"].as_object().into_iter().flatten() {
                if !same_live_value(restored.get(f), Some(v)) {
                    return Err(LiveError::error("song settings exact prior state was not restored"));
                }
            }
            record.borrow_mut()["state"] = json!("undone");
            Ok(success_text(id, &json!({"transactionId":t["id"],"state":"undone","idempotent":false})))
        }
        .await;
        result.unwrap_or_else(|e| {
            record.borrow_mut()["state"] = json!("uncertain");
            adapter_tool_error(id, &e, "Song-settings undo is uncertain; perform fresh discovery.")
        })
    }
}
