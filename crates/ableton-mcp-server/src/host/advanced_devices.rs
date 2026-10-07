//! Bank selection, momentary controls, cross-owner moves, and chain edits.
use super::*;
use super::{arrangement::truthy, device_parameter::fields, reads::AUDITION_DEADLINE_MS};
use kumi_common::{abort::Signal, js::json as js_json};
use sha2::{Digest, Sha256};
fn digest(v: &Value) -> Result<String, LiveError> {
    Ok(hex::encode(Sha256::digest(canonical_mutation_identity(v)?)))
}
fn extend(a: &mut Value, b: &Value) {
    a.as_object_mut().unwrap().extend(b.as_object().unwrap().clone())
}
fn chain_state(chain: &Value) -> Value {
    json!({"colorIndex":chain["colorIndex"],"autoColor":chain["autoColor"],"mute":chain["mute"],"solo":chain["solo"]})
}
fn chain_fence(reference: &Value, chain: &Value) -> String {
    let mut value = json!({"ref":reference});
    if let Some(v) = chain.get("objectIdentity") {
        value["objectIdentity"] = v.clone()
    }
    value["state"] = chain_state(chain);
    js_json::stringify(&value)
}
fn strict(a: Option<&Value>, b: Option<&Value>) -> bool {
    match (a.and_then(Value::as_f64), b.and_then(Value::as_f64)) {
        (Some(a), Some(b)) => a == b,
        _ => a == b,
    }
}
fn expect_result<'a>(result: &'a Value, field: &str) -> Result<&'a Value, LiveError> {
    if result.is_null() {
        Err(LiveError::type_error(format!("Cannot read properties of null (reading '{field}')")))
    } else {
        Ok(&result[field])
    }
}
/// A track's or chain's devices by ref and identity: the neighbours a cross-target move's index lands among.
fn siblings_of(target: &Value) -> Value {
    Value::Array(
        target["devices"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|v| v.is_object())
            .map(|v| fields(v, &["ref", "objectIdentity"]))
            .collect(),
    )
}
impl McpHost {
    async fn advanced_status(&self) -> Result<LiveStatus, LiveError> {
        let status = self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS)))).await?;
        if !status.connected || !status.capabilities.iter().any(|c| c.as_str() == "session.read") {
            return Err(LiveError::error("session read capability is unavailable"));
        }
        Ok(status)
    }
    pub async fn dispatch_advanced_device_tool(
        &self,
        call: &ToolCall,
        signal: Option<&Signal>,
    ) -> Option<Result<Option<Value>, LiveError>> {
        let p = call.arguments.as_ref().unwrap_or(&Value::Null);
        Some(Ok(match call.name.as_str() {
            "live_device_advanced_preview" => Some(self.live_device_advanced_preview_async(&call.id, p).await),
            "live_device_advanced_apply" => self.live_device_advanced_apply_async(&call.id, p, signal).await,
            "live_chain_preview" => Some(self.live_chain_preview_async(&call.id, p).await),
            "live_chain_apply" => self.live_chain_apply_async(&call.id, p, signal).await,
            _ => return None,
        }))
    }
    pub async fn live_device_advanced_preview_async(&self, id: &Value, p: &Value) -> Value {
        if !has_only(
            p,
            &["action", "ref", "bank", "slot", "trackRef", "chainRef", "deviceName", "index", "targetTrackRef", "targetChainRef"],
        ) || !p["action"]
            .as_str()
            .is_some_and(|s| ["set-bank", "re-enable-automation", "save-comparison", "insert-chain", "move-cross"].contains(&s))
        {
            return error(id, -32602, "a valid action is required", None);
        }
        let result=async{
            let status=self.advanced_status().await?;let snapshot=self.views.view_for(None,&[p["ref"].clone(),p["trackRef"].clone(),p["chainRef"].clone(),p["targetTrackRef"].clone(),p["targetChainRef"].clone()],None,&[]).await?;
            let mut payload;let mut prior=json!({});let impact;let mut target_siblings=Value::Null;
            match p["action"].as_str().unwrap(){
                "set-bank"=>{
                    if !status.has_operation("device.bank.set"){return Err(LiveError::error("parameter banks are unavailable"))}
                    if !is_non_empty_string(&p["ref"],256)||!is_integer_in_range(&p["bank"],0.,32.){return Ok(error(id,-32602,"ref and bank (0-32) are required",None))}
                    let row=self.device_row(&snapshot,p["ref"].as_str().unwrap())?;let bank=&row.device["parameterBank"];
                    if bank.is_null(){return Ok(transaction_error(id,"parameter banks are unavailable on this exact device"))}
                    if p["bank"].as_f64().unwrap()>=bank.as_f64().unwrap_or_else(|| kumi_common::js::number::parse(&js_string(bank).unwrap_or_default()).unwrap_or(f64::NAN)){return Ok(error(id,-32602,"bank exceeds the device's parameter bank count",None))}
                    payload=fields(p,&["action","ref","bank"]);if let Some(v)=row.device.get("objectIdentity"){payload["expectedObjectIdentity"]=v.clone()}prior=json!({"bankCount":bank});payload["expectedStateRevision"]=json!(digest(&prior)?);impact="momentary-control-surface-bank-selection-no-undo";
                },
                "re-enable-automation"=>{
                    if !status.has_operation("parameter.re-enable-automation"){return Err(LiveError::error("automation re-enable is unavailable"))}if !is_non_empty_string(&p["ref"],256){return Ok(error(id,-32602,"ref is required",None))}
                    let row=self.parameter_row(&snapshot,p["ref"].as_str().unwrap())?;payload=fields(p,&["action","ref"]);if let Some(v)=row.get("objectIdentity"){payload["expectedObjectIdentity"]=v.clone()}payload["expectedStateRevision"]=json!(digest(&json!({"automationState":row.get("automationState").filter(|v|v.is_string()).cloned().unwrap_or(json!("none"))}))?);impact="momentary-no-undo";
                },
                "save-comparison"=>{
                    if !status.has_operation("device.comparison.save-to-slot"){return Err(LiveError::error("comparison save is unavailable"))}if !is_non_empty_string(&p["ref"],256){return Ok(error(id,-32602,"ref is required",None))}
                    let row=self.device_row(&snapshot,p["ref"].as_str().unwrap())?;let comparison=&row.device["comparison"];if comparison["capability"]!=true{return Ok(transaction_error(id,"A/B comparison is unavailable on this exact device"))}
                    payload=fields(p,&["action","ref"]);if let Some(v)=row.device.get("objectIdentity"){payload["expectedObjectIdentity"]=v.clone()}payload["expectedStateRevision"]=json!(digest(&json!({"canCompareAb":comparison["capability"],"isUsingComparePresetB":comparison["activeSide"].as_f64()==Some(1.)}))?);impact="momentary-no-undo";
                },
                "insert-chain"=>{
                    if !status.has_operation("device.insert"){return Err(LiveError::error("device insertion is unavailable"))}if !["trackRef","chainRef","deviceName"].iter().all(|k|is_non_empty_string(&p[*k],256)){return Ok(error(id,-32602,"trackRef, chainRef, and deviceName are required",None))}
                    let track=snapshot.tracks.iter().flatten().find(|t|json!(t.ref_)==p["trackRef"]).map(|t|serde_json::to_value(t).unwrap()).unwrap_or(Value::Null);if !is_non_empty_string(&track["objectIdentity"],256){return Err(LiveError::error("track identity is not authoritative"))}
                    let found=self.chain_row(&snapshot,p["chainRef"].as_str().unwrap())?;let siblings:Vec<_>=found.chain["devices"].as_array().into_iter().flatten().filter(|v|v.is_object()).map(|v|fields(v,&["ref","objectIdentity"])).collect();payload=fields(p,&["action","trackRef","chainRef","deviceName","index"]);payload["expectedTrackIdentity"]=track["objectIdentity"].clone();payload["expectedSiblings"]=json!(siblings);prior=json!({"siblings":siblings});impact="creates-chain-device-cleanup-guarded";
                },
                _=>{
                    if !status.has_operation("device.move"){return Err(LiveError::error("device move is unavailable"))}if !is_non_empty_string(&p["ref"],256)||!is_integer_in_range(&p["index"],0.,f64::INFINITY){return Ok(error(id,-32602,"ref and index are required",None))}
                    if p.get("targetTrackRef").is_some()==p.get("targetChainRef").is_some(){return Ok(error(id,-32602,"exactly one of targetTrackRef or targetChainRef is required",None))}
                    let row=self.device_row(&snapshot,p["ref"].as_str().unwrap())?;
                    let target=if p.get("targetTrackRef").is_some(){if !is_non_empty_string(&p["targetTrackRef"],256){return Ok(error(id,-32602,"targetTrackRef is invalid",None))}snapshot.tracks.iter().flatten().find(|t|json!(t.ref_)==p["targetTrackRef"]).map(|t|serde_json::to_value(t).unwrap()).unwrap_or(Value::Null)}else{if !is_non_empty_string(&p["targetChainRef"],256){return Ok(error(id,-32602,"targetChainRef is invalid",None))}self.chain_row(&snapshot,p["targetChainRef"].as_str().unwrap())?.chain};
                    if !is_non_empty_string(&target["objectIdentity"],256){return Err(LiveError::error("move target identity is not authoritative"))}if p["index"].as_f64().unwrap()>target["devices"].as_array().map_or(0,Vec::len)as f64{return Ok(error(id,-32602,"index exceeds the exact target sibling collection",None))}
                    // Its own track or chain would only reorder it, which Live's move can't undo here.
                    if target["ref"]==json!(row.owner_ref){return Ok(error(id,-32602,"the device is already on that track or chain: move-cross moves a device to another one",None))}
                    target_siblings=siblings_of(&target);
                    let index=row.siblings.iter().position(|s|s["ref"]==p["ref"]).unwrap_or(0);payload=fields(p,&["action","ref","index","targetTrackRef","targetChainRef"]);payload["expectedObjectIdentity"]=row.device["objectIdentity"].clone();payload["expectedOwnerRef"]=json!(row.owner_ref);payload["expectedOwnerIdentity"]=json!(row.owner_identity);payload["expectedSiblings"]=json!(row.siblings);payload["expectedTrackRef"]=row.track["ref"].clone();payload["expectedTrackIdentity"]=row.track["objectIdentity"].clone();payload["expectedTargetIdentity"]=target["objectIdentity"].clone();payload["priorOwnerRef"]=json!(row.owner_ref);payload["priorIndex"]=json!(index);prior=json!({"ownerRef":row.owner_ref,"ownerIdentity":row.owner_identity,"ownerKind":if row.track["ref"]==row.owner_ref{"track"}else{"chain"},"index":index});impact="moves-device-cross-target";
                }
            }
            let mut t=json!({"id":tempo::transaction_id("devadv"),"epoch":status.epoch,"kind":"device-advanced","fence":js_json::stringify(&json!({"action":p["action"],"payload":payload})),"payload":payload,"prior":prior,"expiresAt":kumi_common::time::now_ms_f64()+TRANSACTION_TTL_MS,"state":"previewed"});if !target_siblings.is_null(){t["targetSiblings"]=target_siblings}self.retain_bounded_transaction(&self.clip_lifecycle_transactions,t.clone(),"device advanced")?;Ok(success_text(id,&json!({"transactionId":t["id"],"epoch":t["epoch"],"action":p["action"],"prior":prior,"impact":impact,"confirmation":"apply","expiresAt":t["expiresAt"]})))
        }.await;
        result.unwrap_or_else(|e| adapter_tool_error(id, &e, "Device-advanced preview requires fresh authoritative state."))
    }
    pub async fn live_device_advanced_apply_async(&self, id: &Value, p: &Value, signal: Option<&Signal>) -> Option<Value> {
        if !valid_transaction_params(p, "apply") {
            return Some(error(id, -32602, "transactionId, confirmation=apply, and idempotencyKey are required", None));
        }
        let record = self.clip_lifecycle_transactions.get(p["transactionId"].as_str().unwrap());
        let t = record.as_ref().map(|r| r.borrow().clone()).unwrap_or(Value::Null);
        if t["kind"] != "device-advanced"
            || (t["state"] == "previewed" && t["expiresAt"].as_f64().is_some_and(|n| n <= kumi_common::time::now_ms_f64()))
        {
            return Some(transaction_error(id, "Unknown or expired device-advanced transaction"));
        }
        let record = record.unwrap();
        if t["state"] == "applied" && t["applyKey"] == p["idempotencyKey"] {
            let mut response = json!({"transactionId":t["id"],"state":"applied"});
            if let Some(v) = t.get("created") {
                response["created"] = v.clone()
            }
            response["idempotent"] = json!(true);
            return Some(success_text(id, &response));
        }
        let reconcile = t["state"] == "uncertain" && t["applyKey"] == p["idempotencyKey"];
        if t["state"] != "previewed" && !reconcile {
            return Some(transaction_error(id, "Transaction is no longer applicable"));
        }
        if signal.is_some_and(Signal::is_cancelled) {
            return None;
        }
        let result=async{
            if reconcile{self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS)))).await?;}let status=self.require_connected(Some("session.read"))?;if json!(status.epoch)!=t["epoch"]{return Ok(transaction_error(id,"Live connection epoch changed; preview again"))}
            let adapter=self.async_adapter();let context=self.transaction_context(p,signal,AUDITION_DEADLINE_MS);let action=t["payload"]["action"].as_str().unwrap_or("");
            // Where the index puts it: among the devices the preview showed on the target.
            if t["targetSiblings"].is_array()&&!reconcile{let snapshot=self.views.view_for(Some(&context),&[t["payload"]["targetTrackRef"].clone(),t["payload"]["targetChainRef"].clone()],None,&[]).await?;let target=if t["payload"]["targetTrackRef"].is_string(){snapshot.tracks.iter().flatten().find(|track|json!(track.ref_)==t["payload"]["targetTrackRef"]).map(|track|serde_json::to_value(track).unwrap()).unwrap_or(Value::Null)}else{self.chain_row(&snapshot,t["payload"]["targetChainRef"].as_str().unwrap_or(""))?.chain};if siblings_of(&target)!=t["targetSiblings"]{return Ok(transaction_error(id,"the devices on the target changed since the preview; preview again"))}}
            {let mut row=record.borrow_mut();row["state"]=json!("applying");row["applyKey"]=p["idempotencyKey"].clone()}
            let args=Value::Object(t["payload"].as_object().unwrap().iter().filter(|(k,_)|!["action","priorOwnerRef","priorIndex"].contains(&k.as_str())).map(|(k,v)|(k.clone(),v.clone())).collect());let operation=match action{"set-bank"=>"device.bank.set","re-enable-automation"=>"parameter.re-enable-automation","save-comparison"=>"device.comparison.save-to-slot","insert-chain"=>"device.insert",_=>"device.move"};let value=adapter.invoke_async(&LiveInvocation::new(operation,args),Some(&context)).await?;
            match action{
                "set-bank"=>if expect_result(&value,"changed")?!=true{return Err(LiveError::error("device bank selection was not confirmed"))},
                "re-enable-automation"=>if expect_result(&value,"done")?!=true{return Err(LiveError::error("automation re-enable was not confirmed"))},
                "save-comparison"=>if expect_result(&value,"done")?!=true{return Err(LiveError::error("comparison save was not confirmed"))},
                "insert-chain"=>{if !is_non_empty_string(expect_result(&value,"ref")?,256)||!is_non_empty_string(&value["objectIdentity"],256){return Err(LiveError::error("chain insertion did not return exact identity"))}record.borrow_mut()["created"]=value;},
                _=>{expect_result(&value,"index")?;if !strict(value.get("index"),t["payload"].get("index"))||!is_non_empty_string(&value["objectIdentity"],256){return Err(LiveError::error("cross-target device move was not confirmed"))}record.borrow_mut()["created"]=value;}
            }
            {let mut row=record.borrow_mut();row["applyKey"]=p["idempotencyKey"].clone();row["state"]=json!("applied")}Ok(success_text(id,&json!({"transactionId":t["id"],"state":"applied","result":record.borrow().get("created").filter(|v|!v.is_null()).cloned().unwrap_or(json!({"done":true})),"idempotent":false})))
        }.await;
        Some(result.unwrap_or_else(|e| {
            self.apply_failed(id, &record, &e, "Device-advanced state is uncertain; perform fresh discovery before retrying.")
        }))
    }
    pub async fn undo_device_advanced_async(&self, id: &Value, p: &Value, signal: Option<&Signal>) -> Value {
        let record = p["transactionId"].as_str().and_then(|id| self.clip_lifecycle_transactions.get(id));
        let t = record.as_ref().map(|r| r.borrow().clone()).unwrap_or(Value::Null);
        if t["kind"] != "device-advanced" {
            return transaction_error(id, "Unknown or expired device-advanced transaction");
        }
        let record = record.unwrap();
        let action = t["payload"]["action"].as_str().unwrap_or("");
        if ["re-enable-automation", "save-comparison", "set-bank"].contains(&action) {
            return transaction_error(id, "Momentary device actions and control-surface bank selection are not undoable");
        }
        if t["state"] == "undone" && t["undoKey"] == p["idempotencyKey"] {
            return success_text(id, &json!({"transactionId":t["id"],"state":"undone","idempotent":true}));
        }
        let reconcile = t["state"] == "uncertain" && t["undoKey"] == p["idempotencyKey"];
        let created = &t["created"];
        if action == "insert-chain" {
            if (t["state"] != "applied" && !reconcile)
                || !is_non_empty_string(&created["ref"], 256)
                || !is_non_empty_string(&created["objectIdentity"], 256)
            {
                return transaction_error(id, "Chain insertion lacks exact created device identity");
            }
        } else if (t["state"] != "applied" && !reconcile) || !truthy(&t["prior"]) {
            return transaction_error(id, "Only an applied or exact-key uncertain device-advanced transaction can be undone");
        }
        let result=async{
            self.begin_undo_recovery(&record,p["idempotencyKey"].as_str().unwrap_or(""))?;let status=self.require_connected(Some("session.read"))?;if json!(status.epoch)!=t["epoch"]{return Ok(transaction_error(id,"Live connection epoch changed; undo refused"))}let adapter=self.async_adapter();let context=self.transaction_context(p,signal,AUDITION_DEADLINE_MS);record.borrow_mut()["undoKey"]=p["idempotencyKey"].clone();if reconcile{self.replay_undo_recovery(&record,adapter.as_ref(),&context).await?;}
            if action=="insert-chain"{record.borrow_mut()["state"]=json!("undoing");self.delete_owned_device_async(adapter.as_ref(),created["ref"].as_str().unwrap(),created["objectIdentity"].as_str().unwrap(),&context,created["createdFingerprint"].as_str(),Some(&record),reconcile).await?;}
            else{
                let snapshot=self.views.view_for(Some(&context),&[created["ref"].clone(),t["prior"]["ownerRef"].clone()],None,&[]).await?;
                if !is_non_empty_string(&created["ref"],256)||!is_non_empty_string(&created["objectIdentity"],256){return Ok(transaction_error(id,"device move lacks exact applied identity"))}let row=self.device_row(&snapshot,created["ref"].as_str().unwrap())?;if row.device["objectIdentity"]!=created["objectIdentity"]{return Ok(transaction_error(id,"the moved device changed after apply; undo refused"))}
                let prior=&t["prior"];let owner_kind=prior.get("ownerKind").filter(|v|!v.is_null()).cloned().unwrap_or(json!(if t["payload"]["expectedOwnerRef"]==t["payload"]["expectedTrackRef"]{"track"}else{"chain"}));let target=if owner_kind=="track"{snapshot.tracks.iter().flatten().find(|v|json!(v.ref_)==prior["ownerRef"]).map(|v|serde_json::to_value(v).unwrap()).unwrap_or(Value::Null)}else{self.chain_row(&snapshot,prior["ownerRef"].as_str().unwrap_or("")).map(|v|v.chain).unwrap_or(Value::Null)};
                let owner_identity=prior.get("ownerIdentity").filter(|v|!v.is_null()).unwrap_or(&t["payload"]["expectedOwnerIdentity"]);if !is_non_empty_string(&target["objectIdentity"],256)||target["objectIdentity"]!=*owner_identity{return Ok(transaction_error(id,"the device's original track or chain changed after apply; undo refused"))}
                record.borrow_mut()["state"]=json!("undoing");let mut args=json!({"ref":created["ref"],"index":prior["index"]});args[if owner_kind=="track"{"targetTrackRef"}else{"targetChainRef"}]=prior["ownerRef"].clone();extend(&mut args,&json!({"expectedObjectIdentity":row.device["objectIdentity"],"expectedOwnerRef":row.owner_ref,"expectedOwnerIdentity":row.owner_identity,"expectedSiblings":row.siblings,"expectedTrackRef":row.track["ref"],"expectedTrackIdentity":row.track["objectIdentity"],"expectedTargetIdentity":target["objectIdentity"]}));let result=self.invoke_undo_recovery(&record,adapter.as_ref(),"device.move",&args,&context).await?;
                expect_result(&result,"index")?;if !strict(result.get("index"),prior.get("index"))||result["objectIdentity"]!=created["objectIdentity"]{return Err(LiveError::error("device move-back was not confirmed"))}
            }
            record.borrow_mut()["state"]=json!("undone");Ok(success_text(id,&json!({"transactionId":t["id"],"state":"undone","idempotent":false})))
        }.await;
        result.unwrap_or_else(|e| {
            record.borrow_mut()["state"] = json!("uncertain");
            adapter_tool_error(
                id,
                &e,
                if action == "insert-chain" {
                    "Chain-insertion undo is uncertain; inspect the exact created device."
                } else {
                    "Device-advanced undo is uncertain; perform fresh discovery."
                },
            )
        })
    }
    pub async fn live_chain_preview_async(&self, id: &Value, p: &Value) -> Value {
        if !has_only(p, &["chainRef", "colorIndex", "autoColor", "mute", "solo"]) || !is_non_empty_string(&p["chainRef"], 256) {
            return error(id, -32602, "chainRef is required", None);
        }
        let mut proposed = json!({});
        for field in ["colorIndex", "autoColor", "mute", "solo"] {
            let Some(v) = p.get(field) else { continue };
            if field == "colorIndex" && !is_integer_in_range(v, 0., 69.) {
                return error(id, -32602, "colorIndex is invalid", None);
            }
            if field != "colorIndex" && !v.is_boolean() {
                return error(id, -32602, &format!("{field} must be boolean"), None);
            }
            proposed[field] = v.clone()
        }
        if proposed.as_object().unwrap().is_empty() {
            return error(id, -32602, "at least one chain field is required", None);
        }
        let result=async{let status=self.advanced_status().await?;if !status.has_operation("chain.set"){return Err(LiveError::error("chain editing is unavailable"))}let snapshot=self.views.view_for(None,&[p["chainRef"].clone()],None,&[]).await?;let found=self.chain_row(&snapshot,p["chainRef"].as_str().unwrap())?;let state=chain_state(&found.chain);let prior=Value::Object(proposed.as_object().unwrap().keys().map(|k|(k.clone(),state[k].clone())).collect());let mut payload=json!({"ref":p["chainRef"]});extend(&mut payload,&proposed);if let Some(v)=found.chain.get("objectIdentity"){payload["expectedObjectIdentity"]=v.clone()}payload["expectedStateRevision"]=json!(digest(&state)?);let t=json!({"id":tempo::transaction_id("chainset"),"epoch":status.epoch,"kind":"chain-set","fence":chain_fence(&p["chainRef"],&found.chain),"clipRef":p["chainRef"],"payload":payload,"prior":prior,"expiresAt":kumi_common::time::now_ms_f64()+TRANSACTION_TTL_MS,"state":"previewed"});self.retain_bounded_transaction(&self.clip_lifecycle_transactions,t.clone(),"chain edit")?;Ok(success_text(id,&json!({"transactionId":t["id"],"epoch":t["epoch"],"chainRef":p["chainRef"],"prior":prior,"proposed":proposed,"impact":"edits-chain","confirmation":"apply","expiresAt":t["expiresAt"]})))}.await;
        result.unwrap_or_else(|e| adapter_tool_error(id, &e, "Chain preview requires fresh authoritative state."))
    }
    pub async fn live_chain_apply_async(&self, id: &Value, p: &Value, signal: Option<&Signal>) -> Option<Value> {
        if !valid_transaction_params(p, "apply") {
            return Some(error(id, -32602, "transactionId, confirmation=apply, and idempotencyKey are required", None));
        }
        let record = self.clip_lifecycle_transactions.get(p["transactionId"].as_str().unwrap());
        let t = record.as_ref().map(|r| r.borrow().clone()).unwrap_or(Value::Null);
        if t["kind"] != "chain-set"
            || (t["state"] == "previewed" && t["expiresAt"].as_f64().is_some_and(|n| n <= kumi_common::time::now_ms_f64()))
        {
            return Some(transaction_error(id, "Unknown or expired chain transaction"));
        }
        let record = record.unwrap();
        if t["state"] == "applied" && t["applyKey"] == p["idempotencyKey"] {
            return Some(success_text(id, &json!({"transactionId":t["id"],"state":"applied","idempotent":true})));
        }
        let reconcile = t["state"] == "uncertain" && t["applyKey"] == p["idempotencyKey"];
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
            if json!(status.epoch) != t["epoch"] {
                return Ok(transaction_error(id, "Live connection epoch changed; preview again"));
            }
            let adapter = self.async_adapter();
            let context = self.transaction_context(p, signal, AUDITION_DEADLINE_MS);
            let reference = t["clipRef"].as_str().unwrap_or("");
            if !reconcile {
                let snapshot = self.views.view_for(Some(&context), &[t["clipRef"].clone()], None, &[]).await?;
                let found = self.chain_row(&snapshot, reference)?;
                if t["fence"] != chain_fence(&t["clipRef"], &found.chain) {
                    return Ok(transaction_error(id, "chain identity or state changed since preview; preview again"));
                }
            }
            {
                let mut row = record.borrow_mut();
                row["state"] = json!("applying");
                row["applyKey"] = p["idempotencyKey"].clone()
            }
            let result = adapter.invoke_async(&LiveInvocation::new("chain.set", t["payload"].clone()), Some(&context)).await?;
            if expect_result(&result, "changed")? != true {
                return Err(LiveError::error("chain change was not confirmed"));
            }
            let snapshot = self.views.view_for(Some(&context), &[t["clipRef"].clone()], None, &[]).await?;
            let verified = self.chain_row(&snapshot, reference)?.chain;
            for field in ["colorIndex", "autoColor", "mute", "solo"] {
                if let Some(v) = t["payload"].get(field) {
                    if !strict(verified.get(field), Some(v)) {
                        return Err(LiveError::error("chain postcondition was not confirmed"));
                    }
                }
            }
            {
                let mut row = record.borrow_mut();
                row["applyKey"] = p["idempotencyKey"].clone();
                row["state"] = json!("applied")
            }
            let mut response = json!({"transactionId":t["id"],"state":"applied"});
            if let Some(v) = result.get("revision") {
                response["revision"] = v.clone()
            }
            response["idempotent"] = json!(false);
            Ok(success_text(id, &response))
        }
        .await;
        Some(
            result.unwrap_or_else(|e| {
                self.apply_failed(id, &record, &e, "Chain state is uncertain; perform fresh discovery before retrying.")
            }),
        )
    }
    pub async fn undo_chain_async(&self, id: &Value, p: &Value, signal: Option<&Signal>) -> Value {
        let record = p["transactionId"].as_str().and_then(|id| self.clip_lifecycle_transactions.get(id));
        let t = record.as_ref().map(|r| r.borrow().clone()).unwrap_or(Value::Null);
        if t["kind"] != "chain-set" {
            return transaction_error(id, "Unknown or expired chain transaction");
        }
        let record = record.unwrap();
        if t["state"] == "undone" && t["undoKey"] == p["idempotencyKey"] {
            return success_text(id, &json!({"transactionId":t["id"],"state":"undone","idempotent":true}));
        }
        let reconcile = t["state"] == "uncertain" && t["undoKey"] == p["idempotencyKey"];
        if (t["state"] != "applied" && !reconcile) || !truthy(&t["clipRef"]) || !truthy(&t["prior"]) {
            return transaction_error(id, "Only an applied or exact-key uncertain chain transaction can be undone");
        }
        let result = async {
            self.begin_undo_recovery(&record, p["idempotencyKey"].as_str().unwrap_or(""))?;
            let status = self.require_connected(Some("session.read"))?;
            if json!(status.epoch) != t["epoch"] {
                return Ok(transaction_error(id, "Live connection epoch changed; undo refused"));
            }
            let adapter = self.async_adapter();
            let context = self.transaction_context(p, signal, AUDITION_DEADLINE_MS);
            record.borrow_mut()["undoKey"] = p["idempotencyKey"].clone();
            if reconcile {
                self.replay_undo_recovery(&record, adapter.as_ref(), &context).await?;
            }
            let snapshot = self.views.view_for(Some(&context), &[t["clipRef"].clone()], None, &[]).await?;
            let found = self.chain_row(&snapshot, t["clipRef"].as_str().unwrap_or(""))?;
            if let Some(moved) = self.undo_target_moved(
                id,
                &record.borrow(),
                "chain",
                &t["clipRef"],
                found.chain.get("objectIdentity"),
                t["payload"].get("expectedObjectIdentity"),
            )? {
                return Ok(moved);
            }
            if !reconcile {
                for (k, v) in t["payload"].as_object().unwrap() {
                    if ["ref", "expectedObjectIdentity", "expectedStateRevision"].contains(&k.as_str()) {
                        continue;
                    }
                    if !strict(found.chain.get(k), Some(v)) {
                        return Ok(transaction_error(id, "chain changed after apply; undo refused"));
                    }
                }
            }
            record.borrow_mut()["state"] = json!("undoing");
            let mut args = json!({"ref":t["clipRef"]});
            extend(&mut args, &t["prior"]);
            if let Some(v) = found.chain.get("objectIdentity") {
                args["expectedObjectIdentity"] = v.clone()
            }
            args["expectedStateRevision"] = json!(digest(&chain_state(&found.chain))?);
            let result = self.invoke_undo_recovery(&record, adapter.as_ref(), "chain.set", &args, &context).await?;
            if expect_result(&result, "changed")? != true {
                return Err(LiveError::error("chain restoration was not confirmed"));
            }
            record.borrow_mut()["state"] = json!("undone");
            Ok(success_text(id, &json!({"transactionId":t["id"],"state":"undone","idempotent":false})))
        }
        .await;
        result.unwrap_or_else(|e| {
            record.borrow_mut()["state"] = json!("uncertain");
            adapter_tool_error(id, &e, "Chain undo is uncertain; perform fresh discovery.")
        })
    }
}
