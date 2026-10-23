//! Renames retain the target's exact identity/name and reconcile only the original mutation key.
use super::*;
use super::{
    device_parameter::{authority_fields, fields},
    reads::AUDITION_DEADLINE_MS,
};
use kumi_common::{abort::Signal, js::json as js_json};
use sha2::{Digest, Sha256};
fn hash(value: &Value) -> Result<String, LiveError> {
    Ok(hex::encode(Sha256::digest(canonical_mutation_identity(value)?)))
}
fn operation(kind: &str) -> String {
    if kind == "takeLane" {
        "take-lane.rename".into()
    } else {
        format!("{kind}.rename")
    }
}
fn fence(current: &Value, kind: &Value) -> Value {
    let mut out = fields(current, &["ref", "objectIdentity", "name"]);
    out["kind"] = kind.clone();
    out
}
impl McpHost {
    pub async fn dispatch_rename_tool(&self, call: &ToolCall, signal: Option<&Signal>) -> Option<Result<Value, LiveError>> {
        let args = call.arguments.as_ref().unwrap_or(&Value::Null);
        Some(match call.name.as_str() {
            "live_object_rename_preview" => Ok(self.live_object_rename_preview_async(&call.id, args).await),
            "live_object_rename_apply" => Ok(self.live_object_rename_apply_async(&call.id, args, signal).await),
            _ => return None,
        })
    }
    pub(super) fn take_lane_row(&self, snapshot: &LiveSnapshot, reference: &str) -> Result<(Value, Value), LiveError> {
        let snapshot = serde_json::to_value(snapshot).unwrap();
        for track in snapshot["tracks"].as_array().into_iter().flatten() {
            if let Some(lane) = track["takeLanes"].as_array().into_iter().flatten().find(|r| r["ref"] == reference) {
                return Ok((track.clone(), lane.clone()));
            }
        }
        Err(LiveError::error("take-lane reference is unknown"))
    }
    pub(super) async fn rename_target_async(
        &self,
        context: Option<&LiveOperationContext>,
        kind: &str,
        reference: &str,
    ) -> Result<Option<Value>, LiveError> {
        if kind == "track" {
            return self.track_one_async(context, reference, &["ref", "objectIdentity", "name", "kind"]).await;
        }
        if kind == "scene" {
            return Ok(self
                .discover_one_async(context, LiveDiscoveryKind::Scene, reference, Some(&["ref", "objectIdentity", "name"]), None)
                .await?
                .map(|mut row| {
                    row["kind"] = json!("scene");
                    row
                }));
        }
        let fallback = LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS));
        self.async_adapter().get_async(&LiveRef(reference.into()), Some(context.unwrap_or(&fallback))).await
    }
    pub(super) async fn rename_revision_async(
        &self,
        context: Option<&LiveOperationContext>,
        kind: &str,
        reference: &str,
        current: &Value,
    ) -> Result<String, LiveError> {
        if kind == "track" || kind == "scene" {
            return hash(&authority_fields(current, &["ref", "objectIdentity", "name"])?);
        }
        self.rename_authority_revision(&self.views.view_for(context, &[json!(reference)], None, &[]).await?, kind, reference)
    }
    pub(super) fn rename_authority_revision(&self, snapshot: &LiveSnapshot, kind: &str, reference: &str) -> Result<String, LiveError> {
        match kind {
            "track" | "scene" => Ok(self.structure_revision(snapshot)),
            "locator" => self.locator_revision(snapshot),
            "takeLane" => {
                let (track, _) = self.take_lane_row(snapshot, reference)?;
                let siblings = track["takeLanes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|lane| authority_fields(lane, &["ref", "objectIdentity", "name"]))
                    .collect::<Result<Vec<_>, _>>()?;
                hash(&json!(siblings))
            }
            "clip" => hash(&self.clip_authority(snapshot, reference)?),
            "device" => {
                let row = self.device_row(snapshot, reference)?;
                hash(
                    &json!({"ref":row.device["ref"],"objectIdentity":row.device["objectIdentity"],"trackRef":row.track["ref"],"trackIdentity":row.track["objectIdentity"],"ownerRef":row.owner_ref,"ownerIdentity":row.owner_identity,"siblings":row.siblings}),
                )
            }
            _ => Err(LiveError::error("rename authority kind is unsupported")),
        }
    }
    pub async fn live_object_rename_preview_async(&self, id: &Value, params: &Value) -> Value {
        if !has_only(params, &["kind", "ref", "name"])
            || !matches!(params["kind"].as_str(), Some("track" | "scene" | "clip" | "device" | "locator" | "takeLane"))
            || !is_non_empty_string(&params["ref"], 256)
            || !is_non_empty_string(&params["name"], 256)
        {
            return error(id, -32602, "kind, ref, and a non-empty name are required", None);
        }
        let result = async {
            self.require_connected(Some("session.read"))?;
            let kind = params["kind"].as_str().unwrap();
            let reference = params["ref"].as_str().unwrap();
            let status = self.require_operation(&operation(kind)).await?;
            let context = LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS));
            let current = self.rename_target_async(Some(&context), kind, reference).await?.unwrap_or(Value::Null);
            if kind == "track" && !matches!(current["kind"].as_str(), Some("regular" | "group" | "return" | "main")) {
                return Err(LiveError::error("track rename is limited to Set and return tracks"));
            }
            if kind == "scene" && current["kind"] != "scene" {
                return Err(LiveError::error("scene reference is not authoritative"));
            }
            if current["ref"] != params["ref"] || !is_non_empty_string(&current["objectIdentity"], 256) || !current["name"].is_string() {
                return Err(LiveError::error("rename target lacks exact authoritative object identity"));
            }
            let old = current["name"].as_str().unwrap();
            let prefix = if kind == "track"
                && current["kind"] == "return"
                && old.as_bytes().first().is_some_and(u8::is_ascii_uppercase)
                && old.as_bytes().get(1) == Some(&b'-')
            {
                &old[..2]
            } else {
                ""
            };
            let requested = params["name"].as_str().unwrap();
            let name = if !prefix.is_empty() && !requested.starts_with(prefix) { format!("{prefix}{requested}") } else { requested.into() };
            if old == name {
                return Err(LiveError::error("rename would not change the target"));
            }
            let transaction = json!({"id":tempo::transaction_id("rename"),"epoch":status.epoch,"kind":"rename","fence":js_json::stringify(&fence(&current,&params["kind"])),"clipRef":reference,"payload":{"kind":kind,"name":name,"expectedAuthorityRevision":self.rename_revision_async(Some(&context),kind,reference,&current).await?},"prior":{"name":current["name"],"objectIdentity":current["objectIdentity"]},"expiresAt":kumi_common::time::now_ms_f64()+TRANSACTION_TTL_MS,"state":"previewed"});
            self.retain_bounded_transaction(&self.clip_lifecycle_transactions, transaction.clone(), "rename")?;
            Ok(success_text(id, &json!({"transactionId":transaction["id"],"epoch":transaction["epoch"],"target":{"kind":kind,"ref":reference,"objectIdentity":current["objectIdentity"],"currentName":current["name"]},"proposedName":name,"impact":"renames-one-live-object","confirmation":"apply","expiresAt":transaction["expiresAt"]})))
        }
        .await;
        result
            .unwrap_or_else(|cause| adapter_tool_error(id, &cause, "Rename preview failed without mutation; rediscover the exact target."))
    }
    pub async fn live_object_rename_apply_async(&self, id: &Value, params: &Value, signal: Option<&Signal>) -> Value {
        if !valid_transaction_params(params, "apply") {
            return error(id, -32602, "transactionId, confirmation=apply, and idempotencyKey are required", None);
        }
        let Some(record) = self.clip_lifecycle_transactions.get(params["transactionId"].as_str().unwrap()) else {
            return transaction_error(id, "Unknown or expired rename transaction");
        };
        let transaction = record.borrow().clone();
        if transaction["kind"] != "rename"
            || !transaction["clipRef"].as_str().is_some_and(|s| !s.is_empty())
            || (transaction["state"] == "previewed"
                && transaction["expiresAt"].as_f64().is_some_and(|v| v <= kumi_common::time::now_ms_f64()))
        {
            return transaction_error(id, "Unknown or expired rename transaction");
        }
        if transaction["state"] == "applied" && transaction["applyKey"] == params["idempotencyKey"] {
            return success_text(
                id,
                &json!({"transactionId":transaction["id"],"state":"applied","name":transaction["payload"]["name"],"idempotent":true}),
            );
        }
        let reconciliation = transaction["state"] == "uncertain" && transaction["applyKey"] == params["idempotencyKey"];
        if transaction["state"] == "uncertain" && !reconciliation {
            return transaction_error(id, "Uncertain rename apply requires the exact original idempotency key");
        }
        if transaction["state"] != "previewed" && !reconciliation {
            return transaction_error(id, "Rename transaction is no longer applicable");
        }
        let result = async {
            let status = if reconciliation {
                self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(AUDITION_DEADLINE_MS)))).await?
            } else {
                self.require_connected(Some("session.read"))?
            };
            if json!(status.epoch) != transaction["epoch"] {
                return Ok(transaction_error(id, "Live connection epoch changed; preview again"));
            }
            let context = self.transaction_context(params, signal, AUDITION_DEADLINE_MS);
            let kind = transaction["payload"]["kind"].as_str().unwrap();
            let reference = transaction["clipRef"].as_str().unwrap();
            let current = self.rename_target_async(Some(&context), kind, reference).await?.unwrap_or(Value::Null);
            let exact = current["ref"] == transaction["clipRef"] && current["objectIdentity"] == transaction["prior"]["objectIdentity"];
            let prior = exact && current["name"] == transaction["prior"]["name"];
            let applied = exact && current["name"] == transaction["payload"]["name"];
            if (!reconciliation && json!(js_json::stringify(&fence(&current,&transaction["payload"]["kind"]))) != transaction["fence"]) || (reconciliation && !prior && !applied) {
                return Ok(transaction_error(id, "Rename target identity or name conflicts with the retained transaction"));
            }
            {
                let mut record = record.borrow_mut();
                record["applyKey"] = params["idempotencyKey"].clone();
                record["state"] = json!("applying");
            }
            self.async_adapter().invoke_async(&LiveInvocation::new(&operation(kind), json!({"ref":reference,"name":transaction["payload"]["name"],"expectedName":transaction["prior"]["name"],"expectedObjectIdentity":transaction["prior"]["objectIdentity"],"expectedAuthorityRevision":transaction["payload"]["expectedAuthorityRevision"]})), Some(&context)).await?;
            let verified = self.rename_target_async(Some(&context), kind, reference).await?.unwrap_or(Value::Null);
            if verified.is_null()
                || verified["objectIdentity"] != transaction["prior"]["objectIdentity"]
                || verified["name"] != transaction["payload"]["name"]
            {
                return Err(LiveError::error("rename postcondition was not confirmed for the exact target"));
            }
            record.borrow_mut()["state"] = json!("applied");
            let mut out = json!({"transactionId":transaction["id"],"state":"applied","ref":reference,"name":verified["name"]});
            if reconciliation {
                out["reconciled"] = json!(true);
            }
            out["idempotent"] = json!(false);
            Ok(success_text(id, &out))
        }
        .await;
        result.unwrap_or_else(|cause: LiveError| {
            let mut record = record.borrow_mut();
            if record["state"] == "applying" {
                let cancelled = !reconciliation && cause.message().contains("cancelled before dispatch");
                record["state"] = json!(if cancelled { "previewed" } else { "uncertain" });
                if cancelled {
                    record.as_object_mut().unwrap().remove("applyKey");
                }
            }
            adapter_tool_error(id, &cause, "Rename state may be uncertain; reconcile with the exact original key after fresh discovery.")
        })
    }
    pub async fn undo_rename_async(&self, id: &Value, params: &Value, signal: Option<&Signal>) -> Value {
        let Some(record) = params["transactionId"].as_str().and_then(|id| self.clip_lifecycle_transactions.get(id)) else {
            return transaction_error(id, "Unknown or expired rename transaction");
        };
        let transaction = record.borrow().clone();
        if transaction["kind"] != "rename" || !transaction["clipRef"].as_str().is_some_and(|s| !s.is_empty()) {
            return transaction_error(id, "Unknown or expired rename transaction");
        }
        if transaction["state"] == "undone" && transaction["undoKey"] == params["idempotencyKey"] {
            return success_text(id, &json!({"transactionId":transaction["id"],"state":"undone","idempotent":true}));
        }
        let reconciliation = transaction["state"] == "uncertain" && transaction["undoKey"] == params["idempotencyKey"];
        if transaction["state"] != "applied" && !reconciliation {
            return transaction_error(id, "Only an applied or exact-key uncertain rename transaction can be undone");
        }
        let result = async {
            self.begin_undo_recovery(&record, params["idempotencyKey"].as_str().unwrap())?;
            let status = self.require_connected(Some("session.read"))?;
            if json!(status.epoch) != transaction["epoch"] {
                return Ok(transaction_error(id, "Live connection epoch changed; undo refused"));
            }
            let adapter = self.async_adapter();
            let context = self.transaction_context(params, signal, AUDITION_DEADLINE_MS);
            record.borrow_mut()["undoKey"] = params["idempotencyKey"].clone();
            if reconciliation {
                self.replay_undo_recovery(&record, &*adapter, &context).await?;
            }
            let kind = transaction["payload"]["kind"].as_str().unwrap();
            let reference = transaction["clipRef"].as_str().unwrap();
            let current = self.rename_target_async(Some(&context), kind, reference).await?.unwrap_or(Value::Null);
            if current.is_null()
                || current["objectIdentity"] != transaction["prior"]["objectIdentity"]
                || current["name"]
                    != if reconciliation { transaction["prior"]["name"].clone() } else { transaction["payload"]["name"].clone() }
            {
                return Ok(transaction_error(
                    id,
                    if reconciliation {
                        "Rename undo replay did not restore prior name"
                    } else {
                        "Renamed object identity or name changed after apply; undo refused"
                    },
                ));
            }
            if !reconciliation {
                record.borrow_mut()["state"] = json!("undoing");
                let revision = self.rename_revision_async(Some(&context), kind, reference, &current).await?;
                self.invoke_undo_recovery(&record, &*adapter, &operation(kind), &json!({"ref":reference,"name":transaction["prior"]["name"],"expectedName":transaction["payload"]["name"],"expectedObjectIdentity":transaction["prior"]["objectIdentity"],"expectedAuthorityRevision":revision}), &context).await?;
            }
            let restored = self.rename_target_async(Some(&context), kind, reference).await?.unwrap_or(Value::Null);
            if restored.is_null()
                || restored["objectIdentity"] != transaction["prior"]["objectIdentity"]
                || restored["name"] != transaction["prior"]["name"]
            {
                return Err(LiveError::error("rename undo was not confirmed for the exact target"));
            }
            record.borrow_mut()["state"] = json!("undone");
            Ok(success_text(id, &json!({"transactionId":transaction["id"],"state":"undone","ref":reference,"name":restored["name"],"idempotent":false})))
        }
        .await;
        result.unwrap_or_else(|cause| {
            if record.borrow()["state"] == "undoing" {
                record.borrow_mut()["state"] = json!("uncertain");
            }
            adapter_tool_error(id, &cause, "Rename undo is uncertain; rediscover the target.")
        })
    }
}
