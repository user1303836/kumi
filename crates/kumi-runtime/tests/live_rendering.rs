#[path = "../../../tests/support/chunks.rs"]
mod chunks;
#[path = "../../../tests/support/fixture_paths.rs"]
mod fixture_paths;
use async_trait::async_trait;
use futures::FutureExt;
use kumi_common::{
    abort::{Signal, SignalExt},
    js::json::stringify,
};
use kumi_runtime::ears::link::{Armed, EarsError, EarsLink, Tap, Transport, Written};
use kumi_runtime::{
    core::{contracts::*, errors::RuntimeError},
    integrations::ableton::{
        connection::LiveConnection,
        history::History,
        mutations::Mutations,
        observation::Observer,
        options::{AbletonOptions, EarsSetup},
        parameters::Parameters,
        remember::{CurrentProject, Remember},
        rendering::Rendering,
    },
    mcp::{
        client::{McpEndpoint, StderrStatus},
        types::{CallToolResult, Implementation, ListToolsResult},
    },
};
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    path::{Path, PathBuf},
    rc::Rc,
};
struct Fixture {
    case: Value,
    calls: Cell<usize>,
    original: RefCell<Signal>,
    cancelled: Cell<bool>,
    tag: RefCell<String>,
    releases: RefCell<Vec<Value>>,
    held_apply: Option<Rc<HeldApply>>,
}
struct HeldApply {
    dispatched: tokio::sync::Notify,
    reply: RefCell<Option<tokio::sync::oneshot::Receiver<()>>>,
}
#[async_trait(?Send)]
impl McpEndpoint for Fixture {
    fn pid(&self) -> Option<u32> {
        None
    }
    fn server_info(&self) -> Option<Implementation> {
        Some(
            serde_json::from_value(
                json!({"name":"kumi-synthetic-bridge","version":self.case["config"].get("version").cloned().unwrap_or(json!("1.0.73"))}),
            )
            .unwrap(),
        )
    }
    async fn list(&self, _: Option<&str>, _: Signal) -> Result<ListToolsResult, RuntimeError> {
        Ok(serde_json::from_value(json!({"tools":self.case["tools"]})).unwrap())
    }
    async fn call(&self, name: &str, args: JsonObject, signal: Signal) -> Result<CallToolResult, RuntimeError> {
        signal.check()?;
        if name == "live_transaction_release" {
            self.releases.borrow_mut().push(json!({"name":name,"args":args}));
            return Ok(serde_json::from_value(json!({"content":[],"structuredContent":{}})).unwrap());
        }
        if let Some(name) = args.get("tracks").and_then(Value::as_array).and_then(|v| v.first()).and_then(|v| v["name"].as_str()) {
            if let Some(tag) = name.split(' ').next_back() {
                *self.tag.borrow_mut() = tag.into();
            }
        }
        let index = self.calls.get();
        let actual = json!({"name":name,"args":args});
        let expected = &self.case["calls"][index];
        eq(&actual, expected, &format!("{} dispatch {index}", self.case["label"]));
        self.calls.set(index + 1);
        if self.case["config"]["cancelOn"].as_str() == Some(name) && !self.cancelled.replace(true) {
            if let Some(held) = &self.held_apply {
                let reply = held.reply.borrow_mut().take().expect("one held apply");
                held.dispatched.notify_one();
                reply.await.expect("release the dispatched apply");
            }
            self.original.borrow().cancel();
        }
        let response = &self.case["responses"][index];
        if let Some(message) = response["throw"].as_str() {
            return Err(if message == "cancelled" { RuntimeError::Aborted } else { RuntimeError::plain(message) });
        }
        tokio::task::yield_now().await;
        let body = serde_json::to_string(&response["reply"]).unwrap().replace("<tag>", &self.tag.borrow());
        serde_json::from_str(&body).map_err(|e| RuntimeError::plain(e.to_string()))
    }
    fn on_catalog_changed(&self, _: Rc<dyn Fn()>) -> Box<dyn Fn()> {
        Box::new(|| {})
    }
    fn on_disconnect(&self, _: Rc<dyn Fn()>) -> Box<dyn Fn()> {
        Box::new(|| {})
    }
    fn stderr_status(&self) -> StderrStatus {
        StderrStatus { bytes: 0, truncated: false }
    }
    async fn close(&self) -> Result<(), RuntimeError> {
        Ok(())
    }
}
fn canonical(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<_> = map.keys().collect();
            keys.sort();
            Value::Object(keys.into_iter().map(|key| (key.clone(), canonical(&map[key]))).collect())
        }
        Value::Array(a) => Value::Array(a.iter().map(canonical).collect()),
        other => other.clone(),
    }
}
thread_local! {
    /// The folder this thread's renders go to, which the oracle calls $EARS.
    static EARS: RefCell<PathBuf> = RefCell::new(PathBuf::new());
}
fn normalized(value: &Value) -> String {
    let root = EARS.with(|ears| ears.borrow().clone());
    let value = fixture_paths::map_strings(value, &|text| fixture_paths::normalize_root(text, root.to_str().unwrap(), "$EARS"));
    let mut text = stringify(&canonical(&value));
    for (pattern, replace) in [
        (r"Kumi · render (\d+) [a-f0-9]{4}", "Kumi · render $1 <tag>"),
        (r"Kumi · Goal best [a-f0-9]{3}", "Kumi · Goal best <tag>"),
        (r"[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}", "<uuid>"),
        (r#""a[0-9a-f]{8}""#, r#""<audition>""#),
        (r"\bc\d+\b", "<change>"),
    ] {
        text = regex::Regex::new(pattern).unwrap().replace_all(&text, replace).into_owned();
    }
    text
}
fn eq(actual: &Value, expected: &Value, label: &str) {
    assert_eq!(normalized(actual), normalized(expected), "{label}");
}
fn wav(folder: &Path, kind: &str) {
    let frames = 48000 * 12;
    let mut bytes = vec![0u8; 44 + frames * 4];
    bytes[..4].copy_from_slice(b"RIFF");
    bytes[4..8].copy_from_slice(&((frames * 4 + 36) as u32).to_le_bytes());
    bytes[8..16].copy_from_slice(b"WAVEfmt ");
    bytes[16..20].copy_from_slice(&16u32.to_le_bytes());
    bytes[20..22].copy_from_slice(&1u16.to_le_bytes());
    bytes[22..24].copy_from_slice(&2u16.to_le_bytes());
    bytes[24..28].copy_from_slice(&48000u32.to_le_bytes());
    bytes[28..32].copy_from_slice(&192000u32.to_le_bytes());
    bytes[32..34].copy_from_slice(&4u16.to_le_bytes());
    bytes[34..36].copy_from_slice(&16u16.to_le_bytes());
    bytes[36..40].copy_from_slice(b"data");
    bytes[40..44].copy_from_slice(&((frames * 4) as u32).to_le_bytes());
    let mut seed = 7u32;
    for i in 0..frames {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        let value = match kind {
            "silence" => 0,
            "noise" => (seed % 16001) as i16 - 8000,
            _ => {
                if i % 436 < 218 {
                    8000
                } else {
                    -8000
                }
            }
        };
        bytes[44 + i * 4..46 + i * 4].copy_from_slice(&value.to_le_bytes());
        bytes[46 + i * 4..48 + i * 4].copy_from_slice(&value.to_le_bytes());
    }
    std::fs::write(folder.join(format!("{kind}.wav")), bytes).unwrap();
}
/// The oracle's cases in 16 tests that nextest runs side by side.
mod source_audition_goal_passes_and_failure_cleanup_match {
    crate::chunks::chunked!(super::replay_cases; part_00 = 0, part_01 = 1, part_02 = 2, part_03 = 3, part_04 = 4, part_05 = 5,
        part_06 = 6, part_07 = 7, part_08 = 8, part_09 = 9, part_10 = 10, part_11 = 11, part_12 = 12, part_13 = 13, part_14 = 14,
        part_15 = 15);
}
/// The oracle's cases at index `chunk`, `chunk + chunks`, …, on a runtime whose clock starts paused.
fn replay_cases(chunk: usize, chunks: usize) {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().start_paused(true).build().unwrap();
    runtime.block_on(tokio::task::LocalSet::new().run_until(replay(chunk, chunks)));
}
async fn replay(chunk: usize, chunks: usize) {
    // Rendering keeps its renders in kumi-ears/<the generation's first 8 characters> in the temporary folder, and
    // closing removes that folder. Each chunk has a generation of its own, so chunks running side by side keep theirs.
    // No result in the oracle carries the generation.
    let generation = format!("chunk-{chunk:02}");
    EARS.with(|ears| *ears.borrow_mut() = std::env::temp_dir().join("kumi-ears").join(&generation));
    let folder = tempfile::tempdir().unwrap();
    for kind in ["square", "noise", "silence"] {
        wav(folder.path(), kind);
    }
    let source: Value = serde_json::from_str(include_str!("support/rendering-oracle.json")).unwrap();
    let fixture = fixture_paths::map_strings(&source, &|text| text.replace("$AUDIO", folder.path().to_str().unwrap()));
    for (case_index, case) in fixture["cases"].as_array().unwrap().iter().enumerate().skip(chunk).step_by(chunks) {
        if std::env::var("KUMI_RENDERING_CASE").is_ok_and(|label| case["label"].as_str() != Some(&label)) {
            continue;
        }
        eprintln!("rendering case {case_index}: {}", case["label"]);
        let config = &case["config"];
        let endpoint = Rc::new(Fixture {
            case: case.clone(),
            calls: Cell::new(0),
            original: RefCell::new(Signal::new()),
            cancelled: Cell::new(false),
            tag: RefCell::new("tag".into()),
            releases: RefCell::new(vec![]),
            held_apply: None,
        });
        let events = Rc::new(RefCell::new(vec![]));
        let actions = Rc::new(RefCell::new(vec![]));
        let auditions = Rc::new(RefCell::new(vec![]));
        let mut options = AbletonOptions::new(Rc::new(|_, _| {}));
        let out = endpoint.clone();
        options.connect = Some(Rc::new(move |_| {
            let out: Rc<dyn McpEndpoint> = out.clone();
            async move { Ok(out) }.boxed_local()
        }));
        options.now = Some(Rc::new(|| chrono::DateTime::from_timestamp_millis(0).unwrap()));
        options.generation = Some(generation.clone());
        options.change_timeout_ms = Some(2000);
        options.fast = Some(false);
        options.ears = Some(EarsSetup::Disabled);
        options.low_disk = Some(Rc::new(|_, _, _| async { None }.boxed_local()));
        let ears = Rc::new(EarsReplay {
            calls: case["earsCalls"].as_array().cloned().unwrap_or_default(),
            at: Cell::new(0),
            folder: folder.path().into(),
        });
        // Ears that can't be set up (no Max for Live, say): each try is counted.
        let opened = Rc::new(Cell::new(0));
        if config["ears"] == "unopenable" {
            let opens = opened.clone();
            options.ears = Some(EarsSetup::Open(Rc::new(move || {
                opens.set(opens.get() + 1);
                async { Err::<Rc<dyn EarsLink>, _>(RuntimeError::plain("Max for Live isn't in this Live")) }.boxed_local()
            })));
        } else if config.get("ears").is_some() {
            let link = ears.clone();
            options.ears = Some(EarsSetup::Open(Rc::new(move || {
                let link: Rc<dyn EarsLink> = link.clone();
                async move { Ok(link) }.boxed_local()
            })));
        }
        let restore = folder.path().join("restore.json");
        let _ = std::fs::remove_file(&restore);
        options.restore_file = Some(restore.to_string_lossy().into_owned());
        if let Some(pending) = config.get("pending") {
            std::fs::write(&restore, stringify(pending)).unwrap();
        }
        let out = events.clone();
        options.on_change = Some(Rc::new(move |event| out.borrow_mut().push(json!(event))));
        let out = actions.clone();
        options.on_action = Some(Rc::new(move |event| out.borrow_mut().push(json!(event))));
        let out = auditions.clone();
        options.on_audition = Some(Rc::new(move |event| {
            let mut event = json!(event);
            event.as_object_mut().unwrap().insert("type".into(), json!("auditioned"));
            out.borrow_mut().push(event);
        }));
        let options = Rc::new(options);
        let connection = LiveConnection::new(options.connection_options());
        connection.start(Signal::new()).await.unwrap();
        connection.tools().unwrap().refresh(Signal::new()).await.unwrap();
        connection.available.set(config["available"] != false);
        connection.lost.set(config["lost"] == true);
        connection.epoch.set((config["noEpoch"] != true).then_some(7.));
        *connection.set.borrow_mut() = Some("fixture".into());
        let remember = Remember::new(connection.clone(), None, None);
        *remember.current.borrow_mut() = Some(Rc::new(CurrentProject {
            identity: "fixture".into(),
            path: config["path"].as_str().map(str::to_owned),
            name: "Fixture Set".into(),
        }));
        let history = Rc::new(History::new(connection.clone(), remember.clone(), options.change_timeout_ms, options.on_change.clone()));
        let observer = Rc::new(Observer::new(connection.clone(), remember));
        observer.tempo.set((config["noTempo"] != true).then_some(120.));
        let mutations = Rc::new(Mutations::new(Rc::new(Parameters::new(history.clone(), options.fast)), observer.clone(), options.clone()));
        let step = mutations.clone();
        let files = mutations.clone();
        let rendering = Rendering::new(
            history.clone(),
            observer,
            &options,
            Rc::new(move |tool, args, signal| {
                let step = step.clone();
                async move { step.step(&tool, args, signal).await }.boxed_local()
            }),
            Rc::new(move |file, signal| {
                let files = files.clone();
                async move { files.clip_file(&file, signal).await }.boxed_local()
            }),
        );
        let mut goal: Option<Rc<dyn GoalRig>> = None;
        for (index, op) in case["operations"].as_array().unwrap().iter().enumerate() {
            let signal = Signal::new();
            if op["abort"] == true {
                signal.cancel();
            }
            *endpoint.original.borrow_mut() = signal.clone();
            let result:Result<Value,RuntimeError>=async{Ok(match op["type"].as_str().unwrap(){
    "reset"=>{rendering.reset_turn(op["continuing"].as_bool().unwrap_or(false));Value::Null},
    "bump"=>{connection.lease.set(connection.lease.get()+1);Value::Null},
    "audition"=>match rendering.audition(&serde_json::from_value(op["request"].clone()).unwrap(),signal).await?{Ok(mut value)=>{value.seconds=0.;json!(value)},Err(message)=>json!(message)},
    "hear"=>match rendering.hear_in_set(&serde_json::from_value(op["request"].clone()).unwrap(),signal).await?{Ok(value)=>json!(value),Err(message)=>json!(message)},
    "restore"=>json!(rendering.restore_after_crash(op["identity"].as_str().unwrap(),op["path"].as_str(),signal).await?),
    "open"=>match rendering.open_goal(&serde_json::from_value(op["request"].clone()).unwrap(),signal).await?{Ok(value)=>{let result=json!({"slots":value.slots(),"screens":value.screens()});goal=Some(value);result},Err(message)=>json!(message)},
    "generation"=>{let trials=op["trials"].as_array().unwrap().iter().map(|trial|GenerationTrial{slot:trial["slot"].as_str().unwrap().into(),knobs:serde_json::from_value(trial["knobs"].clone()).unwrap(),values:serde_json::from_value(trial["values"].clone()).unwrap(),fresh:trial["fresh"].as_bool()}).collect::<Vec<_>>();let v=goal.as_ref().unwrap().generation(&trials,signal,Some(GenerationOptions{screen:op["options"]["screen"].as_bool()})).await?;json!({"scores":v.scores,"gaps":v.gaps,"silent":v.silent,"frozen":v.frozen,"structural":v.structural,"screened":v.screened,"cached":v.cached})},
    "add"=>match goal.as_ref().unwrap().add(&serde_json::from_value(op["candidate"].clone()).unwrap(),signal).await?{Ok(value)=>json!(value),Err(error)=>json!(error)},
    "settle"=>json!(goal.as_ref().unwrap().settle(op["slot"].as_str().unwrap(),&serde_json::from_value::<Vec<_>>(op["knobs"].clone()).unwrap(),&serde_json::from_value::<Vec<_>>(op["values"].clone()).unwrap(),signal).await?),
    "keep"=>json!(goal.as_ref().unwrap().keep_best(op["slot"].as_str().unwrap(),&serde_json::from_value::<Vec<_>>(op["knobs"].clone()).unwrap(),&serde_json::from_value::<Vec<_>>(op["values"].clone()).unwrap(),signal).await?),
    "tidy"=>json!(goal.as_ref().unwrap().tidy(&serde_json::from_value::<Vec<_>>(op.get("top").cloned().unwrap_or(json!([]))).unwrap(),signal).await?),
    "close"=>json!(goal.as_ref().unwrap().close().await?),other=>panic!("unknown {other}")})}.await;
            let value = match result {
                Ok(value) => value,
                Err(RuntimeError::Aborted) => json!({"error":"cancelled"}),
                Err(error) => json!({"error":error.to_string()}),
            };
            let label = format!("{} operation {index}", case["label"]);
            eq(&value, &case["results"][index]["value"], &label);
            for _ in 0..4 {
                tokio::task::yield_now().await;
            }
            let changes: Vec<_> = history.entries.borrow().values().map(|entry| json!(entry.borrow().record)).collect();
            eq(&json!(changes), &case["results"][index]["state"]["changes"], &format!("{label} retained changes"));
            assert_eq!(rendering.round_count(), case["results"][index]["state"]["round"].as_u64().unwrap() as usize);
            let restore: Value = std::fs::read_to_string(&restore).ok().map(|s| serde_json::from_str(&s).unwrap()).unwrap_or(Value::Null);
            eq(&restore, &case["results"][index]["restore"], &format!("{label} crash journal"));
        }
        assert_eq!(endpoint.calls.get(), case["calls"].as_array().unwrap().len(), "{} call count", case["label"]);
        eq(&json!(*events.borrow()), &case["events"], &format!("{} history", case["label"]));
        eq(&json!(*actions.borrow()), &case["actions"], &format!("{} actions", case["label"]));
        eq(&json!(*auditions.borrow()), &case["auditions"], &format!("{} auditions", case["label"]));
        eq(&json!(*endpoint.releases.borrow()), &case["releases"], &format!("{} transaction releases", case["label"]));
        if config["ears"] == "unopenable" {
            assert_eq!(opened.get(), 1, "a listening device that couldn't be set up isn't tried again at each audition");
        }
        rendering.close().await;
        connection.close().await.unwrap();
        assert_eq!(ears.at.get(), ears.calls.len(), "{} Ears call count", case["label"]);
    }
}

struct EarsReplay {
    calls: Vec<Value>,
    at: Cell<usize>,
    folder: std::path::PathBuf,
}
impl EarsReplay {
    fn next(&self, method: &str, args: Value) -> Value {
        let index = self.at.get();
        let call = &self.calls[index];
        assert_eq!(call["method"], method, "Ears call {index}");
        eq(&args, &call["args"], &format!("Ears {method} {index}"));
        self.at.set(index + 1);
        call.clone()
    }
    fn capture(&self, file: &str, recipe: &Value) {
        let pcm = std::fs::read(self.folder.join(format!("{}.wav", recipe["kind"].as_str().unwrap()))).unwrap();
        let samples = (pcm.len() - 44) / 4;
        let stopped = if recipe["playing"] == true { 0 } else { 2000 };
        let jump = recipe["jump"].as_f64();
        let before = if jump.is_some() { 18000 } else { 0 };
        let length = stopped + before + samples;
        let mut bytes = Vec::with_capacity(length * 16);
        let beat = |i: usize| {
            if i < stopped {
                None
            } else if i < stopped + before {
                Some(200.25 + (i - stopped) as f64 / 24000.)
            } else {
                Some(jump.unwrap_or(recipe["position"].as_f64().unwrap()) + (i - stopped - before) as f64 / 24000.)
            }
        };
        for i in 0..length {
            let sample = if i < stopped + before {
                0.
            } else {
                let offset = 44 + (i - stopped - before) * 4;
                i16::from_le_bytes([pcm[offset], pcm[offset + 1]]) as f32 / 32768.
            };
            let phase = beat(i).map(|beat| 1. + beat % 1.).unwrap_or(1.) as f32;
            let position = beat(i.saturating_sub(400)).unwrap_or(0.) as f32;
            for value in [sample, sample, phase, position] {
                bytes.extend(value.to_be_bytes());
            }
        }
        std::fs::write(file, bytes).unwrap();
    }
}
#[async_trait(?Send)]
impl EarsLink for EarsReplay {
    fn port(&self) -> u16 {
        47299
    }
    fn taps(&self) -> Vec<Tap> {
        serde_json::from_value(self.next("taps", json!([]))["result"].clone()).unwrap()
    }
    async fn wait_for(&self, accept: Rc<dyn for<'a> Fn(&'a Tap) -> bool>, timeout: u64, _: Option<Signal>) -> Option<Tap> {
        let call = self.next("waitFor", json!([timeout]));
        let tap: Option<Tap> = serde_json::from_value(call["result"].clone()).unwrap();
        if let Some(tap) = &tap {
            assert!(accept(tap), "Ears predicate must accept the source tap");
        }
        tap
    }
    async fn arm(&self, tap: &Tap, seconds: f64, _: Option<Signal>) -> Result<Armed, EarsError> {
        let call = self.next("arm", json!([tap, seconds]));
        tokio::task::yield_now().await;
        Ok(serde_json::from_value(call["result"].clone()).unwrap())
    }
    async fn write(&self, tap: &Tap, file: &str, _: Option<Signal>) -> Result<Written, EarsError> {
        let call = self.next("write", json!([tap, file]));
        self.capture(file, &call["capture"]);
        tokio::task::yield_now().await;
        let mut result: Written = serde_json::from_value(call["result"].clone()).unwrap();
        result.file = file.into();
        Ok(result)
    }
    fn stop(&self, tap: &Tap) {
        self.next("stop", json!([tap]));
    }
    async fn ping(&self, tap: &Tap, _: Option<Signal>) -> Option<Tap> {
        Some(tap.clone())
    }
    async fn transport(&self, tap: &Tap, _: Option<Signal>) -> Result<Option<Transport>, EarsError> {
        let call = self.next("transport", json!([tap]));
        tokio::task::yield_now().await;
        Ok(serde_json::from_value(call["result"].clone()).unwrap())
    }
    async fn close(&self) {
        self.next("close", json!([]));
    }
}

/// The source cleanup oracle, reached through the real kernel and public audition tool.
#[tokio::test(flavor = "current_thread")]
async fn cancelled_kernel_audition_restores_main_and_removes_scratch_tracks_after_close() {
    use futures::StreamExt;
    use kumi_runtime::{
        ai::{
            error::LanguageModelError,
            types::{CallOptions, FinishReason, FinishReasonUnified, StreamPart, StreamParts, ToolCall},
        },
        integrations::ableton::{integration::Ableton, observation::ObservationHost},
        kernel::agent::{create_agent_kernel, AgentKernelOptions, LanguageModel, ModelBinding},
    };
    use std::time::Duration;
    struct Model {
        input: String,
    }
    #[async_trait(?Send)]
    impl LanguageModel for Model {
        async fn do_stream(&self, _: CallOptions) -> Result<StreamParts, LanguageModelError> {
            Ok(futures::stream::iter(vec![
                StreamPart::ToolCall(ToolCall {
                    tool_call_id: "audition-call".into(),
                    tool_name: "audition".into(),
                    input: self.input.clone(),
                    provider_executed: None,
                    dynamic: None,
                    provider_metadata: None,
                }),
                StreamPart::Finish {
                    usage: Default::default(),
                    finish_reason: FinishReason { unified: FinishReasonUnified::ToolCalls, raw: None },
                    provider_metadata: None,
                },
            ])
            .boxed_local())
        }
    }
    tokio::task::LocalSet::new()
        .run_until(async {
            let folder = tempfile::tempdir().unwrap();
            wav(folder.path(), "square");
            let source: Value = serde_json::from_str(include_str!("support/rendering-oracle.json")).unwrap();
            let source_case = source["cases"].as_array().unwrap().iter().find(|case| case["label"] == "cancel-live_mixer_apply").unwrap();
            let case = fixture_paths::map_strings(source_case, &|text| text.replace("$AUDIO", folder.path().to_str().unwrap()));
            let (acknowledge, reply) = tokio::sync::oneshot::channel();
            let held = Rc::new(HeldApply { dispatched: tokio::sync::Notify::new(), reply: RefCell::new(Some(reply)) });
            let controller = Signal::new();
            let endpoint = Rc::new(Fixture {
                case: case.clone(),
                calls: Cell::new(0),
                original: RefCell::new(controller.clone()),
                cancelled: Cell::new(false),
                tag: RefCell::new("tag".into()),
                releases: RefCell::new(vec![]),
                held_apply: Some(held.clone()),
            });
            let finished = Rc::new(tokio::sync::Notify::new());
            let completed = finished.clone();
            let mut options = AbletonOptions::new(Rc::new(|_, _| {}));
            let connect = endpoint.clone();
            options.connect = Some(Rc::new(move |_| {
                let connect = connect.clone();
                async move { Ok(connect as Rc<dyn McpEndpoint>) }.boxed_local()
            }));
            options.ears = Some(EarsSetup::Disabled);
            options.fast = Some(false);
            options.low_disk = Some(Rc::new(|_, _, _| async { None }.boxed_local()));
            let restore = folder.path().join("restore.json");
            options.restore_file = Some(restore.to_string_lossy().into_owned());
            options.on_action = Some(Rc::new(move |event| {
                if event.title == "Auditioned" {
                    completed.notify_one();
                }
            }));
            let integration = Ableton::new(options);
            integration.start(Signal::new()).await.unwrap();
            integration.connection.tools().unwrap().refresh(Signal::new()).await.unwrap();
            integration.connection.epoch.set(Some(7.0));
            *integration.connection.set.borrow_mut() = Some("fixture".into());
            *integration.history.remember.current.borrow_mut() =
                Some(Rc::new(CurrentProject { identity: "fixture".into(), path: None, name: "Fixture Set".into() }));
            integration.observer.tempo.set(Some(120.0));
            let audition = integration.definitions().into_iter().find(|tool| tool.name() == "audition").unwrap();
            let mut input = case["operations"][0]["request"].as_object().unwrap().clone();
            let from = input.remove("fromBeat").unwrap();
            input.insert("from_beat".into(), from);
            let kernel = Rc::new(
                create_agent_kernel(AgentKernelOptions {
                    conversation: None,
                    instructions: "fixture".into(),
                    tools: vec![audition],
                    signal: Signal::new(),
                    checkpoint: None,
                    max_steps: None,
                    budget: None,
                    binding: ModelBinding {
                        id: "test/fixture".into(),
                        model: Rc::new(Model { input: stringify(&json!(input)) }),
                        budget: None,
                        prepare: Box::new(|request| CallOptions {
                            prompt: request.messages,
                            tools: Some(request.tools),
                            ..Default::default()
                        }),
                    },
                })
                .unwrap(),
            );
            let events = Rc::new(RefCell::new(vec![]));
            let sink = events.clone();
            let active_kernel = kernel.clone();
            let run_signal = controller.clone();
            let running = tokio::task::spawn_local(async move {
                active_kernel
                    .run(
                        "audition",
                        run_signal,
                        Rc::new(move |event| {
                            sink.borrow_mut().push(event);
                            Ok(())
                        }),
                    )
                    .await
            });
            tokio::time::timeout(Duration::from_secs(2), held.dispatched.notified()).await.unwrap();
            assert!(restore.is_file(), "Main restoration is journaled before muting it");
            controller.cancel();
            let result = tokio::time::timeout(Duration::from_secs(1), running).await.unwrap().unwrap().unwrap();
            assert_eq!(result.stop_reason, StopReason::Cancelled);
            tokio::time::timeout(Duration::from_secs(1), kernel.close()).await.unwrap();
            drop(kernel);
            assert!(acknowledge.send(()).is_ok(), "cancelling the kernel dropped audition before its cleanup");
            tokio::time::timeout(Duration::from_secs(2), finished.notified()).await.unwrap();
            assert_eq!(endpoint.calls.get(), case["calls"].as_array().unwrap().len(), "all source restore/undo/transport calls finish");
            assert!(!restore.exists(), "confirmed Main restoration clears the crash journal");
            assert!(integration.history.entries.borrow().is_empty(), "scratch changes are removed after cleanup");
            assert!(matches!(events.borrow().as_slice(), [KernelEvent::ToolStart { .. }]), "late cleanup must not deliver a model result");
            integration.close().await.unwrap();
        })
        .await;
}
