//! Device hierarchy authority, Browser loads, and transaction-created device cleanup.
use super::*;
use super::{
    device_parameter::{fields, DeviceRow},
    retention::TransactionRecord,
};
use kumi_common::{
    abort::Signal,
    js::{json as js_json, string::slice},
    time::now_ms_f64,
};
use std::time::Duration;

pub(super) struct ChainRow {
    pub track: Value,
    pub device: Value,
    pub chain: Value,
}
fn array(value: &Value) -> &[Value] {
    value.as_array().map(Vec::as_slice).unwrap_or(&[])
}
fn nullable_array<'a>(value: &'a Value, name: &str) -> Result<&'a [Value], LiveError> {
    if value.is_null() {
        return Ok(&[]);
    }
    value.as_array().map(Vec::as_slice).ok_or_else(|| LiveError::type_error(format!("({name} ?? []).filter is not a function")))
}
fn find_chain(devices: &Value, reference: &str) -> Result<Option<(Value, Value)>, LiveError> {
    for device in nullable_array(devices, "track.devices")?.iter().filter(|v| v.is_object()) {
        let chains: Vec<_> = nullable_array(&device["chains"], "device.chains")?.iter().filter(|v| v.is_object()).collect();
        if let Some(chain) = chains.iter().find(|v| v["ref"] == reference) {
            return Ok(Some((device.clone(), (*chain).clone())));
        }
        for chain in chains {
            if let Some(row) = find_chain(&chain["devices"], reference)? {
                return Ok(Some(row));
            }
        }
        for pad in nullable_array(&device["drumPads"], "device.drumPads")?.iter().filter(|v| v.is_object()) {
            for chain in nullable_array(&pad["chains"], "pad.chains")?.iter().filter(|v| v.is_object()) {
                if chain["ref"] == reference {
                    return Ok(Some((device.clone(), chain.clone())));
                }
                if let Some(row) = find_chain(&chain["devices"], reference)? {
                    return Ok(Some(row));
                }
            }
        }
    }
    Ok(None)
}
fn name(value: &Value) -> Result<String, LiveError> {
    Ok(slice(&if value.is_null() { String::new() } else { js_string(value)? }, 0, Some(64)))
}
fn names(value: &Value) -> Result<Vec<String>, LiveError> {
    array(value).iter().filter(|v| v.is_object()).take(16).map(|v| name(&v["name"])).collect()
}
fn authority_siblings(value: &Value) -> Result<Vec<Value>, LiveError> {
    array(value)
        .iter()
        .map(|d| {
            if !d.is_object() || !is_non_empty_string(&d["ref"], 256) || !is_non_empty_string(&d["objectIdentity"], 256) {
                return Err(LiveError::error("device sibling identity is incomplete"));
            }
            Ok(fields(d, &["ref", "objectIdentity"]))
        })
        .collect()
}
fn track_fence(track: &Value, authority: &Value) -> String {
    let mut result = json!({"track":track});
    result.as_object_mut().unwrap().extend(authority.as_object().unwrap().clone());
    js_json::stringify(&result)
}
impl McpHost {
    pub(super) fn chain_row(&self, snapshot: &LiveSnapshot, reference: &str) -> Result<ChainRow, LiveError> {
        let snapshot = serde_json::to_value(snapshot).unwrap();
        for track in array(&snapshot["tracks"]) {
            if let Some((device, chain)) = find_chain(&track["devices"], reference)? {
                return Ok(ChainRow { track: track.clone(), device, chain });
            }
        }
        Err(LiveError::error("chain reference is not authoritative"))
    }
    pub(super) fn chain_on_track(&self, snapshot: &LiveSnapshot, reference: &str) -> Result<ChainRow, LiveError> {
        let snapshot = serde_json::to_value(snapshot).unwrap();
        for track in array(&snapshot["tracks"]) {
            if let Ok(Some((device, chain))) = find_chain(&track["devices"], reference) {
                return Ok(ChainRow { track: track.clone(), device, chain });
            }
        }
        Err(LiveError::error("chain reference is not authoritative"))
    }
    pub(super) fn track_device_authority(&self, track: &Value) -> Result<Value, LiveError> {
        if !is_non_empty_string(&track["objectIdentity"], 256) || !track["devices"].is_array() {
            return Err(LiveError::error("track device authority is incomplete"));
        }
        Ok(json!({"expectedTrackIdentity":track["objectIdentity"],"expectedSiblings":authority_siblings(&track["devices"])?}))
    }
    pub(super) fn chain_device_authority(&self, track: &Value, chain: &Value) -> Result<Value, LiveError> {
        if !is_non_empty_string(&track["objectIdentity"], 256)
            || !is_non_empty_string(&chain["ref"], 256)
            || !is_non_empty_string(&chain["objectIdentity"], 256)
            || (!chain["devices"].is_null() && !chain["devices"].is_array())
        {
            return Err(LiveError::error("chain device authority is incomplete"));
        }
        Ok(
            json!({"expectedTrackIdentity":track["objectIdentity"],"chainRef":chain["ref"],"expectedChainIdentity":chain["objectIdentity"],"expectedSiblings":authority_siblings(&chain["devices"])?}),
        )
    }
    pub(super) fn device_fence(&self, row: &DeviceRow) -> String {
        js_json::stringify(
            &json!({"ref":row.device["ref"],"objectIdentity":row.device["objectIdentity"],"track":row.track["ref"],"ownerRef":row.owner_ref,"ownerIdentity":row.owner_identity,"siblings":row.siblings,"enabled":row.device["enabled"]}),
        )
    }
    pub(super) fn device_placement(&self, snapshot: &LiveSnapshot, row: &DeviceRow) -> Result<Value, LiveError> {
        let index = row.siblings.iter().position(|v| v["ref"] == row.device["ref"]).map(|n| n as i64).unwrap_or(-1);
        if row.track["ref"] == row.owner_ref {
            return Ok(json!({"owner":"track","devices":names(&row.track["devices"])?,"index":index}));
        }
        let result = (|| {
            let located = self.chain_on_track(snapshot, &row.owner_ref)?;
            let chains: Vec<_> = array(&located.device["chains"]).iter().filter(|v| v.is_object()).collect();
            let position = chains.iter().position(|v| v["ref"] == located.chain["ref"]).map(|n| n as i64).unwrap_or(-1);
            let rows = chains
                .iter()
                .take(8)
                .map(|v| Ok(json!({"name":name(&v["name"])?,"devices":names(&v["devices"])?})))
                .collect::<Result<Vec<Value>, LiveError>>()?;
            Ok::<_, LiveError>(json!({"owner":"chain","rack":name(&located.device["name"])?,"chain":position,"index":index,"chains":rows}))
        })();
        Ok(result.unwrap_or_else(|_| json!({"owner":"chain","devices":[],"index":index})))
    }
    pub(super) async fn delete_owned_device_async(
        &self,
        adapter: &dyn AsyncLiveAdapter,
        reference: &str,
        identity: &str,
        context: &LiveOperationContext,
        _fingerprint: Option<&str>,
        record: Option<&TransactionRecord>,
        allow_absent: bool,
    ) -> Result<(), LiveError> {
        let snapshot = self.views.view_for(Some(context), &[json!(reference)], None, &[]).await?;
        let located = match self.device_row(&snapshot, reference) {
            Ok(row) => row,
            Err(_) if allow_absent => return Ok(()),
            Err(e) => return Err(e),
        };
        if located.device["objectIdentity"] != identity {
            return Err(LiveError::error("owned device identity changed before cleanup"));
        }
        let args = json!({"ref":reference,"expectedObjectIdentity":identity,"expectedOwnerRef":located.owner_ref,"expectedOwnerIdentity":located.owner_identity,"expectedSiblings":located.siblings,"expectedTrackRef":located.track["ref"],"expectedTrackIdentity":located.track["objectIdentity"]});
        if let Some(record) = record {
            self.invoke_undo_recovery(record, adapter, "device.delete", &args, context).await?;
        } else {
            adapter.invoke_async(&LiveInvocation::new("device.delete", args), Some(context)).await?;
        }
        let remains = async {
            let snapshot = self.views.view_for(Some(context), &[located.track["ref"].clone()], None, &[]).await?;
            Ok::<_, LiveError>(self.device_row(&snapshot, reference)?.device["objectIdentity"] == identity)
        }
        .await;
        if remains.unwrap_or(false) {
            return Err(LiveError::error("owned device cleanup was not confirmed"));
        }
        Ok(())
    }
    pub async fn dispatch_device_lifecycle_tool(
        &self,
        call: &ToolCall,
        signal: Option<&Signal>,
    ) -> Option<Result<Option<Value>, LiveError>> {
        let p = call.arguments.as_ref().unwrap_or(&Value::Null);
        Some(Ok(match call.name.as_str() {
            "live_browser_load_preview" => Some(self.live_browser_load_preview_async(&call.id, p).await),
            "live_browser_load_apply" => self.live_browser_load_apply_async(&call.id, p, signal).await,
            _ => return None,
        }))
    }
    pub async fn live_browser_load_preview_async(&self, id: &Value, p: &Value) -> Value {
        if !has_only(p, &["itemId", "trackRef", "chainRef"])
            || !is_non_empty_string(&p["itemId"], 256)
            || (p.get("trackRef").is_none() && p.get("chainRef").is_none())
            || ["trackRef", "chainRef"].iter().any(|k| p.get(*k).is_some_and(|v| !is_non_empty_string(v, 256)))
        {
            return error(id, -32602, "itemId and a trackRef or chainRef are required", None);
        }
        let result=async{
            let status=self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(reads::AUDITION_DEADLINE_MS)))).await?;
            if !status.connected||!status.capabilities.iter().any(|c|c.as_str()=="session.read"){return Err(LiveError::error("session read capability is unavailable"));}
            if !status.has_operation("browser.load")||!status.has_operation("browser.inspect"){return Err(LiveError::error("browser loading or item inspection is unavailable"));}
            let item=self.async_adapter().invoke_async(&LiveInvocation::new("browser.inspect",json!({"itemId":p["itemId"]})),Some(&LiveOperationContext::with_deadline(self.deadline(reads::AUDITION_DEADLINE_MS)))).await?;
            if item.is_null(){return Err(LiveError::type_error("Cannot read properties of null (reading 'id')"));}
            if item["id"]!=p["itemId"]||item["isDevice"]!=true||!item["name"].is_string()||!is_non_empty_string(&item["objectIdentity"],256){return Err(LiveError::error("browser item lacks exact track-loadable identity"));}
            let snapshot=self.views.view_for(None,&[p["chainRef"].clone(),p["trackRef"].clone()],None,&[]).await?;
            let target=p.get("chainRef").map(|v|self.chain_on_track(&snapshot,v.as_str().unwrap())).transpose()?;
            if target.as_ref().is_some_and(|t|p.get("trackRef").is_some_and(|r|r!=&t.track["ref"])){return Err(LiveError::error("browser target chain isn't on that track"));}
            let reference=target.as_ref().map(|t|t.track["ref"].clone()).unwrap_or_else(||p["trackRef"].clone());
            let snapshot_value=serde_json::to_value(&snapshot).unwrap();let track=array(&snapshot_value["tracks"]).iter().find(|t|t["ref"]==reference);
            let Some(track)=track.filter(|t|["regular","group","audio","midi","return","main"].contains(&t["kind"].as_str().unwrap_or("")))else{return Err(LiveError::error("browser loading is limited to the Set's tracks"));};
            let authority=if let Some(t)=&target{self.chain_device_authority(track,&t.chain)?}else{self.track_device_authority(track)?};
            let mut payload=json!({"itemId":p["itemId"],"trackRef":reference,"expectedName":item["name"],"expectedItemIdentity":item["objectIdentity"]});payload.as_object_mut().unwrap().extend(authority.as_object().unwrap().clone());
            let transaction=json!({"id":tempo::transaction_id("browserload"),"epoch":status.epoch,"kind":"browser-load","fence":track_fence(&reference,&authority),"clipRef":reference,"payload":payload,"expiresAt":now_ms_f64()+TRANSACTION_TTL_MS,"state":"previewed"});
            self.retain_bounded_transaction(&self.clip_lifecycle_transactions,transaction.clone(),"browser load")?;
            let mut item=fields(&item,&["id","name","path","category"]);item["isDevice"]=json!(true);
            let mut reply=json!({"transactionId":transaction["id"],"epoch":transaction["epoch"],"item":item,"trackRef":reference});
            if let Some(target)=target{reply["chainRef"]=target.chain["ref"].clone();reply["chainName"]=target.chain["name"].as_str().map(|s|json!(s)).unwrap_or(Value::Null);reply["rackName"]=target.device["name"].as_str().map(|s|json!(s)).unwrap_or(Value::Null);}
            reply["impact"]=json!("loads-browser-device");reply["confirmation"]=json!("apply");reply["expiresAt"]=transaction["expiresAt"].clone();Ok(success_text(id,&reply))
        }.await;
        result.unwrap_or_else(|e| adapter_tool_error(id, &e, "Browser-load preview requires fresh authoritative state."))
    }
    pub async fn live_browser_load_apply_async(&self, id: &Value, p: &Value, signal: Option<&Signal>) -> Option<Value> {
        if !valid_transaction_params(p, "apply") {
            return Some(error(id, -32602, "transactionId, confirmation=apply, and idempotencyKey are required", None));
        }
        let Some(record) = self.clip_lifecycle_transactions.get(p["transactionId"].as_str().unwrap()).filter(|r| {
            let t = r.borrow();
            t["kind"] == "browser-load" && !(t["state"] == "previewed" && t["expiresAt"].as_f64().unwrap_or(0.) <= now_ms_f64())
        }) else {
            return Some(transaction_error(id, "Unknown or expired browser-load transaction"));
        };
        let t = record.borrow().clone();
        if t["state"] == "applied" && t["applyKey"] == p["idempotencyKey"] {
            let mut out = json!({"transactionId":t["id"],"state":"applied"});
            if let Some(created) = t.get("created") {
                out["created"] = created.clone();
            }
            out["idempotent"] = json!(true);
            return Some(success_text(id, &out));
        }
        let reconciliation = t["state"] == "uncertain" && t["applyKey"] == p["idempotencyKey"];
        if t["state"] != "previewed" && !reconciliation {
            return Some(transaction_error(id, "Transaction is no longer applicable"));
        }
        if signal.is_some_and(Signal::is_cancelled) {
            return None;
        }
        let result=async{
            if reconciliation{self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(reads::AUDITION_DEADLINE_MS)))).await?;}
            let status=self.require_connected(Some("session.read"))?;if json!(status.epoch)!=t["epoch"]{return Ok(transaction_error(id,"Live connection epoch changed; preview again"));}
            let adapter=self.async_adapter();let context=self.transaction_context(p,signal,reads::AUDITION_DEADLINE_MS);
            if !reconciliation{
                let snapshot=self.views.view_for(Some(&context),&[t["payload"]["trackRef"].clone(),t["payload"]["chainRef"].clone()],None,&[]).await?;
                let value=serde_json::to_value(&snapshot).unwrap();let track=array(&value["tracks"]).iter().find(|r|r["ref"]==t["payload"]["trackRef"]);
                let current=track.and_then(|track|{let authority=if let Some(chain)=t["payload"]["chainRef"].as_str(){self.chain_on_track(&snapshot,chain).and_then(|c|self.chain_device_authority(track,&c.chain))}else{self.track_device_authority(track)};authority.ok().map(|a|track_fence(&t["payload"]["trackRef"],&a))});
                if !track.is_some_and(|r|["regular","group","audio","midi","return","main"].contains(&r["kind"].as_str().unwrap_or("")))||current.as_deref()!=t["fence"].as_str(){return Ok(transaction_error(id,"track identity or devices changed since preview; preview again"));}
            }
            record.borrow_mut()["state"]=json!("applying");record.borrow_mut()["applyKey"]=p["idempotencyKey"].clone();
            let loaded=adapter.invoke_async(&LiveInvocation::new("browser.load",t["payload"].clone()),Some(&context)).await?;
            if loaded.is_null(){return Err(LiveError::type_error("Cannot read properties of null (reading 'loaded')"));}
            if loaded["loaded"]!=true||!is_non_empty_string(&loaded["deviceRef"],256)||!is_non_empty_string(&loaded["deviceObjectIdentity"],256)||!is_non_empty_string(&loaded["createdFingerprint"],64){return Err(LiveError::error("browser load did not return exact created device identity"));}
            let snapshot=self.views.view_for(Some(&context),&[loaded["deviceRef"].clone(),t["payload"]["trackRef"].clone(),t["payload"]["chainRef"].clone()],None,&[]).await?;
            let reference=loaded["deviceRef"].as_str().unwrap();let created=self.device_row(&snapshot,reference)?;
            if created.device["objectIdentity"]!=loaded["deviceObjectIdentity"]{return Err(LiveError::error("browser-loaded device identity was not confirmed"));}
            if t["payload"]["chainRef"].is_string()&&t["payload"]["chainRef"]!=created.owner_ref{return Err(LiveError::error("browser-loaded device is not in the requested chain"));}
            let mut fingerprint=arrangement::capture_object_fingerprint(&owned_device_fingerprint_row(&created.device))?;
            if json!(fingerprint)!=loaded["createdFingerprint"]{
                let mut previous=fingerprint.clone();let mut settled=false;
                for _ in 0..12{
                    tokio::time::sleep(Duration::from_millis(250)).await;
                    let snapshot=self.views.view_for(Some(&context),&[loaded["deviceRef"].clone(),t["payload"]["trackRef"].clone()],None,&[]).await?;let again=self.device_row(&snapshot,reference)?;
                    if again.device["objectIdentity"]!=loaded["deviceObjectIdentity"]{return Err(LiveError::error("browser-loaded device identity was not confirmed"));}
                    fingerprint=arrangement::capture_object_fingerprint(&owned_device_fingerprint_row(&again.device))?;
                    settled=fingerprint==previous;previous=fingerprint.clone();if settled{break;}
                }
                if !settled{return Err(LiveError::error("browser-loaded device creation fingerprint was not confirmed"));}
                if status.has_operation("ownership.settle"){
                    let recorded=adapter.invoke_async(&LiveInvocation::new("ownership.settle",json!({"ref":loaded["deviceRef"],"expectedObjectIdentity":loaded["deviceObjectIdentity"],"expectedFingerprint":fingerprint})),Some(&context)).await?;
                    if recorded.is_null(){return Err(LiveError::type_error("Cannot read properties of null (reading 'settled')"));}
                    if recorded["settled"]!=true||recorded["fingerprint"]!=fingerprint{return Err(LiveError::error("browser-loaded device settled state was not recorded"));}
                }
            }
            record.borrow_mut()["created"]=json!({"deviceRef":loaded["deviceRef"],"objectIdentity":loaded["deviceObjectIdentity"],"fingerprint":fingerprint});record.borrow_mut()["applyKey"]=p["idempotencyKey"].clone();record.borrow_mut()["state"]=json!("applied");
            Ok(success_text(id,&json!({"transactionId":t["id"],"state":"applied","deviceRef":loaded["deviceRef"],"placement":self.device_placement(&snapshot,&created)?,"idempotent":false})))
        }.await;
        Some(result.unwrap_or_else(|e| {
            // Nothing was sent before applying: it stays as it was, to try again.
            if record.borrow()["state"] == "applying" {
                record.borrow_mut()["state"] = json!("uncertain");
            }
            adapter_tool_error(id, &e, "Browser load is uncertain; perform fresh discovery before retrying.")
        }))
    }
    pub async fn undo_browser_load_async(&self, id: &Value, p: &Value, signal: Option<&Signal>) -> Value {
        let Some(record) =
            self.clip_lifecycle_transactions.get(p["transactionId"].as_str().unwrap()).filter(|r| r.borrow()["kind"] == "browser-load")
        else {
            return transaction_error(id, "Unknown Browser-load transaction");
        };
        let t = record.borrow().clone();
        if t["state"] == "undone" && t["undoKey"] == p["idempotencyKey"] {
            return success_text(id, &json!({"transactionId":t["id"],"state":"undone","idempotent":true}));
        }
        let reconciliation = t["state"] == "uncertain" && t["undoKey"] == p["idempotencyKey"];
        if (t["state"] != "applied" && !reconciliation)
            || !is_non_empty_string(&t["created"]["deviceRef"], 256)
            || !is_non_empty_string(&t["created"]["objectIdentity"], 256)
        {
            return transaction_error(id, "Browser load lacks exact created device identity");
        }
        let result = async {
            self.begin_undo_recovery(&record, p["idempotencyKey"].as_str().unwrap())?;
            let status = self.require_connected(Some("session.read"))?;
            if json!(status.epoch) != t["epoch"] {
                return Ok(transaction_error(id, "Live connection epoch changed; undo refused"));
            }
            let adapter = self.async_adapter();
            let context = self.transaction_context(p, signal, reads::AUDITION_DEADLINE_MS);
            record.borrow_mut()["undoKey"] = p["idempotencyKey"].clone();
            if reconciliation {
                self.replay_undo_recovery(&record, adapter.as_ref(), &context).await?;
            }
            record.borrow_mut()["state"] = json!("undoing");
            self.delete_owned_device_async(
                adapter.as_ref(),
                t["created"]["deviceRef"].as_str().unwrap(),
                t["created"]["objectIdentity"].as_str().unwrap(),
                &context,
                t["created"]["fingerprint"].as_str(),
                Some(&record),
                reconciliation,
            )
            .await?;
            record.borrow_mut()["state"] = json!("undone");
            Ok(success_text(id, &json!({"transactionId":t["id"],"state":"undone","idempotent":false})))
        }
        .await;
        result.unwrap_or_else(|e| {
            record.borrow_mut()["state"] = json!("uncertain");
            adapter_tool_error(id, &e, "Browser-load undo is uncertain; inspect the exact created device.")
        })
    }
}
