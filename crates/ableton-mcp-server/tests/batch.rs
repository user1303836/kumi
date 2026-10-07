use ableton_mcp_server::{live::*, transactions::batch::*};
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
};
fn patch(state: &mut Value, patches: &Value) {
    for patch in patches.as_array().unwrap() {
        let parts: Vec<_> = patch[0].as_str().unwrap().split('.').collect();
        let mut parent = &mut *state;
        for part in &parts[..parts.len() - 1] {
            parent = if parent.is_array() { &mut parent[part.parse::<usize>().unwrap()] } else { &mut parent[*part] };
        }
        let key = parts.last().unwrap();
        if patch[1] == "$delete" {
            parent.as_object_mut().unwrap().shift_remove(*key);
        } else if parent.is_array() {
            parent[key.parse::<usize>().unwrap()] = patch[1].clone();
        } else {
            parent[*key] = patch[1].clone();
        }
    }
}
fn same(left: &Value, right: &Value, label: &str) {
    assert_eq!(canonical(left).unwrap(), canonical(right).unwrap(), "{label}");
}
#[tokio::test]
async fn batch_preview_validation_and_authority_plans_match_typescript() {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/batch-oracle.json")).unwrap();
    for row in fixture["validation"].as_array().unwrap() {
        let sim = Rc::new(DeterministicLiveSimulator::new());
        if let Some(patches) = row.get("patch") {
            patch(&mut sim.state.borrow_mut(), patches);
        }
        let manager = BatchTransactionManager::new(sim, None, None);
        let result = manager.preview_async(&row["request"]).await;
        if let Some(error) = row.get("error") {
            assert_eq!(result.unwrap_err().message(), error.as_str().unwrap(), "{}", row["name"]);
        } else {
            let mut result = result.unwrap_or_else(|error| panic!("{}: {error}", row["name"]));
            result.as_object_mut().unwrap().shift_remove("transactionId");
            result.as_object_mut().unwrap().shift_remove("expiresAt");
            same(&result, &row["result"], row["name"].as_str().unwrap());
        }
    }
}
#[derive(Default)]
struct Faults {
    deny: Cell<bool>,
    deny_read: Cell<usize>,
    reads: Cell<usize>,
    lost_at: Cell<usize>,
    executions: Cell<usize>,
    replays: Cell<usize>,
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
        self.faults.reads.set(self.faults.reads.get() + 1);
        let result = self.sim.snapshot();
        if self.faults.reads.get() == self.faults.deny_read.get() {
            self.faults.deny.set(true);
        }
        result
    }
    async fn discover_async(&self, _: &LiveDiscoveryRequest, _: Option<&LiveOperationContext>) -> Result<LiveDiscoveryResult, LiveError> {
        Err(LiveError::error("unused discovery"))
    }
    async fn get_async(&self, r: &LiveRef, _: Option<&LiveOperationContext>) -> Result<Option<Value>, LiveError> {
        self.sim.get(r)
    }
    async fn invoke_async(&self, i: &LiveInvocation, c: Option<&LiveOperationContext>) -> Result<Value, LiveError> {
        let value = serde_json::to_value(i).unwrap();
        self.faults.calls.borrow_mut().push(value.clone());
        let key = format!(
            "{}:{}:{}",
            c.and_then(|c| c.transaction_id.as_deref()).unwrap_or("undefined"),
            c.and_then(|c| c.idempotency_key.as_deref()).unwrap_or("undefined"),
            kumi_common::js::json::stringify(&value)
        );
        if let Some(result) = self.faults.ledger.borrow().get(&key) {
            self.faults.replays.set(self.faults.replays.get() + 1);
            return Ok(result.clone());
        }
        let result = self.sim.invoke(i)?;
        self.faults.ledger.borrow_mut().insert(key, result.clone());
        let executions = self.faults.executions.get() + 1;
        self.faults.executions.set(executions);
        if executions == self.faults.lost_at.get() {
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
#[tokio::test]
async fn batch_results_dispatches_recovery_and_state_match_typescript() {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/batch-oracle.json")).unwrap();
    for scenario in fixture["scenarios"].as_array().unwrap() {
        let sim = Rc::new(DeterministicLiveSimulator::new());
        let faults = Rc::new(Faults::default());
        faults.deny_read.set(scenario["fault"]["denyRead"].as_u64().unwrap_or(0) as usize);
        faults.lost_at.set(scenario["fault"]["lostAt"].as_u64().unwrap_or(0) as usize);
        let adapter = Rc::new(Adapter { sim: sim.clone(), faults: faults.clone() });
        let policy_faults = faults.clone();
        let manager = BatchTransactionManager::new(
            adapter,
            Some(Rc::new(move |_| if policy_faults.deny.get() { Err(LiveError::error("policy denied")) } else { Ok(()) })),
            None,
        );
        let mut id = String::new();
        for step in scenario["steps"].as_array().unwrap() {
            let action = &step["action"];
            let key = action["key"].as_str();
            let label = format!("{}: {action}", scenario["name"]);
            let result: Result<Option<Value>, LiveError> = match action["method"].as_str().unwrap() {
                "preview" => manager.preview_async(&json!({"operations":scenario["operations"]})).await.map(|result| {
                    id = result["transactionId"].as_str().unwrap().into();
                    Some(result)
                }),
                "apply" => manager
                    .apply_async(&id, action.get("confirmation").unwrap_or(&json!("apply")), key.unwrap_or("batch-apply"), None)
                    .await
                    .map(Some),
                "undo" => manager
                    .undo_async(&id, action.get("confirmation").unwrap_or(&json!("undo")), key.unwrap_or("batch-undo"), None)
                    .await
                    .map(Some),
                "edit" => sim
                    .simulate_external_edit(
                        &LiveRef::from(action["ref"].as_str().unwrap()),
                        action["property"].as_str().unwrap(),
                        action["value"].clone(),
                    )
                    .map(|_| None),
                "patch" => {
                    patch(&mut sim.state.borrow_mut(), &action["patch"]);
                    Ok(None)
                }
                "deny" => {
                    faults.deny.set(action["value"].as_bool().unwrap());
                    Ok(None)
                }
                "release" => Ok(Some(json!(manager.release(&id)))),
                "finalize" => manager.finalize(&id).map(Some),
                "reconnect" => sim.reconnect().map(|_| None),
                other => panic!("{other}"),
            };
            if let Some(error) = step.get("error") {
                assert_eq!(result.unwrap_err().message(), error.as_str().unwrap(), "{label}");
            } else {
                let mut result = result.unwrap_or_else(|error| panic!("{label}: {error}"));
                if let Some(Value::Object(object)) = &mut result {
                    if object.get("transactionId").is_some() {
                        object.insert("transactionId".into(), "$transaction".into());
                    }
                    object.shift_remove("expiresAt");
                }
                match step.get("result") {
                    Some(expected) => same(result.as_ref().unwrap(), expected, &label),
                    None => assert!(result.is_none(), "{label}"),
                }
            }
            assert_eq!(fingerprint(&sim.state.borrow()).unwrap(), step["stateHash"].as_str().unwrap(), "state {label}");
        }
        same(&json!(*faults.calls.borrow()), &scenario["calls"], &format!("{} dispatches", scenario["name"]));
        assert_eq!(json!(faults.executions.get()), scenario["executions"], "{}", scenario["name"]);
        assert_eq!(json!(faults.replays.get()), scenario["replays"], "{}", scenario["name"]);
    }
}
/// Live as the Remote Script shows it, over the simulator: tracks' refs go by place ("track:at-{index}"), so a track
/// made or deleted moves the refs of the ones after it and the next track takes a deleted one's ref; mixer values are
/// kept as 32-bit floats (a written 0.6 reads back 0.6000000238418579); and a parameter on a whole-number range
/// keeps the nearest whole number (marked `wholeNumbers` here, a test-only flag).
struct LikeLive {
    sim: Rc<DeterministicLiveSimulator>,
    /// An operation the Remote Script refuses once, before anything changes ("; nothing changed").
    refuse: RefCell<Option<&'static str>>,
    /// Reads check their deadline, as the remote adapter does; the read with this number (from 1) first stalls past it.
    deadlines: Cell<bool>,
    stall_at: Cell<usize>,
    reads: Cell<usize>,
}
impl LikeLive {
    fn new(sim: Rc<DeterministicLiveSimulator>) -> Self {
        let live = LikeLive { sim, refuse: RefCell::new(None), deadlines: Cell::new(false), stall_at: Cell::new(0), reads: Cell::new(0) };
        live.settle();
        live
    }
    fn settle(&self) {
        let f32 = |value: &mut Value| {
            if let Some(n) = value.as_f64() {
                *value = json!(n as f32 as f64);
            }
        };
        let mut state = self.sim.state.borrow_mut();
        let places: HashMap<String, String> = state["tracks"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
            .filter_map(|(index, track)| Some((track["ref"].as_str()?.to_owned(), format!("track:at-{index}"))))
            .collect();
        fn rename(value: &mut Value, places: &HashMap<String, String>) {
            match value {
                Value::String(text) => {
                    if let Some(place) = places.get(text.as_str()) {
                        *text = place.clone();
                    }
                }
                Value::Array(items) => items.iter_mut().for_each(|item| rename(item, places)),
                Value::Object(fields) => fields.values_mut().for_each(|item| rename(item, places)),
                _ => {}
            }
        }
        rename(&mut state, &places);
        for track in state["tracks"].as_array_mut().unwrap() {
            if let Some(mixer) = track.get_mut("mixer").and_then(Value::as_object_mut) {
                for (key, value) in mixer.iter_mut() {
                    match key.as_str() {
                        "volume" | "pan" | "cueVolume" => f32(value),
                        "sends" => value.as_array_mut().into_iter().flatten().for_each(f32),
                        _ => {}
                    }
                }
            }
            for parameter in track["devices"]
                .as_array_mut()
                .into_iter()
                .flatten()
                .flat_map(|device| device["parameters"].as_array_mut().into_iter().flatten())
            {
                if parameter["wholeNumbers"] == true {
                    parameter["value"] = json!(parameter["value"].as_f64().unwrap().round());
                }
            }
        }
    }
}
impl LiveAdapter for LikeLive {
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
impl AsyncLiveAdapter for LikeLive {
    async fn snapshot_async(&self, c: Option<&LiveOperationContext>, _: Option<&LiveSnapshotRequest>) -> Result<LiveSnapshot, LiveError> {
        self.reads.set(self.reads.get() + 1);
        if self.deadlines.get() {
            let deadline = c.and_then(|c| c.deadline_ms).unwrap();
            if self.reads.get() == self.stall_at.get() {
                tokio::time::sleep(std::time::Duration::from_millis((deadline - kumi_common::time::now_ms() as f64).max(0.0) as u64 + 50))
                    .await;
            }
            if deadline <= kumi_common::time::now_ms() as f64 {
                return Err(LiveError::error("remote adapter deadline is invalid or expired"));
            }
        }
        self.sim.snapshot()
    }
    async fn discover_async(&self, _: &LiveDiscoveryRequest, _: Option<&LiveOperationContext>) -> Result<LiveDiscoveryResult, LiveError> {
        Err(LiveError::error("unused discovery"))
    }
    async fn get_async(&self, r: &LiveRef, _: Option<&LiveOperationContext>) -> Result<Option<Value>, LiveError> {
        self.sim.get(r)
    }
    async fn invoke_async(&self, i: &LiveInvocation, _: Option<&LiveOperationContext>) -> Result<Value, LiveError> {
        if self.refuse.borrow().is_some_and(|operation| operation == i.operation) {
            self.refuse.replace(None);
            return Err(LiveError::MutationNotDispatched(format!(
                "request failed: {} state changed since the preview; nothing changed",
                i.operation
            )));
        }
        let mut result = self.sim.invoke(i)?;
        let made = self.sim.state.borrow()["tracks"].as_array().unwrap().iter().position(|track| track["ref"] == result["ref"]);
        self.settle();
        // A made track's ref and fingerprint as the Remote Script reports them: by its place.
        if let Some(index) = made.filter(|_| i.operation == "track.create") {
            let reference = format!("track:at-{index}");
            let snapshot = serde_json::to_value(self.sim.snapshot()?).unwrap();
            let track = snapshot["tracks"].as_array().unwrap().iter().find(|track| track["ref"] == reference).unwrap();
            let owned = owned_track_fingerprint_row(&serde_json::from_value::<Track>(track.clone()).unwrap());
            let clips: Vec<_> = snapshot["arrangement"]["clips"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|clip| clip["trackRef"] == reference || clip["parentRef"] == reference)
                .collect();
            result["ref"] = json!(reference);
            result["createdFingerprint"] =
                json!(fingerprint(&without_playback_state(&json!({"track":owned,"arrangementClips":clips}))).unwrap());
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
/// The simulator's Set with a return and Main after its track, as Live always has them, and a whole-number parameter
/// (a root note's 0 to 11) on its Utility.
fn like_live() -> (Rc<DeterministicLiveSimulator>, Rc<LikeLive>) {
    let sim = Rc::new(DeterministicLiveSimulator::new());
    {
        let mut state = sim.state.borrow_mut();
        let drums = state["tracks"][0].clone();
        for (name, kind) in [("A Reverb", "return"), ("Main", "main")] {
            let mut track = json!({"ref":format!("track:{kind}"),"objectIdentity":format!("simulator:track:{kind}"),"name":name,"kind":kind,"clips":[],"clipSlots":[],"devices":[],"sends":[]});
            for key in ["volume", "pan", "mute", "solo", "armed", "mixer", "routing"] {
                track[key] = drums[key].clone();
            }
            state["tracks"].as_array_mut().unwrap().push(track);
        }
        state["tracks"][0]["devices"][0]["parameters"].as_array_mut().unwrap().push(json!({"ref":"parameter:root-1","objectIdentity":"simulator:parameter:root-1","name":"Root","value":0,"min":0,"max":11,"automatable":true,"quantization":0,"enabled":true,"revision":1,"wholeNumbers":true}));
    }
    let live = Rc::new(LikeLive::new(sim.clone()));
    (sim, live)
}
#[tokio::test]
async fn a_batch_is_confirmed_and_undone_on_float32_values_a_shorter_sends_list_and_a_whole_number_live_kept() {
    let (sim, live) = like_live();
    let values = || {
        let state = sim.state.borrow();
        let track = &state["tracks"][0];
        json!([track["mixer"]["volume"], track["mixer"]["sends"], track["devices"][0]["parameters"][1]["value"]])
    };
    let before = values();
    let manager = BatchTransactionManager::new(live, None, None);
    let preview = manager
        .preview_async(&json!({"operations":[
            {"kind":"mixer.set","trackRef":"track:at-0","volume":0.6,"sends":[0.3]},
            {"kind":"device.parameter.set","deviceRef":"device:utility-1","parameterRef":"parameter:root-1","value":6.5}
        ]}))
        .await
        .unwrap();
    let id = preview["transactionId"].as_str().unwrap();
    let applied = manager.apply_async(id, &json!("apply"), "batch-apply-key", None).await.unwrap();
    assert_eq!(applied["state"], "applied", "{applied}");
    // Live holds what it was asked for at its own precision, the second send as it was, and the root note it kept.
    assert_eq!(values(), json!([0.6f32 as f64, [0.3f32 as f64, 0.25], 7.0]));
    let undone = manager.undo_async(id, &json!("undo"), "batch-undo-key", None).await.unwrap();
    assert_eq!(undone["state"], "undone", "{undone}");
    assert_eq!(values(), before, "every value back");
}
#[tokio::test]
async fn a_step_refused_before_it_changed_anything_rolls_back_the_steps_before_it() {
    let (sim, live) = like_live();
    let volume = || sim.state.borrow()["tracks"][0]["mixer"]["volume"].clone();
    let before = volume();
    live.refuse.replace(Some("device.parameter.set"));
    let manager = BatchTransactionManager::new(live, None, None);
    let preview = manager
        .preview_async(&json!({"operations":[
            {"kind":"mixer.set","trackRef":"track:at-0","volume":0.5},
            {"kind":"device.parameter.set","deviceRef":"device:utility-1","parameterRef":"parameter:gain-1","value":0.25}
        ]}))
        .await
        .unwrap();
    let id = preview["transactionId"].as_str().unwrap();
    // Nothing of the second step reached Live: the first is taken back, not left for reconciling.
    let result = manager.apply_async(id, &json!("apply"), "batch-apply-key", None).await.unwrap();
    assert_eq!(
        (result["state"].clone(), result["failedIndex"].clone(), result["rolledBack"].clone()),
        (json!("compensated"), json!(1), json!(1)),
        "{result}"
    );
    assert_eq!(volume(), before);
}
#[tokio::test]
async fn a_rollback_after_the_apply_ran_out_of_time_has_time_of_its_own() {
    let (sim, live) = like_live();
    let volume = || sim.state.borrow()["tracks"][0]["mixer"]["volume"].clone();
    let before = volume();
    let manager = BatchTransactionManager::new(live.clone(), None, None);
    let preview = manager
        .preview_async(&json!({"operations":[
            {"kind":"mixer.set","trackRef":"track:at-0","volume":0.5},
            {"kind":"device.parameter.set","deviceRef":"device:utility-1","parameterRef":"parameter:gain-1","value":0.25}
        ]}))
        .await
        .unwrap();
    let id = preview["transactionId"].as_str().unwrap();
    // The apply's reads: the first step's two, then the second step's first, which waits out the deadline.
    live.deadlines.set(true);
    live.stall_at.set(live.reads.get() + 3);
    let context = LiveOperationContext::with_deadline(kumi_common::time::now_ms() as f64 + 500.0);
    let result = manager.apply_async(id, &json!("apply"), "batch-apply-key", Some(&context)).await.unwrap();
    assert_eq!((result["state"].clone(), result["rolledBack"].clone()), (json!("compensated"), json!(1)), "{result}");
    assert_eq!(volume(), before);
}
#[tokio::test]
async fn two_tracks_made_in_one_batch_are_confirmed_and_their_deletion_on_undo_is_too_with_refs_by_place() {
    let (sim, live) = like_live();
    let before = sim.state.borrow().clone();
    let manager = BatchTransactionManager::new(live, None, None);
    let preview = manager
        .preview_async(&json!({"operations":[
            {"kind":"track.create","name":"Bass","trackKind":"midi","index":1},
            {"kind":"track.create","name":"Lead","trackKind":"audio","index":1}
        ]}))
        .await
        .unwrap();
    let id = preview["transactionId"].as_str().unwrap();
    // Making Bass moves the return's and Main's refs: Lead still finds the Set as previewed, and is made above Bass.
    let applied = manager.apply_async(id, &json!("apply"), "batch-apply-key", None).await.unwrap();
    assert_eq!(applied["state"], "applied", "{applied}");
    let names: Vec<_> = sim.state.borrow()["tracks"].as_array().unwrap().iter().map(|track| track["name"].clone()).collect();
    assert_eq!(names, ["Drums", "Lead", "Bass", "A Reverb", "Main"]);
    // Deleting Lead hands its ref back to Bass, and deleting Bass hands its to the return: both still confirmed.
    let undone = manager.undo_async(id, &json!("undo"), "batch-undo-key", None).await.unwrap();
    assert_eq!(undone["state"], "undone", "{undone}");
    assert_eq!(fingerprint(&sim.state.borrow()).unwrap(), fingerprint(&before).unwrap(), "the Set as it was");
}
#[test]
fn parameter_helpers_preserve_macro_alias_order_and_float_tolerance() {
    let rows = json!([{"ref":"p1","objectIdentity":"i1","value":1},{"ref":"p1","objectIdentity":"i1","value":2},{"ref":"p2","objectIdentity":"i2"},{"ref":"p1","objectIdentity":"i3"}]);
    assert_eq!(unique_parameter_rows(rows.as_array().unwrap()), vec![rows[0].clone(), rows[2].clone(), rows[3].clone()]);
    assert!(same_parameter_value(&json!(0.29999998), &json!(0.3)));
    assert!(!same_parameter_value(&json!(0.2999), &json!(0.3)));
    let snapshot = json!({"tracks":[{"ref":"t","objectIdentity":"ti","devices":[{"ref":"d","objectIdentity":"di","parameters":[rows[0]],"macros":[rows[1],rows[2]]}]}]});
    let authority = parameter_authority(&snapshot, "p2").unwrap();
    assert_eq!(authority["siblings"], json!([{"ref":"p1","objectIdentity":"i1"},{"ref":"p2","objectIdentity":"i2"}]));
    let nested =
        json!([{"ref":"root","chains":[{"devices":[{"ref":"chain-child"}]}],"drumPads":[{"chains":[{"devices":[{"ref":"pad-child"}]}]}]}]);
    assert_eq!(
        flatten_device_rows(&nested).iter().map(|row| row["ref"].as_str().unwrap()).collect::<Vec<_>>(),
        vec!["root", "chain-child", "pad-child"]
    );
}
