//! Arrangement and take-lane creation retain exact identities for available undo.
use super::*;
use arrangement::capture_object_fingerprint;
use kumi_common::{
    abort::{Signal, SignalExt},
    js::json as js_json,
};
use sha2::{Digest, Sha256};
fn nonnegative(value: &Value) -> bool {
    value.as_f64().is_some_and(|n| n.is_finite() && n >= 0.0)
}
fn positive(value: &Value) -> bool {
    value.as_f64().is_some_and(|n| n.is_finite() && n > 0.0)
}
/// A track's take lanes as Live's Remote Script hashes them for a lane create: each one's ref, identity and name.
fn track_lanes(track: &Value) -> Value {
    json!(track["takeLanes"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|lane| json!({"ref":lane["ref"],"objectIdentity":lane["objectIdentity"],"name":lane["name"].as_str().unwrap_or("")}))
        .collect::<Vec<_>>())
}
fn lane_siblings(lane: &Value) -> Value {
    json!(lane["clips"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|clip| {
            let mut row = json!({});
            for field in ["ref", "objectIdentity"] {
                if let Some(value) = clip.get(field) {
                    row[field] = value.clone();
                }
            }
            row
        })
        .collect::<Vec<_>>())
}
impl McpHost {
    pub async fn dispatch_arrangement_clip_tool(
        &self,
        call: &ToolCall,
        signal: Option<&Signal>,
    ) -> Option<Result<Option<Value>, LiveError>> {
        let p = call.arguments.as_ref().unwrap_or(&Value::Null);
        Some(Ok(match call.name.as_str() {
            "live_arrangement_clip_preview" => Some(self.live_arrangement_clip_preview_async(&call.id, p).await),
            "live_arrangement_clip_apply" => self.live_arrangement_clip_apply_async(&call.id, p, signal).await,
            _ => return None,
        }))
    }

    pub async fn live_arrangement_clip_preview_async(&self, id: &Value, params: &Value) -> Value {
        if params.is_object() && params["action"] == "create-lane" {
            return self.take_lane_create_preview_async(id, params).await;
        }
        if params.is_object() && params["action"] == "delete" {
            return transaction_error(
                id,
                "Arbitrary Arrangement clip deletion is unavailable; use live_undo only for an exact transaction-created clip",
            );
        }
        if !has_only(params, &["action", "kind", "trackRef", "position", "length", "name", "filePath", "clipRef", "takeLaneRef"])
            || params["action"] != "create"
        {
            return error(id, -32602, "action=create is required; arbitrary Arrangement deletion is unavailable", None);
        }
        let create_kind = params.get("kind").filter(|v| !v.is_null()).cloned().unwrap_or(json!("midi"));
        if create_kind != "midi" && create_kind != "audio" {
            return error(id, -32602, "kind must be midi or audio", None);
        }
        let take_lane = params.get("takeLaneRef").is_some();
        if take_lane && (create_kind != "midi" || !is_non_empty_string(&params["takeLaneRef"], 256)) {
            return error(id, -32602, "takeLaneRef requires kind=midi", None);
        }

        let result = async {
            let status = self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(reads::AUDITION_DEADLINE_MS)))).await?;
            if !status.connected || !status.capabilities.iter().any(|c| c.as_str() == "session.read") {
                return Err(LiveError::error("session read capability is unavailable"));
            }
            let operation = if take_lane {
                "take-lane.clip.create"
            } else if create_kind == "audio" {
                "arrangement.audio-clip.create"
            } else {
                "arrangement.clip.create"
            };
            if !status.has_operation(operation) {
                return Err(LiveError::error(format!("{operation} is unavailable")));
            }

            let snapshot = self.views.view_for(None, &[params["trackRef"].clone(), params["takeLaneRef"].clone()], None, &[]).await?;
            let fence = self.arrangement_fence(&snapshot, &[params["trackRef"].clone()])?;
            if take_lane {
                if !nonnegative(&params["position"]) || !positive(&params["length"]) || !is_non_empty_string(&params["name"], 256) {
                    return Ok(error(id, -32602, "position, length, and name are required for a take-lane clip create", None));
                }
                let (_, lane) = self.take_lane_row(&snapshot, params["takeLaneRef"].as_str().unwrap())?;
                if !is_non_empty_string(&lane["objectIdentity"], 256) {
                    return Err(LiveError::error("take-lane identity is not authoritative"));
                }
                let siblings = lane_siblings(&lane);
                if siblings.as_array().unwrap().iter().any(|r| r.get("objectIdentity").is_none()) {
                    return Err(LiveError::error("mutation authority contains an unsupported value"));
                }
                let revision = hex::encode(Sha256::digest(canonical_mutation_identity(&siblings)?));
                let payload = json!({
                "takeLaneRef":params["takeLaneRef"],
                "position":params["position"],
                "length":params["length"],
                "name":params["name"],
                "expectedTakeLaneIdentity":lane["objectIdentity"],
                "expectedCollectionRevision":revision}
                );
                let fence = js_json::stringify(&json!({
                "takeLaneRef":params["takeLaneRef"],
                "laneIdentity":lane["objectIdentity"],
                "siblings":siblings}
                ));
                let t = json!({
                "id":tempo::transaction_id("arrclip"),
                "epoch":status.epoch,
                "kind":"arrangement-take-lane-create",
                "fence":fence,
                "payload":payload,
                "expiresAt":kumi_common::time::now_ms_f64()+TRANSACTION_TTL_MS,
                "state":"previewed"}
                );
                self.retain_bounded_transaction(&self.clip_lifecycle_transactions, t.clone(), "arrangement clip")?;
                return Ok(success_text(
                    id,
                    &json!({
                    "transactionId":t["id"],
                    "epoch":t["epoch"],
                    "action":"create",
                    "kind":"take-lane",
                    "payload":payload,
                    "impact":"creates-take-lane-clip-no-undo",
                    "confirmation":"apply",
                    "expiresAt":t["expiresAt"]}
                    ),
                ));
            }
            if !is_non_empty_string(&params["trackRef"], 256) || !nonnegative(&params["position"]) {
                return Ok(error(id, -32602, "trackRef and position are required for create", None));
            }
            if create_kind == "midi" && (!positive(&params["length"]) || !is_non_empty_string(&params["name"], 256)) {
                return Ok(error(id, -32602, "length and name are required for a MIDI clip create", None));
            }
            if create_kind == "audio" && !is_non_empty_string(&params["filePath"], 1024) {
                return Ok(error(id, -32602, "filePath is required for an Arrangement audio import", None));
            }
            if params.get("name").is_some_and(|v| !is_non_empty_string(v, 256)) {
                return Ok(error(id, -32602, "name is invalid", None));
            }

            let value = serde_json::to_value(&snapshot).unwrap();
            let track = value["tracks"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|r| r["ref"] == params["trackRef"])
                .filter(|r| is_non_empty_string(&r["objectIdentity"], 256))
                .ok_or_else(|| LiveError::error("track identity is not authoritative"))?;
            let mut payload = json!({
            "trackRef":params["trackRef"]}
            );
            if create_kind == "audio" {
                payload["filePath"] = params["filePath"].clone();
            }
            payload["position"] = params["position"].clone();
            if create_kind == "midi" {
                payload["length"] = params["length"].clone();
            }
            if let Some(name) = params.get("name") {
                payload["name"] = name.clone();
            }
            payload["expectedTrackIdentity"] = track["objectIdentity"].clone();
            payload["expectedCollectionRevision"] =
                json!(self.arrangement_collection_revision(&snapshot, params["trackRef"].as_str().unwrap())?);

            let t = json!({
            "id":tempo::transaction_id("arrclip"),
            "epoch":status.epoch,
            "kind":if create_kind=="audio"{
            "arrangement-audio-create"}
            else{
            "arrangement-create"}
            ,
            "fence":fence,
            "payload":payload,
            "expiresAt":kumi_common::time::now_ms_f64()+TRANSACTION_TTL_MS,
            "state":"previewed"}
            );
            self.retain_bounded_transaction(&self.clip_lifecycle_transactions, t.clone(), "arrangement clip")?;
            Ok(success_text(
                id,
                &json!({
                "transactionId":t["id"],
                "epoch":t["epoch"],
                "action":"create",
                "kind":create_kind,
                "payload":payload,
                "impact":if create_kind=="audio"{
                "creates-arrangement-audio-clip"}
                else{
                "creates-arrangement-clip"}
                ,
                "confirmation":"apply",
                "expiresAt":t["expiresAt"]}
                ),
            ))
        }
        .await;
        result.unwrap_or_else(|e| adapter_tool_error(id, &e, "Arrangement-clip preview requires fresh authoritative state."))
    }
    /// A new take lane on a track (Live 12's comping lanes): read-only preview, fenced on the track and its lanes.
    async fn take_lane_create_preview_async(&self, id: &Value, params: &Value) -> Value {
        if !has_only(params, &["action", "trackRef", "name"])
            || !is_non_empty_string(&params["trackRef"], 256)
            || params.get("name").is_some_and(|name| !is_non_empty_string(name, 256))
        {
            return error(id, -32602, "trackRef is required for a take lane, and its name, if given, is 1 to 256 characters", None);
        }
        let result = async {
            let status = self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(reads::AUDITION_DEADLINE_MS)))).await?;
            if !status.connected || !status.capabilities.iter().any(|c| c.as_str() == "session.read") {
                return Err(LiveError::error("session read capability is unavailable"));
            }
            if !status.has_operation("take-lane.create") {
                return Err(LiveError::error("take-lane.create is unavailable"));
            }
            let snapshot = self.views.view_for(None, &[params["trackRef"].clone()], None, &[]).await?;
            let value = serde_json::to_value(&snapshot).unwrap();
            let track = value["tracks"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|r| r["ref"] == params["trackRef"])
                .filter(|r| is_non_empty_string(&r["objectIdentity"], 256))
                .ok_or_else(|| LiveError::error("track identity is not authoritative"))?;
            let lanes = track_lanes(track);
            let mut payload = json!({
                "trackRef":params["trackRef"],
                "expectedTrackIdentity":track["objectIdentity"],
                "expectedTakeLaneCollectionRevision":hex::encode(Sha256::digest(canonical_mutation_identity(&lanes)?))
            });
            if let Some(name) = params.get("name") {
                payload["name"] = name.clone();
            }
            let fence = js_json::stringify(&json!({"trackRef":params["trackRef"],"trackIdentity":track["objectIdentity"],"lanes":lanes}));
            let t = json!({
                "id":tempo::transaction_id("arrclip"),
                "epoch":status.epoch,
                "kind":"take-lane-create",
                "fence":fence,
                "payload":payload,
                "expiresAt":kumi_common::time::now_ms_f64()+TRANSACTION_TTL_MS,
                "state":"previewed"
            });
            self.retain_bounded_transaction(&self.clip_lifecycle_transactions, t.clone(), "arrangement clip")?;
            Ok(success_text(
                id,
                &json!({
                    "transactionId":t["id"],
                    "epoch":t["epoch"],
                    "action":"create-lane",
                    "payload":payload,
                    // Live's API deletes no take lane: only Live's own undo takes it back.
                    "impact":"creates-take-lane-live-undo-only",
                    "confirmation":"apply",
                    "expiresAt":t["expiresAt"]
                }),
            ))
        }
        .await;
        result.unwrap_or_else(|e| adapter_tool_error(id, &e, "Take-lane preview requires fresh authoritative state."))
    }
    pub async fn live_arrangement_clip_apply_async(&self, id: &Value, params: &Value, signal: Option<&Signal>) -> Option<Value> {
        if !valid_transaction_params(params, "apply") {
            return Some(error(id, -32602, "transactionId, confirmation=apply, and idempotencyKey are required", None));
        }
        let Some(record) = self.clip_lifecycle_transactions.get(params["transactionId"].as_str().unwrap()) else {
            return Some(transaction_error(id, "Unknown or expired arrangement-clip transaction"));
        };
        let t = record.borrow().clone();
        if !matches!(
            t["kind"].as_str(),
            Some(
                "arrangement-create"
                    | "arrangement-delete"
                    | "arrangement-audio-create"
                    | "arrangement-take-lane-create"
                    | "take-lane-create"
            )
        ) || (t["state"] == "previewed" && t["expiresAt"].as_f64().is_some_and(|n| n <= kumi_common::time::now_ms_f64()))
        {
            return Some(transaction_error(id, "Unknown or expired arrangement-clip transaction"));
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
            let payload = &t["payload"];

            if !reconciliation && t["kind"] == "take-lane-create" {
                let snapshot = self.views.view_for(Some(&context), &[payload["trackRef"].clone()], None, &[]).await?;
                let value = serde_json::to_value(&snapshot).unwrap();
                let track = value["tracks"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|r| r["ref"] == payload["trackRef"])
                    .cloned()
                    .unwrap_or(Value::Null);
                if js_json::stringify(
                    &json!({"trackRef":payload["trackRef"],"trackIdentity":track["objectIdentity"],"lanes":track_lanes(&track)}),
                ) != t["fence"]
                {
                    return Ok(transaction_error(id, "the track or its take lanes changed since preview; preview again"));
                }
            }
            if !reconciliation && !matches!(t["kind"].as_str(), Some("arrangement-take-lane-create" | "take-lane-create")) {
                let snapshot = self.views.view_for(Some(&context), &[payload["trackRef"].clone()], None, &[]).await?;
                if self.arrangement_fence(&snapshot, &[payload["trackRef"].clone()])? != t["fence"] {
                    return Ok(transaction_error(id, "Arrangement changed since preview; preview again"));
                }
            }

            if !reconciliation && t["kind"] == "arrangement-take-lane-create" {
                let snapshot = self.views.view_for(Some(&context), &[payload["takeLaneRef"].clone()], None, &[]).await?;
                let (_, lane) = self.take_lane_row(&snapshot, payload["takeLaneRef"].as_str().unwrap())?;
                if js_json::stringify(&json!({
                "takeLaneRef":payload["takeLaneRef"],
                "laneIdentity":lane["objectIdentity"],
                "siblings":lane_siblings(&lane)}
                )) != t["fence"]
                {
                    return Ok(transaction_error(id, "take lane or its clips changed since preview; preview again"));
                }
            }

            let operation = match t["kind"].as_str().unwrap() {
                "arrangement-create" => "arrangement.clip.create",
                "arrangement-audio-create" => "arrangement.audio-clip.create",
                "arrangement-take-lane-create" => "take-lane.clip.create",
                "take-lane-create" => "take-lane.create",
                _ => "arrangement.clip.delete",
            };
            {
                let mut row = record.borrow_mut();
                row["state"] = json!("applying");
                row["applyKey"] = params["idempotencyKey"].clone();
            }
            let mut result = adapter.invoke_async(&LiveInvocation::new(operation, payload.clone()), Some(&context)).await?;
            let creates = t["kind"] != "arrangement-delete";
            if creates {
                if result.is_null() {
                    return Err(LiveError::type_error("Cannot read properties of null (reading 'ref')"));
                }
                if ![&result["ref"], &result["objectIdentity"]].iter().all(|v| is_non_empty_string(v, 256))
                    || !is_non_empty_string(&result["createdFingerprint"], 64)
                {
                    return Err(LiveError::error("Arrangement clip creation did not return exact identity"));
                }
            }

            record.borrow_mut()["created"] = result.clone();
            if t["kind"] == "take-lane-create" {
                // The new lane, read back where Live put it.
                let snapshot = self.views.view_for(Some(&context), &[payload["trackRef"].clone()], None, &[]).await?;
                let (_, lane) = self.take_lane_row(&snapshot, result["ref"].as_str().unwrap())?;
                if lane["objectIdentity"] != result["objectIdentity"] {
                    return Err(LiveError::error("created take lane identity was not confirmed"));
                }
                result["fingerprint"] = result["createdFingerprint"].clone();
                record.borrow_mut()["created"] = result.clone();
            } else if creates {
                let created_clip = self.clip_row(
                    &self
                        .views
                        .view_for(
                            Some(&context),
                            &[result["ref"].clone(), payload["trackRef"].clone(), payload["takeLaneRef"].clone()],
                            None,
                            &[],
                        )
                        .await?,
                    result["ref"].as_str().unwrap(),
                )?;
                if created_clip.clip["objectIdentity"] != result["objectIdentity"]
                    || capture_object_fingerprint(&created_clip.clip)? != result["createdFingerprint"]
                {
                    return Err(LiveError::error("created Arrangement clip identity or creation fingerprint was not confirmed"));
                }
                result["fingerprint"] = result["createdFingerprint"].clone();
                record.borrow_mut()["created"] = result.clone();
            }
            {
                let mut row = record.borrow_mut();
                row["applyKey"] = params["idempotencyKey"].clone();
                row["state"] = json!("applied");
            }
            Ok(success_text(
                id,
                &json!({
                "transactionId":t["id"],
                "state":"applied",
                "result":result,
                "idempotent":false}
                ),
            ))
        }
        .await;
        Some(result.unwrap_or_else(|e| {
            record.borrow_mut()["state"] = json!("uncertain");
            adapter_tool_error(id, &e, "Arrangement-clip state is uncertain; perform fresh discovery before retrying.")
        }))
    }
    pub async fn undo_arrangement_clip_async(&self, id: &Value, params: &Value, signal: Option<&Signal>) -> Value {
        let record = params["transactionId"].as_str().and_then(|id| self.clip_lifecycle_transactions.get(id));
        if record.as_ref().is_some_and(|r| r.borrow()["kind"] == "take-lane-create") {
            return transaction_error(id, "Live's API deletes no take lane; Live's own undo takes it back");
        }
        if record.as_ref().is_some_and(|r| r.borrow()["kind"] == "arrangement-take-lane-create") {
            return transaction_error(id, "The public LOM exposes no take-lane clip deletion; undo is unavailable for this transaction");
        }

        let Some(record) =
            record.filter(|r| matches!(r.borrow()["kind"].as_str(), Some("arrangement-create" | "arrangement-audio-create")))
        else {
            return transaction_error(id, "Only an applied Arrangement clip creation has automatic undo authority");
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
            return transaction_error(id, "Arrangement clip creation lacks exact undo identity");
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
                None,
            )
            .await?;
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
            adapter_tool_error(id, &e, "Arrangement clip undo is uncertain; inspect the exact created clip.")
        })
    }
}
