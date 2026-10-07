//! Scene property transactions and bounded confirmation of audible scene launch.
use super::*;
use kumi_common::{
    abort::{Signal, SignalExt},
    js::json as js_json,
};
fn sha256_canonical(value: &Value) -> Result<String, LiveError> {
    use sha2::{Digest, Sha256};
    Ok(format!("{:x}", Sha256::digest(canonical_mutation_identity(value)?.as_bytes())))
}
const FIELDS: [&str; 6] = ["colorIndex", "tempo", "tempoEnabled", "signatureNumerator", "signatureDenominator", "timeSignatureEnabled"];
fn find(snapshot: &LiveSnapshot, reference: &Value) -> Option<Value> {
    snapshot.scenes.as_ref()?.iter().find(|s| json!(s.ref_) == *reference).map(|s| serde_json::to_value(s).unwrap())
}

fn state(scene: &Value) -> Value {
    let mut value = json!({});
    for f in FIELDS {
        value[f] = scene[f].clone();
    }
    value
}
fn fence(reference: &Value, scene: &Value) -> String {
    let mut value = json!({
    "ref":reference}
    );
    if let Some(identity) = scene.get("objectIdentity") {
        value["objectIdentity"] = identity.clone();
    }
    value["state"] = json!(FIELDS.map(|f| scene[f].clone()));
    js_json::stringify(&value)
}

fn fire_state(snapshot: &LiveSnapshot, scene: Option<&Value>) -> Result<Value, LiveError> {
    let playback =
        snapshot.playback.as_ref().ok_or_else(|| LiveError::type_error("Cannot read properties of undefined (reading 'transport')"))?;
    Ok(json!({
    "isTriggered":scene.map(|s|s["isTriggered"].clone()).unwrap_or(Value::Null),
    "playing":playback.transport.playing}
    ))
}

fn fire_fence(reference: &Value, scene: &Value, fire: &Value) -> String {
    let mut value = json!({
    "ref":reference}
    );
    if let Some(identity) = scene.get("objectIdentity") {
        value["objectIdentity"] = identity.clone();
    }
    value["fireState"] = fire.clone();
    js_json::stringify(&value)
}

impl McpHost {
    pub(super) fn scene_collection_revision(&self, snapshot: &LiveSnapshot) -> Result<String, LiveError> {
        let scenes =
            snapshot.scenes.as_ref().ok_or_else(|| LiveError::type_error("Cannot read properties of undefined (reading 'map')"))?;
        let siblings: Vec<_> = scenes
            .iter()
            .map(|s| {
                let s = serde_json::to_value(s).unwrap();
                let mut value = json!({});
                for f in ["ref", "objectIdentity", "name"] {
                    if let Some(v) = s.get(f) {
                        value[f] = v.clone();
                    }
                }
                for f in FIELDS {
                    value[f] = s[f].clone();
                }
                value
            })
            .collect();
        if siblings.iter().any(|s| !is_non_empty_string(&s["ref"], 256) || !is_non_empty_string(&s["objectIdentity"], 256)) {
            return Err(LiveError::error("scene collection authority is incomplete"));
        }
        sha256_canonical(&json!(siblings))
    }

    pub(super) fn scene_state_revision(&self, scene: &Value) -> Result<String, LiveError> {
        sha256_canonical(&state(scene))
    }
    async fn scene_view(&self, context: Option<&LiveOperationContext>, fire: bool) -> Result<LiveSnapshot, LiveError> {
        let parts = if fire { vec![LiveSnapshotPart::Scenes, LiveSnapshotPart::Playback] } else { vec![LiveSnapshotPart::Scenes] };
        self.views.view(context, LiveViewScope::Indices(vec![]), Some(&parts)).await
    }

    pub async fn dispatch_scene_tool(&self, call: &ToolCall, signal: Option<&Signal>) -> Option<Result<Option<Value>, LiveError>> {
        let p = call.arguments.as_ref().unwrap_or(&Value::Null);
        Some(Ok(match call.name.as_str() {
            "live_scene_preview" => Some(self.live_scene_preview_async(&call.id, p).await),
            "live_scene_apply" => self.live_scene_apply_async(&call.id, p, signal).await,
            "live_scene_fire_preview" => Some(self.live_scene_fire_preview_async(&call.id, p).await),
            "live_scene_fire_apply" => self.live_scene_fire_apply_async(&call.id, p, signal).await,
            _ => return None,
        }))
    }

    pub async fn live_scene_preview_async(&self, id: &Value, params: &Value) -> Value {
        let allowed: Vec<_> = std::iter::once("ref").chain(FIELDS).collect();
        if !has_only(params, &allowed) || !is_non_empty_string(&params["ref"], 256) {
            return error(id, -32602, "ref is required", None);
        }
        let mut proposed = json!({});
        for f in FIELDS {
            let Some(v) = params.get(f) else { continue };
            let message = match f {
                "tempoEnabled" | "timeSignatureEnabled" if !v.is_boolean() => Some(format!("{f} must be boolean")),
                "colorIndex" if !is_integer_in_range(v, 0.0, 69.0) => Some("colorIndex is out of bounds".into()),
                "tempo" if !v.as_f64().is_some_and(|n| n.is_finite() && (20.0..=999.0).contains(&n)) => {
                    Some("tempo is out of bounds".into())
                }
                "signatureNumerator" | "signatureDenominator" if !is_integer_in_range(v, 1.0, 99.0) => {
                    Some(format!("{f} is out of bounds"))
                }
                _ => None,
            };
            if let Some(message) = message {
                return error(id, -32602, &message, None);
            }
            proposed[f] = v.clone();
        }
        if proposed.as_object().unwrap().is_empty() {
            return error(id, -32602, "at least one scene field is required", None);
        }

        let result = async {
            let status = self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(reads::AUDITION_DEADLINE_MS)))).await?;
            if !status.connected || !status.capabilities.iter().any(|c| c.as_str() == "session.read") {
                return Err(LiveError::error("session read capability is unavailable"));
            }
            if !status.has_operation("scene.set") {
                return Err(LiveError::error("scene editing is unavailable"));
            }
            let snapshot = self.scene_view(None, false).await?;
            let scene = find(&snapshot, &params["ref"])
                .filter(|s| is_non_empty_string(&s["objectIdentity"], 256))
                .ok_or_else(|| LiveError::error("scene identity is not authoritative"))?;
            let mut prior = json!({});
            for f in proposed.as_object().unwrap().keys() {
                prior[f] = scene[f].clone();
            }
            if proposed.get("tempo").is_some() && prior.get("tempoEnabled").is_none() {
                prior["tempoEnabled"] = scene["tempoEnabled"].clone();
            }
            if (proposed.get("signatureNumerator").is_some() || proposed.get("signatureDenominator").is_some())
                && prior.get("timeSignatureEnabled").is_none()
            {
                prior["timeSignatureEnabled"] = scene["timeSignatureEnabled"].clone();
            }
            let mut payload = json!({
            "ref":params["ref"]}
            );
            payload.as_object_mut().unwrap().extend(proposed.as_object().unwrap().clone());
            payload["expectedObjectIdentity"] = scene["objectIdentity"].clone();
            payload["expectedAuthorityRevision"] = json!(self.scene_collection_revision(&snapshot)?);
            payload["expectedStateRevision"] = json!(self.scene_state_revision(&scene)?);
            let t = json!({
            "id":tempo::transaction_id("sceneset"),
            "epoch":status.epoch,
            "kind":"scene-set",
            "fence":fence(&params["ref"],
            &scene),
            "payload":payload,
            "prior":prior,
            "expiresAt":kumi_common::time::now_ms_f64()+TRANSACTION_TTL_MS,
            "state":"previewed"}
            );
            self.retain_bounded_transaction(&self.clip_lifecycle_transactions, t.clone(), "scene edit")?;
            Ok(success_text(
                id,
                &json!({
                "transactionId":t["id"],
                "epoch":t["epoch"],
                "ref":params["ref"],
                "prior":prior,
                "proposed":proposed,
                "impact":"edits-scene",
                "confirmation":"apply",
                "expiresAt":t["expiresAt"]}
                ),
            ))
        }
        .await;
        result.unwrap_or_else(|e| adapter_tool_error(id, &e, "Scene preview requires fresh authoritative state."))
    }
    pub async fn live_scene_fire_preview_async(&self, id: &Value, params: &Value) -> Value {
        if !has_only(params, &["ref"]) || !is_non_empty_string(&params["ref"], 256) {
            return error(id, -32602, "ref is required", None);
        }
        let result = async {
            let status = self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(reads::AUDITION_DEADLINE_MS)))).await?;
            if !status.connected || !status.capabilities.iter().any(|c| c.as_str() == "session.read") {
                return Err(LiveError::error("session read capability is unavailable"));
            }
            if !status.has_operation("scene.fire-selected") {
                return Err(LiveError::error("scene fire-as-selected is unavailable"));
            }
            let snapshot = self.scene_view(None, true).await?;
            let scene = find(&snapshot, &params["ref"])
                .filter(|s| is_non_empty_string(&s["objectIdentity"], 256))
                .ok_or_else(|| LiveError::error("scene identity is not authoritative"))?;
            if scene["isEmpty"] == true {
                return Ok(adapter_tool_error(
                    id,
                    &LiveError::error("that scene has no clips to play, so launching it would only stop what's playing"),
                    "Nothing was launched. Put clips in the scene first, or launch another.",
                ));
            }
            let fire = fire_state(&snapshot, Some(&scene))?;
            let payload = json!({
            "ref":params["ref"],
            "expectedObjectIdentity":scene["objectIdentity"],
            "expectedAuthorityRevision":self.scene_collection_revision(&snapshot)?,
            "expectedStateRevision":sha256_canonical(&fire)?}
            );
            let t = json!({
            "id":tempo::transaction_id("scenefire"),
            "epoch":status.epoch,
            "kind":"scene-fire",
            "fence":fire_fence(&params["ref"],
            &scene,
            &fire),
            "payload":payload,
            "prior":{
            "isTriggered":scene["isTriggered"]}
            ,
            "expiresAt":kumi_common::time::now_ms_f64()+TRANSACTION_TTL_MS,
            "state":"previewed"}
            );
            self.retain_bounded_transaction(&self.clip_lifecycle_transactions, t.clone(), "scene fire")?;
            Ok(success_text(
                id,
                &json!({
                "transactionId":t["id"],
                "epoch":t["epoch"],
                "ref":params["ref"],
                "fireState":fire,
                "impact":"fires-scene-audible-direct-no-undo",
                "confirmation":"apply",
                "expiresAt":t["expiresAt"]}
                ),
            ))
        }
        .await;
        result.unwrap_or_else(|e| adapter_tool_error(id, &e, "Scene-fire preview requires fresh authoritative state."))
    }

    pub async fn live_scene_apply_async(&self, id: &Value, params: &Value, signal: Option<&Signal>) -> Option<Value> {
        self.apply_scene(id, params, signal, false).await
    }

    pub async fn live_scene_fire_apply_async(&self, id: &Value, params: &Value, signal: Option<&Signal>) -> Option<Value> {
        self.apply_scene(id, params, signal, true).await
    }

    async fn apply_scene(&self, id: &Value, params: &Value, signal: Option<&Signal>, fire: bool) -> Option<Value> {
        if !valid_transaction_params(params, "apply") {
            return Some(error(id, -32602, "transactionId, confirmation=apply, and idempotencyKey are required", None));
        }
        let unknown = if fire { "Unknown or expired scene-fire transaction" } else { "Unknown or expired scene transaction" };
        let Some(record) = self.clip_lifecycle_transactions.get(params["transactionId"].as_str().unwrap()) else {
            return Some(transaction_error(id, unknown));
        };
        let t = record.borrow().clone();
        if t["kind"] != if fire { "scene-fire" } else { "scene-set" }
            || (t["state"] == "previewed" && t["expiresAt"].as_f64().is_some_and(|n| n <= kumi_common::time::now_ms_f64()))
        {
            return Some(transaction_error(id, unknown));
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
            let payload = &t["payload"];
            if !reconciliation {
                let snapshot = self.scene_view(Some(&context), fire).await?;
                let scene = find(&snapshot, &payload["ref"]);
                let current = if fire {
                    let fire = fire_state(&snapshot, scene.as_ref())?;
                    scene.as_ref().map(|s| fire_fence(&payload["ref"], s, &fire))
                } else {
                    scene.as_ref().map(|s| fence(&payload["ref"], s))
                };
                if current.as_deref() != t["fence"].as_str() {
                    return Ok(transaction_error(
                        id,
                        if fire {
                            "scene identity or fire state changed since preview; preview again"
                        } else {
                            "scene identity or state changed since preview; preview again"
                        },
                    ));
                }
            }
            {
                let mut r = record.borrow_mut();
                r["state"] = json!("applying");
                r["applyKey"] = params["idempotencyKey"].clone();
            }
            let result = adapter
                .invoke_async(&LiveInvocation::new(if fire { "scene.fire-selected" } else { "scene.set" }, payload.clone()), Some(&context))
                .await?;
            if tuning::field(&result, if fire { "fired" } else { "changed" })? != Some(&json!(true)) {
                return Err(LiveError::error(if fire { "scene fire was not confirmed" } else { "scene change was not confirmed" }));
            }
            if fire {
                let mut confirmed = false;
                for attempt in 0..8 {
                    if attempt > 0 {
                        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                    }
                    let verified = self.scene_view(Some(&context), true).await?;
                    let scene = find(&verified, &payload["ref"]);
                    let playback = verified
                        .playback
                        .as_ref()
                        .ok_or_else(|| LiveError::type_error("Cannot read properties of undefined (reading 'firedTargets')"))?;
                    let playback = serde_json::to_value(playback).unwrap();
                    confirmed = scene.is_some_and(|s| s["isTriggered"] == true)
                        || playback["transport"]["playing"] == true
                        || ["firedTargets", "playingTargets"]
                            .iter()
                            .flat_map(|k| playback[*k].as_array().into_iter().flatten())
                            .any(|target| target["sceneRef"] == payload["ref"]);
                    if confirmed {
                        break;
                    }
                }
                if !confirmed {
                    return Err(LiveError::error("scene fire postcondition was not confirmed"));
                }
            } else {
                let verified = find(&self.scene_view(Some(&context), false).await?, &payload["ref"])
                    .ok_or_else(|| LiveError::error("edited scene disappeared after apply"))?;
                for f in FIELDS {
                    if let Some(proposed) = payload.get(f) {
                        if !same_live_value(verified.get(f), Some(proposed)) {
                            return Err(LiveError::error("scene postcondition was not confirmed"));
                        }
                    }
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
            if !fire {
                if let Some(revision) = result.get("revision") {
                    reply["revision"] = revision.clone();
                }
            }
            reply["idempotent"] = json!(false);
            Ok(success_text(id, &reply))
        }
        .await;
        Some(result.unwrap_or_else(|e| {
            self.apply_failed(
                id,
                &record,
                &e,
                if fire {
                    "Scene-fire state is uncertain; inspect Live before retrying."
                } else {
                    "Scene state is uncertain; perform fresh discovery before retrying."
                },
            )
        }))
    }

    pub async fn undo_scene_async(&self, id: &Value, params: &Value, signal: Option<&Signal>) -> Value {
        let Some(record) = params["transactionId"]
            .as_str()
            .and_then(|id| self.clip_lifecycle_transactions.get(id))
            .filter(|r| r.borrow()["kind"] == "scene-set")
        else {
            return transaction_error(id, "Unknown or expired scene transaction");
        };
        let t = record.borrow().clone();
        if t["state"] == "undone" && t["undoKey"] == params["idempotencyKey"] {
            return success_text(
                id,
                &json!({
                "transactionId":t["id"],
                "state":"undone",
                "idempotent":true}
                ),
            );
        }
        let reconciliation = t["state"] == "uncertain" && t["undoKey"] == params["idempotencyKey"];
        if (t["state"] != "applied" && !reconciliation) || !arrangement::truthy(&t["prior"]) {
            return transaction_error(id, "Only an applied or exact-key uncertain scene transaction can be undone");
        }
        let result = async {
            self.begin_undo_recovery(&record, params["idempotencyKey"].as_str().unwrap())?;
            let status = self.require_connected(Some("session.read"))?;
            if json!(status.epoch) != t["epoch"] {
                return Ok(transaction_error(id, "Live connection epoch changed; undo refused"));
            }
            let adapter = self.async_adapter();
            let context = self.transaction_context(params, signal, reads::AUDITION_DEADLINE_MS);
            record.borrow_mut()["undoKey"] = params["idempotencyKey"].clone();
            if reconciliation {
                self.replay_undo_recovery(&record, adapter.as_ref(), &context).await?;
            }
            let snapshot = self.scene_view(Some(&context), false).await?;
            let payload = &t["payload"];
            let scene = find(&snapshot, &payload["ref"])
                .filter(|s| is_non_empty_string(&s["objectIdentity"], 256))
                .ok_or_else(|| LiveError::error("scene identity is unavailable"))?;
            if scene["objectIdentity"] != payload["expectedObjectIdentity"] {
                return Ok(transaction_error(id, "scene identity changed after apply; undo refused"));
            }
            if !reconciliation {
                for (f, value) in payload.as_object().unwrap() {
                    if ["ref", "expectedObjectIdentity", "expectedAuthorityRevision", "expectedStateRevision"].contains(&f.as_str()) {
                        continue;
                    }
                    if !same_live_value(scene.get(f), Some(value)) {
                        return Ok(transaction_error(id, "scene changed after apply; undo refused"));
                    }
                }
            }
            let prior = &t["prior"];
            let Some(restore) = scene_restore_fields(prior.as_object().unwrap()) else {
                return Ok(transaction_error(id, "the scene's prior state can't be written back exactly; undo refused"));
            };
            record.borrow_mut()["state"] = json!("undoing");
            let mut args = json!({
            "ref":payload["ref"]}
            );
            args.as_object_mut().unwrap().extend(restore);
            args["expectedObjectIdentity"] = scene["objectIdentity"].clone();
            args["expectedAuthorityRevision"] = json!(self.scene_collection_revision(&snapshot)?);
            args["expectedStateRevision"] = json!(self.scene_state_revision(&scene)?);
            let result = self.invoke_undo_recovery(&record, adapter.as_ref(), "scene.set", &args, &context).await?;
            if tuning::field(&result, "changed")? != Some(&json!(true)) {
                return Err(LiveError::error("scene restoration was not confirmed"));
            }
            let restored = find(&self.scene_view(Some(&context), false).await?, &payload["ref"])
                .ok_or_else(|| LiveError::error("scene disappeared after undo"))?;
            for (f, v) in prior.as_object().unwrap() {
                if !scene_field_restored(&restored, f, v, prior) {
                    return Err(LiveError::error("scene exact prior state was not restored"));
                }
            }
            record.borrow_mut()["state"] = json!("undone");
            Ok(success_text(
                id,
                &json!({
                "transactionId":t["id"],
                "state":"undone",
                "idempotent":false}
                ),
            ))
        }
        .await;
        result.unwrap_or_else(|e| {
            record.borrow_mut()["state"] = json!("uncertain");
            adapter_tool_error(id, &e, "Scene undo is uncertain; perform fresh discovery.")
        })
    }
}
