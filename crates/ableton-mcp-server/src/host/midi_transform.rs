//! MIDI transform transactions preserve source clips and resume exact indexed note plans.
use super::*;
use crate::midi_transforms::*;
use kumi_common::{
    abort::{Signal, SignalExt},
    js::{json as js_json, string as js_string},
};
use midi_plan::*;

fn valid_params(params: &Value) -> bool {
    params.as_object().is_some_and(|p| {
        p.len() <= 12
            && p.values().all(|v| match v {
                Value::String(_) | Value::Number(_) => true,
                Value::Array(a) => {
                    !a.is_empty()
                        && a.len() <= 32
                        && a.iter().all(|v| v.as_str().is_some_and(|s| (1..=32).contains(&js_string::utf16_len(s))))
                }
                Value::Object(o) => {
                    o.len() <= 32 && o.iter().all(|(k, v)| js_string::utf16_len(k) <= 32 && v.as_f64().is_some_and(f64::is_finite))
                }
                _ => false,
            })
    })
}
fn merge(target: &mut Value, source: &Value) {
    if let Some(o) = source.as_object() {
        for (k, v) in o {
            target[k] = v.clone();
        }
    }
}
fn full_fence(reference: &Value, notes: &[Value], revision: &str, authority: &Value, target: &Value) -> String {
    let mut value = json!({
    "ref":reference,
    "notes":notes,
    "notesRevision":revision,
    "authority":authority}
    );
    merge(&mut value, target);
    js_json::stringify(&value)
}
fn transform_notes(notes: &[Value], kind: &str, params: &Value, length: f64) -> Result<(Vec<Value>, Value), LiveError> {
    let notes: Vec<Note> = serde_json::from_value(json!(notes)).map_err(|e| LiveError::error(e.to_string()))?;
    let outcome = apply_midi_transform(
        &notes,
        &MidiTransformSpec { r#type: kind.into(), params: params.as_object().cloned().unwrap_or_default() },
        Some(length),
    )
    .map_err(|e| LiveError::error(e.to_string()))?;

    Ok((outcome.notes.iter().map(|n| serde_json::to_value(n).unwrap()).collect(), serde_json::to_value(outcome).unwrap()))
}
/// Why a transform of an Arrangement clip can't write into a copy: a copy goes to a Session slot, which an Arrangement
/// clip can't be copied to, so its notes change in place.
fn arrangement_refusal(name: &str, kind: &str, generative: bool) -> String {
    if generative {
        format!("\"{name}\" is an Arrangement clip, and a generative transform ({kind}) writes into a copy in a Session slot, which an Arrangement clip can't be copied to: run it on a Session clip, or use a transform that changes notes in place (scope in-place)")
    } else {
        format!("\"{name}\" is an Arrangement clip: its notes change in place (scope in-place); duplicate scope copies into a Session slot, which an Arrangement clip can't be copied to")
    }
}
fn note_diff(before: &[Value], after: &[Value]) -> Result<Value, LiveError> {
    let before: Vec<Note> = serde_json::from_value(json!(before)).map_err(|e| LiveError::error(e.to_string()))?;
    let after: Vec<Note> = serde_json::from_value(json!(after)).map_err(|e| LiveError::error(e.to_string()))?;
    Ok(serde_json::to_value(diff_notes(&before, &after)).unwrap())
}
fn len(value: &Value) -> usize {
    value.as_array().map_or(0, Vec::len)
}
fn discover_drums(snapshot: &Value) -> Result<(Value, Vec<String>), LiveError> {
    fn visit(devices: &[Value], mapping: &mut Value, found: &mut Vec<String>) -> Result<(), LiveError> {
        for device in devices.iter().filter(|v| v.is_object()) {
            for chain in device["chains"].as_array().into_iter().flatten().filter(|v| v.is_object()) {
                let name = helpers::js_string(chain.get("name").filter(|v| !v.is_null()).unwrap_or(&json!("")))?;
                let lower = name.to_lowercase();
                let note = &chain["inNote"];
                if is_integer_in_range(note, 0.0, 127.0) {
                    for (role, matches) in [
                        ("kick", lower.contains("kick")),
                        ("snare", lower.contains("snare")),
                        ("openHat", lower.contains("open") && lower.contains("hat")),
                        ("closedHat", lower.contains("hat")),
                        ("clap", lower.contains("clap")),
                        ("ride", lower.contains("ride")),
                        ("crash", lower.contains("crash")),
                        ("highTom", lower.contains("high") && lower.contains("tom")),
                        ("lowTom", lower.contains("low") && lower.contains("tom")),
                        ("midTom", lower.contains("tom")),
                    ] {
                        if mapping.get(role).is_none() && matches {
                            mapping[role] = note.clone();
                            found.push(format!("{role}={} (chain \"{name}\")", js_json::stringify(note)));
                            break;
                        }
                    }
                }
                visit(chain["devices"].as_array().map(Vec::as_slice).unwrap_or(&[]), mapping, found)?;
            }
        }
        Ok(())
    }
    let mut mapping = json!({});
    let mut found = vec![];
    for track in snapshot["tracks"].as_array().into_iter().flatten().filter(|v| v.is_object()) {
        visit(track["devices"].as_array().map(Vec::as_slice).unwrap_or(&[]), &mut mapping, &mut found)?;
    }

    let assumptions = if found.is_empty() {
        vec![]
    } else {
        vec![format!("drum mapping discovered from the Set's drum-chain notes: {}", found.join(", "))]
    };
    Ok((mapping, assumptions))
}
impl McpHost {
    async fn resolve_midi_context(
        &self,
        kind: &str,
        params: &Value,
        snapshot: &LiveSnapshot,
        status: &LiveStatus,
    ) -> Result<(Value, Vec<String>), LiveError> {
        let mut resolved = params.clone();
        let mut assumptions = vec![];
        if ["chord-progression", "bassline"].contains(&kind) && resolved.get("root").is_none() && resolved.get("scale").is_none() {
            let tokens = if kind == "chord-progression" {
                ["chords", "numerals"].iter().find_map(|name| resolved.get(*name).filter(|v| !v.is_null())).unwrap_or(&resolved["symbols"])
            } else {
                &resolved["chords"]
            };

            let roman = regex::Regex::new(r"(?i)^(vii|vi|iv|v|iii|ii|i)(°|dim)?(7)?$").unwrap();
            if tokens.as_array().is_some_and(|tokens| {
                !tokens.is_empty() && tokens.iter().all(|v| v.as_str().is_some_and(|s| roman.is_match(js_string::trim(s))))
            }) {
                if !status.has_operation("tuning.read") {
                    return Err(LiveError::error("roman-numeral input requires an explicit key/mode: tuning state is unavailable"));
                }

                let tuning = self
                    .async_adapter()
                    .invoke_async(&LiveInvocation::new("tuning.read", json!({"setRef":snapshot.set.as_ref().ok_or_else(||LiveError::type_error("Cannot read properties of undefined (reading 'ref')"))?.ref_})), None)
                    .await?;
                if tuning.is_null() {
                    return Err(LiveError::type_error("Cannot read properties of null (reading 'scale')"));
                }
                let scale = &tuning["scale"];
                let root = &scale["rootNote"];
                let name = scale["scaleName"].as_str().map(|s| {
                    let lower = js_string::trim(s).to_lowercase();
                    let mut result = String::new();
                    let mut space = false;
                    for c in lower.chars() {
                        if js_string::trim(&c.to_string()).is_empty() {
                            if !space {
                                result.push('-');
                            }
                            space = true;
                        } else {
                            result.push(c);
                            space = false;
                        }
                    }
                    result
                });

                if !is_integer_in_range(root, 0.0, 11.0) || name.as_ref().is_none_or(String::is_empty) {
                    return Err(LiveError::error("roman-numeral input requires an explicit key/mode: the Set does not name a song scale"));
                }

                let name = name.unwrap();
                resolved["root"] = root.clone();
                resolved["scale"] = json!(name);
                assumptions.push(format!("key/mode discovered from the Set's song scale: root {}, {name}", js_json::stringify(root)));
            }
        }
        if kind == "drum-pattern" && resolved.get("mapping").is_none() {
            let (mapping, found) = discover_drums(&serde_json::to_value(snapshot).unwrap())?;
            if mapping.as_object().unwrap().is_empty() {
                return Err(LiveError::error("drum-pattern requires an explicit mapping: no drum-chain notes were discovered in the Set"));
            }
            resolved["mapping"] = mapping;
            assumptions.extend(found);
        }

        Ok((resolved, assumptions))
    }
    pub async fn dispatch_midi_transform_tool(&self, call: &ToolCall, signal: Option<&Signal>) -> Option<Result<Option<Value>, LiveError>> {
        let p = call.arguments.as_ref().unwrap_or(&Value::Null);
        Some(match call.name.as_str() {
            "live_midi_transform_preview" => self.live_midi_transform_preview_async(&call.id, p).await.map(Some),
            "live_midi_transform_apply" => Ok(self.live_midi_transform_apply_async(&call.id, p, signal).await),
            _ => return None,
        })
    }
    pub async fn live_midi_transform_preview_async(&self, id: &Value, params: &Value) -> Result<Value, LiveError> {
        let mut valid = has_only(params, &["clipRef", "transform", "params", "scope", "target"])
            && is_non_empty_string(&params["clipRef"], 256)
            && params["transform"].as_str().is_some_and(|s| MIDI_TRANSFORM_TYPES.contains(&s))
            && valid_params(&params["params"]);

        if valid {
            if let Some(scope) = params.get("scope") {
                valid = ["in-place", "duplicate"].contains(&helpers::js_string(scope)?.as_str());
            }
        }
        if !valid {
            return Ok(error(
                id,
                -32602,
                "clipRef, a known transform, and bounded params (strings, numbers, string arrays, or flat pitch maps) are required",
                None,
            ));
        }

        let kind = params["transform"].as_str().unwrap();
        let generative = GENERATIVE_TRANSFORMS.contains(&kind);
        let probe = serde_json::to_value(midi_expression_probe()).unwrap();

        let result=async{
            let status=self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(reads::AUDITION_DEADLINE_MS)))).await?;
            if !status.connected||!["session.midi_note.write",
"session.midi_note.read"].iter().all(|wanted|status.capabilities.iter().any(|c|c.as_str()==*wanted)){
return Err(LiveError::error("midi note read/write capability is unavailable"));
}

            for operation in ["snapshot",
"note.update",
"note.delete",
"note.add-batch"]{
if !status.has_operation(operation){
return Err(LiveError::error(format!("{operation} is unavailable")));
}
}

            let scope=params.get("scope");
            let snapshot=if kind=="drum-pattern"&&params["params"].get("mapping").is_none(){
self.views.whole_set(None,
Some(audition::TRACK_CONTENT_PARTS)).await?}
else{
self.views.view_for(None,
&[params["clipRef"].clone(),
if params["target"].is_object(){
params["target"]["trackRef"].clone()}
else{
Value::Null}
],
None,
&[]).await?}
;

            let clip=self.note_clip(&snapshot, None,params["clipRef"].as_str().unwrap()).await?;
            // An Arrangement clip's notes change in place: a generative transform (whatever its scope) and a copy are
            // refused first, so the model isn't sent toward a copy or a Session slot before this.
            if clip.arrangement&&(generative||scope==Some(&json!("duplicate"))){
return Ok(transaction_error(id,&arrangement_refusal(&clip.name,kind,generative)));
}
            if scope==Some(&json!("in-place"))&&generative&&probe["deleteRecreatePreservesExpression"]!=true{
return Ok(transaction_error(id,
"Generative transforms delete and recreate notes, which cannot preserve per-note expression the canonical schema does not expose; use duplicate scope so the source clip is preserved"));
}
            if scope==Some(&json!("duplicate"))&&!params["target"].is_object(){
return Ok(error(id,
-32602,
"duplicate scope requires an exact target {trackRef, sceneIndex} naming an empty Session slot",
None));
}
            if clip.notes.iter().any(|n|!n["id"].is_number()){return Err(LiveError::error("stable note identity is unavailable for this clip"));}
            let(resolved,
mut assumptions)=if ["chord-progression",
"bassline",
"drum-pattern"].contains(&kind){
self.resolve_midi_context(kind,
&params["params"],
&snapshot,
&status).await?}
else{
(params["params"].clone(),
vec![])}
;

            let(notes,
outcome)=match transform_notes(&clip.notes,
kind,
&resolved,
clip.end){
Ok(v)=>v,
Err(e)=>return Ok(error(id,
-32602,
e.message(),
None))}
;
let diff=note_diff(&clip.notes,
&notes)?;

            let(add,update,delete)=(len(&diff["add"]),len(&diff["update"]),len(&diff["delete"]));
            if add+update+delete==0{return Ok(transaction_error(id,"transform produced no changes"));}
            if add+clip.notes.len()-delete>MIDI_TRANSFORM_MAX_NOTES{
return Ok(transaction_error(id,
&format!("transform result exceeds the bounded {MIDI_TRANSFORM_MAX_NOTES}-note limit")));
}

            let large=update>MIDI_TRANSFORM_LARGE_UPDATE_THRESHOLD;
            let effective=scope.cloned().unwrap_or_else(||json!(if generative||large{"duplicate"}else{"in-place"}));
            if effective=="in-place"&&generative{
return Ok(transaction_error(id,
"Generative transforms default to duplicate scope; request an exact duplicate target"));
}

            if effective=="in-place"&&large&&scope.is_none(){
return Ok(transaction_error(id,
"Large transforms default to duplicate scope; pass scope=in-place explicitly to edit the source clip"));
}

            // A large transform's copy (its default) goes to a Session slot too.
            if effective=="duplicate"&&clip.arrangement{
return Ok(transaction_error(id,&arrangement_refusal(&clip.name,kind,generative)));
}
            if effective=="duplicate"&&!params["target"].is_object(){
return Ok(error(id,
-32602,
"duplicate scope requires an exact target {trackRef, sceneIndex} naming an empty Session slot",
None));
}

            let mut target_authority=Value::Null;let mut target_fence=json!({});
            if effective=="duplicate"{
                for operation in ["clip.duplicate",
"clip.delete"]{
if !status.has_operation(operation){
return Err(LiveError::error(format!("{operation} is unavailable for duplicate-scope transforms")));
}
}

                let snapshot_value=serde_json::to_value(&snapshot).unwrap();let index=&params["target"]["sceneIndex"];
                let(track,slot,scene)=clip_duplicate::target_rows(&snapshot_value,&params["target"]["trackRef"],index);
                if track.is_null()||!is_non_empty_string(&track["objectIdentity"],
256){
return Err(LiveError::error("target track identity is not authoritative"));
}

                if slot.is_null()||scene.is_null()||![&slot["ref"],
&slot["objectIdentity"],
&scene["ref"],
&scene["objectIdentity"]].iter().all(|v|is_non_empty_string(v,
256)){
return Err(LiveError::error("target slot or scene identity is invalid"));
}

                if arrangement::truthy(&slot["clipRef"]){return Err(LiveError::error("target Session slot is occupied"));}
                target_authority=json!({
"trackRef":slot.get("parentRef").filter(|v|!v.is_null()).unwrap_or(&params["target"]["trackRef"]),
"sceneIndex":index,
"slotRef":slot["ref"],
"slotIdentity":slot["objectIdentity"],
"trackIdentity":track["objectIdentity"],
"sceneRef":scene["ref"],
"sceneIdentity":scene["objectIdentity"]}
);

                target_fence=json!({
"target":slot["ref"],
"targetIdentity":slot["objectIdentity"],
"targetTrackIdentity":track["objectIdentity"],
"targetSceneIdentity":scene["objectIdentity"],
"empty":slot["empty"]}
);

            }
            let seed=outcome.get("seed").cloned().unwrap_or(Value::Null);
            let t=json!({
"id":tempo::transaction_id("miditransform"),
"epoch":status.epoch,
"kind":"midi-transform",
"fence":full_fence(&params["clipRef"],
&clip.notes,
&clip.notes_revision,
&clip.authority,
&target_fence),
"clipRef":params["clipRef"],
"payload":{
"transform":kind,
"params":resolved,
"scope":effective,
"generative":generative,
"seed":seed,
"target":target_authority,
"diff":diff,
"sourceRevision":clip.notes_revision,
"authority":clip.authority,
"expectedResultContent":note_digest(&notes,
false)?,
"expectedResultIdentity":note_digest(&notes,
true)?,
"clipLength":clip.end}
,
"prior":{
"notes":clip.notes}
,
"expiresAt":kumi_common::time::now_ms_f64()+TRANSACTION_TTL_MS,
"state":"previewed"}
);

            self.retain_bounded_transaction(&self.clip_lifecycle_transactions,t.clone(),"MIDI transform")?;
            assumptions.extend(outcome["assumptions"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string));
            let mut mpe=probe.clone();
mpe["refusedInPlace"]=json!(generative&&probe["deleteRecreatePreservesExpression"]!=true);
mpe["note"]=json!("Per-note Pitch/Slide/Pressure are not in the canonical note schema and are never authored or silently erased by transforms; update-only transforms patch exposed fields through note.update, which preserves unexposed per-note data.");

            Ok(success_text(id,
&json!({
"transactionId":t["id"],
"epoch":t["epoch"],
"transform":kind,
"scope":effective,
"clipRef":params["clipRef"],
"sourceRevision":clip.notes_revision,
"diff":{
"add":add,
"update":update,
"delete":delete,
"notes":diff}
,
"constraints":{
"sourceNotes":clip.notes.len(),
"resultNotes":add+clip.notes.len()-delete,
"generative":generative,
"largeEdit":large,
"duplicateFirstDefault":generative||large}
,
"assumptions":assumptions,
"params":resolved,
"seed":seed,
"mpe":mpe,
"undo":if effective=="duplicate"{
"live_undo deletes the exact transaction-created duplicate clip"}
else{
"live_undo restores the exact prior note fields through note.update"}
,
"impact":if effective=="duplicate"{
"creates-one-transformed-duplicate-clip"}
else{
"transforms-midi-notes-in-place"}
,
"confirmation":"apply",
"expiresAt":t["expiresAt"]}
)))
        }.await;
        Ok(result.unwrap_or_else(|e| adapter_tool_error(id, &e, "MIDI transform preview requires fresh authoritative clip state.")))
    }
    pub async fn live_midi_transform_apply_async(&self, id: &Value, params: &Value, signal: Option<&Signal>) -> Option<Value> {
        if !valid_transaction_params(params, "apply") {
            return Some(error(id, -32602, "transactionId, confirmation=apply, and idempotencyKey are required", None));
        }

        let Some(record) = self.clip_lifecycle_transactions.get(params["transactionId"].as_str().unwrap()) else {
            return Some(transaction_error(id, "Unknown or expired MIDI-transform transaction"));
        };
        let t = record.borrow().clone();

        if t["kind"] != "midi-transform"
            || (t["state"] == "previewed" && t["expiresAt"].as_f64().is_some_and(|n| n <= kumi_common::time::now_ms_f64()))
        {
            return Some(transaction_error(id, "Unknown or expired MIDI-transform transaction"));
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
            let status = self.require_connected(Some("session.midi_note.write"))?;
            if json!(status.epoch) != t["epoch"] {
                return Ok(transaction_error(id, "Live connection epoch changed; preview again"));
            }

            let adapter = self.async_adapter();
            let context = self.transaction_context(params, signal, reads::AUDITION_DEADLINE_MS);
            let reference = t["clipRef"].as_str().unwrap();
            let payload = &t["payload"];
            let scope = &payload["scope"];
            let diff = &payload["diff"];

            if scope == "in-place" {
                if !reconciliation {
                    let current = self
                        .note_clip(
                            &self.views.view_for(Some(&context), &[t["clipRef"].clone()], None, &[]).await?,
                            Some(&context),
                            reference,
                        )
                        .await?;

                    if full_fence(&t["clipRef"], &current.notes, &current.notes_revision, &current.authority, &json!({}))
                        != full_fence(
                            &t["clipRef"],
                            t["prior"]["notes"].as_array().unwrap(),
                            payload["sourceRevision"].as_str().unwrap(),
                            &payload["authority"],
                            &json!({}),
                        )
                    {
                        return Ok(transaction_error(id, "clip identity or notes changed since preview; preview again"));
                    }
                }

                {
                    let mut row = record.borrow_mut();
                    row["state"] = json!("applying");
                    row["applyKey"] = params["idempotencyKey"].clone();
                }
                let plan = build_note_plan(diff);
                let prior = t["prior"]["notes"].as_array().unwrap();
                self.execute_note_plan(Some(&record), adapter.as_ref(), &context, reference, &plan, prior, true, None).await?;

                let verified = self
                    .note_clip(&self.views.view_for(Some(&context), &[t["clipRef"].clone()], None, &[]).await?, Some(&context), reference)
                    .await?;
                // Exactly as previewed, or so at the precision Live keeps notes (it reads 0.7 back as 0.699999988).
                if note_digest(&verified.notes, true)? != payload["expectedResultIdentity"]
                    && live_note_digest(&verified.notes, true)? != note_plan_result_digest(prior, &plan, true)?
                {
                    return Err(LiveError::error("MIDI transform postcondition was not confirmed"));
                }

                self.delete_undo_plan(&record);
                record.borrow_mut()["state"] = json!("applied");
                return Ok(success_text(
                    id,
                    &json!({
                    "transactionId":t["id"],
                    "state":"applied",
                    "scope":scope,
                    "updated":len(&diff["update"]),
                    "idempotent":false}
                    ),
                ));
            }
            let target = &payload["target"];
            if !arrangement::truthy(target) {
                return Ok(transaction_error(id, "Duplicate-scope transform lacks exact target authority"));
            }

            let snapshot = self
                .views
                .view_for(Some(&context), &[t["clipRef"].clone(), target["trackRef"].clone(), target["slotRef"].clone()], None, &[])
                .await?;

            if !reconciliation {
                let current = self.note_clip(&snapshot, Some(&context), reference).await?;
                let value = serde_json::to_value(&snapshot).unwrap();
                let track = value["tracks"].as_array().into_iter().flatten().find(|r| r["ref"] == target["trackRef"]);
                let slot = track.and_then(|r| r["clipSlots"].as_array()).into_iter().flatten().find(|r| r["ref"] == target["slotRef"]);
                let mut fence_target = json!({
                "target":target["slotRef"]}
                );
                if let Some(identity) = slot.and_then(|r| r.get("objectIdentity")) {
                    fence_target["targetIdentity"] = identity.clone();
                }
                if let Some(identity) = track.and_then(|r| r.get("objectIdentity")) {
                    fence_target["targetTrackIdentity"] = identity.clone();
                }
                fence_target["targetSceneIdentity"] = target["sceneIdentity"].clone();
                if let Some(empty) = slot.and_then(|r| r.get("empty")) {
                    fence_target["empty"] = empty.clone();
                }

                if full_fence(&t["clipRef"], &current.notes, &current.notes_revision, &current.authority, &fence_target) != t["fence"] {
                    return Ok(transaction_error(id, "clip identity, notes, or target slot changed since preview; preview again"));
                }
            }
            {
                let mut row = record.borrow_mut();
                row["state"] = json!("applying");
                row["applyKey"] = params["idempotencyKey"].clone();
            }
            let mut duplicate_ref = t["created"].get("ref").cloned();
            let mut duplicate_identity = t["created"].get("objectIdentity").cloned();
            if duplicate_ref.is_none() || duplicate_identity.is_none() {
                let mut args = json!({
                "ref":t["clipRef"],
                "targetTrackRef":target["trackRef"],
                "targetSceneIndex":target["sceneIndex"],
                "arrangementPosition":null}
                );
                merge(&mut args, &payload["authority"]);

                merge(
                    &mut args,
                    &json!({
                    "expectedContentFingerprint":capture_bounded_fingerprint(&self.clip_row(&snapshot,
                    reference)?.clip)?,
                    "expectedTargetTrackIdentity":target["trackIdentity"],
                    "expectedTargetSlotRef":target["slotRef"],
                    "expectedTargetSlotIdentity":target["slotIdentity"],
                    "expectedTargetSceneRef":target["sceneRef"],
                    "expectedTargetSceneIdentity":target["sceneIdentity"],
                    "expectedTargetCollectionRevision":null}
                    ),
                );

                let duplicate = adapter.invoke_async(&LiveInvocation::new("clip.duplicate", args), Some(&context)).await?;
                if duplicate.is_null() {
                    return Err(LiveError::type_error("Cannot read properties of null (reading 'ref')"));
                }
                if !is_non_empty_string(&duplicate["ref"], 256) || !is_non_empty_string(&duplicate["objectIdentity"], 256) {
                    return Err(LiveError::error("clip duplication did not return exact identity"));
                }

                duplicate_ref = Some(duplicate["ref"].clone());
                duplicate_identity = Some(duplicate["objectIdentity"].clone());
                let mut created = json!({
                "ref":duplicate_ref,
                "objectIdentity":duplicate_identity}
                );
                if duplicate["createdFingerprint"].is_string() {
                    created["fingerprint"] = duplicate["createdFingerprint"].clone();
                }
                record.borrow_mut()["created"] = created;
            }
            let duplicate_ref = duplicate_ref.unwrap();
            let duplicate_identity = duplicate_identity.unwrap();
            let reference = duplicate_ref.as_str().ok_or_else(|| LiveError::error("clip reference is unavailable"))?;

            let refs = [duplicate_ref.clone(), target["trackRef"].clone(), target["slotRef"].clone()];
            let duplicate = self.clip_row(&self.views.view_for(Some(&context), &refs, None, &[]).await?, reference)?;
            if duplicate.clip["objectIdentity"] != duplicate_identity {
                return Err(LiveError::error("transform duplicate identity changed since creation"));
            }

            let duplicate =
                self.note_clip(&self.views.view_for(Some(&context), &refs, None, &[]).await?, Some(&context), reference).await?;
            if record.borrow()["payload"]["duplicateInitial"].is_null() {
                record.borrow_mut()["payload"]["duplicateInitial"] = json!(duplicate.notes);
            }
            let initial = record.borrow()["payload"]["duplicateInitial"].as_array().unwrap().clone();
            let (transformed, _) = transform_notes(
                &initial,
                payload["transform"].as_str().unwrap(),
                &payload["params"],
                payload["clipLength"].as_f64().unwrap(),
            )?;

            let plan = build_note_plan(&note_diff(&initial, &transformed)?);
            self.execute_note_plan(Some(&record), adapter.as_ref(), &context, reference, &plan, &initial, false, None).await?;
            let verified_snapshot = self.views.view_for(Some(&context), &refs, None, &[]).await?;
            let verified = self.note_clip(&verified_snapshot, Some(&context), reference).await?;

            // Exactly as previewed, or the previewed result held at the precision Live keeps notes.
            if note_digest(&verified.notes, false)? != payload["expectedResultContent"]
                && (note_digest(&transformed, false)? != payload["expectedResultContent"]
                    || live_note_digest(&verified.notes, false)? != note_plan_result_digest(&initial, &plan, false)?)
            {
                return Err(LiveError::error("duplicate transform postcondition was not confirmed"));
            }

            record.borrow_mut()["created"] = json!({
            "ref":duplicate_ref,
            "objectIdentity":duplicate_identity,
            "fingerprint":capture_bounded_fingerprint(&self.clip_row(&verified_snapshot,
            reference)?.clip)?}
            );

            self.delete_undo_plan(&record);
            record.borrow_mut()["state"] = json!("applied");
            Ok(success_text(
                id,
                &json!({
                "transactionId":t["id"],
                "state":"applied",
                "scope":scope,
                "created":record.borrow()["created"],
                "idempotent":false}
                ),
            ))
        }
        .await;
        Some(result.unwrap_or_else(|e| {
            apply_failed(id, &record, &e, "MIDI-transform state is uncertain; perform fresh discovery before retrying.")
        }))
    }
    pub async fn undo_midi_transform_async(&self, id: &Value, params: &Value, signal: Option<&Signal>) -> Value {
        let Some(record) = params["transactionId"]
            .as_str()
            .and_then(|id| self.clip_lifecycle_transactions.get(id))
            .filter(|r| r.borrow()["kind"] == "midi-transform")
        else {
            return transaction_error(id, "Unknown or expired MIDI-transform transaction");
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
        if (t["state"] != "applied" && !reconciliation) || !arrangement::truthy(&t["clipRef"]) {
            return transaction_error(id, "Only an applied or exact-key uncertain MIDI-transform transaction can be undone");
        }

        let result = async {
            let status = self.require_connected(Some("session.read"))?;
            if json!(status.epoch) != t["epoch"] {
                return Ok(transaction_error(id, "Live connection epoch changed; undo refused"));
            }

            let adapter = self.async_adapter();
            let context = self.transaction_context(params, signal, reads::AUDITION_DEADLINE_MS);
            let reference = t["clipRef"].as_str().unwrap();
            let prior = t["prior"]["notes"].as_array().unwrap();

            if t["payload"]["scope"] == "duplicate" {
                if !is_non_empty_string(&t["created"]["ref"], 256) || !is_non_empty_string(&t["created"]["objectIdentity"], 256) {
                    return Ok(transaction_error(id, "Duplicate-scope transform lacks exact created identity"));
                }

                self.begin_undo_recovery(&record, params["idempotencyKey"].as_str().unwrap())?;
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
            } else {
                let identity = t["payload"]["authority"].get("expectedObjectIdentity");
                if !reconciliation {
                    let current = self
                        .note_clip(
                            &self.views.view_for(Some(&context), &[t["clipRef"].clone()], None, &[]).await?,
                            Some(&context),
                            reference,
                        )
                        .await?;
                    if let Some(moved) = self.undo_target_moved(
                        id,
                        &record.borrow(),
                        "clip",
                        &t["clipRef"],
                        current.authority.get("expectedObjectIdentity"),
                        identity,
                    )? {
                        return Ok(moved);
                    }

                    let ids: HashSet<_> = current.notes.iter().map(|n| js_json::stringify(&n["id"])).collect();
                    // As the transform left them, at the precision Live keeps notes.
                    let as_applied = || -> Result<bool, LiveError> {
                        Ok(note_digest(&current.notes, true)? == t["payload"]["expectedResultIdentity"]
                            || live_note_digest(&current.notes, true)?
                                == note_plan_result_digest(prior, &build_note_plan(&t["payload"]["diff"]), true)?)
                    };
                    if ids.len() != prior.len() || prior.iter().any(|n| !ids.contains(&js_json::stringify(&n["id"]))) || !as_applied()? {
                        return Ok(transaction_error(id, "notes changed after apply; undo refused"));
                    }
                }
                self.begin_undo_recovery(&record, params["idempotencyKey"].as_str().unwrap())?;
                {
                    let mut row = record.borrow_mut();
                    row["undoKey"] = params["idempotencyKey"].clone();
                    row["state"] = json!("undoing");
                }

                let updates = t["payload"]["diff"]["update"].as_array().cloned().unwrap_or_default();
                let updated: std::collections::HashMap<_, _> = updates.iter().rev().map(|n| (js_json::stringify(&n["id"]), n)).collect();
                let by_id: std::collections::HashMap<_, _> = prior.iter().map(|n| (js_json::stringify(&n["id"]), n)).collect();
                let applied: Vec<_> = prior
                    .iter()
                    .map(|n| {
                        let mut n = n.clone();
                        if let Some(update) = updated.get(&js_json::stringify(&n["id"])) {
                            merge(&mut n, update);
                        }
                        n
                    })
                    .collect();

                let restores: Vec<_> =
                    updates.iter().filter_map(|n| by_id.get(&js_json::stringify(&n["id"]))).map(|n| transform_patch(n)).collect();
                let steps: Vec<_> = restores.chunks(256).map(|items| json!({"operation":"note.update","items":items})).collect();
                self.execute_note_plan(
                    Some(&record),
                    adapter.as_ref(),
                    &context,
                    reference,
                    &steps,
                    &applied,
                    true,
                    identity.and_then(Value::as_str),
                )
                .await?;

                let verified = self
                    .note_clip(&self.views.view_for(Some(&context), &[t["clipRef"].clone()], None, &[]).await?, Some(&context), reference)
                    .await?;
                if note_digest(&verified.notes, true)? != note_digest(prior, true)? {
                    return Err(LiveError::error("MIDI-transform undo did not restore exact prior notes"));
                }
            }
            record.borrow_mut()["state"] = json!("undone");
            Ok(success_text(id, &json!({"transactionId":t["id"],"state":"undone","idempotent":false})))
        }
        .await;
        result.unwrap_or_else(|e| {
            record.borrow_mut()["state"] = json!("uncertain");
            adapter_tool_error(id, &e, "MIDI-transform undo is uncertain; perform fresh discovery.")
        })
    }
}
