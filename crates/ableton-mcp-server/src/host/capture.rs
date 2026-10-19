//! Mapper-owned ephemeral resampling capture and independent, identity-fenced recovery.
use super::reads::AUDITION_DEADLINE_MS;
use super::retention::TransactionRecord;
use super::*;
use crate::audio_diagnosis::{diagnose_audio_with_live_context_value, AudioSourceKind, AudioSourceProvenance};
use crate::audio_file::{self, DecodedCaptureFile};
use base64::Engine;
use kumi_common::{
    abort::Signal,
    time::{iso_string, now_ms_f64},
};
use rand::RngCore;
use std::{rc::Weak, time::Duration};

pub(super) struct CaptureController {
    record: Weak<RefCell<Value>>,
    signal: Signal,
}
fn random_confirmation() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}
fn context(host: &McpHost) -> LiveOperationContext {
    LiveOperationContext::with_deadline(host.deadline(AUDITION_DEADLINE_MS))
}
fn file_error(error: audio_file::AudioFileError) -> LiveError {
    LiveError::error(error.to_string())
}
fn rows(value: &Value) -> impl Iterator<Item = &Value> {
    value.as_array().into_iter().flatten()
}
fn recovery_result(residual: Vec<String>) -> Value {
    let mut seen = HashSet::new();
    let residual: Vec<_> = residual.into_iter().filter(|r| seen.insert(r.clone())).collect();
    json!({"safe":residual.is_empty(),"residual":residual})
}
fn mapper_residual(residual: &mut Vec<String>, status: &Value, prefix: &str) {
    for value in rows(&status["residual"]) {
        if let Some(value) = value.as_str().filter(|v| !v.is_empty()) {
            residual.push(format!("{prefix}:{value}"));
        }
    }
}
fn capture_status_redacted(status: Value) -> Result<Value, LiveError> {
    if status.is_null() {
        return Err(LiveError::type_error("Cannot read properties of null (reading 'clip')"));
    }
    let mut redacted = match &status {
        Value::Object(object) => Value::Object(object.clone()),
        Value::Array(items) => Value::Object(items.iter().enumerate().map(|(i, v)| (i.to_string(), v.clone())).collect()),
        Value::String(text) => {
            Value::Object(text.encode_utf16().enumerate().map(|(i, c)| (i.to_string(), json!(String::from_utf16_lossy(&[c])))).collect())
        }
        _ => json!({}),
    };
    redacted.as_object_mut().unwrap().remove("recoveryToken");
    if status["clip"].is_object() {
        let mut clip = device_parameter::fields(&status["clip"], &["ref", "name", "length", "isAudio"]);
        clip["fileAvailable"] = json!(status["clip"]["filePath"].as_str().is_some_and(|v| !v.is_empty()));
        redacted["clip"] = clip;
    }
    Ok(redacted)
}

fn require_capture_capability(status: &LiveStatus, recovery: bool) -> Result<(), LiveError> {
    let operations = if recovery {
        &["audio.capture.status", "audio.capture.emergency-stop", "audio.capture.cleanup"][..]
    } else {
        &[
            "audio.capture.inspect",
            "audio.capture.start",
            "audio.capture.stop",
            "audio.capture.status",
            "audio.capture.emergency-stop",
            "audio.capture.cleanup",
        ][..]
    };
    if !status.connected
        || status.epoch.is_none()
        || status.provenance.as_ref().map(|p| p.as_str()) != Some("real-live")
        || (!recovery && !status.capabilities.iter().any(|c| c.as_str() == "audio.capture.resampling"))
        || operations.iter().any(|o| !status.has_operation(o))
    {
        return Err(LiveError::error("verified real-Live resampling capture capability is unavailable"));
    }
    Ok(())
}
async fn wait_for(milliseconds: f64, signal: Option<&Signal>) -> Result<(), LiveError> {
    if signal.is_some_and(Signal::is_cancelled) {
        return Err(LiveError::error("operation cancelled"));
    }
    let wait = tokio::time::sleep(Duration::from_secs_f64(milliseconds.max(0.0) / 1000.0));
    if let Some(signal) = signal {
        tokio::select! { biased; _ = signal.cancelled() => Err(LiveError::error("operation cancelled")), _ = wait => Ok(()) }
    } else {
        wait.await;
        Ok(())
    }
}
impl McpHost {
    pub async fn dispatch_capture_tool(
        self: &Rc<Self>,
        call: &ToolCall,
        signal: Option<&Signal>,
    ) -> Option<Result<Option<Value>, LiveError>> {
        let params = call.arguments.as_ref().unwrap_or(&Value::Null);
        Some(Ok(match call.name.as_str() {
            "live_audio_capture_preview" => Some(self.live_audio_capture_preview_async(&call.id, params).await),
            "live_audio_capture_apply" => self.live_audio_capture_apply_async(&call.id, params, signal).await,
            "live_audio_capture_status" => Some(self.live_audio_capture_status_async(&call.id, call.arguments.as_ref()).await),
            "live_audio_capture_emergency_stop" => Some(self.live_audio_capture_emergency_stop_async(&call.id, params).await),
            _ => return None,
        }))
    }
    pub async fn live_audio_capture_preview_async(&self, id: &Value, params: &Value) -> Value {
        if !has_only(params, &["setName", "sourceSlotRef", "destinationSlotRef", "durationSeconds", "consent", "outputSafety"])
            || ["setName", "sourceSlotRef", "destinationSlotRef"].iter().any(|k| !is_non_empty_string(&params[k], 256))
            || !params["durationSeconds"].as_f64().is_some_and(|v| v.is_finite() && (1.0..=9.0).contains(&v))
            || params["consent"] != "ephemeral-analysis-and-delete"
        {
            return error(id, -32602, "exact Set/slots, 1-9 second duration, consent, and output safety are required", None);
        }
        let result = async {
            // Source validateOutputSafety intentionally accepts all inputs; outputSafetyOf supplies the default.
            let status = self.fresh_status(Some(&context(self))).await?;
            require_capture_capability(&status, false)?;
            let plan = self
                .async_adapter()
                .invoke_async(
                    &LiveInvocation::new(
                        "audio.capture.inspect",
                        device_parameter::fields(params, &["setName", "sourceSlotRef", "destinationSlotRef"]),
                    ),
                    Some(&context(self)),
                )
                .await?;
            if plan.is_null() {
                return Err(LiveError::type_error("Cannot read properties of null (reading 'supported')"));
            }
            if plan["supported"] != true
                || !is_non_empty_string(&plan["fence"], 64)
                || !is_non_empty_string(&plan["destinationTrackRef"], 256)
                || !plan["prior"].is_object()
            {
                return Err(LiveError::error("capture mapper did not return a complete authoritative plan"));
            }
            let transaction = json!({"id":tempo::transaction_id("audio_capture"),"captureId":tempo::transaction_id("capture"),"epoch":status.epoch,"setName":params["setName"],"sourceSlotRef":params["sourceSlotRef"],"destinationSlotRef":params["destinationSlotRef"],"destinationTrackRef":plan["destinationTrackRef"],"fence":plan["fence"],"prior":plan["prior"],"durationMs":(params["durationSeconds"].as_f64().unwrap()*1000.0).round(),"outputSafety":output_safety_of(&params["outputSafety"]),"confirmation":random_confirmation(),"expiresAt":now_ms_f64()+60_000.0,"state":"previewed"});
            self.retain_bounded_transaction(&self.audio_capture_transactions, transaction.clone(), "audio capture")?;
            let mut out = json!({"transactionId":transaction["id"],"captureId":transaction["captureId"],"epoch":transaction["epoch"],"sourceSlotRef":transaction["sourceSlotRef"],"destinationSlotRef":transaction["destinationSlotRef"],"destinationTrackRef":transaction["destinationTrackRef"],"prior":transaction["prior"],"durationSeconds":transaction["durationMs"].as_f64().unwrap()/1000.0,"consent":params["consent"],"rawRetention":"ephemeral-until-analysis-then-unlink","outputSafety":transaction["outputSafety"],"audibleImpact":"plays-one-exact-session-clip-while-recording-one-exact-resampling-slot","confirmation":transaction["confirmation"],"expiresAt":transaction["expiresAt"],"recovery":{"watchdog":true,"statusTool":"live_audio_capture_status","emergencyTool":"live_audio_capture_emergency_stop"}});
            if let Some(mode) = plan.get("captureMode") {
                out["captureMode"] = mode.clone();
            }
            Ok(success_text(id, &out))
        }
        .await;
        result.unwrap_or_else(|e|adapter_tool_error(id,&e,"Capture preview made no changes; choose a real-Live source clip and exact empty audio destination slot in the disposable Set."))
    }
    async fn capture_recovery_status(&self, adapter: &dyn AsyncLiveAdapter) -> Result<Value, LiveError> {
        let status = self.capture_mapper_status(adapter).await?;
        if status.is_null() {
            return Err(LiveError::type_error("Cannot read properties of null (reading 'state')"));
        }
        Ok(status)
    }
    async fn wait_for_captured_media(
        &self,
        adapter: &dyn AsyncLiveAdapter,
        signal: Option<&Signal>,
        deadline: Option<f64>,
    ) -> Result<Value, LiveError> {
        let deadline = deadline.unwrap_or_else(|| now_ms_f64() + 5000.0);
        let mut status = self.capture_recovery_status(adapter).await?;
        while now_ms_f64() < deadline {
            if status["state"] == "failed" {
                return Err(LiveError::error("capture mapper reported failed cleanup or stop state"));
            }
            if status["playbackStopped"] == true
                && status["clip"].is_object()
                && is_non_empty_string(&status["clip"]["ref"], 256)
                && is_non_empty_string(&status["clip"]["filePath"], 4096)
            {
                return Ok(status);
            }
            wait_for(100.0, signal).await?;
            status = self.capture_recovery_status(adapter).await?;
        }
        Err(LiveError::error("capture media identity did not become authoritative before the bounded deadline"))
    }
    async fn recover_audio_capture(&self, transaction: &TransactionRecord, acquired: Option<DecodedCaptureFile>) -> Value {
        let mut residual = Vec::new();
        let mut media = acquired;
        let mut raw_confirmed_absent = false;
        let t = transaction.borrow().clone();
        let result = async {
            let adapter = self.async_adapter();
            let mut status = self.capture_recovery_status(&*adapter).await?;
            if status["state"] == "idle" {
                if t["startDispatched"] == true || t["mapperToken"].as_str().is_some_and(|s| !s.is_empty()) || media.is_some() {
                    residual.push("capture-lifecycle-is-not-observable".into());
                }
                return Ok::<_, LiveError>(());
            }
            if ["captureId", "sourceSlotRef", "destinationSlotRef"].iter().any(|k| status[k] != t[k]) {
                residual.push("foreign-capture-lifecycle-observed".into());
                return Ok(());
            }
            mapper_residual(&mut residual, &status, "mapper");
            if status["active"] == true
                || status["playbackStopped"] != true
                || matches!(status["state"].as_str(), Some("active" | "failed"))
            {
                if adapter
                    .invoke_async(
                        &LiveInvocation::new(
                            "audio.capture.emergency-stop",
                            device_parameter::fields(&t, &["captureId", "sourceSlotRef", "destinationSlotRef"]),
                        ),
                        Some(&context(self)),
                    )
                    .await
                    .is_err()
                {
                    residual.push("capture-emergency-stop-unverified".into());
                }
                status = self.capture_recovery_status(&*adapter).await?;
                if status["captureId"] != t["captureId"] {
                    residual.push("capture-identity-changed-during-recovery".into());
                    return Ok(());
                }
                let stop_deadline = now_ms_f64() + 5000.0;
                while (status["active"] == true || status["playbackStopped"] != true) && now_ms_f64() < stop_deadline {
                    wait_for(100.0, None).await?;
                    status = self.capture_recovery_status(&*adapter).await?;
                    if status["captureId"] != t["captureId"] {
                        break;
                    }
                }
                mapper_residual(&mut residual, &status, "mapper-after-stop");
            }
            if status["active"] == true || status["playbackStopped"] != true {
                residual.push("capture-playback-not-stopped".into());
            }
            if matches!(status["state"].as_str(), Some("stopped" | "captured" | "failed")) && !status["clip"].is_object() {
                let expires = status["expiresAt"].as_f64().unwrap_or_else(now_ms_f64);
                let deadline = (now_ms_f64() + 12000.0).min((now_ms_f64() + 5000.0).max(expires + 2000.0));
                match self.wait_for_captured_media(&*adapter, None, Some(deadline)).await {
                    Ok(next) => status = next,
                    Err(_) => {
                        residual.push("capture-media-finalization-unresolved".into());
                        status = self.capture_recovery_status(&*adapter).await?;
                    }
                }
            }
            if status["captureId"] != t["captureId"] {
                residual.push("capture-identity-changed-before-cleanup".into());
                return Ok(());
            }
            let clip = status.get("clip").filter(|c| c.is_object()).cloned();
            let token =
                t.get("mapperToken").filter(|v| !v.is_null()).or_else(|| status.get("recoveryToken").filter(|v| v.is_string())).cloned();
            if media.is_none() && clip.as_ref().is_some_and(|c| is_non_empty_string(&c["filePath"], 4096)) {
                let path = clip.as_ref().unwrap()["filePath"].as_str().unwrap();
                let acquire = async {
                    let snapshot =
                        self.views.view(Some(&context(self)), LiveViewScope::Indices(vec![]), Some(&[LiveSnapshotPart::Set])).await?;
                    let snapshot = serde_json::to_value(snapshot).unwrap();
                    if let Some(project) = snapshot["set"]["filePath"].as_str().filter(|s| !s.is_empty()) {
                        transaction.borrow_mut()["projectFilePath"] = json!(project);
                        media = Some(
                            audio_file::decode_owned_wave_file(path, project, t["startedAt"].as_f64().unwrap_or_else(now_ms_f64))
                                .await
                                .map_err(file_error)?,
                        );
                    } else {
                        residual.push("saved-project-path-unavailable".into());
                    }
                    Ok::<_, LiveError>(())
                }
                .await;
                if acquire.is_err() {
                    let absent = async {
                        let snapshot =
                            self.views.view(Some(&context(self)), LiveViewScope::Indices(vec![]), Some(&[LiveSnapshotPart::Set])).await?;
                        let snapshot = serde_json::to_value(snapshot).unwrap();
                        if let Some(project) = snapshot["set"]["filePath"].as_str().filter(|s| !s.is_empty()) {
                            transaction.borrow_mut()["projectFilePath"] = json!(project);
                            return audio_file::capture_media_is_absent(path, project).await.map_err(file_error);
                        }
                        Ok(false)
                    }
                    .await;
                    raw_confirmed_absent = absent.unwrap_or(false);
                    if !raw_confirmed_absent {
                        residual.push("raw-media-could-not-be-verified-for-unlink".into());
                    }
                }
            }
            let cleaned_media = media.as_ref().map(DecodedCaptureFile::owned);
            let mut raw_cleanup_safe = raw_confirmed_absent
                || transaction.borrow()["rawPrimaryUnlinked"] == true
                || (media.is_none() && status["state"] == "cleaned");
            if let Some(media) = media.as_ref().filter(|_| transaction.borrow()["rawPrimaryUnlinked"] != true) {
                match audio_file::unlink_owned_capture_file(&media.owned()).await {
                    Ok(()) => {
                        transaction.borrow_mut()["rawPrimaryUnlinked"] = json!(true);
                        raw_cleanup_safe = true;
                    }
                    Err(_) => residual.push("transaction-owned-raw-file-not-unlinked".into()),
                }
            }
            let mut live_cleanup_safe = status["state"] == "cleaned";
            if let (Some(clip), Some(token)) = (
                clip.as_ref().filter(|c| is_non_empty_string(&c["ref"], 256)),
                token.as_ref().filter(|v| v.as_str().is_some_and(|s| !s.is_empty())),
            ) {
                if raw_cleanup_safe {
                    match adapter
                        .invoke_async(
                            &LiveInvocation::new(
                                "audio.capture.cleanup",
                                json!({"captureId":t["captureId"],"token":token,"expectedClipRef":clip["ref"]}),
                            ),
                            Some(&context(self)),
                        )
                        .await
                    {
                        Ok(cleaned) if !cleaned.is_null() => {
                            mapper_residual(&mut residual, &cleaned, "mapper-cleanup");
                            live_cleanup_safe = true;
                        }
                        _ => residual.push("transaction-owned-live-clip-not-cleaned".into()),
                    }
                } else if status["state"] != "cleaned" {
                    residual.push("capture-live-clip-retained-for-raw-recovery".into());
                }
            } else if status["state"] != "cleaned" {
                residual.push(
                    if !raw_cleanup_safe {
                        "capture-live-clip-retained-for-raw-recovery"
                    } else if clip.is_some() {
                        "capture-cleanup-authority-unavailable"
                    } else {
                        "capture-live-clip-state-unresolved"
                    }
                    .into(),
                );
            }
            let project = transaction.borrow()["projectFilePath"].as_str().filter(|s| !s.is_empty()).map(str::to_owned);
            if let (Some(cleaned), Some(project)) = (cleaned_media.filter(|_| live_cleanup_safe), project) {
                let late = async {
                    audio_file::unlink_late_capture_companions(&cleaned).await.map_err(file_error)?;
                    if !audio_file::capture_media_is_absent(&cleaned.real_path, &project).await.map_err(file_error)? {
                        return Err(LiveError::error("capture media remains after late companion sweep"));
                    }
                    Ok::<_, LiveError>(())
                }
                .await;
                if late.is_err() {
                    residual.push("late-capture-companion-not-cleaned".into());
                }
            }
            let final_status = self.capture_recovery_status(&*adapter).await?;
            if final_status["captureId"] != t["captureId"] {
                residual.push("capture-final-identity-mismatch".into());
            }
            mapper_residual(&mut residual, &final_status, "mapper-final");
            if final_status["active"] == true || final_status["playbackStopped"] != true {
                residual.push("capture-remains-active".into());
            }
            if final_status["state"] != "cleaned" {
                residual.push("capture-lifecycle-not-cleaned".into());
            }
            if final_status["clip"].is_object() {
                residual.push("capture-clip-remains-present".into());
            }
            match self
                .views
                .view_for(
                    Some(&context(self)),
                    &[final_status["destinationTrackRef"].clone(), t["destinationTrackRef"].clone(), t["destinationSlotRef"].clone()],
                    None,
                    &[],
                )
                .await
            {
                Err(_) => residual.push("fresh-recovery-snapshot-unavailable".into()),
                Ok(snapshot) => {
                    let snapshot = serde_json::to_value(snapshot).unwrap();
                    let playback = &snapshot["playback"];
                    if ["playing", "arrangementRecord", "sessionRecord"].iter().any(|k| playback["transport"][k] != false)
                        || rows(&playback["firedTargets"]).next().is_some()
                        || rows(&playback["playingTargets"]).next().is_some()
                    {
                        residual.push("fresh-playback-readback-not-stopped".into());
                    }
                    let reference = if is_non_empty_string(&final_status["destinationTrackRef"], 256) {
                        &final_status["destinationTrackRef"]
                    } else {
                        &t["destinationTrackRef"]
                    };
                    let destination = rows(&snapshot["tracks"]).find(|track| {
                        &track["ref"] == reference || rows(&track["clipSlots"]).any(|slot| slot["ref"] == t["destinationSlotRef"])
                    });
                    let slot = destination.and_then(|track| rows(&track["clipSlots"]).find(|slot| slot["ref"] == t["destinationSlotRef"]));
                    if slot.is_none_or(|slot| slot["empty"] != true || slot.get("clipRef").is_some_and(arrangement::truthy)) {
                        residual.push("fresh-destination-slot-not-empty".into());
                    }
                    if let Some(destination) = destination {
                        for (prior, key, message) in [
                            ("arm", "armed", "fresh-destination-arm-not-restored"),
                            ("monitoring", "monitoringState", "fresh-destination-monitoring-not-restored"),
                        ] {
                            if t["prior"].get(prior).is_some_and(|v| destination.get(key) != Some(v)) {
                                residual.push(message.into());
                            }
                        }
                        if t["prior"].get("route").is_some_and(|v| destination["routing"].get("inputType") != Some(v)) {
                            residual.push("fresh-destination-route-not-restored".into());
                        }
                        if t["prior"].get("arm").is_none() && destination["armed"] != false {
                            residual.push("fresh-destination-remains-armed".into());
                        }
                    }
                }
            }
            Ok(())
        }
        .await;
        if result.is_err() {
            residual.push("capture-emergency-recovery-unavailable".into());
        }
        recovery_result(residual)
    }
    async fn await_audio_capture_apply(&self, id: &Value, record: &TransactionRecord, signal: Option<&Signal>) -> Option<Value> {
        let Some(inflight) = self.record_operation(record) else {
            return Some(transaction_error(id, "Audio-capture apply is no longer in flight"));
        };
        let waiters = record.borrow()["waiters"].as_u64().unwrap_or(0) + 1;
        record.borrow_mut()["waiters"] = json!(waiters);
        let mut aborted = signal.is_some_and(Signal::is_cancelled);
        let settled = if aborted {
            None
        } else if let Some(signal) = signal {
            tokio::select! {biased; value=inflight=>Some(value), _=signal.cancelled()=>{aborted=true;None}}
        } else {
            Some(inflight.await)
        };
        let remaining = record.borrow()["waiters"].as_u64().unwrap_or(1).saturating_sub(1);
        record.borrow_mut()["waiters"] = json!(remaining);
        if aborted && remaining == 0 {
            if let Some(controller) =
                self.capture_controllers.borrow().iter().find(|c| c.record.upgrade().is_some_and(|r| Rc::ptr_eq(&r, record)))
            {
                controller.signal.cancel();
            }
        }
        settled.and_then(|result| result.ok()).filter(|v| !v.is_null()).map(|mut value| {
            value["id"] = id.clone();
            value
        })
    }
    pub async fn live_audio_capture_apply_async(self: &Rc<Self>, id: &Value, params: &Value, signal: Option<&Signal>) -> Option<Value> {
        if !has_only(params, &["transactionId", "confirmation", "idempotencyKey"])
            || !is_non_empty_string(&params["transactionId"], 128)
            || !is_non_empty_string(&params["confirmation"], 128)
            || !is_idempotency_key(&params["idempotencyKey"])
        {
            return Some(error(id, -32602, "transactionId, exact confirmation, and idempotencyKey are required", None));
        }
        let Some(record) = self.audio_capture_transactions.get(params["transactionId"].as_str().unwrap()) else {
            return Some(transaction_error(id, "Unknown or expired audio-capture transaction"));
        };
        let t = record.borrow().clone();
        if params["confirmation"] != t["confirmation"] {
            return Some(transaction_error(id, "Audio-capture confirmation is invalid"));
        }
        if t["state"] == "completed" && t["applyKey"] == params["idempotencyKey"] && t.get("result").is_some_and(arrangement::truthy) {
            let mut result = t["result"].clone();
            result["idempotent"] = json!(true);
            return Some(success_text(id, &result));
        }
        if t.get("inflight").is_some_and(arrangement::truthy) {
            if t["applyKey"] != params["idempotencyKey"] {
                return Some(transaction_error(id, "Audio-capture apply is already in progress with a different idempotency key"));
            }
            return self.await_audio_capture_apply(id, &record, signal).await;
        }
        if t["state"] != "previewed" || t["expiresAt"].as_f64().is_some_and(|v| v <= now_ms_f64()) {
            return Some(transaction_error(id, "Audio-capture preview expired or is no longer applicable"));
        }
        if signal.is_some_and(Signal::is_cancelled) {
            return None;
        }
        {
            let mut t = record.borrow_mut();
            t["applyKey"] = params["idempotencyKey"].clone();
            t["state"] = json!("applying");
            t["abortController"] = json!({});
        }
        let controller = Signal::new();
        self.capture_controllers.borrow_mut().retain(|c| c.record.upgrade().is_some_and(|r| !Rc::ptr_eq(&r, &record)));
        self.capture_controllers.borrow_mut().push(CaptureController { record: Rc::downgrade(&record), signal: controller.clone() });
        let host = self.clone();
        let held = record.clone();
        let request_id = id.clone();
        let _ = self.start_record_operation(&record, async move {
            let result = host.dispatch_audio_capture_apply(&request_id, &held, Some(&controller)).await.unwrap_or(Value::Null);
            {
                let mut t = held.borrow_mut();
                let object = t.as_object_mut().unwrap();
                object.remove("inflight");
                object.remove("abortController");
            }
            host.capture_controllers.borrow_mut().retain(|c| c.record.upgrade().is_some_and(|r| !Rc::ptr_eq(&r, &held)));
            Ok(result)
        });
        self.await_audio_capture_apply(id, &record, signal).await
    }
    async fn dispatch_audio_capture_apply(&self, id: &Value, record: &TransactionRecord, signal: Option<&Signal>) -> Option<Value> {
        let mut acquired = None;
        let t = record.borrow().clone();
        let result = async {
            let status = self.fresh_status(Some(&context(self))).await;
            tokio::task::yield_now().await;
            let status = status?;
            require_capture_capability(&status, false)?;
            if json!(status.epoch) != t["epoch"] {
                return Err(LiveError::error("Live connection epoch changed; capture must be previewed again"));
            }
            let adapter = self.async_adapter();
            if signal.is_some_and(Signal::is_cancelled) {
                return Err(LiveError::error("audio capture cancelled before audible dispatch"));
            }
            let started_at = now_ms_f64();
            {
                let mut t = record.borrow_mut();
                t["startedAt"] = json!(started_at);
                t["startDispatched"] = json!(true);
            }
            let key = json!({"transactionId":t["id"],"idempotencyKey":t["applyKey"]});
            let invoke_context = |signal: Option<&Signal>| self.transaction_context(&key, signal, AUDITION_DEADLINE_MS);
            let started =
                adapter.invoke_async(&LiveInvocation::new("audio.capture.start", json!({"captureId":t["captureId"],"setName":t["setName"],"sourceSlotRef":t["sourceSlotRef"],"destinationSlotRef":t["destinationSlotRef"],"fence":t["fence"],"maxDurationMs":10000.0_f64.min(t["durationMs"].as_f64().unwrap()+3000.0),"outputSafety":t["outputSafety"]})), Some(&invoke_context(signal))).await?;
            if !is_non_empty_string(&started["token"], 128) || started["state"] != "active" {
                return Err(LiveError::error("capture mapper did not confirm bounded authority"));
            }
            {
                let mut t = record.borrow_mut();
                t["mapperToken"] = started["token"].clone();
                t["state"] = json!("capturing");
            }
            wait_for(t["durationMs"].as_f64().unwrap(), signal).await?;
            adapter.invoke_async(&LiveInvocation::new("audio.capture.stop", json!({"captureId":t["captureId"],"token":started["token"]})), Some(&invoke_context(None))).await?;
            let capture = self.wait_for_captured_media(&*adapter, signal, None).await?;
            if rows(&capture["residual"]).next().is_some() {
                return Err(LiveError::error("capture mapper reported residual state"));
            }
            let snapshot = self.views.view_for(Some(&invoke_context(signal)), &[t["sourceSlotRef"].clone()], None, &[]).await?;
            let snapshot = serde_json::to_value(snapshot).unwrap();
            let project = snapshot["set"]["filePath"]
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| LiveError::error("capture requires an authoritatively saved Live Set path"))?;
            record.borrow_mut()["projectFilePath"] = json!(project);
            acquired = Some(
                audio_file::decode_owned_wave_file(capture["clip"]["filePath"].as_str().unwrap(), project, started_at)
                    .await
                    .map_err(file_error)?,
            );
            record.borrow_mut()["state"] = json!("analyzing");
            let media = acquired.as_ref().unwrap();
            let bytes: Vec<_> = media.samples.iter().flat_map(|v| v.to_le_bytes()).collect();
            let source = json!({"pcmBase64":base64::engine::general_purpose::STANDARD.encode(bytes),"sampleRate":media.sample_rate,"channels":media.channels,"channelLayout":if media.channels==1{vec!["M"]}else{vec!["L","R"]}});
            let analysis = self
                .analysis_runner
                .run_value(&json!({"mode":"analyze","source":source}), signal.cloned(), None)
                .await
                .map_err(|e| LiveError::error(e.to_string()))?;
            let track = rows(&snapshot["tracks"])
                .find(|track| rows(&track["clipSlots"]).any(|slot| slot["ref"] == t["sourceSlotRef"]))
                .ok_or_else(|| LiveError::error("capture source slot is absent from the fresh diagnosis snapshot"))?;
            let typed = serde_json::from_value(analysis.clone()).map_err(|e| LiveError::error(e.to_string()))?;
            let provenance = AudioSourceProvenance {
                kind: AudioSourceKind::VerifiedLiveResamplingCapture,
                observed_at: iso_string(now_ms_f64() as i64),
                description: "Mapper-owned Session-slot Resampling capture".into(),
                capture_id: t["captureId"].as_str().map(str::to_owned),
            };
            let diagnosis = diagnose_audio_with_live_context_value(
                &typed,
                &snapshot,
                t["epoch"].as_i64().unwrap(),
                track["ref"].as_str().unwrap(),
                &provenance,
                None,
            )
            .map_err(|e| LiveError::error(e.to_string()))?;
            let summary = json!({"format":media.format,"bitsPerSample":media.bits_per_sample,"sampleRate":media.sample_rate,"channels":media.channels,"durationSeconds":media.duration_seconds,"byteLength":media.byte_length,"byteLengthBounded":true,"rawPathReturned":false});
            audio_file::unlink_owned_capture_file(&media.owned()).await.map_err(file_error)?;
            record.borrow_mut()["rawPrimaryUnlinked"] = json!(true);
            adapter.invoke_async(&LiveInvocation::new("audio.capture.cleanup", json!({"captureId":t["captureId"],"token":started["token"],"expectedClipRef":capture["clip"]["ref"]})), Some(&invoke_context(None))).await?;
            audio_file::unlink_late_capture_companions(&media.owned()).await.map_err(file_error)?;
            if !audio_file::capture_media_is_absent(&media.real_path, project).await.map_err(file_error)? {
                return Err(LiveError::error("capture media did not verify absent after Live clip cleanup"));
            }
            acquired = None;
            let final_status = self.capture_recovery_status(&*adapter).await?;
            let final_snapshot = self.views.view_for(Some(&context(self)), &[t["destinationTrackRef"].clone()], None, &[]).await?;
            let final_snapshot = serde_json::to_value(final_snapshot).unwrap();
            let transport = &final_snapshot["playback"]["transport"];
            let destination = rows(&final_snapshot["tracks"]).find(|track| track["ref"] == t["destinationTrackRef"]);
            if final_status["state"] != "cleaned"
                || final_status["active"] != false
                || ["playing", "arrangementRecord", "sessionRecord"].iter().any(|k| transport[k] != false)
                || destination.is_none_or(|d| {
                    d.get("armed") != t["prior"].get("arm")
                        || d.get("monitoringState") != t["prior"].get("monitoring")
                        || d["routing"].get("inputType") != t["prior"].get("route")
                })
            {
                return Err(LiveError::error("capture teardown did not verify the exact stopped baseline"));
            }
            let result = json!({"transactionId":t["id"],"captureId":t["captureId"],"state":"completed","provenance":"real-live","sourceSlotRef":t["sourceSlotRef"],"destinationSlotRef":t["destinationSlotRef"],"durationRequestedSeconds":t["durationMs"].as_f64().unwrap()/1000.0,"media":summary,"analysis":analysis,"diagnosis":diagnosis,"cleanup":{"captureStopped":true,"transportStopped":true,"routingRestored":true,"armRestored":true,"monitoringRestored":true,"liveClipDeleted":true,"rawFileUnlinked":true,"rawAudioRetained":false},"idempotent":false});
            {
                let mut t = record.borrow_mut();
                t["result"] = result.clone();
                t["state"] = json!("completed");
            }
            Ok::<_, LiveError>(success_text(id, &result))
        }
        .await;
        match result {
            Ok(result) => Some(result),
            Err(cause) => {
                let recovery = self.recover_audio_capture(record, acquired).await;
                let aborted = signal.is_some_and(Signal::is_cancelled);
                let state = if aborted && recovery["safe"] == true { "cancelled" } else { "uncertain" };
                record.borrow_mut()["state"] = json!(state);
                if aborted {
                    return None;
                }
                let body = json!({"reason":"capture lifecycle did not reach a verified clean completion","failureClass":if matches!(cause,LiveError::RangeError(_)){"bounded-input-or-media-validation"}else{"capture-lifecycle-failure"},"captureId":t["captureId"],"state":state,"cleanup":recovery,"remediation":if recovery["safe"]==true{"Preview again from fresh stopped state."}else{"Use live_audio_capture_status and the independent emergency-stop tool; do not retry capture while residual state remains."}});
                Some(response(id, json!({"content":[{"type":"text","text":kumi_common::js::json::stringify(&body)}],"isError":true})))
            }
        }
    }
    pub async fn live_audio_capture_status_async(&self, id: &Value, params: Option<&Value>) -> Value {
        if !utility_params(params) {
            return error(id, -32602, "capture status takes no arguments", None);
        }
        let result = async {
            let status = self.fresh_status(Some(&context(self))).await?;
            require_capture_capability(&status, true)?;
            Ok::<_, LiveError>(success_text(id, &capture_status_redacted(self.capture_mapper_status(&*self.async_adapter()).await?)?))
        }
        .await;
        result.unwrap_or_else(|e| {
            adapter_tool_error(
                id,
                &e,
                "Capture status is unavailable; independently verify Live recording, transport, arm, monitoring, and routing state.",
            )
        })
    }
    pub async fn live_audio_capture_emergency_stop_async(&self, id: &Value, params: &Value) -> Value {
        if !has_only(params, &["confirmation", "captureId", "sourceSlotRef", "destinationSlotRef"])
            || params["confirmation"] != "emergency-stop-and-clean"
            || !is_non_empty_string(&params["captureId"], 128)
            || ["sourceSlotRef", "destinationSlotRef"].iter().any(|k| !is_non_empty_string(&params[k], 256))
        {
            return error(id, -32602, "exact fresh capture identities and confirmation are required", None);
        }
        let synthetic = Rc::new(RefCell::new(
            json!({"id":format!("recovery_{}",params["captureId"].as_str().unwrap()),"captureId":params["captureId"],"epoch":0,"setName":"recovery","sourceSlotRef":params["sourceSlotRef"],"destinationSlotRef":params["destinationSlotRef"],"destinationTrackRef":params["destinationSlotRef"],"fence":"","prior":{},"durationMs":0,"outputSafety":{},"confirmation":"","expiresAt":now_ms_f64()+10000.0,"state":"uncertain"}),
        ));
        let result = async {
            let status = self.fresh_status(Some(&context(self))).await?;
            require_capture_capability(&status, true)?;
            let observed = self.capture_mapper_status(&*self.async_adapter()).await?;
            if ["captureId", "sourceSlotRef", "destinationSlotRef"].iter().any(|k| observed[k] != params[k]) {
                return Err(LiveError::error("capture emergency observation is stale or inexact"));
            }
            {
                let mut t = synthetic.borrow_mut();
                t["epoch"] = json!(status.epoch);
                t["startedAt"] = json!(observed["startedAt"].as_f64().unwrap_or_else(now_ms_f64));
                if let Some(token) = observed["recoveryToken"].as_str() {
                    t["mapperToken"] = json!(token);
                }
            }
            let recovery = self.recover_audio_capture(&synthetic, None).await;
            Ok(success_text(id, &json!({"captureId":params["captureId"],"state":if recovery["safe"]==true{"cleaned"}else{"uncertain"},"stopped":true,"cleanup":recovery,"rawPathReturned":false})))
        }
        .await;
        result.unwrap_or_else(|e| {
            adapter_tool_error(
                id,
                &e,
                "Emergency capture cleanup is uncertain; manually stop Live and inspect the exact destination slot and project media.",
            )
        })
    }
}

#[cfg(test)]
#[path = "capture_tests.rs"]
mod tests;
