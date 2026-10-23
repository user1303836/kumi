//! Guarded Session audition, exact playback ownership, and emergency stop.
use super::*;
use base64::Engine;
use kumi_common::{
    abort::{Signal, SignalExt},
    js::json as js_json,
};
use rand::RngCore;
use retention::TransactionRecord;
use sha2::{Digest, Sha256};
pub(super) const TRACK_CONTENT_PARTS: &[LiveSnapshotPart] =
    &[LiveSnapshotPart::Set, LiveSnapshotPart::Tracks, LiveSnapshotPart::Scenes, LiveSnapshotPart::Playback];
struct AuditionState {
    set: Value,

    scene: Value,

    tracks: Vec<Value>,

    playback: Value,

    playback_revision: String,

    eligible: Vec<String>,
}
pub(super) fn random_confirmation() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}
pub(super) fn active_targets(playback: &Value) -> Vec<Value> {
    playback["firedTargets"]
        .as_array()
        .into_iter()
        .flatten()
        .chain(playback["playingTargets"].as_array().into_iter().flatten())
        .cloned()
        .collect()
}
pub(super) fn target_key(target: &Value) -> String {
    format!(
        "{}|{}|{}",
        target["trackRef"].as_str().unwrap_or("undefined"),
        target["clipSlotRef"].as_str().unwrap_or("undefined"),
        target["sceneRef"].as_str().unwrap_or("undefined")
    )
}
fn unsafe_monitoring(tracks: &[Value]) -> bool {
    tracks.iter().any(|t| {
        if matches!(t["kind"].as_str(), Some("regular" | "audio" | "midi")) {
            t["armed"] != false || !matches!(t["monitoringState"].as_str(), Some("off" | "auto"))
        } else {
            t["armed"] == true || t["monitoringState"] == "in"
        }
    })
}
fn json_rows(value: &Value) -> Vec<Value> {
    value.as_array().cloned().unwrap_or_default()
}
fn fields(row: &Value, names: &[&str]) -> Value {
    let mut result = json!({});
    for name in names {
        if let Some(value) = row.get(*name) {
            result[*name] = value.clone();
        }
    }
    result
}
fn authoritative_field(row: &Value, name: &str) -> Result<Value, LiveError> {
    row.get(name).cloned().ok_or_else(|| LiveError::error("mutation authority contains an unsupported value"))
}
fn valid_confirmation(params: &Value) -> bool {
    has_only(params, &["transactionId", "confirmation", "idempotencyKey"])
        && is_non_empty_string(&params["transactionId"], 128)
        && is_non_empty_string(&params["confirmation"], 128)
        && is_idempotency_key(&params["idempotencyKey"])
}
impl McpHost {
    pub async fn dispatch_audition_tool(
        self: &Rc<Self>,

        call: &ToolCall,

        signal: Option<&Signal>,
    ) -> Option<Result<Option<Value>, LiveError>> {
        let p = call.arguments.as_ref().unwrap_or(&Value::Null);
        Some(match call.name.as_str() {
            "live_session_audition_preview" => Ok(Some(self.live_session_audition_preview_async(&call.id, p).await)),

            "live_session_audition_apply" => Ok(self.live_session_audition_apply_async(&call.id, p, signal).await),

            "live_session_audition_stop" => Ok(self.live_session_audition_stop_async(&call.id, p, signal).await),

            "live_session_emergency_stop" => self.live_session_emergency_stop_async(&call.id, p, signal).await,

            _ => return None,
        })
    }

    fn audition_authority_revision(&self, snapshot: &LiveSnapshot, scene_ref: &str, eligible: &[String]) -> Result<String, LiveError> {
        let snapshot = serde_json::to_value(snapshot).unwrap();
        let scenes = json_rows(&snapshot["scenes"]);
        let scene = scenes.iter().find(|s| s["ref"] == scene_ref).ok_or_else(|| LiveError::error("audition scene is not authoritative"))?;
        let tracks = json_rows(&snapshot["tracks"]);
        let mut keys = eligible.to_vec();
        keys.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
        let mut targets = vec![];
        for key in keys {
            let keys: Vec<_> = key.split('|').collect();
            let track = tracks.iter().find(|t| t["ref"] == keys[0]);
            let slots = track.map(|t| json_rows(&t["clipSlots"])).unwrap_or_default();
            let slot = slots.iter().find(|s| s["ref"] == keys.get(1).copied().unwrap_or(""));
            let clips = track.map(|t| json_rows(&t["clips"])).unwrap_or_default();
            let clip = slot.and_then(|s| clips.iter().find(|c| c["ref"] == s["clipRef"]));
            let (Some(track), Some(slot), Some(clip)) = (track, slot, clip) else {
                return Err(LiveError::error("audition target hierarchy is incomplete"));
            };
            if keys.get(2) != Some(&scene_ref) {
                return Err(LiveError::error("audition target hierarchy is incomplete"));
            }
            targets.push(json!({"trackRef":authoritative_field(track,
"ref")?,
"trackIdentity":authoritative_field(track,
"objectIdentity")?,
"slotRef":authoritative_field(slot,
"ref")?,
"slotIdentity":authoritative_field(slot,
"objectIdentity")?,
"sceneRef":authoritative_field(scene,
"ref")?,
"sceneIdentity":authoritative_field(scene,
"objectIdentity")?,
"clipRef":authoritative_field(clip,
"ref")?,
"clipIdentity":authoritative_field(clip,
"objectIdentity")?}));
        }
        let set = &snapshot["set"];
        let value = json!({"set":{"ref":authoritative_field(set,
"ref")?,
"objectIdentity":authoritative_field(set,
"objectIdentity")?},
"scene":{"ref":authoritative_field(scene,
"ref")?,
"objectIdentity":authoritative_field(scene,
"objectIdentity")?,
"index":scene["index"]},
"targets":targets});
        Ok(hex::encode(Sha256::digest(canonical_mutation_identity(&value)?.as_bytes())))
    }
    fn audition_snapshot(&self, snapshot: &LiveSnapshot, scene_ref: &str) -> Result<AuditionState, LiveError> {
        let snapshot = serde_json::to_value(snapshot).unwrap();
        let scenes = json_rows(&snapshot["scenes"]);
        let scene = scenes
            .iter()
            .find(|s| s["ref"] == scene_ref)
            .filter(|s| s["index"].as_f64().is_some_and(|i| i.fract() == 0.0))
            .cloned()
            .ok_or_else(|| LiveError::error("audition scene is not authoritative"))?;
        let tracks = json_rows(&snapshot["tracks"]);
        let playback = &snapshot["playback"];
        if !playback["firedTargets"].is_array() || !playback["playingTargets"].is_array() {
            return Err(LiveError::error("authoritative Session playback is unavailable"));
        }
        let mut eligible = vec![];
        for track in &tracks {
            for slot in json_rows(&track["clipSlots"]) {
                if slot["sceneIndex"].as_f64() == scene["index"].as_f64()
                    && slot["ref"].is_string()
                    && track["ref"].is_string()
                    && slot["clipRef"].is_string()
                {
                    eligible.push(format!("{}|{}|{scene_ref}", track["ref"].as_str().unwrap(), slot["ref"].as_str().unwrap()));
                }
            }
        }
        if eligible.iter().any(|key| key.split('|').count() != 3) {
            return Err(LiveError::error("audition references are not encodable as target keys"));
        }
        let track_states: Vec<_> =
            tracks.iter().map(|t| fields(t, &["ref", "armed", "monitoringState", "playingSlotIndex", "firedSlotIndex"])).collect();
        let revision = js_json::stringify(&json!({"playback":playback,
"tracks":track_states,
"scenes":scenes}));
        Ok(AuditionState { set: snapshot["set"].clone(), scene, tracks, playback: playback.clone(), playback_revision: revision, eligible })
    }
    fn validate_audition_safety(
        &self,

        status: &LiveStatus,

        state: &AuditionState,

        safety: &Value,

        set_name: &Value,
    ) -> Result<(), LiveError> {
        if !has_only(safety, &["safe", "provenance", "observedAt", "scope"])
            || safety["safe"] != true
            || !is_non_empty_string(&safety["provenance"], 512)
            || matches!(safety["provenance"].as_str(), Some("unknown" | "simulator"))
        {
            return Err(LiveError::error("explicit authoritative output-safety evidence is required"));
        }
        if state.set["name"] != *set_name {
            return Err(LiveError::error("disposable Set identity does not match authoritative state"));
        }
        let transport = &state.playback["transport"];
        if transport["playing"] != false || transport["arrangementRecord"] != false || transport["sessionRecord"] != false {
            return Err(LiveError::error("audition requires stopped, non-recording authoritative playback state"));
        }
        let quantization = &transport["launchQuantization"]["normalized"];
        if !arrangement::truthy(quantization) || matches!(quantization.as_str(), Some("none" | "unknown" | "free")) {
            return Err(LiveError::error("launch quantization is unsafe or unknown"));
        }
        if unsafe_monitoring(&state.tracks) {
            return Err(LiveError::error("armed, input-monitored, or unknown-monitoring target prevents audition"));
        }
        if !active_targets(&state.playback).is_empty() {
            return Err(LiveError::error("existing Session playback prevents audition"));
        }
        if !["session.audition-launch", "session.audition-stop", "session.emergency-stop", "session.playback"]
            .iter()
            .all(|op| status.has_operation(op))
        {
            return Err(LiveError::error("required guarded audition, emergency stop, and playback inspection operations are unavailable"));
        }
        Ok(())
    }
    pub async fn live_session_audition_preview_async(&self, id: &Value, params: &Value) -> Value {
        if !has_only(params, &["sceneRef", "setName", "outputSafety"])
            || !is_non_empty_string(&params["sceneRef"], 256)
            || !is_non_empty_string(&params["setName"], 256)
            || !params["outputSafety"].is_object()
        {
            return error(id, -32602, "sceneRef, setName, and outputSafety are required", None);
        }
        let result = async {
            let status = self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(reads::AUDITION_DEADLINE_MS)))).await?;
            if !status.connected || !status.capabilities.iter().any(|c| c.as_str() == "session.read") {
                return Err(LiveError::error("session read capability is unavailable"));
            }
            let snapshot = self.views.whole_set(None, Some(TRACK_CONTENT_PARTS)).await?;
            let state = self.audition_snapshot(&snapshot, params["sceneRef"].as_str().unwrap())?;
            self.validate_audition_safety(&status, &state, &params["outputSafety"], &params["setName"])?;
            if state.eligible.is_empty() {
                return Err(LiveError::error("audition scene has no authoritative playable clip slots"));
            }
            if !is_non_empty_string(&state.set["objectIdentity"], 256) {
                return Err(LiveError::error("disposable Set identity is unavailable"));
            }
            let t = json!({
            "id":tempo::transaction_id("audition"),
            "epoch":status.epoch,
            "sceneRef":params["sceneRef"],
            "sceneRevision":js_json::stringify(&state.scene),
            "playbackRevision":state.playback_revision,
            "eligibleTargetKeys":state.eligible,
            "authorityRevision":self.audition_authority_revision(&snapshot,
            params["sceneRef"].as_str().unwrap(),
            &state.eligible)?,
            "setName":params["setName"],
            "setIdentity":state.set["objectIdentity"],
            "outputSafety":output_safety_of(&params["outputSafety"]),
            "confirmation":random_confirmation(),
            "stopConfirmation":random_confirmation(),
            "expiresAt":kumi_common::time::now_ms_f64()+TRANSACTION_TTL_MS,
            "state":"previewed"}
            );
            self.retain_bounded_transaction(&self.audition_transactions, t.clone(), "audition")?;
            Ok(success_text(
                id,
                &json!({
                "transactionId":t["id"],
                "epoch":t["epoch"],
                "scene":state.scene,
                "sceneRevision":t["sceneRevision"],
                "playbackRevision":t["playbackRevision"],
                "eligibleTargets":t["eligibleTargetKeys"],
                "disposableSet":{
                "expected":t["setName"],
                "observed":state.set["name"],
                "matches":true}
                ,
                "baseline":{
                "stopped":true,
                "arrangementRecord":false,
                "sessionRecord":false}
                ,
                "launchQuantization":state.playback["transport"]["launchQuantization"],
                "outputSafety":t["outputSafety"],
                "audibleImpact":"potentially-audible-session-scene-launch",
                "confirmation":t["confirmation"],
                "stopConfirmation":t["stopConfirmation"],
                "expiresAt":t["expiresAt"]}
                ),
            ))
        }
        .await;

        result.unwrap_or_else(|e| {
            adapter_tool_error(
                id,
                &e,
                "Audition preview refused; obtain fresh authoritative discovery and explicit output-safety evidence.",
            )
        })
    }
    fn audition_apply_error(&self, id: &Value, cause: &LiveError, record: &TransactionRecord) -> Value {
        adapter_tool_error(
            id,
            cause,
            if record.borrow()["state"] == "previewed" {
                "Audition apply failed before dispatch; the preview remains available until expiry."
            } else {
                "Audition state is uncertain; do not retry. Perform fresh playback discovery before stopping or recovering."
            },
        )
    }
    pub async fn live_session_audition_apply_async(self: &Rc<Self>, id: &Value, params: &Value, signal: Option<&Signal>) -> Option<Value> {
        if !valid_confirmation(params) {
            return Some(error(id, -32602, "transactionId, exact confirmation, and idempotencyKey are required", None));
        }
        let Some(record) = self.audition_transactions.get(params["transactionId"].as_str().unwrap()) else {
            return Some(transaction_error(id, "Unknown or expired audition transaction"));
        };
        let t = record.borrow().clone();
        if t["state"] == "previewed" && t["expiresAt"].as_f64().is_some_and(|n| n <= kumi_common::time::now_ms_f64()) {
            return Some(transaction_error(id, "Unknown or expired audition transaction"));
        }
        if t["state"] == "applied" && t["applyKey"] == params["idempotencyKey"] && t["confirmation"] == params["confirmation"] {
            return Some(success_text(
                id,
                &json!({"transactionId":t["id"],
"state":"applied",
"launched":t["launched"],
"stopConfirmation":t["stopConfirmation"],
"idempotent":true}),
            ));
        }
        if t["state"] == "applying" {
            let promise = self.record_operation(&record);
            if t["applyKey"] != params["idempotencyKey"] || t["confirmation"] != params["confirmation"] || promise.is_none() {
                return Some(transaction_error(id, "Audition apply is already in progress with a different request"));
            }
            return Some(match promise.unwrap().await {
                Ok(mut outcome) => {
                    outcome["idempotent"] = json!(true);
                    success_text(id, &outcome)
                }
                Err(e) => self.audition_apply_error(id, &e, &record),
            });
        }
        let reconciliation =
            t["state"] == "uncertain" && t["applyKey"] == params["idempotencyKey"] && t["confirmation"] == params["confirmation"];
        if (t["state"] != "previewed" && !reconciliation) || t["confirmation"] != params["confirmation"] {
            return Some(transaction_error(id, "Exact audition confirmation is required"));
        }
        if signal.is_some_and(Signal::aborted) {
            return None;
        }
        {
            let mut r = record.borrow_mut();
            r["state"] = json!("applying");
            r["applyKey"] = params["idempotencyKey"].clone();
        }
        let host = self.clone();
        let held = record.clone();
        let signal = signal.cloned();
        let promise =
            self.start_record_operation(&record, async move { host.dispatch_audition_apply(&held, signal.as_ref(), reconciliation).await });
        Some(match promise.await {
            Ok(mut outcome) => {
                outcome["idempotent"] = json!(false);
                success_text(id, &outcome)
            }
            Err(e) => self.audition_apply_error(id, &e, &record),
        })
    }
    async fn dispatch_audition_apply(
        &self,

        record: &TransactionRecord,

        signal: Option<&Signal>,

        reconciliation: bool,
    ) -> Result<Value, LiveError> {
        let t = record.borrow().clone();
        let mut dispatched = reconciliation;
        let result = async {
            if reconciliation {
                let fresh = self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(reads::AUDITION_DEADLINE_MS)))).await;
                tokio::task::yield_now().await;
                fresh?;
            }
            let status = self.require_connected(Some("session.read"))?;
            if json!(status.epoch) != t["epoch"] {
                return Err(LiveError::error("Live connection epoch changed; preview again"));
            }
            let adapter = self.async_adapter();
            let context = self.transaction_context(
                &json!({
                "transactionId":t["id"],
                "idempotencyKey":t["applyKey"]}
                ),
                signal,
                reads::AUDITION_DEADLINE_MS,
            );
            let before = self.views.whole_set(Some(&context), Some(TRACK_CONTENT_PARTS)).await;
            tokio::task::yield_now().await;
            let before = before?;
            let state = self.audition_snapshot(&before, t["sceneRef"].as_str().unwrap())?;
            let active = active_targets(&state.playback);
            let eligible = t["eligibleTargetKeys"].as_array().unwrap();
            if reconciliation && !active.is_empty() {
                if active.iter().any(|a| a["sceneRef"] != t["sceneRef"] || !eligible.contains(&json!(target_key(a)))) {
                    return Err(LiveError::error("external playback appeared during audition reconciliation"));
                }
                let launched = json!({
                "launched":t["sceneRef"],
                "targets":active}
                );
                {
                    let mut r = record.borrow_mut();
                    r["launched"] = launched.clone();
                    r["state"] = json!("applied");
                }
                return Ok(json!({
                "transactionId":t["id"],
                "state":"applied",
                "launched":launched,
                "verified":{
                "sceneRef":t["sceneRef"],
                "firedOrPlaying":true}
                ,
                "stopConfirmation":t["stopConfirmation"],
                "reconciled":true}
                ));
            }
            let keys: Vec<_> = eligible.iter().map(|v| v.as_str().unwrap().to_owned()).collect();
            if js_json::stringify(&state.scene) != t["sceneRevision"]
                || state.playback_revision != t["playbackRevision"]
                || state.set["objectIdentity"] != t["setIdentity"]
                || self.audition_authority_revision(&before, t["sceneRef"].as_str().unwrap(), &keys)? != t["authorityRevision"]
            {
                return Err(LiveError::error("audition state or identity hierarchy changed since preview"));
            }
            self.validate_audition_safety(&status, &state, &t["outputSafety"], &t["setName"])?;
            if signal.is_some_and(Signal::aborted) {
                return Err(LiveError::error("audition apply cancelled before dispatch"));
            }
            dispatched = true;
            let result = adapter
                .invoke_async(
                    &LiveInvocation::new(
                        "session.audition-launch",
                        json!({
                        "ref":t["sceneRef"],
                        "setName":t["setName"],
                        "sceneName":state.scene["name"],
                        "sceneIndex":state.scene["index"],
                        "playbackRevision":state.playback["revision"],
                        "eligibleTargets":t["eligibleTargetKeys"],
                        "expectedSetIdentity":t["setIdentity"],
                        "expectedAuthorityRevision":t["authorityRevision"],
                        "outputSafety":t["outputSafety"]}
                        ),
                    ),
                    Some(&context),
                )
                .await?;
            let targets = json_rows(&result["targets"]);
            if result["launched"] != t["sceneRef"]
                || targets.iter().any(|a| !a.is_object() || a["sceneRef"] != t["sceneRef"] || !eligible.contains(&json!(target_key(a))))
            {
                return Err(LiveError::error("guarded launch result does not match the audition target"));
            }
            let launched = json!({
            "launched":result["launched"],
            "targets":targets}
            );
            record.borrow_mut()["launched"] = launched.clone();
            let mut verified = false;
            while kumi_common::time::now_ms_f64() < context.deadline_ms.unwrap() - 250.0 {
                let after = serde_json::to_value(self.views.playback(Some(&context)).await?).unwrap();
                let active = active_targets(&after);
                if active.iter().any(|a| a["sceneRef"] != t["sceneRef"] || !eligible.contains(&json!(target_key(a)))) {
                    return Err(LiveError::error("external playback appeared during launch verification"));
                }
                if !active.is_empty() {
                    verified = true;
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            if !verified {
                return Err(LiveError::error("scene launch was not confirmed by fresh fired or playing target evidence"));
            }
            record.borrow_mut()["state"] = json!("applied");
            Ok(json!({
            "transactionId":t["id"],
            "state":"applied",
            "launched":launched,
            "verified":{
            "sceneRef":t["sceneRef"],
            "firedOrPlaying":true}
            ,
            "stopConfirmation":t["stopConfirmation"]}
            ))
        }
        .await;

        if result.is_err() {
            let mut r = record.borrow_mut();
            r["state"] = json!(if dispatched { "uncertain" } else { "previewed" });
            if !dispatched {
                r.as_object_mut().unwrap().remove("applyKey");
            }
        }
        result
    }
    pub async fn live_session_audition_stop_async(self: &Rc<Self>, id: &Value, params: &Value, signal: Option<&Signal>) -> Option<Value> {
        if !valid_confirmation(params) {
            return Some(error(id, -32602, "transactionId, exact stop confirmation, and idempotencyKey are required", None));
        }
        let Some(record) = self.audition_transactions.get(params["transactionId"].as_str().unwrap()) else {
            return Some(transaction_error(id, "Unknown audition transaction"));
        };
        let t = record.borrow().clone();
        if t["state"] == "stopped" && t["stopKey"] == params["idempotencyKey"] && params["confirmation"] == t["stopConfirmation"] {
            return Some(success_text(
                id,
                &json!({"transactionId":t["id"],
"state":"stopped",
"idempotent":true}),
            ));
        }
        if params["confirmation"] != t["stopConfirmation"] {
            return Some(transaction_error(id, "Exact audition stop confirmation is required"));
        }
        if t["state"] == "stopping" {
            let promise = self.record_operation(&record);
            if t["stopKey"] != params["idempotencyKey"] || promise.is_none() {
                return Some(transaction_error(id, "Audition stop is already in progress with a different request"));
            }
            return Some(match promise.unwrap().await {
                Ok(mut outcome) => {
                    outcome["idempotent"] = json!(true);
                    success_text(id, &outcome)
                }
                Err(e) => adapter_tool_error(id, &e, "Stop is uncertain; do not retry. Perform fresh authoritative playback discovery."),
            });
        }
        if !matches!(t["state"].as_str(), Some("applied" | "uncertain")) {
            return Some(transaction_error(id, "Only mapper-owned applied or uncertain audition playback can be stopped"));
        }
        if t["state"] == "uncertain" && t.get("stopKey").is_some() && t["stopKey"] != params["idempotencyKey"] {
            return Some(transaction_error(id, "Uncertain audition stop requires the exact original idempotency key"));
        }
        if signal.is_some_and(Signal::aborted) {
            return None;
        }
        let stopping_uncertain = t["state"] == "uncertain";
        {
            let mut r = record.borrow_mut();
            r["state"] = json!("stopping");
            r["stopKey"] = params["idempotencyKey"].clone();
        }
        let host = self.clone();
        let held = record.clone();
        let signal = signal.cloned();
        let promise = self
            .start_record_operation(&record, async move { host.dispatch_audition_stop(&held, signal.as_ref(), stopping_uncertain).await });
        Some(match promise.await {
            Ok(mut outcome) => {
                outcome["idempotent"] = json!(false);
                success_text(id, &outcome)
            }
            Err(e) => adapter_tool_error(id, &e, "Stop is uncertain; do not retry. Perform fresh authoritative playback discovery."),
        })
    }
    async fn dispatch_audition_stop(
        &self,

        record: &TransactionRecord,

        signal: Option<&Signal>,

        stopping_uncertain: bool,
    ) -> Result<Value, LiveError> {
        let t = record.borrow().clone();
        let mut dispatched = false;
        let result = async {
            let status = self.require_connected(Some("session.read"))?;
            if json!(status.epoch) != t["epoch"] {
                return Err(LiveError::error("Live connection epoch changed; stop refused"));
            }
            let adapter = self.async_adapter();
            let context = self.transaction_context(
                &json!({
                "transactionId":t["id"],
                "idempotencyKey":t["stopKey"]}
                ),
                signal,
                reads::AUDITION_DEADLINE_MS,
            );
            let snapshot = self.views.whole_set(Some(&context), Some(TRACK_CONTENT_PARTS)).await;
            tokio::task::yield_now().await;
            let snapshot = snapshot?;
            let before = self.audition_snapshot(&snapshot, t["sceneRef"].as_str().unwrap())?;
            let eligible = t["eligibleTargetKeys"].as_array().unwrap();
            let keys: Vec<_> = eligible.iter().map(|v| v.as_str().unwrap().to_owned()).collect();
            if js_json::stringify(&before.scene) != t["sceneRevision"]
                || before.set["name"] != t["setName"]
                || before.set["objectIdentity"] != t["setIdentity"]
                || self.audition_authority_revision(&snapshot, t["sceneRef"].as_str().unwrap(), &keys)? != t["authorityRevision"]
                || before.playback["transport"]["arrangementRecord"] != false
                || before.playback["transport"]["sessionRecord"] != false
                || unsafe_monitoring(&before.tracks)
            {
                return Err(LiveError::error("audition ownership or safety state changed; stop refused"));
            }
            let active = active_targets(&before.playback);
            if !active.is_empty() {
                if active.iter().any(|a| a["sceneRef"] != t["sceneRef"] || !eligible.contains(&json!(target_key(a)))) {
                    return Err(LiveError::error("owned playback is unknown or external playback is active; global stop refused"));
                }
                if signal.is_some_and(Signal::aborted) {
                    return Err(LiveError::error("audition stop cancelled before dispatch"));
                }
                dispatched = true;
                adapter
                    .invoke_async(
                        &LiveInvocation::new(
                            "session.audition-stop",
                            json!({
                            "ref":t["sceneRef"],
                            "setName":t["setName"],
                            "eligibleTargets":t["eligibleTargetKeys"],
                            "expectedSetIdentity":t["setIdentity"],
                            "expectedAuthorityRevision":t["authorityRevision"]}
                            ),
                        ),
                        Some(&context),
                    )
                    .await?;
            }
            let mut confirmed = false;
            while kumi_common::time::now_ms_f64() < context.deadline_ms.unwrap() - 250.0 {
                let state = serde_json::to_value(self.views.playback(Some(&context)).await?).unwrap();
                if state["transport"]["playing"] == false && active_targets(&state).is_empty() {
                    confirmed = true;
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            if !confirmed {
                return Err(LiveError::error("stop acknowledged without fresh stopped verification"));
            }
            record.borrow_mut()["state"] = json!("stopped");
            Ok(json!({
            "transactionId":t["id"],
            "state":"stopped",
            "restoredBaseline":true}
            ))
        }
        .await;

        if result.is_err() {
            record.borrow_mut()["state"] = json!(if dispatched || stopping_uncertain { "uncertain" } else { "applied" });
        }
        result
    }
    pub async fn live_session_emergency_stop_async(
        &self,

        id: &Value,

        params: &Value,

        signal: Option<&Signal>,
    ) -> Result<Option<Value>, LiveError> {
        let validation = (|| -> Result<bool, LiveError> {
            Ok(has_only(params, &["confirmation", "expectedTargets", "expectedRecording", "idempotencyKey"])
                && params["confirmation"] == "emergency-stop"
                && ["stopped", "session", "arrangement", "both"].contains(&helpers::js_string(&params["expectedRecording"])?.as_str())
                && params["expectedTargets"].as_array().is_some_and(|targets| {
                    targets.len() <= 100000
                        && targets.iter().all(|t| is_non_empty_string(t, 1024))
                        && targets.iter().filter_map(Value::as_str).collect::<std::collections::HashSet<_>>().len() == targets.len()
                })
                && params.get("idempotencyKey").is_none_or(is_idempotency_key))
        })();
        if !validation? {
            return Ok(Some(error(
                id,
                -32602,
                "confirmation=emergency-stop plus exact freshly observed active playback targets and recording mode are required",
                None,
            )));
        }
        let result = async {
            let status = self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(reads::AUDITION_DEADLINE_MS)))).await?;
            if !status.connected || !status.capabilities.iter().any(|c| c.as_str() == "session.read") {
                return Err(LiveError::error("session read capability is unavailable"));
            }
            if !status.has_operation("session.emergency-stop") {
                return Err(LiveError::error("emergency stop operation is unavailable"));
            }
            let adapter = self.async_adapter();
            let context = self.transaction_context(params, signal, reads::AUDITION_DEADLINE_MS);
            let playback = serde_json::to_value(self.views.playback(Some(&context)).await?).unwrap();
            let mut keys: Vec<_> = active_targets(&playback).iter().map(target_key).collect();
            keys.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
            keys.dedup();
            let mut expected: Vec<_> =
                params["expectedTargets"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_owned()).collect();
            expected.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
            if keys != expected {
                return Err(LiveError::error("expected targets do not match fresh authoritative playback; perform fresh discovery"));
            }
            if signal.is_some_and(Signal::aborted) {
                return Ok(None);
            }
            let session = playback["transport"]["sessionRecord"] == true;
            let arrangement = playback["transport"]["arrangementRecord"] == true;
            let recording = if session && arrangement {
                "both"
            } else if session {
                "session"
            } else if arrangement {
                "arrangement"
            } else {
                "stopped"
            };
            if params["expectedRecording"] != recording {
                return Err(LiveError::error(
                    "expected recording mode does not match fresh authoritative playback; perform fresh discovery",
                ));
            }
            let result = adapter
                .invoke_async(
                    &LiveInvocation::new(
                        "session.emergency-stop",
                        json!({
                        "expectedTargets":keys,
                        "expectedRecording":recording}
                        ),
                    ),
                    Some(&context),
                )
                .await?;
            let mut confirmed = false;
            while kumi_common::time::now_ms_f64() < context.deadline_ms.unwrap() - 250.0 {
                let after = serde_json::to_value(self.views.playback(Some(&context)).await?).unwrap();
                if after["transport"]["playing"] == false
                    && after["transport"]["sessionRecord"] == false
                    && after["transport"]["arrangementRecord"] == false
                    && active_targets(&after).is_empty()
                {
                    confirmed = true;
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            if !confirmed {
                return Err(LiveError::error("emergency stop was not confirmed by fresh authoritative state"));
            }
            if result.is_null() {
                return Err(LiveError::type_error("Cannot read properties of null (reading 'stoppedTargets')"));
            }
            Ok(Some(success_text(
                id,
                &json!({
                "stopped":true,
                "stoppedTargets":result.get("stoppedTargets").filter(|v|!v.is_null()).cloned().unwrap_or(json!(keys)),
                "recordingStopped":result["recordingStopped"]==true}
                ),
            )))
        }
        .await;

        Ok(result.unwrap_or_else(|e| {
            Some(adapter_tool_error(
                id,
                &e,
                "Emergency stop is uncertain; perform fresh authoritative playback discovery before any further action.",
            ))
        }))
    }
}
