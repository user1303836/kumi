//! Rack state, macro counts, structural actions and rack-view restoration.
use super::*;
use super::{arrangement::truthy, device_parameter::fields, reads::AUDITION_DEADLINE_MS};
use kumi_common::{
    abort::Signal,
    js::{json as js_json, string},
};
use sha2::{Digest, Sha256};
const VIEW_FIELDS: &[&str] = &["selectedChainRef", "selectedPadIndex", "padScrollPosition", "showChainDevices"];
fn digest(v: &Value) -> Result<String, LiveError> {
    Ok(hex::encode(Sha256::digest(canonical_mutation_identity(v)?)))
}
fn objects(v: &Value) -> impl Iterator<Item = &Value> {
    v.as_array().into_iter().flatten().filter(|v| v.is_object())
}
fn extend(a: &mut Value, b: &Value) {
    a.as_object_mut().unwrap().extend(b.as_object().unwrap().clone())
}
fn strict(a: Option<&Value>, b: Option<&Value>) -> bool {
    match (a.and_then(Value::as_f64), b.and_then(Value::as_f64)) {
        (Some(a), Some(b)) => a == b,
        _ => a == b,
    }
}
/// A rack's view state as its row has it: the Remote Script sends it as the device's `view` (Live's
/// `RackDevice.View`), the simulator as `rackView`.
fn rack_view(device: &Value) -> &Value {
    if device["rackView"].is_object() {
        &device["rackView"]
    } else {
        &device["view"]
    }
}
fn view_state(device: &Value) -> Value {
    json!({"padScrollPosition":rack_view(device)["padScrollPosition"],"showChainDevices":rack_view(device)["showChainDevices"]})
}
fn title(view: bool) -> &'static str {
    if view {
        "Rack-view"
    } else {
        "Rack"
    }
}
fn label(view: bool) -> &'static str {
    if view {
        "rack-view"
    } else {
        "rack"
    }
}
fn result_field<'a>(value: &'a Value, key: &str) -> Result<&'a Value, LiveError> {
    if value.is_null() {
        Err(LiveError::type_error(format!("Cannot read properties of null (reading '{key}')")))
    } else {
        Ok(&value[key])
    }
}
impl McpHost {
    pub(super) fn rack_state_revision(&self, device: &Value) -> Result<String, LiveError> {
        let mut state = json!({"visibleMacroCount":device["visibleMacroCount"],"selectedVariationIndex":device["selectedVariationIndex"],"variationCount":device["variationCount"]});
        for key in ["macros", "chains", "drumPads"] {
            let identities = objects(&device[key])
                .map(|v| {
                    v.get("objectIdentity").cloned().ok_or_else(|| LiveError::error("mutation authority contains an unsupported value"))
                })
                .collect::<Result<Vec<_>, _>>()?;
            state[key] = json!(identities)
        }
        digest(&state)
    }
    fn rack_fence(&self, p: &Value, device: &Value, view: bool) -> Result<String, LiveError> {
        let mut value = if view { json!({"ref":p["ref"]}) } else { json!({"action":p["action"],"ref":p["ref"]}) };
        if let Some(v) = device.get("objectIdentity") {
            value["objectIdentity"] = v.clone()
        }
        if view {
            value["state"] = view_state(device)
        } else {
            value["stateRevision"] = json!(self.rack_state_revision(device)?)
        }
        Ok(js_json::stringify(&value))
    }
    pub async fn dispatch_rack_tool(&self, call: &ToolCall, signal: Option<&Signal>) -> Option<Result<Option<Value>, LiveError>> {
        let p = call.arguments.as_ref().unwrap_or(&Value::Null);
        Some(Ok(match call.name.as_str() {
            "live_rack_preview" => Some(self.rack_preview(&call.id, p, false).await),
            "live_rack_apply" => self.rack_apply(&call.id, p, false, signal).await,
            "live_rack_view_preview" => Some(self.rack_preview(&call.id, p, true).await),
            "live_rack_view_apply" => self.rack_apply(&call.id, p, true, signal).await,
            _ => return None,
        }))
    }
    async fn rack_preview(&self, id: &Value, p: &Value, view: bool) -> Value {
        if view {
            let mut keys = vec!["rackRef"];
            keys.extend_from_slice(VIEW_FIELDS);
            if !has_only(p, &keys) || !is_non_empty_string(&p["rackRef"], 256) {
                return error(id, -32602, "rackRef is required", None);
            }
            if VIEW_FIELDS.iter().all(|k| p.get(*k).is_none()) {
                return error(id, -32602, "at least one rack view field is required", None);
            }
        } else if !has_only(p, &["action", "rackRef", "selectedVariationIndex", "index", "sourceIndex", "targetIndex"])
            || !p["action"].as_str().is_some_and(|s| {
                [
                    "set",
                    "add-macro",
                    "remove-macro",
                    "randomize-macros",
                    "insert-chain",
                    "copy-pad",
                    "store-variation",
                    "recall-variation",
                    "delete-variation",
                ]
                .contains(&s)
            })
            || !is_non_empty_string(&p["rackRef"], 256)
        {
            return error(id, -32602, "action and rackRef are required", None);
        }
        let result=async{
   let status=self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS)))).await?;if !status.connected||!status.capabilities.iter().any(|c|c.as_str()=="session.read"){return Err(LiveError::error("session read capability is unavailable"))}if view&&!status.has_operation("rack.view.set"){return Err(LiveError::error("rack view editing is unavailable"))}let snapshot=self.views.view_for(None,&[p["rackRef"].clone()],None,&[]).await?;let row=self.device_row(&snapshot,p["rackRef"].as_str().unwrap())?;if row.device["canHaveChains"]!=true{return Ok(transaction_error(id,if view{"rack view requires a rack device"}else{"rack operations require a rack device"}))}
   let mut prior=json!({});let mut proposed=json!({});let mut payload;if view{
    if let Some(value)=p.get("selectedChainRef"){if !value.is_null(){if !is_non_empty_string(value,256){return Ok(error(id,-32602,"selectedChainRef is invalid",None))}self.chain_row(&snapshot,value.as_str().unwrap())?;}proposed["selectedChainRef"]=value.clone()}
    for(field,low)in [("selectedPadIndex",-1.),("padScrollPosition",0.)]{if let Some(value)=p.get(field){if !is_integer_in_range(value,low,127.){return Ok(error(id,-32602,&format!("{field} is invalid"),None))}proposed[field]=value.clone()}}
    if let Some(value)=p.get("showChainDevices"){if !value.is_boolean(){return Ok(error(id,-32602,"showChainDevices must be boolean",None))}proposed["showChainDevices"]=value.clone()}
    let rack=rack_view(&row.device);prior=json!({"selectedChainRef":rack["selectedChainRef"],"selectedPadIndex":rack["selectedPadIndex"],"padScrollPosition":rack["padScrollPosition"],"showChainDevices":rack["showChainDevices"]});payload=json!({"ref":p["rackRef"]});extend(&mut payload,&proposed);
   }else{
    let _revision=self.rack_state_revision(&row.device)?;
    payload=json!({"action":p["action"],"ref":p["rackRef"]});if p["action"]=="set"{
     if !status.has_operation("rack.set"){return Err(LiveError::error("rack editing is unavailable"))}if let Some(value)=p.get("selectedVariationIndex"){if !is_integer_in_range(value,-1.,100000.){return Ok(error(id,-32602,"selectedVariationIndex is invalid",None))}proposed["selectedVariationIndex"]=value.clone()}if proposed.as_object().unwrap().is_empty(){return Ok(error(id,-32602,"at least one rack field is required",None))}prior=json!({"selectedVariationIndex":row.device["selectedVariationIndex"]});extend(&mut payload,&proposed);
    }else{
     if !status.has_operation("rack.action"){return Err(LiveError::error("rack actions are unavailable"))}if p["action"]=="remove-macro"&&p.get("index").is_some(){return Ok(error(id,-32602,"remove-macro takes no index in the public LOM",None))}if p["action"]=="copy-pad"&&!["sourceIndex","targetIndex"].iter().all(|k|is_integer_in_range(&p[*k],0.,f64::INFINITY)){return Ok(error(id,-32602,"sourceIndex and targetIndex are required",None))}if p["action"]=="add-macro"||p["action"]=="remove-macro"{prior=json!({"visibleMacroCount":row.device["visibleMacroCount"]})}extend(&mut payload,&fields(p,&["index","sourceIndex","targetIndex"]));
    }
   }
   if let Some(value)=row.device.get("objectIdentity"){payload["expectedObjectIdentity"]=value.clone()}payload["expectedStateRevision"]=json!(if view{digest(&view_state(&row.device))?}else{self.rack_state_revision(&row.device)?});let t=json!({"id":tempo::transaction_id(if view{"rackview"}else{"rack"}),"epoch":status.epoch,"kind":label(view),"fence":self.rack_fence(&payload,&row.device,view)?,"payload":payload,"prior":prior,"expiresAt":kumi_common::time::now_ms_f64()+TRANSACTION_TTL_MS,"state":"previewed"});self.retain_bounded_transaction(&self.clip_lifecycle_transactions,t.clone(),if view{"rack view"}else{"rack"})?;let mut response=json!({"transactionId":t["id"],"epoch":t["epoch"]});if !view{response["action"]=p["action"].clone()}response["rackRef"]=p["rackRef"].clone();if !view{response["rackName"]=row.device["name"].as_str().map(|s|json!(s)).unwrap_or(Value::Null)}response["prior"]=prior;if view{response["proposed"]=proposed}response["impact"]=json!(if view{"edits-rack-view"}else if p["action"]=="set"||p["action"]=="add-macro"||p["action"]=="remove-macro"{"edits-rack"}else{"momentary-rack-action-no-undo"});response["confirmation"]=json!("apply");response["expiresAt"]=t["expiresAt"].clone();Ok(success_text(id,&response))
  }.await;
        result.unwrap_or_else(|e| adapter_tool_error(id, &e, &format!("{} preview requires fresh authoritative state.", title(view))))
    }
    async fn rack_apply(&self, id: &Value, p: &Value, view: bool, signal: Option<&Signal>) -> Option<Value> {
        if !valid_transaction_params(p, "apply") {
            return Some(error(id, -32602, "transactionId, confirmation=apply, and idempotencyKey are required", None));
        }
        let record = self.clip_lifecycle_transactions.get(p["transactionId"].as_str().unwrap());
        let t = record.as_ref().map(|r| r.borrow().clone()).unwrap_or(Value::Null);
        if t["kind"] != label(view)
            || (t["state"] == "previewed" && t["expiresAt"].as_f64().is_some_and(|n| n <= kumi_common::time::now_ms_f64()))
        {
            return Some(transaction_error(id, &format!("Unknown or expired {} transaction", label(view))));
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
        let result=async{
   if reconcile{self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS)))).await?;}let status=self.require_connected(Some("session.read"))?;if json!(status.epoch)!=t["epoch"]{return Ok(transaction_error(id,"Live connection epoch changed; preview again"))}let adapter=self.async_adapter();let context=self.transaction_context(p,signal,AUDITION_DEADLINE_MS);let reference=t["payload"]["ref"].as_str().unwrap_or("");if !reconcile{let snapshot=self.views.view_for(Some(&context),&[t["payload"]["ref"].clone()],None,&[]).await?;let row=self.device_row(&snapshot,reference)?;if t["fence"]!=self.rack_fence(&t["payload"],&row.device,view)?{return Ok(transaction_error(id,if view{"rack identity or view state changed since preview; preview again"}else{"rack identity or state changed since preview; preview again"}))}}
   let action=t["payload"]["action"].as_str().unwrap_or("");{let mut row=record.borrow_mut();row["state"]=json!("applying");row["applyKey"]=p["idempotencyKey"].clone()}let mut args=t["payload"].clone();if action=="set"{args.as_object_mut().unwrap().remove("action");}let result=adapter.invoke_async(&LiveInvocation::new(if view{"rack.view.set"}else if action=="set"{"rack.set"}else{"rack.action"},args),Some(&context)).await?;
   if view||action=="set"{
    if result_field(&result,"changed")?!=true{return Err(LiveError::error(if view{"rack view change was not confirmed"}else{"rack change was not confirmed"}))}if !view{let snapshot=self.views.view_for(Some(&context),&[t["payload"]["ref"].clone()],None,&[]).await?;let device=self.device_row(&snapshot,reference)?.device;if t["payload"].get("selectedVariationIndex").is_some_and(|v|!strict(device.get("selectedVariationIndex"),Some(v))){return Err(LiveError::error("rack postcondition was not confirmed"))}}
   }else{
    if result_field(&result,"done")?!=true{return Err(LiveError::error("rack action was not confirmed"))}if action=="add-macro"||action=="remove-macro"{let snapshot=self.views.view_for(Some(&context),&[t["payload"]["ref"].clone()],None,&[]).await?;record.borrow_mut()["created"]=json!({"visibleMacroCount":self.device_row(&snapshot,reference)?.device["visibleMacroCount"]})}
    if action=="insert-chain"&&is_non_empty_string(&result["chainRef"],256){record.borrow_mut()["created"]=json!({"chainRef":result["chainRef"]});let placement=async{let snapshot=self.views.view_for(Some(&context),&[t["payload"]["ref"].clone()],None,&[]).await?;let rack=self.device_row(&snapshot,reference)?.device;let chains:Vec<_>=objects(&rack["chains"]).collect();let name=|v:&Value|->Result<String,LiveError>{Ok(string::head(&js_string(v.get("name").filter(|v|!v.is_null()).unwrap_or(&json!("")))?,64))};let rows=chains.iter().take(8).map(|c|Ok(json!({"name":name(c)?,"devices":objects(&c["devices"]).take(16).map(name).collect::<Result<Vec<_>,_>>()?}))).collect::<Result<Vec<_>,LiveError>>()?;Ok::<_,LiveError>(json!({"owner":"rack","rack":name(&rack)?,"chain":chains.iter().position(|c|c["ref"]==result["chainRef"]).map(|n|n as i64).unwrap_or(-1),"chains":rows}))}.await;if let Ok(placement)=placement{record.borrow_mut()["created"]["placement"]=placement}}
   }
   {let mut row=record.borrow_mut();row["applyKey"]=p["idempotencyKey"].clone();row["state"]=json!("applied")}let mut response=json!({"transactionId":t["id"],"state":"applied"});if let Some(v)=result.get("revision"){response["revision"]=v.clone()}if !view{if is_non_empty_string(&result["chainRef"],256){response["chainRef"]=result["chainRef"].clone()}let latest=record.borrow();if latest["created"]["placement"].is_object(){response["placement"]=latest["created"]["placement"].clone()}if let Some(v)=latest["created"].get("visibleMacroCount"){response["visibleMacroCount"]=v.clone()}}response["idempotent"]=json!(false);Ok(success_text(id,&response))
  }.await;
        Some(result.unwrap_or_else(|e| {
            apply_failed(id, &record, &e, &format!("{} state is uncertain; perform fresh discovery before retrying.", title(view)))
        }))
    }
    pub async fn undo_rack_async(&self, id: &Value, p: &Value, signal: Option<&Signal>) -> Value {
        let view = p["transactionId"].as_str().is_some_and(|s| s.starts_with("rackview_"));
        let record = p["transactionId"].as_str().and_then(|id| self.clip_lifecycle_transactions.get(id));
        let t = record.as_ref().map(|r| r.borrow().clone()).unwrap_or(Value::Null);
        if t["kind"] != label(view) {
            return transaction_error(id, &format!("Unknown or expired {} transaction", label(view)));
        }
        let record = record.unwrap();
        let macro_action = t["payload"]["action"] == "add-macro" || t["payload"]["action"] == "remove-macro";
        if !view && t["payload"]["action"] != "set" && !macro_action {
            return transaction_error(id, "Rack actions are momentary or structural and not undoable");
        }
        if t["state"] == "undone" && t["undoKey"] == p["idempotencyKey"] {
            return success_text(id, &json!({"transactionId":t["id"],"state":"undone","idempotent":true}));
        }
        let reconcile = t["state"] == "uncertain" && t["undoKey"] == p["idempotencyKey"];
        if (t["state"] != "applied" && !reconcile) || !truthy(&t["prior"]) {
            return transaction_error(id, &format!("Only an applied or exact-key uncertain {} transaction can be undone", label(view)));
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
            if !reconcile {
                if view {
                    for field in ["padScrollPosition", "showChainDevices"] {
                        if t["payload"].get(field).is_some_and(|v| !strict(rack_view(&row.device).get(field), Some(v))) {
                            return Ok(transaction_error(id, "rack view changed after apply; undo refused"));
                        }
                    }
                } else if t["payload"]
                    .get("selectedVariationIndex")
                    .is_some_and(|v| !strict(row.device.get("selectedVariationIndex"), Some(v)))
                {
                    return Ok(transaction_error(id, "rack changed after apply; undo refused"));
                }
            }
            let mut args = json!({"ref":t["payload"]["ref"]});
            if macro_action {
                let left = &t["created"]["visibleMacroCount"];
                if !left.is_number()
                    || !t["prior"]["visibleMacroCount"].is_number()
                    || !strict(row.device.get("visibleMacroCount"), Some(left))
                {
                    return Ok(transaction_error(id, "rack macros changed after apply; undo refused"));
                }
                args["action"] = json!(if t["payload"]["action"] == "add-macro" { "remove-macro" } else { "add-macro" })
            } else if !view {
                extend(&mut args, &t["prior"])
            }
            record.borrow_mut()["state"] = json!("undoing");
            if let Some(v) = row.device.get("objectIdentity") {
                args["expectedObjectIdentity"] = v.clone()
            }
            args["expectedStateRevision"] =
                json!(if view { digest(&view_state(&row.device))? } else { self.rack_state_revision(&row.device)? });
            if view {
                for field in VIEW_FIELDS {
                    if t["payload"].get(*field).is_some() {
                        if let Some(v) = t["prior"].get(*field) {
                            args[*field] = v.clone()
                        }
                    }
                }
            }
            let result = self
                .invoke_undo_recovery(
                    &record,
                    adapter.as_ref(),
                    if view {
                        "rack.view.set"
                    } else if macro_action {
                        "rack.action"
                    } else {
                        "rack.set"
                    },
                    &args,
                    &context,
                )
                .await?;
            if macro_action {
                if result_field(&result, "done")? != true {
                    return Err(LiveError::error("rack macro restoration was not confirmed"));
                }
                let snapshot = self.views.view_for(Some(&context), &[t["payload"]["ref"].clone()], None, &[]).await?;
                if !strict(
                    self.device_row(&snapshot, t["payload"]["ref"].as_str().unwrap_or(""))?.device.get("visibleMacroCount"),
                    t["prior"].get("visibleMacroCount"),
                ) {
                    return Err(LiveError::error("rack macro restoration was not confirmed"));
                }
            } else if result_field(&result, "changed")? != true {
                return Err(LiveError::error(if view {
                    "rack view restoration was not confirmed"
                } else {
                    "rack restoration was not confirmed"
                }));
            }
            record.borrow_mut()["state"] = json!("undone");
            Ok(success_text(id, &json!({"transactionId":t["id"],"state":"undone","idempotent":false})))
        }
        .await;
        result.unwrap_or_else(|e| {
            record.borrow_mut()["state"] = json!("uncertain");
            adapter_tool_error(id, &e, &format!("{} undo is uncertain; perform fresh discovery.", title(view)))
        })
    }
}
