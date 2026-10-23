//! Drum Rack pads, owned sample loading and batch recovery at arbitrary rack depth.
use super::*;
use super::{arrangement::truthy, device_parameter::fields, reads::AUDITION_DEADLINE_MS};
use kumi_common::{
    abort::Signal,
    js::{json as js_json, string},
};
use sha2::{Digest, Sha256};
use std::path::Path;
mod apply;
fn rows(value: &Value) -> &[Value] {
    value.as_array().map(Vec::as_slice).unwrap_or(&[])
}
fn digest(value: &Value) -> Result<String, LiveError> {
    Ok(hex::encode(Sha256::digest(canonical_mutation_identity(value)?)))
}
fn chain_revision(pad: &Value) -> Result<String, LiveError> {
    digest(&json!(rows(&pad["chains"])
        .iter()
        .map(|c| c.get("objectIdentity").cloned().ok_or_else(|| LiveError::error("mutation authority contains an unsupported value")))
        .collect::<Result<Vec<_>, _>>()?))
}
fn sample_name(path: &str) -> String {
    string::head(&Path::new(path).file_stem().unwrap_or_default().to_string_lossy(), 256)
}
fn note_label(note: f64) -> String {
    format!(
        "{}{}",
        ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"][(note as usize) % 12],
        (note / 12.).floor() as i64 - 2
    )
}
fn extend(a: &mut Value, b: &Value) {
    a.as_object_mut().unwrap().extend(b.as_object().unwrap().clone())
}
fn field<'a>(value: &'a Value, key: &str) -> Result<&'a Value, LiveError> {
    if value.is_null() {
        Err(LiveError::type_error(format!("Cannot read properties of null (reading '{key}')")))
    } else {
        Ok(&value[key])
    }
}
impl McpHost {
    pub(super) fn drum_pad_row(&self, snapshot: &LiveSnapshot, reference: &str) -> Result<Value, LiveError> {
        fn visit(devices: &Value, reference: &str, depth: usize) -> Option<Value> {
            if depth > 256 {
                return None;
            }
            for device in rows(devices).iter().filter(|v| v.is_object()) {
                let pads: Vec<_> = rows(&device["drumPads"]).iter().filter(|v| v.is_object()).collect();
                if let Some(pad) = pads.iter().find(|p| p["ref"] == reference) {
                    return Some((*pad).clone());
                }
                for chain in
                    rows(&device["chains"]).iter().chain(pads.iter().flat_map(|p| rows(&p["chains"]).iter())).filter(|v| v.is_object())
                {
                    if let Some(pad) = visit(&chain["devices"], reference, depth + 1) {
                        return Some(pad);
                    }
                }
            }
            None
        }
        for track in snapshot.tracks.iter().flatten() {
            let track = serde_json::to_value(track).unwrap();
            if let Some(pad) = visit(&track["devices"], reference, 0) {
                if !is_non_empty_string(&pad["objectIdentity"], 256) {
                    return Err(LiveError::error("drum pad identity is unavailable"));
                }
                return Ok(pad);
            }
        }
        Err(LiveError::error("drum pad reference is not authoritative"))
    }
    pub(super) fn drum_rack_pad(&self, snapshot: &LiveSnapshot, reference: &str, note: f64) -> Result<Value, LiveError> {
        let device = self.device_row(snapshot, reference)?.device;
        let pads = rows(&device["drumPads"]);
        if pads.is_empty() {
            return Err(LiveError::error("drum pad loading needs a Drum Rack"));
        }
        pads.iter().find(|p| p["note"].as_f64() == Some(note)).filter(|p| is_non_empty_string(&p["ref"], 256)).cloned().ok_or_else(|| {
            LiveError::error(format!("drum pad note {} isn't among the rack's visible pads", kumi_common::js::number::to_string(note)))
        })
    }
    pub async fn dispatch_drum_pad_tool(&self, call: &ToolCall, signal: Option<&Signal>) -> Option<Result<Option<Value>, LiveError>> {
        let p = call.arguments.as_ref().unwrap_or(&Value::Null);
        Some(Ok(match call.name.as_str() {
            "live_drum_pad_preview" => Some(self.live_drum_pad_preview_async(&call.id, p).await),
            "live_drum_pad_apply" => self.live_drum_pad_apply_async(&call.id, p, signal).await,
            _ => return None,
        }))
    }
    pub async fn live_drum_pad_preview_async(&self, id: &Value, p: &Value) -> Value {
        if !has_only(p, &["action", "padRef", "note", "solo", "deviceRef", "filePath", "allowedRoot", "pads", "instrument"]) {
            return error(id, -32602, "action and padRef are required", None);
        }
        if p.get("instrument").is_some_and(|v| v != "Simpler" && v != "Drum Sampler") {
            return error(id, -32602, "instrument must be Simpler or Drum Sampler", None);
        }
        if p.get("instrument").is_some() && p["action"] != "load-sample" && p["action"] != "load-samples" {
            return error(id, -32602, "an instrument goes only with loading samples through load-sample or load-samples", None);
        }
        if p["action"] == "load-samples" {
            return self.drum_pad_batch_preview(id, p).await;
        }
        if !p["action"].as_str().is_some_and(|a| ["set", "delete-all-chains", "load-sample", "sample-chain"].contains(&a)) {
            return error(id, -32602, "action must be set, delete-all-chains, load-sample, sample-chain or load-samples", None);
        }
        if p.get("pads").is_some() {
            return error(id, -32602, "pads go only with load-samples", None);
        }
        let chained = p["action"] == "sample-chain";
        let loading = p["action"] == "load-sample" || chained;
        if if loading {
            !is_non_empty_string(&p["deviceRef"], 256) || !is_integer_in_range(&p["note"], 0., 127.)
        } else {
            !is_non_empty_string(&p["padRef"], 256)
        } {
            return error(
                id,
                -32602,
                if loading { "deviceRef and note (0-127) are required to load a sample" } else { "action and padRef are required" },
                None,
            );
        }
        if !loading && ["deviceRef", "filePath", "allowedRoot"].iter().any(|k| p.get(*k).is_some()) {
            return error(id, -32602, "deviceRef and a sample file go only with load-sample", None);
        }
        let mut staging = None;
        let result=async{
   let status=self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS)))).await?;if !status.connected||!status.capabilities.iter().any(|c|c.as_str()=="session.read"){return Err(LiveError::error("session read capability is unavailable"))}let snapshot=self.views.view_for(None,&[p["deviceRef"].clone(),p["padRef"].clone()],None,&[]).await?;let pad=if loading{self.drum_rack_pad(&snapshot,p["deviceRef"].as_str().unwrap(),p["note"].as_f64().unwrap())?}else{self.drum_pad_row(&snapshot,p["padRef"].as_str().unwrap())?};let prior;let mut payload;
   if p["action"]=="set"{
    if !status.has_operation("drum-pad.set"){return Err(LiveError::error("drum pad editing is unavailable"))}if p.get("note").is_some(){return Ok(error(id,-32602,"DrumPad.note is read-only in the public LOM and cannot be assigned",None))}let mut proposed=json!({});if let Some(value)=p.get("solo"){if !value.is_boolean(){return Ok(error(id,-32602,"solo must be boolean",None))}proposed["solo"]=value.clone()}if proposed.as_object().unwrap().is_empty(){return Ok(error(id,-32602,"at least one pad field is required",None))}prior=json!({"note":pad["note"],"solo":pad["solo"]});payload=json!({"action":p["action"],"ref":p["padRef"]});extend(&mut payload,&proposed);if let Some(v)=pad.get("objectIdentity"){payload["expectedObjectIdentity"]=v.clone()}payload["expectedStateRevision"]=json!(digest(&prior)?);
   }else if loading{
    if !status.has_operation(if chained{"drum-pad.sample-chain"}else{"drum-pad.load-sample"}){return Err(LiveError::error(if chained{"loading a sample without the Browser needs Kumi's Live extension, which isn't connected"}else{"drum pad sample loading is unavailable"}))}if !rows(&pad["chains"]).is_empty(){return Err(LiveError::error("drum pad already has a sound; choose an empty pad"))}let file=self.audio_import_file_authority(&p["filePath"],&p["allowedRoot"]).await?;let path=self.stage_verified_import_file(file["canonicalPath"].as_str().unwrap(),&file).await?;staging=Some(path.clone());let name=sample_name(file["canonicalPath"].as_str().unwrap());prior=json!({"file":file,"chainCount":0});payload=json!({"action":p["action"],"ref":pad["ref"]});if let Some(v)=pad.get("objectIdentity"){payload["expectedObjectIdentity"]=v.clone()}if chained{payload["rackRef"]=p["deviceRef"].clone();payload["note"]=p["note"].clone()}payload["samplePath"]=json!(path);payload["name"]=json!(name);if chained{let device=self.device_row(&snapshot,p["deviceRef"].as_str().unwrap())?.device;if let Some(value)=device.get("name"){payload["expectedName"]=value.clone()}}
    if p["instrument"]=="Drum Sampler"{let preset=self.write_drum_sampler_preset(&path,&name)?;payload["instrument"]=json!("Drum Sampler");extend(&mut payload,&preset);if let Err(error)=self.await_browser_items(&[preset["presetItemId"].as_str().unwrap().into()]).await{self.release_drum_sampler_preset(&preset["presetPath"]);return Err(error)}}
   }else{
    if !status.has_operation("drum-pad.delete-all-chains"){return Err(LiveError::error("delete-all-chains is unavailable"))}let chains:Vec<_>=rows(&pad["chains"]).iter().filter(|v|v.is_object()).collect();prior=json!({"chainCount":chains.len()});payload=json!({"action":p["action"],"ref":p["padRef"],"expectedObjectIdentity":pad["objectIdentity"],"expectedStateRevision":digest(&json!(chains.iter().map(|c|c.get("objectIdentity").cloned().ok_or_else(||LiveError::error("mutation authority contains an unsupported value"))).collect::<Result<Vec<_>,_>>()?))?});
   }
   let mut fenced=json!({"action":p["action"],"ref":pad["ref"]});if let Some(value)=pad.get("objectIdentity"){fenced["objectIdentity"]=value.clone()}fenced["payload"]=payload.clone();let t=json!({"id":tempo::transaction_id("drumpad"),"epoch":status.epoch,"kind":"drum-pad","fence":js_json::stringify(&fenced),"payload":payload,"prior":prior,"expiresAt":kumi_common::time::now_ms_f64()+TRANSACTION_TTL_MS,"state":"previewed"});self.retain_bounded_transaction(&self.clip_lifecycle_transactions,t.clone(),"drum pad")?;staging=None;let mut response=json!({"transactionId":t["id"],"epoch":t["epoch"],"action":p["action"],"padRef":pad["ref"],"note":pad["note"]});if loading{response["sample"]=json!({"path":prior["file"]["canonicalPath"]})}else{response["prior"]=prior}response["impact"]=json!(if p["action"]=="set"{"edits-drum-pad"}else if loading{"loads-a-sample-onto-an-empty-drum-pad"}else{"deletes-all-pad-chains-no-undo"});response["confirmation"]=json!("apply");response["expiresAt"]=t["expiresAt"].clone();Ok(success_text(id,&response))
  }.await;
        result.unwrap_or_else(|e| {
            if let Some(path) = staging {
                self.release_staged_import_file(&json!(path))
            }
            adapter_tool_error(id, &e, "Drum-pad preview requires fresh authoritative state.")
        })
    }
    async fn drum_pad_batch_preview(&self, id: &Value, p: &Value) -> Value {
        let requested = &p["pads"];
        if !is_non_empty_string(&p["deviceRef"], 256)
            || requested.as_array().is_none_or(|a| a.is_empty() || a.len() > 128)
            || ["padRef", "note", "solo", "filePath", "allowedRoot"].iter().any(|k| p.get(*k).is_some())
        {
            return error(id, -32602, "load-samples takes the rack's deviceRef and 1 to 128 pads", None);
        }
        let requested = requested.as_array().unwrap();
        if !requested.iter().all(|item| {
            has_only(item, &["note", "filePath", "allowedRoot", "instrument"])
                && item.get("instrument").is_none_or(|v| v == "Simpler" || v == "Drum Sampler")
                && (p.get("instrument").is_none() || item.get("instrument").is_none())
                && is_integer_in_range(&item["note"], 0., 127.)
        }) {
            return error(id, -32602, "each pad needs its note (0-127), a sample file and its allowedRoot", None);
        }
        let mut notes = HashSet::new();
        if requested.iter().any(|p| !notes.insert(p["note"].as_f64().unwrap() as i64)) {
            return error(id, -32602, "each pad takes one sample", None);
        }
        let mut staged = vec![];
        let mut presets = vec![];
        let result=async{
   let status=self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS)))).await?;if !status.connected||!status.capabilities.iter().any(|c|c.as_str()=="session.read"){return Err(LiveError::error("session read capability is unavailable"))}if !status.has_operation("drum-pad.load-samples"){return Err(LiveError::error("drum pad sample loading is unavailable"))}let snapshot=self.views.view_for(None,&[p["deviceRef"].clone()],None,&[]).await?;let mut pads=vec![];let mut files=vec![];
   for item in requested{let note=item["note"].as_f64().unwrap();let pad=self.drum_rack_pad(&snapshot,p["deviceRef"].as_str().unwrap(),note)?;if !rows(&pad["chains"]).is_empty(){return Err(LiveError::error(format!("drum pad {} already has a sound; choose an empty pad",note_label(note))))}let mut file=self.audio_import_file_authority(&item["filePath"],&item["allowedRoot"]).await?;let path=self.stage_verified_import_file(file["canonicalPath"].as_str().unwrap(),&file).await?;staged.push(path.clone());let name=sample_name(file["canonicalPath"].as_str().unwrap());let preset=if item.get("instrument").filter(|v|!v.is_null()).or_else(||p.get("instrument")).is_some_and(|v|v=="Drum Sampler"){Some(self.write_drum_sampler_preset(&path,&name)?)}else{None};if let Some(v)=&preset{presets.push(v["presetPath"].clone())}let mut row=json!({"ref":pad["ref"]});if let Some(v)=pad.get("objectIdentity"){row["expectedObjectIdentity"]=v.clone()}row["samplePath"]=json!(path);row["name"]=json!(name);if let Some(preset)=preset{row["instrument"]=json!("Drum Sampler");extend(&mut row,&preset)}pads.push(row);file["note"]=item["note"].clone();files.push(file);
   }
   let waiting:Vec<_>=pads.iter().filter_map(|p|p["presetItemId"].as_str().map(str::to_owned)).collect();if !waiting.is_empty(){self.await_browser_items(&waiting).await?;}let payload=json!({"action":"load-samples","deviceRef":p["deviceRef"],"pads":pads});let t=json!({"id":tempo::transaction_id("drumpad"),"epoch":status.epoch,"kind":"drum-pad","fence":js_json::stringify(&json!({"action":"load-samples","ref":p["deviceRef"],"payload":payload})),"payload":payload,"prior":{"files":files},"expiresAt":kumi_common::time::now_ms_f64()+TRANSACTION_TTL_MS,"state":"previewed"});self.retain_bounded_transaction(&self.clip_lifecycle_transactions,t.clone(),"drum pad")?;staged.clear();presets.clear();Ok(success_text(id,&json!({"transactionId":t["id"],"epoch":t["epoch"],"action":"load-samples","deviceRef":p["deviceRef"],"pads":pads.iter().enumerate().map(|(i,p)|json!({"padRef":p["ref"],"note":files[i]["note"],"sample":{"path":files[i]["canonicalPath"]}})).collect::<Vec<_>>(),"impact":"loads-samples-onto-empty-drum-pads","confirmation":"apply","expiresAt":t["expiresAt"]})))
  }.await;
        result.unwrap_or_else(|e| {
            for path in staged {
                self.release_staged_import_file(&json!(path))
            }
            for path in presets {
                self.release_drum_sampler_preset(&path)
            }
            adapter_tool_error(id, &e, "Drum-pad preview requires fresh authoritative state.")
        })
    }
}
