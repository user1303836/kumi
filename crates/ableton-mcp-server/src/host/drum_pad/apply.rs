use super::*;
impl McpHost {
    pub async fn live_drum_pad_apply_async(&self, id: &Value, p: &Value, signal: Option<&Signal>) -> Option<Value> {
        if !valid_transaction_params(p, "apply") {
            return Some(error(id, -32602, "transactionId, confirmation=apply, and idempotencyKey are required", None));
        }
        let record = self.clip_lifecycle_transactions.get(p["transactionId"].as_str().unwrap());
        let t = record.as_ref().map(|r| r.borrow().clone()).unwrap_or(Value::Null);
        if t["kind"] != "drum-pad"
            || (t["state"] == "previewed" && t["expiresAt"].as_f64().is_some_and(|n| n <= kumi_common::time::now_ms_f64()))
        {
            return Some(transaction_error(id, "Unknown or expired drum-pad transaction"));
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
            let action = t["payload"]["action"].as_str().unwrap_or("");
            let payload = &t["payload"];
            if ["load-sample", "sample-chain"].contains(&action) && !reconcile {
                let snapshot = self.views.view_for(Some(&context), &[payload["ref"].clone()], None, &[]).await?;
                let current = self.drum_pad_row(&snapshot, payload["ref"].as_str().unwrap_or(""))?;
                if current["objectIdentity"] != payload["expectedObjectIdentity"] || !rows(&current["chains"]).is_empty() {
                    self.release_staged_import_for(&t);
                    return Ok(transaction_error(id, "drum pad changed since preview; preview again"));
                }
                if !truthy(&t["prior"]["file"]) {
                    self.release_staged_import_for(&t);
                    return Ok(transaction_error(id, "sample file authority is missing; preview again"));
                }
                self.verify_staged_import_file(payload["samplePath"].as_str().unwrap_or(""), &t["prior"]["file"]).await?;
            }
            let batch = if action == "load-samples" { rows(&payload["pads"]) } else { &[] };
            let mut references = vec![payload["deviceRef"].clone()];
            references.extend(batch.iter().map(|p| p["ref"].clone()));
            if action == "load-samples" && !reconcile {
                let snapshot = self.views.view_for(Some(&context), &references, None, &[]).await?;
                for pad in batch {
                    let current = self.drum_pad_row(&snapshot, pad["ref"].as_str().unwrap_or(""))?;
                    if current["objectIdentity"] != pad["expectedObjectIdentity"] || !rows(&current["chains"]).is_empty() {
                        self.release_staged_import_for(&t);
                        return Ok(transaction_error(id, "a drum pad changed since preview; preview again"));
                    }
                }
                let files = t["prior"]["files"].as_array();
                if files.is_none_or(|files| files.len() != batch.len()) {
                    self.release_staged_import_for(&t);
                    return Ok(transaction_error(id, "sample file authority is missing; preview again"));
                }
                for (pad, file) in batch.iter().zip(files.unwrap()) {
                    self.verify_staged_import_file(pad["samplePath"].as_str().unwrap_or(""), file).await?;
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
            let mut args = payload.clone();
            args.as_object_mut().unwrap().remove("action");
            match action {
                "load-samples" => {
                    let result = adapter
                        .invoke_async(
                            &LiveInvocation::new(
                                "drum-pad.load-samples",
                                json!({"pads":batch.iter().map(|p|self.drum_pad_load_args(p)).collect::<Vec<_>>()}),
                            ),
                            Some(&context),
                        )
                        .await?;
                    let loaded: Vec<_> = rows(field(&result, "pads")?).iter().filter(|v| v.is_object()).cloned().collect();
                    if loaded.len() != batch.len()
                        || loaded.iter().enumerate().any(|(i, r)| {
                            r["ref"] != batch[i]["ref"]
                                || !is_non_empty_string(&r["chainIdentity"], 256)
                                || !is_non_empty_string(&r["deviceIdentity"], 256)
                        })
                    {
                        return Err(LiveError::error("drum pad sample loads did not return exact identities"));
                    }
                    let verified = self.views.view_for(Some(&context), &references, None, &[]).await?;
                    for (i, item) in loaded.iter().enumerate() {
                        let pad = self.drum_pad_row(&verified, batch[i]["ref"].as_str().unwrap_or(""))?;
                        let chains = rows(&pad["chains"]);
                        if chains.len() != 1 || chains[0]["objectIdentity"] != item["chainIdentity"] {
                            return Err(LiveError::error("drum pad sample load postcondition was not confirmed"));
                        }
                    }
                    record.borrow_mut()["created"] = json!({"pads":loaded});
                    self.release_drum_sampler_presets(&record.borrow());
                }
                "load-sample" => {
                    let result = adapter
                        .invoke_async(&LiveInvocation::new("drum-pad.load-sample", self.drum_pad_load_args(payload)), Some(&context))
                        .await?;
                    if !is_non_empty_string(field(&result, "chainIdentity")?, 256) || !is_non_empty_string(&result["deviceIdentity"], 256) {
                        return Err(LiveError::error("drum pad sample load did not return exact identities"));
                    }
                    let snapshot = self.views.view_for(Some(&context), &[payload["ref"].clone()], None, &[]).await?;
                    let pad = self.drum_pad_row(&snapshot, payload["ref"].as_str().unwrap_or(""))?;
                    let chains = rows(&pad["chains"]);
                    if chains.len() != 1 || chains[0]["objectIdentity"] != result["chainIdentity"] {
                        return Err(LiveError::error("drum pad sample load postcondition was not confirmed"));
                    }
                    record.borrow_mut()["created"] = result;
                    self.release_drum_sampler_presets(&record.borrow());
                }
                "sample-chain" => {
                    adapter
                        .invoke_async(
                            &LiveInvocation::new(
                                "drum-pad.sample-chain",
                                fields(payload, &["rackRef", "note", "samplePath", "name", "expectedName"]),
                            ),
                            Some(&context),
                        )
                        .await?;
                    let snapshot = self.views.view_for(Some(&context), &[payload["ref"].clone()], None, &[]).await?;
                    let pad = self.drum_pad_row(&snapshot, payload["ref"].as_str().unwrap_or(""))?;
                    let chains = rows(&pad["chains"]);
                    let devices = chains.first().map(|c| rows(&c["devices"])).unwrap_or(&[]);
                    if chains.len() != 1
                        || devices.len() != 1
                        || !is_non_empty_string(&chains[0]["objectIdentity"], 256)
                        || !is_non_empty_string(&devices[0]["objectIdentity"], 256)
                    {
                        return Err(LiveError::error("drum pad sample load postcondition was not confirmed"));
                    }
                    record.borrow_mut()["created"] =
                        json!({"chainIdentity":chains[0]["objectIdentity"],"deviceIdentity":devices[0]["objectIdentity"]});
                }
                "set" => {
                    let result = adapter.invoke_async(&LiveInvocation::new("drum-pad.set", args), Some(&context)).await?;
                    if field(&result, "changed")? != true {
                        return Err(LiveError::error("drum pad change was not confirmed"));
                    }
                    let snapshot = self.views.view_for(Some(&context), &[payload["ref"].clone()], None, &[]).await?;
                    let pad = self.drum_pad_row(&snapshot, payload["ref"].as_str().unwrap_or(""))?;
                    if payload.get("solo").is_some_and(|value| pad.get("solo") != Some(value)) {
                        return Err(LiveError::error("drum pad postcondition was not confirmed"));
                    }
                }
                _ => {
                    let result = adapter.invoke_async(&LiveInvocation::new("drum-pad.delete-all-chains", args), Some(&context)).await?;
                    if field(&result, "deleted")?.as_f64() != t["prior"]["chainCount"].as_f64() {
                        return Err(LiveError::error("delete-all-chains count was not confirmed"));
                    }
                    let snapshot = self.views.view_for(Some(&context), &[payload["ref"].clone()], None, &[]).await?;
                    let pad = self.drum_pad_row(&snapshot, payload["ref"].as_str().unwrap_or(""))?;
                    if !rows(&pad["chains"]).is_empty() {
                        return Err(LiveError::error("delete-all-chains postcondition was not confirmed"));
                    }
                }
            }
            {
                let mut row = record.borrow_mut();
                row["applyKey"] = p["idempotencyKey"].clone();
                row["state"] = json!("applied")
            }
            let mut response = json!({"transactionId":t["id"],"state":"applied"});
            if let Some(value) = record.borrow().get("created").filter(|v| truthy(v)) {
                response["result"] = value.clone()
            }
            response["idempotent"] = json!(false);
            Ok(success_text(id, &response))
        }
        .await;
        Some(result.unwrap_or_else(|e| {
            self.apply_failed(id, &record, &e, "Drum-pad state is uncertain; perform fresh discovery before retrying.")
        }))
    }
    pub async fn undo_drum_pad_async(&self, id: &Value, p: &Value, signal: Option<&Signal>) -> Value {
        let record = p["transactionId"].as_str().and_then(|id| self.clip_lifecycle_transactions.get(id));
        let t = record.as_ref().map(|r| r.borrow().clone()).unwrap_or(Value::Null);
        if t["kind"] != "drum-pad" {
            return transaction_error(id, "Unknown or expired drum-pad transaction");
        }
        let record = record.unwrap();
        let payload = &t["payload"];
        let action = payload["action"].as_str().unwrap_or("");
        if action == "delete-all-chains" {
            return transaction_error(id, "Deleted pad chains cannot be reconstructed; undo is unavailable for this transaction");
        }
        if t["state"] == "undone" && t["undoKey"] == p["idempotencyKey"] {
            return success_text(id, &json!({"transactionId":t["id"],"state":"undone","idempotent":true}));
        }
        let reconcile = t["state"] == "uncertain" && t["undoKey"] == p["idempotencyKey"];
        if (t["state"] != "applied" && !reconcile) || !truthy(&t["prior"]) {
            return transaction_error(id, "Only an applied or exact-key uncertain drum-pad transaction can be undone");
        }
        let result=async{
   self.begin_undo_recovery(&record,p["idempotencyKey"].as_str().unwrap_or(""))?;let status=self.require_connected(Some("session.read"))?;if json!(status.epoch)!=t["epoch"]{return Ok(transaction_error(id,"Live connection epoch changed; undo refused"))}let adapter=self.async_adapter();let context=self.transaction_context(p,signal,AUDITION_DEADLINE_MS);record.borrow_mut()["undoKey"]=p["idempotencyKey"].clone();if reconcile{self.replay_undo_recovery(&record,adapter.as_ref(),&context).await?;}let mut references=vec![payload["ref"].clone(),payload["deviceRef"].clone()];references.extend(rows(&payload["pads"]).iter().filter(|v|v.is_object()).map(|v|v["ref"].clone()));let snapshot=self.views.view_for(Some(&context),&references,None,&[]).await?;
   if action=="load-samples"{
    let batch=rows(&payload["pads"]);let made=rows(&t["created"]["pads"]);let pads=batch.iter().map(|p|self.drum_pad_row(&snapshot,p["ref"].as_str().unwrap_or(""))).collect::<Result<Vec<_>,_>>()?;
    // A retry skips a pad the first try already cleared, but a pad with something on it must still hold what this
    // load made: never the producer's sample dropped there since.
    for(index,pad)in pads.iter().enumerate(){let chains=rows(&pad["chains"]);if reconcile&&chains.is_empty(){continue}let devices=chains.first().map(|c|rows(&c["devices"])).unwrap_or(&[]);if chains.len()!=1||chains[0].get("objectIdentity")!=made.get(index).and_then(|v|v.get("chainIdentity"))||devices.len()!=1||devices[0].get("objectIdentity")!=made.get(index).and_then(|v|v.get("deviceIdentity")){return Ok(transaction_error(id,"a drum pad changed after its sample loaded; undo refused"))}}
    record.borrow_mut()["state"]=json!("undoing");for(index,pad)in pads.iter().enumerate().rev(){if reconcile&&rows(&pad["chains"]).is_empty(){continue}let cleared=self.invoke_undo_recovery(&record,adapter.as_ref(),"drum-pad.delete-all-chains",&json!({"ref":batch[index]["ref"],"expectedObjectIdentity":pad["objectIdentity"],"expectedStateRevision":chain_revision(pad)?}),&context).await?;if !field(&cleared,"deleted")?.is_number(){return Err(LiveError::error("drum pad clearing was not confirmed"))}}self.release_staged_import_for(&t);
   }else{
    let pad=self.drum_pad_row(&snapshot,payload["ref"].as_str().unwrap_or(""))?;if ["load-sample","sample-chain"].contains(&action){let chains=rows(&pad["chains"]);let devices=chains.first().map(|c|rows(&c["devices"])).unwrap_or(&[]);let cleared_before=reconcile&&chains.is_empty();if !cleared_before&&(chains.len()!=1||chains[0].get("objectIdentity")!=t["created"].get("chainIdentity")||devices.len()!=1||devices[0].get("objectIdentity")!=t["created"].get("deviceIdentity")){return Ok(transaction_error(id,"drum pad changed after the sample loaded; undo refused"))}record.borrow_mut()["state"]=json!("undoing");if !cleared_before{let cleared=self.invoke_undo_recovery(&record,adapter.as_ref(),"drum-pad.delete-all-chains",&json!({"ref":payload["ref"],"expectedObjectIdentity":pad["objectIdentity"],"expectedStateRevision":chain_revision(&pad)?}),&context).await?;if !field(&cleared,"deleted")?.is_number(){return Err(LiveError::error("drum pad clearing was not confirmed"))}}self.release_staged_import_for(&t);
    }else{if let Some(moved)=self.undo_target_moved(id,&record.borrow(),"drum pad",&payload["ref"],pad.get("objectIdentity"),payload.get("expectedObjectIdentity"))?{return Ok(moved)}if !reconcile&&payload.get("solo").is_some_and(|value|pad.get("solo")!=Some(value)){return Ok(transaction_error(id,"drum pad changed after apply; undo refused"))}record.borrow_mut()["state"]=json!("undoing");let mut restore=json!({"ref":payload["ref"],"expectedObjectIdentity":pad["objectIdentity"],"expectedStateRevision":digest(&json!({"note":pad["note"],"solo":pad["solo"]}))?});if let Some(value)=t["prior"].get("solo").filter(|v|!v.is_null()){restore["solo"]=value.clone()}let result=self.invoke_undo_recovery(&record,adapter.as_ref(),"drum-pad.set",&restore,&context).await?;if field(&result,"changed")?!=true{return Err(LiveError::error("drum pad restoration was not confirmed"))}}
   }record.borrow_mut()["state"]=json!("undone");Ok(success_text(id,&json!({"transactionId":t["id"],"state":"undone","idempotent":false})))
  }.await;
        result.unwrap_or_else(|e| {
            record.borrow_mut()["state"] = json!("uncertain");
            adapter_tool_error(id, &e, "Drum-pad undo is uncertain; perform fresh discovery.")
        })
    }
}
