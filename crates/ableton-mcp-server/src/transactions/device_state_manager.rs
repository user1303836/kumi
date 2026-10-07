use super::super::batch::{
    compensation_context, forget_undispatched, invoke_checkpoint, not_dispatched, parameter_authority, parameter_holds, parameter_revision,
    parameter_target, same_parameter_value, MutationCheckpoint,
};
use super::*;
use crate::live::{AsyncLiveAdapter, LiveInvocation, LiveOperationContext, LiveSnapshotPart, LiveStatus, LiveViewScope, LiveViews};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::RngCore;
use std::{cell::RefCell, collections::HashMap, rc::Rc};
fn now() -> f64 {
    kumi_common::time::now_ms() as f64
}
#[derive(Clone)]
struct Record(Rc<RefCell<Value>>);
impl Record {
    fn get(&self, key: &str) -> Value {
        self.0.borrow()[key].clone()
    }
    fn put(&self, key: &str, value: impl Into<Value>) {
        self.0.borrow_mut()[key] = value.into();
    }
    fn is(&self, key: &str, value: &str) -> bool {
        self.0.borrow()[key] == value
    }
    fn step(&self, undo: bool, index: usize) -> Value {
        self.0.borrow()[if undo { "undoSteps" } else { "steps" }][index].clone()
    }
    fn set_step(&self, undo: bool, index: usize, key: &str, value: impl Into<Value>) {
        self.0.borrow_mut()[if undo { "undoSteps" } else { "steps" }][index][key] = value.into();
    }
}
pub struct DeviceStateTransactionManager {
    adapter: Rc<dyn AsyncLiveAdapter>,
    views: Rc<LiveViews>,
    records: RefCell<Vec<(String, Record)>>,
    idempotency: RefCell<HashMap<String, (String, Value)>>,
}
impl DeviceStateTransactionManager {
    pub fn new(adapter: Rc<dyn AsyncLiveAdapter>, views: Option<Rc<LiveViews>>) -> Self {
        let view_adapter = adapter.clone();
        Self {
            adapter,
            views: views.unwrap_or_else(|| Rc::new(LiveViews::new(move || view_adapter.clone()))),
            records: RefCell::new(Vec::new()),
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
        records.retain(|(_, r)| number(&r.get("expiresAt")) > now || protected(r));
        self.idempotency.borrow_mut().retain(|_, (id, _)| records.iter().any(|(key, _)| key == id));
        while records.len() >= 512 {
            let index = records
                .iter()
                .position(|(_, r)| !protected(r))
                .or_else(|| records.iter().position(|(_, r)| r.is("state", "applied")))
                .ok_or_else(|| fail("device state transaction capacity is exhausted by recovery-protected work"))?;
            records.remove(index);
        }
        records.push((string(&record["transactionId"]).into(), Record(Rc::new(RefCell::new(record)))));
        Ok(())
    }
    fn require(&self) -> Result<LiveStatus, LiveError> {
        let status = self.adapter.status()?;
        if !status.connected || status.epoch.is_none() {
            return Err(fail("live-capability-unavailable:connection"));
        }
        for capability in ["devices", "parameters", "device.parameter.write"] {
            if !status.capabilities.iter().any(|c| c.as_str() == capability) {
                return Err(fail(format!("live-capability-unavailable:{capability}")));
            }
        }
        for operation in ["snapshot", "device.parameter.set"] {
            if !status.has_operation(operation) {
                return Err(fail(format!("live-operation-unavailable:{operation}")));
            }
        }
        Ok(status)
    }
    async fn view_for(&self, context: Option<&LiveOperationContext>, refs: &[Value]) -> Result<Value, LiveError> {
        Ok(serde_json::to_value(self.views.view_for(context, refs, None, &[]).await?)?)
    }
    pub async fn preview_async(&self, plan: &Value, mode: &str, amount: Option<f64>) -> Result<Value, LiveError> {
        let status = self.require()?;
        let refs = std::iter::once(plan["deviceRef"].clone())
            .chain(array(&plan["dispositions"]).iter().map(|row| row["parameterRef"].clone()))
            .collect::<Vec<_>>();
        let snapshot = self.view_for(None, &refs).await?;
        let mut steps = Vec::new();
        for row in array(&plan["dispositions"]).iter().filter(|row| row["disposition"] == "applicable") {
            let target = parameter_target(&snapshot, string(&row["deviceRef"]), string(&row["parameterRef"]))?;
            let authority = parameter_authority(&snapshot, string(&row["parameterRef"]))?;
            if !number(&target.parameter["value"]).is_finite() {
                return Err(fail(format!("device state parameter {} has no authoritative numeric value", string(&row["path"]))));
            }
            steps.push(json!({"parameterRef":row["parameterRef"],"deviceRef":row["deviceRef"],"path":row["path"],"priorValue":target.parameter["value"],"priorRevision":parameter_revision(target.parameter),"authorityDigest":fingerprint(&authority)?,"proposedValue":row["proposedValue"],"completed":false}));
        }
        let mut random = [0u8; 18];
        rand::rng().fill_bytes(&mut random);
        let id = format!("devstate_{}", URL_SAFE_NO_PAD.encode(random));
        let expires = now() + DEVICE_STATE_TRANSACTION_TTL_MS;
        let mut record = json!({"transactionId":id,"epoch":status.epoch,"expiresAt":expires,"state":"previewed","deviceRef":plan["deviceRef"],"mode":mode,"steps":steps});
        if let Some(amount) = amount {
            record["amount"] = json!(amount);
        }
        self.retain(record)?;
        Ok(json!({"transactionId":id,"epoch":status.epoch,"expiresAt":expires}))
    }
    fn step_args(&self, snapshot: &Value, step: &Value, value: &Value, expected_revision: f64) -> Result<Value, LiveError> {
        let authority = parameter_authority(snapshot, string(&step["parameterRef"]))?;
        Ok(
            json!({"ref":step["parameterRef"],"value":value,"expectedRevision":expected_revision,"expectedObjectIdentity":authority["parameterIdentity"],"expectedOwnerRef":authority["ownerRef"],"expectedOwnerIdentity":authority["ownerIdentity"],"expectedTrackRef":authority["trackRef"],"expectedTrackIdentity":authority["trackIdentity"],"expectedSiblings":authority["siblings"]}),
        )
    }
    async fn checkpoint(
        &self,
        record: &Record,
        undo: bool,
        index: usize,
        context: Option<&LiveOperationContext>,
    ) -> Result<Value, LiveError> {
        let step = record.step(undo, index);
        let mut checkpoint: MutationCheckpoint = serde_json::from_value(step)?;
        let result = invoke_checkpoint(&*self.adapter, &mut checkpoint, context).await;
        for (key, value) in serde_json::to_value(checkpoint)?.as_object().unwrap() {
            record.set_step(undo, index, key, value.clone());
        }
        result
    }
    async fn revert(&self, context: Option<&LiveOperationContext>, record: &Record, mode: &str) -> Result<usize, LiveError> {
        let steps = record.get("steps");
        if record.get("undoSteps").is_null() {
            record.put("undoSteps", json!(array(&steps).iter().map(|_| json!({"completed":false})).collect::<Vec<_>>()));
        }
        let mut reverted = 0;
        for index in (0..array(&steps).len()).rev() {
            let step = record.step(false, index);
            if mode == "rollback" && step["completed"] != true {
                continue;
            }
            let checkpoint = record.step(true, index);
            if checkpoint["completed"] == true {
                continue;
            }
            let refs = [step["deviceRef"].clone(), step["parameterRef"].clone()];
            if checkpoint.get("invocation").is_none() {
                let snapshot = self.view_for(context, &refs).await?;
                let target = parameter_target(&snapshot, string(&step["deviceRef"]), string(&step["parameterRef"]))?;
                let authority = parameter_authority(&snapshot, string(&step["parameterRef"]))?;
                if !parameter_holds(target.parameter, &step["proposedValue"]) || json!(fingerprint(&authority)?) != step["authorityDigest"]
                {
                    return Err(fail(format!(
                        "device state {mode} step {index} ({}) parameter value or identity changed after apply",
                        string(&step["path"])
                    )));
                }
                record.set_step(
                    true,
                    index,
                    "invocation",
                    json!(LiveInvocation::new(
                        "device.parameter.set",
                        self.step_args(&snapshot, &step, &step["priorValue"], parameter_revision(target.parameter))?
                    )),
                );
            }
            self.checkpoint(record, true, index, context).await?;
            let verified = self.view_for(context, &refs).await?;
            let target = parameter_target(&verified, string(&step["deviceRef"]), string(&step["parameterRef"]))?;
            if !same_parameter_value(&target.parameter["value"], &step["priorValue"])
                || json!(fingerprint(&parameter_authority(&verified, string(&step["parameterRef"]))?)?) != step["authorityDigest"]
            {
                return Err(fail(format!(
                    "device state {mode} step {index} ({}) prior-value restoration identity or value was not confirmed",
                    string(&step["path"])
                )));
            }
            record.set_step(true, index, "completed", true);
            reverted += 1;
        }
        Ok(reverted)
    }
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
        if let Some((existing, result)) = self.idempotency.borrow().get(key) {
            if existing != id {
                return Err(fail("idempotency key conflicts with another transaction"));
            }
            let mut result = result.clone();
            result["idempotent"] = true.into();
            return Ok(result);
        }
        let record = self
            .record(id)
            .filter(|record| !record.is("state", "previewed") || number(&record.get("expiresAt")) > now())
            .ok_or_else(|| fail("device state preview expired; preview again"))?;
        let started = now();
        let bound = bound_context(id, key, context);
        let span = bound.deadline_ms.unwrap_or(started) - started;
        let context = Some(&bound);
        let reconciliation = record.is("state", "uncertain") && !record.is("recoveryMode", "undo") && record.is("applyKey", key);
        if record.is("state", "uncertain") && !reconciliation {
            return Err(fail("device state is uncertain; reconcile with the exact original idempotency key"));
        }
        if !record.is("state", "previewed") && !reconciliation {
            return Err(fail("device state transaction is no longer applicable"));
        }
        if reconciliation {
            self.views.view(context, LiveViewScope::Indices(vec![]), Some(&[LiveSnapshotPart::Set])).await?;
        }
        let status = self.require()?;
        if json!(status.epoch) != record.get("epoch") {
            return Err(fail("Live connection epoch changed; preview again"));
        }
        if reconciliation && record.is("recoveryMode", "compensate") {
            return match self.revert(context, &record, "rollback").await {
                Ok(_) => Ok(self.compensated(&record, key)),
                Err(error) => {
                    record.put("state", "uncertain");
                    Err(error)
                }
            };
        }
        record.put("state", "applying");
        record.put("recoveryMode", "apply");
        record.put("applyKey", key);
        let mut results = array(&record.get("steps"))
            .iter()
            .enumerate()
            .map(|(index, step)| {
                if step["completed"] == true && step["result"].is_object() {
                    step["result"].clone()
                } else {
                    json!({"index":index,"path":step["path"],"replayed":step["completed"]})
                }
            })
            .collect::<Vec<_>>();
        // The step whose invocation this call recorded (a replayed one's was recorded by an earlier call).
        let mut fresh = None;
        let operation=async{for(index,result)in results.iter_mut().enumerate(){let step=record.step(false,index);if step["completed"]==true{continue;}fresh=None;let replayed=step.get("invocation").is_some();let refs=[step["deviceRef"].clone(),step["parameterRef"].clone()];if !replayed{let snapshot=self.view_for(context,&refs).await?;let target=parameter_target(&snapshot,string(&step["deviceRef"]),string(&step["parameterRef"]))?;let authority=parameter_authority(&snapshot,string(&step["parameterRef"]))?;if !same_parameter_value(&target.parameter["value"],&step["priorValue"])||parameter_revision(target.parameter)!=number(&step["priorRevision"])||json!(fingerprint(&authority)?)!=step["authorityDigest"]{return Err(fail(format!("device state step {index} ({}) parameter identity, value, or revision changed since preview",string(&step["path"]))));}record.set_step(false,index,"invocation",json!(LiveInvocation::new("device.parameter.set",self.step_args(&snapshot,&step,&step["proposedValue"],number(&step["priorRevision"]))?)));fresh=Some(index);}
 self.checkpoint(&record,false,index,context).await?;let snapshot=self.view_for(context,&refs).await?;let verified=parameter_target(&snapshot,string(&step["deviceRef"]),string(&step["parameterRef"]))?;if !parameter_holds(verified.parameter,&step["proposedValue"])||parameter_revision(verified.parameter)<=number(&step["priorRevision"])||json!(fingerprint(&parameter_authority(&snapshot,string(&step["parameterRef"]))?)?)!=step["authorityDigest"]{return Err(fail(format!("device state step {index} ({}) identity or postcondition was not confirmed",string(&step["path"]))));}
 // The whole number Live kept is the change made: undo checks for it.
 if !same_parameter_value(&verified.parameter["value"],&step["proposedValue"]){record.set_step(false,index,"proposedValue",verified.parameter["value"].clone());}
 let mut value=json!({"index":index,"path":step["path"],"value":verified.parameter["value"],"revision":parameter_revision(verified.parameter)});if replayed{value["replayed"]=true.into();}record.set_step(false,index,"completed",true);record.set_step(false,index,"result",value.clone());*result=value;}Ok::<(),LiveError>(())}.await;
        if let Err(cause) = operation {
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
                return Err(cause);
            }
            let index = array(&record.get("steps")).iter().position(|step| step["completed"] != true).map(|i| i as i64).unwrap_or(-1);
            record.put("failedIndex", index);
            record.put(
                "failureReason",
                if kumi_common::js::string::utf16_len(message) > 160 {
                    format!("{}...", kumi_common::js::string::head(message, 157))
                } else {
                    message.into()
                },
            );
            record.put("recoveryMode", "compensate");
            let rollback = compensation_context(&bound, span);
            return match self.revert(Some(&rollback), &record, "rollback").await {
                Ok(_) => Ok(self.compensated(&record, key)),
                Err(error) => {
                    record.put("state", "uncertain");
                    Err(fail(format!("device state recall failed at step {index} and exact rollback failed; reconcile with the exact original idempotency key ({})",error.message())))
                }
            };
        }
        record.put("state", "applied");
        let mut result = json!({"transactionId":id,"state":"applied","mode":record.get("mode"),"deviceRef":record.get("deviceRef"),"applied":results,"epoch":record.get("epoch"),"idempotent":false});
        if !record.get("amount").is_null() {
            result["amount"] = record.get("amount");
        }
        self.idempotency.borrow_mut().insert(key.into(), (id.into(), result.clone()));
        Ok(result)
    }
    fn compensated(&self, record: &Record, key: &str) -> Value {
        record.put("state", "undone");
        let mut result = json!({"transactionId":record.get("transactionId"),"state":"compensated","rolledBack":array(&record.get("undoSteps")).iter().filter(|step|step["completed"]==true).count(),"idempotent":false});
        if !record.get("failedIndex").is_null() {
            result["failedIndex"] = record.get("failedIndex");
        }
        if !record.get("failureReason").is_null() {
            result["reason"] = record.get("failureReason");
        }
        self.idempotency.borrow_mut().insert(key.into(), (string(&record.get("transactionId")).into(), result.clone()));
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
        if record.as_ref().is_some_and(|r| r.is("state", "undone") && r.is("undoKey", key)) {
            return Ok(json!({"transactionId":id,"state":"undone","idempotent":true}));
        }
        let record = record.ok_or_else(|| fail("Only an applied or exact-key uncertain device-state transaction can be undone"))?;
        let context = bound_context(id, key, context);
        let context = Some(&context);
        let reconciliation = record.is("state", "uncertain") && record.is("recoveryMode", "undo") && record.is("undoKey", key);
        if !reconciliation && !record.is("state", "applied") {
            return Err(fail("Only an applied or exact-key uncertain device-state transaction can be undone"));
        }
        if reconciliation {
            self.views.view(context, LiveViewScope::Indices(vec![]), Some(&[LiveSnapshotPart::Set])).await?;
        }
        let status = self.require()?;
        if json!(status.epoch) != record.get("epoch") {
            return Err(fail("Live connection epoch changed; undo refused"));
        }
        record.put("state", "undoing");
        record.put("recoveryMode", "undo");
        record.put("undoKey", key);
        match self.revert(context, &record, "undo").await {
            Ok(reverted) => {
                record.put("state", "undone");
                Ok(json!({"transactionId":id,"state":"undone","restored":reverted,"idempotent":false}))
            }
            Err(error) => {
                record.put("state", "uncertain");
                Err(error)
            }
        }
    }
    /// The record itself, without waiting on a borrow: the host marks work a panic cut short through it.
    pub fn try_record(&self, id: &str) -> Option<Rc<RefCell<Value>>> {
        self.records.try_borrow().ok()?.iter().find(|(key, _)| key == id).map(|(_, record)| record.0.clone())
    }
    pub fn is_finalizable(&self, id: &str) -> bool {
        self.record(id).is_some_and(|r| ["uncertain", "applied", "undone"].iter().any(|state| r.is("state", state)))
    }
    pub fn finalize(&self, id: &str) -> Result<Value, LiveError> {
        if !self.is_finalizable(id) {
            return Err(fail("device state recovery record is not finalizable"));
        }
        let prior = self.record(id).unwrap().get("state");
        self.records.borrow_mut().retain(|(key, _)| key != id);
        self.idempotency.borrow_mut().retain(|_, (key, _)| key != id);
        Ok(json!({"transactionId":id,"finalized":true,"priorState":prior}))
    }
}
fn bound_context(id: &str, key: &str, context: Option<&LiveOperationContext>) -> LiveOperationContext {
    let mut context = context.cloned().unwrap_or_default();
    if context.deadline_ms.is_none() {
        context.deadline_ms = Some(now() + 5000.);
    }
    context.transaction_id = Some(id.into());
    context.idempotency_key = Some(key.into());
    context
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::live::DeterministicLiveSimulator;
    #[test]
    fn recovery_capacity_retires_applied_undo_but_never_uncertain_work() {
        let manager = DeviceStateTransactionManager::new(Rc::new(DeterministicLiveSimulator::new()), None);
        for index in 0..512 {
            let id = format!("state-{index}");
            manager
                .records
                .borrow_mut()
                .push((id.clone(), Record(Rc::new(RefCell::new(json!({"transactionId":id,"expiresAt":0,"state":"applied"}))))));
        }
        manager.retain(json!({"transactionId":"next","expiresAt":now()+1000.,"state":"previewed"})).unwrap();
        assert_eq!(manager.records.borrow().len(), 512);
        assert!(manager.record("state-0").is_none());
        assert!(manager.record("state-1").is_some());
        for (_, record) in manager.records.borrow().iter() {
            record.put("state", "uncertain");
        }
        assert_eq!(
            manager.retain(json!({"transactionId":"overflow","expiresAt":now()+1000.,"state":"previewed"})).unwrap_err().message(),
            "device state transaction capacity is exhausted by recovery-protected work"
        );
    }
    #[test]
    fn expired_terminal_records_and_finalization_remove_only_owned_replay_authority() {
        let manager = DeviceStateTransactionManager::new(Rc::new(DeterministicLiveSimulator::new()), None);
        for (id, state) in [("done", "undone"), ("uncertain", "uncertain"), ("applied", "applied")] {
            manager
                .records
                .borrow_mut()
                .push((id.into(), Record(Rc::new(RefCell::new(json!({"transactionId":id,"expiresAt":0,"state":state}))))));
            manager.idempotency.borrow_mut().insert(id.into(), (id.into(), json!({"state":state})));
        }
        manager.retain(json!({"transactionId":"new","expiresAt":now()+1000.,"state":"previewed"})).unwrap();
        assert!(manager.record("done").is_none());
        assert!(!manager.idempotency.borrow().contains_key("done"));
        assert!(manager.is_finalizable("uncertain"));
        assert_eq!(manager.finalize("uncertain").unwrap()["priorState"], "uncertain");
        assert!(!manager.idempotency.borrow().contains_key("uncertain"));
        assert!(manager.is_finalizable("applied"));
        assert!(!manager.is_finalizable("new"));
        assert!(manager.finalize("new").is_err());
        assert!(manager.idempotency.borrow().contains_key("applied"));
    }
}
