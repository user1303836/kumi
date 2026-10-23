//! Insert, enable, and move devices with shared verified sample staging.
use super::device_parameter::DeviceRow;
use super::*;
use kumi_common::{abort::Signal, js::json as js_json, time::now_ms_f64};
use sha2::{Digest, Sha256};
fn track(snapshot: &LiveSnapshot, reference: &Value) -> Option<Value> {
    snapshot.tracks.as_ref()?.iter().find(|t| Some(t.ref_.as_str()) == reference.as_str()).map(|t| serde_json::to_value(t).unwrap())
}
fn fence(reference: &Value, authority: &Value) -> String {
    let mut v = json!({"track":reference});
    v.as_object_mut().unwrap().extend(authority.as_object().unwrap().clone());
    js_json::stringify(&v)
}
fn device_authority(row: &DeviceRow, reference: &Value) -> Value {
    json!({"ref":reference,"expectedObjectIdentity":row.device["objectIdentity"],"expectedOwnerRef":row.owner_ref,"expectedOwnerIdentity":row.owner_identity,"expectedSiblings":row.siblings,"expectedTrackRef":row.track["ref"],"expectedTrackIdentity":row.track["objectIdentity"]})
}
fn enable_revision(value: &Value) -> Result<String, LiveError> {
    Ok(hex::encode(Sha256::digest(canonical_mutation_identity(&json!({"enabled":value}))?)))
}
impl McpHost {
    pub async fn dispatch_device_basic_tool(&self, call: &ToolCall, signal: Option<&Signal>) -> Option<Result<Option<Value>, LiveError>> {
        let p = call.arguments.as_ref().unwrap_or(&Value::Null);
        Some(Ok(match call.name.as_str() {
            "live_device_preview" => Some(self.live_device_preview_async(&call.id, p).await),
            "live_device_apply" => self.live_device_apply_async(&call.id, p, signal).await,
            _ => return None,
        }))
    }
    pub async fn live_device_preview_async(&self, id: &Value, p: &Value) -> Value {
        if p.is_object() && p["action"] == "delete" {
            return transaction_error(
                id,
                "Arbitrary device deletion is unavailable; use live_undo only for an exact transaction-created device",
            );
        }
        if !has_only(p, &["action", "trackRef", "deviceName", "deviceRef", "index", "enabled", "filePath", "allowedRoot"])
            || !matches!(p["action"].as_str(), Some("insert" | "enable" | "move"))
        {
            return error(id, -32602, "action insert/enable/move is required; arbitrary device deletion is unavailable", None);
        }
        if (p.get("filePath").is_some() || p.get("allowedRoot").is_some()) && (p["action"] != "insert" || p["deviceName"] != "Simpler") {
            return error(id, -32602, "a sample file goes only with inserting a Simpler", None);
        }
        let mut staging = Value::Null;
        let result=async{
            let status=self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(reads::AUDITION_DEADLINE_MS)))).await?;
            if !status.connected||!status.capabilities.iter().any(|c|c.as_str()=="session.read"){return Err(LiveError::error("session read capability is unavailable"));}
            let snapshot=self.views.view_for(None,&[p["trackRef"].clone(),p["deviceRef"].clone()],None,&[]).await?;
            let mut payload=json!({"action":p["action"]});let mut prior=None;let fingerprint;
            if p["action"]=="insert"{
                if !status.has_operation("device.insert"){return Err(LiveError::error("device insertion is unavailable"));}
                if !is_non_empty_string(&p["trackRef"],256)||!is_non_empty_string(&p["deviceName"],256){return Ok(error(id,-32602,"trackRef and deviceName are required for insert",None));}
                if p.get("index").is_some_and(|v|!is_integer_in_range(v,-1.,100_000.)){return Ok(error(id,-32602,"index is invalid",None));}
                let track=track(&snapshot,&p["trackRef"]).ok_or_else(||LiveError::error("track is not authoritative"))?;let authority=self.track_device_authority(&track)?;
                payload["trackRef"]=p["trackRef"].clone();payload["deviceName"]=p["deviceName"].clone();payload.as_object_mut().unwrap().extend(authority.as_object().unwrap().clone());
                if let Some(index)=p.get("index"){payload["index"]=index.clone();}fingerprint=fence(&p["trackRef"],&authority);
                if p.get("filePath").is_some()||p.get("allowedRoot").is_some(){let file=self.audio_import_file_authority(&p["filePath"],&p["allowedRoot"]).await?;staging=json!(self.stage_verified_import_file(file["canonicalPath"].as_str().unwrap(),&file).await?);payload["samplePath"]=staging.clone();prior=Some(json!({"file":file}));}
            }else{
                if !is_non_empty_string(&p["deviceRef"],256){return Ok(error(id,-32602,"deviceRef is required",None));}
                let operation=if p["action"]=="enable"{"device.enable"}else{"device.move"};if !status.has_operation(operation){return Err(LiveError::error(format!("{operation} is unavailable")));}
                let row=self.device_row(&snapshot,p["deviceRef"].as_str().unwrap())?;
                if !is_non_empty_string(&row.device["objectIdentity"],256){return Err(LiveError::error("device object identity is unavailable"));}
                if p["action"]=="enable"&&!p["enabled"].is_boolean(){return Ok(error(id,-32602,"enabled must be boolean",None));}
                if p["action"]=="move"&&!is_integer_in_range(&p["index"],0.,100_000.){return Ok(error(id,-32602,"index is invalid",None));}
                payload.as_object_mut().unwrap().extend(device_authority(&row,&p["deviceRef"]).as_object().unwrap().clone());
                if p["action"]=="enable"{if !row.device["enabled"].is_boolean(){return Err(LiveError::error("device enable state is unavailable"));}payload["enabled"]=p["enabled"].clone();payload["expectedStateRevision"]=json!(enable_revision(&row.device["enabled"])?);prior=Some(json!({"enabled":row.device["enabled"]}));}
                if p["action"]=="move"{payload["index"]=p["index"].clone();prior=Some(json!({"index":row.siblings.iter().position(|s|s["ref"]==row.device["ref"]).map(|n|n as i64).unwrap_or(-1)}));}
                fingerprint=self.device_fence(&row);
            }
            let mut t=json!({"id":tempo::transaction_id("device"),"epoch":status.epoch,"kind":"device","fence":fingerprint,"clipRef":if p["deviceRef"].is_null(){p["trackRef"].clone()}else{p["deviceRef"].clone()},"payload":payload,"expiresAt":now_ms_f64()+TRANSACTION_TTL_MS,"state":"previewed"});if let Some(prior)=prior{t["prior"]=prior;}
            self.retain_bounded_transaction(&self.clip_lifecycle_transactions,t.clone(),"device")?;staging=Value::Null;
            let mut reply=json!({"transactionId":t["id"],"epoch":t["epoch"],"action":p["action"],"payload":payload,"impact":format!("device-{}",p["action"].as_str().unwrap()),"confirmation":"apply","expiresAt":t["expiresAt"]});
            if arrangement::truthy(&t["prior"]["file"]){reply["sample"]=json!({"path":t["prior"]["file"]["canonicalPath"],"size":t["prior"]["file"]["size"]});}
            Ok(success_text(id,&reply))
        }.await;
        result.unwrap_or_else(|e| {
            self.release_staged_import_file(&staging);
            adapter_tool_error(id, &e, "Device preview requires fresh authoritative state.")
        })
    }
    pub async fn live_device_apply_async(&self, id: &Value, p: &Value, signal: Option<&Signal>) -> Option<Value> {
        if !valid_transaction_params(p, "apply") {
            return Some(error(id, -32602, "transactionId, confirmation=apply, and idempotencyKey are required", None));
        }
        let Some(record) = self.clip_lifecycle_transactions.get(p["transactionId"].as_str().unwrap()).filter(|r| {
            let t = r.borrow();
            t["kind"] == "device" && !(t["state"] == "previewed" && t["expiresAt"].as_f64().unwrap_or(0.) <= now_ms_f64())
        }) else {
            return Some(transaction_error(id, "Unknown or expired device transaction"));
        };
        let t = record.borrow().clone();
        if t["state"] == "applied" && t["applyKey"] == p["idempotencyKey"] {
            let mut reply = json!({"transactionId":t["id"],"state":"applied"});
            if let Some(created) = t.get("created") {
                reply["created"] = created.clone();
            }
            reply["idempotent"] = json!(true);
            return Some(success_text(id, &reply));
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
            let adapter = self.async_adapter();
            let context = self.transaction_context(p, signal, reads::AUDITION_DEADLINE_MS);
            let action = t["payload"]["action"].as_str().unwrap();
            if !reconciliation {
                let snapshot = self
                    .views
                    .view_for(Some(&context), &[t["payload"]["trackRef"].clone(), t["payload"]["ref"].clone()], None, &[])
                    .await?;
                if action == "insert" {
                    let row = track(&snapshot, &t["payload"]["trackRef"]);
                    let current =
                        row.as_ref().map(|r| self.track_device_authority(r).map(|a| fence(&t["payload"]["trackRef"], &a))).transpose()?;
                    if current.as_deref() != t["fence"].as_str() {
                        self.release_staged_import_for(&t);
                        return Ok(transaction_error(id, "track identity or devices changed since preview; preview again"));
                    }
                    if let Some(sample) = t["payload"]["samplePath"].as_str() {
                        let file = &t["prior"]["file"];
                        if !arrangement::truthy(file) {
                            self.release_staged_import_for(&t);
                            return Ok(transaction_error(id, "sample file authority is missing; preview again"));
                        }
                        self.verify_staged_import_file(sample, file).await?;
                    }
                } else {
                    let located = self.device_row(&snapshot, t["payload"]["ref"].as_str().unwrap())?;
                    if json!(self.device_fence(&located)) != t["fence"] {
                        return Ok(transaction_error(id, "device state changed since preview; preview again"));
                    }
                }
            }
            let operation = match action {
                "insert" => "device.insert",
                "enable" => "device.enable",
                _ => "device.move",
            };
            let mut args = t["payload"].clone();
            args.as_object_mut().unwrap().remove("action");
            if let Err(error) = self.keep_staged(&t) {
                return Ok(transaction_error(
                    id,
                    &format!("the staged sample couldn't be kept for the Set ({}); nothing was sent to Live", error.message()),
                ));
            }
            record.borrow_mut()["state"] = json!("applying");
            record.borrow_mut()["applyKey"] = p["idempotencyKey"].clone();
            let mut result = adapter.invoke_async(&LiveInvocation::new(operation, args), Some(&context)).await?;
            if (action == "insert" || action == "move") && result.is_null() {
                return Err(LiveError::type_error("Cannot read properties of null (reading 'ref')"));
            }
            if (action == "insert" || action == "move")
                && (!is_non_empty_string(&result["ref"], 256)
                    || !is_non_empty_string(&result["objectIdentity"], 256)
                    || (action == "insert" && !is_non_empty_string(&result["createdFingerprint"], 64)))
            {
                return Err(LiveError::error(format!("device {action} did not return exact identity")));
            }
            if action == "move" && result["objectIdentity"] != t["payload"]["expectedObjectIdentity"] {
                return Err(LiveError::error("device move returned a different object identity"));
            }
            record.borrow_mut()["created"] = result.clone();
            if action == "insert" {
                let snapshot =
                    self.views.view_for(Some(&context), &[result["ref"].clone(), t["payload"]["trackRef"].clone()], None, &[]).await?;
                let created = self.device_row(&snapshot, result["ref"].as_str().unwrap())?;
                if created.device["objectIdentity"] != result["objectIdentity"]
                    || json!(arrangement::capture_object_fingerprint(&owned_device_fingerprint_row(&created.device))?)
                        != result["createdFingerprint"]
                {
                    return Err(LiveError::error("inserted device identity or creation fingerprint was not confirmed"));
                }
                result["fingerprint"] = result["createdFingerprint"].clone();
                record.borrow_mut()["created"] = result.clone();
            }
            record.borrow_mut()["applyKey"] = p["idempotencyKey"].clone();
            record.borrow_mut()["state"] = json!("applied");
            Ok(success_text(id, &json!({"transactionId":t["id"],"state":"applied","result":result,"idempotent":false})))
        }
        .await;
        Some(
            result.unwrap_or_else(|e| {
                self.apply_failed(id, &record, &e, "Device state is uncertain; perform fresh discovery before retrying.")
            }),
        )
    }
    pub async fn undo_device_basic_async(&self, id: &Value, p: &Value, signal: Option<&Signal>) -> Value {
        let Some(record) =
            self.clip_lifecycle_transactions.get(p["transactionId"].as_str().unwrap()).filter(|r| r.borrow()["kind"] == "device")
        else {
            return transaction_error(id, "Unknown device transaction");
        };
        let t = record.borrow().clone();
        if t["state"] == "undone" && t["undoKey"] == p["idempotencyKey"] {
            return success_text(id, &json!({"transactionId":t["id"],"state":"undone","idempotent":true}));
        }
        let reconciliation = t["state"] == "uncertain" && t["undoKey"] == p["idempotencyKey"];
        if t["state"] != "applied" && !reconciliation {
            return transaction_error(id, "Only an applied or exact-key uncertain device transaction can be undone");
        }
        let result = async {
            let (_, steps) = self.begin_undo_recovery(&record, p["idempotencyKey"].as_str().unwrap())?;
            let status = self.require_connected(Some("session.read"))?;
            if json!(status.epoch) != t["epoch"] {
                return Ok(transaction_error(id, "Live connection epoch changed; undo refused"));
            }
            let adapter = self.async_adapter();
            let context = self.transaction_context(p, signal, reads::AUDITION_DEADLINE_MS);
            let action = t["payload"]["action"].as_str().unwrap();
            record.borrow_mut()["undoKey"] = p["idempotencyKey"].clone();
            if reconciliation {
                self.replay_undo_recovery(&record, adapter.as_ref(), &context).await?;
                if let Some(replayed) =
                    steps.last().map(|s| s.borrow()["result"].clone()).filter(|v| v.is_object() && is_non_empty_string(&v["ref"], 256))
                {
                    if action == "move" {
                        record.borrow_mut()["created"] = replayed;
                    }
                }
            }
            record.borrow_mut()["state"] = json!("undoing");
            if action == "insert" {
                if !is_non_empty_string(&t["created"]["ref"], 256) || !is_non_empty_string(&t["created"]["objectIdentity"], 256) {
                    return Err(LiveError::error("inserted device identity is unavailable"));
                }
                self.delete_owned_device_async(
                    adapter.as_ref(),
                    t["created"]["ref"].as_str().unwrap(),
                    t["created"]["objectIdentity"].as_str().unwrap(),
                    &context,
                    t["created"]["createdFingerprint"].as_str(),
                    Some(&record),
                    reconciliation,
                )
                .await?;
                self.release_staged_import_for(&t);
            } else {
                let reference = if action == "move" { record.borrow()["created"]["ref"].clone() } else { t["payload"]["ref"].clone() };
                let snapshot = self.views.view_for(Some(&context), &[reference.clone()], None, &[]).await?;
                let located = self.device_row(&snapshot, reference.as_str().unwrap_or(""))?;
                if action == "enable" {
                    if let Some(moved) = Self::moved_target(
                        "device",
                        &reference,
                        located.device.get("objectIdentity"),
                        t["payload"].get("expectedObjectIdentity"),
                    )? {
                        return Err(LiveError::error(moved));
                    }
                }
                if reconciliation {
                    if action == "enable" && located.device["enabled"] != t["prior"]["enabled"] {
                        return Err(LiveError::error("device-enable undo replay did not restore prior state"));
                    }
                    if action == "move"
                        && json!(located.siblings.iter().position(|s| s["ref"] == reference).map(|n| n as i64).unwrap_or(-1))
                            != t["prior"]["index"]
                    {
                        return Err(LiveError::error("device-move undo replay did not restore prior location"));
                    }
                } else {
                    let mut args = device_authority(&located, &reference);
                    if action == "enable" {
                        if located.device["enabled"] != t["payload"]["enabled"] || !t["prior"]["enabled"].is_boolean() {
                            return Err(LiveError::error("device enable state changed after apply"));
                        }
                        args["enabled"] = t["prior"]["enabled"].clone();
                        args["expectedStateRevision"] = json!(enable_revision(&located.device["enabled"])?);
                    } else if action == "move" {
                        let index = located.siblings.iter().position(|s| s["ref"] == reference).map(|n| n as i64).unwrap_or(-1);
                        if located.device["objectIdentity"] != t["payload"]["expectedObjectIdentity"]
                            || json!(index) != t["created"]["index"]
                            || !is_integer_in_range(&t["prior"]["index"], 0., 100_000.)
                        {
                            return Err(LiveError::error("moved device identity or location changed after apply"));
                        }
                        args["index"] = t["prior"]["index"].clone();
                    } else {
                        return Err(LiveError::error("arbitrary device deletion has no automatic undo authority"));
                    }
                    let result = self
                        .invoke_undo_recovery(
                            &record,
                            adapter.as_ref(),
                            if action == "enable" { "device.enable" } else { "device.move" },
                            &args,
                            &context,
                        )
                        .await?;
                    if result.is_null() {
                        return Err(LiveError::type_error("Cannot read properties of null (reading 'changed')"));
                    }
                    if result["changed"] != true && !is_non_empty_string(&result["ref"], 256) {
                        return Err(LiveError::error("device restoration was not confirmed"));
                    }
                    if action == "move" {
                        record.borrow_mut()["created"] = result;
                    }
                }
            }
            record.borrow_mut()["state"] = json!("undone");
            Ok(success_text(id, &json!({"transactionId":t["id"],"state":"undone","idempotent":false})))
        }
        .await;
        result.unwrap_or_else(|e| {
            record.borrow_mut()["state"] = json!("uncertain");
            adapter_tool_error(id, &e, "Device undo is uncertain; inspect the exact device hierarchy.")
        })
    }
}
