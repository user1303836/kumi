use super::*;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::RngCore;
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    rc::Rc,
};
fn now() -> f64 {
    kumi_common::time::now_ms() as f64
}
#[derive(Clone)]
pub(super) struct Record(Rc<RefCell<Value>>);
impl Record {
    pub(super) fn get(&self, key: &str) -> Value {
        self.0.borrow()[key].clone()
    }
    pub(super) fn put(&self, key: &str, value: impl Into<Value>) {
        self.0.borrow_mut()[key] = value.into();
    }
    pub(super) fn is(&self, key: &str, value: &str) -> bool {
        self.0.borrow()[key] == value
    }
    pub(super) fn step(&self, undo: bool, index: usize) -> Value {
        self.0.borrow()[if undo { "undoSteps" } else { "steps" }][index].clone()
    }
    pub(super) fn set_step(&self, undo: bool, index: usize, key: &str, value: impl Into<Value>) {
        self.0.borrow_mut()[if undo { "undoSteps" } else { "steps" }][index][key] = value.into();
    }
}
pub type AssertBatchPolicy = Rc<dyn Fn(&[String]) -> Result<(), LiveError>>;
pub struct BatchTransactionManager {
    pub(super) adapter: Rc<dyn AsyncLiveAdapter>,
    pub(super) views: Rc<LiveViews>,
    pub(super) assert_policy: AssertBatchPolicy,
    records: RefCell<Vec<(String, Record)>>,
    idempotency: RefCell<HashMap<String, (String, Value)>>,
}
impl BatchTransactionManager {
    pub fn new(adapter: Rc<dyn AsyncLiveAdapter>, assert_policy: Option<AssertBatchPolicy>, views: Option<Rc<LiveViews>>) -> Self {
        let view_adapter = adapter.clone();
        Self {
            adapter,
            views: views.unwrap_or_else(|| Rc::new(LiveViews::new(move || view_adapter.clone()))),
            assert_policy: assert_policy.unwrap_or_else(|| Rc::new(|_| Ok(()))),
            records: RefCell::new(vec![]),
            idempotency: RefCell::new(HashMap::new()),
        }
    }
    fn record(&self, id: &str) -> Option<Record> {
        self.records.borrow().iter().find(|(key, _)| key == id).map(|(_, record)| record.clone())
    }
    fn retain(&self, record: Value) -> Result<(), LiveError> {
        let mut records = self.records.borrow_mut();
        let now = now();
        let protected = |record: &Record| ["applying", "applied", "undoing", "uncertain"].iter().any(|state| record.is("state", state));
        records.retain(|(_, record)| number(&record.get("expiresAt")) > now || protected(record));
        self.idempotency.borrow_mut().retain(|_, (id, _)| records.iter().any(|(key, _)| key == id));
        while records.len() >= 512 {
            let index = records
                .iter()
                .position(|(_, record)| !protected(record))
                .or_else(|| records.iter().position(|(_, record)| record.is("state", "applied")))
                .ok_or_else(|| fail("transaction batch capacity is exhausted by recovery-protected work"))?;
            records.remove(index);
        }
        records.push((string(&record["transactionId"]).into(), Record(Rc::new(RefCell::new(record)))));
        Ok(())
    }
    pub fn release(&self, id: &str) -> bool {
        if !self.record(id).is_some_and(|record| record.is("state", "applied")) {
            return false;
        }
        self.records.borrow_mut().retain(|(key, _)| key != id);
        true
    }
    pub(super) fn policy(&self, record: &Record) -> Result<(), LiveError> {
        (self.assert_policy)(&array(&record.get("operations")).iter().map(|op| string(&op["kind"]).to_owned()).collect::<Vec<_>>())
    }
    async fn operations_view(
        &self,
        context: Option<&LiveOperationContext>,
        operations: &Value,
        also: &[Value],
    ) -> Result<Value, LiveError> {
        let mut refs = vec![];
        for operation in array(operations) {
            match string(&operation["kind"]) {
                "mixer.set" | "track.rename" | "routing.arm" => refs.push(operation["trackRef"].clone()),
                "device.parameter.set" => refs.extend([operation["deviceRef"].clone(), operation["parameterRef"].clone()]),
                "clip.set" => refs.push(operation["clipRef"].clone()),
                _ => {}
            }
        }
        refs.extend_from_slice(also);
        Ok(serde_json::to_value(self.views.view_for(context, &refs, None, &[]).await?)?)
    }
    pub(super) async fn record_view(
        &self,
        context: Option<&LiveOperationContext>,
        record: &Record,
        also: &[Value],
    ) -> Result<Value, LiveError> {
        let mut refs: Vec<_> = array(&record.get("created")).iter().map(|row| row["ref"].clone()).collect();
        refs.extend_from_slice(also);
        self.operations_view(context, &record.get("operations"), &refs).await
    }
    fn require(&self, capabilities: &Value, operations: &Value) -> Result<LiveStatus, LiveError> {
        let status = self.adapter.status()?;
        if !status.connected || status.epoch.is_none() {
            return Err(fail("live-capability-unavailable:connection"));
        }
        for capability in array(capabilities) {
            if !status.capabilities.iter().any(|c| c.as_str() == string(capability)) {
                return Err(fail(format!("live-capability-unavailable:{}", string(capability))));
            }
        }
        for operation in array(operations) {
            if !status.has_operation(string(operation)) {
                return Err(fail(format!("live-operation-unavailable:{}", string(operation))));
            }
        }
        Ok(status)
    }
    pub async fn preview_async(&self, request: &Value) -> Result<Value, LiveError> {
        if !request.is_object()
            || !request["operations"].as_array().is_some_and(|ops| (1..=MAX_BATCH_OPERATIONS).contains(&ops.len()))
            || request.as_object().unwrap().keys().any(|key| key != "operations")
        {
            return Err(fail(format!("transaction batch requires 1-{MAX_BATCH_OPERATIONS} operations")));
        }
        let operations: Vec<_> =
            array(&request["operations"]).iter().enumerate().map(|(i, op)| validate_operation(op, i)).collect::<Result<_, _>>()?;
        (self.assert_policy)(&operations.iter().map(|op| string(&op["kind"]).to_owned()).collect::<Vec<_>>())?;
        let mut targets = HashSet::new();
        for operation in &operations {
            if let Some(key) = batch_target_key(operation) {
                if !targets.insert(key.clone()) {
                    return Err(fail(format!(
                        "transaction batch mutates the same exact target twice ({key}); split it into sequential batches"
                    )));
                }
            }
        }
        let mut names = HashSet::new();
        for op in operations.iter().filter(|op| op["kind"] == "track.create") {
            if !names.insert(string(&op["name"])) {
                return Err(fail("transaction batch creates the same track name twice"));
            }
        }
        let mut capabilities = vec![];
        let mut required_ops = vec![];
        let mut kinds = vec![];
        for op in &operations {
            let kind = string(&op["kind"]);
            let (caps, ops) = operation_requirements(kind).unwrap();
            for value in caps {
                if !capabilities.contains(value) {
                    capabilities.push(*value);
                }
            }
            for value in ops {
                if !required_ops.contains(value) {
                    required_ops.push(*value);
                }
            }
            if !kinds.contains(&kind) {
                kinds.push(kind);
            }
        }
        let capabilities = json!(capabilities);
        let required_ops = json!(required_ops);
        let status = self.require(&capabilities, &required_ops)?;
        let snapshot = self.operations_view(None, &json!(operations), &[]).await?;
        let plans: Vec<_> =
            operations.iter().enumerate().map(|(index, op)| plan_operation(&snapshot, op, index)).collect::<Result<_, _>>()?;
        let mut random = [0; 18];
        rand::rng().fill_bytes(&mut random);
        let id = format!("batch_{}", URL_SAFE_NO_PAD.encode(random));
        let expires = now() + BATCH_TRANSACTION_TTL_MS;
        let mut record = json!({"transactionId":id,"epoch":status.epoch,"expiresAt":expires,"state":"previewed","operations":operations,"plans":plans,"requiredCapabilities":capabilities,"requiredOperations":required_ops,"steps":plans.iter().map(|_|json!({"completed":false})).collect::<Vec<_>>()});
        if operations.iter().any(|operation| operation["kind"] == "track.create") {
            record["structureIdentity"] = structure_identity(&snapshot).into();
        }
        self.retain(record)?;
        Ok(
            json!({"transactionId":id,"epoch":status.epoch,"operations":plans,"summary":{"operationCount":plans.len(),"kinds":kinds,"targets":plans.iter().map(|plan|plan["summary"].clone()).collect::<Vec<_>>()},"impact":"applies-sequential-batch-with-guarded-compensation","confirmation":"apply","expiresAt":expires}),
        )
    }
    pub(super) async fn checkpoint(
        &self,
        record: &Record,
        undo: bool,
        index: usize,
        context: Option<&LiveOperationContext>,
    ) -> Result<Value, LiveError> {
        let step = record.step(undo, index);
        if step["acknowledged"] == true {
            return Ok(step["wireResult"].clone());
        }
        let invocation = step.get("invocation").ok_or_else(|| fail("transaction batch dispatch checkpoint is missing"))?;
        let result = self.adapter.invoke_async(&serde_json::from_value(invocation.clone())?, context).await?;
        record.set_step(undo, index, "wireResult", result.clone());
        record.set_step(undo, index, "acknowledged", true);
        Ok(result)
    }
    pub fn is_finalizable(&self, id: &str) -> bool {
        self.record(id).is_some_and(|record| ["uncertain", "applied", "undone"].iter().any(|state| record.is("state", state)))
    }
    pub fn finalize(&self, id: &str) -> Result<Value, LiveError> {
        if !self.is_finalizable(id) {
            return Err(fail("transaction batch recovery record is not finalizable"));
        }
        let prior = self.record(id).unwrap().get("state");
        self.records.borrow_mut().retain(|(key, _)| key != id);
        self.idempotency.borrow_mut().retain(|_, (key, _)| key != id);
        Ok(json!({"transactionId":id,"finalized":true,"priorState":prior}))
    }
}

fn bound_context(id: &str, key: &str, context: Option<&LiveOperationContext>) -> LiveOperationContext {
    let mut bound = context.cloned().unwrap_or_else(|| LiveOperationContext::with_deadline(now() + 5000.0));
    // Omitting a Rust optional deadline corresponds to an absent JS property.
    if bound.deadline_ms.is_none() {
        bound.deadline_ms = Some(now() + 5000.0);
    }
    bound.transaction_id = Some(id.into());
    bound.idempotency_key = Some(key.into());
    bound
}
impl BatchTransactionManager {
    pub async fn apply_async(
        &self,
        id: &str,
        confirmation: &Value,
        key: &str,
        context: Option<&LiveOperationContext>,
    ) -> Result<Value, LiveError> {
        if confirmation != "apply" {
            return Err(fail("confirmation=apply is required"));
        }
        if let Some((prior, result)) = self.idempotency.borrow().get(key) {
            if prior != id {
                return Err(fail("idempotency key conflicts with another transaction"));
            }
            let mut result = result.clone();
            result["idempotent"] = true.into();
            return Ok(result);
        }
        let record = self
            .record(id)
            .filter(|record| !record.is("state", "previewed") || number(&record.get("expiresAt")) > now())
            .ok_or_else(|| fail("transaction batch preview expired; preview again"))?;
        if record.is("state", "applied") && record.is("applyKey", key) {
            return Ok(json!({"transactionId":id,"state":"applied","idempotent":true}));
        }
        let started = now();
        let bound = bound_context(id, key, context);
        let span = bound.deadline_ms.unwrap_or(started) - started;
        let context = Some(&bound);
        self.policy(&record)?;
        let reconciliation = record.is("state", "uncertain") && !record.is("recoveryMode", "undo") && record.is("applyKey", key);
        if record.is("state", "uncertain") && !reconciliation {
            return Err(fail("transaction batch state is uncertain; reconcile with the exact original idempotency key"));
        }
        if !record.is("state", "previewed") && !reconciliation {
            return Err(fail("transaction batch is no longer applicable"));
        }
        if reconciliation {
            self.views.view(context, LiveViewScope::Indices(vec![]), Some(&[LiveSnapshotPart::Set])).await?;
        }
        let status = self.require(&record.get("requiredCapabilities"), &record.get("requiredOperations"))?;
        if json!(status.epoch) != record.get("epoch") {
            return Err(fail("Live connection epoch changed; preview again"));
        }
        if reconciliation && record.is("recoveryMode", "compensate") {
            return match self.revert(context, &record, "rollback").await {
                Ok(_) => Ok(self.compensated(&record, key)),
                Err(cause) => {
                    record.put("state", "uncertain");
                    Err(cause)
                }
            };
        }
        record.put("state", "applying");
        record.put("recoveryMode", "apply");
        record.put("applyKey", key);
        let operations = record.get("operations");
        let mut applied: Vec<_> = array(&record.get("steps"))
            .iter()
            .enumerate()
            .map(|(index, step)| {
                if step["completed"] == true && step["result"].is_object() {
                    step["result"].clone()
                } else {
                    json!({"index":index,"kind":operations[index]["kind"],"replayed":step["completed"]})
                }
            })
            .collect();
        // The step whose invocation this call recorded (a replayed one's was recorded by an earlier call).
        let mut fresh = None;
        let result = async {
            for (index, item) in applied.iter_mut().enumerate() {
                if record.step(false, index)["completed"] == true {
                    continue;
                }
                fresh = None;
                self.policy(&record)?;
                let replayed = record.step(false, index).get("invocation").is_some();
                if !replayed {
                    let snapshot = self.record_view(context, &record, &[]).await?;
                    record.set_step(false, index, "invocation", self.step_args(&snapshot, &record, index)?);
                    fresh = Some(index);
                }
                self.policy(&record)?;
                let result = self.checkpoint(&record, false, index, context).await?;
                let mut verified = self.verify_step(context, &record, index, &result).await?;
                if replayed {
                    verified["replayed"] = true.into();
                }
                record.set_step(false, index, "completed", true);
                record.set_step(false, index, "result", verified.clone());
                *item = verified;
            }
            Ok::<_, LiveError>(())
        }
        .await;
        if let Err(cause) = result {
            if not_dispatched(&cause) {
                let mut steps = record.get("steps");
                forget_undispatched(&mut steps, fresh);
                record.put("steps", steps);
            }
            let message = cause.message();
            let lower = message.to_ascii_lowercase();
            if array(&record.get("steps")).iter().any(|step| step.get("invocation").is_some() && step["completed"] != true)
                || ["uncertain", "disconnect", "timeout", "cancel"].iter().any(|word| lower.contains(word))
            {
                record.put("state", "uncertain");
                record.put("recoveryMode", "apply");
                return Err(cause);
            }
            let failed =
                array(&record.get("steps")).iter().position(|step| step["completed"] != true).map(|index| index as i64).unwrap_or(-1);
            record.put("failedIndex", failed);
            let failure = if kumi_common::js::string::utf16_len(message) > 160 {
                format!("{}...", String::from_utf16_lossy(&message.encode_utf16().take(157).collect::<Vec<_>>()))
            } else {
                message.into()
            };
            record.put("failureReason", failure);
            record.put("recoveryMode", "compensate");
            let rollback = compensation_context(&bound, span);
            return match self.revert(Some(&rollback), &record, "rollback").await {
                Ok(_) => Ok(self.compensated(&record, key)),
                Err(compensation) => {
                    record.put("state", "uncertain");
                    Err(fail(format!("transaction batch failed at operation {failed} and exact rollback failed; reconcile with the exact original idempotency key ({})",compensation.message())))
                }
            };
        }
        record.put("state", "applied");
        let result = json!({"transactionId":id,"state":"applied","operations":applied,"epoch":record.get("epoch"),"idempotent":false});
        self.idempotency.borrow_mut().insert(key.into(), (id.into(), result.clone()));
        Ok(result)
    }
    fn compensated(&self, record: &Record, key: &str) -> Value {
        record.put("state", "undone");
        let id = record.get("transactionId");
        let mut result = json!({"transactionId":id,"state":"compensated"});
        for (destination, source) in [("failedIndex", "failedIndex"), ("reason", "failureReason")] {
            let value = record.get(source);
            if !value.is_null() {
                result[destination] = value;
            }
        }
        result["rolledBack"] = json!(array(&record.get("undoSteps")).iter().filter(|step| step["completed"] == true).count());
        result["idempotent"] = false.into();
        self.idempotency.borrow_mut().insert(key.into(), (string(&id).into(), result.clone()));
        result
    }
    pub async fn undo_async(
        &self,
        id: &str,
        confirmation: &Value,
        key: &str,
        context: Option<&LiveOperationContext>,
    ) -> Result<Value, LiveError> {
        if confirmation != "undo" {
            return Err(fail("confirmation=undo is required"));
        }
        let record = self.record(id);
        if record.as_ref().is_some_and(|record| record.is("state", "undone") && record.is("undoKey", key)) {
            return Ok(json!({"transactionId":id,"state":"undone","idempotent":true}));
        }
        let record = record.ok_or_else(|| fail("Only an applied or exact-key uncertain batch transaction can be undone"))?;
        let context = bound_context(id, key, context);
        let context = Some(&context);
        self.policy(&record)?;
        let reconciliation = record.is("state", "uncertain") && record.is("recoveryMode", "undo") && record.is("undoKey", key);
        if !reconciliation && !record.is("state", "applied") {
            return Err(fail("Only an applied or exact-key uncertain batch transaction can be undone"));
        }
        if reconciliation {
            self.views.view(context, LiveViewScope::Indices(vec![]), Some(&[LiveSnapshotPart::Set])).await?;
        }
        let status = self.require(&record.get("requiredCapabilities"), &record.get("requiredOperations"))?;
        if json!(status.epoch) != record.get("epoch") {
            return Err(fail("Live connection epoch changed; undo refused"));
        }
        if !array(&record.get("steps")).iter().all(|step| step["completed"] == true) {
            return Err(fail("transaction batch has unapplied steps and cannot be undone as a whole"));
        }
        record.put("state", "undoing");
        record.put("recoveryMode", "undo");
        record.put("undoKey", key);
        match self.revert(context, &record, "undo").await {
            Ok(reverted) => {
                record.put("state", "undone");
                Ok(json!({"transactionId":id,"state":"undone","restored":reverted,"idempotent":false}))
            }
            Err(cause) => {
                record.put("state", "uncertain");
                Err(cause)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn capacity_protects_inflight_work_and_retires_oldest_applied_undo() {
        let manager = BatchTransactionManager::new(Rc::new(DeterministicLiveSimulator::new()), None, None);
        for index in 0..512 {
            let id = format!("batch-{index}");
            manager
                .records
                .borrow_mut()
                .push((id.clone(), Record(Rc::new(RefCell::new(json!({"transactionId":id,"expiresAt":0,"state":"applied"}))))));
        }
        manager.retain(json!({"transactionId":"next","expiresAt":now()+1000.0,"state":"previewed"})).unwrap();
        assert_eq!(manager.records.borrow().len(), 512);
        assert!(manager.record("batch-0").is_none());
        assert!(manager.record("batch-1").is_some());
        for (_, record) in manager.records.borrow().iter() {
            record.put("state", "applying");
        }
        assert!(manager
            .retain(json!({"transactionId":"overflow","expiresAt":now()+1000.0,"state":"previewed"}))
            .unwrap_err()
            .message()
            .contains("capacity is exhausted"));
    }
    #[test]
    fn expired_terminal_records_clean_keys_but_expired_uncertain_records_retain_authority() {
        let manager = BatchTransactionManager::new(Rc::new(DeterministicLiveSimulator::new()), None, None);
        for (id, state) in [("done", "undone"), ("uncertain", "uncertain"), ("applied", "applied")] {
            manager
                .records
                .borrow_mut()
                .push((id.into(), Record(Rc::new(RefCell::new(json!({"transactionId":id,"expiresAt":0,"state":state}))))));
            manager.idempotency.borrow_mut().insert(id.into(), (id.into(), json!({"state":state})));
        }
        manager.retain(json!({"transactionId":"new","expiresAt":now()+1000.0,"state":"previewed"})).unwrap();
        assert!(manager.record("done").is_none());
        assert!(!manager.idempotency.borrow().contains_key("done"));
        assert!(manager.is_finalizable("uncertain"));
        assert_eq!(manager.finalize("uncertain").unwrap()["priorState"], "uncertain");
        assert!(!manager.idempotency.borrow().contains_key("uncertain"));
        assert!(manager.release("applied"));
        assert!(manager.idempotency.borrow().contains_key("applied"), "source retains replay result until the next retention pass");
    }
}
