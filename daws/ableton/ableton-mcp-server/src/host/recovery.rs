//! Undo replay plans retain original arguments and distinguish refusals before mutation.
use super::*;
use crate::bridge::remote_adapter::READ_ONLY_INVOKES;
use retention::TransactionRecord;
use std::rc::Weak;

pub(super) struct RecoveryPlan {
    record: Weak<RefCell<Value>>,
    key: String,
    prior_state: Option<Value>,
    retried: bool,
    steps: Vec<Rc<RefCell<Value>>>,
}
pub(super) struct UndoRefusal {
    pub record: TransactionRecord,
    pub message: String,
}
struct WatchedAdapter {
    adapter: Rc<dyn AsyncLiveAdapter>,
    watches: Vec<Rc<Cell<usize>>>,
}
impl WatchedAdapter {
    fn begin(&self, operation: &str) -> bool {
        let changes = !READ_ONLY_INVOKES.contains(&operation);
        if changes {
            for watch in &self.watches {
                watch.set(watch.get() + 1);
            }
        }
        changes
    }
    fn finish<T>(&self, changes: bool, result: Result<T, LiveError>) -> Result<T, LiveError> {
        if changes && result.as_ref().is_err_and(|error| matches!(error, LiveError::MutationNotDispatched(_)) || nothing_changed(error)) {
            for watch in &self.watches {
                watch.set(watch.get() - 1);
            }
        }
        result
    }
}
impl LiveAdapter for WatchedAdapter {
    fn status(&self) -> Result<LiveStatus, LiveError> {
        self.adapter.status()
    }
    fn snapshot(&self) -> Result<LiveSnapshot, LiveError> {
        self.adapter.snapshot()
    }
    fn get(&self, r: &LiveRef) -> Result<Option<Value>, LiveError> {
        self.adapter.get(r)
    }
    fn invoke(&self, i: &LiveInvocation) -> Result<Value, LiveError> {
        let changes = self.begin(&i.operation);
        self.finish(changes, self.adapter.invoke(i))
    }
    fn subscribe(&self, l: LiveListener) -> Result<Unsubscribe, LiveError> {
        self.adapter.subscribe(l)
    }
    fn reconnect(&self) -> Result<LiveStatus, LiveError> {
        self.adapter.reconnect()
    }
}
#[async_trait::async_trait(?Send)]
impl AsyncLiveAdapter for WatchedAdapter {
    async fn snapshot_async(&self, c: Option<&LiveOperationContext>, r: Option<&LiveSnapshotRequest>) -> Result<LiveSnapshot, LiveError> {
        self.adapter.snapshot_async(c, r).await
    }
    async fn discover_async(&self, r: &LiveDiscoveryRequest, c: Option<&LiveOperationContext>) -> Result<LiveDiscoveryResult, LiveError> {
        self.adapter.discover_async(r, c).await
    }
    async fn get_async(&self, r: &LiveRef, c: Option<&LiveOperationContext>) -> Result<Option<Value>, LiveError> {
        self.adapter.get_async(r, c).await
    }
    async fn invoke_async(&self, i: &LiveInvocation, c: Option<&LiveOperationContext>) -> Result<Value, LiveError> {
        let changes = self.begin(&i.operation);
        let result = self.adapter.invoke_async(i, c).await;
        self.finish(changes, result)
    }
    async fn reconnect_async(&self, c: Option<&LiveOperationContext>) -> Result<LiveStatus, LiveError> {
        self.adapter.reconnect_async(c).await
    }
    async fn close(&self) -> Result<(), LiveError> {
        self.adapter.close().await
    }
    fn has_refresh_status_async(&self) -> bool {
        self.adapter.has_refresh_status_async()
    }
    async fn refresh_status_async(&self, c: Option<&LiveOperationContext>) -> Result<LiveStatus, LiveError> {
        self.adapter.refresh_status_async(c).await
    }
    fn has_subscribe_status(&self) -> bool {
        self.adapter.has_subscribe_status()
    }
    fn subscribe_status(&self, l: StatusListener) -> Unsubscribe {
        self.adapter.subscribe_status(l)
    }
    fn has_retire_transaction_async(&self) -> bool {
        self.adapter.has_retire_transaction_async()
    }
    async fn retire_transaction_async(&self, id: &str, c: Option<&LiveOperationContext>, terminal: bool) -> Result<Value, LiveError> {
        self.adapter.retire_transaction_async(id, c, terminal).await
    }
    fn retires_on_its_own(&self) -> bool {
        self.adapter.retires_on_its_own()
    }
    fn has_expect_state_digest(&self) -> bool {
        self.adapter.has_expect_state_digest()
    }
    fn expect_state_digest(&self, id: &str, i: &LiveInvocation) {
        self.adapter.expect_state_digest(id, i)
    }
}
impl McpHost {
    pub(super) fn moved_target(
        what: &str,
        reference: &Value,
        found: Option<&Value>,
        made: Option<&Value>,
    ) -> Result<Option<String>, LiveError> {
        if made.is_some_and(|v| !v.is_null())
            && kumi_common::js::json::stringify(found.unwrap_or(&Value::Null)) == kumi_common::js::json::stringify(made.unwrap())
        {
            return Ok(None);
        }
        Ok(Some(format!("the {what} at {} isn't the one this change was made on any more (something was added, removed or moved since), so undoing there would change another {what}",
helpers::js_string(reference)?)))
    }
    pub(super) fn undo_target_moved(
        &self,
        id: &Value,
        record: &Value,
        what: &str,
        reference: &Value,
        found: Option<&Value>,
        made: Option<&Value>,
    ) -> Result<Option<Value>, LiveError> {
        let Some(moved) = Self::moved_target(what, reference, found, made)? else { return Ok(None) };
        Ok(Some(if record["state"] == "applied" {
            reason_error(id,
&format!("Undo stopped before it changed anything in Live: {moved}"),
&format!("Nothing changed in Live, and the change is still in place. Find the {what} again with live_discover, and change it back by hand if it still needs to."))
        } else {
            reason_error(
                id,
                &moved,
                &format!("Find the {what} again with live_discover and look at it: an earlier try of this undo may have changed it."),
            )
        }))
    }

    /// Apply the source host's shared undo refusal and no-dispatch state reconciliation.
    pub async fn with_undo_watch<F>(&self, id: &Value, params: &Value, execute: F) -> Result<Value, LiveError>
    where
        F: std::future::Future<Output = Result<Value, LiveError>>,
    {
        let transaction_id = params["transactionId"].as_str();
        if let Some(id) = transaction_id {
            self.undo_refusals.borrow_mut().remove(id);
        }
        let undoing = transaction_id.and_then(|id| self.transaction_record(id));
        let before = undoing.as_ref().and_then(|r| r.borrow().get("state").cloned());
        let watch = Rc::new(Cell::new(0));
        self.undo_watches.borrow_mut().push(watch.clone());
        struct WatchGuard<'a> {
            watches: &'a RefCell<Vec<Rc<Cell<usize>>>>,
            watch: Rc<Cell<usize>>,
        }
        impl Drop for WatchGuard<'_> {
            fn drop(&mut self) {
                self.watches.borrow_mut().retain(|w| !Rc::ptr_eq(w, &self.watch));
            }
        }
        let guard = WatchGuard { watches: &self.undo_watches, watch: watch.clone() };
        let result = execute.await;
        drop(guard);
        let result = result?;
        let refusal = transaction_id.and_then(|id| self.undo_refusals.borrow_mut().remove(id));
        if let Some(refusal) = refusal {
            if refusal.record.borrow()["state"] == "uncertain" {
                refusal.record.borrow_mut()["state"] = json!("applied");
                self.delete_undo_plan(&refusal.record);
                return Ok(reason_error(id, &format!("Undo refused before anything changed in Live: {}", adapter_reason(&refusal.message)), "Nothing changed in Live, and the change is still in place. Later changes may have moved or replaced what it made, so its undo can no longer be proven; change it by hand if needed."));
            }
        }
        let failed = (result["result"]["isError"] == true).then(|| result["result"]["content"][0]["text"].as_str()).flatten();
        let Some(record) = undoing else { return Ok(result) };
        if before.as_ref() != Some(&json!("applied")) || record.borrow()["state"] == "undone" || watch.get() > 0 || failed.is_none() {
            return Ok(result);
        }
        let refused = record.borrow()["state"] == "applied";
        {
            let mut record = record.borrow_mut();
            record["state"] = json!("applied");
            record.as_object_mut().unwrap().remove("undoKey");
        }
        self.delete_undo_plan(&record);
        if refused {
            return Ok(result);
        }
        let failed = failed.unwrap();
        let parsed = serde_json::from_str::<Value>(failed).ok();
        let reason = parsed.as_ref().and_then(|v| v["reason"].as_str()).unwrap_or(failed);
        Ok(reason_error(
            id,
            &format!("Undo stopped before it changed anything in Live: {reason}"),
            "Nothing changed in Live, and the change is still in place. A later undo checks it again from the start.",
        ))
    }
    pub(super) fn push_undo_recovery_step(&self, record: &TransactionRecord, value: Value) -> Result<Rc<RefCell<Value>>, LiveError> {
        let mut plans = self.undo_recovery_plans.borrow_mut();
        let plan = plans
            .iter_mut()
            .find(|p| p.record.upgrade().is_some_and(|r| Rc::ptr_eq(&r, record)))
            .ok_or_else(|| LiveError::error("undo recovery plan is unavailable"))?;
        let step = Rc::new(RefCell::new(value));
        plan.steps.push(step.clone());
        Ok(step)
    }
    pub(super) fn delete_undo_plan(&self, record: &TransactionRecord) {
        self.undo_recovery_plans.borrow_mut().retain(|p| p.record.upgrade().is_some_and(|r| !Rc::ptr_eq(&r, record)));
    }
    pub(super) fn async_adapter(&self) -> Rc<dyn AsyncLiveAdapter> {
        let watches = self.undo_watches.borrow();
        if watches.is_empty() {
            self.adapter.clone()
        } else {
            Rc::new(WatchedAdapter { adapter: self.adapter.clone(), watches: watches.clone() })
        }
    }
    pub(super) fn begin_undo_recovery(&self, record: &TransactionRecord, key: &str) -> Result<(bool, Vec<Rc<RefCell<Value>>>), LiveError> {
        let reconciliation = record.borrow()["state"] == "uncertain";
        let mut plans = self.undo_recovery_plans.borrow_mut();
        plans.retain(|plan| plan.record.strong_count() > 0);
        let found = plans.iter().position(|plan| plan.record.upgrade().is_some_and(|r| Rc::ptr_eq(&r, record)));
        let index = if let Some(index) = found {
            if plans[index].key != key {
                return Err(LiveError::error("uncertain undo requires the exact original idempotency key"));
            }
            index
        } else {
            plans.push(RecoveryPlan {
                record: Rc::downgrade(record),
                key: key.into(),
                prior_state: record.borrow().get("state").cloned(),
                retried: false,
                steps: vec![],
            });
            plans.len() - 1
        };
        if reconciliation {
            plans[index].retried = true;
        }
        Ok((reconciliation, plans[index].steps.clone()))
    }
    pub(super) fn note_undo_refusal(&self, record: &TransactionRecord, cause: &LiveError, context: &LiveOperationContext) {
        if !matches!(cause, LiveError::MutationNotDispatched(_)) && !nothing_changed(cause) {
            return;
        }
        let plans = self.undo_recovery_plans.borrow();
        let Some(plan) = plans.iter().find(|plan| plan.record.upgrade().is_some_and(|r| Rc::ptr_eq(&r, record))) else { return };
        if plan.prior_state.as_ref() == Some(&json!("applied"))
            && !plan.retried
            && plan.steps.iter().all(|step| step.borrow()["completed"] != true)
        {
            if let Some(id) = &context.transaction_id {
                self.undo_refusals.borrow_mut().insert(id.clone(), UndoRefusal { record: record.clone(), message: cause.message().into() });
            }
        }
    }
    pub(super) async fn replay_undo_recovery(
        &self,
        record: &TransactionRecord,
        adapter: &dyn AsyncLiveAdapter,
        context: &LiveOperationContext,
    ) -> Result<(), LiveError> {
        let steps = self
            .undo_recovery_plans
            .borrow()
            .iter()
            .find(|plan| plan.record.upgrade().is_some_and(|r| Rc::ptr_eq(&r, record)))
            .map(|plan| plan.steps.clone())
            .unwrap_or_default();
        for step in steps {
            let row = step.borrow().clone();
            if row["completed"] != true {
                let result = adapter
                    .invoke_async(&LiveInvocation::new(row["operation"].as_str().unwrap(), row["args"].clone()), Some(context))
                    .await?;
                let mut row = step.borrow_mut();
                row["result"] = result;
                row["completed"] = json!(true);
            }
        }
        Ok(())
    }
    pub(super) async fn invoke_undo_recovery(
        &self,
        record: &TransactionRecord,
        adapter: &dyn AsyncLiveAdapter,
        operation: &str,
        args: &Value,
        context: &LiveOperationContext,
    ) -> Result<Value, LiveError> {
        let step = {
            let mut plans = self.undo_recovery_plans.borrow_mut();
            let plan = plans
                .iter_mut()
                .find(|plan| plan.record.upgrade().is_some_and(|r| Rc::ptr_eq(&r, record)))
                .ok_or_else(|| LiveError::error("undo recovery plan was not initialized"))?;
            let mut found = None;
            for candidate in &plan.steps {
                let row = candidate.borrow();
                if row["operation"] == operation && canonical_mutation_identity(&row["args"])? == canonical_mutation_identity(args)? {
                    found = Some(candidate.clone());
                    break;
                }
            }
            found.unwrap_or_else(|| {
                let step = Rc::new(RefCell::new(json!({"operation":operation,"args":args,"completed":false})));
                plan.steps.push(step.clone());
                step
            })
        };
        if step.borrow()["completed"] != true {
            let row = step.borrow().clone();
            let result =
                adapter.invoke_async(&LiveInvocation::new(row["operation"].as_str().unwrap(), row["args"].clone()), Some(context)).await;
            let result = match result {
                Ok(value) => value,
                Err(cause) => {
                    self.note_undo_refusal(record, &cause, context);
                    return Err(cause);
                }
            };
            let mut row = step.borrow_mut();
            row["result"] = result;
            row["completed"] = json!(true);
        }
        let result = step.borrow()["result"].clone();
        Ok(result)
    }
    pub(super) fn retain_bounded_transaction(
        &self,
        map: &BoundedTransactionMap,
        transaction: Value,
        kind: &str,
    ) -> Result<TransactionRecord, LiveError> {
        let now = kumi_common::time::now_ms_f64();
        for (key, candidate) in map.entries() {
            let candidate = candidate.borrow();
            if candidate["expiresAt"].as_f64().is_some_and(|expires| expires <= now)
                && !retention::RECOVERY_PROTECTED_STATES.contains(&candidate["state"].as_str().unwrap_or(""))
                && !retention::is_in_flight(&key)
            {
                map.delete(&key);
            }
        }
        let id = transaction["id"].as_str().ok_or_else(|| LiveError::error("transaction id is unavailable"))?.to_owned();
        map.insert(&id, transaction).map_err(|cause| {
            if cause.message().contains("capacity is exhausted") {
                LiveError::error(format!("{kind} transaction capacity is exhausted by in-flight work"))
            } else {
                cause
            }
        })
    }
    pub fn live_transaction_release(&self, id: &Value, params: &Value) -> Value {
        if !has_only(params, &["transactionIds"])
            || !params["transactionIds"]
                .as_array()
                .is_some_and(|items| (1..=64).contains(&items.len()) && items.iter().all(|v| is_non_empty_string(v, 128)))
        {
            return error(id, -32602, "transactionIds (1 to 64) are required", None);
        }
        let mut released = 0;
        let mut kept = vec![];
        for transaction_id in params["transactionIds"].as_array().unwrap() {
            let transaction_id = transaction_id.as_str().unwrap();
            if transaction_id.starts_with("batch_") {
                if !retention::is_in_flight(transaction_id) && self.batch_transactions.release(transaction_id) {
                    released += 1;
                } else {
                    kept.push(transaction_id);
                }
                continue;
            }
            let mut found = false;
            for map in [
                &self.audio_capture_transactions,
                &self.transactions,
                &self.arrangement_transactions,
                &self.session_structure_transactions,
                &self.device_parameter_transactions,
                &self.device_parameters_transactions,
                &self.audition_transactions,
                &self.transport_transactions,
                &self.clip_launch_transactions,
                &self.note_edit_transactions,
                &self.clip_lifecycle_transactions,
            ] {
                if let Some(record) = map.get(transaction_id) {
                    found = true;
                    if record.borrow()["state"] == "applied" && !retention::is_in_flight(transaction_id) {
                        map.delete(transaction_id);
                        released += 1;
                    } else {
                        kept.push(transaction_id);
                    }
                    break;
                }
            }
            if !found
                && (self.in_flight_mutations.borrow().contains_key(transaction_id)
                    || self.has_semantic_export(transaction_id)
                    || self.undo_refusals.borrow().contains_key(transaction_id)
                    || self.song_history_calls.borrow().iter().any(|(key, _)| key == transaction_id))
            {
                kept.push(transaction_id);
            }
        }
        let mut result = json!({"released":released});
        if !kept.is_empty() {
            result["kept"] = json!(kept);
        }
        success_text(id, &result)
    }
}

fn finalization_error(id: &Value, reason: &str) -> Value {
    reason_error(id,reason,"Reconcile or manually recover the exact transaction, prove all audible work stopped, then submit the explicit finalization evidence.")
}
impl McpHost {
    pub(super) async fn capture_mapper_status(&self, adapter: &dyn AsyncLiveAdapter) -> Result<Value, LiveError> {
        adapter
            .invoke_async(
                &LiveInvocation::new("audio.capture.status", json!({})),
                Some(&LiveOperationContext::with_deadline(self.deadline(reads::AUDITION_DEADLINE_MS))),
            )
            .await
    }
    async fn retire_recovery_authority(&self, transaction_id: &str) -> bool {
        if !self.adapter.has_retire_transaction_async() {
            return true;
        }
        self.adapter
            .retire_transaction_async(
                transaction_id,
                Some(&LiveOperationContext::with_deadline(kumi_common::time::now_ms_f64() + 5000.0)),
                true,
            )
            .await
            .is_ok()
    }
    pub async fn live_recovery_finalize_async(&self, id: &Value, params: &Value) -> Result<Value, LiveError> {
        if !has_only(params, &["transactionId", "resolution", "confirmation", "evidence"])
            || !is_non_empty_string(&params["transactionId"], 128)
            || !["manually-restored", "accepted-current-state"].contains(&js_string(&params["resolution"])?.as_str())
            || params["confirmation"] != "finalize-recovery-record"
            || !has_only(&params["evidence"], &["provenance", "observedAt", "scope"])
            || !is_non_empty_string(&params["evidence"]["provenance"], 512)
            || !is_non_empty_string(&params["evidence"]["scope"], 256)
            || params["evidence"].get("observedAt").is_some_and(|v| !is_non_empty_string(v, 64))
        {
            return Ok(finalization_error(id, "Invalid recovery-finalization arguments."));
        }
        let transaction_id = params["transactionId"].as_str().unwrap();
        if self.recovery_finalization_in_flight.get() {
            return Ok(finalization_error(id, "Another recovery finalization safety barrier is in progress."));
        }
        if self.active_async_operations.get() > 0 || retention::any_in_flight() {
            return Ok(finalization_error(
                id,
                "Another asynchronous operation, mutation, or reconciliation is in flight; global safety finalization refused.",
            ));
        }
        self.recovery_finalization_in_flight.set(true);
        retention::mark_in_flight(transaction_id);
        struct Barrier<'a> {
            flag: &'a Cell<bool>,
            id: &'a str,
        }
        impl Drop for Barrier<'_> {
            fn drop(&mut self) {
                retention::clear_in_flight(self.id);
                self.flag.set(false);
            }
        }
        let _barrier = Barrier { flag: &self.recovery_finalization_in_flight, id: transaction_id };
        let maps = [
            &self.transactions,
            &self.arrangement_transactions,
            &self.session_structure_transactions,
            &self.device_parameter_transactions,
            &self.device_parameters_transactions,
            &self.audition_transactions,
            &self.transport_transactions,
            &self.clip_launch_transactions,
            &self.note_edit_transactions,
            &self.clip_lifecycle_transactions,
            &self.audio_capture_transactions,
        ];
        let owner = maps.iter().copied().find(|map| map.get(transaction_id).is_some());
        let midi_finalizable = owner.is_none() && self.midi_transactions.is_finalizable(transaction_id);
        let batch_finalizable = owner.is_none() && !midi_finalizable && self.batch_transactions.is_finalizable(transaction_id);
        let device_finalizable =
            owner.is_none() && !midi_finalizable && !batch_finalizable && self.device_state_transactions.is_finalizable(transaction_id);
        if owner.is_none() && !midi_finalizable && !batch_finalizable && !device_finalizable {
            return Ok(finalization_error(id, "Recovery transaction was not found or is not finalizable."));
        }
        let adapter = self.async_adapter();
        let status = self.fresh_status(Some(&LiveOperationContext::with_deadline(self.deadline(reads::AUDITION_DEADLINE_MS)))).await?;
        let playback = self.views.playback(Some(&LiveOperationContext::with_deadline(self.deadline(reads::AUDITION_DEADLINE_MS)))).await?;
        let safety = serde_json::to_value(playback).unwrap();
        let transport = &safety["transport"];
        if !transport.is_object()
            || transport["playing"] != false
            || transport["arrangementRecord"] != false
            || transport["sessionRecord"] != false
            || safety["playingTargets"].as_array().is_some_and(|v| !v.is_empty())
            || safety["firedTargets"].as_array().is_some_and(|v| !v.is_empty())
        {
            return Ok(finalization_error(
                id,
                "Recovery finalization requires authoritative stopped playback and recording with no active Session targets.",
            ));
        }
        if status.has_operation("realtime.stats") {
            let realtime = adapter
                .invoke_async(
                    &LiveInvocation::new("realtime.stats", json!({})),
                    Some(&LiveOperationContext::with_deadline(self.deadline(reads::AUDITION_DEADLINE_MS))),
                )
                .await?;
            if realtime.is_null() {
                return Err(LiveError::type_error("Cannot read properties of null (reading 'armed')"));
            }
            if realtime["armed"] != false || realtime["pending"].as_f64() != Some(0.0) {
                return Ok(finalization_error(
                    id,
                    "Recovery finalization requires realtime authority to be disarmed with no pending writes.",
                ));
            }
        }
        if midi_finalizable || batch_finalizable || device_finalizable {
            if !self.retire_recovery_authority(transaction_id).await {
                return Ok(finalization_error(id, "Remote replay authority could not be retired; finalization refused."));
            }
            let mut finalized = if midi_finalizable {
                self.midi_transactions.finalize(transaction_id)?
            } else if batch_finalizable {
                self.batch_transactions.finalize(transaction_id)?
            } else {
                self.device_state_transactions.finalize(transaction_id)?
            };
            finalized["resolution"] = params["resolution"].clone();
            finalized["evidence"] = params["evidence"].clone();
            finalized["liveMutated"] = json!(false);
            finalized["recoveryAuthorityRetired"] = json!(true);
            return Ok(success_text(id, &finalized));
        }
        let owner = owner.unwrap();
        let record = owner.get(transaction_id).unwrap();
        let held = record.borrow().clone();
        let audio_owner = std::ptr::eq(owner, &self.audio_capture_transactions);
        if if audio_owner {
            held["state"] != "uncertain"
        } else {
            !matches!(held["state"].as_str(), Some("uncertain" | "applied" | "undone"))
                || retention::ACTIVE_TRANSACTION_STATES.contains(&held["state"].as_str().unwrap_or(""))
        } {
            return Ok(finalization_error(id, "Active or unresolved transaction work cannot be finalized."));
        }
        if audio_owner {
            let observed = self.capture_mapper_status(&*adapter).await?;
            if observed.is_null() {
                return Err(LiveError::type_error("Cannot read properties of null (reading 'captureId')"));
            }
            if observed.get("captureId") != held.get("captureId")
                || observed.get("sourceSlotRef") != held.get("sourceSlotRef")
                || observed.get("destinationSlotRef") != held.get("destinationSlotRef")
                || observed["state"] != "cleaned"
                || observed["active"] != false
                || observed["playbackStopped"] != true
                || observed["clip"].is_object()
                || observed["residual"].as_array().is_some_and(|v| !v.is_empty())
            {
                return Ok(finalization_error(
                    id,
                    "Audio-capture finalization requires exact mapper-cleaned identity and no residual clip.",
                ));
            }
        }
        if held["kind"] == "realtime-arm" && held["state"] != "undone" && json!(status.epoch) == held["epoch"] {
            return Ok(finalization_error(
                id,
                "Realtime recovery must be reconciled or disarmed while the original Live epoch remains active.",
            ));
        }
        if !self.retire_recovery_authority(transaction_id).await {
            return Ok(finalization_error(id, "Remote replay authority could not be retired; finalization refused."));
        }
        owner.delete(transaction_id);
        Ok(success_text(
            id,
            &json!({"transactionId":transaction_id,"finalized":true,"priorState":held["state"],"resolution":params["resolution"],"evidence":params["evidence"],"liveMutated":false,"recoveryAuthorityRetired":true}),
        ))
    }
}

#[cfg(test)]
#[path = "recovery_tests.rs"]
mod tests;
