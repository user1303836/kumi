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
/// Where an Arrangement clip row sits, in beats: its start and its right edge.
fn arrangement_span(clip: &Value) -> Option<(f64, f64)> {
    let span = (clip["start"].as_f64()?, helpers::arrangement_clip_end(clip));
    span.1.is_finite().then_some(span)
}
/// The clips a move of an Arrangement clip to `position` replaces, as dropping it there in Live does: each
/// one's name and span, the part replaced (`from`–`to`; all of it when `whole`), and whether it's audio.
/// Live crashes when an Arrangement clip is copied onto a span a clip already holds, so these are cleared
/// before the clip lands.
fn in_new_place(snapshot: &Value, moving: &Value, track: &Value, position: f64) -> Vec<Value> {
    let Some((start, end)) = arrangement_span(moving) else { return vec![] };
    let target_end = position + (end - start);
    let clips = snapshot["arrangement"]["clips"].as_array().into_iter().flatten();
    clips
        .filter(|clip| clip["trackRef"] == track["ref"] && clip["ref"] != moving["ref"])
        .filter_map(|clip| {
            let (other_start, other_end) = arrangement_span(clip)?;
            (other_start < target_end - 1e-6 && other_end > position + 1e-6).then(|| {
                json!({
                    "name": clip["name"].as_str().unwrap_or("").chars().take(60).collect::<String>(),
                    "start": other_start,
                    "end": other_end,
                    "from": other_start.max(position),
                    "to": other_end.min(target_end),
                    "whole": other_start >= position - 1e-6 && other_end <= target_end + 1e-6,
                    // Live's own rows call every Arrangement clip "midi" (its clips all offer note calls); `isAudio`
                    // is what tells.
                    "audio": clip["isAudio"] == true || clip["kind"] == "audio",
                    "looping": clip["looping"] == true
                })
            })
        })
        .collect()
}
/// A clip a move replaces, as its refusal names it: "Vox" (beats 0 to 8).
fn named(clip: &Value) -> String {
    let beat = |v: &Value| kumi_common::js::number::to_string(v.as_f64().unwrap_or(f64::NAN));
    format!("\"{}\" (beats {} to {})", clip["name"].as_str().unwrap_or(""), beat(&clip["start"]), beat(&clip["end"]))
}

/// A failure after Kumi's Live extension cut into what a move replaces: what was cut, first, and the way back.
/// The cut stays until undone, so it never reads as nothing changed.
fn after_cuts(error: LiveError, cut: &[&Value]) -> LiveError {
    if cut.is_empty() {
        return error;
    }
    let beat = |v: &Value| kumi_common::js::number::to_string(v.as_f64().unwrap_or(f64::NAN));
    let what =
        cut.iter().map(|c| format!("{} at beats {} to {}", named(c), beat(&c["from"]), beat(&c["to"]))).collect::<Vec<_>>().join(" and ");
    let reason = error.message().strip_prefix("request failed: ").unwrap_or(error.message()).replace("; nothing changed", "");
    let back = if cut.len() == 1 { "Live's own undo puts it back in one step" } else { "Live's own undo puts them back, one step per cut" };
    LiveError::error(format!("Kumi cut {what} with its Live extension, then couldn't finish the move; {back}. Why: {reason}"))
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
            let mut replaces = vec![];
            let fence;
            if row.arrangement {
                if !status.has_operation("arrangement.clip.move") {
                    return Err(LiveError::error("arrangement clip move is unavailable"));
                }
                if !params["position"].as_f64().is_some_and(|n| n.is_finite() && n >= 0.0) {
                    return Ok(error(id, -32602, "position is required for an Arrangement clip move", None));
                }
                let position = params["position"].as_f64().unwrap();
                if let (Some(track), None) = (&row.track, &row.take_lane) {
                    replaces = in_new_place(&serde_json::to_value(&snapshot).unwrap(), &row.clip, track, position);
                    // The Remote Script cuts MIDI clips itself, looped ones too (Live keeps a loop's phase in its
                    // start marker); an audio clip crossing the new place is cut by Kumi's Live extension first,
                    // over the part that's replaced (one crossing both edges is split, keeping both ends).
                    let cuts: Vec<&Value> = replaces.iter().filter(|r| r["audio"] == true && r["whole"] != true).collect();
                    let beat = |v: &Value| kumi_common::js::number::to_string(v.as_f64().unwrap_or(f64::NAN));
                    if let Some(cut) = cuts.first() {
                        if !status.has_operation("clip.clear-range") {
                            return Err(LiveError::error(format!(
                                "{} crosses beats {} to {} of the clip's new place, and Kumi's Live extension cuts audio clips; without it, delete it first (delete_clip) or pick a free spot",
                                named(cut),
                                beat(&cut["from"]),
                                beat(&cut["to"])
                            )));
                        }
                        payload["clearFirst"] = json!(cuts
                            .iter()
                            .map(|r| json!({"trackRef":track["ref"],"fromBeat":r["from"],"toBeat":r["to"],"expectedName":track["name"]}))
                            .collect::<Vec<_>>());
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
            let mut t = json!({
            "id":tempo::transaction_id("clipmove"),
            "epoch":status.epoch,
            "kind":"move",
            "fence":fence,
            "clipRef":params["clipRef"],
            "payload":payload,
            "expiresAt":kumi_common::time::now_ms_f64()+TRANSACTION_TTL_MS,
            "state":"previewed"}
            );
            if !replaces.is_empty() {
                // What the preview showed in the new place, which the apply checks again before cutting anything.
                t["replaces"] = json!(replaces);
            }

            self.retain_bounded_transaction(&self.clip_lifecycle_transactions, t.clone(), "clip move")?;
            let mut body = json!({
            "transactionId":t["id"],
            "epoch":t["epoch"],
            "clipRef":params["clipRef"],
            "payload":payload,
            "impact":"moves-clip",
            "confirmation":"apply",
            "expiresAt":t["expiresAt"]});
            if !replaces.is_empty() {
                for r in &mut replaces {
                    let r = r.as_object_mut().unwrap();
                    r.remove("audio");
                    r.remove("looping");
                }
                body["impact"] = json!("moves-clip-replacing");
                body["replaces"] = json!(replaces);
                body["kept"] = json!("Kumi can't bring back what the move replaces; Live's undo can.");
            }
            Ok(success_text(id, &body))
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
                if let (Some(row), Some(snapshot)) = (&row, &snapshot) {
                    if !row.arrangement
                        || arrangement_move_fence(&t["clipRef"], &row.clip, &capture_object_fingerprint(&row.clip)?) != t["fence"]
                    {
                        return Ok(transaction_error(
                            id,
                            "Arrangement clip identity, position, or content changed since preview; preview again",
                        ));
                    }
                    // The move replaces only what the preview showed: a clip dropped or dragged into the new place
                    // since would be cut or removed unnamed. Checked before anything is cut.
                    let now = match (&row.track, &row.take_lane) {
                        (Some(track), None) => in_new_place(
                            &serde_json::to_value(snapshot).unwrap(),
                            &row.clip,
                            track,
                            payload["position"].as_f64().unwrap_or(f64::NAN),
                        ),
                        _ => vec![],
                    };
                    if json!(now) != t.get("replaces").cloned().unwrap_or_else(|| json!([])) {
                        return Ok(transaction_error(
                            id,
                            "What's in the clip's new place changed since preview, so Kumi cut and moved nothing; preview again",
                        ));
                    }
                }

                {
                    let mut t = record.borrow_mut();
                    t["state"] = json!("applying");
                    t["applyKey"] = params["idempotencyKey"].clone();
                }
                let mut args = json!({
                "ref":t["clipRef"],
                "position":payload["position"],
                "expectedObjectIdentity":payload["expectedObjectIdentity"],
                "expectedAuthorityRevision":payload["expectedAuthorityRevision"],
                "expectedContentFingerprint":payload["expectedContentFingerprint"]});
                // The audio clips Kumi's Live extension cuts, in the order it cuts them, as the preview named them.
                let cutting: Vec<&Value> =
                    t["replaces"].as_array().into_iter().flatten().filter(|r| r["audio"] == true && r["whole"] != true).collect();
                let mut cut = 0;
                if reconciliation && t["moveArgs"].is_object() {
                    args = t["moveArgs"].clone();
                    cut = cutting.len();
                } else if let Some(cuts) = payload["clearFirst"].as_array().filter(|cuts| !cuts.is_empty()) {
                    // Kumi's Live extension cuts the audio clips crossing the new place's edges, each over the part
                    // replaced. That can renumber the track's clips, so the clip to move is found again by its
                    // identity, with fresh authority.
                    let found = async {
                        for each in cuts {
                            adapter.invoke_async(&LiveInvocation::new("clip.clear-range", each.clone()), Some(&context)).await?;
                            cut += 1;
                        }
                        let fresh = self.views.view_for(Some(&context), &[cuts[0]["trackRef"].clone()], None, &[]).await?;
                        let value = serde_json::to_value(&fresh).unwrap();
                        let moving = value["arrangement"]["clips"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .find(|c| c["objectIdentity"] == payload["expectedObjectIdentity"]);
                        let Some((moving, reference)) = moving.and_then(|c| Some((c, c["ref"].as_str()?))) else {
                            return Err(LiveError::error("the clip wasn't found again"));
                        };
                        // A split before the clip renumbers it in Live, and its ref is part of its content's
                        // fingerprint: nothing else of it may have changed, and the move checks the fresh one.
                        let unnumbered = |clip: &Value| {
                            let mut clip = clip.clone();
                            clip.as_object_mut().map(|c| c.remove("ref"));
                            capture_object_fingerprint(&clip)
                        };
                        if row.as_ref().is_some_and(|row| unnumbered(&row.clip).ok() != unnumbered(moving).ok()) {
                            return Err(LiveError::error("the clip changed while Kumi cut its new place"));
                        }
                        Ok((reference.to_owned(), self.arrangement_clip_authority(&fresh, reference)?, capture_object_fingerprint(moving)?))
                    }
                    .await;
                    let (reference, authority, fingerprint) = found.map_err(|e| after_cuts(e, &cutting[..cut.min(cutting.len())]))?;
                    args["ref"] = json!(reference);
                    args["expectedContentFingerprint"] = json!(fingerprint);
                    merge(&mut args, &authority);
                    record.borrow_mut()["moveArgs"] = args.clone();
                }
                let result = adapter
                    .invoke_async(&LiveInvocation::new("arrangement.clip.move", args), Some(&context))
                    .await
                    .map_err(|e| after_cuts(e, &cutting[..cut.min(cutting.len())]))?;

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
                    // Moving back would replace what's in the clip's old place now; an undo never does.
                    let prior = payload["priorPosition"].as_f64().unwrap_or(f64::NAN);
                    if let Some(there) = current.track.as_ref().and_then(|track| {
                        in_new_place(&serde_json::to_value(&snapshot).unwrap(), &current.clip, track, prior).into_iter().next()
                    }) {
                        return Err(LiveError::error(format!(
                            "{} is in the clip's old place now, so Kumi left the clip where it is; undo refused",
                            named(&there)
                        )));
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
