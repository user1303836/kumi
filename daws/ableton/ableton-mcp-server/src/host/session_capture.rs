//! MIDI and scene capture retain exact created-object identities for recovery.
use super::*;
use arrangement::capture_object_fingerprint;
use audition::TRACK_CONTENT_PARTS;
use kumi_common::{
    abort::{Signal, SignalExt},
    js::json as js_json,
};
use retention::TransactionRecord;
use sha2::{Digest, Sha256};
fn rows(value: &Value) -> &[Value] {
    value.as_array().map(Vec::as_slice).unwrap_or(&[])
}
fn fields(value: &Value, names: &[&str]) -> Value {
    let mut row = json!({});
    for name in names {
        if let Some(value) = value.get(*name) {
            row[*name] = value.clone();
        }
    }
    row
}
fn authority_fields(value: &Value, names: &[&str]) -> Result<Value, LiveError> {
    if names.iter().any(|name| value.get(*name).is_none()) {
        return Err(LiveError::error("mutation authority contains an unsupported value"));
    }
    Ok(fields(value, names))
}
fn clips(snapshot: &LiveSnapshot) -> Vec<Value> {
    snapshot.tracks.iter().flatten().flat_map(|track| track.clips.iter().map(|clip| serde_json::to_value(clip).unwrap())).collect()
}
/// Whether a clip that is `identity` is still in `snapshot`: in a Session slot, a take lane or the Arrangement.
/// Arrangement clip refs are positional (`{track}:{index}`): the next clip takes a deleted one's ref, so only its
/// identity says it's still there.
fn clip_remains(snapshot: &LiveSnapshot, identity: &str) -> bool {
    owned_clip_ref(snapshot, identity).is_some()
}
/// Where the clip that is `identity` is now: its ref, wherever it is in `snapshot`.
fn owned_clip_ref(snapshot: &LiveSnapshot, identity: &str) -> Option<String> {
    let s = serde_json::to_value(snapshot).unwrap();
    rows(&s["tracks"])
        .iter()
        .flat_map(|track| rows(&track["clips"]).iter().chain(rows(&track["takeLanes"]).iter().flat_map(|lane| rows(&lane["clips"]))))
        .chain(rows(&s["arrangement"]["clips"]))
        .find(|clip| clip["objectIdentity"] == identity)
        .and_then(|clip| clip["ref"].as_str().map(str::to_owned))
}
impl McpHost {
    pub(super) fn capture_authority_revision(&self, snapshot: &LiveSnapshot) -> Result<String, LiveError> {
        let s = serde_json::to_value(snapshot).unwrap();
        let tracks: Vec<_> = rows(&s["tracks"])
            .iter()
            .map(|track| {
                let mut row = authority_fields(track, &["ref", "objectIdentity"])?;
                row["clips"] = json!(rows(&track["clips"])
                    .iter()
                    .map(|clip| authority_fields(clip, &["ref", "objectIdentity", "notesRevision"]))
                    .collect::<Result<Vec<_>, _>>()?);
                Ok::<_, LiveError>(row)
            })
            .collect::<Result<_, _>>()?;
        let scenes: Vec<_> = rows(&s["scenes"])
            .iter()
            .map(|scene| authority_fields(scene, &["ref", "objectIdentity", "index"]))
            .collect::<Result<_, _>>()?;

        let authority = json!({"tracks":tracks,"scenes":scenes,"playbackRevision":s["playback"]["revision"]});
        Ok(hex::encode(Sha256::digest(canonical_mutation_identity(&authority)?)))
    }
    pub(super) fn capture_fence(&self, snapshot: &LiveSnapshot) -> Result<String, LiveError> {
        let s = serde_json::to_value(snapshot).unwrap();
        let tracks: Vec<_> = rows(&s["tracks"])
            .iter()
            .map(|track| {
                let mut row = fields(track, &["ref", "kind"]);
                row["clips"] =
                    json!(rows(&track["clips"]).iter().map(|clip| fields(clip, &["ref", "name", "length", "notes"])).collect::<Vec<_>>());
                row
            })
            .collect();
        let scenes: Vec<_> = rows(&s["scenes"]).iter().map(|scene| fields(scene, &["ref", "name", "index"])).collect();
        let mut playback = fields(&s["playback"], &["revision", "firedTargets", "playingTargets"]);
        let mut transport = s["playback"]["transport"].clone();
        transport["position"] = Value::Null;
        playback["transport"] = transport;
        Ok(js_json::stringify(&json!({
        "structure":self.structure_revision(snapshot),
        "tracks":tracks,
        "scenes":scenes,
        "playback":playback}
        )))
    }
    pub(super) async fn delete_owned_clip_async(
        &self,
        adapter: &dyn AsyncLiveAdapter,
        reference: &str,
        identity: &str,
        context: &LiveOperationContext,
        _expected_fingerprint: Option<&str>,
        recovery_record: Option<&TransactionRecord>,
        allow_absent: bool,
        _expected_notes_revision: Option<&str>,
    ) -> Result<(), LiveError> {
        let snapshot = self.views.view_for(Some(context), &[json!(reference)], None, &[]).await?;
        // Arrangement clip refs are positional: a clip added or deleted before it moves it to another ref, so it's
        // found by its identity (and a retry after it went finds nothing left to delete).
        let Some(at) = owned_clip_ref(&snapshot, identity) else {
            if allow_absent {
                return Ok(());
            }
            self.clip_row(&snapshot, reference)?;
            return Err(LiveError::error("owned clip identity changed before cleanup"));
        };
        let located = self.clip_row(&snapshot, &at)?;
        // Live's ownership of a made clip (the remote adapter's cleanup token, the Remote Script's ledger row) is keyed
        // by the ref it was made at, so a delete at any other ref would be refused there: a clip that moved is refused
        // here, clearly and with nothing sent.
        if at != reference {
            return Err(LiveError::error(if located.arrangement {
                "the clip moved since it was made (a clip before it was added or removed): delete it in Live"
            } else {
                "the clip moved with its scene since it was made (a scene was added or removed above it): delete it in Live"
            }));
        }
        let operation = if located.arrangement { "arrangement.clip.delete" } else { "clip.delete" };
        let authority = if located.arrangement {
            self.arrangement_clip_authority(&snapshot, reference)?
        } else {
            self.clip_authority(&snapshot, reference)?
        };
        let mut args = json!({
        "ref":reference}
        );
        for (k, v) in authority.as_object().unwrap() {
            args[k] = v.clone();
        }

        if let Some(record) = recovery_record {
            self.invoke_undo_recovery(record, adapter, operation, &args, context).await?;
        } else {
            adapter.invoke_async(&LiveInvocation::new(operation, args), Some(context)).await?;
        }
        let parent = located.track.as_ref().and_then(|row| row.get("ref")).filter(|v| !v.is_null()).cloned().unwrap_or(json!(reference));
        let after = self.views.view_for(Some(context), &[parent], None, &[]).await?;
        if clip_remains(&after, identity) {
            return Err(LiveError::error("owned clip cleanup was not confirmed"));
        }
        Ok(())
    }
    pub async fn dispatch_session_capture_tool(
        &self,
        call: &ToolCall,
        signal: Option<&Signal>,
    ) -> Option<Result<Option<Value>, LiveError>> {
        let p = call.arguments.as_ref().unwrap_or(&Value::Null);
        Some(Ok(match call.name.as_str() {
            "live_capture_midi_preview" => Some(self.live_capture_preview_async(&call.id, p, "capture-midi").await),
            "live_scene_capture_preview" => Some(self.live_capture_preview_async(&call.id, p, "scene-capture").await),
            "live_capture_midi_apply" => self.live_capture_apply_async(&call.id, p, "capture-midi", signal).await,
            "live_scene_capture_apply" => self.live_capture_apply_async(&call.id, p, "scene-capture", signal).await,
            _ => return None,
        }))
    }
    pub async fn live_capture_preview_async(&self, id: &Value, params: &Value, kind: &str) -> Value {
        if !has_only(params, &[]) {
            return error(id, -32602, "capture preview accepts no arguments", None);
        }
        let result = async {
            let status = self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(reads::AUDITION_DEADLINE_MS)))).await?;
            let (operation, recovery_operation) =
                if kind == "capture-midi" { ("session.capture-midi", "clip.delete") } else { ("scene.capture", "scene.delete") };
            if !status.connected
                || !status.capabilities.iter().any(|c| c.as_str() == "session.read")
                || !status.has_operation(operation)
                || !status.has_operation(recovery_operation)
            {
                return Err(LiveError::error(format!("{kind} is unavailable")));
            }
            let snapshot = self
                .views
                .whole_set(
                    Some(&LiveOperationContext::with_deadline(self.deadline(reads::AUDITION_DEADLINE_MS))),
                    Some(TRACK_CONTENT_PARTS),
                )
                .await?;
            let t = json!({
            "id":tempo::transaction_id(if kind=="capture-midi"{
            "capturemidi"}
            else{
            "scenecapture"}
            ),
            "epoch":status.epoch,
            "kind":kind,
            "fence":self.capture_fence(&snapshot)?,
            "payload":{
            "expectedStateRevision":self.capture_authority_revision(&snapshot)?}
            ,
            "expiresAt":kumi_common::time::now_ms_f64()+TRANSACTION_TTL_MS,
            "state":"previewed"}
            );

            self.retain_bounded_transaction(&self.clip_lifecycle_transactions, t.clone(), kind)?;
            Ok(success_text(
                id,
                &json!({
                "transactionId":t["id"],
                "epoch":t["epoch"],
                "impact":if kind=="capture-midi"{
                "creates-session-midi-clips"}
                else{
                "creates-one-session-scene"}
                ,
                "confirmation":"apply",
                "expiresAt":t["expiresAt"]}
                ),
            ))
        }
        .await;
        result.unwrap_or_else(|e| adapter_tool_error(id, &e, "Capture preview failed without mutation; rediscover Session state."))
    }
    pub async fn live_capture_apply_async(&self, id: &Value, params: &Value, kind: &str, signal: Option<&Signal>) -> Option<Value> {
        if !valid_transaction_params(params, "apply") {
            return Some(error(id, -32602, "transactionId, confirmation=apply, and idempotencyKey are required", None));
        }
        let Some(record) = self.clip_lifecycle_transactions.get(params["transactionId"].as_str().unwrap()) else {
            return Some(transaction_error(id, "Unknown or expired capture transaction"));
        };
        let t = record.borrow().clone();
        if t["kind"] != kind || (t["state"] == "previewed" && t["expiresAt"].as_f64().is_some_and(|n| n <= kumi_common::time::now_ms_f64()))
        {
            return Some(transaction_error(id, "Unknown or expired capture transaction"));
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
            return Some(transaction_error(id, "Capture transaction is no longer applicable"));
        }
        if signal.is_some_and(Signal::aborted) {
            return None;
        }
        {
            let mut row = record.borrow_mut();
            row["state"] = json!("applying");
            row["applyKey"] = params["idempotencyKey"].clone();
        }
        let mut dispatched = reconciliation;

        let result = async {
            if reconciliation {
                self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(reads::AUDITION_DEADLINE_MS)))).await?;
            }
            let status = self.require_connected(Some("session.read"))?;
            if json!(status.epoch) != t["epoch"] {
                let mut row = record.borrow_mut();
                if reconciliation {
                    // What the first apply did is still unknown: the record stays the marker recovery needs.
                    row["state"] = json!("uncertain");
                } else {
                    row["state"] = json!("previewed");
                    row.as_object_mut().unwrap().remove("applyKey");
                }
                return Ok(transaction_error(id, "Live connection epoch changed; preview again"));
            }

            let adapter = self.async_adapter();
            let context = self.transaction_context(params, signal, reads::AUDITION_DEADLINE_MS);
            if !reconciliation {
                let before = self.views.whole_set(Some(&context), Some(TRACK_CONTENT_PARTS)).await?;
                if self.capture_fence(&before)? != t["fence"] {
                    let mut row = record.borrow_mut();
                    row["state"] = json!("previewed");
                    row.as_object_mut().unwrap().remove("applyKey");
                    return Ok(transaction_error(id, "Session state changed since capture preview; preview again"));
                }
            }

            dispatched = true;
            if kind == "capture-midi" {
                let result =
                    adapter.invoke_async(&LiveInvocation::new("session.capture-midi", t["payload"].clone()), Some(&context)).await?;
                if result.is_null() {
                    return Err(LiveError::type_error("Cannot read properties of null (reading 'clips')"));
                }
                let references: Vec<_> = rows(&result["clips"]).iter().filter(|v| v.is_string()).cloned().collect();
                let identities: Vec<_> = rows(&result["clipIdentities"]).iter().filter(|v| v.is_object()).cloned().collect();
                let after = self.views.view_for(Some(&context), &references, None, &[]).await?;
                let authoritative = clips(&after);

                if result["captured"] != json!(!references.is_empty())
                    || identities.len() != references.len()
                    || references.iter().any(|r| !authoritative.iter().any(|c| c["ref"] == *r))
                {
                    return Err(LiveError::error("MIDI capture postcondition was not confirmed"));
                }
                let owned: Vec<_> = references
                    .iter()
                    .map(|reference| {
                        let identity = identities.iter().find(|i| i["ref"] == *reference);
                        let clip = authoritative.iter().find(|c| c["ref"] == *reference);
                        let (Some(identity), Some(clip)) = (identity, clip) else {
                            return Err(LiveError::error("captured MIDI object identity or creation fingerprint is unavailable"));
                        };
                        if !is_non_empty_string(&identity["objectIdentity"], 256)
                            || !is_non_empty_string(&identity["createdFingerprint"], 64)
                            || capture_object_fingerprint(clip)? != identity["createdFingerprint"]
                        {
                            return Err(LiveError::error("captured MIDI object identity or creation fingerprint is unavailable"));
                        }
                        Ok(json!({
                        "ref":reference,
                        "objectIdentity":identity["objectIdentity"],
                        "fingerprint":identity["createdFingerprint"]}
                        ))
                    })
                    .collect::<Result<_, LiveError>>()?;
                record.borrow_mut()["created"] = json!({
                "clips":owned}
                );
            } else {
                let result = adapter.invoke_async(&LiveInvocation::new("scene.capture", t["payload"].clone()), Some(&context)).await?;
                let after = self.views.whole_set(Some(&context), Some(TRACK_CONTENT_PARTS)).await?;
                if result.is_null() {
                    return Err(LiveError::type_error("Cannot read properties of null (reading 'ref')"));
                }
                let scene = after.scenes.iter().flatten().find(|scene| json!(scene.ref_.0) == result["ref"]);

                if result["captured"] != true
                    || !result["ref"].is_string()
                    || !is_non_empty_string(&result["objectIdentity"], 256)
                    || !is_non_empty_string(&result["createdFingerprint"], 64)
                    || scene.is_none()
                    || self.session_structure_created_fingerprint(&after, "scene", &result["ref"])? != result["createdFingerprint"]
                {
                    return Err(LiveError::error("scene capture postcondition was not confirmed"));
                }
                record.borrow_mut()["created"] = json!({
                "sceneRef":result["ref"],
                "objectIdentity":result["objectIdentity"],
                "fingerprint":result["createdFingerprint"]}
                );
            }
            record.borrow_mut()["state"] = json!("applied");
            Ok(success_text(
                id,
                &json!({"transactionId":t["id"],"state":"applied","created":record.borrow()["created"],"idempotent":false}),
            ))
        }
        .await;
        Some(result.unwrap_or_else(|e| {
            let mut row = record.borrow_mut();
            row["state"] = json!(if dispatched { "uncertain" } else { "previewed" });
            if !dispatched {
                row.as_object_mut().unwrap().remove("applyKey");
            }
            adapter_tool_error(
                id,
                &e,
                if dispatched {
                    "Capture state is uncertain; perform fresh discovery before recovery."
                } else {
                    "Capture apply failed before dispatch; preview remains available until expiry."
                },
            )
        }))
    }
    pub async fn undo_session_capture_async(&self, id: &Value, params: &Value, signal: Option<&Signal>) -> Value {
        let Some(record) = params["transactionId"]
            .as_str()
            .and_then(|id| self.clip_lifecycle_transactions.get(id))
            .filter(|r| matches!(r.borrow()["kind"].as_str(), Some("capture-midi" | "scene-capture")))
        else {
            return transaction_error(id, "Unknown capture transaction");
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
        if t["state"] != "applied" && !reconciliation {
            return transaction_error(id, "Only an applied or exact-key uncertain capture transaction can be undone");
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

            let owned: Vec<_> = rows(&t["created"]["clips"]).iter().filter(|v| v.is_object()).cloned().collect();
            let references: Vec<_> = owned.iter().map(|o| o["ref"].clone()).collect();
            let snapshot = if t["kind"] == "capture-midi" {
                self.views.view_for(Some(&context), &references, None, &[]).await?
            } else {
                self.views.whole_set(Some(&context), Some(TRACK_CONTENT_PARTS)).await?
            };
            record.borrow_mut()["state"] = json!("undoing");

            if t["kind"] == "capture-midi" {
                let current = clips(&snapshot);
                for owned in &owned {
                    if let Some(clip) = current.iter().find(|c| owned["ref"].is_string() && c["ref"] == owned["ref"]) {
                        if !is_non_empty_string(&owned["objectIdentity"], 256) || owned["fingerprint"] != capture_object_fingerprint(clip)?
                        {
                            return Err(LiveError::error("captured MIDI clip identity or content changed before undo"));
                        }
                    }
                }

                for owned in &owned {
                    if let (Some(reference), Some(identity)) = (
                        owned["ref"].as_str(),
                        owned["objectIdentity"].as_str().filter(|_| is_non_empty_string(&owned["objectIdentity"], 256)),
                    ) {
                        self.delete_owned_clip_async(
                            adapter.as_ref(),
                            reference,
                            identity,
                            &context,
                            owned["fingerprint"].as_str(),
                            Some(&record),
                            true,
                            None,
                        )
                        .await?;
                    }
                }

                let after = self.views.view_for(Some(&context), &references, None, &[]).await?;
                let remaining = clips(&after);
                if owned.iter().any(|o| o["ref"].is_string() && remaining.iter().any(|c| c["ref"] == o["ref"])) {
                    return Err(LiveError::error("captured MIDI clip deletion was not confirmed"));
                }
            } else {
                // Scene refs are positional: once the captured scene is deleted the next scene has its ref, so it's
                // confirmed gone by its identity, and a retry after the delete went finds nothing left to delete.
                let identity =
                    t["created"]["objectIdentity"].as_str().filter(|_| is_non_empty_string(&t["created"]["objectIdentity"], 256));
                let present = |snapshot: &LiveSnapshot| {
                    identity
                        .is_some_and(|identity| snapshot.scenes.iter().flatten().any(|s| s.object_identity.as_deref() == Some(identity)))
                };
                let reference = t["created"]["sceneRef"].as_str();
                let found =
                    identity.and_then(|identity| snapshot.scenes.iter().flatten().find(|s| s.object_identity.as_deref() == Some(identity)));
                // Scene refs are positional, and so are its slots' and clips' in what it was made with: a scene added
                // or removed above it leaves nothing to check it against.
                if found.is_some_and(|scene| Some(scene.ref_.0.as_str()) != reference) {
                    return Err(LiveError::error(
                        "the captured scene moved since it was made (a scene was added or removed above it): delete it in Live",
                    ));
                }
                // Gone by its identity: a retry finds nothing left to delete; a first try still checks what's at its ref.
                let scene = found.or_else(|| {
                    reference.filter(|_| !reconciliation).and_then(|r| snapshot.scenes.iter().flatten().find(|s| s.ref_.0 == r))
                });
                if let Some(scene) = scene {
                    if !is_non_empty_string(&t["created"]["objectIdentity"], 256)
                        || t["created"]["fingerprint"]
                            != self.session_structure_created_fingerprint(&snapshot, "scene", &json!(scene.ref_.0))?
                    {
                        return Err(LiveError::error("captured scene identity or content changed before undo"));
                    }
                    self.invoke_undo_recovery(
                        &record,
                        adapter.as_ref(),
                        "scene.delete",
                        &json!({
                        "ref":scene.ref_.0,
                        "expectedStructureRevision":self.structure_revision(&snapshot),
                        "expectedObjectIdentity":t["created"]["objectIdentity"]}
                        ),
                        &context,
                    )
                    .await?;
                }

                if identity.is_some() {
                    let after = self.views.view(Some(&context), LiveViewScope::Indices(vec![]), Some(&[LiveSnapshotPart::Scenes])).await?;
                    if present(&after) {
                        return Err(LiveError::error("captured scene deletion was not confirmed"));
                    }
                }
            }
            record.borrow_mut()["state"] = json!("undone");
            Ok(success_text(id, &json!({"transactionId":t["id"],"state":"undone","idempotent":false})))
        }
        .await;
        result.unwrap_or_else(|e| {
            record.borrow_mut()["state"] = json!("uncertain");
            adapter_tool_error(id, &e, "Capture undo is uncertain; perform fresh Session discovery.")
        })
    }
}
