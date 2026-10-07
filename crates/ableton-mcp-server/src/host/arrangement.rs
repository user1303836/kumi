//! Arrangement locator transactions retain atomic creation evidence and replayable cleanup.
use super::*;
use kumi_common::abort::Signal;
use retention::TransactionRecord;
use sha2::{Digest, Sha256};
pub(super) fn capture_object_fingerprint(value: &Value) -> Result<String, LiveError> {
    Ok(hex::encode(Sha256::digest(canonical_mutation_identity(&without_playback_state(value))?.as_bytes())))
}
pub(super) fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(v) => *v,
        Value::Number(v) => v.as_f64().is_some_and(|n| n != 0.0 && !n.is_nan()),
        Value::String(v) => !v.is_empty(),
        _ => true,
    }
}
fn valid_section(p: &Value) -> bool {
    has_only(p, &["start", "end", "startName", "endName"])
        && p["start"].as_f64().is_some_and(|v| v.is_finite() && (0.0..=100000.0).contains(&v))
        && p["end"].as_f64().is_some_and(|v| v.is_finite() && v > p["start"].as_f64().unwrap_or(f64::INFINITY) && v <= 100000.0)
        && is_non_empty_string(&p["startName"], 128)
        && is_non_empty_string(&p["endName"], 128)
        && p["startName"] != p["endName"]
}
fn locators(snapshot: &LiveSnapshot) -> Result<Vec<Value>, LiveError> {
    Ok(snapshot
        .arrangement
        .as_ref()
        .ok_or_else(|| LiveError::type_error("Cannot read properties of undefined (reading 'locators')"))?
        .locators
        .iter()
        .map(|v| serde_json::to_value(v).unwrap())
        .collect())
}
fn proposed(t: &Value) -> [Value; 2] {
    [json!({"name":t["startName"],"position":t["start"]}), json!({"name":t["endName"],"position":t["end"]})]
}
fn exact_locator(a: &Value, b: &Value) -> bool {
    a["ref"] == b["ref"]
        && a["objectIdentity"] == b["objectIdentity"]
        && a["name"] == b["name"]
        && a["position"].as_f64() == b["position"].as_f64()
}
/// The locator a transaction made, wherever it is now. Live's locator refs are positional
/// (`{epoch}:locator:{index}`): one added or deleted before it moves it to another ref, and the next takes a
/// deleted one's ref, so only its identity finds it.
fn owned_locator<'a>(rows: &'a [Value], created: &Value) -> Option<&'a Value> {
    rows.iter().find(|row| row["objectIdentity"] == created["objectIdentity"])
}
/// Whether an owned locator still has the name and place it was made with, at whichever ref it has now.
fn unchanged_locator(row: &Value, created: &Value) -> bool {
    row["name"] == created["name"] && row["position"].as_f64() == created["position"].as_f64()
}
impl McpHost {
    pub async fn dispatch_arrangement_tool(&self, call: &ToolCall, signal: Option<&Signal>) -> Option<Result<Value, LiveError>> {
        let args = call.arguments.as_ref().unwrap_or(&Value::Null);
        Some(Ok(match (call.name.as_str(), call.asynchronous) {
            ("live_arrangement_section_preview", true) => self.live_arrangement_preview_async(&call.id, args).await,
            ("live_arrangement_section_preview", false) => self.live_arrangement_preview(&call.id, args),
            ("live_arrangement_section_apply", true) => self.live_arrangement_apply_async(&call.id, args, signal).await,
            ("live_arrangement_section_apply", false) => self.live_arrangement_apply(&call.id, args),
            _ => return None,
        }))
    }
    pub(super) fn locator_revision(&self, snapshot: &LiveSnapshot) -> Result<String, LiveError> {
        let revision = snapshot
            .arrangement
            .as_ref()
            .ok_or_else(|| LiveError::type_error("Cannot read properties of undefined (reading 'locatorRevision')"))?
            .locator_revision
            .as_deref()
            .unwrap_or("");
        if revision.len() != 64 || !revision.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)) {
            return Err(LiveError::error("locator collection revision is unavailable"));
        }
        Ok(revision.into())
    }
    pub(super) fn locator_delete_args(
        &self,
        snapshot: &LiveSnapshot,
        reference: &Value,
        expected_identity: Option<&Value>,
    ) -> Result<Value, LiveError> {
        let rows = locators(snapshot)?;
        let row = rows.iter().find(|r| r["ref"] == *reference);
        let identity = expected_identity.filter(|v| !v.is_null()).or_else(|| row.and_then(|r| r.get("objectIdentity")));
        if row.is_none() || !identity.is_some_and(|v| is_non_empty_string(v, 256)) || row.unwrap().get("objectIdentity") != identity {
            return Err(LiveError::error("locator identity changed before deletion"));
        }
        Ok(json!({"ref":reference,"expectedObjectIdentity":identity,"expectedCollectionRevision":self.locator_revision(snapshot)?}))
    }
    async fn arrangement_view(&self, context: Option<&LiveOperationContext>) -> Result<LiveSnapshot, LiveError> {
        self.views.view(context, LiveViewScope::Indices(vec![]), Some(&[LiveSnapshotPart::Arrangement])).await
    }
    fn make_arrangement_preview(
        &self,
        id: &Value,
        params: &Value,
        status: &LiveStatus,
        snapshot: &LiveSnapshot,
    ) -> Result<Value, LiveError> {
        let prior = locators(snapshot)?;
        if prior.iter().any(|v| !is_non_empty_string(&v["objectIdentity"], 256)) {
            return Err(LiveError::error("locator object identity is unavailable"));
        }
        if prior.iter().any(|v| {
            v["name"] == params["startName"]
                || v["name"] == params["endName"]
                || v["position"].as_f64() == params["start"].as_f64()
                || v["position"].as_f64() == params["end"].as_f64()
        }) {
            return Err(LiveError::error("Arrangement locator target collides with existing state"));
        }
        let transaction = json!({"id":tempo::transaction_id("arrangement"),"epoch":status.epoch,"revision":self.locator_revision(snapshot)?,"start":params["start"],"end":params["end"],"startName":params["startName"],"endName":params["endName"],"prior":prior,"expiresAt":kumi_common::time::now_ms_f64()+TRANSACTION_TTL_MS,"state":"previewed"});
        self.arrangement_transactions.insert(transaction["id"].as_str().unwrap(), transaction.clone())?;
        Ok(success_text(
            id,
            &json!({"transactionId":transaction["id"],"epoch":transaction["epoch"],"revision":transaction["revision"],"prior":prior,"proposed":proposed(&transaction),"impact":"creates-arrangement-locators","confirmation":"apply","expiresAt":transaction["expiresAt"]}),
        ))
    }
    pub fn live_arrangement_preview(&self, id: &Value, params: &Value) -> Value {
        if !valid_section(params) {
            return error(id, -32602, "Arrangement section range and distinct names are required", None);
        }
        let result = (|| {
            let status = self.require_connected(Some("arrangement.read"))?;
            self.make_arrangement_preview(id, params, &status, &self.adapter.snapshot()?)
        })();
        result.unwrap_or_else(|e| {
            adapter_tool_error(id, &e, "Arrangement preview failed without mutation; discover locators and choose a collision-free range.")
        })
    }
    pub async fn live_arrangement_preview_async(&self, id: &Value, params: &Value) -> Value {
        if !valid_section(params) {
            return error(id, -32602, "Arrangement section range and distinct names are required", None);
        }
        let result = async {
            let status = self.require_connected(Some("arrangement.read"))?;
            self.make_arrangement_preview(id, params, &status, &self.arrangement_view(None).await?)
        }
        .await;
        result.unwrap_or_else(|e| {
            adapter_tool_error(id, &e, "Arrangement preview failed without mutation; discover locators and choose a collision-free range.")
        })
    }
    fn arrangement_apply_record(&self, id: &Value, params: &Value, asynchronous: bool) -> Result<(TransactionRecord, bool), Value> {
        if !valid_transaction_params(params, "apply") {
            return Err(error(id, -32602, "transactionId, confirmation=apply, and idempotencyKey are required", None));
        }
        let record = self
            .arrangement_transactions
            .get(params["transactionId"].as_str().unwrap())
            .ok_or_else(|| transaction_error(id, "Unknown or expired Arrangement transaction"))?;
        let t = record.borrow();
        if t["state"] == "applied" && t["applyKey"] == params["idempotencyKey"] {
            return Err(success_text(id, &json!({"transactionId":t["id"],"state":"applied","locators":t["created"],"idempotent":true})));
        }
        if t["state"] == "applied" {
            return Err(transaction_error(id, "Arrangement idempotency key conflicts with the applied transaction"));
        }
        let reconciliation = asynchronous && t["state"] == "uncertain" && t["applyKey"] == params["idempotencyKey"];
        if t["state"] == "uncertain" && !reconciliation {
            return Err(transaction_error(
                id,
                if asynchronous {
                    "Arrangement apply is uncertain; reconcile with the exact original idempotency key"
                } else {
                    "Arrangement apply is uncertain; read authoritative locators before retrying"
                },
            ));
        }
        if (t["state"] != "previewed" && !reconciliation)
            || (t["state"] == "previewed" && t["expiresAt"].as_f64().is_some_and(|t| t <= kumi_common::time::now_ms_f64()))
        {
            return Err(transaction_error(id, "Arrangement preview expired or is no longer applicable"));
        }
        drop(t);
        Ok((record, reconciliation))
    }
    fn confirm_arrangement_created(&self, created: &[Value], authoritative: &[Value]) -> Result<bool, LiveError> {
        for locator in created {
            let mut confirmed = false;
            for item in authoritative {
                if exact_locator(item, locator) && capture_object_fingerprint(item)? == locator["fingerprint"] {
                    confirmed = true;
                    break;
                }
            }
            if !confirmed {
                return Ok(false);
            }
        }
        Ok(true)
    }
    pub fn live_arrangement_apply(&self, id: &Value, params: &Value) -> Value {
        let (record, _) = match self.arrangement_apply_record(id, params, false) {
            Ok(v) => v,
            Err(v) => return v,
        };
        let t = record.borrow().clone();
        let result = (|| {
            let status = self.require_connected(Some("arrangement.write"))?;
            if json!(status.epoch) != t["epoch"] {
                return Ok(transaction_error(id, "Live connection epoch changed; preview again"));
            }
            let mut snapshot = self.adapter.snapshot()?;
            if self.locator_revision(&snapshot)? != t["revision"] {
                return Ok(transaction_error(id, "Arrangement locators changed since preview"));
            }
            let mut created = vec![];
            let dispatched = (|| {
                for proposed in proposed(&t) {
                    let mut args = proposed.clone();
                    args["expectedCollectionRevision"] = json!(self.locator_revision(&snapshot)?);
                    let result = self.adapter.invoke(&LiveInvocation::new("locator.add", args))?;
                    if result.is_null() {
                        return Err(LiveError::type_error("Cannot read properties of null (reading 'ref')"));
                    }
                    if !truthy(&result["ref"])
                        || !is_non_empty_string(&result["objectIdentity"], 256)
                        || !is_non_empty_string(&result["createdFingerprint"], 64)
                        || result["name"] != proposed["name"]
                        || result["position"].as_f64() != proposed["position"].as_f64()
                    {
                        return Err(LiveError::error("Live did not return exact atomically owned locator identity"));
                    }
                    created.push(json!({"ref":result["ref"],"objectIdentity":result["objectIdentity"],"name":result["name"],"position":result["position"],"fingerprint":result["createdFingerprint"]}));
                    snapshot = self.adapter.snapshot()?;
                }
                Ok(())
            })();
            if let Err(cause) = dispatched {
                for locator in created.iter().rev() {
                    let compensated = (|| {
                        let snapshot = self.adapter.snapshot()?;
                        self.adapter.invoke(&LiveInvocation::new(
                            "locator.delete",
                            self.locator_delete_args(&snapshot, &locator["ref"], Some(&locator["objectIdentity"]))?,
                        ))
                    })();
                    if compensated.is_err() {
                        let mut record = record.borrow_mut();
                        record["state"] = json!("uncertain");
                        record["created"] = json!(created);
                        return Err(LiveError::error("Arrangement apply compensation failed; read locators before retrying"));
                    }
                }
                return Err(cause);
            }
            if !self.confirm_arrangement_created(&created, &locators(&self.adapter.snapshot()?)?)? {
                let mut record = record.borrow_mut();
                record["state"] = json!("uncertain");
                record["created"] = json!(created);
                return Err(LiveError::error(
                    "Live did not confirm unchanged atomically owned Arrangement locators; read authoritative state before retrying",
                ));
            }
            {
                let mut record = record.borrow_mut();
                record["created"] = json!(created);
                record["applyKey"] = params["idempotencyKey"].clone();
                record["state"] = json!("applied");
            }
            Ok(success_text(
                id,
                &json!({"transactionId":t["id"],"state":"applied","locators":created,"epoch":t["epoch"],"idempotent":false}),
            ))
        })();
        result.unwrap_or_else(|e| adapter_tool_error(id, &e, "Arrangement apply uncertain; read authoritative locators before retrying."))
    }
    async fn compensate_arrangement_async(
        &self,
        record: &TransactionRecord,
        adapter: &dyn AsyncLiveAdapter,
        context: &LiveOperationContext,
    ) -> Result<(), LiveError> {
        let created = record.borrow()["created"].as_array().cloned().unwrap_or_default();
        {
            let mut r = record.borrow_mut();
            if r["compensationSteps"].is_null() {
                r["compensationSteps"] = json!([]);
            }
            r["recoveryMode"] = json!("compensate");
        }
        for (index, locator) in created.iter().rev().enumerate() {
            let mut step = record.borrow()["compensationSteps"].get(index).cloned().unwrap_or(Value::Null);
            if step.is_null() {
                let snapshot = self.arrangement_view(Some(context)).await?;
                let rows = locators(&snapshot)?;
                let Some(row) = owned_locator(&rows, locator) else {
                    // Not there by its identity: gone, or another locator took its ref. Unless the row at its ref is
                    // the one made, under another identity than Live returned: that one can't be taken back safely.
                    let at_ref = rows.iter().find(|r| r["ref"] == locator["ref"]).map(capture_object_fingerprint).transpose()?;
                    if at_ref.is_some_and(|fingerprint| locator["fingerprint"] == fingerprint) {
                        return Err(LiveError::error("transaction-owned locator changed before compensation"));
                    }
                    continue;
                };
                // Its fingerprint covers the ref it was made at: a shift to another ref isn't a change.
                let mut made = row.clone();
                made["ref"] = locator["ref"].clone();
                if !truthy(&locator["fingerprint"]) || capture_object_fingerprint(&made)? != locator["fingerprint"] {
                    return Err(LiveError::error("transaction-owned locator changed before compensation"));
                }
                step = json!({"args":self.locator_delete_args(&snapshot,&row["ref"],Some(&locator["objectIdentity"]))?,"completed":false});
                let mut r = record.borrow_mut();
                let steps = r["compensationSteps"].as_array_mut().unwrap();
                if steps.len() <= index {
                    steps.resize(index + 1, Value::Null);
                }
                steps[index] = step.clone();
            }
            if step["completed"] != true {
                adapter.invoke_async(&LiveInvocation::new("locator.delete", step["args"].clone()), Some(context)).await?;
                record.borrow_mut()["compensationSteps"][index]["completed"] = json!(true);
            }
        }
        let after = locators(&self.arrangement_view(Some(context)).await?)?;
        if created.iter().any(|c| owned_locator(&after, c).is_some()) {
            return Err(LiveError::error("Arrangement compensation left transaction-owned locators"));
        }
        Ok(())
    }
    pub async fn live_arrangement_apply_async(&self, id: &Value, params: &Value, signal: Option<&Signal>) -> Value {
        let (record, reconciliation) = match self.arrangement_apply_record(id, params, true) {
            Ok(v) => v,
            Err(v) => return v,
        };
        let t = record.borrow().clone();
        let result = async {
            if reconciliation {
                self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(reads::AUDITION_DEADLINE_MS)))).await?;
            }
            let status = self.require_connected(Some("arrangement.write"))?;
            if json!(status.epoch) != t["epoch"] {
                return Ok(transaction_error(id, "Live connection epoch changed; preview again"));
            }
            let adapter = self.async_adapter();
            let context = self.transaction_context(params, signal, reads::AUDITION_DEADLINE_MS);
            if reconciliation && t["recoveryMode"] == "compensate" {
                return Ok(match self.compensate_arrangement_async(&record, &*adapter, &context).await {
                    Ok(()) => {
                        record.borrow_mut()["state"] = json!("undone");
                        success_text(id, &json!({"transactionId":t["id"],"state":"compensated","residuals":[],"idempotent":false}))
                    }
                    Err(e) => {
                        record.borrow_mut()["state"] = json!("uncertain");
                        adapter_tool_error(id, &e, "Arrangement compensation remains uncertain; inspect authoritative locators.")
                    }
                });
            }
            let mut snapshot = self.arrangement_view(Some(&context)).await?;
            if !reconciliation && self.locator_revision(&snapshot)? != t["revision"] {
                return Ok(transaction_error(id, "Arrangement locators changed since preview"));
            }
            let mut created = t["created"].as_array().cloned().unwrap_or_default();
            let mut dispatch_ambiguous = false;
            {
                let mut r = record.borrow_mut();
                if r["recoverySteps"].is_null() {
                    r["recoverySteps"] = json!([]);
                }
                r["recoveryMode"] = json!("apply");
                r["state"] = json!("applying");
                r["applyKey"] = params["idempotencyKey"].clone();
            }
            let dispatched = async {
                for (index, proposed) in proposed(&t).iter().enumerate() {
                    let mut step = record.borrow()["recoverySteps"].get(index).cloned().unwrap_or(Value::Null);
                    if step.is_null() {
                        let mut args = proposed.clone();
                        args["expectedCollectionRevision"] = json!(self.locator_revision(&snapshot)?);
                        step = json!({"args":args});
                        let mut r = record.borrow_mut();
                        let steps = r["recoverySteps"].as_array_mut().unwrap();
                        if steps.len() <= index {
                            steps.resize(index + 1, Value::Null);
                        }
                        steps[index] = step.clone();
                    }
                    let mut result = step["result"].clone();
                    if !truthy(&result) {
                        dispatch_ambiguous = true;
                        result = adapter.invoke_async(&LiveInvocation::new("locator.add", step["args"].clone()), Some(&context)).await?;
                        dispatch_ambiguous = false;
                    }
                    if !truthy(&result["ref"])
                        || !is_non_empty_string(&result["objectIdentity"], 256)
                        || !is_non_empty_string(&result["createdFingerprint"], 64)
                    {
                        return Err(LiveError::error("Live did not return atomic created locator ownership evidence"));
                    }
                    record.borrow_mut()["recoverySteps"][index]["result"] = result.clone();
                    if !created.iter().any(|c| c["ref"] == result["ref"]) {
                        let mut owned = result.clone();
                        owned["fingerprint"] = result["createdFingerprint"].clone();
                        created.push(owned);
                    }
                    record.borrow_mut()["created"] = json!(created);
                    snapshot = self.arrangement_view(Some(&context)).await?;
                    let owned = created.iter().find(|c| c["ref"] == result["ref"]).unwrap();
                    let rows = locators(&snapshot)?;
                    let row = rows.iter().find(|r| r["ref"] == owned["ref"]);
                    if row.is_none()
                        || row.unwrap()["objectIdentity"] != owned["objectIdentity"]
                        || capture_object_fingerprint(row.unwrap())? != owned["fingerprint"]
                    {
                        return Err(LiveError::error("created locator changed after atomic creation"));
                    }
                    if result["name"] != proposed["name"] || result["position"].as_f64() != proposed["position"].as_f64() {
                        return Err(LiveError::error("Live did not confirm exact created locator state"));
                    }
                }
                Ok(())
            }
            .await;
            if let Err(cause) = dispatched {
                record.borrow_mut()["created"] = json!(created);
                if dispatch_ambiguous && (reconciliation || !nothing_changed(&cause)) {
                    let mut r = record.borrow_mut();
                    r["recoveryMode"] = json!("apply");
                    r["state"] = json!("uncertain");
                    return Err(cause);
                }
                match self.compensate_arrangement_async(&record, &*adapter, &context).await {
                    Ok(()) => record.borrow_mut()["state"] = json!("undone"),
                    Err(_) => {
                        let mut r = record.borrow_mut();
                        r["state"] = json!("uncertain");
                        r["recoveryMode"] = json!("compensate");
                        return Err(LiveError::error("Arrangement apply compensation failed; retry the exact key to reconcile cleanup"));
                    }
                }
                return Err(cause);
            }
            if !self.confirm_arrangement_created(&created, &locators(&self.arrangement_view(Some(&context)).await?)?)? {
                let mut r = record.borrow_mut();
                r["state"] = json!("uncertain");
                r["created"] = json!(created);
                return Err(LiveError::error(
                    "Live did not confirm unchanged atomically owned Arrangement locators; read authoritative state before retrying",
                ));
            }
            {
                let mut r = record.borrow_mut();
                r["created"] = json!(created);
                r["applyKey"] = params["idempotencyKey"].clone();
                r["state"] = json!("applied");
            }
            Ok(success_text(
                id,
                &json!({"transactionId":t["id"],"state":"applied","locators":created,"epoch":t["epoch"],"idempotent":false}),
            ))
        }
        .await;
        result.unwrap_or_else(|e| {
            let undone = record.borrow()["state"] == "undone";
            apply_failed(
                id,
                &record,
                &e,
                if undone {
                    "Nothing changed in Live."
                } else {
                    "Arrangement apply uncertain; read authoritative locators before retrying."
                },
            )
        })
    }
    pub fn undo_arrangement(&self, id: &Value, params: &Value) -> Value {
        let Some(record) = params["transactionId"].as_str().and_then(|id| self.arrangement_transactions.get(id)) else {
            return transaction_error(id, "Arrangement state is uncertain; read authoritative locators before undo");
        };
        let t = record.borrow().clone();
        if t["state"] == "uncertain" {
            return transaction_error(id, "Arrangement state is uncertain; read authoritative locators before undo");
        }
        if t["state"] != "applied" || !truthy(&t["created"]) {
            return transaction_error(id, "Only an applied Arrangement transaction can be undone");
        }
        if t["undoKey"] == params["idempotencyKey"] {
            return success_text(id, &json!({"transactionId":t["id"],"state":"undone","idempotent":true}));
        }
        let result = (|| {
            let status = self.require_connected(Some("arrangement.write"))?;
            if json!(status.epoch) != t["epoch"] {
                return Ok(transaction_error(id, "Live connection epoch changed; undo refused"));
            }
            let current = locators(&self.adapter.snapshot()?)?;
            let created = t["created"].as_array().unwrap();
            if !created.iter().all(|c| owned_locator(&current, c).is_some_and(|r| unchanged_locator(r, c))) {
                return Ok(transaction_error(id, "Arrangement locator identity or content changed after apply; undo refused"));
            }
            let undone = (|| {
                for c in created.iter().rev() {
                    let snapshot = self.adapter.snapshot()?;
                    let rows = locators(&snapshot)?;
                    let reference = owned_locator(&rows, c).map_or(&c["ref"], |row| &row["ref"]);
                    self.adapter.invoke(&LiveInvocation::new(
                        "locator.delete",
                        self.locator_delete_args(&snapshot, reference, Some(&c["objectIdentity"]))?,
                    ))?;
                }
                Ok(())
            })();
            if let Err(cause) = undone {
                record.borrow_mut()["state"] = json!("uncertain");
                return Err(cause);
            }
            {
                let mut r = record.borrow_mut();
                r["state"] = json!("undone");
                r["undoKey"] = params["idempotencyKey"].clone();
            }
            Ok(success_text(id, &json!({"transactionId":t["id"],"state":"undone","restored":t["prior"],"idempotent":false})))
        })();
        result.unwrap_or_else(|e| adapter_tool_error(id, &e, "Arrangement undo refused; inspect authoritative locators."))
    }
    pub async fn undo_arrangement_async(&self, id: &Value, params: &Value, signal: Option<&Signal>) -> Value {
        let Some(record) = params["transactionId"].as_str().and_then(|id| self.arrangement_transactions.get(id)) else {
            return transaction_error(id, "Only an applied or exact-key uncertain Arrangement transaction can be undone");
        };
        let t = record.borrow().clone();
        let reconciliation = t["state"] == "uncertain" && t["undoKey"] == params["idempotencyKey"];
        if t["state"] == "undone" && t["undoKey"] == params["idempotencyKey"] {
            return success_text(id, &json!({"transactionId":t["id"],"state":"undone","idempotent":true}));
        }
        if (t["state"] != "applied" && !reconciliation) || !truthy(&t["created"]) {
            return transaction_error(id, "Only an applied or exact-key uncertain Arrangement transaction can be undone");
        }
        let result = async {
            let status = self.require_connected(Some("arrangement.write"))?;
            if json!(status.epoch) != t["epoch"] {
                return Ok(transaction_error(id, "Live connection epoch changed; undo refused"));
            }
            let adapter = self.async_adapter();
            let context = self.transaction_context(params, signal, reads::AUDITION_DEADLINE_MS);
            self.begin_undo_recovery(&record, params["idempotencyKey"].as_str().unwrap())?;
            record.borrow_mut()["undoKey"] = params["idempotencyKey"].clone();
            if reconciliation {
                self.replay_undo_recovery(&record, &*adapter, &context).await?;
            }
            let current = locators(&self.arrangement_view(Some(&context)).await?)?;
            let created = t["created"].as_array().unwrap();
            for c in created {
                if owned_locator(&current, c).is_some_and(|r| !unchanged_locator(r, c)) {
                    return Ok(transaction_error(id, "Arrangement locator identity or content changed after apply; undo refused"));
                }
            }
            let undone = async {
                record.borrow_mut()["state"] = json!("undoing");
                for c in created.iter().rev() {
                    let snapshot = self.arrangement_view(Some(&context)).await?;
                    let rows = locators(&snapshot)?;
                    let Some(row) = owned_locator(&rows, c) else { continue };
                    self.invoke_undo_recovery(
                        &record,
                        &*adapter,
                        "locator.delete",
                        &self.locator_delete_args(&snapshot, &row["ref"], Some(&c["objectIdentity"]))?,
                        &context,
                    )
                    .await?;
                }
                let current = locators(&self.arrangement_view(Some(&context)).await?)?;
                if created.iter().any(|c| owned_locator(&current, c).is_some()) {
                    return Err(LiveError::error("Arrangement undo left transaction-owned locators"));
                }
                Ok(())
            }
            .await;
            if let Err(cause) = undone {
                record.borrow_mut()["state"] = json!("uncertain");
                return Err(cause);
            }
            record.borrow_mut()["state"] = json!("undone");
            Ok(success_text(id, &json!({"transactionId":t["id"],"state":"undone","restored":t["prior"],"idempotent":false})))
        }
        .await;
        result.unwrap_or_else(|e| adapter_tool_error(id, &e, "Arrangement undo refused; inspect authoritative locators."))
    }
}
