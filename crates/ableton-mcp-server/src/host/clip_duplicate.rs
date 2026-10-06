//! Session and Arrangement clip duplication with exact created-object cleanup.
use super::*;
use arrangement::capture_object_fingerprint;
use kumi_common::{
    abort::{Signal, SignalExt},
    js::json as js_json,
};
use sha2::{Digest, Sha256};
fn rows(value: &Value) -> &[Value] {
    value.as_array().map(Vec::as_slice).unwrap_or(&[])
}
fn string_field(value: &Value, name: &str) -> Result<String, LiveError> {
    value.get(name).map(helpers::js_string).unwrap_or_else(|| Ok("undefined".into()))
}
pub(super) fn target_rows(snapshot: &Value, track: &Value, index: &Value) -> (Value, Value, Value) {
    let track = rows(&snapshot["tracks"]).iter().find(|t| t["ref"] == *track).cloned().unwrap_or(Value::Null);
    let slot = rows(&track["clipSlots"]).iter().find(|s| s["sceneIndex"].as_f64() == index.as_f64()).cloned().unwrap_or(Value::Null);
    let scene = rows(&snapshot["scenes"]).iter().find(|s| s["index"].as_f64() == index.as_f64()).cloned().unwrap_or(Value::Null);
    (track, slot, scene)
}

pub(super) fn target_fence(authority: &Value, fingerprint: &str, track: &Value, target: &Value, scene: &Value) -> String {
    js_json::stringify(&json!({
    "sourceAuthority":authority,
    "sourceFingerprint":fingerprint,
    "target":target["ref"],
    "targetIdentity":target["objectIdentity"],
    "targetTrackIdentity":track["objectIdentity"],
    "targetSceneIdentity":scene["objectIdentity"],
    "empty":target["empty"]}
    ))
}

impl McpHost {
    pub(super) fn arrangement_fence(&self, snapshot: &LiveSnapshot, track_refs: &[Value]) -> Result<String, LiveError> {
        let s = serde_json::to_value(snapshot).unwrap();
        let tracks: HashSet<_> = track_refs.iter().filter_map(Value::as_str).collect();
        let mut clips = vec![];
        for clip in rows(&s["arrangement"]["clips"]) {
            if !clip.is_object() || !tracks.contains(string_field(clip, "trackRef")?.as_str()) {
                continue;
            }
            let fields = ["ref", "objectIdentity", "trackRef", "name", "start", "length"]
                .iter()
                .map(|field| string_field(clip, field))
                .collect::<Result<Vec<_>, _>>()?;
            clips.push(format!("{}:{}", fields.join(":"), kumi_common::js::number::to_string(arrangement_clip_end(clip))));
        }
        Ok(js_json::stringify(&json!(clips)))
    }

    pub(super) fn notes_revision(notes: &[Value]) -> Result<String, LiveError> {
        Ok(hex::encode(Sha256::digest(canonical_mutation_identity(&json!(notes))?)))
    }
    pub async fn dispatch_clip_duplicate_tool(&self, call: &ToolCall, signal: Option<&Signal>) -> Option<Result<Option<Value>, LiveError>> {
        let p = call.arguments.as_ref().unwrap_or(&Value::Null);
        Some(Ok(match call.name.as_str() {
            "live_clip_duplicate_preview" => Some(self.live_clip_duplicate_preview_async(&call.id, p).await),
            "live_clip_duplicate_apply" => self.live_clip_duplicate_apply_async(&call.id, p, signal).await,
            _ => return None,
        }))
    }

    pub async fn live_clip_duplicate_preview_async(&self, id: &Value, params: &Value) -> Value {
        if !has_only(params, &["clipRef", "targetTrackRef", "targetSceneIndex", "arrangementPosition"])
            || !is_non_empty_string(&params["clipRef"], 256)
        {
            return error(id, -32602, "clipRef is required", None);
        }
        let to_arrangement = params.get("arrangementPosition").is_some();
        if to_arrangement && !params["arrangementPosition"].as_f64().is_some_and(|n| n.is_finite() && n >= 0.0) {
            return error(id, -32602, "arrangementPosition is out of bounds", None);
        }
        if !to_arrangement
            && (!is_non_empty_string(&params["targetTrackRef"], 256) || !is_integer_in_range(&params["targetSceneIndex"], 0.0, 100000.0))
        {
            return error(id, -32602, "targetTrackRef and targetSceneIndex are required for Session duplication", None);
        }

        let result = async {
            let status = self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(reads::AUDITION_DEADLINE_MS)))).await?;
            if !status.connected || !status.capabilities.iter().any(|c| c.as_str() == "session.read") {
                return Err(LiveError::error("session read capability is unavailable"));
            }
            if !status.has_operation("clip.duplicate") {
                return Err(LiveError::error("clip duplication is unavailable"));
            }
            let reference = params["clipRef"].as_str().unwrap();
            let snapshot = self.views.view_for(None, &[params["clipRef"].clone(), params["targetTrackRef"].clone()], None, &[]).await?;
            let source = self.clip_row(&snapshot, reference)?;

            if source.arrangement && to_arrangement {
                return Err(LiveError::error("arrangement clips cannot duplicate to the Arrangement"));
            }
            let authority = self.clip_authority(&snapshot, reference)?;
            let fingerprint = capture_object_fingerprint(&source.clip)?;
            if source.arrangement {
                return Err(LiveError::error("clip duplication requires an authoritative Session source clip"));
            }

            let mut payload = json!({
            "ref":params["clipRef"],
            "targetTrackRef":null,
            "targetSceneIndex":null,
            "arrangementPosition":null}
            );
            for (k, v) in authority.as_object().unwrap() {
                payload[k] = v.clone();
            }
            payload["expectedContentFingerprint"] = json!(fingerprint);
            for field in [
                "expectedTargetTrackIdentity",
                "expectedTargetSlotRef",
                "expectedTargetSlotIdentity",
                "expectedTargetSceneRef",
                "expectedTargetSceneIdentity",
                "expectedTargetCollectionRevision",
            ] {
                payload[field] = Value::Null;
            }

            let fence;
            if to_arrangement {
                payload["arrangementPosition"] = params["arrangementPosition"].clone();
                let track = &source.track.as_ref().unwrap()["ref"];
                // Live cuts what a copy lands on, and a copy that splits or covers a clip fails after the cut, so
                // nothing is copied over another clip yet.
                let start = params["arrangementPosition"].as_f64().unwrap_or(f64::NAN);
                let end = start + source.clip["length"].as_f64().unwrap_or(f64::NAN);
                let value = serde_json::to_value(&snapshot).unwrap();
                if let Some(other) = value["arrangement"]["clips"].as_array().into_iter().flatten().find(|clip| {
                    clip["trackRef"] == *track
                        && clip["start"].as_f64().is_some_and(|other_start| other_start < end - 1e-6)
                        && helpers::arrangement_clip_end(clip) > start + 1e-6
                }) {
                    let beat = |v: f64| kumi_common::js::number::to_string(v);
                    return Err(LiveError::error(format!(
                        "Kumi can't place a copy over other clips yet: \u{201c}{}\u{201d} is at beats {} to {}. Clear that span first or pick a free spot",
                        other["name"].as_str().unwrap_or(""),
                        beat(other["start"].as_f64().unwrap_or(f64::NAN)),
                        beat(helpers::arrangement_clip_end(other))
                    )));
                }
                payload["expectedTargetCollectionRevision"] =
                    json!(self.arrangement_collection_revision(&snapshot, track.as_str().unwrap())?);
                fence = js_json::stringify(&json!({
                "arrangement":self.arrangement_fence(&snapshot,
                &[track.clone()])?,
                "sourceAuthority":authority,
                "sourceFingerprint":fingerprint}
                ));
            } else {
                let snapshot = serde_json::to_value(&snapshot).unwrap();
                let (track, target, scene) = target_rows(&snapshot, &params["targetTrackRef"], &params["targetSceneIndex"]);
                if track.is_null() || !is_non_empty_string(&track["objectIdentity"], 256) {
                    return Err(LiveError::error("target track identity is not authoritative"));
                }
                if target.is_null()
                    || scene.is_null()
                    || ![&target["ref"], &target["objectIdentity"], &scene["ref"], &scene["objectIdentity"]]
                        .iter()
                        .all(|v| is_non_empty_string(v, 256))
                {
                    return Err(LiveError::error("target slot or scene identity is invalid"));
                }
                if arrangement::truthy(&target["clipRef"]) {
                    return Err(LiveError::error("target Session slot is occupied"));
                }
                payload["targetTrackRef"] = params["targetTrackRef"].clone();
                payload["targetSceneIndex"] = params["targetSceneIndex"].clone();
                payload["expectedTargetTrackIdentity"] = track["objectIdentity"].clone();
                payload["expectedTargetSlotRef"] = target["ref"].clone();
                payload["expectedTargetSlotIdentity"] = target["objectIdentity"].clone();
                payload["expectedTargetSceneRef"] = scene["ref"].clone();
                payload["expectedTargetSceneIdentity"] = scene["objectIdentity"].clone();
                fence = target_fence(&authority, &fingerprint, &track, &target, &scene);
            }

            let t = json!({
            "id":tempo::transaction_id("clipdup"),
            "epoch":status.epoch,
            "kind":"duplicate",
            "fence":fence,
            "clipRef":params["clipRef"],
            "payload":payload,
            "expiresAt":kumi_common::time::now_ms_f64()+TRANSACTION_TTL_MS,
            "state":"previewed"}
            );
            self.retain_bounded_transaction(&self.clip_lifecycle_transactions, t.clone(), "clip duplicate")?;
            let destination = if to_arrangement {
                json!({
                "arrangementPosition":payload["arrangementPosition"]}
                )
            } else {
                json!({
                "trackRef":payload["targetTrackRef"],
                "sceneIndex":payload["targetSceneIndex"]}
                )
            };
            Ok(success_text(
                id,
                &json!({
                "transactionId":t["id"],
                "epoch":t["epoch"],
                "source":params["clipRef"],
                "destination":destination,
                "impact":"duplicates-clip",
                "confirmation":"apply",
                "expiresAt":t["expiresAt"]}
                ),
            ))
        }
        .await;
        result.unwrap_or_else(|e| adapter_tool_error(id, &e, "Clip-duplicate preview requires fresh authoritative state."))
    }
    pub async fn live_clip_duplicate_apply_async(&self, id: &Value, params: &Value, signal: Option<&Signal>) -> Option<Value> {
        if !valid_transaction_params(params, "apply") {
            return Some(error(id, -32602, "transactionId, confirmation=apply, and idempotencyKey are required", None));
        }
        let Some(record) = self.clip_lifecycle_transactions.get(params["transactionId"].as_str().unwrap()) else {
            return Some(transaction_error(id, "Unknown or expired clip-duplicate transaction"));
        };
        let t = record.borrow().clone();
        if t["kind"] != "duplicate"
            || (t["state"] == "previewed" && t["expiresAt"].as_f64().is_some_and(|n| n <= kumi_common::time::now_ms_f64()))
        {
            return Some(transaction_error(id, "Unknown or expired clip-duplicate transaction"));
        }
        if t["state"] == "applied" && t["applyKey"] == params["idempotencyKey"] {
            return Some(success_text(
                id,
                &json!({
                "transactionId":t["id"],
                "state":"applied",
                "created":t["created"],
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
            let snapshot =
                self.views.view_for(Some(&context), &[t["clipRef"].clone(), payload["targetTrackRef"].clone()], None, &[]).await?;

            if !reconciliation && !payload["arrangementPosition"].is_null() {
                let source = self.clip_row(&snapshot, reference)?;
                let track = source.track.as_ref().map(|t| t["ref"].clone()).unwrap_or(Value::Null);
                if js_json::stringify(&json!({
                "arrangement":self.arrangement_fence(&snapshot,
                &[track])?,
                "sourceAuthority":self.clip_authority(&snapshot,
                reference)?,
                "sourceFingerprint":capture_object_fingerprint(&source.clip)?}
                )) != t["fence"]
                {
                    return Ok(transaction_error(
                        id,
                        "Arrangement or source clip identity or content changed since preview; preview again",
                    ));
                }
            } else if !reconciliation {
                let authority = self.clip_authority(&snapshot, reference)?;
                let source = self.clip_row(&snapshot, reference)?;
                let snapshot = serde_json::to_value(snapshot).unwrap();
                let (track, target, scene) = target_rows(&snapshot, &payload["targetTrackRef"], &payload["targetSceneIndex"]);
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
                let mut row = record.borrow_mut();
                row["state"] = json!("applying");
                row["applyKey"] = params["idempotencyKey"].clone();
            }
            let created = adapter.invoke_async(&LiveInvocation::new("clip.duplicate", payload.clone()), Some(&context)).await?;
            if !created["ref"].is_string()
                || !is_non_empty_string(&created["objectIdentity"], 256)
                || !is_non_empty_string(&created["createdFingerprint"], 64)
            {
                return Err(LiveError::error("clip duplication did not return exact created identity"));
            }
            let created_clip = self.clip_row(
                &self
                    .views
                    .view_for(Some(&context), &[created["ref"].clone(), t["clipRef"].clone(), payload["targetTrackRef"].clone()], None, &[])
                    .await?,
                created["ref"].as_str().unwrap(),
            )?;
            if created_clip.clip["objectIdentity"] != created["objectIdentity"]
                || capture_object_fingerprint(&created_clip.clip)? != created["createdFingerprint"]
            {
                return Err(LiveError::error("duplicated clip identity or creation fingerprint was not confirmed"));
            }

            let notes_revision = if created_clip.arrangement && created_clip.clip["kind"] == "midi" {
                Some(Self::notes_revision(&self.clip_notes_async(created["ref"].as_str().unwrap(), Some(&context)).await?)?)
            } else {
                None
            };
            let mut created = created;
            created["fingerprint"] = created["createdFingerprint"].clone();
            if let Some(revision) = notes_revision {
                created["notesRevision"] = json!(revision);
            }
            {
                let mut row = record.borrow_mut();
                row["created"] = created.clone();
                row["applyKey"] = params["idempotencyKey"].clone();
                row["state"] = json!("applied");
            }
            Ok(success_text(
                id,
                &json!({
                "transactionId":t["id"],
                "state":"applied",
                "created":created,
                "idempotent":false}
                ),
            ))
        }
        .await;
        Some(result.unwrap_or_else(|e| {
            record.borrow_mut()["state"] = json!("uncertain");
            adapter_tool_error(id, &e, "Clip duplication is uncertain; perform fresh discovery before retrying.")
        }))
    }
    pub async fn undo_clip_duplicate_async(&self, id: &Value, params: &Value, signal: Option<&Signal>) -> Value {
        let Some(record) = params["transactionId"]
            .as_str()
            .and_then(|id| self.clip_lifecycle_transactions.get(id))
            .filter(|r| r.borrow()["kind"] == "duplicate")
        else {
            return transaction_error(id, "Unknown clip-duplicate transaction");
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
        if (t["state"] != "applied" && !reconciliation)
            || !is_non_empty_string(&t["created"]["ref"], 256)
            || !is_non_empty_string(&t["created"]["objectIdentity"], 256)
        {
            return transaction_error(id, "Only an applied or exact-key uncertain identity-bound clip duplicate can be undone");
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
            record.borrow_mut()["state"] = json!("undoing");
            self.delete_owned_clip_async(
                adapter.as_ref(),
                t["created"]["ref"].as_str().unwrap(),
                t["created"]["objectIdentity"].as_str().unwrap(),
                &context,
                t["created"]["fingerprint"].as_str(),
                Some(&record),
                reconciliation,
                t["created"]["notesRevision"].as_str(),
            )
            .await?;
            record.borrow_mut()["state"] = json!("undone");
            Ok(success_text(
                id,
                &json!({
                "transactionId":t["id"],
                "state":"undone",
                "deleted":t["created"]["ref"],
                "idempotent":false}
                ),
            ))
        }
        .await;
        result.unwrap_or_else(|e| {
            record.borrow_mut()["state"] = json!("uncertain");
            adapter_tool_error(id, &e, "Clip-duplicate undo is uncertain; inspect the exact destination.")
        })
    }
}
