//! Guarded clip content actions and transient playback operations.
use super::*;
use kumi_common::{
    abort::{Signal, SignalExt},
    js::json as js_json,
};
use sha2::{Digest, Sha256};
const ACTIONS: [&str; 6] = ["crop", "duplicate-loop", "duplicate-region", "scrub-start", "scrub-stop", "move-playing-position"];
fn content(action: &Value) -> bool {
    ["crop", "duplicate-loop", "duplicate-region"].iter().any(|a| action == a)
}
fn state(clip: &Value) -> Value {
    json!({"isPlaying":clip["isPlaying"],"length":clip["length"],"loopStart":clip["loopStart"],"loopEnd":clip["loopEnd"]})
}
fn fence(reference: &Value, clip: &Value, state: &Value, content: &Value) -> String {
    let mut value = json!({
    "ref":reference}
    );
    if let Some(identity) = clip.get("objectIdentity") {
        value["objectIdentity"] = identity.clone();
    }
    value["state"] = state.clone();
    value["contentFingerprint"] = content.clone();
    js_json::stringify(&value)
}

impl McpHost {
    pub async fn dispatch_clip_action_tool(&self, call: &ToolCall, signal: Option<&Signal>) -> Option<Result<Option<Value>, LiveError>> {
        let p = call.arguments.as_ref().unwrap_or(&Value::Null);
        Some(Ok(match call.name.as_str() {
            "live_clip_action_preview" => Some(self.live_clip_action_preview_async(&call.id, p).await),
            "live_clip_action_apply" => self.live_clip_action_apply_async(&call.id, p, signal).await,
            _ => return None,
        }))
    }

    pub async fn live_clip_action_preview_async(&self, id: &Value, params: &Value) -> Value {
        if !has_only(params, &["clipRef", "action", "regionStart", "regionEnd", "destination", "offset"])
            || !is_non_empty_string(&params["clipRef"], 256)
            || !ACTIONS.iter().any(|a| params["action"] == *a)
        {
            return error(id, -32602, "clipRef and a valid action are required", None);
        }
        let result = async {
            let status = self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(reads::AUDITION_DEADLINE_MS)))).await?;
            if !status.connected || !status.capabilities.iter().any(|c| c.as_str() == "session.read") {
                return Err(LiveError::error("session read capability is unavailable"));
            }
            if !status.has_operation("clip.action") {
                return Err(LiveError::error("clip actions are unavailable"));
            }
            let reference = params["clipRef"].as_str().unwrap();
            let snapshot = self.views.view_for(None, &[params["clipRef"].clone()], None, &[]).await?;
            let row = self.clip_row(&snapshot, reference)?;
            let mut payload = json!({
            "ref":params["clipRef"],
            "action":params["action"]}
            );
            if params["action"] == "duplicate-region" {
                let start = params["regionStart"].as_f64().filter(|n| n.is_finite() && *n >= 0.0);
                let end = params["regionEnd"].as_f64().filter(|n| n.is_finite());
                let destination = params["destination"].as_f64().filter(|n| n.is_finite() && *n >= 0.0);
                if start.zip(end).is_none_or(|(s, e)| e <= s) || destination.is_none() {
                    return Ok(error(id, -32602, "duplicate-region requires regionStart, regionEnd, and destination", None));
                }
                for f in ["regionStart", "regionEnd", "destination"] {
                    payload[f] = params[f].clone();
                }
            }
            if params["action"] == "scrub-start" || params["action"] == "move-playing-position" {
                if !params["offset"].as_f64().is_some_and(f64::is_finite) {
                    return Ok(error(id, -32602, "offset is required", None));
                }
                payload["offset"] = params["offset"].clone();
            }
            let authority = self.clip_properties_mutation_authority(&snapshot, reference)?;
            for f in ["expectedObjectIdentity", "expectedAuthorityRevision"] {
                if let Some(v) = authority.get(f) {
                    payload[f] = v.clone();
                }
            }
            let state = state(&row.clip);
            payload["expectedStateRevision"] = json!(hex::encode(Sha256::digest(canonical_mutation_identity(&state)?)));
            if content(&params["action"]) {
                payload["expectedContentFingerprint"] = json!(arrangement::capture_object_fingerprint(&row.clip)?);
            }
            let prior = json!({
            "length":row.clip["length"],
            "playingPosition":row.clip["playingPosition"],
            "loopStart":row.clip["loopStart"],
            "loopEnd":row.clip["loopEnd"]}
            );
            let t = json!({
            "id":tempo::transaction_id("clipaction"),
            "epoch":status.epoch,
            "kind":"clip-action",
            "fence":fence(&params["clipRef"],
            &row.clip,
            &state,
            &payload["expectedContentFingerprint"]),
            "clipRef":params["clipRef"],
            "payload":payload,
            "prior":prior,
            "expiresAt":kumi_common::time::now_ms_f64()+TRANSACTION_TTL_MS,
            "state":"previewed"}
            );
            self.retain_bounded_transaction(&self.clip_lifecycle_transactions, t.clone(), "clip action")?;
            Ok(success_text(
                id,
                &json!({
                "transactionId":t["id"],
                "epoch":t["epoch"],
                "action":params["action"],
                "clipRef":params["clipRef"],
                "prior":prior,
                "impact":if content(&params["action"]){
                "edits-clip-content-not-undoable"}
                else{
                "transient-clip-playback-state"}
                ,
                "confirmation":"apply",
                "expiresAt":t["expiresAt"]}
                ),
            ))
        }
        .await;
        result.unwrap_or_else(|e| adapter_tool_error(id, &e, "Clip-action preview requires fresh authoritative state."))
    }

    pub async fn live_clip_action_apply_async(&self, id: &Value, params: &Value, signal: Option<&Signal>) -> Option<Value> {
        if !valid_transaction_params(params, "apply") {
            return Some(error(id, -32602, "transactionId, confirmation=apply, and idempotencyKey are required", None));
        }
        let Some(record) = self.clip_lifecycle_transactions.get(params["transactionId"].as_str().unwrap()) else {
            return Some(transaction_error(id, "Unknown or expired clip-action transaction"));
        };
        let t = record.borrow().clone();
        if t["kind"] != "clip-action"
            || (t["state"] == "previewed" && t["expiresAt"].as_f64().is_some_and(|n| n <= kumi_common::time::now_ms_f64()))
        {
            return Some(transaction_error(id, "Unknown or expired clip-action transaction"));
        }
        if t["state"] == "applied" && t["applyKey"] == params["idempotencyKey"] {
            return Some(success_text(
                id,
                &json!({
                "transactionId":t["id"],
                "state":"applied",
                "idempotent":true}
                ),
            ));
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
            let status = self.require_connected(Some("session.read"))?;
            if json!(status.epoch) != t["epoch"] {
                return Ok(transaction_error(id, "Live connection epoch changed; preview again"));
            }
            let adapter = self.async_adapter();
            let context = self.transaction_context(params, signal, reads::AUDITION_DEADLINE_MS);
            let reference = t["clipRef"].as_str().unwrap();
            let payload = &t["payload"];
            if !reconciliation {
                let snapshot = self.views.view_for(Some(&context), &[t["clipRef"].clone()], None, &[]).await?;
                let row = self.clip_row(&snapshot, reference)?;
                let content =
                    if content(&payload["action"]) { json!(arrangement::capture_object_fingerprint(&row.clip)?) } else { Value::Null };
                if fence(&t["clipRef"], &row.clip, &state(&row.clip), &content) != t["fence"] {
                    return Ok(transaction_error(id, "clip identity, state, or content changed since preview; preview again"));
                }
            }
            {
                let mut r = record.borrow_mut();
                r["state"] = json!("applying");
                r["applyKey"] = params["idempotencyKey"].clone();
            }
            let result = adapter.invoke_async(&LiveInvocation::new("clip.action", payload.clone()), Some(&context)).await?;
            if tuning::field(&result, "changed")? != Some(&json!(true)) {
                return Err(LiveError::error("clip action was not confirmed"));
            }
            let verified = self.clip_row(&self.views.view_for(Some(&context), &[t["clipRef"].clone()], None, &[]).await?, reference)?.clip;
            let action = &payload["action"];
            let length = verified["length"].as_f64().unwrap_or(f64::NAN);
            let prior = &t["prior"];
            if action == "crop" {
                let Some((start, end)) = prior["loopStart"].as_f64().zip(prior["loopEnd"].as_f64()).filter(|(s, e)| e > s) else {
                    return Err(LiveError::error("clip crop loop state is unavailable"));
                };
                if (length - (end - start)).abs() > 1e-6 {
                    return Err(LiveError::error("clip crop postcondition was not confirmed"));
                }
            }
            if action == "duplicate-loop" {
                let Some((start, end)) = prior["loopStart"].as_f64().zip(prior["loopEnd"].as_f64()) else {
                    return Err(LiveError::error("clip loop state is unavailable"));
                };
                if (length - (prior["length"].as_f64().unwrap_or(f64::NAN) + (end - start))).abs() > 1e-6 {
                    return Err(LiveError::error("clip loop duplication postcondition was not confirmed"));
                }
            }
            if action == "duplicate-region" {
                let span = payload["regionEnd"].as_f64().unwrap() - payload["regionStart"].as_f64().unwrap();
                let prior_length = prior["length"].as_f64().unwrap_or(f64::NAN);
                let destination = payload["destination"].as_f64().unwrap_or(prior_length);
                if (length - prior_length.max(destination + span)).abs() > 1e-6 {
                    return Err(LiveError::error("clip region duplication postcondition was not confirmed"));
                }
            }
            {
                let mut r = record.borrow_mut();
                r["applyKey"] = params["idempotencyKey"].clone();
                r["state"] = json!("applied");
            }
            let mut reply = json!({
            "transactionId":t["id"],
            "state":"applied"}
            );
            if let Some(revision) = result.get("revision") {
                reply["revision"] = revision.clone();
            }
            reply["idempotent"] = json!(false);
            Ok(success_text(id, &reply))
        }
        .await;
        Some(result.unwrap_or_else(|e| apply_failed(id, &record, &e, "Clip state is uncertain; perform fresh discovery before retrying.")))
    }
}
