//! Looper properties, transport actions and exact export destinations.
use super::*;
use super::{arrangement::truthy, reads::AUDITION_DEADLINE_MS};
use kumi_common::{abort::Signal, js::json as js_json};
use sha2::{Digest, Sha256};
fn writable(row: &Value) -> Value {
    json!({"overdubAfterRecord":row["overdubAfterRecord"],"recordLengthIndex":row["recordLengthIndex"]})
}
fn digest(v: &Value) -> Result<String, LiveError> {
    Ok(hex::encode(Sha256::digest(canonical_mutation_identity(v)?)))
}
fn strict(a: Option<&Value>, b: Option<&Value>) -> bool {
    match (a.and_then(Value::as_f64), b.and_then(Value::as_f64)) {
        (Some(a), Some(b)) => a == b,
        _ => a == b,
    }
}
impl McpHost {
    pub async fn dispatch_looper_tool(&self, call: &ToolCall, signal: Option<&Signal>) -> Option<Result<Option<Value>, LiveError>> {
        let p = call.arguments.as_ref().unwrap_or(&Value::Null);
        Some(Ok(match call.name.as_str() {
            "live_looper_preview" => Some(self.live_looper_preview_async(&call.id, p).await),
            "live_looper_apply" => self.live_looper_apply_async(&call.id, p, signal).await,
            _ => return None,
        }))
    }
    pub async fn live_looper_preview_async(&self, id: &Value, p: &Value) -> Value {
        if !has_only(p, &["action", "deviceRef", "slotRef", "overdubAfterRecord", "recordLengthIndex"])
            || !p["action"].as_str().is_some_and(|s| {
                ["set", "record", "overdub", "play", "stop", "clear", "undo", "double-speed", "half-speed", "export"].contains(&s)
            })
            || !is_non_empty_string(&p["deviceRef"], 256)
        {
            return error(id, -32602, "action and deviceRef are required", None);
        }
        let result=async{
   let status=self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS)))).await?;if !status.connected||!status.capabilities.iter().any(|c|c.as_str()=="session.read"){return Err(LiveError::error("session read capability is unavailable"))}let snapshot=self.views.view_for(None,&[p["deviceRef"].clone(),p["slotRef"].clone()],None,&[]).await?;let row=self.device_row(&snapshot,p["deviceRef"].as_str().unwrap())?;let looper=&row.device["looper"];let mut prior=json!({});let mut payload=json!({"action":p["action"],"ref":p["deviceRef"]});let mut state=writable(looper);
   if p["action"]=="set"{
    if !status.has_operation("looper.set"){return Err(LiveError::error("looper properties are unavailable"))}let mut proposed=json!({});if let Some(v)=p.get("overdubAfterRecord"){if !v.is_boolean(){return Ok(error(id,-32602,"overdubAfterRecord must be boolean",None))}proposed["overdubAfterRecord"]=v.clone()}if let Some(v)=p.get("recordLengthIndex"){if !is_integer_in_range(v,0.,8.){return Ok(error(id,-32602,"recordLengthIndex is out of bounds",None))}proposed["recordLengthIndex"]=v.clone()}if proposed.as_object().unwrap().is_empty(){return Ok(error(id,-32602,"at least one looper field is required (loopLength and tempo are read-only; speed changes are double-speed/half-speed actions)",None))}for(k,v)in proposed.as_object().unwrap(){prior[k]=looper[k].clone();payload[k]=v.clone()}
   }else{
    if !status.has_operation("looper.action"){return Err(LiveError::error("looper actions are unavailable"))}if p["action"]=="export"{if !is_non_empty_string(&p["slotRef"],256){return Ok(error(id,-32602,"export requires an exact empty target clip slot (slotRef)",None))}let snapshot=serde_json::to_value(&snapshot).unwrap();let slot=snapshot["tracks"].as_array().into_iter().flatten().flat_map(|t|t["clipSlots"].as_array().into_iter().flatten()).find(|s|s["ref"]==p["slotRef"]).ok_or_else(||LiveError::error("export target clip slot is not authoritative"))?;if slot.get("clipRef").is_some_and(|v|!v.is_null()){return Ok(error(id,-32602,"export target clip slot is not empty",None))}}else if p.get("slotRef").is_some(){return Ok(error(id,-32602,"slotRef is only valid for the export action",None))}
    if let Some(v)=p.get("slotRef"){payload["slotRef"]=v.clone()}for key in ["loopLength","tempo","state"]{state[key]=looper[key].clone()}
   }
   if let Some(v)=row.device.get("objectIdentity"){payload["expectedObjectIdentity"]=v.clone()}payload["expectedStateRevision"]=json!(digest(&state)?);let mut fence=json!({"action":p["action"],"ref":p["deviceRef"]});if let Some(v)=row.device.get("objectIdentity"){fence["objectIdentity"]=v.clone()}fence["payload"]=payload.clone();let t=json!({"id":tempo::transaction_id("looper"),"epoch":status.epoch,"kind":"looper","fence":js_json::stringify(&fence),"payload":payload,"prior":prior,"expiresAt":kumi_common::time::now_ms_f64()+TRANSACTION_TTL_MS,"state":"previewed"});self.retain_bounded_transaction(&self.clip_lifecycle_transactions,t.clone(),"looper")?;Ok(success_text(id,&json!({"transactionId":t["id"],"epoch":t["epoch"],"action":p["action"],"deviceRef":p["deviceRef"],"prior":prior,"impact":if p["action"]=="set"{"edits-looper"}else if p["action"]=="export"{"exports-audio-to-exact-clip-slot-no-undo"}else{"momentary-looper-action-no-undo"},"confirmation":"apply","expiresAt":t["expiresAt"]})))
  }.await;
        result.unwrap_or_else(|e| adapter_tool_error(id, &e, "Looper preview requires fresh authoritative state."))
    }
    pub async fn live_looper_apply_async(&self, id: &Value, p: &Value, signal: Option<&Signal>) -> Option<Value> {
        if !valid_transaction_params(p, "apply") {
            return Some(error(id, -32602, "transactionId, confirmation=apply, and idempotencyKey are required", None));
        }
        let record = self.clip_lifecycle_transactions.get(p["transactionId"].as_str().unwrap());
        let t = record.as_ref().map(|r| r.borrow().clone()).unwrap_or(Value::Null);
        if t["kind"] != "looper"
            || (t["state"] == "previewed" && t["expiresAt"].as_f64().is_some_and(|n| n <= kumi_common::time::now_ms_f64()))
        {
            return Some(transaction_error(id, "Unknown or expired looper transaction"));
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
            let set = t["payload"]["action"] == "set";
            {
                let mut row = record.borrow_mut();
                row["state"] = json!("applying");
                row["applyKey"] = p["idempotencyKey"].clone()
            }
            let mut args = t["payload"].clone();
            if set {
                args.as_object_mut().unwrap().remove("action");
            }
            let result =
                adapter.invoke_async(&LiveInvocation::new(if set { "looper.set" } else { "looper.action" }, args), Some(&context)).await?;
            let expected = if set { "changed" } else { "done" };
            if result.is_null() {
                return Err(LiveError::type_error(format!("Cannot read properties of null (reading '{expected}')")));
            }
            if result[expected] != true {
                return Err(LiveError::error(if set { "looper change was not confirmed" } else { "looper action was not confirmed" }));
            }
            if set {
                let snapshot = self.views.view_for(Some(&context), &[t["payload"]["ref"].clone()], None, &[]).await?;
                let device = self.device_row(&snapshot, t["payload"]["ref"].as_str().unwrap_or(""))?.device;
                for field in ["overdubAfterRecord", "recordLengthIndex"] {
                    if let Some(value) = t["payload"].get(field) {
                        if !strict(device["looper"].get(field), Some(value)) {
                            return Err(LiveError::error("looper postcondition was not confirmed"));
                        }
                    }
                }
            }
            {
                let mut row = record.borrow_mut();
                row["applyKey"] = p["idempotencyKey"].clone();
                row["state"] = json!("applied")
            }
            Ok(success_text(id, &json!({"transactionId":t["id"],"state":"applied","idempotent":false})))
        }
        .await;
        Some(
            result.unwrap_or_else(|e| {
                self.apply_failed(id, &record, &e, "Looper state is uncertain; perform fresh discovery before retrying.")
            }),
        )
    }
    pub async fn undo_looper_async(&self, id: &Value, p: &Value, signal: Option<&Signal>) -> Value {
        let record = p["transactionId"].as_str().and_then(|id| self.clip_lifecycle_transactions.get(id));
        let t = record.as_ref().map(|r| r.borrow().clone()).unwrap_or(Value::Null);
        if t["kind"] != "looper" {
            return transaction_error(id, "Unknown or expired looper transaction");
        }
        let record = record.unwrap();
        if t["payload"]["action"] != "set" {
            return transaction_error(id, "Looper actions are momentary and not undoable");
        }
        if t["state"] == "undone" && t["undoKey"] == p["idempotencyKey"] {
            return success_text(id, &json!({"transactionId":t["id"],"state":"undone","idempotent":true}));
        }
        let reconcile = t["state"] == "uncertain" && t["undoKey"] == p["idempotencyKey"];
        if (t["state"] != "applied" && !reconcile) || !truthy(&t["prior"]) {
            return transaction_error(id, "Only an applied or exact-key uncertain looper transaction can be undone");
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
            let looper = &row.device["looper"];
            if !reconcile {
                for field in ["overdubAfterRecord", "recordLengthIndex"] {
                    if let Some(v) = t["payload"].get(field) {
                        if !strict(looper.get(field), Some(v)) {
                            return Ok(transaction_error(id, "looper changed after apply; undo refused"));
                        }
                    }
                }
            }
            record.borrow_mut()["state"] = json!("undoing");
            let mut args = json!({"ref":t["payload"]["ref"]});
            args.as_object_mut()
                .unwrap()
                .extend(t["prior"].as_object().unwrap().iter().filter(|(_, v)| !v.is_null()).map(|(k, v)| (k.clone(), v.clone())));
            if let Some(v) = row.device.get("objectIdentity") {
                args["expectedObjectIdentity"] = v.clone()
            }
            args["expectedStateRevision"] = json!(digest(&writable(looper))?);
            let result = self.invoke_undo_recovery(&record, adapter.as_ref(), "looper.set", &args, &context).await?;
            if result.is_null() {
                return Err(LiveError::type_error("Cannot read properties of null (reading 'changed')"));
            }
            if result["changed"] != true {
                return Err(LiveError::error("looper restoration was not confirmed"));
            }
            record.borrow_mut()["state"] = json!("undone");
            Ok(success_text(id, &json!({"transactionId":t["id"],"state":"undone","idempotent":false})))
        }
        .await;
        result.unwrap_or_else(|e| {
            record.borrow_mut()["state"] = json!("uncertain");
            adapter_tool_error(id, &e, "Looper undo is uncertain; perform fresh discovery.")
        })
    }
}
