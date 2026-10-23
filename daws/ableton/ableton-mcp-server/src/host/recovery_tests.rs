use super::*;
struct Adapter {
    sim: DeterministicLiveSimulator,
    status: LiveStatus,
    options: Value,
    calls: RefCell<Vec<Value>>,
}
impl Adapter {
    fn new(options: &Value) -> Self {
        let sim = DeterministicLiveSimulator::new();
        let mut status = serde_json::to_value(sim.status().unwrap()).unwrap();
        status["operations"] = if options.get("realtime").is_some() { json!(["snapshot", "realtime.stats"]) } else { json!(["snapshot"]) };
        if let Some(patch) = options["status"].as_object() {
            for (k, v) in patch {
                status[k] = v.clone();
            }
        }
        {
            let mut state = sim.state.borrow_mut();
            if let Some(patch) = options["transport"].as_object() {
                for (k, v) in patch {
                    state["playback"]["transport"][k] = v.clone();
                }
            }
            for key in ["playingTargets", "firedTargets"] {
                if let Some(v) = options.get(key) {
                    state["playback"][key] = v.clone();
                }
            }
        }
        Self { sim, status: serde_json::from_value(status).unwrap(), options: options.clone(), calls: Default::default() }
    }
}
impl LiveAdapter for Adapter {
    fn status(&self) -> Result<LiveStatus, LiveError> {
        Ok(self.status.clone())
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
    async fn snapshot_async(&self, c: Option<&LiveOperationContext>, r: Option<&LiveSnapshotRequest>) -> Result<LiveSnapshot, LiveError> {
        self.sim.snapshot_async(c, r).await
    }
    async fn discover_async(&self, r: &LiveDiscoveryRequest, c: Option<&LiveOperationContext>) -> Result<LiveDiscoveryResult, LiveError> {
        self.sim.discover_async(r, c).await
    }
    async fn get_async(&self, r: &LiveRef, c: Option<&LiveOperationContext>) -> Result<Option<Value>, LiveError> {
        self.sim.get_async(r, c).await
    }
    async fn invoke_async(&self, i: &LiveInvocation, c: Option<&LiveOperationContext>) -> Result<Value, LiveError> {
        self.calls.borrow_mut().push(json!({"invocation":i,"deadline":c.is_some_and(|c|c.deadline_ms.is_some())}));
        if let Some(fault) = self.options["watchFault"].as_str() {
            match fault {
                "error" => return Err(LiveError::error("injected failure")),
                "refusal" => return Err(LiveError::MutationNotDispatched("not dispatched".into())),
                "nothing" => return Err(LiveError::error("Nothing changed in Live: refused")),
                _ => {}
            }
            return Ok(match i.operation.as_str() {
                "application.message" => json!({"shown":true}),
                "undo.step.begin" => json!({"open":true,"stepId":"step-fixture","expiresAt":9999999999999i64}),
                "undo.step.end" => json!({"open":false}),
                "browser.inspect" => json!({"id":"item","objectIdentity":"browser-object","name":"Sample"}),
                "browser.preview.start" => json!({"previewId":"p".repeat(32)}),
                "browser.preview.stop" => json!({"stopped":true}),
                _ => json!({}),
            });
        }
        if self.options["invokeFail"] == true {
            return Err(LiveError::error("injected status failure"));
        }
        match i.operation.as_str() {
            "realtime.stats" => Ok(self.options["realtime"].clone()),
            "audio.capture.status" => Ok(self.options.get("capture").cloned().unwrap_or(json!({}))),
            _ => self.sim.invoke_async(i, c).await,
        }
    }
    async fn reconnect_async(&self, _: Option<&LiveOperationContext>) -> Result<LiveStatus, LiveError> {
        self.sim.reconnect()
    }
    async fn close(&self) -> Result<(), LiveError> {
        Ok(())
    }
    fn has_refresh_status_async(&self) -> bool {
        true
    }
    async fn refresh_status_async(&self, _: Option<&LiveOperationContext>) -> Result<LiveStatus, LiveError> {
        self.calls.borrow_mut().push(json!({"refresh":true}));
        if self.options["refreshFail"] == true {
            return Err(LiveError::error("injected refresh failure"));
        }
        self.status()
    }
    fn has_retire_transaction_async(&self) -> bool {
        self.options["noRetire"] != true
    }
    async fn retire_transaction_async(&self, id: &str, c: Option<&LiveOperationContext>, terminal: bool) -> Result<Value, LiveError> {
        self.calls.borrow_mut().push(json!({"retire":id,"deadline":c.is_some_and(|c|c.deadline_ms.is_some()),"terminal":terminal}));
        if self.options["retireFail"] == true {
            return Err(LiveError::error("injected retirement failure"));
        }
        Ok(Value::Null)
    }
}
fn clean(mut value: Value) -> Value {
    if let Some(text) = value["result"]["content"][0]["text"].as_str() {
        if let Ok(body) = serde_json::from_str::<Value>(text) {
            value["result"]["content"][0]["text"] = body;
        }
    }
    value
}
fn same(a: &Value, b: &Value, label: &str) {
    assert_eq!(canonical_mutation_identity(a).unwrap(), canonical_mutation_identity(b).unwrap(), "{label}");
}
fn args() -> Value {
    json!({"transactionId":"fixture","resolution":"manually-restored","confirmation":"finalize-recovery-record","evidence":{"provenance":"observed","scope":"the exact Set"}})
}
#[tokio::test]
async fn recovery_finalization_safety_identity_retirement_and_refusal_match_source() {
    let fixture: Value =
        serde_json::from_str(include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/host-recovery-oracle.json"))).unwrap();
    for (index, row) in fixture["rows"].as_array().unwrap().iter().enumerate() {
        let o = &row["options"];
        let adapter = Rc::new(Adapter::new(o));
        let host = McpHost::new(adapter.clone(), McpHostOptions::default()).unwrap();
        let owner = match o["owner"].as_str().unwrap_or("transactions") {
            "transactions" => &host.transactions,
            "arrangementTransactions" => &host.arrangement_transactions,
            "sessionStructureTransactions" => &host.session_structure_transactions,
            "deviceParameterTransactions" => &host.device_parameter_transactions,
            "deviceParametersTransactions" => &host.device_parameters_transactions,
            "auditionTransactions" => &host.audition_transactions,
            "transportTransactions" => &host.transport_transactions,
            "clipLaunchTransactions" => &host.clip_launch_transactions,
            "noteEditTransactions" => &host.note_edit_transactions,
            "clipLifecycleTransactions" => &host.clip_lifecycle_transactions,
            "audioCaptureTransactions" => &host.audio_capture_transactions,
            _ => panic!(),
        };
        let mut record = json!({"id":"fixture","state":"applied","epoch":1,"expiresAt":kumi_common::time::now_ms_f64()+100000.0});
        if let Some(patch) = o["record"].as_object() {
            for (k, v) in patch {
                record[k] = v.clone();
            }
        }
        if o["present"] != false {
            owner.insert("fixture", record).unwrap();
        }
        host.recovery_finalization_in_flight.set(o["barrier"] == true);
        host.active_async_operations.set(usize::from(o["active"] == true));
        let result = host.live_recovery_finalize_async(&json!(1), &row["args"]).await.unwrap_or_else(|e| json!({"error":e.message()}));
        let label = format!("row {index} {o} args={}", row["args"]);
        same(&clean(result), &row["result"], &label);
        same(&json!(*adapter.calls.borrow()), &row["calls"], &format!("{label} calls"));
        assert_eq!(owner.get("fixture").is_some(), row["exists"].as_bool().unwrap(), "{label}");
        assert_eq!(host.recovery_finalization_in_flight.get(), row["barrier"].as_bool().unwrap(), "{label}");
        assert!(!retention::any_in_flight());
    }
}
#[tokio::test]
async fn recovery_finalization_refuses_global_in_flight_work_and_resets_after_failure() {
    let adapter = Rc::new(Adapter::new(&json!({"refreshFail":true})));
    let host = McpHost::new(adapter, McpHostOptions::default()).unwrap();
    host.transactions.insert("fixture", json!({"id":"fixture","state":"uncertain","epoch":1,"expiresAt":0})).unwrap();
    retention::mark_in_flight("another");
    let reply = clean(host.live_recovery_finalize_async(&json!(1), &args()).await.unwrap());
    assert!(reply["result"]["content"][0]["text"]["reason"].as_str().unwrap().contains("global safety"));
    retention::clear_in_flight("another");
    assert!(host.live_recovery_finalize_async(&json!(2), &args()).await.is_err());
    assert!(!retention::any_in_flight());
    assert!(!host.recovery_finalization_in_flight.get());
    assert!(host.transactions.get("fixture").is_some());
}

#[tokio::test]
async fn recovery_finalization_retires_manager_owned_records_without_undoing_live() {
    for kind in ["midi", "batch", "device"] {
        let mut adapter = Adapter::new(&json!({}));
        adapter.status.operations =
            adapter.sim.status().unwrap().operations.map(|ops| ops.into_iter().filter(|op| op != "realtime.stats").collect());
        let adapter = Rc::new(adapter);
        {
            let mut state = adapter.sim.state.borrow_mut();
            state["scenes"]
                .as_array_mut()
                .unwrap()
                .push(json!({"ref":"scene:scene-2","objectIdentity":"simulator:scene:scene-2","name":"Scene 2","index":1}));
            state["tracks"][0]["clipSlots"].as_array_mut().unwrap().push(json!({"ref":"clip-slot:track-1:1","parentRef":"track:track-1","objectIdentity":"simulator:clip-slot:track-1:1","sceneIndex":1,"clipRef":null,"empty":true}));
        }
        let host = McpHost::new(adapter.clone(), McpHostOptions::default()).unwrap();
        let preview = match kind {
            "midi" => host
                .midi_transactions
                .preview_async(&mut json!({"trackRef":"track:track-1","sceneIndex":1,"name":"Created","length":4,"notes":[]}))
                .await
                .unwrap(),
            "batch" => host
                .batch_transactions
                .preview_async(&json!({"operations":[{"kind":"track.rename","trackRef":"track:track-1","name":"Changed"}]}))
                .await
                .unwrap(),
            _ => {
                let reference = adapter.sim.state.borrow()["tracks"][0]["devices"][0]["ref"].clone();
                host.device_state_transactions
                    .preview_async(&json!({"deviceRef":reference,"dispositions":[]}), "recall", None)
                    .await
                    .unwrap()
            }
        };
        let txid = preview["transactionId"].as_str().unwrap();
        match kind {
            "midi" => host.midi_transactions.apply_async(txid, &json!("apply"), "apply-key", None).await.unwrap(),
            "batch" => host.batch_transactions.apply_async(txid, &json!("apply"), "apply-key", None).await.unwrap(),
            _ => host.device_state_transactions.apply_async(txid, &json!("apply"), "apply-key", None).await.unwrap(),
        };
        let before = adapter.sim.state.borrow().clone();
        adapter.calls.borrow_mut().clear();
        let mut params = args();
        params["transactionId"] = json!(txid);
        let outcome = clean(host.live_recovery_finalize_async(&json!(1), &params).await.unwrap());
        let body = &outcome["result"]["content"][0]["text"];
        assert_eq!(outcome["result"]["isError"], false, "{kind}: {outcome}");
        assert_eq!(body["finalized"], true);
        assert_eq!(body["priorState"], "applied");
        assert_eq!(body["liveMutated"], false);
        assert_eq!(body["recoveryAuthorityRetired"], true);
        assert_eq!(*adapter.sim.state.borrow(), before);
        assert_eq!(*adapter.calls.borrow(), vec![json!({"refresh":true}), json!({"retire":txid,"deadline":true,"terminal":true})]);
        let repeated = clean(host.live_recovery_finalize_async(&json!(2), &params).await.unwrap());
        assert_eq!(repeated["result"]["isError"], true);
        assert!(!retention::any_in_flight());
    }
}

#[tokio::test]
async fn undo_watches_include_other_live_invocations_but_exclude_reads_and_proven_refusals() {
    let fixture: Value =
        serde_json::from_str(include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/host-recovery-oracle.json"))).unwrap();
    for row in fixture["watches"].as_array().unwrap() {
        let mut adapter = Adapter::new(&json!({"watchFault":row["fault"]}));
        adapter.status.operations = Some(crate::registry::live_registry_operations().to_vec());
        let adapter = Rc::new(adapter);
        let host = McpHost::new(adapter.clone(), McpHostOptions::default()).unwrap();
        let record =
            host.transactions.insert("fixture", json!({"id":"fixture","state":"applied","epoch":1,"expiresAt":9999999999999i64})).unwrap();
        let params = json!({"transactionId":"fixture","confirmation":"undo","idempotencyKey":"undo-key"});
        let result = host
            .with_undo_watch(&json!(1), &params, async {
                match row["tool"].as_str().unwrap() {
                    "message" => host.live_message_async(&json!(2), &json!({"text":"Message"})).await,
                    "python" => host.live_run_python_async(&json!(2), &json!({"code":"1"}), None).await,
                    "undo-begin" => host.live_undo_step_begin_async(&json!(2), &json!({})).await,
                    "undo-end" => host.live_undo_step_end_async(&json!(2), &json!({})).await,
                    "browser-preview" => host.live_browser_preview_async(&json!(2), &json!({"itemId":"item"})).await,
                    "browser-stop" => host.live_browser_preview_stop_async(&json!(2), &json!({"previewId":"p".repeat(32)})).await,
                    "observe-poll" => host.live_observe_poll_async(&json!(2), &json!({"subscriptionId":"one"})).await,
                    _ => host.live_song_state_async(&json!(2), &json!({})).await,
                };
                record.borrow_mut()["state"] = json!("uncertain");
                Ok(adapter_tool_error(&json!(1), &LiveError::error("an undo read failed"), "Check the Set."))
            })
            .await
            .unwrap();
        same(&clean(result), &row["result"], &row.to_string());
        assert_eq!(record.borrow()["state"], row["state"]);
        let calls: Vec<_> = adapter.calls.borrow().iter().filter_map(|c| c.get("invocation").map(|i| i["operation"].clone())).collect();
        same(&json!(calls), &row["calls"], "calls");
        assert!(host.undo_watches.borrow().is_empty());
    }
}
