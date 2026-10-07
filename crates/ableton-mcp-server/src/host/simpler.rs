//! Verified file replacement retains the new sample until successful restoration.
use super::*;
use super::{arrangement::truthy, reads::AUDITION_DEADLINE_MS};
use kumi_common::{abort::Signal, js::json as js_json};
use sha2::{Digest, Sha256};
/// The file a Simpler plays: the Remote Script's row has it as its sample's `filePath` (the simulator's as
/// `samplePath`), and an empty Simpler has none ("", as Live fences it).
fn path_of(device: &Value) -> Value {
    device
        .get("samplePath")
        .filter(|v| !v.is_null())
        .or_else(|| device["sample"].get("filePath").filter(|v| !v.is_null()))
        .cloned()
        .unwrap_or(json!(""))
}
fn revision(path: &Value) -> Result<String, LiveError> {
    Ok(hex::encode(Sha256::digest(canonical_mutation_identity(&json!({"filePath":path}))?)))
}
fn fence(reference: &Value, device: &Value) -> String {
    let mut row = json!({"ref":reference});
    if let Some(v) = device.get("objectIdentity") {
        row["objectIdentity"] = v.clone()
    }
    row["filePath"] = path_of(device);
    js_json::stringify(&row)
}
impl McpHost {
    pub async fn dispatch_simpler_tool(&self, call: &ToolCall, signal: Option<&Signal>) -> Option<Result<Option<Value>, LiveError>> {
        let p = call.arguments.as_ref().unwrap_or(&Value::Null);
        Some(Ok(match call.name.as_str() {
            "live_simpler_preview" => Some(self.live_simpler_preview_async(&call.id, p).await),
            "live_simpler_apply" => self.live_simpler_apply_async(&call.id, p, signal).await,
            _ => return None,
        }))
    }
    pub async fn live_simpler_preview_async(&self, id: &Value, p: &Value) -> Value {
        if !has_only(p, &["deviceRef", "filePath", "allowedRoot"]) || !is_non_empty_string(&p["deviceRef"], 256) {
            return error(id, -32602, "deviceRef, filePath, and allowedRoot are required", None);
        }
        let result=async{
   let authority=self.audio_import_file_authority(&p["filePath"],&p["allowedRoot"]).await?;let status=self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS)))).await?;if !status.connected||!status.capabilities.iter().any(|c|c.as_str()=="session.read"){return Err(LiveError::error("session read capability is unavailable"))}if !status.has_operation("simpler.replace-sample"){return Err(LiveError::error("sample replacement is unavailable"))}
   let snapshot=self.views.view_for(None,&[p["deviceRef"].clone()],None,&[]).await?;let row=self.device_row(&snapshot,p["deviceRef"].as_str().unwrap())?;let current=path_of(&row.device);let staging=self.stage_verified_import_file(authority["canonicalPath"].as_str().unwrap(),&authority).await?;
   let retained=(||{let mut payload=json!({"ref":p["deviceRef"],"filePath":staging});if let Some(v)=row.device.get("objectIdentity"){payload["expectedObjectIdentity"]=v.clone()}payload["expectedStateRevision"]=json!(revision(&current)?);let t=json!({"id":tempo::transaction_id("simpler"),"epoch":status.epoch,"kind":"simpler","fence":fence(&p["deviceRef"],&row.device),"payload":payload,"prior":{"file":authority,"samplePath":current},"expiresAt":kumi_common::time::now_ms_f64()+TRANSACTION_TTL_MS,"state":"previewed"});self.retain_bounded_transaction(&self.clip_lifecycle_transactions,t.clone(),"simpler")?;Ok(success_text(id,&json!({"transactionId":t["id"],"epoch":t["epoch"],"deviceRef":p["deviceRef"],"currentSample":current,"file":{"path":authority["canonicalPath"],"size":authority["size"],"sha256":authority["sha256"]},"impact":"replaces-simpler-sample","confirmation":"apply","expiresAt":t["expiresAt"]})))})();
   if retained.is_err(){self.release_staged_import_file(&json!(staging))}retained
  }.await;
        result.unwrap_or_else(|e| adapter_tool_error(id, &e, "Simpler preview requires fresh authoritative state and a readable file."))
    }
    pub async fn live_simpler_apply_async(&self, id: &Value, p: &Value, signal: Option<&Signal>) -> Option<Value> {
        if !valid_transaction_params(p, "apply") {
            return Some(error(id, -32602, "transactionId, confirmation=apply, and idempotencyKey are required", None));
        }
        let record = self.clip_lifecycle_transactions.get(p["transactionId"].as_str().unwrap());
        let t = record.as_ref().map(|r| r.borrow().clone()).unwrap_or(Value::Null);
        if t["kind"] != "simpler"
            || (t["state"] == "previewed" && t["expiresAt"].as_f64().is_some_and(|n| n <= kumi_common::time::now_ms_f64()))
        {
            // Only an expired replacement of its own: another kind's id names files that may be in use.
            if t["kind"] == "simpler" {
                self.release_staged_import_for(&t);
            }
            return Some(transaction_error(id, "Unknown or expired simpler transaction"));
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
            // A reconcile's first apply may have loaded the staged file: it stays.
            if json!(status.epoch) != t["epoch"] {
                if !reconcile {
                    self.release_staged_import_for(&t);
                }
                return Ok(transaction_error(id, "Live connection epoch changed; preview again"));
            }
            if !truthy(&t["prior"]["file"]) {
                if !reconcile {
                    self.release_staged_import_for(&t);
                }
                return Ok(transaction_error(id, "simpler file authority is missing; preview again"));
            }
            let path = t["payload"]["filePath"].as_str().unwrap_or("");
            if !std::path::Path::new(path).exists() {
                return Ok(transaction_error(id, "staged import file is no longer available; preview again"));
            }
            self.verify_staged_import_file(path, &t["prior"]["file"]).await?;
            let adapter = self.async_adapter();
            let context = self.transaction_context(p, signal, AUDITION_DEADLINE_MS);
            if !reconcile {
                let snapshot = self.views.view_for(Some(&context), &[t["payload"]["ref"].clone()], None, &[]).await?;
                let row = self.device_row(&snapshot, t["payload"]["ref"].as_str().unwrap_or(""))?;
                if t["fence"] != fence(&t["payload"]["ref"], &row.device) {
                    self.release_staged_import_for(&t);
                    return Ok(transaction_error(id, "device identity or sample state changed since preview; preview again"));
                }
            }
            if let Err(error) = self.keep_staged(&t) {
                return Ok(transaction_error(
                    id,
                    &format!("the staged sample couldn't be kept for the Set ({}); nothing was sent to Live", error.message()),
                ));
            }
            {
                let mut row = record.borrow_mut();
                row["state"] = json!("applying");
                row["applyKey"] = p["idempotencyKey"].clone()
            }
            let result = adapter.invoke_async(&LiveInvocation::new("simpler.replace-sample", t["payload"].clone()), Some(&context)).await?;
            if result.is_null() {
                return Err(LiveError::type_error("Cannot read properties of null (reading 'changed')"));
            }
            if result["changed"] != true {
                return Err(LiveError::error("sample replacement was not confirmed"));
            }
            {
                let mut row = record.borrow_mut();
                row["created"] = json!({"samplePath":result.get("filePath").filter(|v|!v.is_null()).unwrap_or(&t["payload"]["filePath"])});
                row["applyKey"] = p["idempotencyKey"].clone();
                row["state"] = json!("applied")
            }
            Ok(success_text(id, &json!({"transactionId":t["id"],"state":"applied","result":result,"idempotent":false})))
        }
        .await;
        Some(
            result
                .unwrap_or_else(|e| apply_failed(id, &record, &e, "Simpler state is uncertain; perform fresh discovery before retrying.")),
        )
    }
    pub async fn undo_simpler_async(&self, id: &Value, p: &Value, signal: Option<&Signal>) -> Value {
        let record = p["transactionId"].as_str().and_then(|id| self.clip_lifecycle_transactions.get(id));
        let t = record.as_ref().map(|r| r.borrow().clone()).unwrap_or(Value::Null);
        if t["kind"] != "simpler" {
            return transaction_error(id, "Unknown or expired simpler transaction");
        }
        let record = record.unwrap();
        if t["state"] == "undone" && t["undoKey"] == p["idempotencyKey"] {
            return success_text(id, &json!({"transactionId":t["id"],"state":"undone","idempotent":true}));
        }
        let reconcile = t["state"] == "uncertain" && t["undoKey"] == p["idempotencyKey"];
        if (t["state"] != "applied" && !reconcile) || !truthy(&t["prior"]) {
            return transaction_error(id, "Only an applied or exact-key uncertain simpler transaction can be undone");
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
            let prior = &t["prior"];
            if prior["samplePath"].as_str().is_none_or(str::is_empty) {
                return Ok(transaction_error(id, "prior sample path is unavailable"));
            }
            let snapshot = self.views.view_for(Some(&context), &[t["payload"]["ref"].clone()], None, &[]).await?;
            let row = self.device_row(&snapshot, t["payload"]["ref"].as_str().unwrap_or(""))?;
            if let Some(moved) = self.undo_target_moved(
                id,
                &record.borrow(),
                "device",
                &t["payload"]["ref"],
                row.device.get("objectIdentity"),
                t["payload"].get("expectedObjectIdentity"),
            )? {
                return Ok(moved);
            }
            let current = path_of(&row.device);
            if !reconcile && current != *t["created"].get("samplePath").filter(|v| !v.is_null()).unwrap_or(&t["payload"]["filePath"]) {
                return Ok(transaction_error(id, "simpler sample changed after apply; undo refused"));
            }
            record.borrow_mut()["state"] = json!("undoing");
            let mut args = json!({"ref":t["payload"]["ref"],"filePath":prior["samplePath"]});
            if let Some(v) = row.device.get("objectIdentity") {
                args["expectedObjectIdentity"] = v.clone()
            }
            args["expectedStateRevision"] = json!(revision(&current)?);
            let result = self.invoke_undo_recovery(&record, adapter.as_ref(), "simpler.replace-sample", &args, &context).await?;
            if result.is_null() {
                return Err(LiveError::type_error("Cannot read properties of null (reading 'changed')"));
            }
            if result["changed"] != true {
                return Err(LiveError::error("simpler sample restoration was not confirmed"));
            }
            self.release_staged_import_for(&t);
            record.borrow_mut()["state"] = json!("undone");
            Ok(success_text(id, &json!({"transactionId":t["id"],"state":"undone","idempotent":false})))
        }
        .await;
        result.unwrap_or_else(|e| {
            record.borrow_mut()["state"] = json!("uncertain");
            adapter_tool_error(id, &e, "Simpler undo is uncertain; perform fresh discovery.")
        })
    }
}
