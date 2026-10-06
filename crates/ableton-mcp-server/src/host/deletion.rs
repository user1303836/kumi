//! Explicit producer-requested deletion uses current object and hierarchy identities.
use super::*;
use kumi_common::{abort::Signal, js::json as js_json, time::now_ms_f64};
const KEPT: &str = "Kumi can't bring this back; Live's undo can.";
struct Plan {
    operation: String,
    payload: Value,
    target: Value,
}
fn device_fence(reference: &Value, row: &device_parameter::DeviceRow) -> String {
    js_json::stringify(
        &json!({"ref":reference,"objectIdentity":row.device["objectIdentity"],"ownerRef":row.owner_ref,"ownerIdentity":row.owner_identity,"siblings":row.siblings,"trackRef":row.track["ref"],"trackIdentity":row.track["objectIdentity"]}),
    )
}
/// Whether a device that is `identity` is still in `snapshot`'s tracks, in chains and drum pads too. Live's device
/// refs are positional: the next device takes a deleted one's ref, so only its identity says it's still there.
fn device_remains(snapshot: &LiveSnapshot, identity: &Value) -> bool {
    fn walk(devices: &Value, identity: &Value, depth: usize) -> bool {
        depth < 64
            && devices.as_array().into_iter().flatten().any(|device| {
                device["objectIdentity"] == *identity
                    || device["chains"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .chain(
                            device["drumPads"]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .flat_map(|pad| pad["chains"].as_array().into_iter().flatten()),
                        )
                        .any(|chain| walk(&chain["devices"], identity, depth + 1))
            })
    }
    let snapshot = serde_json::to_value(snapshot).unwrap();
    snapshot["tracks"].as_array().into_iter().flatten().any(|track| walk(&track["devices"], identity, 0))
}
impl McpHost {
    pub async fn dispatch_deletion_tool(&self, call: &ToolCall, signal: Option<&Signal>) -> Option<Result<Option<Value>, LiveError>> {
        let p = call.arguments.as_ref().unwrap_or(&Value::Null);
        if call.name == "live_device_delete_preview" {
            return Some(Ok(Some(self.live_device_delete_preview_async(&call.id, p).await)));
        }
        if call.name == "live_device_delete_apply" {
            return Some(Ok(self.live_device_delete_apply_async(&call.id, p, signal).await));
        }
        for kind in ["clip", "scene", "track", "locator"] {
            if call.name == format!("live_{kind}_delete_preview") {
                return Some(Ok(Some(self.live_deletion_preview_async(&call.id, p, kind).await)));
            }
            if call.name == format!("live_{kind}_delete_apply") {
                return Some(Ok(self.live_deletion_apply_async(&call.id, p, kind, signal).await));
            }
        }
        None
    }
    pub async fn live_device_delete_preview_async(&self, id: &Value, p: &Value) -> Value {
        if !has_only(p, &["ref"]) || !is_non_empty_string(&p["ref"], 256) {
            return error(id, -32602, "ref is required", None);
        }
        let result=async{
            let status=self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(reads::AUDITION_DEADLINE_MS)))).await?;if !status.connected||!status.capabilities.iter().any(|c|c.as_str()=="session.read"){return Err(LiveError::error("session read capability is unavailable"));}if !status.has_operation("device.delete"){return Err(LiveError::error("device deletion is unavailable"));}
            let snapshot=self.views.view_for(None,&[p["ref"].clone()],None,&[]).await?;let row=self.device_row(&snapshot,p["ref"].as_str().unwrap())?;
            let payload=json!({"ref":p["ref"],"expectedObjectIdentity":row.device["objectIdentity"],"expectedOwnerRef":row.owner_ref,"expectedOwnerIdentity":row.owner_identity,"expectedSiblings":row.siblings,"expectedTrackRef":row.track["ref"],"expectedTrackIdentity":row.track["objectIdentity"],"explicitDeletion":true});
            let mut prior=json!({});if let Some(name)=row.device.get("name"){prior["name"]=name.clone();}let t=json!({"id":tempo::transaction_id("devdel"),"epoch":status.epoch,"kind":"device-delete","fence":device_fence(&p["ref"],&row),"payload":payload,"prior":prior,"expiresAt":now_ms_f64()+TRANSACTION_TTL_MS,"state":"previewed"});self.retain_bounded_transaction(&self.clip_lifecycle_transactions,t.clone(),"device delete")?;
            let mut device=json!({});for field in ["name","kind"]{if let Some(v)=row.device.get(field){device[field]=v.clone();}}Ok(success_text(id,&json!({"transactionId":t["id"],"epoch":t["epoch"],"ref":p["ref"],"device":device,"impact":"deletes-device-no-undo","confirmation":"apply","expiresAt":t["expiresAt"]})))
        }.await;
        result.unwrap_or_else(|e| adapter_tool_error(id, &e, "Device-delete preview requires fresh authoritative state."))
    }
    pub async fn live_device_delete_apply_async(&self, id: &Value, p: &Value, signal: Option<&Signal>) -> Option<Value> {
        if !valid_transaction_params(p, "apply") {
            return Some(error(id, -32602, "transactionId, confirmation=apply, and idempotencyKey are required", None));
        }
        let Some(record) = self.clip_lifecycle_transactions.get(p["transactionId"].as_str().unwrap()).filter(|r| {
            let t = r.borrow();
            t["kind"] == "device-delete" && !(t["state"] == "previewed" && t["expiresAt"].as_f64().unwrap_or(0.) <= now_ms_f64())
        }) else {
            return Some(transaction_error(id, "Unknown or expired device-delete transaction"));
        };
        let t = record.borrow().clone();
        if t["state"] == "applied" && t["applyKey"] == p["idempotencyKey"] {
            return Some(success_text(id, &json!({"transactionId":t["id"],"state":"applied","idempotent":true})));
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
                let snapshot = self.views.view_for(Some(&context), &[t["payload"]["ref"].clone()], None, &[]).await?;
                let row = self.device_row(&snapshot, t["payload"]["ref"].as_str().unwrap())?;
                if device_fence(&t["payload"]["ref"], &row) != t["fence"] {
                    return Ok(transaction_error(id, "device or sibling hierarchy changed since preview; preview again"));
                }
            }
            record.borrow_mut()["state"] = json!("applying");
            record.borrow_mut()["applyKey"] = p["idempotencyKey"].clone();
            let result =
                self.async_adapter().invoke_async(&LiveInvocation::new("device.delete", t["payload"].clone()), Some(&context)).await?;
            if result.is_null() {
                return Err(LiveError::type_error("Cannot read properties of null (reading 'deleted')"));
            }
            if result["deleted"] != t["payload"]["ref"] {
                return Err(LiveError::error("device deletion was not confirmed"));
            }
            let after = self
                .views
                .view_for(Some(&context), &[t["payload"]["ref"].clone(), t["payload"]["expectedTrackRef"].clone()], None, &[])
                .await?;
            if device_remains(&after, &t["payload"]["expectedObjectIdentity"]) {
                return Err(LiveError::error("deleted device remains discoverable after apply"));
            }
            record.borrow_mut()["applyKey"] = p["idempotencyKey"].clone();
            record.borrow_mut()["state"] = json!("applied");
            Ok(success_text(id, &json!({"transactionId":t["id"],"state":"applied","kept":KEPT,"idempotent":false})))
        }
        .await;
        Some(result.unwrap_or_else(|e| {
            record.borrow_mut()["state"] = json!("uncertain");
            adapter_tool_error(id, &e, "Device state is uncertain; perform fresh discovery before retrying.")
        }))
    }
    async fn deletion_plan(&self, kind: &str, reference: &str, context: Option<&LiveOperationContext>) -> Result<Plan, LiveError> {
        if kind == "clip" {
            let snapshot = self.views.view_for(context, &[json!(reference)], None, &[]).await?;
            let located = self.clip_row(&snapshot, reference)?;
            if located.take_lane.is_some() {
                return Err(LiveError::error("a take lane's clip isn't deleted here: edit the take lane in Live"));
            }
            let track = located.track.as_ref().unwrap_or(&Value::Null);
            let mut target = json!({"ref":reference,"name":located.clip["name"],"arrangement":located.arrangement,"trackRef":track["ref"],"trackName":track["name"]});
            if located.arrangement {
                target["start"] = located.clip["start"].clone();
                target["length"] = located.clip["length"].clone();
            }
            let mut payload = json!({"ref":reference});
            for (k, v) in self.clip_authority(&snapshot, reference)?.as_object().unwrap() {
                payload[k] = v.clone();
            }
            payload["explicitDeletion"] = json!(true);
            return Ok(Plan {
                operation: if located.arrangement { "arrangement.clip.delete" } else { "clip.delete" }.into(),
                payload,
                target,
            });
        }
        if kind == "locator" {
            let snapshot = self.views.view(context, LiveViewScope::Indices(vec![]), Some(&[LiveSnapshotPart::Arrangement])).await?;
            let s = serde_json::to_value(&snapshot).unwrap();
            let locator = s["arrangement"]["locators"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|r| r["ref"] == reference)
                .ok_or_else(|| LiveError::error("locator reference is not authoritative"))?;
            let mut payload = self.locator_delete_args(&snapshot, &json!(reference), None)?;
            payload["explicitDeletion"] = json!(true);
            return Ok(Plan {
                operation: "locator.delete".into(),
                payload,
                target: json!({"ref":reference,"name":locator["name"],"position":locator["position"]}),
            });
        }
        let snapshot = self.structure_view(context).await?;
        let s = serde_json::to_value(&snapshot).unwrap();
        let rows = s[if kind == "scene" { "scenes" } else { "tracks" }]
            .as_array()
            .ok_or_else(|| LiveError::type_error("Cannot read properties of undefined (reading 'find')"))?;
        let row = rows
            .iter()
            .find(|r| r["ref"] == reference)
            .filter(|r| is_non_empty_string(&r["objectIdentity"], 256))
            .ok_or_else(|| LiveError::error(format!("{kind} reference is not authoritative")))?;
        let target = if kind == "scene" {
            if rows.len() < 2 {
                return Err(LiveError::error("a Set keeps at least one scene"));
            }
            json!({"ref":reference,"name":row["name"],"index":row["index"]})
        } else {
            if row["kind"] == "return" {
                return Err(LiveError::error("a return track is deleted with live_track_structure (action delete-return)"));
            }
            if row["kind"] == "main" {
                return Err(LiveError::error("the Main track can't be deleted"));
            }
            fn inside(rows: &[Value], group: &Value, depth: usize) -> Vec<Value> {
                if depth > 64 {
                    return vec![];
                }
                let mut out = vec![];
                for row in rows.iter().filter(|r| r.get("groupTrackRef") == Some(group)) {
                    out.push(row["name"].clone());
                    out.extend(inside(rows, &row["ref"], depth + 1));
                }
                out
            }
            let grouped = inside(rows, &json!(reference), 0);
            let mut target = json!({"ref":reference,"name":row["name"],"kind":row["kind"]});
            if !grouped.is_empty() {
                target["alsoDeletes"] = json!(grouped);
            }
            target
        };
        Ok(Plan {
            operation: format!("{kind}.delete"),
            payload: json!({"ref":reference,"expectedStructureRevision":self.structure_revision(&snapshot),"expectedObjectIdentity":row["objectIdentity"],"explicitDeletion":true}),
            target,
        })
    }
    pub async fn live_deletion_preview_async(&self, id: &Value, p: &Value, kind: &str) -> Value {
        let field = format!("{kind}Ref");
        if !has_only(p, &[&field]) || !is_non_empty_string(&p[&field], 256) {
            return error(id, -32602, &format!("{field} is required"), None);
        }
        let reference = p[&field].as_str().unwrap();
        let result=async{
            let status=self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(reads::AUDITION_DEADLINE_MS)))).await?;if !status.connected{return Err(LiveError::error("Live is not connected"));}let context=LiveOperationContext::with_deadline(self.deadline(reads::AUDITION_DEADLINE_MS));let plan=self.deletion_plan(kind,reference,Some(&context)).await?;if !status.has_operation(&plan.operation){return Err(LiveError::error(format!("{} is unavailable on this Live shape",plan.operation)));}
            let fence=canonical_mutation_identity(&plan.payload)?;let mut payload=plan.payload;payload["operation"]=json!(plan.operation);let t=json!({"id":tempo::transaction_id(&format!("{kind}del")),"epoch":status.epoch,"kind":format!("{kind}-delete"),"fence":fence,"clipRef":reference,"payload":payload,"prior":plan.target,"expiresAt":now_ms_f64()+TRANSACTION_TTL_MS,"state":"previewed"});self.retain_bounded_transaction(&self.clip_lifecycle_transactions,t.clone(),&format!("{kind} delete"))?;
            let mut response=json!({"transactionId":t["id"],"epoch":t["epoch"]});response[kind]=plan.target;response["impact"]=json!(format!("deletes-{kind}-no-undo"));response["kept"]=json!(KEPT);response["confirmation"]=json!("apply");response["expiresAt"]=t["expiresAt"].clone();Ok(success_text(id,&response))
        }.await;
        result.unwrap_or_else(|e| {
            adapter_tool_error(
                id,
                &e,
                &format!("The {kind} wasn't deleted; discover it again and preview the deletion from fresh references."),
            )
        })
    }
    pub async fn live_deletion_apply_async(&self, id: &Value, p: &Value, kind: &str, signal: Option<&Signal>) -> Option<Value> {
        if !valid_transaction_params(p, "apply") {
            return Some(error(id, -32602, "transactionId, confirmation=apply, and idempotencyKey are required", None));
        }
        let Some(record) = self.clip_lifecycle_transactions.get(p["transactionId"].as_str().unwrap()).filter(|r| {
            let t = r.borrow();
            t["kind"] == format!("{kind}-delete")
                && arrangement::truthy(&t["clipRef"])
                && !(t["state"] == "previewed" && t["expiresAt"].as_f64().unwrap_or(0.) <= now_ms_f64())
        }) else {
            return Some(transaction_error(id, &format!("Unknown or expired {kind}-delete transaction")));
        };
        let t = record.borrow().clone();
        if t["state"] == "applied" && t["applyKey"] == p["idempotencyKey"] {
            return Some(success_text(
                id,
                &json!({"transactionId":t["id"],"state":"applied","deleted":t["clipRef"],"kept":KEPT,"idempotent":true}),
            ));
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
            let status = self.require_connected(None)?;
            if json!(status.epoch) != t["epoch"] {
                return Ok(transaction_error(id, "Live connection epoch changed; preview again"));
            }
            let context = self.transaction_context(p, signal, reads::AUDITION_DEADLINE_MS);
            let reference = t["clipRef"].as_str().unwrap();
            if !reconciliation {
                let current = self.deletion_plan(kind, reference, Some(&context)).await.ok();
                if current.as_ref().map(|p| canonical_mutation_identity(&p.payload)).transpose()?.is_none_or(|f| f != t["fence"]) {
                    return Ok(transaction_error(id, &format!("the {kind} or what surrounds it changed since the preview; preview again")));
                }
            }
            record.borrow_mut()["state"] = json!("applying");
            record.borrow_mut()["applyKey"] = p["idempotencyKey"].clone();
            let mut payload = t["payload"].clone();
            let operation = payload.as_object_mut().unwrap().remove("operation").unwrap();
            let result = self
                .async_adapter()
                .invoke_async(&LiveInvocation::new(operation.as_str().unwrap(), payload.clone()), Some(&context))
                .await?;
            if result.is_null() {
                return Err(LiveError::type_error("Cannot read properties of null (reading 'deleted')"));
            }
            if result["deleted"] != t["clipRef"] {
                return Err(LiveError::error(format!("Live didn't confirm deleting the {kind}")));
            }
            if self
                .deletion_plan(kind, reference, Some(&context))
                .await
                .ok()
                .is_some_and(|p| p.payload["expectedObjectIdentity"] == payload["expectedObjectIdentity"])
            {
                return Err(LiveError::error(format!("the deleted {kind} is still there")));
            }
            record.borrow_mut()["state"] = json!("applied");
            Ok(success_text(id, &json!({"transactionId":t["id"],"state":"applied","deleted":t["clipRef"],"kept":KEPT,"idempotent":false})))
        }
        .await;
        Some(result.unwrap_or_else(|e| {
            if record.borrow()["state"] == "applying" || reconciliation {
                record.borrow_mut()["state"] = json!("uncertain");
            }
            adapter_tool_error(
                id,
                &e,
                &format!("Whether the {kind} is gone is uncertain; discover it again before retrying with the same key."),
            )
        }))
    }
}
