//! Arrangement and take-lane creation retain exact identities for available undo.
use super::*;
use arrangement::capture_object_fingerprint;
use kumi_common::{
    abort::{Signal, SignalExt},
    js::json as js_json,
};
use sha2::{Digest, Sha256};
/// Kumi's undo of a clip in a take lane, refused: Live's API can't delete one.
pub(super) const LANE_CLIP_UNDO: &str = "Live's API deletes no clip in a take lane; Live's own undo takes it back";
/// What to do after a refused preview: nothing reached Live.
pub(super) const NOTHING_CHANGED: &str = "Nothing changed in Live: fix what the reason says (or take another route) and preview again.";
/// What to do instead of Kumi's undo of a take lane, or of a clip in one.
pub(super) const LIVE_UNDOES_IT: &str =
    "Nothing changed. Undo it in Live (Cmd-Z, or live_song_undo): Live takes back what came after it first.";
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
/// Why a track can't have a take lane, if it can't (Live 12.4.15b5 says "This track does not support take lanes").
fn no_lanes(track: &Value) -> Option<&'static str> {
    match track["kind"].as_str() {
        Some("return" | "main") => {
            Some("Live's return and main tracks have no take lanes (Live: “This track does not support take lanes”).")
        }
        Some("group") => Some("A group track holds no clips, so it has no take lanes: use one of the tracks in it."),
        _ => None,
    }
}
/// Where a clip in a take lane sits, in beats: its start and its right edge (endTime; start plus length without it).
fn lane_span(clip: &Value) -> Option<(f64, f64)> {
    let start = clip["start"].as_f64()?;
    Some((start, clip["endTime"].as_f64().or_else(|| Some(start + clip["length"].as_f64()?))?))
}
fn beats(value: f64) -> String {
    kumi_common::js::number::to_string((value * 1000.0).round() / 1000.0)
}
/// The clips in a take lane that [start, end) lands on, each by name and span. Live lays a new lane clip over them
/// as it does on the main lane (12.4.15b5: inside one splits it, over an edge trims it, over all of it removes it), and
/// its API can't delete or move a lane clip, so neither Kumi's undo nor anything else could put them back.
fn lane_clips_under(lane: &Value, start: f64, end: f64) -> Vec<String> {
    lane["clips"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|clip| {
            let (from, to) = lane_span(clip)?;
            let name =
                clip["name"].as_str().filter(|name| !name.is_empty()).map_or("a clip with no name".into(), |name| format!("“{name}”"));
            (from < end - 1e-6 && to > start + 1e-6).then(|| format!("{name} (beats {}–{})", beats(from), beats(to)))
        })
        .collect()
}
/// The clips in a take lane that end past `start`: what an audio file placed there might land on, its length unknown.
pub(super) fn lane_clips_from(lane: &Value, start: f64) -> Vec<String> {
    lane_clips_under(lane, start, f64::INFINITY)
}
/// What Kumi says for a clip it would lay over others in a take lane.
pub(super) fn lane_taken(lane: &Value, under: &[String]) -> String {
    let named = under.join(", ");
    let mut first = named.chars();
    format!(
        "{} {} in take lane “{}”: Live would cut {}, and its API can't put a clip in a lane back. Choose a free span in the lane, or another lane.",
        first.next().map(|c| c.to_uppercase().chain(first).collect::<String>()).unwrap_or_default(),
        if under.len() == 1 { "is there" } else { "are there" },
        lane["name"].as_str().unwrap_or(""),
        if under.len() == 1 { "it" } else { "them" }
    )
}
/// What Kumi says for an audio file it would place over others in a take lane, or before one.
pub(super) fn lane_audio_taken(lane: &Value, under: &[String]) -> String {
    format!(
        "{} Kumi puts an audio file in a take lane only past its last clip, since the file's length shows only once Live places it.",
        lane_taken(lane, under)
    )
}
/// Why a lane's track can't take this clip, as Live has it, if it can't: a clip of the other kind, or a frozen track.
pub(super) fn lane_track_refuses(track: &Value, audio: bool) -> Option<String> {
    if track["isFrozen"] == json!(true) {
        return Some(format!(
            "“{}” is frozen, and Live puts no clips on a frozen track (Live: “Clips cannot be created on frozen tracks”): unfreeze it first.",
            track["name"].as_str().unwrap_or("")
        ));
    }
    match (track["mediaKind"].as_str(), audio) {
        (Some("audio"), false) => {
            Some("A MIDI clip can't go in an audio track's take lane (Live: “MIDI clips can only be created on MIDI tracks”).".into())
        }
        (Some("midi"), true) => {
            Some("An audio file can't go in a MIDI track's take lane (Live: “Audio clips can only be created on audio tracks”).".into())
        }
        _ => None,
    }
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
        if take_lane && params.get("trackRef").is_some_and(|track| !is_non_empty_string(track, 256)) {
            return error(id, -32602, "trackRef, given with takeLaneRef, is the lane's track", None);
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
                let (lane_track, lane) = self.take_lane_row(&snapshot, params["takeLaneRef"].as_str().unwrap())?;
                if !is_non_empty_string(&lane["objectIdentity"], 256) {
                    return Err(LiveError::error("take-lane identity is not authoritative"));
                }
                if params.get("trackRef").is_some_and(|track| *track != lane_track["ref"]) {
                    let owner = lane_track["name"].as_str().unwrap_or("");
                    return Err(LiveError::error(format!(
                        "Take lane “{}” is on “{owner}”, not on that track: give “{owner}”'s trackRef, or a lane of that track's.",
                        lane["name"].as_str().unwrap_or("")
                    )));
                }
                if let Some(why) = lane_track_refuses(&lane_track, false) {
                    return Err(LiveError::error(why));
                }
                let start = params["position"].as_f64().unwrap();
                let under = lane_clips_under(&lane, start, start + params["length"].as_f64().unwrap());
                if !under.is_empty() {
                    return Err(LiveError::error(lane_taken(&lane, &under)));
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
                    "takeLane":{"ref":params["takeLaneRef"],"name":lane["name"]},
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
            let track = value["tracks"].as_array().into_iter().flatten().find(|r| r["ref"] == params["trackRef"]);
            if let Some(why) = track.and_then(no_lanes) {
                return Err(LiveError::error(why));
            }
            let track = track
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
                // The fence holds the lane's clips by identity, and a clip stretched into the span meanwhile keeps
                // its own, so the span is checked again where it lands.
                let start = payload["position"].as_f64().unwrap_or(0.0);
                let under = lane_clips_under(&lane, start, start + payload["length"].as_f64().unwrap_or(0.0));
                if !under.is_empty() {
                    return Ok(reason_error(id, &lane_taken(&lane, &under), NOTHING_CHANGED));
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
            return reason_error(id, "Live's API deletes no take lane; Live's own undo takes it back", LIVE_UNDOES_IT);
        }
        if record.as_ref().is_some_and(|r| r.borrow()["kind"] == "arrangement-take-lane-create") {
            return reason_error(id, LANE_CLIP_UNDO, LIVE_UNDOES_IT);
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn an_audio_file_goes_in_a_take_lane_only_past_its_last_clip() {
        let lane = json!({"name":"Comp","clips":[{"name":"Take","start":4.0,"endTime":36.0},{"name":"Short","start":40.0,"length":2.0}]});
        // Before a take (the file would run into it), inside one, or past one but before the next: refused.
        for position in [0.0, 8.0, 38.0] {
            assert!(!lane_clips_from(&lane, position).is_empty(), "{position}");
        }
        assert_eq!(lane_clips_from(&lane, 8.0), ["“Take” (beats 4–36)", "“Short” (beats 40–42)"]);
        // Past the last one's right edge: free.
        assert!(lane_clips_from(&lane, 42.0).is_empty());
        assert_eq!(
            lane_taken(&lane, &lane_clips_from(&lane, 38.0)),
            "“Short” (beats 40–42) is there in take lane “Comp”: Live would cut it, and its API can't put a clip in a lane back. Choose a free span in the lane, or another lane."
        );
    }
    #[test]
    fn a_lane_takes_only_its_own_kind_of_clip_and_none_on_a_frozen_track() {
        let refused = |track: Value, audio: bool| lane_track_refuses(&track, audio).unwrap_or_default();
        assert!(refused(json!({"mediaKind":"midi"}), true).starts_with("An audio file can't go in a MIDI track's take lane"));
        assert!(refused(json!({"mediaKind":"audio"}), false).starts_with("A MIDI clip can't go in an audio track's take lane"));
        assert!(refused(json!({"mediaKind":"audio","isFrozen":true,"name":"Vox"}), true).starts_with("“Vox” is frozen"));
        assert!(lane_track_refuses(&json!({"mediaKind":"audio","isFrozen":false}), true).is_none());
        assert!(lane_track_refuses(&json!({"mediaKind":"midi"}), false).is_none());
    }
}
