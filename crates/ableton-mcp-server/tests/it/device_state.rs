use ableton_mcp_server::{
    live::*,
    transactions::{
        batch::{canonical, fingerprint},
        device_state::*,
    },
};
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
};
fn oracle() -> Value {
    serde_json::from_str(include_str!("fixtures/device-state-oracle.json")).unwrap()
}
fn same(left: &Value, right: &Value, label: &str) {
    assert_eq!(canonical(left).unwrap(), canonical(right).unwrap(), "{label}");
}
#[test]
fn subtree_files_validation_and_refusal_reports_match_source() {
    let fixture = oracle();
    for row in fixture["builds"].as_array().unwrap() {
        let result = build_device_state_file(&row["snapshot"], row["ref"].as_str().unwrap(), "saved");
        if let Some(error) = row.get("error") {
            assert_eq!(result.unwrap_err().message(), error.as_str().unwrap(), "{}", row["name"]);
        } else {
            let mut result = result.unwrap();
            result["savedAt"] = "$time".into();
            same(&result, &row["result"], row["name"].as_str().unwrap());
        }
    }
    for row in fixture["validations"].as_array().unwrap() {
        let result = validate_device_state_file(&row["file"]);
        if let Some(error) = row.get("error") {
            assert_eq!(result.unwrap_err().message(), error.as_str().unwrap(), "{}", row["name"]);
        } else {
            same(&result.unwrap(), &row["result"], row["name"].as_str().unwrap());
        }
    }
    for row in fixture["plans"].as_array().unwrap() {
        let result = plan_device_state_recall(&row["snapshot"], &row["file"], "device:d", &row["options"]);
        if let Some(error) = row.get("error") {
            let result = result.unwrap_err();
            assert_eq!(result.message(), error.as_str().unwrap(), "{}", row["name"]);
            if let Some(report) = row.get("report") {
                same(result.device_state_report.as_ref().unwrap(), report, row["name"].as_str().unwrap());
            } else {
                assert!(result.device_state_report.is_none());
            }
        } else {
            same(&result.unwrap(), &row["result"], row["name"].as_str().unwrap());
        }
    }
}
#[test]
fn a_device_with_two_parameters_of_one_name_is_saved_validated_and_recalled() {
    let parameter = |reference: &str, value: f64| json!({"ref":reference,"objectIdentity":format!("identity-{reference}"),"name":"Gain","value":value,"min":0,"max":1,"quantization":0,"automatable":true,"enabled":true});
    let snapshot = json!({"tracks":[{"ref":"track:1","objectIdentity":"track-1","devices":[{"ref":"device:1","objectIdentity":"device-1","name":"Utility","kind":"audio-effect","className":"StereoGain","parameters":[parameter("p1",0.5),parameter("p2",0.25)]}]}]});
    let file = build_device_state_file(&snapshot, "device:1", "saved").unwrap();
    let paths: Vec<_> = file["parameters"].as_array().unwrap().iter().map(|row| row["path"].clone()).collect();
    assert_eq!(paths, [json!("Utility/Gain"), json!("Utility/Gain#2")]);
    validate_device_state_file(&file).unwrap();
    let plan = plan_device_state_recall(&snapshot, &file, "device:1", &json!({})).unwrap();
    assert_eq!((plan["applicable"].clone(), plan["dispositions"][1]["parameterRef"].clone()), (json!(2), json!("p2")));
}
#[test]
fn deterministic_morph_float64_and_quantization_match_source() {
    let fixture = oracle();
    for row in fixture["morph"].as_array().unwrap() {
        let a = row["args"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect::<Vec<_>>();
        let actual = morph_value(a[0], a[1], a[2], a[3], a[4], a[5]);
        let expected = row["result"].as_f64().unwrap();
        assert_eq!(actual, expected, "{row}");
    }
    assert!(morph_value(f64::NAN, 1., 0.5, 0., 1., 0.).is_nan());
    assert!(morph_value(0., f64::INFINITY, 0., 0., 1., 0.).is_nan());
}
#[derive(Default)]
struct Faults {
    config: Value,
    fail_verification: Cell<bool>,
    executions: Cell<usize>,
    replays: Cell<usize>,
    forged_calls: Cell<usize>,
    ledger: RefCell<HashMap<String, Value>>,
    calls: RefCell<Vec<Value>>,
}
struct Adapter {
    sim: Rc<DeterministicLiveSimulator>,
    faults: Rc<Faults>,
}
impl LiveAdapter for Adapter {
    fn status(&self) -> Result<LiveStatus, LiveError> {
        self.sim.status()
    }
    fn snapshot(&self) -> Result<LiveSnapshot, LiveError> {
        self.sim.snapshot()
    }
    fn get(&self, r: &LiveRef) -> Result<Option<Value>, LiveError> {
        self.sim.get(r)
    }
    fn invoke(&self, i: &LiveInvocation) -> Result<Value, LiveError> {
        self.sim.invoke(i)
    }
    fn subscribe(&self, l: LiveListener) -> Result<Unsubscribe, LiveError> {
        self.sim.subscribe(l)
    }
    fn reconnect(&self) -> Result<LiveStatus, LiveError> {
        self.sim.reconnect()
    }
}
#[async_trait::async_trait(?Send)]
impl AsyncLiveAdapter for Adapter {
    async fn snapshot_async(&self, _: Option<&LiveOperationContext>, _: Option<&LiveSnapshotRequest>) -> Result<LiveSnapshot, LiveError> {
        if self.faults.fail_verification.replace(false) {
            return Err(LiveError::error("snapshot unavailable"));
        }
        self.sim.snapshot()
    }
    async fn discover_async(&self, _: &LiveDiscoveryRequest, _: Option<&LiveOperationContext>) -> Result<LiveDiscoveryResult, LiveError> {
        Err(LiveError::error("unused discovery"))
    }
    async fn get_async(&self, r: &LiveRef, _: Option<&LiveOperationContext>) -> Result<Option<Value>, LiveError> {
        self.sim.get(r)
    }
    async fn invoke_async(&self, i: &LiveInvocation, c: Option<&LiveOperationContext>) -> Result<Value, LiveError> {
        let faults = &self.faults;
        if faults.config["forged"] == true {
            let count = faults.forged_calls.get() + 1;
            faults.forged_calls.set(count);
            if count == 1 {
                let reference = self.sim.state.borrow()["tracks"][0]["devices"][0]["parameters"][0]["ref"].as_str().unwrap().to_owned();
                self.sim.simulate_external_edit(&LiveRef::from(reference.as_str()), "value", json!(0.5))?;
                return Err(LiveError::error("remote adapter request state uncertain after dispatch timeout"));
            }
        }
        let context = c.unwrap();
        assert!(context.transaction_id.is_some() && context.idempotency_key.is_some());
        let value = serde_json::to_value(i).unwrap();
        faults.calls.borrow_mut().push(value.clone());
        let key = kumi_common::js::json::stringify(&json!([context.transaction_id, context.idempotency_key, value]));
        if let Some(result) = faults.ledger.borrow().get(&key) {
            faults.replays.set(faults.replays.get() + 1);
            // The replay authority was retired (or the preview went stale): the replay runs nothing.
            if faults.config["refuseReplay"] == true {
                return Err(LiveError::MutationNotDispatched(
                    "request failed: mutation replay authority has been retired; nothing changed".into(),
                ));
            }
            return Ok(result.clone());
        }
        let result = self.sim.invoke(i)?;
        faults.ledger.borrow_mut().insert(key, result.clone());
        let count = faults.executions.get() + 1;
        faults.executions.set(count);
        if faults.config["verificationAt"] == json!(count) {
            faults.fail_verification.set(true);
        }
        if faults.config["identityAt"] == json!(count) {
            self.sim.state.borrow_mut()["tracks"][0]["devices"][0]["parameters"][0]["objectIdentity"] = "external:replacement".into();
        }
        if faults.config["lostAt"].as_array().is_some_and(|v| v.contains(&json!(count))) {
            return Err(LiveError::error("remote adapter request state uncertain after dispatch timeout"));
        }
        Ok(result)
    }
    async fn reconnect_async(&self, _: Option<&LiveOperationContext>) -> Result<LiveStatus, LiveError> {
        self.sim.reconnect()
    }
    async fn close(&self) -> Result<(), LiveError> {
        Ok(())
    }
}
#[tokio::test(flavor = "current_thread")]
async fn a_replayed_recall_step_refused_on_reconcile_stays_uncertain_for_its_first_dispatch_may_have_changed_live() {
    let sim = Rc::new(DeterministicLiveSimulator::new());
    {
        let mut state = sim.state.borrow_mut();
        let device = &mut state["tracks"][0]["devices"][0];
        let mut second = device["parameters"][0].clone();
        second["ref"] = "parameter:second".into();
        second["objectIdentity"] = "simulator:second".into();
        second["name"] = "Second".into();
        second["value"] = 0.75.into();
        device["parameters"].as_array_mut().unwrap().push(second);
    }
    let file = build_device_state_file(&serde_json::to_value(sim.snapshot().unwrap()).unwrap(), "device:utility-1", "saved").unwrap();
    {
        let mut state = sim.state.borrow_mut();
        state["tracks"][0]["devices"][0]["parameters"][0]["value"] = 0.125.into();
        state["tracks"][0]["devices"][0]["parameters"][1]["value"] = 0.25.into();
    }
    // The second step's change reaches Live and its reply is lost; reconciling replays it, and the replay is refused.
    let faults = Rc::new(Faults { config: json!({"lostAt":[2],"refuseReplay":true}), ..Default::default() });
    let manager = DeviceStateTransactionManager::new(Rc::new(Adapter { sim: sim.clone(), faults }), None);
    let plan =
        plan_device_state_recall(&serde_json::to_value(sim.snapshot().unwrap()).unwrap(), &file, "device:utility-1", &json!({})).unwrap();
    let id = manager.preview_async(&plan, "recall", None).await.unwrap()["transactionId"].as_str().unwrap().to_owned();
    let values = || {
        let state = sim.state.borrow();
        [0, 1].map(|index| state["tracks"][0]["devices"][0]["parameters"][index]["value"].clone())
    };
    let lost = manager.apply_async(&id, &json!("apply"), "recall-apply-key", None).await.unwrap_err();
    assert!(lost.message().contains("uncertain"), "{lost}");
    let applied = values();
    assert_eq!(applied, [json!(0.5), json!(0.75)]);
    // The refusal says the replay ran nothing, not that the first dispatch didn't: nothing is taken back.
    let refused = manager.apply_async(&id, &json!("apply"), "recall-apply-key", None).await.unwrap_err();
    assert!(refused.message().contains("nothing changed"), "{refused}");
    assert_eq!(values(), applied, "no step compensated");
}
#[tokio::test(flavor = "current_thread")]
async fn recall_and_recovery_match_source_results_dispatches_and_state_hashes() {
    let fixture = oracle();
    for scenario in fixture["scenarios"].as_array().unwrap() {
        let sim = Rc::new(DeterministicLiveSimulator::new());
        let device_ref;
        {
            let mut state = sim.state.borrow_mut();
            let device = &mut state["tracks"][0]["devices"][0];
            device_ref = device["ref"].as_str().unwrap().to_owned();
            let mut second = device["parameters"][0].clone();
            second["ref"] = "parameter:second".into();
            second["objectIdentity"] = "simulator:second".into();
            second["name"] = "Second".into();
            second["value"] = 0.7.into();
            device["parameters"].as_array_mut().unwrap().push(second);
        }
        let file = build_device_state_file(&serde_json::to_value(sim.snapshot().unwrap()).unwrap(), &device_ref, "recovery").unwrap();
        {
            let mut state = sim.state.borrow_mut();
            state["tracks"][0]["devices"][0]["parameters"][0]["value"] = 0.1.into();
            state["tracks"][0]["devices"][0]["parameters"][1]["value"] = 0.2.into();
        }
        let faults = Rc::new(Faults { config: scenario["fault"].clone(), ..Default::default() });
        let manager = DeviceStateTransactionManager::new(Rc::new(Adapter { sim: sim.clone(), faults: faults.clone() }), None);
        let mut id = String::new();
        for step in scenario["steps"].as_array().unwrap() {
            let action = &step["action"];
            let label = format!("{}: {action}", scenario["name"]);
            let result: Result<Option<Value>, LiveError> = match action["method"].as_str().unwrap() {
                "preview" => {
                    let snapshot = serde_json::to_value(sim.snapshot().unwrap()).unwrap();
                    let plan =
                        plan_device_state_recall(&snapshot, &file, &device_ref, action.get("options").unwrap_or(&json!({}))).unwrap();
                    manager
                        .preview_async(&plan, action["mode"].as_str().unwrap_or("recall"), action["options"]["amount"].as_f64())
                        .await
                        .map(|result| {
                            id = result["transactionId"].as_str().unwrap().into();
                            Some(result)
                        })
                }
                "apply" => manager
                    .apply_async(
                        &id,
                        action.get("confirmation").unwrap_or(&json!("apply")),
                        action["key"].as_str().unwrap_or("apply"),
                        None,
                    )
                    .await
                    .map(Some),
                "undo" => manager
                    .undo_async(&id, action.get("confirmation").unwrap_or(&json!("undo")), action["key"].as_str().unwrap_or("undo"), None)
                    .await
                    .map(Some),
                "edit" => {
                    let reference = sim.state.borrow()["tracks"][0]["devices"][0]["parameters"][action["index"].as_u64().unwrap() as usize]
                        ["ref"]
                        .as_str()
                        .unwrap()
                        .to_owned();
                    sim.simulate_external_edit(
                        &LiveRef::from(reference.as_str()),
                        action["property"].as_str().unwrap_or("value"),
                        action["value"].clone(),
                    )
                    .map(|_| None)
                }
                "identity" => {
                    sim.state.borrow_mut()["tracks"][0]["devices"][0]["parameters"][action["index"].as_u64().unwrap() as usize]
                        ["objectIdentity"] = action["value"].clone();
                    Ok(None)
                }
                "reconnect" => sim.reconnect().map(|_| None),
                "finalize" => manager.finalize(&id).map(Some),
                other => panic!("{other}"),
            };
            if let Some(error) = step.get("error") {
                assert_eq!(result.unwrap_err().message(), error.as_str().unwrap(), "{label}");
            } else {
                let mut result = result.unwrap_or_else(|error| panic!("{label}: {error}"));
                if let Some(Value::Object(object)) = &mut result {
                    if object.contains_key("transactionId") {
                        object.insert("transactionId".into(), "$transaction".into());
                    }
                    object.shift_remove("expiresAt");
                }
                if let Some(expected) = step.get("result") {
                    same(result.as_ref().unwrap(), expected, &label);
                } else {
                    assert!(result.is_none(), "{label}");
                }
            }
            assert_eq!(fingerprint(&sim.state.borrow()).unwrap(), step["stateHash"].as_str().unwrap(), "state {label}");
        }
        same(&json!(*faults.calls.borrow()), &scenario["calls"], &format!("{} dispatches", scenario["name"]));
        assert_eq!(json!(faults.executions.get()), scenario["executions"], "{}", scenario["name"]);
        assert_eq!(json!(faults.replays.get()), scenario["replays"], "{}", scenario["name"]);
    }
}
