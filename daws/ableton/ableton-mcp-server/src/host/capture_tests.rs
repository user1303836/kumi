use super::*;
fn fixture() -> Value {
    serde_json::from_str(include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/host-capture-oracle.json"))).unwrap()
}
#[test]
fn capture_fixture_normalizes_the_temporary_path_separator() {
    for (root, path) in [("/tmp/capture", "/tmp/capture/Capture.wav"), (r"C:\Temp\capture", r"C:\Temp\capture\Capture.wav")] {
        assert_eq!(
            clean(json!({"clip":{"filePath":path},"other":"keep\\this"}), root),
            json!({"clip":{"filePath":"$root/Capture.wav"},"other":"keep\\this"})
        );
    }
}
fn clean(mut value: Value, root: &str) -> Value {
    if let Some(text) = value["result"]["content"][0]["text"].as_str() {
        if let Ok(body) = serde_json::from_str::<Value>(text) {
            value["result"]["content"][0]["text"] = body;
        }
    }
    fn walk(value: &mut Value, root: &str) {
        if let Some(text) = value["result"]["content"][0]["text"].as_str() {
            if let Ok(body) = serde_json::from_str::<Value>(text) {
                value["result"]["content"][0]["text"] = body;
            }
        }
        match value {
            Value::Object(o) => {
                for (k, v) in o {
                    if let Some(s) = v.as_str() {
                        if !root.is_empty() && s.contains(root) {
                            *v = json!(s.replace(root, "$root").replace("$root\\", "$root/"));
                        } else if s.starts_with("audio_capture_") {
                            *v = json!("$transaction");
                        } else if s.starts_with("capture_") {
                            *v = json!("$capture");
                        } else if k == "confirmation" && s.len() == 43 {
                            *v = json!("$confirmation");
                        }
                    }
                    if ["expiresAt", "startedAt"].contains(&k.as_str()) {
                        *v = json!("$time");
                    }
                    if ["observedAt", "analyzedAt", "diagnosedAt", "createdAt", "capturedAt"].contains(&k.as_str()) {
                        *v = json!("$iso");
                    }
                    if k == "diagnosisId" {
                        assert_eq!(v.as_str().unwrap().len(), 64);
                        *v = json!("$diagnosis");
                    }
                    walk(v, root);
                }
            }
            Value::Array(a) => {
                for v in a {
                    walk(v, root)
                }
            }
            _ => {}
        }
    }
    walk(&mut value, root);
    value
}
fn same(a: &Value, b: &Value, label: &str) {
    assert_eq!(canonical_mutation_identity(a).unwrap(), canonical_mutation_identity(b).unwrap(), "{label}");
}
fn call_context(c: Option<&LiveOperationContext>) -> Value {
    let Some(c) = c else { return Value::Null };
    let mut value = json!({"deadline":c.deadline_ms.is_some()});
    if c.signal.is_some() {
        value["signal"] = json!(true);
    }
    if let Some(k) = &c.idempotency_key {
        value["idempotencyKey"] = json!(k);
    }
    if let Some(k) = &c.transaction_id {
        value["transactionId"] = json!(k);
    }
    value
}
struct Adapter {
    sim: DeterministicLiveSimulator,
    status: LiveStatus,
    options: Value,
    calls: RefCell<Vec<Value>>,
    status_index: Cell<usize>,
    snapshot: RefCell<Value>,
}
impl Adapter {
    fn new(o: &Value, f: &Value) -> Self {
        let mut status = f["status"].clone();
        if let Some(p) = o["status"].as_object() {
            for (k, v) in p {
                status[k] = v.clone();
            }
        }
        Self {
            sim: DeterministicLiveSimulator::new(),
            status: serde_json::from_value(status).unwrap(),
            options: o.clone(),
            calls: Default::default(),
            status_index: Cell::new(0),
            snapshot: RefCell::new(o.get("snapshot").unwrap_or(&f["snapshot"]).clone()),
        }
    }
}
impl LiveAdapter for Adapter {
    fn status(&self) -> Result<LiveStatus, LiveError> {
        Ok(self.status.clone())
    }
    fn snapshot(&self) -> Result<LiveSnapshot, LiveError> {
        serde_json::from_value(self.snapshot.borrow().clone()).map_err(|e| LiveError::error(e.to_string()))
    }
    fn get(&self, r: &LiveRef) -> Result<Option<Value>, LiveError> {
        self.sim.get(r)
    }
    fn invoke(&self, _: &LiveInvocation) -> Result<Value, LiveError> {
        Err(LiveError::error("async only"))
    }
    fn subscribe(&self, l: LiveListener) -> Result<Unsubscribe, LiveError> {
        self.sim.subscribe(l)
    }
    fn reconnect(&self) -> Result<LiveStatus, LiveError> {
        self.status()
    }
}
#[async_trait::async_trait(?Send)]
impl AsyncLiveAdapter for Adapter {
    async fn snapshot_async(&self, c: Option<&LiveOperationContext>, r: Option<&LiveSnapshotRequest>) -> Result<LiveSnapshot, LiveError> {
        self.calls.borrow_mut().push(clean(json!({"method":"snapshot","request":r,"context":call_context(c)}), ""));
        if self.options["snapshotError"] == true {
            return Err(LiveError::error("snapshot unavailable"));
        }
        self.snapshot()
    }
    async fn discover_async(&self, _: &LiveDiscoveryRequest, _: Option<&LiveOperationContext>) -> Result<LiveDiscoveryResult, LiveError> {
        serde_json::from_value(json!({"epoch":1,"kind":"track","items":[],"truncated":false,"revision":"capture"}))
            .map_err(|e| LiveError::error(e.to_string()))
    }
    async fn get_async(&self, r: &LiveRef, _: Option<&LiveOperationContext>) -> Result<Option<Value>, LiveError> {
        self.get(r)
    }
    async fn invoke_async(&self, i: &LiveInvocation, c: Option<&LiveOperationContext>) -> Result<Value, LiveError> {
        self.calls.borrow_mut().push(clean(json!({"method":"invoke","invocation":i,"context":call_context(c)}), ""));
        if let Some(message) = self.options["errors"][&i.operation].as_str() {
            return Err(LiveError::error(message));
        }
        match i.operation.as_str() {
            "audio.capture.inspect" => Ok(self.options.get("plan").cloned().unwrap_or_else(|| fixture()["plan"].clone())),
            "audio.capture.status" => {
                let defaults = json!([{"state":"cleaned","active":false,"playbackStopped":true,"captureId":"capture_fixture","sourceSlotRef":"clip-slot:source:0","destinationSlotRef":"clip-slot:capture:0","destinationTrackRef":"track:capture"}]);
                let statuses = self.options.get("statuses").unwrap_or(&defaults).as_array().unwrap();
                let index = self.status_index.get();
                self.status_index.set(index + 1);
                let status = statuses[index.min(statuses.len() - 1)].clone();
                if let Some(error) = status["error"].as_str() {
                    Err(LiveError::error(error))
                } else {
                    Ok(status)
                }
            }
            "audio.capture.cleanup" => Ok(self.options.get("cleaned").cloned().unwrap_or(json!({"cleaned":true}))),
            _ => Ok(json!({"stopped":true})),
        }
    }
    async fn reconnect_async(&self, _: Option<&LiveOperationContext>) -> Result<LiveStatus, LiveError> {
        self.status()
    }
    async fn close(&self) -> Result<(), LiveError> {
        Ok(())
    }
    fn has_refresh_status_async(&self) -> bool {
        true
    }
    async fn refresh_status_async(&self, c: Option<&LiveOperationContext>) -> Result<LiveStatus, LiveError> {
        self.calls.borrow_mut().push(json!({"method":"status","context":call_context(c)}));
        if self.options["statusError"] == true {
            Err(LiveError::error("status unavailable"))
        } else {
            self.status()
        }
    }
}
#[tokio::test]
async fn capture_validation_and_status_match_source() {
    let f = fixture();
    for (index, row) in f["rows"].as_array().unwrap().iter().enumerate() {
        let adapter = Rc::new(Adapter::new(&row["options"], &f));
        let host = Rc::new(McpHost::new(adapter.clone(), McpHostOptions::default()).unwrap());
        let tool = row["tool"].as_str().unwrap();
        let args = &row["args"];
        let result = match tool {
            "liveAudioCapturePreviewAsync" => host.live_audio_capture_preview_async(&json!(1), args).await,
            "liveAudioCaptureApplyAsync" => host.live_audio_capture_apply_async(&json!(1), args, None).await.unwrap_or(Value::Null),
            "liveAudioCaptureStatusAsync" => {
                host.live_audio_capture_status_async(&json!(1), if row["omitted"] == true { None } else { Some(args) }).await
            }
            "liveAudioCaptureEmergencyStopAsync" => host.live_audio_capture_emergency_stop_async(&json!(1), args).await,
            _ => panic!(),
        };
        same(&clean(result, ""), &row["result"], &format!("row {index} {tool} {args}"));
        same(&json!(*adapter.calls.borrow()), &row["calls"], &format!("row {index} calls"));
    }
}
#[tokio::test]
async fn capture_recovery_authority_and_residuals_match_source() {
    let f = fixture();
    for (index, row) in f["recovery"].as_array().unwrap().iter().enumerate() {
        let adapter = Rc::new(Adapter::new(&row["options"], &f));
        let host = McpHost::new(adapter.clone(), McpHostOptions::default()).unwrap();
        let mut record = f["record"].clone();
        for (k, v) in row["patch"].as_object().unwrap() {
            record[k] = v.clone();
        }
        let record = Rc::new(RefCell::new(record));
        let result = host.recover_audio_capture(&record, None).await;
        same(&result, &row["result"], &format!("recovery {index} {}", row["label"]));
        same(&json!(*adapter.calls.borrow()), &row["calls"], &format!("recovery {index} calls"));
        same(&clean(record.borrow().clone(), ""), &row["record"], &format!("recovery {index} record"));
    }
}

struct FileAdapter {
    base: Adapter,
    root: tempfile::TempDir,
    capture: RefCell<Value>,
    scenario: String,
    failed: Cell<bool>,
    epoch: Cell<i64>,
    applying: Cell<bool>,
    wave: Vec<u8>,
}
impl FileAdapter {
    fn new(scenario: &str, f: &Value) -> Self {
        let root = tempfile::tempdir().unwrap();
        let escaped = serde_json::to_string(root.path().to_str().unwrap()).unwrap();
        let snapshot: Value =
            serde_json::from_str(&serde_json::to_string(&f["snapshot"]).unwrap().replace("$root", &escaped[1..escaped.len() - 1])).unwrap();
        std::fs::write(root.path().join("Disposable.als"), b"set").unwrap();
        Self {
            base: Adapter::new(&json!({"snapshot":snapshot}), f),
            root,
            capture: RefCell::new(Value::Null),
            scenario: scenario.into(),
            failed: Cell::new(false),
            epoch: Cell::new(1),
            applying: Cell::new(false),
            wave: base64::engine::general_purpose::STANDARD.decode(f["waveBase64"].as_str().unwrap()).unwrap(),
        }
    }
    fn media_path(&self) -> std::path::PathBuf {
        self.root.path().join("Capture.wav")
    }
    fn companion_path(&self) -> std::path::PathBuf {
        self.root.path().join("Capture.wav.asd")
    }
    async fn stop(&self) -> Result<Value, LiveError> {
        if self.capture.borrow().is_null() {
            return Err(LiveError::error("no capture"));
        }
        {
            let mut snapshot = self.base.snapshot.borrow_mut();
            for key in ["playing", "arrangementRecord", "sessionRecord"] {
                snapshot["playback"]["transport"][key] = json!(false);
            }
            let dest = snapshot["tracks"].as_array_mut().unwrap().last_mut().unwrap();
            dest["armed"] = json!(false);
            dest["monitoringState"] = json!("auto");
            dest["routing"]["inputType"] = json!("Ext. In");
            dest["clipSlots"][0]["clipRef"] = json!("clip:captured");
            dest["clipSlots"][0]["empty"] = json!(false);
        }
        tokio::fs::write(self.media_path(), &self.wave).await.unwrap();
        let mut capture = self.capture.borrow_mut();
        capture["state"] = json!("captured");
        capture["active"] = json!(false);
        capture["playbackStopped"] = json!(true);
        capture["clip"] =
            json!({"ref":"clip:captured","name":"Capture","length":0.1,"isAudio":true,"filePath":self.media_path().to_str().unwrap()});
        let mut result = capture.clone();
        result["stopped"] = json!(true);
        Ok(result)
    }
}
impl LiveAdapter for FileAdapter {
    fn status(&self) -> Result<LiveStatus, LiveError> {
        let mut status = self.base.status.clone();
        status.epoch = Some(self.epoch.get());
        Ok(status)
    }
    fn snapshot(&self) -> Result<LiveSnapshot, LiveError> {
        self.base.snapshot()
    }
    fn get(&self, r: &LiveRef) -> Result<Option<Value>, LiveError> {
        self.base.get(r)
    }
    fn invoke(&self, i: &LiveInvocation) -> Result<Value, LiveError> {
        self.base.invoke(i)
    }
    fn subscribe(&self, l: LiveListener) -> Result<Unsubscribe, LiveError> {
        self.base.subscribe(l)
    }
    fn reconnect(&self) -> Result<LiveStatus, LiveError> {
        self.status()
    }
}
#[async_trait::async_trait(?Send)]
impl AsyncLiveAdapter for FileAdapter {
    async fn snapshot_async(&self, c: Option<&LiveOperationContext>, r: Option<&LiveSnapshotRequest>) -> Result<LiveSnapshot, LiveError> {
        self.base.snapshot_async(c, r).await
    }
    async fn discover_async(&self, r: &LiveDiscoveryRequest, c: Option<&LiveOperationContext>) -> Result<LiveDiscoveryResult, LiveError> {
        self.base.discover_async(r, c).await
    }
    async fn get_async(&self, r: &LiveRef, c: Option<&LiveOperationContext>) -> Result<Option<Value>, LiveError> {
        self.base.get_async(r, c).await
    }
    async fn invoke_async(&self, i: &LiveInvocation, c: Option<&LiveOperationContext>) -> Result<Value, LiveError> {
        self.base
            .calls
            .borrow_mut()
            .push(clean(json!({"method":"invoke","invocation":i,"context":call_context(c)}), self.root.path().to_str().unwrap()));
        let op = i.operation.strip_prefix("audio.capture.").unwrap();
        if !self.failed.get() && ["start", "stop", "cleanup"].contains(&op) && self.scenario == format!("before-{op}") {
            self.failed.set(true);
            return Err(LiveError::error(format!("injected {op} failure: {}", self.root.path().display())));
        }
        if !self.failed.get() && self.scenario == "range-start" && op == "start" {
            self.failed.set(true);
            return Err(LiveError::range_error("bounded private path"));
        }
        let result = match op {
            "inspect" => {
                let mut p = fixture()["plan"].clone();
                p["sourceSlotRef"] = i.args["sourceSlotRef"].clone();
                p["destinationSlotRef"] = i.args["destinationSlotRef"].clone();
                p
            }
            "start" => {
                *self.capture.borrow_mut() = json!({"active":true,"state":"active","captureId":i.args["captureId"],"sourceSlotRef":i.args["sourceSlotRef"],"destinationSlotRef":i.args["destinationSlotRef"],"destinationTrackRef":"track:capture","startedAt":now_ms_f64(),"expiresAt":now_ms_f64()+i.args["maxDurationMs"].as_f64().unwrap(),"recoveryToken":"mapper-token-0000000000000000","residual":[]});
                let mut snapshot = self.base.snapshot.borrow_mut();
                snapshot["playback"]["transport"]["playing"] = json!(true);
                let dest = snapshot["tracks"].as_array_mut().unwrap().last_mut().unwrap();
                dest["armed"] = json!(true);
                dest["monitoringState"] = json!("off");
                dest["routing"]["inputType"] = json!("Resampling");
                json!({"captureId":i.args["captureId"],"token":self.capture.borrow()["recoveryToken"],"expiresAt":self.capture.borrow()["expiresAt"],"state":"active"})
            }
            "stop" | "emergency-stop" => self.stop().await?,
            "status" => {
                if self.capture.borrow().is_null() {
                    json!({"active":false,"state":"idle"})
                } else {
                    self.capture.borrow().clone()
                }
            }
            "cleanup" => {
                let mut capture = self.capture.borrow_mut();
                if capture.is_null() || i.args["expectedClipRef"] != capture["clip"]["ref"] {
                    return Err(LiveError::error("wrong clip"));
                }
                capture["state"] = json!("cleaned");
                capture["active"] = json!(false);
                capture["recoveryToken"] = Value::Null;
                capture.as_object_mut().unwrap().remove("clip");
                let mut snapshot = self.base.snapshot.borrow_mut();
                let dest = snapshot["tracks"].as_array_mut().unwrap().last_mut().unwrap();
                dest["clipSlots"][0]["clipRef"] = Value::Null;
                dest["clipSlots"][0]["empty"] = json!(true);
                json!({"cleaned":true,"filePath":self.media_path().to_str().unwrap()})
            }
            _ => return Err(LiveError::error(format!("unexpected {}", i.operation))),
        };
        if op == "stop" {
            match self.scenario.as_str() {
                "invalid-wave" => {
                    tokio::fs::write(self.media_path(), b"not-a-wave").await.unwrap();
                }
                "missing-media" => {
                    tokio::fs::remove_file(self.media_path()).await.unwrap();
                }
                "mapper-residual" => self.capture.borrow_mut()["residual"] = json!(["destination-route-changed-externally"]),
                _ => {}
            }
        }
        if op == "cleanup" {
            match self.scenario.as_str() {
                "late-asd" => {
                    tokio::fs::write(self.companion_path(), [3u8; 128]).await.unwrap();
                }
                "final-arm" => self.base.snapshot.borrow_mut()["tracks"].as_array_mut().unwrap().last_mut().unwrap()["armed"] = json!(true),
                "raw-replacement" => {
                    tokio::fs::write(self.media_path(), &self.wave).await.unwrap();
                }
                _ => {}
            }
        }
        if !self.failed.get() && self.scenario == format!("after-{op}") {
            self.failed.set(true);
            return Err(LiveError::error(format!("injected {op} failure: {}", self.root.path().display())));
        }
        if self.scenario == "bad-start-token" && op == "start" {
            return Ok(json!({}));
        }
        Ok(result)
    }
    async fn reconnect_async(&self, _: Option<&LiveOperationContext>) -> Result<LiveStatus, LiveError> {
        self.status()
    }
    async fn close(&self) -> Result<(), LiveError> {
        Ok(())
    }
    fn has_refresh_status_async(&self) -> bool {
        true
    }
    async fn refresh_status_async(&self, c: Option<&LiveOperationContext>) -> Result<LiveStatus, LiveError> {
        self.base.calls.borrow_mut().push(json!({"method":"status","context":call_context(c)}));
        if self.applying.get() && self.scenario == "before-status" {
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
        self.status()
    }
}
fn numerically_same(a: &Value, b: &Value, path: &str) {
    match (a, b) {
        (Value::Number(a), Value::Number(b)) => {
            let a = a.as_f64().unwrap();
            let b = b.as_f64().unwrap();
            assert!((a - b).abs() <= 1e-9_f64.max(b.abs() * 1e-10), "{path}: {a} != {b}");
        }
        (Value::Object(a), Value::Object(b)) => {
            let mut ka: Vec<_> = a.keys().collect();
            let mut kb: Vec<_> = b.keys().collect();
            ka.sort();
            kb.sort();
            assert_eq!(ka, kb, "{path}: keys");
            for (k, v) in a {
                numerically_same(v, &b[k], &format!("{path}.{k}"));
            }
        }
        (Value::Array(a), Value::Array(b)) => {
            assert_eq!(a.len(), b.len(), "{path}: array length");
            for (i, (a, b)) in a.iter().zip(b).enumerate() {
                numerically_same(a, b, &format!("{path}[{i}]"));
            }
        }
        _ => assert_eq!(a, b, "{path}"),
    }
}
#[tokio::test]
async fn capture_media_lifecycle_traces_match_source() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let f = fixture();
            for row in f["workflows"].as_array().unwrap() {
                let scenario = row["scenario"].as_str().unwrap();
                let adapter = Rc::new(FileAdapter::new(scenario, &f));
                let host = Rc::new(McpHost::new(adapter.clone(), McpHostOptions::default()).unwrap());
                let root = adapter.root.path().to_str().unwrap();
                let mut results = vec![];
                let preview = host.live_audio_capture_preview_async(&json!(1), &f["valid"]).await;
                let body: Value = serde_json::from_str(preview["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
                results.push(clean(preview, root));
                let record = host.audio_capture_transactions.get(body["transactionId"].as_str().unwrap()).unwrap();
                record.borrow_mut()["durationMs"] = json!(1);
                if scenario == "epoch" {
                    adapter.epoch.set(2);
                }
                if scenario == "expired" {
                    record.borrow_mut()["expiresAt"] = json!(0);
                }
                for (id, key) in [(2, "capture-key"), (3, "capture-key"), (4, "other-key")] {
                    let args = json!({"transactionId":body["transactionId"],"confirmation":if scenario=="bad-confirmation"{json!("bad")}else{body["confirmation"].clone()},"idempotencyKey":key});
                    results.push(clean(
                        host.live_audio_capture_apply_async(&json!(id), &args, None).await.unwrap_or(Value::Null),
                        root,
                    ));
                }
                results.push(clean(host.live_audio_capture_status_async(&json!(5), Some(&json!({}))).await, root));
                numerically_same(&json!(results), &row["results"], &format!("{scenario} results"));
                numerically_same(&clean(record.borrow().clone(), root), &row["record"], &format!("{scenario} record"));
                same(&json!(*adapter.base.calls.borrow()), &row["calls"], &format!("{scenario} calls"));
                assert_eq!(adapter.media_path().exists(), row["media"].as_bool().unwrap(), "{scenario} raw media");
                assert_eq!(adapter.companion_path().exists(), row["companion"].as_bool().unwrap(), "{scenario} companion");
                same(&clean(adapter.capture.borrow().clone(), root), &row["capture"], &format!("{scenario} capture"));
                same(
                    &clean(adapter.base.snapshot.borrow()["tracks"].as_array().unwrap().last().unwrap().clone(), root),
                    &row["destination"],
                    &format!("{scenario} destination"),
                );
            }
        })
        .await;
}

#[tokio::test]
async fn capture_concurrent_waiters_and_cancellation_match_source() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let f = fixture();
            for row in f["concurrent"].as_array().unwrap() {
                let scenario = row["scenario"].as_str().unwrap();
                let adapter = Rc::new(FileAdapter::new(scenario, &f));
                let host = Rc::new(McpHost::new(adapter.clone(), McpHostOptions::default()).unwrap());
                let root = adapter.root.path().to_str().unwrap();
                let preview = host.live_audio_capture_preview_async(&json!(1), &f["valid"]).await;
                let body: Value = serde_json::from_str(preview["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
                let record = host.audio_capture_transactions.get(body["transactionId"].as_str().unwrap()).unwrap();
                record.borrow_mut()["durationMs"] = json!(50);
                adapter.applying.set(true);
                let args = json!({"transactionId":body["transactionId"],"confirmation":body["confirmation"],"idempotencyKey":"shared-key"});
                let first = Signal::new();
                let second = Signal::new();
                if scenario == "preaborted" {
                    first.cancel();
                }
                let first_call = async { host.live_audio_capture_apply_async(&json!(2), &args, Some(&first)).await };
                let second_call = async {
                    if ["before-status", "preaborted"].contains(&scenario) {
                        None
                    } else {
                        let mut args = args.clone();
                        if scenario == "wrong-key" {
                            args["idempotencyKey"] = json!("other-key");
                        }
                        host.live_audio_capture_apply_async(&json!(3), &args, Some(&second)).await
                    }
                };
                let cancel = async {
                    if ["one-cancelled", "both-cancelled", "before-status"].contains(&scenario) {
                        tokio::time::sleep(Duration::from_millis(5)).await;
                        first.cancel();
                        if scenario == "both-cancelled" {
                            second.cancel();
                        }
                    }
                };
                let (a, b, ()) = tokio::join!(first_call, second_call, cancel);
                if let Some(inflight) = host.record_operation(&record) {
                    inflight.await.unwrap();
                }
                numerically_same(&clean(json!([a, b]), root), &row["results"], &format!("{scenario} concurrent results"));
                numerically_same(&clean(record.borrow().clone(), root), &row["record"], &format!("{scenario} concurrent record"));
                same(&json!(*adapter.base.calls.borrow()), &row["calls"], &format!("{scenario} concurrent calls"));
                same(&clean(adapter.capture.borrow().clone(), root), &row["capture"], &format!("{scenario} concurrent capture"));
                assert_eq!(adapter.media_path().exists(), row["media"].as_bool().unwrap(), "{scenario} raw media");
                assert_eq!(adapter.companion_path().exists(), row["companion"].as_bool().unwrap(), "{scenario} companion");
                assert!(host.capture_controllers.borrow().is_empty(), "{scenario}: completed shared controller retained");
            }
        })
        .await;
}

#[tokio::test]
async fn capture_media_polling_and_deadline_match_source() {
    let f = fixture();
    for row in f["waiting"].as_array().unwrap() {
        let adapter = Adapter::new(&json!({"statuses":row["statuses"]}), &f);
        let host = McpHost::default();
        let result = host
            .wait_for_captured_media(&adapter, None, Some(if row["expired"] == true { 0.0 } else { now_ms_f64() + 1000.0 }))
            .await
            .unwrap_or_else(|e| json!({"error":e.message()}));
        same(&result, &row["result"], &format!("wait {}", row["label"]));
        same(&json!(*adapter.calls.borrow()), &row["calls"], &format!("wait {} calls", row["label"]));
    }
}
