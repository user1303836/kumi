//! Copy a device beside itself and retain exact identity for its deletion.
use super::device_parameter::{fields, DeviceRow};
use super::*;
use kumi_common::{
    abort::Signal,
    js::{json as js_json, string::slice},
    time::now_ms_f64,
};
fn copy_fence(reference: &Value, row: &DeviceRow) -> String {
    js_json::stringify(
        &json!({"ref":reference,"objectIdentity":row.device["objectIdentity"],"ownerRef":row.owner_ref,"ownerIdentity":row.owner_identity,"siblings":row.siblings,"trackRef":row.track["ref"],"trackIdentity":row.track["objectIdentity"]}),
    )
}
impl McpHost {
    pub async fn dispatch_device_copy_tool(&self, call: &ToolCall, signal: Option<&Signal>) -> Option<Result<Option<Value>, LiveError>> {
        let p = call.arguments.as_ref().unwrap_or(&Value::Null);
        Some(Ok(match call.name.as_str() {
            "live_device_duplicate_preview" => Some(self.live_device_duplicate_preview_async(&call.id, p).await),
            "live_device_duplicate_apply" => self.live_device_duplicate_apply_async(&call.id, p, signal).await,
            _ => return None,
        }))
    }
    pub async fn live_device_duplicate_preview_async(&self, id: &Value, p: &Value) -> Value {
        if !has_only(p, &["deviceRef"]) || !is_non_empty_string(&p["deviceRef"], 256) {
            return error(id, -32602, "deviceRef is required", None);
        }
        let result=async{
            let status=self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(reads::AUDITION_DEADLINE_MS)))).await?;
            if !status.has_operation("device.duplicate")||!status.has_operation("device.delete"){return Err(LiveError::error("copying a device is unavailable on this Live shape"));}
            let snapshot=self.views.view_for(Some(&LiveOperationContext::with_deadline(self.deadline(reads::AUDITION_DEADLINE_MS))),&[p["deviceRef"].clone()],None,&[]).await?;
            let row=self.device_row(&snapshot,p["deviceRef"].as_str().unwrap())?;
            if !is_non_empty_string(&row.device["objectIdentity"],256){return Err(LiveError::error("device identity is not authoritative"));}
            if row.device["deviceType"]=="instrument"{
                let name=if row.device["name"].is_null(){String::new()}else{js_string(&row.device["name"])?};
                return Ok(adapter_tool_error(id,&LiveError::error(format!("a chain holds one instrument, so Live can't copy \"{}\" beside itself",slice(&name,0,Some(80)))),"No device was copied: duplicate its track instead."));
            }
            let mut payload=json!({"ref":p["deviceRef"]});if let Some(name)=row.device.get("name"){payload["expectedName"]=name.clone();}
            payload["expectedObjectIdentity"]=row.device["objectIdentity"].clone();payload["expectedOwnerRef"]=json!(row.owner_ref);payload["expectedOwnerIdentity"]=json!(row.owner_identity);payload["expectedSiblings"]=json!(row.siblings);
            let t=json!({"id":tempo::transaction_id("devdup"),"epoch":status.epoch,"kind":"device-duplicate","fence":copy_fence(&p["deviceRef"],&row),"clipRef":p["deviceRef"],"payload":payload,"prior":{"trackRef":row.track["ref"],"siblings":row.siblings},"expiresAt":now_ms_f64()+TRANSACTION_TTL_MS,"state":"previewed"});
            self.retain_bounded_transaction(&self.clip_lifecycle_transactions,t.clone(),"device copy")?;
            let mut device=json!({"ref":p["deviceRef"]});device.as_object_mut().unwrap().extend(fields(&row.device,&["name","kind"]).as_object().unwrap().clone());
            Ok(success_text(id,&json!({"transactionId":t["id"],"epoch":t["epoch"],"device":device,"trackRef":row.track["ref"],"impact":"copies-device","confirmation":"apply","expiresAt":t["expiresAt"]})))
        }.await;
        result.unwrap_or_else(|e| adapter_tool_error(id, &e, "No device was copied; discover it again and preview from fresh references."))
    }
    pub async fn live_device_duplicate_apply_async(&self, id: &Value, p: &Value, signal: Option<&Signal>) -> Option<Value> {
        if !valid_transaction_params(p, "apply") {
            return Some(error(id, -32602, "transactionId, confirmation=apply, and idempotencyKey are required", None));
        }
        let Some(record) = self.clip_lifecycle_transactions.get(p["transactionId"].as_str().unwrap()).filter(|r| {
            let t = r.borrow();
            t["kind"] == "device-duplicate" && !(t["state"] == "previewed" && t["expiresAt"].as_f64().unwrap_or(0.) <= now_ms_f64())
        }) else {
            return Some(transaction_error(id, "Unknown or expired device-copy transaction"));
        };
        let t = record.borrow().clone();
        if t["state"] == "applied" && t["applyKey"] == p["idempotencyKey"] {
            let mut result = json!({"transactionId":t["id"],"state":"applied"});
            if let Some(created) = t.get("created") {
                result["created"] = created.clone();
            }
            result["idempotent"] = json!(true);
            return Some(success_text(id, &result));
        }
        if t["state"] != "previewed" {
            return Some(transaction_error(id, "Transaction is no longer applicable: look at the device's chain, then preview again"));
        }
        if signal.is_some_and(Signal::is_cancelled) {
            return None;
        }
        let reference = &t["payload"]["ref"];
        let result = async {
            let status = self.require_connected(None)?;
            if json!(status.epoch) != t["epoch"] {
                return Ok(transaction_error(id, "Live connection epoch changed; preview again"));
            }
            let context = self.transaction_context(p, signal, reads::AUDITION_DEADLINE_MS);
            let snapshot = self.views.view_for(Some(&context), &[reference.clone()], None, &[]).await?;
            let current = self.device_row(&snapshot, reference.as_str().unwrap())?;
            if json!(copy_fence(reference, &current)) != t["fence"] {
                return Ok(transaction_error(id, "the device or its chain changed since the preview; preview again"));
            }
            record.borrow_mut()["state"] = json!("applying");
            record.borrow_mut()["applyKey"] = p["idempotencyKey"].clone();
            let result =
                self.async_adapter().invoke_async(&LiveInvocation::new("device.duplicate", t["payload"].clone()), Some(&context)).await?;
            if result.is_null() {
                return Err(LiveError::type_error("Cannot read properties of null (reading 'ref')"));
            }
            if !is_non_empty_string(&result["ref"], 256) {
                return Err(LiveError::error("Live didn't say where the copy is"));
            }
            let snapshot = self.views.view_for(Some(&context), &[reference.clone(), result["ref"].clone()], None, &[]).await?;
            let copy = self.device_row(&snapshot, result["ref"].as_str().unwrap())?;
            let prior = t["prior"]["siblings"].as_array().unwrap();
            let index = copy.siblings.iter().position(|s| s["ref"] == copy.device["ref"]);
            if copy.siblings.len() != prior.len() + 1
                || index
                    .and_then(|n| n.checked_sub(1))
                    .and_then(|n| copy.siblings.get(n))
                    .is_none_or(|s| s["objectIdentity"] != current.device["objectIdentity"])
                || (is_non_empty_string(&result["objectIdentity"], 256) && result["objectIdentity"] != copy.device["objectIdentity"])
            {
                return Err(LiveError::error("the copy isn't straight after the device"));
            }
            let mut created = fields(&copy.device, &["ref", "objectIdentity", "name"]);
            created["index"] = json!(index.map(|n| n as i64).unwrap_or(-1));
            if is_non_empty_string(&result["createdFingerprint"], 64) {
                created["fingerprint"] = result["createdFingerprint"].clone();
            }
            record.borrow_mut()["created"] = created.clone();
            record.borrow_mut()["state"] = json!("applied");
            Ok(success_text(id, &json!({"transactionId":t["id"],"state":"applied","created":created,"idempotent":false})))
        }
        .await;
        Some(result.unwrap_or_else(|e| {
            apply_failed(id, &record, &e, "Whether the device was copied is uncertain: look at its chain before trying again.")
        }))
    }
    pub async fn undo_device_copy_async(&self, id: &Value, p: &Value, signal: Option<&Signal>) -> Value {
        let Some(record) =
            self.clip_lifecycle_transactions.get(p["transactionId"].as_str().unwrap()).filter(|r| r.borrow()["kind"] == "device-duplicate")
        else {
            return transaction_error(id, "Unknown or expired device-copy transaction");
        };
        let t = record.borrow().clone();
        if t["state"] == "undone" && t["undoKey"] == p["idempotencyKey"] {
            return success_text(id, &json!({"transactionId":t["id"],"state":"undone","idempotent":true}));
        }
        let reconciliation = t["state"] == "uncertain" && t["undoKey"] == p["idempotencyKey"];
        if (t["state"] != "applied" && !reconciliation) || !arrangement::truthy(&t["created"]) {
            return transaction_error(id, "Only an applied or exact-key uncertain device-copy transaction can be undone");
        }
        let result=async{
            self.begin_undo_recovery(&record,p["idempotencyKey"].as_str().unwrap())?;let status=self.require_connected(None)?;if json!(status.epoch)!=t["epoch"]{return Ok(transaction_error(id,"Live connection epoch changed; undo refused"));}
            let adapter=self.async_adapter();let context=self.transaction_context(p,signal,reads::AUDITION_DEADLINE_MS);record.borrow_mut()["undoKey"]=p["idempotencyKey"].clone();let created=&t["created"];
            let row=async{let snapshot=self.views.view_for(Some(&context),&[created["ref"].clone()],None,&[]).await?;self.device_row(&snapshot,created["ref"].as_str().unwrap())}.await.ok();
            if row.as_ref().is_none_or(|r|r.device["objectIdentity"]!=created["objectIdentity"]){
                if reconciliation&&row.is_none(){record.borrow_mut()["state"]=json!("undone");return Ok(success_text(id,&json!({"transactionId":t["id"],"state":"undone","reconciled":true,"idempotent":false})));}
                return Ok(transaction_error(id,"the copy moved or changed after it was made; undo refused"));
            }
            let row=row.unwrap();record.borrow_mut()["state"]=json!("undoing");
            if arrangement::truthy(&created["fingerprint"]){self.delete_owned_device_async(adapter.as_ref(),created["ref"].as_str().unwrap(),created["objectIdentity"].as_str().unwrap(),&context,created["fingerprint"].as_str(),Some(&record),false).await?;}else{
                self.invoke_undo_recovery(&record,adapter.as_ref(),"device.delete",&json!({"ref":created["ref"],"expectedObjectIdentity":row.device["objectIdentity"],"expectedOwnerRef":row.owner_ref,"expectedOwnerIdentity":row.owner_identity,"expectedSiblings":row.siblings,"expectedTrackRef":row.track["ref"],"expectedTrackIdentity":row.track["objectIdentity"],"explicitDeletion":true}),&context).await?;
            }
            let remains=async{let snapshot=self.views.view_for(Some(&context),&[row.track["ref"].clone()],None,&[]).await?;Ok::<_,LiveError>(self.device_row(&snapshot,created["ref"].as_str().unwrap())?.device["objectIdentity"]==created["objectIdentity"])}.await.unwrap_or(false);
            if remains{return Err(LiveError::error("the copy is still there"));}record.borrow_mut()["state"]=json!("undone");Ok(success_text(id,&json!({"transactionId":t["id"],"state":"undone","idempotent":false})))
        }.await;
        result.unwrap_or_else(|e| {
            record.borrow_mut()["state"] = json!("uncertain");
            adapter_tool_error(id, &e, "Device-copy undo is uncertain; look at the device's chain, then retry with the same key.")
        })
    }
}
