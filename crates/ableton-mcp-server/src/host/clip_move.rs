//! Atomic Session moves and guarded Arrangement moves, including exact-key restoration.
use super::*;
use arrangement::capture_object_fingerprint;
use clip_duplicate::{target_fence, target_rows};
use kumi_common::{
    abort::{Signal, SignalExt},
    js::json as js_json,
};

fn arrangement_move_fence(reference: &Value, clip: &Value, fingerprint: &str) -> String {
    let mut row = json!({"ref":reference});
    for field in ["objectIdentity", "start"] {
        if let Some(v) = clip.get(field) {
            row[field] = v.clone();
        }
    }
    row["contentFingerprint"] = json!(fingerprint);
    js_json::stringify(&row)
}
fn merge(target: &mut Value, source: &Value) {
    if let Some(o) = source.as_object() {
        for (k, v) in o {
            target[k] = v.clone();
        }
    }
}
fn number_equal(a: &Value, b: &Value) -> bool {
    match (a.as_f64(), b.as_f64()) {
        (Some(a), Some(b)) => a == b,
        _ => a == b,
    }
}
fn exact_created(result: &Value) -> bool {
    is_non_empty_string(&result["ref"], 256)
        && is_non_empty_string(&result["objectIdentity"], 256)
        && is_non_empty_string(&result["createdFingerprint"], 64)
}
/// Where an Arrangement clip row sits, in beats: its start and its right edge (Live's end time; start
/// plus length on a row without one).
fn arrangement_span(clip: &Value) -> Option<(f64, f64)> {
    let start = clip["start"].as_f64()?;
    Some((start, clip["endTime"].as_f64().filter(|end| *end > start).or_else(|| Some(start + clip["length"].as_f64()?))?))
}
/// The refusal for moving an Arrangement clip onto another clip of its track, which isn't done yet: Live
/// crashes when an Arrangement clip is copied onto a span a clip already holds, and a move is a copy.
fn arrangement_move_blocker(snapshot: &Value, moving: &Value, track: &Value, position: f64) -> Option<String> {
    let (start, end) = arrangement_span(moving)?;
    let target_end = position + (end - start);
    let clips = snapshot["arrangement"]["clips"].as_array().into_iter().flatten();
    clips.filter(|clip| clip["trackRef"] == track["ref"] && clip["ref"] != moving["ref"]).find_map(|clip| {
        let (other_start, other_end) = arrangement_span(clip)?;
        (other_start < target_end - 1e-6 && other_end > position + 1e-6).then(|| {
            let name: String = clip["name"].as_str().unwrap_or("").chars().take(60).collect();
            let beat = kumi_common::js::number::to_string;
            format!(
                "Kumi can't move a clip onto another clip yet: \"{name}\" (beats {} to {}) is in the way at beat {}; clear that span first (clear_range) or pick a free spot",
                beat(other_start),
                beat(other_end),
                beat(position)
            )
        })
    })
}

impl McpHost {
    pub async fn dispatch_clip_move_tool(&self, call: &ToolCall, signal: Option<&Signal>) -> Option<Result<Option<Value>, LiveError>> {
        let p = call.arguments.as_ref().unwrap_or(&Value::Null);
        Some(Ok(match call.name.as_str() {
            "live_clip_move_preview" => Some(self.live_clip_move_preview_async(&call.id, p).await),
            "live_clip_move_apply" => self.live_clip_move_apply_async(&call.id, p, signal).await,
            _ => return None,
        }))
    }
    pub async fn live_clip_move_preview_async(&self, id: &Value, params: &Value) -> Value {
        if !has_only(params, &["clipRef", "position", "targetTrackRef", "targetSceneIndex"])
            || !is_non_empty_string(&params["clipRef"], 256)
        {
            return error(id, -32602, "clipRef is required", None);
        }
        let result = async {
            let status = self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(reads::AUDITION_DEADLINE_MS)))).await?;
            if !status.connected || !status.capabilities.iter().any(|c| c.as_str() == "session.read") {
                return Err(LiveError::error("session read capability is unavailable"));
            }

            let snapshot = self.views.view_for(None, &[params["clipRef"].clone(), params["targetTrackRef"].clone()], None, &[]).await?;
            let reference = params["clipRef"].as_str().unwrap();
            let row = self.clip_row(&snapshot, reference)?;
            let mut payload = json!({});
            let fence;
            if row.arrangement {
                if !status.has_operation("arrangement.clip.move") {
                    return Err(LiveError::error("arrangement clip move is unavailable"));
                }
                if !params["position"].as_f64().is_some_and(|n| n.is_finite() && n >= 0.0) {
                    return Ok(error(id, -32602, "position is required for an Arrangement clip move", None));
                }
                if let (Some(track), None) = (&row.track, &row.take_lane) {
                    let value = serde_json::to_value(&snapshot).unwrap();
                    if let Some(reason) = arrangement_move_blocker(&value, &row.clip, track, params["position"].as_f64().unwrap()) {
                        return Err(LiveError::error(reason));
                    }
                }

                payload["ref"] = params["clipRef"].clone();
                payload["position"] = params["position"].clone();
                merge(&mut payload, &self.arrangement_clip_authority(&snapshot, reference)?);

                let fingerprint = capture_object_fingerprint(&row.clip)?;
                payload["expectedContentFingerprint"] = json!(fingerprint);
                if let Some(start) = row.clip.get("start") {
                    payload["priorPosition"] = start.clone();
                }
                fence = arrangement_move_fence(&params["clipRef"], &row.clip, &fingerprint);
            } else {
                if !status.has_operation("clip.move") {
                    return Err(LiveError::error("atomic Session clip move is unavailable"));
                }
                if !is_non_empty_string(&params["targetTrackRef"], 256) || !is_integer_in_range(&params["targetSceneIndex"], 0.0, 100000.0)
                {
                    return Ok(error(id, -32602, "targetTrackRef and targetSceneIndex are required for a Session slot move", None));
                }

                let value = serde_json::to_value(&snapshot).unwrap();
                let (track, target, scene) = target_rows(&value, &params["targetTrackRef"], &params["targetSceneIndex"]);
                if target.is_null() {
                    return Err(LiveError::error("target scene index is invalid"));
                }
                if arrangement::truthy(&target["clipRef"]) {
                    return Err(LiveError::error("target Session slot is occupied"));
                }
                let authority = self.clip_authority(&snapshot, reference)?;
                let fingerprint = capture_object_fingerprint(&row.clip)?;
                if !is_non_empty_string(&track["objectIdentity"], 256)
                    || !is_non_empty_string(&target["objectIdentity"], 256)
                    || scene.is_null()
                    || !is_non_empty_string(&scene["ref"], 256)
                    || !is_non_empty_string(&scene["objectIdentity"], 256)
                {
                    return Err(LiveError::error("Session move target identity is incomplete"));
                }

                let mut duplicate = json!({
                "ref":params["clipRef"],
                "targetTrackRef":params["targetTrackRef"],
                "targetSceneIndex":params["targetSceneIndex"],
                "arrangementPosition":null}
                );

                merge(&mut duplicate, &authority);
                merge(
                    &mut duplicate,
                    &json!({
                    "expectedContentFingerprint":fingerprint,
                    "expectedTargetTrackIdentity":track["objectIdentity"],
                    "expectedTargetSlotRef":target["ref"],
                    "expectedTargetSlotIdentity":target["objectIdentity"],
                    "expectedTargetSceneRef":scene["ref"],
                    "expectedTargetSceneIdentity":scene["objectIdentity"],
                    "expectedTargetCollectionRevision":null}
                    ),
                );

                payload["duplicate"] = duplicate;
                payload["deleteRef"] = params["clipRef"].clone();
                payload["deleteAuthority"] = authority.clone();
                if let Some(index) = row
                    .track
                    .as_ref()
                    .and_then(|t| t["clipSlots"].as_array())
                    .into_iter()
                    .flatten()
                    .find(|s| s["clipRef"] == params["clipRef"])
                    .and_then(|s| s.get("sceneIndex"))
                {
                    payload["sourceSceneIndex"] = index.clone();
                }

                if !is_integer_in_range(&payload["sourceSceneIndex"], 0.0, 100000.0) {
                    return Err(LiveError::error("Session move source scene identity is incomplete"));
                }

                fence = target_fence(&authority, &fingerprint, &track, &target, &scene);
            }
            let t = json!({
            "id":tempo::transaction_id("clipmove"),
            "epoch":status.epoch,
            "kind":"move",
            "fence":fence,
            "clipRef":params["clipRef"],
            "payload":payload,
            "expiresAt":kumi_common::time::now_ms_f64()+TRANSACTION_TTL_MS,
            "state":"previewed"}
            );

            self.retain_bounded_transaction(&self.clip_lifecycle_transactions, t.clone(), "clip move")?;
            Ok(success_text(
                id,
                &json!({
                "transactionId":t["id"],
                "epoch":t["epoch"],
                "clipRef":params["clipRef"],
                "payload":payload,
                "impact":"moves-clip",
                "confirmation":"apply",
                "expiresAt":t["expiresAt"]}
                ),
            ))
        }
        .await;
        result.unwrap_or_else(|e| adapter_tool_error(id, &e, "Clip-move preview requires fresh authoritative state."))
    }
    pub async fn live_clip_move_apply_async(&self, id: &Value, params: &Value, signal: Option<&Signal>) -> Option<Value> {
        if !valid_transaction_params(params, "apply") {
            return Some(error(id, -32602, "transactionId, confirmation=apply, and idempotencyKey are required", None));
        }
        let Some(record) = self.clip_lifecycle_transactions.get(params["transactionId"].as_str().unwrap()) else {
            return Some(transaction_error(id, "Unknown or expired clip-move transaction"));
        };
        let t = record.borrow().clone();
        if t["kind"] != "move"
            || (t["state"] == "previewed" && t["expiresAt"].as_f64().is_some_and(|n| n <= kumi_common::time::now_ms_f64()))
        {
            return Some(transaction_error(id, "Unknown or expired clip-move transaction"));
        }
        if t["state"] == "applied" && t["applyKey"] == params["idempotencyKey"] {
            return Some(success_text(id, &json!({"transactionId":t["id"],"state":"applied","created":t["created"],"idempotent":true})));
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
            let snapshot = if reconciliation {
                None
            } else {
                Some(
                    self.views
                        .view_for(Some(&context), &[t["clipRef"].clone(), payload["duplicate"]["targetTrackRef"].clone()], None, &[])
                        .await?,
                )
            };

            if payload.get("position").is_some() {
                let row = if let Some(s) = &snapshot { Some(self.clip_row(s, t["clipRef"].as_str().unwrap())?) } else { None };
                if let Some(row) = &row {
                    if !row.arrangement
                        || arrangement_move_fence(&t["clipRef"], &row.clip, &capture_object_fingerprint(&row.clip)?) != t["fence"]
                    {
                        return Ok(transaction_error(
                            id,
                            "Arrangement clip identity, position, or content changed since preview; preview again",
                        ));
                    }
                }

                {
                    let mut t = record.borrow_mut();
                    t["state"] = json!("applying");
                    t["applyKey"] = params["idempotencyKey"].clone();
                }
                let result = adapter
                    .invoke_async(
                        &LiveInvocation::new(
                            "arrangement.clip.move",
                            json!({
                            "ref":t["clipRef"],
                            "position":payload["position"],
                            "expectedObjectIdentity":payload["expectedObjectIdentity"],
                            "expectedAuthorityRevision":payload["expectedAuthorityRevision"],
                            "expectedContentFingerprint":payload["expectedContentFingerprint"]}
                            ),
                        ),
                        Some(&context),
                    )
                    .await?;

                if result.is_null() {
                    return Err(LiveError::type_error("Cannot read properties of null (reading 'ref')"));
                }
                if !exact_created(&result) {
                    return Err(LiveError::error("Arrangement clip move did not return exact created identity"));
                }
                let after = self
                    .views
                    .view_for(
                        Some(&context),
                        &[
                            result["ref"].clone(),
                            row.as_ref().and_then(|r| r.track.as_ref()).map(|r| r["ref"].clone()).unwrap_or(Value::Null),
                        ],
                        None,
                        &[],
                    )
                    .await?;

                let moved = self.clip_row(&after, result["ref"].as_str().unwrap())?;
                if !moved.arrangement
                    || moved.clip["objectIdentity"] != result["objectIdentity"]
                    || !number_equal(&moved.clip["start"], &payload["position"])
                    || capture_object_fingerprint(&moved.clip)? != result["createdFingerprint"]
                {
                    return Err(LiveError::error("Arrangement clip move result identity was not confirmed"));
                }

                let mut created = result.clone();
                created["fingerprint"] = result["createdFingerprint"].clone();
                record.borrow_mut()["created"] = created;
            } else {
                let duplicate = &payload["duplicate"];
                if let Some(snapshot) = &snapshot {
                    let authority = self.clip_authority(snapshot, t["clipRef"].as_str().unwrap())?;
                    let source = self.clip_row(snapshot, t["clipRef"].as_str().unwrap())?;
                    let (track, target, scene) =
                        target_rows(&serde_json::to_value(snapshot).unwrap(), &duplicate["targetTrackRef"], &duplicate["targetSceneIndex"]);

                    if track.is_null()
                        || target.is_null()
                        || scene.is_null()
                        || arrangement::truthy(&target["clipRef"])
                        || target_fence(&authority, &capture_object_fingerprint(&source.clip)?, &track, &target, &scene) != t["fence"]
                    {
                        return Ok(transaction_error(
                            id,
                            "source clip content or target Session identity changed since preview; preview again",
                        ));
                    }
                }
                {
                    let mut t = record.borrow_mut();
                    t["state"] = json!("applying");
                    t["applyKey"] = params["idempotencyKey"].clone();
                }
                let moved = adapter.invoke_async(&LiveInvocation::new("clip.move", duplicate.clone()), Some(&context)).await?;
                if !moved["ref"].is_string()
                    || !is_non_empty_string(&moved["objectIdentity"], 256)
                    || !is_non_empty_string(&moved["createdFingerprint"], 64)
                {
                    return Err(LiveError::error("Session clip move did not return exact destination identity"));
                }

                let after = self
                    .views
                    .view_for(
                        Some(&context),
                        &[
                            moved["ref"].clone(),
                            duplicate["targetTrackRef"].clone(),
                            payload["deleteAuthority"]["expectedTrackRef"].clone(),
                        ],
                        None,
                        &[],
                    )
                    .await?;

                if self.clip_row(&after, t["clipRef"].as_str().unwrap()).is_ok() {
                    return Err(LiveError::error("Session clip move source still exists"));
                }

                let destination = self.clip_row(&after, moved["ref"].as_str().unwrap())?;
                if destination.clip["objectIdentity"] != moved["objectIdentity"]
                    || capture_object_fingerprint(&destination.clip)? != moved["createdFingerprint"]
                {
                    return Err(LiveError::error("Session clip move destination identity or creation fingerprint changed"));
                }

                record.borrow_mut()["created"] = json!({
                "ref":moved["ref"],
                "objectIdentity":moved["objectIdentity"],
                "fingerprint":moved["createdFingerprint"],
                "deleted":t["clipRef"]}
                );
            }
            {
                let mut t = record.borrow_mut();
                t["applyKey"] = params["idempotencyKey"].clone();
                t["state"] = json!("applied");
            }
            Ok(success_text(
                id,
                &json!({"transactionId":t["id"],"state":"applied","created":record.borrow()["created"],"idempotent":false}),
            ))
        }
        .await;
        Some(result.unwrap_or_else(|e| {
            record.borrow_mut()["state"] = json!("uncertain");
            adapter_tool_error(id, &e, "Clip move is uncertain; perform fresh discovery before retrying.")
        }))
    }
    pub async fn undo_clip_move_async(&self, id: &Value, params: &Value, signal: Option<&Signal>) -> Value {
        let Some(record) = params["transactionId"]
            .as_str()
            .and_then(|id| self.clip_lifecycle_transactions.get(id))
            .filter(|r| r.borrow()["kind"] == "move")
        else {
            return transaction_error(id, "Unknown clip-move transaction");
        };
        let t = record.borrow().clone();
        if t["state"] == "undone" && t["undoKey"] == params["idempotencyKey"] {
            return success_text(id, &json!({"transactionId":t["id"],"state":"undone","idempotent":true}));
        }
        let reconciliation = t["state"] == "uncertain" && t["undoKey"] == params["idempotencyKey"];
        if (t["state"] != "applied" && !reconciliation)
            || !is_non_empty_string(&t["created"]["ref"], 256)
            || !is_non_empty_string(&t["created"]["objectIdentity"], 256)
            || !is_non_empty_string(&t["created"]["fingerprint"], 64)
        {
            return transaction_error(id, "Clip move lacks exact applied identity and content fingerprint");
        }
        let result = async {
            let (_, steps) = self.begin_undo_recovery(&record, params["idempotencyKey"].as_str().unwrap())?;
            let status = self.require_connected(Some("session.read"))?;
            if json!(status.epoch) != t["epoch"] {
                return Ok(transaction_error(id, "Live connection epoch changed; undo refused"));
            }
            let adapter = self.async_adapter();
            let context = self.transaction_context(params, signal, reads::AUDITION_DEADLINE_MS);
            {
                let mut row = record.borrow_mut();
                row["undoKey"] = params["idempotencyKey"].clone();
                if row["payload"]["appliedRef"].is_null() {
                    row["payload"]["appliedRef"] = t["created"]["ref"].clone();
                }
            }

            if reconciliation {
                self.replay_undo_recovery(&record, adapter.as_ref(), &context).await?;
            }
            record.borrow_mut()["state"] = json!("undoing");
            let payload = record.borrow()["payload"].clone();
            if payload.get("position").is_some() {
                let result = if reconciliation {
                    steps
                        .last()
                        .map(|r| r.borrow()["result"].clone())
                        .filter(Value::is_object)
                        .ok_or_else(|| LiveError::error("Arrangement clip move replay result is unavailable"))?
                } else {
                    let snapshot = self.views.view_for(Some(&context), &[t["created"]["ref"].clone()], None, &[]).await?;
                    let current = self.clip_row(&snapshot, t["created"]["ref"].as_str().unwrap())?;
                    if !current.arrangement
                        || current.clip["objectIdentity"] != t["created"]["objectIdentity"]
                        || !number_equal(&current.clip["start"], &payload["position"])
                        || capture_object_fingerprint(&current.clip)? != t["created"]["fingerprint"]
                    {
                        return Err(LiveError::error("Arrangement clip identity, position, or content changed after apply; undo refused"));
                    }

                    let mut args = json!({
                    "ref":t["created"]["ref"],
                    "position":payload["priorPosition"]}
                    );
                    merge(&mut args, &self.arrangement_clip_authority(&snapshot, t["created"]["ref"].as_str().unwrap())?);
                    args["expectedContentFingerprint"] = t["created"]["fingerprint"].clone();

                    self.invoke_undo_recovery(&record, adapter.as_ref(), "arrangement.clip.move", &args, &context).await?
                };
                if result.is_null() {
                    return Err(LiveError::type_error("Cannot read properties of null (reading 'ref')"));
                }
                if !is_non_empty_string(&result["ref"], 256)
                    || !is_non_empty_string(&result["objectIdentity"], 256)
                    || !number_equal(&result["start"], &payload["priorPosition"])
                {
                    return Err(LiveError::error("Arrangement clip move restoration was not confirmed"));
                }

                let snapshot =
                    self.views.view_for(Some(&context), &[result["ref"].clone(), t["created"]["ref"].clone()], None, &[]).await?;
                let restored = self.clip_row(&snapshot, result["ref"].as_str().unwrap())?;
                if restored.clip["objectIdentity"] != result["objectIdentity"]
                    || !number_equal(&restored.clip["start"], &payload["priorPosition"])
                {
                    return Err(LiveError::error("Arrangement clip move prior location was not verified"));
                }

                record.borrow_mut()["created"] = result;
            } else {
                let restored = if reconciliation {
                    steps
                        .last()
                        .map(|r| r.borrow()["result"].clone())
                        .filter(Value::is_object)
                        .ok_or_else(|| LiveError::error("Session clip move replay result is unavailable"))?
                } else {
                    let snapshot = self
                        .views
                        .view_for(
                            Some(&context),
                            &[t["created"]["ref"].clone(), payload["deleteAuthority"]["expectedTrackRef"].clone()],
                            None,
                            &[],
                        )
                        .await?;

                    let current = self.clip_row(&snapshot, t["created"]["ref"].as_str().unwrap())?;
                    if current.clip["objectIdentity"] != t["created"]["objectIdentity"]
                        || capture_object_fingerprint(&current.clip)? != t["created"]["fingerprint"]
                    {
                        return Err(LiveError::error("Session clip identity or content changed after apply; undo refused"));
                    }

                    let authority = self.clip_authority(&snapshot, t["created"]["ref"].as_str().unwrap())?;
                    let original = &payload["deleteAuthority"];
                    let mut args = json!({
                    "ref":t["created"]["ref"],
                    "targetTrackRef":original["expectedTrackRef"],
                    "targetSceneIndex":payload["sourceSceneIndex"],
                    "arrangementPosition":null}
                    );

                    merge(&mut args, &authority);
                    merge(
                        &mut args,
                        &json!({
                        "expectedContentFingerprint":t["created"]["fingerprint"],
                        "expectedTargetTrackIdentity":original["expectedTrackIdentity"],
                        "expectedTargetSlotRef":original["expectedSlotRef"],
                        "expectedTargetSlotIdentity":original["expectedSlotIdentity"],
                        "expectedTargetSceneRef":original["expectedSceneRef"],
                        "expectedTargetSceneIdentity":original["expectedSceneIdentity"],
                        "expectedTargetCollectionRevision":null}
                        ),
                    );

                    self.invoke_undo_recovery(&record, adapter.as_ref(), "clip.move", &args, &context).await?
                };
                if restored.is_null() {
                    return Err(LiveError::type_error("Cannot read properties of null (reading 'ref')"));
                }
                if !is_non_empty_string(&restored["ref"], 256) || !is_non_empty_string(&restored["objectIdentity"], 256) {
                    return Err(LiveError::error("Session clip move restoration did not return exact identity"));
                }

                let snapshot = self
                    .views
                    .view_for(
                        Some(&context),
                        &[restored["ref"].clone(), payload["appliedRef"].clone(), payload["deleteAuthority"]["expectedTrackRef"].clone()],
                        None,
                        &[],
                    )
                    .await?;

                let row = self.clip_row(&snapshot, restored["ref"].as_str().unwrap())?;
                if row.clip["objectIdentity"] != restored["objectIdentity"] {
                    return Err(LiveError::error("Session clip restoration identity changed"));
                }
                if self.clip_row(&snapshot, payload["appliedRef"].as_str().unwrap()).is_ok() {
                    return Err(LiveError::error("moved destination remains after restoration"));
                }

                record.borrow_mut()["created"] = restored;
            }
            record.borrow_mut()["state"] = json!("undone");
            Ok(success_text(
                id,
                &json!({"transactionId":t["id"],"state":"undone","restored":record.borrow()["created"],"idempotent":false}),
            ))
        }
        .await;
        result.unwrap_or_else(|e| {
            record.borrow_mut()["state"] = json!("uncertain");
            adapter_tool_error(id, &e, "Clip-move undo is uncertain; inspect both source and destination slots.")
        })
    }
}
