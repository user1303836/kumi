//! Return-track creation/deletion and track/scene duplication.
use super::*;
use kumi_common::{abort::Signal, js::json as js_json, time::now_ms_f64};
fn operation(action: &Value) -> &'static str {
    match action.as_str() {
        Some("create-return") => "track.create-return",
        Some("delete-return") => "track.delete-return",
        Some("duplicate-track") => "track.duplicate",
        _ => "scene.duplicate",
    }
}
impl McpHost {
    pub async fn dispatch_track_structure_tool(
        &self,
        call: &ToolCall,
        signal: Option<&Signal>,
    ) -> Option<Result<Option<Value>, LiveError>> {
        let p = call.arguments.as_ref().unwrap_or(&Value::Null);
        Some(Ok(match call.name.as_str() {
            "live_track_structure_preview" => Some(self.live_track_structure_preview_async(&call.id, p).await),
            "live_track_structure_apply" => self.live_track_structure_apply_async(&call.id, p, signal).await,
            _ => return None,
        }))
    }
    pub async fn live_track_structure_preview_async(&self, id: &Value, p: &Value) -> Value {
        if !has_only(p, &["action", "name", "ref"]) {
            return error(id, -32602, "action is required", None);
        }
        if !p["action"].as_str().is_some_and(|a| ["create-return", "delete-return", "duplicate-track", "duplicate-scene"].contains(&a)) {
            return error(id, -32602, "action is invalid", None);
        }
        let result=async{
            let status=self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(reads::AUDITION_DEADLINE_MS)))).await?;if !status.connected||!status.capabilities.iter().any(|c|c.as_str()=="session.read"){return Err(LiveError::error("session read capability is unavailable"));}let operation=operation(&p["action"]);if !status.has_operation(operation){return Err(LiveError::error(format!("{operation} is unavailable")));}
            let snapshot=self.structure_view(None).await?;let revision=self.structure_revision(&snapshot);let s=serde_json::to_value(&snapshot).unwrap();let mut payload=json!({"action":p["action"]});
            if p["action"]=="create-return"{if p.get("name").is_some_and(|v|!is_non_empty_string(v,256)){return Ok(error(id,-32602,"name is invalid",None));}if let Some(name)=p.get("name"){payload["name"]=name.clone();}payload["expectedStructureRevision"]=json!(revision);
            }else{
                if !is_non_empty_string(&p["ref"],256){return Ok(error(id,-32602,"ref is required",None));}
                let action=p["action"].as_str().unwrap();let rows=s[if action=="duplicate-scene"{"scenes"}else{"tracks"}].as_array().ok_or_else(||LiveError::type_error("Cannot read properties of undefined (reading 'find')"))?;
                let row=rows.iter().find(|r|r["ref"]==p["ref"]&&if action=="delete-return"{r["kind"]=="return"}else if action=="duplicate-track"{!r["kind"].as_str().is_some_and(|k|["return","main","master"].contains(&k))}else{true}).filter(|r|is_non_empty_string(&r["objectIdentity"],256));
                let Some(row)=row else{return Ok(transaction_error(id,match action{"delete-return"=>"return-track reference is unknown","duplicate-track"=>"track reference is unknown or not a regular track",_=>"scene reference is unknown"}))};
                // Live copies a group with every track inside it, which this change couldn't confirm or take back as one.
                if action=="duplicate-track"&&row["kind"]=="group"{return Ok(reason_error(id,"Kumi can't duplicate a group track: Live copies every track inside it too","Nothing changed in Live. Duplicate it in Live itself, or duplicate the tracks inside it one by one."));}
                payload["ref"]=p["ref"].clone();payload["expectedObjectIdentity"]=row["objectIdentity"].clone();payload["expectedStructureRevision"]=json!(revision);if action=="delete-return"{payload["explicitDeletion"]=json!(true);}
            }
            let t=json!({"id":tempo::transaction_id("trackstruct"),"epoch":status.epoch,"kind":"track-structure","fence":js_json::stringify(&json!({"action":p["action"],"payload":payload,"structureRevision":revision})),"payload":payload,"prior":{},"expiresAt":now_ms_f64()+TRANSACTION_TTL_MS,"state":"previewed"});self.retain_bounded_transaction(&self.clip_lifecycle_transactions,t.clone(),"track structure")?;
            Ok(success_text(id,&json!({"transactionId":t["id"],"epoch":t["epoch"],"action":p["action"],"impact":if p["action"]=="delete-return"{"deletes-return-track-no-undo"}else{"creates-track-structure"},"confirmation":"apply","expiresAt":t["expiresAt"]})))
        }.await;
        result.unwrap_or_else(|e| adapter_tool_error(id, &e, "Track-structure preview requires fresh authoritative state."))
    }
    pub async fn live_track_structure_apply_async(&self, id: &Value, p: &Value, signal: Option<&Signal>) -> Option<Value> {
        if !valid_transaction_params(p, "apply") {
            return Some(error(id, -32602, "transactionId, confirmation=apply, and idempotencyKey are required", None));
        }
        let Some(record) = self.clip_lifecycle_transactions.get(p["transactionId"].as_str().unwrap()).filter(|r| {
            let t = r.borrow();
            t["kind"] == "track-structure" && !(t["state"] == "previewed" && t["expiresAt"].as_f64().unwrap_or(0.) <= now_ms_f64())
        }) else {
            return Some(transaction_error(id, "Unknown or expired track-structure transaction"));
        };
        let t = record.borrow().clone();
        if t["state"] == "applied" && t["applyKey"] == p["idempotencyKey"] {
            let mut response = json!({"transactionId":t["id"],"state":"applied"});
            if let Some(created) = t.get("created") {
                response["created"] = created.clone();
            }
            response["idempotent"] = json!(true);
            return Some(success_text(id, &response));
        }
        let reconciliation = t["state"] == "uncertain" && t["applyKey"] == p["idempotencyKey"];
        if t["state"] != "previewed" && !reconciliation {
            return Some(transaction_error(id, "Transaction is no longer applicable"));
        }
        if signal.is_some_and(Signal::is_cancelled) {
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
            let context = self.transaction_context(p, signal, reads::AUDITION_DEADLINE_MS);
            if !reconciliation {
                let revision = self.structure_revision(&self.structure_view(Some(&context)).await?);
                if js_json::stringify(&json!({"action":t["payload"]["action"],"payload":t["payload"],"structureRevision":revision}))
                    != t["fence"]
                {
                    return Ok(transaction_error(id, "structure changed since preview; preview again"));
                }
            }
            record.borrow_mut()["state"] = json!("applying");
            record.borrow_mut()["applyKey"] = p["idempotencyKey"].clone();
            let mut args = t["payload"].clone();
            args.as_object_mut().unwrap().remove("action");
            let result =
                self.async_adapter().invoke_async(&LiveInvocation::new(operation(&t["payload"]["action"]), args), Some(&context)).await?;
            if t["payload"]["action"] == "delete-return" {
                if result.is_null() {
                    return Err(LiveError::type_error("Cannot read properties of null (reading 'deleted')"));
                }
                if result["deleted"] != t["payload"]["ref"] {
                    return Err(LiveError::error("return-track deletion was not confirmed"));
                }
            } else {
                if result.is_null() {
                    return Err(LiveError::type_error("Cannot read properties of null (reading 'ref')"));
                }
                if !is_non_empty_string(&result["ref"], 256)
                    || !is_non_empty_string(&result["objectIdentity"], 256)
                    || !is_non_empty_string(&result["createdFingerprint"], 64)
                {
                    return Err(LiveError::error("track-structure creation did not return exact identity"));
                }
                let kind = if t["payload"]["action"] == "duplicate-scene" { "scene" } else { "track" };
                let mut item = json!({"kind":kind,"ref":result["ref"]});
                if result["index"].is_number() {
                    item["index"] = result["index"].clone();
                }
                let snapshot = self.structure_owned_view(Some(&context), &[item]).await?;
                let mut created = result.clone();
                created["contentFingerprint"] = json!(self.session_structure_created_fingerprint(&snapshot, kind, &result["ref"])?);
                record.borrow_mut()["created"] = created;
            }
            record.borrow_mut()["applyKey"] = p["idempotencyKey"].clone();
            record.borrow_mut()["state"] = json!("applied");
            Ok(success_text(id, &json!({"transactionId":t["id"],"state":"applied","result":result,"idempotent":false})))
        }
        .await;
        Some(result.unwrap_or_else(|e| {
            self.apply_failed(id, &record, &e, "Track-structure state is uncertain; perform fresh discovery before retrying.")
        }))
    }
    pub async fn undo_track_structure_async(&self, id: &Value, p: &Value, signal: Option<&Signal>) -> Value {
        let Some(record) =
            self.clip_lifecycle_transactions.get(p["transactionId"].as_str().unwrap()).filter(|r| r.borrow()["kind"] == "track-structure")
        else {
            return transaction_error(id, "Unknown or expired track-structure transaction");
        };
        let t = record.borrow().clone();
        if t["payload"]["action"] == "delete-return" {
            return transaction_error(id, "Deleted return tracks cannot be reconstructed; undo is unavailable for this transaction");
        }
        if t["state"] == "undone" && t["undoKey"] == p["idempotencyKey"] {
            return success_text(id, &json!({"transactionId":t["id"],"state":"undone","idempotent":true}));
        }
        let reconciliation = t["state"] == "uncertain" && t["undoKey"] == p["idempotencyKey"];
        if (t["state"] != "applied" && !reconciliation)
            || !is_non_empty_string(&t["created"]["ref"], 256)
            || !is_non_empty_string(&t["created"]["objectIdentity"], 256)
        {
            return transaction_error(id, "Only an applied track-structure creation has automatic undo authority");
        }
        let result=async{
            self.begin_undo_recovery(&record,p["idempotencyKey"].as_str().unwrap())?;let status=self.require_connected(Some("session.read"))?;if json!(status.epoch)!=t["epoch"]{return Ok(transaction_error(id,"Live connection epoch changed; undo refused"));}let adapter=self.async_adapter();let context=self.transaction_context(p,signal,reads::AUDITION_DEADLINE_MS);record.borrow_mut()["undoKey"]=p["idempotencyKey"].clone();if reconciliation{self.replay_undo_recovery(&record,adapter.as_ref(),&context).await?;}record.borrow_mut()["state"]=json!("undoing");
            let action=t["payload"]["action"].as_str().unwrap();let snapshot=self.structure_owned_view(Some(&context),&[json!({"kind":if action=="duplicate-scene"{"scene"}else{"track"},"ref":t["created"]["ref"]})]).await?;
            let result=self.invoke_undo_recovery(&record,adapter.as_ref(),match action{"create-return"=>"track.delete-return","duplicate-track"=>"track.delete",_=>"scene.delete"},&json!({"ref":t["created"]["ref"],"expectedObjectIdentity":t["created"]["objectIdentity"],"expectedStructureRevision":self.structure_revision(&snapshot)}),&context).await?;
            if result.is_null(){return Err(LiveError::type_error("Cannot read properties of null (reading 'deleted')"));}if result["deleted"]!=t["created"]["ref"]{return Err(LiveError::error(if action=="create-return"{"return-track cleanup was not confirmed"}else{"duplicated structure cleanup was not confirmed"}));}
            record.borrow_mut()["state"]=json!("undone");Ok(success_text(id,&json!({"transactionId":t["id"],"state":"undone","idempotent":false})))
        }.await;
        result.unwrap_or_else(|e| {
            record.borrow_mut()["state"] = json!("uncertain");
            adapter_tool_error(id, &e, "Track-structure undo is uncertain; perform fresh discovery.")
        })
    }
}
