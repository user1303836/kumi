//! Session/take-lane audio import and verified copies into the saved project.
use super::*;
use super::{arrangement::truthy, device_parameter::fields, reads::AUDITION_DEADLINE_MS};
use kumi_common::{abort::Signal, js::json as js_json};
use sha2::{Digest, Sha256};
use std::path::Path;

fn rows(value: &Value) -> &[Value] {
    value.as_array().map(Vec::as_slice).unwrap_or(&[])
}
fn same(a: &Value, b: &Value) -> bool {
    match (a.as_f64(), b.as_f64()) {
        (Some(a), Some(b)) => a == b,
        _ => a == b,
    }
}
fn siblings(lane: &Value) -> Vec<Value> {
    rows(&lane["clips"]).iter().map(|c| fields(c, &["ref", "objectIdentity"])).collect()
}
fn session_fence(track_ref: &Value, track: &Value, slot: &Value, scene: &Value) -> String {
    let mut fence = json!({"trackRef":track_ref});
    for (source, key, destination) in [
        (track, "objectIdentity", "trackIdentity"),
        (slot, "ref", "slotRef"),
        (slot, "objectIdentity", "slotIdentity"),
        (scene, "ref", "sceneRef"),
        (scene, "objectIdentity", "sceneIdentity"),
    ] {
        if let Some(value) = source.get(key) {
            fence[destination] = value.clone();
        }
    }
    js_json::stringify(&fence)
}
fn lane_fence(reference: &Value, lane: &Value) -> String {
    let mut fence = json!({"takeLaneRef":reference});
    if let Some(identity) = lane.get("objectIdentity") {
        fence["laneIdentity"] = identity.clone();
    }
    fence["siblings"] = json!(siblings(lane));
    js_json::stringify(&fence)
}
impl McpHost {
    pub async fn dispatch_audio_import_tool(&self, call: &ToolCall, signal: Option<&Signal>) -> Option<Result<Option<Value>, LiveError>> {
        let args = call.arguments.as_ref().unwrap_or(&Value::Null);
        Some(Ok(match call.name.as_str() {
            "live_audio_import_preview" => Some(self.live_audio_import_preview_async(&call.id, args).await),
            "live_audio_import_apply" => self.live_audio_import_apply_async(&call.id, args, signal).await,
            "live_project_import" => Some(self.live_project_import_async(&call.id, args).await),
            _ => return None,
        }))
    }
    pub async fn live_audio_import_preview_async(&self, id: &Value, params: &Value) -> Value {
        if !has_only(params, &["filePath", "allowedRoot", "trackRef", "sceneIndex", "takeLaneRef", "position", "name"]) {
            return error(id,-32602,"filePath and allowedRoot plus a Session (trackRef, sceneIndex) or take-lane (takeLaneRef, position) destination are required",None);
        }
        let lane = params.get("takeLaneRef").is_some();
        if lane && (params.get("trackRef").is_some() || params.get("sceneIndex").is_some()) {
            return error(id, -32602, "takeLaneRef is mutually exclusive with trackRef/sceneIndex", None);
        }
        if !lane && !is_integer_in_range(&params["sceneIndex"], 0., 100_000.) {
            return error(id, -32602, "sceneIndex is invalid", None);
        }
        if lane && (!is_non_empty_string(&params["takeLaneRef"], 256) || !is_finite_at_least(&params["position"], 0.)) {
            return error(id, -32602, "takeLaneRef and position are required for a take-lane import", None);
        }
        if params.get("name").is_some_and(|v| !is_non_empty_string(v, 256)) {
            return error(id, -32602, "name is invalid", None);
        }
        let result = async {
            let authority = self.audio_import_file_authority(&params["filePath"], &params["allowedRoot"]).await?;
            // The file is copied once the destination is known to take it: a refused preview copies nothing. Where it
            // would go is checked first, as before.
            self.import_staging_root()?;
            let mut staged = None::<String>;
            let retained = async {
                let status = self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS)))).await?;
                if !status.connected || !status.capabilities.iter().any(|c| c.as_str() == "session.read") {
                    return Err(LiveError::error("session read capability is unavailable"));
                }
                let mut transaction = json!({"epoch":status.epoch,"kind":"session-audio-create","state":"previewed"});
                let file = json!({"path":authority["canonicalPath"],"size":authority["size"],"sha256":authority["sha256"]});
                let mut response = json!({"epoch":transaction["epoch"]});
                if lane {
                    if !status.has_operation("take-lane.audio-clip.create") {
                        return Err(LiveError::error("take-lane audio import is unavailable"));
                    }
                    let snapshot = self.views.view_for(None, &[params["takeLaneRef"].clone()], None, &[]).await?;
                    let (lane_track, lane) = self.take_lane_row(&snapshot, params["takeLaneRef"].as_str().unwrap())?;
                    if !is_non_empty_string(&lane["objectIdentity"], 256) {
                        return Err(LiveError::error("take-lane identity is not authoritative"));
                    }
                    // A file's length in beats shows only once Live places it, and Live lays it over what's in the
                    // lane (cutting it, which its API can't put back): so only past the lane's last clip.
                    let under = arrangement_clip::lane_clips_from(&lane, params["position"].as_f64().unwrap());
                    let refused = arrangement_clip::lane_track_refuses(&lane_track, true)
                        .or_else(|| (!under.is_empty()).then(|| arrangement_clip::lane_audio_taken(&lane, &under)));
                    if let Some(why) = refused {
                        return Ok(reason_error(id, &why, arrangement_clip::NOTHING_CHANGED));
                    }
                    let mut payload = fields(params, &["takeLaneRef", "position", "name"]);
                    payload["filePath"] = Value::Null;
                    payload["expectedTakeLaneIdentity"] = lane["objectIdentity"].clone();
                    payload["expectedCollectionRevision"] =
                        json!(hex::encode(Sha256::digest(canonical_mutation_identity(&json!(siblings(&lane)))?)));
                    transaction["fence"] = json!(lane_fence(&params["takeLaneRef"], &lane));
                    transaction["payload"] = payload;
                    transaction["prior"] = json!({"file":authority,"destination":"take-lane"});
                    response["takeLaneRef"] = params["takeLaneRef"].clone();
                    response["position"] = params["position"].clone();
                    response["impact"] = json!("creates-take-lane-audio-clip-no-undo");
                } else {
                    if !status.has_operation("session.audio-clip.create") {
                        return Err(LiveError::error("session audio import is unavailable"));
                    }
                    let snapshot = self.views.view_for(None, &[params["trackRef"].clone()], None, &[]).await?;
                    let snapshot = serde_json::to_value(snapshot).unwrap();
                    let track = rows(&snapshot["tracks"])
                        .iter()
                        .find(|t| t["ref"] == params["trackRef"])
                        .filter(|t| is_non_empty_string(&t["objectIdentity"], 256))
                        .ok_or_else(|| LiveError::error("track identity is not authoritative"))?;
                    let slot =
                        rows(&track["clipSlots"]).iter().filter(|s| s.is_object()).find(|s| same(&s["sceneIndex"], &params["sceneIndex"]));
                    let scene = rows(&snapshot["scenes"]).iter().find(|s| same(&s["index"], &params["sceneIndex"]));
                    let Some((slot, scene)) = slot.zip(scene).filter(|(slot, scene)| {
                        [&slot["ref"], &slot["objectIdentity"], &scene["ref"], &scene["objectIdentity"]]
                            .iter()
                            .all(|v| is_non_empty_string(v, 256))
                    }) else {
                        return Err(LiveError::error("Session import target identity is incomplete"));
                    };
                    if truthy(&slot["clipRef"]) {
                        return Ok(transaction_error(id, "Session slot is occupied"));
                    }
                    let mut payload = fields(params, &["trackRef", "sceneIndex", "name"]);
                    payload["filePath"] = Value::Null;
                    for (key, value) in [
                        ("expectedTrackIdentity", &track["objectIdentity"]),
                        ("expectedSlotRef", &slot["ref"]),
                        ("expectedSlotIdentity", &slot["objectIdentity"]),
                        ("expectedSceneRef", &scene["ref"]),
                        ("expectedSceneIdentity", &scene["objectIdentity"]),
                    ] {
                        payload[key] = value.clone();
                    }
                    transaction["fence"] = json!(session_fence(&params["trackRef"], track, slot, scene));
                    transaction["clipRef"] = params["trackRef"].clone();
                    transaction["payload"] = payload;
                    transaction["prior"] = json!({"file":authority});
                    response["trackRef"] = params["trackRef"].clone();
                    response["sceneIndex"] = params["sceneIndex"].clone();
                    response["impact"] = json!("creates-session-audio-clip");
                }
                let staging = self.stage_verified_import_file(authority["canonicalPath"].as_str().unwrap(), &authority).await?;
                transaction["payload"]["filePath"] = json!(staging);
                staged = Some(staging);
                transaction["id"] = json!(tempo::transaction_id("audioimport"));
                transaction["expiresAt"] = json!(kumi_common::time::now_ms_f64() + TRANSACTION_TTL_MS);
                response["transactionId"] = transaction["id"].clone();
                response["file"] = file;
                response["confirmation"] = json!("apply");
                response["expiresAt"] = transaction["expiresAt"].clone();
                self.retain_bounded_transaction(&self.clip_lifecycle_transactions, transaction, "audio import")?;
                Ok(success_text(id, &response))
            }
            .await;
            if let (Err(_), Some(staging)) = (&retained, &staged) {
                self.release_staged_import_file(&json!(staging))
            }
            retained
        }
        .await;
        result.unwrap_or_else(|cause| {
            adapter_tool_error(id, &cause, "Audio-import preview requires fresh authoritative state and a readable file.")
        })
    }
    pub async fn live_audio_import_apply_async(&self, id: &Value, params: &Value, signal: Option<&Signal>) -> Option<Value> {
        if !valid_transaction_params(params, "apply") {
            return Some(error(id, -32602, "transactionId, confirmation=apply, and idempotencyKey are required", None));
        }
        let record = self.clip_lifecycle_transactions.get(params["transactionId"].as_str().unwrap());
        let t = record.as_ref().map(|t| t.borrow().clone()).unwrap_or(Value::Null);
        if t.is_null()
            || t["kind"] != "session-audio-create"
            || (t["state"] == "previewed" && t["expiresAt"].as_f64().is_some_and(|n| n <= kumi_common::time::now_ms_f64()))
        {
            // Only an expired import of its own: another kind's id names files that may be in use.
            if t["kind"] == "session-audio-create" {
                self.release_staged_import_for(&t);
            }
            return Some(transaction_error(id, "Unknown or expired audio-import transaction"));
        }
        let record = record.unwrap();
        if t["state"] == "applied" && t["applyKey"] == params["idempotencyKey"] {
            return Some(success_text(id, &json!({"transactionId":t["id"],"state":"applied","created":t["created"],"idempotent":true})));
        }
        let reconcile = t["state"] == "uncertain" && t["applyKey"] == params["idempotencyKey"];
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
            // A reconcile's first apply may have made the clip from the staged file: it stays.
            if json!(status.epoch) != t["epoch"] {
                if !reconcile {
                    self.release_staged_import_for(&t);
                }
                return Ok(transaction_error(id, "Live connection epoch changed; preview again"));
            }
            let file = &t["prior"]["file"];
            if !truthy(file) {
                if !reconcile {
                    self.release_staged_import_for(&t);
                }
                return Ok(transaction_error(id, "audio import file authority is missing; preview again"));
            }
            let payload = &t["payload"];
            let path = payload["filePath"].as_str().unwrap_or("");
            if !Path::new(path).exists() {
                return Ok(transaction_error(id, "staged import file is no longer available; preview again"));
            }
            self.verify_staged_import_file(path, file).await?;
            let adapter = self.async_adapter();
            let context = self.transaction_context(params, signal, AUDITION_DEADLINE_MS);
            let lane = payload.get("takeLaneRef").is_some();
            if !reconcile && lane {
                let snapshot = self.views.view_for(Some(&context), &[payload["takeLaneRef"].clone()], None, &[]).await?;
                let (_, lane) = self.take_lane_row(&snapshot, payload["takeLaneRef"].as_str().unwrap_or(""))?;
                if t["fence"] != lane_fence(&payload["takeLaneRef"], &lane) {
                    self.release_staged_import_for(&t);
                    return Ok(transaction_error(id, "take lane or its clips changed since preview; preview again"));
                }
                // A clip stretched past where the file goes keeps its identity: the lane is checked again.
                let under = arrangement_clip::lane_clips_from(&lane, payload["position"].as_f64().unwrap_or(0.0));
                if !under.is_empty() {
                    self.release_staged_import_for(&t);
                    return Ok(reason_error(id, &arrangement_clip::lane_audio_taken(&lane, &under), arrangement_clip::NOTHING_CHANGED));
                }
            }
            if !reconcile && !lane {
                let snapshot = self.views.view_for(Some(&context), &[payload["trackRef"].clone()], None, &[]).await?;
                let snapshot = serde_json::to_value(snapshot).unwrap();
                let track = rows(&snapshot["tracks"]).iter().find(|r| r["ref"] == payload["trackRef"]);
                let slot = track.and_then(|t| {
                    rows(&t["clipSlots"]).iter().filter(|r| r.is_object()).find(|r| same(&r["sceneIndex"], &payload["sceneIndex"]))
                });
                let scene = rows(&snapshot["scenes"]).iter().find(|r| same(&r["index"], &payload["sceneIndex"]));
                let current = track
                    .zip(slot)
                    .zip(scene)
                    .filter(|((track, slot), scene)| t["fence"] == session_fence(&payload["trackRef"], track, slot, scene));
                let Some(((_, slot), _)) = current else {
                    self.release_staged_import_for(&t);
                    return Ok(transaction_error(id, "Session import target changed since preview; preview again"));
                };
                if truthy(&slot["clipRef"]) {
                    self.release_staged_import_for(&t);
                    return Ok(transaction_error(id, "Session slot became occupied since preview; preview again"));
                }
            }
            if let Err(error) = self.keep_staged(&t) {
                return Ok(transaction_error(
                    id,
                    &format!("the staged sample couldn't be kept for the Set ({}); nothing was sent to Live", error.message()),
                ));
            }
            {
                let mut t = record.borrow_mut();
                t["state"] = json!("applying");
                t["applyKey"] = params["idempotencyKey"].clone();
            }
            let result = adapter
                .invoke_async(
                    &LiveInvocation::new(if lane { "take-lane.audio-clip.create" } else { "session.audio-clip.create" }, payload.clone()),
                    Some(&context),
                )
                .await?;
            if result.is_null() {
                return Err(LiveError::type_error("Cannot read properties of null (reading 'ref')"));
            }
            if !is_non_empty_string(&result["ref"], 256)
                || !is_non_empty_string(&result["objectIdentity"], 256)
                || !is_non_empty_string(&result["createdFingerprint"], 64)
                || !is_non_empty_string(&result["filePath"], 1024)
            {
                return Err(LiveError::error("Session audio import did not return exact identity"));
            }
            let snapshot = self
                .views
                .view_for(Some(&context), &[result["ref"].clone(), payload["trackRef"].clone(), payload["takeLaneRef"].clone()], None, &[])
                .await?;
            let located = self.clip_row(&snapshot, result["ref"].as_str().unwrap())?;
            if located.clip["objectIdentity"] != result["objectIdentity"] {
                return Err(LiveError::error("created clip identity was not confirmed by a fresh snapshot"));
            }
            if !is_non_empty_string(&located.clip["filePath"], 1024) {
                return Err(LiveError::error("created clip file was not confirmed by a fresh snapshot"));
            }
            if lane {
                if located.take_lane.as_ref().and_then(|l| l.get("ref")) != payload.get("takeLaneRef") {
                    return Err(LiveError::error("created clip destination was not confirmed by a fresh snapshot"));
                }
            } else {
                let Some(track) = located.track.as_ref().filter(|r| r["ref"] == payload["trackRef"]) else {
                    return Err(LiveError::error("created clip destination was not confirmed by a fresh snapshot"));
                };
                if rows(&track["clipSlots"])
                    .iter()
                    .filter(|r| r.is_object())
                    .find(|r| r["clipRef"] == result["ref"])
                    .is_none_or(|slot| !same(&slot["sceneIndex"], &payload["sceneIndex"]))
                {
                    return Err(LiveError::error("created clip destination was not confirmed by a fresh snapshot"));
                }
            }
            let mapped = adapter.get_async(&LiveRef(result["ref"].as_str().unwrap().into()), Some(&context)).await?;
            if mapped.as_ref().is_none_or(|v| v["objectIdentity"] != result["objectIdentity"]) {
                return Err(LiveError::error("created clip ref does not resolve against the authoritative mapper"));
            }
            {
                let mut t = record.borrow_mut();
                t["created"] = result.clone();
                t["applyKey"] = params["idempotencyKey"].clone();
                t["state"] = json!("applied");
            }
            Ok(success_text(id, &json!({"transactionId":t["id"],"state":"applied","result":result,"idempotent":false})))
        }
        .await;
        Some(result.unwrap_or_else(|cause| {
            apply_failed(id, &record, &cause, "Audio-import state is uncertain; perform fresh discovery before retrying.")
        }))
    }
    pub async fn undo_audio_import_async(&self, id: &Value, params: &Value, signal: Option<&Signal>) -> Value {
        let record = params["transactionId"].as_str().and_then(|id| self.clip_lifecycle_transactions.get(id));
        let t = record.as_ref().map(|t| t.borrow().clone()).unwrap_or(Value::Null);
        if t["kind"] == "session-audio-create" && t["payload"].get("takeLaneRef").is_some() {
            return reason_error(id, arrangement_clip::LANE_CLIP_UNDO, arrangement_clip::LIVE_UNDOES_IT);
        }
        if t["kind"] != "session-audio-create" {
            return transaction_error(id, "Only an applied Session audio import has automatic undo authority");
        }
        let record = record.unwrap();
        if t["state"] == "undone" && t["undoKey"] == params["idempotencyKey"] {
            return success_text(id, &json!({"transactionId":t["id"],"state":"undone","idempotent":true}));
        }
        let reconcile = t["state"] == "uncertain" && t["undoKey"] == params["idempotencyKey"];
        if (t["state"] != "applied" && !reconcile)
            || !is_non_empty_string(&t["created"]["ref"], 256)
            || !is_non_empty_string(&t["created"]["objectIdentity"], 256)
        {
            return transaction_error(id, "Session audio import lacks exact undo identity");
        }
        let result = async {
            self.begin_undo_recovery(&record, params["idempotencyKey"].as_str().unwrap_or(""))?;
            let status = self.require_connected(Some("session.read"))?;
            if json!(status.epoch) != t["epoch"] {
                return Ok(transaction_error(id, "Live connection epoch changed; undo refused"));
            }
            let adapter = self.async_adapter();
            let context = self.transaction_context(params, signal, AUDITION_DEADLINE_MS);
            record.borrow_mut()["undoKey"] = params["idempotencyKey"].clone();
            if reconcile {
                self.replay_undo_recovery(&record, adapter.as_ref(), &context).await?;
            }
            record.borrow_mut()["state"] = json!("undoing");
            self.delete_owned_clip_async(
                adapter.as_ref(),
                t["created"]["ref"].as_str().unwrap(),
                t["created"]["objectIdentity"].as_str().unwrap(),
                &context,
                t["created"]["createdFingerprint"].as_str(),
                Some(&record),
                reconcile,
                None,
            )
            .await?;
            self.release_staged_import_file(&t["payload"]["filePath"]);
            record.borrow_mut()["state"] = json!("undone");
            Ok(success_text(id, &json!({"transactionId":t["id"],"state":"undone","deleted":t["created"]["ref"],"idempotent":false})))
        }
        .await;
        result.unwrap_or_else(|cause| {
            record.borrow_mut()["state"] = json!("uncertain");
            adapter_tool_error(id, &cause, "Audio-import undo is uncertain; inspect the exact created clip.")
        })
    }
    pub async fn live_project_import_async(&self, id: &Value, params: &Value) -> Value {
        if !has_only(params, &["filePath", "allowedRoot"])
            || !is_non_empty_string(&params["filePath"], 1024)
            || !is_non_empty_string(&params["allowedRoot"], 1024)
            || ["filePath", "allowedRoot"].iter().any(|k| params[*k].as_str().is_some_and(|s| s.contains('\0')))
        {
            return error(id, -32602, "filePath and allowedRoot (the folder it must be in) are required", None);
        }
        if ["filePath", "allowedRoot"].iter().any(|k| params[*k].as_str().is_some_and(kumi_common::path::network_or_device)) {
            return error(id, -32602, "files on a network share aren't imported: copy the file onto this computer first", None);
        }
        let mut staged = None;
        let result = async {
            let path = params["filePath"].as_str().unwrap();
            let stat = std::fs::symlink_metadata(path).map_err(|_| LiveError::error("there's no file at that path"))?;
            if stat.file_type().is_symlink() {
                return Err(LiveError::error("that path is a link: import the file itself, from where it is"));
            }
            let authority = self.audio_import_file_authority(&params["filePath"], &params["allowedRoot"]).await?;
            self.require_operation("project.import").await?;
            let path = self.stage_verified_import_file(authority["canonicalPath"].as_str().unwrap(), &authority).await?;
            staged = Some(path.clone());
            let imported = self
                .async_adapter()
                .invoke_async(
                    &LiveInvocation::new("project.import", json!({"filePath":path})),
                    Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS))),
                )
                .await?;
            if imported.is_null() {
                return Err(LiveError::type_error("Cannot read properties of null (reading 'path')"));
            }
            let mut response = json!({"filePath":authority["canonicalPath"],"bytes":authority["size"],"sha256":authority["sha256"]});
            if let Some(path) = imported.get("path") {
                response["path"] = path.clone();
            }
            Ok(success_text(id, &response))
        }
        .await;
        if let Some(path) = staged {
            self.release_staged_import_file(&json!(path))
        }
        result.unwrap_or_else(|cause| adapter_tool_error(id, &cause, "Nothing was copied into the project."))
    }
}
