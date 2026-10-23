//! Tempo transactions preserve Set identity, exact replay keys, and prior-state restoration.
use super::*;
use base64::Engine;
use kumi_common::abort::Signal;
use rand::RngCore;
fn valid_tempo(params: &Value) -> bool {
    has_only(params, &["tempo"]) && params["tempo"].as_f64().is_some_and(|n| n.is_finite() && (20.0..=999.0).contains(&n))
}
fn set_row(snapshot: &LiveSnapshot) -> Result<Value, LiveError> {
    snapshot
        .set
        .as_ref()
        .map(|set| serde_json::to_value(set).unwrap())
        .ok_or_else(|| LiveError::type_error("Cannot read properties of undefined (reading 'tempo')"))
}
pub(super) fn transaction_id(prefix: &str) -> String {
    let mut bytes = [0u8; 18];
    rand::rng().fill_bytes(&mut bytes);
    format!("{prefix}_{}", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
}
impl McpHost {
    pub async fn dispatch_tempo_tool(&self, call: &ToolCall, signal: Option<&Signal>) -> Option<Result<Value, LiveError>> {
        let args = call.arguments.as_ref().unwrap_or(&Value::Null);
        Some(match (call.name.as_str(), call.asynchronous) {
            ("live_tempo_preview", true) => self.live_tempo_preview_async(&call.id, args).await,
            ("live_tempo_preview", false) => Ok(self.live_tempo_preview(&call.id, args)),
            ("live_tempo_apply", true) => Ok(self.live_tempo_apply_async(&call.id, args, signal).await),
            ("live_tempo_apply", false) => Ok(self.live_tempo_apply(&call.id, args)),
            ("live_transaction_release", _) => Ok(self.live_transaction_release(&call.id, args)),
            _ => return None,
        })
    }
    fn make_tempo_preview(&self, id: &Value, params: &Value, status: &LiveStatus, set: &Value) -> Result<Value, LiveError> {
        if !set["tempo"].as_f64().is_some_and(f64::is_finite) || !is_non_empty_string(&set["objectIdentity"], 256) {
            return Err(LiveError::error("authoritative Set tempo identity is unavailable"));
        }
        let transaction_id = transaction_id("tempo");
        let transaction = json!({"id":transaction_id,"setRef":set["ref"],"setIdentity":set["objectIdentity"],"priorTempo":set["tempo"],"proposedTempo":params["tempo"],"epoch":status.epoch,"expiresAt":kumi_common::time::now_ms_f64()+TRANSACTION_TTL_MS,"state":"previewed"});
        self.transactions.insert(&transaction_id, transaction.clone())?;
        let now = kumi_common::time::now_ms_f64();
        for (key, record) in self.transactions.entries() {
            let record = record.borrow();
            if record["state"] == "previewed" && record["expiresAt"].as_f64().is_some_and(|expires| expires <= now) {
                self.transactions.delete(&key);
            }
        }
        Ok(success_text(
            id,
            &json!({"transactionId":transaction_id,"epoch":transaction["epoch"],"target":transaction["setRef"],"priorTempo":transaction["priorTempo"],"proposedTempo":transaction["proposedTempo"],"impact":"audible-transport","confirmation":"apply","expiresAt":transaction["expiresAt"]}),
        ))
    }
    pub fn live_tempo_preview(&self, id: &Value, params: &Value) -> Value {
        if !valid_tempo(params) {
            return error(id, -32602, "tempo must be a finite number from 20 to 999", None);
        }
        let result = (|| {
            let status = self.require_connected(Some("transport"))?;
            let set = set_row(&self.adapter.snapshot()?)?;
            self.make_tempo_preview(id, params, &status, &set)
        })();
        result.unwrap_or_else(|cause| {
            adapter_tool_error(id, &cause, "Tempo preview unavailable. Verify the Live adapter connection and retry.")
        })
    }
    pub async fn live_tempo_preview_async(&self, id: &Value, params: &Value) -> Result<Value, LiveError> {
        if !valid_tempo(params) {
            return Ok(error(id, -32602, "tempo must be a finite number from 20 to 999", None));
        }
        let status = self.require_connected(Some("transport"))?;
        let snapshot = self.views.view(None, LiveViewScope::Indices(vec![]), Some(&[LiveSnapshotPart::Set])).await?;
        let set = set_row(&snapshot)?;
        if !set["tempo"].as_f64().is_some_and(f64::is_finite) || !is_non_empty_string(&set["objectIdentity"], 256) {
            return Ok(adapter_tool_error(
                id,
                &LiveError::error("authoritative Set tempo identity is unavailable"),
                "Tempo preview requires fresh authoritative tempo evidence.",
            ));
        }
        self.make_tempo_preview(id, params, &status, &set)
    }
    pub fn live_tempo_apply(&self, id: &Value, params: &Value) -> Value {
        if !valid_transaction_params(params, "apply") {
            return error(id, -32602, "transactionId, confirmation=apply, and idempotencyKey are required", None);
        }
        let Some(record) = self.transactions.get(params["transactionId"].as_str().unwrap()) else {
            return transaction_error(id, "Unknown or expired transaction");
        };
        let transaction = record.borrow().clone();
        if transaction["state"] == "applied" && transaction["applyKey"] == params["idempotencyKey"] {
            return success_text(
                id,
                &json!({"transactionId":transaction["id"],"state":"applied","tempo":transaction["appliedTempo"],"idempotent":true}),
            );
        }
        if transaction["state"] != "previewed" {
            return transaction_error(id, "Transaction is no longer applicable");
        }
        let result = (|| {
            if transaction["expiresAt"].as_f64().is_some_and(|expires| expires <= kumi_common::time::now_ms_f64()) {
                self.transactions.delete(transaction["id"].as_str().unwrap());
                return Ok(transaction_error(id, "Tempo preview expired; preview again"));
            }
            let status = self.require_connected(Some("transport"))?;
            if json!(status.epoch) != transaction["epoch"] {
                return Ok(transaction_error(id, "Live connection epoch changed; preview again"));
            }
            let reference = LiveRef(transaction["setRef"].as_str().unwrap().into());
            let current = self.adapter.get(&reference)?.unwrap_or(Value::Null);
            if current["objectIdentity"] != transaction["setIdentity"] || current["tempo"].as_f64() != transaction["priorTempo"].as_f64() {
                return Ok(transaction_error(id, "Set identity or tempo changed since preview; preview again"));
            }
            self.adapter.invoke(&LiveInvocation::new("tempo.set",json!({"ref":transaction["setRef"],"value":transaction["proposedTempo"],"expectedTempo":transaction["priorTempo"],"expectedObjectIdentity":transaction["setIdentity"]})))?;
            let applied = self.adapter.get(&reference)?.unwrap_or(Value::Null);
            if applied["objectIdentity"] != transaction["setIdentity"] || applied["tempo"].as_f64() != transaction["proposedTempo"].as_f64()
            {
                return Ok(transaction_error(id, "Live did not confirm the requested exact Set tempo"));
            }
            {
                let mut record = record.borrow_mut();
                record["appliedTempo"] = applied["tempo"].clone();
                record["applyKey"] = params["idempotencyKey"].clone();
                record["state"] = json!("applied");
            }
            Ok(success_text(
                id,
                &json!({"transactionId":transaction["id"],"state":"applied","tempo":applied["tempo"],"epoch":transaction["epoch"],"idempotent":false}),
            ))
        })();
        result.unwrap_or_else(|cause| adapter_tool_error(id, &cause, "Tempo apply failed; inspect Live state before retrying."))
    }
    pub async fn live_tempo_apply_async(&self, id: &Value, params: &Value, signal: Option<&Signal>) -> Value {
        if !valid_transaction_params(params, "apply") {
            return error(id, -32602, "transactionId, confirmation=apply, and idempotencyKey are required", None);
        }
        let Some(record) = self.transactions.get(params["transactionId"].as_str().unwrap()) else {
            return transaction_error(id, "Unknown or expired transaction");
        };
        let transaction = record.borrow().clone();
        if transaction["state"] == "applied" && transaction["applyKey"] == params["idempotencyKey"] {
            return success_text(
                id,
                &json!({"transactionId":transaction["id"],"state":"applied","tempo":transaction["appliedTempo"],"idempotent":true}),
            );
        }
        let reconciliation = transaction["state"] == "uncertain" && transaction["applyKey"] == params["idempotencyKey"];
        if transaction["state"] == "uncertain" && !reconciliation {
            return transaction_error(id, "Tempo state is uncertain; reconcile with the exact original idempotency key");
        }
        if transaction["state"] != "previewed" && !reconciliation {
            return transaction_error(id, "Transaction is no longer applicable");
        }
        if transaction["state"] == "previewed"
            && transaction["expiresAt"].as_f64().is_some_and(|expires| expires <= kumi_common::time::now_ms_f64())
        {
            return transaction_error(id, "Tempo preview expired; preview again");
        }
        let result=async{
            if reconciliation{self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(reads::AUDITION_DEADLINE_MS)))).await?;}
            let status=self.require_connected(Some("transport"))?;if json!(status.epoch)!=transaction["epoch"]{return Ok(transaction_error(id,"Live connection epoch changed; preview again"));}
            let adapter=self.async_adapter();let context=self.transaction_context(params,signal,reads::AUDITION_DEADLINE_MS);let reference=LiveRef(transaction["setRef"].as_str().unwrap().into());
            let current=adapter.get_async(&reference,Some(&context)).await?.unwrap_or(Value::Null);
            if !reconciliation && (current["objectIdentity"]!=transaction["setIdentity"] || current["tempo"].as_f64() != transaction["priorTempo"].as_f64()){return Ok(transaction_error(id,"Set identity or tempo changed since preview; preview again"));}
            {let mut record=record.borrow_mut();record["state"]=json!("applying");record["applyKey"]=params["idempotencyKey"].clone();}
            adapter.invoke_async(&LiveInvocation::new("tempo.set",json!({"ref":transaction["setRef"],"value":transaction["proposedTempo"],"expectedTempo":transaction["priorTempo"],"expectedObjectIdentity":transaction["setIdentity"]})),Some(&context)).await?;
            let applied=adapter.get_async(&reference,Some(&context)).await?.unwrap_or(Value::Null);
            if applied["objectIdentity"]!=transaction["setIdentity"] || applied["tempo"].as_f64() != transaction["proposedTempo"].as_f64(){return Err(LiveError::error("Live did not confirm the requested exact Set tempo"));}
            {let mut record=record.borrow_mut();record["appliedTempo"]=applied["tempo"].clone();record["state"]=json!("applied");}
            Ok(success_text(id,&json!({"transactionId":transaction["id"],"state":"applied","tempo":applied["tempo"],"epoch":transaction["epoch"],"idempotent":false})))
        }.await;
        result.unwrap_or_else(|cause: LiveError| {
            let mut record = record.borrow_mut();
            if record["state"] == "applying" {
                record["state"] = json!(if cause.message().contains("cancelled before dispatch") { "previewed" } else { "uncertain" });
                if record["state"] == "previewed" {
                    record.as_object_mut().unwrap().remove("applyKey");
                }
            }
            adapter_tool_error(id, &cause, "Tempo apply may be uncertain; perform fresh authoritative discovery and do not retry blindly.")
        })
    }
    pub fn undo_tempo(&self, id: &Value, params: &Value) -> Value {
        let Some(record) = params["transactionId"].as_str().and_then(|id| self.transactions.get(id)) else {
            return transaction_error(id, "Unknown or expired transaction");
        };
        let transaction = record.borrow().clone();
        if transaction["state"] == "undone" && transaction["undoKey"] == params["idempotencyKey"] {
            return success_text(
                id,
                &json!({"transactionId":transaction["id"],"state":"undone","tempo":transaction["priorTempo"],"idempotent":true}),
            );
        }
        if transaction["state"] != "applied" {
            return transaction_error(id, "Only an applied tempo transaction can be undone");
        }
        let result = (|| {
            let status = self.require_connected(Some("transport"))?;
            if json!(status.epoch) != transaction["epoch"] {
                return Ok(transaction_error(id, "Live connection epoch changed; undo refused"));
            }
            let reference = LiveRef(transaction["setRef"].as_str().unwrap().into());
            let current = self.adapter.get(&reference)?.unwrap_or(Value::Null);
            if current["objectIdentity"] != transaction["setIdentity"] || current["tempo"].as_f64() != transaction["appliedTempo"].as_f64()
            {
                return Ok(transaction_error(id, "Set identity or tempo changed after apply; undo refused"));
            }
            self.adapter.invoke(&LiveInvocation::new("tempo.set",json!({"ref":transaction["setRef"],"value":transaction["priorTempo"],"expectedTempo":transaction["appliedTempo"],"expectedObjectIdentity":transaction["setIdentity"]})))?;
            let restored = self.adapter.get(&reference)?.unwrap_or(Value::Null);
            if restored["objectIdentity"] != transaction["setIdentity"] || restored["tempo"].as_f64() != transaction["priorTempo"].as_f64()
            {
                return Ok(transaction_error(id, "Live did not confirm exact Set tempo restoration"));
            }
            {
                let mut record = record.borrow_mut();
                record["undoKey"] = params["idempotencyKey"].clone();
                record["state"] = json!("undone");
            }
            Ok(success_text(
                id,
                &json!({"transactionId":transaction["id"],"state":"undone","tempo":restored["tempo"],"epoch":transaction["epoch"],"idempotent":false}),
            ))
        })();
        result.unwrap_or_else(|cause| adapter_tool_error(id, &cause, "Tempo undo failed; inspect Live state before retrying."))
    }
    pub async fn undo_tempo_async(&self, id: &Value, params: &Value, signal: Option<&Signal>) -> Value {
        let Some(record) = params["transactionId"].as_str().and_then(|id| self.transactions.get(id)) else {
            return transaction_error(id, "Unknown or expired transaction");
        };
        let transaction = record.borrow().clone();
        if transaction["state"] == "undone" && transaction["undoKey"] == params["idempotencyKey"] {
            return success_text(
                id,
                &json!({"transactionId":transaction["id"],"state":"undone","tempo":transaction["priorTempo"],"idempotent":true}),
            );
        }
        let reconciliation = transaction["state"] == "uncertain" && transaction["undoKey"] == params["idempotencyKey"];
        if transaction["state"] != "applied" && !reconciliation {
            return transaction_error(id, "Only an applied or exact-key uncertain tempo transaction can be undone");
        }
        let result=async{
            self.begin_undo_recovery(&record,params["idempotencyKey"].as_str().unwrap())?;
            let status=self.require_connected(Some("transport"))?;if json!(status.epoch)!=transaction["epoch"]{return Ok(transaction_error(id,"Live connection epoch changed; undo refused"));}
            let adapter=self.async_adapter();let context=self.transaction_context(params,signal,reads::AUDITION_DEADLINE_MS);record.borrow_mut()["undoKey"]=params["idempotencyKey"].clone();
            if reconciliation{self.replay_undo_recovery(&record,&*adapter,&context).await?;}
            let reference=LiveRef(transaction["setRef"].as_str().unwrap().into());let current=adapter.get_async(&reference,Some(&context)).await?.unwrap_or(Value::Null);
            if current["objectIdentity"]!=transaction["setIdentity"] || (reconciliation && current["tempo"].as_f64() != transaction["priorTempo"].as_f64()){return Ok(transaction_error(id,if reconciliation{"Tempo undo replay did not restore exact prior state"}else{"The Set changed since this tempo change (another Set is open); undo refused"}));}
            if !reconciliation && current["tempo"].as_f64() != transaction["priorTempo"].as_f64(){record.borrow_mut()["state"]=json!("undoing");self.invoke_undo_recovery(&record,&*adapter,"tempo.set",&json!({"ref":transaction["setRef"],"value":transaction["priorTempo"],"expectedTempo":current["tempo"],"expectedObjectIdentity":transaction["setIdentity"]}),&context).await?;}
            let restored=adapter.get_async(&reference,Some(&context)).await?.unwrap_or(Value::Null);if restored["objectIdentity"]!=transaction["setIdentity"] || restored["tempo"].as_f64() != transaction["priorTempo"].as_f64(){return Err(LiveError::error("Live did not confirm exact Set tempo restoration"));}
            record.borrow_mut()["state"]=json!("undone");Ok(success_text(id,&json!({"transactionId":transaction["id"],"state":"undone","tempo":restored["tempo"],"epoch":transaction["epoch"],"idempotent":false})))
        }.await;
        result.unwrap_or_else(|cause| {
            if record.borrow()["state"] == "undoing" {
                record.borrow_mut()["state"] = json!("uncertain");
            }
            adapter_tool_error(id, &cause, "Tempo undo is uncertain; perform fresh authoritative discovery.")
        })
    }
}
