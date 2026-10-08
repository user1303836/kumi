use async_trait::async_trait;
use futures::FutureExt;
use kumi_common::{
    abort::{Signal, SignalExt},
    js::json::stringify,
};
use kumi_runtime::{
    core::{contracts::*, errors::RuntimeError},
    hands::*,
    integrations::ableton::{
        command_tools::{CommandTools, CommandToolsOptions},
        connection::{ConnectionOptions, LiveConnection},
        history::History,
        options::HandsSetup,
        remember::Remember,
    },
    mcp::{
        client::{McpEndpoint, StderrStatus},
        types::{CallToolResult, ListToolsResult},
    },
};
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
};
struct Fixture {
    config: Value,
    changed: Cell<bool>,
    calls: RefCell<Vec<Value>>,
    hands_calls: RefCell<Vec<Value>>,
    counts: RefCell<HashMap<String, usize>>,
    /// What Kumi told the producer ({"told": …}) and kept in HISTORY ({"record": …}), in order.
    told: RefCell<Vec<Value>>,
}
fn wrap(value: Value) -> CallToolResult {
    serde_json::from_value(if value.is_object() {
        json!({"content":[{"type":"text","text":stringify(&value)}],"structuredContent":value})
    } else {
        json!({"content":[{"type":"text","text":stringify(&value)}]})
    })
    .unwrap()
}
impl Fixture {
    fn hand(&self, method: &str, args: Vec<Value>) -> Result<Value, HandsError> {
        let mut call = vec![json!(method)];
        call.extend(args);
        self.hands_calls.borrow_mut().push(json!(call));
        let mut counts = self.counts.borrow_mut();
        let count = counts.entry(method.into()).or_default();
        let configured = self.config["hands"][method].as_array().and_then(|v| v.get(*count).or_else(|| v.last()));
        *count += 1;
        if let Some(error) = configured.and_then(|v| v.get("throw")).and_then(Value::as_str) {
            return Err(HandsError::new(error, HandsErrorKind::Failed));
        }
        // Live's tracks change once a menu item or keys are pressed, or once the method configured is called.
        if self.config["changesOn"].as_str().map_or(method == "menu" || method == "keys", |on| on == method) {
            self.changed.set(true);
        }
        Ok(configured.cloned().unwrap_or_else(|| match method {
            "trusted" => json!(true),
            "menus" => json!([{"path":["Edit","Freeze Track"],"enabled":true,"key":"f","modifiers":1048576},{"path":["Edit","Unfreeze Track"],"enabled":true},{"path":["Edit","Flatten Track"],"enabled":true},{"path":["Edit","Group Tracks"],"enabled":true},{"path":["Edit","Ungroup Tracks"],"enabled":true},{"path":["File","Save Live Set"],"enabled":true},{"path":["Edit","Duplicate"],"enabled":true},{"path":["Create","Consolidate"],"enabled":true}]),
            "dialog" => json!({"open":false}),
            _ => json!({"ok":true}),
        }))
    }
}
#[async_trait(?Send)]
impl Hands for Fixture {
    async fn trusted(&self, prompt: bool) -> Result<bool, HandsError> {
        Ok(self.hand("trusted", vec![json!(prompt)])?.as_bool().unwrap())
    }
    async fn menus(&self, _: Option<Signal>) -> Result<Vec<MenuItem>, HandsError> {
        Ok(serde_json::from_value(self.hand("menus", vec![])?).unwrap())
    }
    async fn tracks(&self, tracks: &[Track], _: Option<Signal>) -> Result<HandsReply, HandsError> {
        Ok(serde_json::from_value(self.hand("tracks", vec![json!(tracks)])?).unwrap())
    }
    async fn menu(&self, path: &[String], options: MenuOptions) -> Result<HandsReply, HandsError> {
        let options = if options.titles.is_empty() { json!({}) } else { json!({"titles":options.titles}) };
        Ok(serde_json::from_value(self.hand("menu", vec![json!(path), options])?).unwrap())
    }
    async fn keys(&self, keys: &[String], _: KeysOptions) -> Result<HandsReply, HandsError> {
        Ok(serde_json::from_value(self.hand("keys", vec![json!(keys), json!({})])?).unwrap())
    }
    async fn dialog(&self, _: Option<Signal>) -> Result<Dialog, HandsError> {
        Ok(serde_json::from_value(self.hand("dialog", vec![])?).unwrap())
    }
    async fn answer(&self, answer: &str, _: Option<Signal>) -> Result<HandsReply, HandsError> {
        Ok(serde_json::from_value(self.hand("answer", vec![json!(answer)])?).unwrap())
    }
    async fn toggles(&self, set: &[ToggleSet], _: Option<Signal>) -> Result<HandsReply, HandsError> {
        Ok(serde_json::from_value(self.hand("toggles", vec![json!(set)])?).unwrap())
    }
    async fn windows(&self, _: Option<Signal>) -> Result<Vec<Window>, HandsError> {
        unreachable!()
    }
    async fn file(&self, path: &str, kind: &str, _: Option<Signal>) -> Result<HandsReply, HandsError> {
        // As Live does once its Save dialog is filled: the Set is written there, or, in a folder that isn't a
        // project, in a project folder Live makes beside it ("<name> Project", seen in Live 12.4).
        if self.config["writes"] != false && kind == "save" {
            let path = std::path::Path::new(path);
            if self.config["project"] == true {
                let project = path.parent().unwrap().join(format!("{} Project", path.file_stem().unwrap().to_string_lossy()));
                std::fs::create_dir_all(&project).unwrap();
                std::fs::write(project.join(path.file_name().unwrap()), b"Set").unwrap();
            } else {
                std::fs::write(path, b"Set").unwrap();
            }
        }
        Ok(serde_json::from_value(self.hand("file", vec![json!(path), json!(kind)])?).unwrap())
    }
    fn close(&self) {}
}
#[async_trait(?Send)]
impl McpEndpoint for Fixture {
    fn pid(&self) -> Option<u32> {
        None
    }
    fn server_info(&self) -> Option<kumi_runtime::mcp::types::Implementation> {
        Some(serde_json::from_value(json!({"name":"fixture","version":"1.0.73"})).unwrap())
    }
    async fn list(&self, _: Option<&str>, _: Signal) -> Result<ListToolsResult, RuntimeError> {
        Ok(serde_json::from_value(json!({"tools":(["live_status","live_discover","live_device_read"].iter().map(|name|json!({"name":name,"inputSchema":{"type":"object"}})).collect::<Vec<_>>())})).unwrap())
    }
    async fn call(&self, name: &str, args: JsonObject, signal: Signal) -> Result<CallToolResult, RuntimeError> {
        signal.check()?;
        self.calls.borrow_mut().push(json!({"name":name,"args":args}));
        if let Some(error) = self.config["throwRead"].as_str() {
            return Err(RuntimeError::plain(error));
        }
        // Live's status, as configured in turn (the last one again after).
        if name == "live_status" {
            if let Some(all) = self.config["status"].as_array() {
                let mut counts = self.counts.borrow_mut();
                let count = counts.entry("live_status".into()).or_default();
                let status = all.get(*count).or_else(|| all.last()).cloned().unwrap();
                *count += 1;
                return Ok(wrap(status));
            }
            return Ok(wrap(json!({"connected":true,"adapter":"remote-script","epoch":7})));
        }
        if name == "live_device_read" {
            return Ok(self
                .config
                .get("deviceRead")
                .map(|v| serde_json::from_value(v.clone()).unwrap())
                .unwrap_or_else(|| wrap(json!({"names":self.config.get("names").cloned().unwrap_or(json!(["Cutoff","Drive"]))}))));
        }
        if args.get("kind") == Some(&json!("device")) {
            return Ok(self
                .config
                .get("deviceDiscovery")
                .map(|v| serde_json::from_value(v.clone()).unwrap())
                .unwrap_or_else(|| wrap(json!({"items":[{"ref":"7:device:0:0","name":self.config.get("deviceName").cloned().unwrap_or(json!("Unknown Synth"))}]}))));
        }
        if args.get("kind") == Some(&json!("parameter")) {
            return Ok(self
                .config
                .get("parameterRead")
                .map(|v| serde_json::from_value(v.clone()).unwrap())
                .unwrap_or_else(|| wrap(json!({"items":self.config.get("parameters").cloned().unwrap_or(json!([{"ref":"7:parameter:0:0:0","name":"Device On"},{"ref":"7:parameter:0:0:1","name":"Cutoff","displayValue":"1 kHz"}]))}))));
        }
        let tracks = self.config.get("tracks").cloned().unwrap_or(json!([{"ref":"7:track:0","name":"Bass","kind":"midi","isFrozen":false,"isVisible":true},{"ref":"7:track:1","name":"Drums","kind":"audio","isFrozen":true,"isVisible":true}]));
        let tracks = if self.changed.get() { self.config.get("after").cloned().unwrap_or(tracks) } else { tracks };
        Ok(wrap(
            json!({"epoch":7,"kind":args["kind"],"items":if args["kind"]=="track"{tracks}else{json!([])},"revision":"r1","truncated":false}),
        ))
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
fn canonical(v: &Value) -> Value {
    match v {
        Value::Object(o) => {
            let mut keys = o.keys().collect::<Vec<_>>();
            keys.sort();
            Value::Object(keys.into_iter().map(|k| (k.clone(), canonical(&o[k]))).collect())
        }
        Value::Array(v) => json!(v.iter().map(canonical).collect::<Vec<_>>()),
        _ => v.clone(),
    }
}
fn eq(a: Value, b: &Value, label: &str) {
    assert_eq!(stringify(&canonical(&a)), stringify(&canonical(b)), "{label}");
}
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn command_and_plugin_traces_match_typescript() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let fixture: Value = serde_json::from_str(include_str!("support/command-tools-oracle.json")).unwrap();
            for case in fixture["cases"].as_array().unwrap() {
                let label = case["label"].as_str().unwrap();
                let config = &case["config"];
                let endpoint = Rc::new(Fixture {
                    config: config.clone(),
                    changed: Cell::new(false),
                    calls: RefCell::new(vec![]),
                    hands_calls: RefCell::new(vec![]),
                    counts: RefCell::new(HashMap::new()),
                    told: RefCell::new(vec![]),
                });
                let connected = endpoint.clone();
                let mut options = ConnectionOptions::new(Rc::new(|_, _| {}));
                options.connect = Some(Rc::new(move |_| {
                    let ep: Rc<dyn McpEndpoint> = connected.clone();
                    async move { Ok(ep) }.boxed_local()
                }));
                options.now =
                    Some(Rc::new(|| chrono::DateTime::parse_from_rfc3339("2026-10-03T12:00:00Z").unwrap().with_timezone(&chrono::Utc)));
                options.generation = Some("connection".into());
                let library = tempfile::tempdir().unwrap();
                let connection = LiveConnection::new(options);
                connection.start(Signal::new()).await.unwrap();
                connection.tools().unwrap().refresh(Signal::new()).await.unwrap();
                connection.available.set(config["available"] != false);
                connection.lost.set(config["lost"] == true);
                connection.epoch.set(if config["noEpoch"] == true { None } else { Some(7.0) });
                if let Some(refs) = config["refs"].as_array() {
                    for r in refs {
                        connection.references.borrow_mut().refs.insert(r[0].as_str().unwrap().into(), r[1].as_str().unwrap().into());
                    }
                }
                if let Some(shorts) = config["shorts"].as_array() {
                    for r in shorts {
                        connection.references.borrow_mut().short_ref(r.as_str().unwrap());
                    }
                }
                let events = Rc::new(RefCell::new(vec![]));
                let out = events.clone();
                let remember = Remember::new(connection.clone(), None, None);
                let history = Rc::new(History::new(
                    connection.clone(),
                    remember.clone(),
                    None,
                    Some(Rc::new(move |r| {
                        let mut r = serde_json::to_value(r).unwrap();
                        r["id"] = json!("<id>");
                        out.borrow_mut().push(r);
                    })),
                ));
                let actions = Rc::new(RefCell::new(vec![]));
                let out = actions.clone();
                let hands = endpoint.clone();
                let no_hands = config["noHands"] == true;
                let hands = if config["disabled"] == true {
                    HandsSetup::Disabled
                } else {
                    HandsSetup::Open(Rc::new(move || {
                        let hands: Rc<dyn Hands> = hands.clone();
                        async move { Ok(if no_hands { None } else { Some(hands) }) }.boxed_local()
                    }))
                };
                let act_calls = Rc::new(RefCell::new(vec![]));
                let out_act = act_calls.clone();
                let act_throw = config["actThrow"].as_str().map(str::to_owned);
                let observation_throw = config["observationThrow"] == true;
                let act_reply = config.get("act").cloned().unwrap_or(json!({"text":"selected","isError":false}));
                let commands = CommandTools::new(
                    connection.clone(),
                    history.clone(),
                    remember,
                    CommandToolsOptions {
                        user_library: Some(library.path().to_string_lossy().into()),
                        hands: Some(hands),
                        on_action: Some(Rc::new(move |a| out.borrow_mut().push(json!(a)))),
                        ..Default::default()
                    },
                    Rc::new(move |name, args, _| {
                        out_act.borrow_mut().push(json!([name,args]));
                        let result = act_reply.clone();
                        let error = act_throw.clone();
                        async move {
                            if let Some(error) = error {
                                return Err(if observation_throw { RuntimeError::Observation(error) } else { RuntimeError::plain(error) });
                            }
                            Ok(if result["isError"] == true {
                                ToolResult::error(result["text"].as_str().unwrap())
                            } else {
                                ToolResult::text(result["text"].as_str().unwrap())
                            })
                        }
                        .boxed_local()
                    }),
                );
                let mut results = Vec::new();
                for op in case["ops"].as_array().unwrap() {
                    let signal = Signal::new();
                    if op["abort"] == true {
                        signal.cancel();
                    }
                    let input = op["input"].as_object().unwrap();
                    let result = if op["plugin"] == true {
                        commands.plugin_tool(input, signal).await
                    } else {
                        commands.live_command(input, signal).await
                    };
                    let result = match result {
                        Ok(r) => json!({"text":r.text,"isError":r.is_error}),
                        Err(RuntimeError::Aborted) => json!({"error":"cancelled"}),
                        Err(e) => json!({"error":e.to_string()}),
                    };
                    let refs = connection.references.borrow();
                    results.push(json!({"result":result,"state":{"epoch":connection.epoch.get(),"lease":connection.lease.get(),"refs":refs.refs.iter().collect::<Vec<_>>(),"known":refs.known.iter().collect::<Vec<_>>(),"cursors":refs.cursors.iter().collect::<Vec<_>>()}}));
                }
                connection.close().await.unwrap();
                // JSON objects in text are compared structurally; all plain messages stay exact.
                let mut expected = case["results"].clone();
                let mut actual = json!(results);
                for list in [&mut expected, &mut actual] {
                    for r in list.as_array_mut().unwrap() {
                        if let Some(s) = r["result"]["text"].as_str() {
                            if let Ok(v) = serde_json::from_str::<Value>(s) {
                                r["result"]["text"] = v;
                            }
                        }
                    }
                }
                normalize_paths(&mut actual, library.path().to_str().unwrap());
                if !cfg!(target_os="macos") && (config["disabled"]==true || config["noHands"]==true) { for item in expected.as_array_mut().unwrap() { item["result"]["text"]=json!("Kumi can\'t use Live\'s menus on this computer."); } }
                eq(actual, &expected, &format!("{label} results"));
                eq(json!(*endpoint.calls.borrow()), &case["calls"], &format!("{label} dispatches"));
                eq(json!(*endpoint.hands_calls.borrow()), &case["handsCalls"], &format!("{label} hands"));
                eq(json!(*act_calls.borrow()), &case["actCalls"], &format!("{label} acts"));
                eq(json!(*actions.borrow()), &case["actions"], &format!("{label} actions"));
                eq(json!(*events.borrow()), &case["events"], &format!("{label} history"));
                assert!(history.entries.borrow().is_empty());
                let folder = library.path().join("Kumi/Wavetables");
                let mut files = if folder.exists() {
                    std::fs::read_dir(&folder).unwrap().map(|entry| entry.unwrap().path()).collect::<Vec<_>>()
                } else {
                    vec![]
                };
                files.sort();
                use sha2::{Digest, Sha256};
                let files = files.iter().map(|path| json!({"name":path.file_name().unwrap().to_str().unwrap(),"sha256":hex::encode(Sha256::digest(std::fs::read(path).unwrap()))})).collect::<Vec<_>>();
                // The source preserves the CPU's sign bit for Infinity * sin(0)'s NaN.
                // Compare whole-file hashes with the recorded source on the same CPU;
                // every finite WAV continues to use its original shared byte oracle.
                let architecture = match std::env::consts::ARCH {
                    "aarch64" => "arm64",
                    "x86_64" => "x64",
                    other => other,
                };
                let expected_files = if case.get("filesByArchitecture").is_some() && fixture["filesArchitecture"] != architecture {
                    case["filesByArchitecture"].get(architecture).expect("record the exact source WAV fixture for this architecture")
                } else {
                    &case["files"]
                };
                eq(json!(files), expected_files, &format!("{label} files"));
            }
        })
        .await;
}

fn normalize_paths(value: &mut Value, library: &str) {
    match value {
        Value::String(text) if text.starts_with(library) => *text = format!("<library>{}", text[library.len()..].replace('\\', "/")),
        Value::Array(values) => {
            for value in values {
                normalize_paths(value, library)
            }
        }
        Value::Object(values) => {
            for value in values.values_mut() {
                normalize_paths(value, library)
            }
        }
        _ => {}
    }
}

/// CommandTools over a Fixture, for one case's config: what live_command answers, the hands calls it made,
/// and the connection (to play Live dropping Kumi while a Set opens).
async fn set_file_harness(config: Value) -> (CommandTools, Rc<Fixture>, Rc<LiveConnection>) {
    set_file_harness_with(config, |_| {}).await
}
async fn set_file_harness_with(
    config: Value,
    adjust: impl FnOnce(&mut ConnectionOptions),
) -> (CommandTools, Rc<Fixture>, Rc<LiveConnection>) {
    let endpoint = Rc::new(Fixture {
        config,
        changed: Cell::new(false),
        calls: RefCell::new(vec![]),
        hands_calls: RefCell::new(vec![]),
        counts: RefCell::new(HashMap::new()),
        told: RefCell::new(vec![]),
    });
    let connected = endpoint.clone();
    let mut options = ConnectionOptions::new(Rc::new(|_, _| {}));
    options.connect = Some(Rc::new(move |_| {
        let ep: Rc<dyn McpEndpoint> = connected.clone();
        async move { Ok(ep) }.boxed_local()
    }));
    options.generation = Some("connection".into());
    adjust(&mut options);
    let connection = LiveConnection::new(options);
    connection.start(Signal::new()).await.unwrap();
    connection.tools().unwrap().refresh(Signal::new()).await.unwrap();
    connection.available.set(true);
    connection.lost.set(false);
    connection.epoch.set(Some(7.0));
    let remember = Remember::new(connection.clone(), None, None);
    let kept = endpoint.clone();
    let history = Rc::new(History::new(
        connection.clone(),
        remember.clone(),
        None,
        Some(Rc::new(move |r| kept.told.borrow_mut().push(json!({"record":serde_json::to_value(r).unwrap()["title"]})))),
    ));
    let hands = endpoint.clone();
    let told = endpoint.clone();
    let commands = CommandTools::new(
        connection.clone(),
        history,
        remember,
        CommandToolsOptions {
            hands: Some(HandsSetup::Open(Rc::new(move || {
                let hands: Rc<dyn Hands> = hands.clone();
                async move { Ok(Some(hands)) }.boxed_local()
            }))),
            on_action: Some(Rc::new(move |a| told.told.borrow_mut().push(json!({"told":a.title})))),
            ..Default::default()
        },
        Rc::new(|_, _, _| async { Ok(ToolResult::text("selected")) }.boxed_local()),
    );
    (commands, endpoint, connection)
}
const FILE_MENU: &str = r#"[{"path":["File","New Live Set"],"enabled":true,"key":"Ctrl+N"},{"path":["File","Open Live Set"],"enabled":true},{"path":["File","Save Live Set"],"enabled":true},{"path":["File","Save Live Set As"],"enabled":true}]"#;

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn a_set_is_saved_as_a_file_started_new_or_opened_through_lives_file_menu() {
    // #189: "save this set as CHAOS 02 in the project folder" took 96 s through PowerShell, and "start a new
    // Set" became a hand-forged .als Live called corrupt.
    tokio::task::LocalSet::new()
        .run_until(async {
            let folder = tempfile::tempdir().unwrap();
            let menus: Value = serde_json::from_str(FILE_MENU).unwrap();
            let saving = json!({"open":true,"title":"Save Live Set As","words":[],"buttons":["Save","Cancel"],"file":"save"});

            if cfg!(target_os = "macos") {
                // On a Mac, Kumi's hands can't fill Live's file dialogs yet: nothing is pressed.
                let (commands, hands, _) = set_file_harness(json!({"hands":{"menus":[menus]}})).await;
                let song = folder.path().join("Song.als");
                std::fs::write(&song, b"Set").unwrap();
                for input in [
                    json!({"command":"save_as","path":folder.path().join("CHAOS 02")}),
                    json!({"command":"open_set","path":song}),
                    json!({"command":"save","path":folder.path().join("CHAOS 02")}),
                ] {
                    let result = commands.live_command(input.as_object().unwrap(), Signal::new()).await.unwrap();
                    assert!(result.is_error && result.text.starts_with("On a Mac, Kumi can't fill in Live's Save and Open dialogs yet"), "{input}: {}", result.text);
                }
                assert!(!hands.hands_calls.borrow().iter().any(|c| c[0] == "menu"));
            } else {
                // Save as: the Save dialog is filled and the Set is written there, .als added.
                let (commands, hands, _) = set_file_harness(json!({"hands":{"menus":[menus],"dialog":[saving,{"open":false}]}})).await;
                let path = folder.path().join("CHAOS 02");
                let result = commands.live_command(&json!({"command":"save_as","path":path}).as_object().unwrap().clone(), Signal::new()).await.unwrap();
                assert!(!result.is_error, "{}", result.text);
                let saved: Value = serde_json::from_str(&result.text).unwrap();
                assert_eq!(saved["pressed"], "File › Save Live Set As");
                assert_eq!(saved["saved"], folder.path().join("CHAOS 02.als").display().to_string());
                assert!(hands.hands_calls.borrow().iter().any(|c| {
                    c[0] == "file" && c[1] == folder.path().join("CHAOS 02.als").display().to_string() && c[2] == "save"
                }));
            }

            // A new Set with unsaved changes: Live's prompt is answered as asked (No discards them on Windows),
            // and the answer comes once Live is back with the new Set.
            let prompt = json!({"open":true,"title":"Ableton Live","words":["Save changes to \"Untitled\" before closing?"],"buttons":["Yes","No","Cancel"]});
            let (commands, hands, connection) =
                set_file_harness(json!({"hands":{"menus":[menus],"dialog":[prompt,{"open":false}],"answer":[{"ok":true,"pressed":"No"}]}})).await;
            let switching = connection.clone();
            let watched = hands.clone();
            tokio::task::spawn_local(async move {
                while !watched.hands_calls.borrow().iter().any(|c| c[0] == "answer") {
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
                switching.lost.set(true);
                tokio::time::sleep(std::time::Duration::from_millis(3000)).await;
                switching.lost.set(false);
            });
            let result =
                commands.live_command(&json!({"command":"new_set","save_current":"no"}).as_object().unwrap().clone(), Signal::new()).await.unwrap();
            assert!(!result.is_error, "{}", result.text);
            let opened: Value = serde_json::from_str(&result.text).unwrap();
            assert_eq!(opened["opened"], "a new Set");
            assert!(opened["note"].as_str().unwrap().contains("References from before are gone"));
            assert!(hands.hands_calls.borrow().iter().any(|c| c[0] == "answer" && c[1] == "No"));

            // Unsaved changes and no word on them: the prompt comes back, for the producer to decide.
            let (commands, hands, _) = set_file_harness(json!({"hands":{"menus":[menus],"dialog":[prompt]}})).await;
            let result = commands.live_command(&json!({"command":"new_set"}).as_object().unwrap().clone(), Signal::new()).await.unwrap();
            let asked: Value = serde_json::from_str(&result.text).unwrap();
            assert_eq!(asked["dialog"]["buttons"], json!(["Yes", "No", "Cancel"]));
            assert!(asked["next"].as_str().unwrap().starts_with("Live asks whether to save the open Set first: answer Cancel, ask the producer"));
            assert!(!hands.hands_calls.borrow().iter().any(|c| c[0] == "answer"));

            // Paths that can't work are refused before anything is pressed.
            let (commands, hands, _) = set_file_harness(json!({"hands":{"menus":[menus]}})).await;
            for (input, said) in [
                (json!({"command":"open_set","path":folder.path().join("nowhere.als")}), "There's no Set at"),
                (json!({"command":"save_as"}), "save_as needs path"),
                (json!({"command":"save_as","path":"CHAOS 03.als"}), "path is a full path"),
                (json!({"command":"save_as","path":folder.path().join("missing").join("x.als")}), "doesn't exist"),
            ] {
                let result = commands.live_command(input.as_object().unwrap(), Signal::new()).await.unwrap();
                assert!(result.is_error && result.text.contains(said), "{input}: {}", result.text);
            }
            assert!(!hands.hands_calls.borrow().iter().any(|c| c[0] == "menu"));
        })
        .await;
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn a_path_goes_only_into_the_dialog_it_is_for_and_replacing_a_file_is_the_producers_call() {
    // On a Mac these are refused before anything is pressed (the test above).
    if cfg!(target_os = "macos") {
        return;
    }
    tokio::task::LocalSet::new()
        .run_until(async {
            let folder = tempfile::tempdir().unwrap();
            let menus: Value = serde_json::from_str(FILE_MENU).unwrap();
            let song = folder.path().join("Song B.als");
            std::fs::write(&song, b"the producer's song").unwrap();
            let prompt = json!({"open":true,"title":"Ableton Live","words":["Save changes to \"Untitled\" before closing?"],"buttons":["Yes","No","Cancel"]});
            let saving = json!({"open":true,"title":"Save Live Set As","words":[],"buttons":["Save","Cancel"],"file":"save"});
            let opening = json!({"open":true,"title":"Open Live Set","words":[],"buttons":["Open","Cancel"],"file":"open"});
            let calls = |hands: &Fixture, method: &str| hands.hands_calls.borrow().iter().filter(|c| c[0] == method).cloned().collect::<Vec<_>>();

            // The open Set was never saved and the producer said to keep it: Live's Save dialog for it comes
            // up before the Open dialog. The path of the Set to open never goes into it, where Save would
            // write "Untitled" over song B.
            let (commands, hands, _) = set_file_harness(json!({"writes":false,"hands":{"menus":[menus],"dialog":[prompt,saving],"answer":[{"ok":true,"pressed":"Yes"}]}})).await;
            let result = commands
                .live_command(&json!({"command":"open_set","path":song,"save_current":"yes"}).as_object().unwrap().clone(), Signal::new())
                .await
                .unwrap();
            let said: Value = serde_json::from_str(&result.text).unwrap();
            assert!(said["next"].as_str().unwrap().contains("never saved"), "{}", result.text);
            assert_eq!(calls(&hands, "answer"), vec![json!(["answer", "Yes"])]);
            assert!(calls(&hands, "file").is_empty(), "{:?}", hands.hands_calls.borrow());
            assert_eq!(std::fs::read(&song).unwrap(), b"the producer's song");

            // The Open dialog takes it, as the dialog to open with.
            let (commands, hands, connection) = set_file_harness(json!({"writes":false,"hands":{"menus":[menus],"dialog":[opening,{"open":false}]}})).await;
            let switching = connection.clone();
            let watched = hands.clone();
            tokio::task::spawn_local(async move {
                while !watched.hands_calls.borrow().iter().any(|c| c[0] == "file") {
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
                switching.lost.set(true);
                tokio::time::sleep(std::time::Duration::from_millis(3000)).await;
                switching.lost.set(false);
            });
            let result = commands.live_command(&json!({"command":"open_set","path":song}).as_object().unwrap().clone(), Signal::new()).await.unwrap();
            assert!(!result.is_error, "{}", result.text);
            assert_eq!(serde_json::from_str::<Value>(&result.text).unwrap()["openedSet"], song.display().to_string());
            assert_eq!(calls(&hands, "file"), vec![json!(["file", song.display().to_string(), "open"])]);

            // A Mac's prompt has its own buttons: discarding is Don't Save there.
            let mac = json!({"open":true,"title":"","words":["Do you want to save the changes to \"Untitled\" before closing?"],"buttons":["Save","Don’t Save","Cancel"]});
            let (commands, hands, _) = set_file_harness(json!({"hands":{"menus":[menus],"dialog":[mac,{"open":false}]}})).await;
            let _ = commands.live_command(&json!({"command":"new_set","save_current":"no"}).as_object().unwrap().clone(), Signal::new()).await.unwrap();
            assert_eq!(calls(&hands, "answer"), vec![json!(["answer", "Don’t Save"])]);

            // Saving over a file that's there: Windows asks, and that's the producer's to answer.
            let replace = json!({"open":true,"title":"Confirm Save As","words":["Song B.als already exists.","Do you want to replace it?"],"buttons":["Yes","No"]});
            let (commands, hands, _) = set_file_harness(json!({"writes":false,"hands":{"menus":[menus],"dialog":[saving,replace]}})).await;
            let result = commands.live_command(&json!({"command":"save_as","path":song}).as_object().unwrap().clone(), Signal::new()).await.unwrap();
            let said: Value = serde_json::from_str(&result.text).unwrap();
            assert!(said["next"].as_str().unwrap().starts_with(&format!("A file is already at {}: ask the producer.", song.display())), "{}", result.text);
            assert!(calls(&hands, "answer").is_empty());
            assert_eq!(std::fs::read(&song).unwrap(), b"the producer's song");
        })
        .await;
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn lives_own_dialogs_order_folders_and_progress_and_a_switch_seen_through_the_bridge() {
    // What Live 12.4 on Windows did when it was tried (#189, round 7).
    if cfg!(target_os = "macos") {
        return;
    }
    tokio::task::LocalSet::new()
        .run_until(async {
            let folder = tempfile::tempdir().unwrap();
            let menus: Value = serde_json::from_str(FILE_MENU).unwrap();
            let song = folder.path().join("Song B.als");
            std::fs::write(&song, b"the producer's song").unwrap();
            let prompt = json!({"open":true,"title":"","words":["Save changes to \"Untitled\" before closing?"],"buttons":["Yes","No","Cancel"]});
            let saving = json!({"open":true,"title":"Save Live Set As:","words":["File name:","Save as type:"],"buttons":["Save","Cancel"],"file":"save"});
            let opening = json!({"open":true,"title":"Open Live Set","words":[],"buttons":["Open","Cancel"],"file":"open"});
            let calls = |hands: &Fixture, method: &str| hands.hands_calls.borrow().iter().filter(|c| c[0] == method).cloned().collect::<Vec<_>>();

            // Live's own order opening a Set over a never-saved one: its Open dialog takes the path, then it asks to
            // save, and Yes brings the Save dialog for the open Set. Nothing is typed there.
            let (commands, hands, _) =
                set_file_harness(json!({"writes":false,"hands":{"menus":[menus],"dialog":[opening,prompt,saving],"answer":[{"ok":true,"pressed":"Yes"}]}})).await;
            let result = commands
                .live_command(&json!({"command":"open_set","path":song,"save_current":"yes"}).as_object().unwrap().clone(), Signal::new())
                .await
                .unwrap();
            let said: Value = serde_json::from_str(&result.text).unwrap();
            assert!(said["next"].as_str().unwrap().contains("never saved"), "{}", result.text);
            assert_eq!(calls(&hands, "file"), vec![json!(["file", song.display().to_string(), "open"])]);

            // Save as into a folder that isn't a project: Live makes "<name> Project" there, and says where it went.
            let (commands, _, _) = set_file_harness(json!({"project":true,"hands":{"menus":[menus],"dialog":[saving,{"open":false}]}})).await;
            let result =
                commands.live_command(&json!({"command":"save_as","path":folder.path().join("Probe")}).as_object().unwrap().clone(), Signal::new()).await.unwrap();
            let saved: Value = serde_json::from_str(&result.text).unwrap();
            assert_eq!(saved["saved"], folder.path().join("Probe Project").join("Probe.als").display().to_string(), "{}", result.text);
            assert!(saved["note"].as_str().unwrap().contains("project folder"), "{}", result.text);

            // A dialog read before its controls are up is read again; a progress window is waited out.
            let blank = json!({"open":true,"title":"","words":[],"buttons":[]});
            let progress = json!({"open":true,"title":"Saving...","words":[],"buttons":["Cancel"]});
            let (commands, hands, _) = set_file_harness(json!({"hands":{"menus":[menus],"dialog":[blank,saving,progress,{"open":false}]}})).await;
            let result =
                commands.live_command(&json!({"command":"save_as","path":folder.path().join("Two")}).as_object().unwrap().clone(), Signal::new()).await.unwrap();
            assert_eq!(serde_json::from_str::<Value>(&result.text).unwrap()["saved"], folder.path().join("Two.als").display().to_string(), "{}", result.text);
            assert_eq!(calls(&hands, "file").len(), 1);

            // No focus feed watching (plain lines, a script): the switch is seen through the bridge's own status.
            let (commands, _, _) = set_file_harness(json!({
                "status":[{"connected":false,"adapter":"remote-script","epoch":7},{"connected":true,"adapter":"remote-script","epoch":8}],
                "hands":{"menus":[menus],"dialog":[prompt,{"open":false}],"answer":[{"ok":true,"pressed":"No"}]}
            }))
            .await;
            let result =
                commands.live_command(&json!({"command":"new_set","save_current":"no"}).as_object().unwrap().clone(), Signal::new()).await.unwrap();
            assert_eq!(serde_json::from_str::<Value>(&result.text).unwrap()["opened"], "a new Set", "{}", result.text);
        })
        .await;
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn a_switch_the_models_answer_finishes_stays_kumis_and_one_cancelled_there_doesnt() {
    // Round 7 (review): new_set handed back Windows' replace prompt; the model asked the producer and answered
    // Yes, and Live went on to the new Set, which then read as one the producer opened by hand.
    if cfg!(target_os = "macos") {
        return;
    }
    tokio::task::LocalSet::new()
        .run_until(async {
            let folder = tempfile::tempdir().unwrap();
            let menus: Value = serde_json::from_str(FILE_MENU).unwrap();
            let taken = folder.path().join("Song B.als");
            std::fs::write(&taken, b"the producer's song").unwrap();
            let prompt = json!({"open":true,"title":"","words":["Save changes to \"Untitled\" before closing?"],"buttons":["Yes","No","Cancel"]});
            let saving = json!({"open":true,"title":"Save Live Set As:","words":["File name:","Save as type:"],"buttons":["Save","Cancel"],"file":"save"});
            // A German Windows: its replace prompt is known as a two-button question once the path was a file.
            let replace = json!({"open":true,"title":"Speichern unter bestätigen","words":["Song B.als ist bereits vorhanden."],"buttons":["Ja","Nein"]});
            for (answer, cause) in [("Ja", DisconnectCause::AskedSet), ("Cancel", DisconnectCause::Set)] {
                let causes = Rc::new(RefCell::new(vec![]));
                let seen = causes.clone();
                let (commands, _, connection) = set_file_harness_with(
                    json!({"writes":false,"hands":{"menus":[menus],"dialog":[prompt,saving,replace],"answer":[{"ok":true,"pressed":"Yes"},{"ok":true,"pressed":answer}]}}),
                    move |options| {
                        options.on_connection = Rc::new(move |_, cause| seen.borrow_mut().push(cause));
                        options.live_running = Some(Rc::new(|| async { true }.boxed_local()));
                    },
                )
                .await;
                let result = commands
                    .live_command(&json!({"command":"new_set","path":taken,"save_current":"yes"}).as_object().unwrap().clone(), Signal::new())
                    .await
                    .unwrap();
                let said: Value = serde_json::from_str(&result.text).unwrap();
                assert!(said["next"].as_str().unwrap().starts_with("A file is already at"), "{}", result.text);
                commands.live_command(&json!({"answer":answer}).as_object().unwrap().clone(), Signal::new()).await.unwrap();
                connection.lose_live();
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                assert_eq!(causes.borrow().last(), Some(&Some(cause)), "after {answer}");
            }
        })
        .await;
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn a_switch_cancelled_at_lives_prompt_is_no_longer_expected() {
    // Round 7: the 120 s expectation outlived a cancel, so a Set the producer opened by hand right after
    // counted as Kumi's and the request carried on into it (#188).
    tokio::task::LocalSet::new()
        .run_until(async {
            let menus: Value = serde_json::from_str(FILE_MENU).unwrap();
            let prompt = json!({"open":true,"title":"Ableton Live","words":["Save changes to \"Untitled\" before closing?"],"buttons":["Yes","No","Cancel"]});
            let causes = Rc::new(RefCell::new(vec![]));
            let seen = causes.clone();
            let (commands, _, connection) = set_file_harness_with(json!({"hands":{"menus":[menus],"dialog":[prompt]}}), move |options| {
                options.on_connection = Rc::new(move |_, cause| seen.borrow_mut().push(cause));
                options.live_running = Some(Rc::new(|| async { true }.boxed_local()));
            })
            .await;
            let result =
                commands.live_command(&json!({"command":"new_set","save_current":"cancel"}).as_object().unwrap().clone(), Signal::new()).await.unwrap();
            assert_eq!(serde_json::from_str::<Value>(&result.text).unwrap()["cancelled"], true, "{}", result.text);
            connection.lose_live();
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            assert_eq!(causes.borrow().last(), Some(&Some(DisconnectCause::Set)), "a Set opened by hand, not one Kumi asked for");
        })
        .await;
}

/// Live 12.4's Separate Stems dialog as Kumi's hands read it on Windows: a toggle for each stem, Merge Stems
/// (greyed out unless two or three stems are on) and Quality Mode (on for High Quality). Live remembers the
/// choices from the last time.
fn stems_dialog(on: [bool; 4], merge: (bool, bool), high: bool) -> Value {
    let mut toggles: Vec<Value> = ["Vocals", "Drums", "Bass", "Others"]
        .iter()
        .zip(on)
        .map(|(name, on)| json!({"name":format!("{name}, Include or exclude the {name} stem."),"id":format!("{name}CheckControl"),"on":on}))
        .collect();
    toggles.push(json!({"name":"Merge Stems","id":"MergeStems.MergeStemsCheckControl","on":merge.0,"enabled":merge.1}));
    toggles.push(json!({"name":"Quality Mode, Choose between High Speed or High Quality.","id":"HighQualityCheckControl","on":high}));
    json!({"open":true,"title":"Separate Stems","words":["Merge to Single Track","High Speed","High Quality","This action will stop audio."],"buttons":["Separate","Cancel"],"toggles":toggles})
}
const CREATE_MENU: &str = r#"[{"path":["Create","Separate Stems to New Audio Tracks"],"enabled":true},{"path":["Create","Slice to New MIDI Track"],"enabled":true}]"#;
/// Live's progress window while it separates: only Cancel, which Kumi waits out.
fn separating() -> Value {
    json!({"open":true,"title":"","words":["Separating Stems: Break...","40 %"],"buttons":["Cancel"]})
}
fn break_tracks() -> (Value, Value) {
    (
        json!([{"ref":"7:track:0","name":"Break","kind":"audio","isFrozen":false,"isVisible":true}]),
        json!([{"ref":"7:track:0","name":"Break","kind":"audio"},{"ref":"7:track:1","name":"Break Stems","kind":"group"},{"ref":"7:track:2","name":"Vocals Break","kind":"audio"},{"ref":"7:track:3","name":"Drums Break","kind":"audio"}]),
    )
}
fn calls(hands: &Fixture, method: &str) -> Vec<Value> {
    hands.hands_calls.borrow().iter().filter(|c| c[0] == method).cloned().collect()
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn separate_stems_picks_the_stems_in_lives_dialog_presses_separate_and_waits_for_the_tracks() {
    // Live asks which stems with toggles Kumi's hands didn't read: the model saw Separate and Cancel, words
    // with no state, and no way to pick a stem.
    tokio::task::LocalSet::new()
        .run_until(async {
            let menus: Value = serde_json::from_str(CREATE_MENU).unwrap();
            let (tracks, after) = break_tracks();
            // Live had Vocals only at High Quality from the last time.
            let asked = stems_dialog([true, false, false, false], (false, false), true);
            let set = stems_dialog([true, true, false, false], (false, true), false)["toggles"].clone();
            let (commands, hands, _) = set_file_harness(json!({
                "tracks":tracks,"after":after,
                "hands":{"menus":[menus],"dialog":[asked,separating(),{"open":false}],"toggles":[{"ok":true,"toggles":set,"missing":[],"refused":[]}],"answer":[{"ok":true,"pressed":"Separate"}]}
            }))
            .await;
            let input = json!({"command":"separate_stems","clip":"selected","stems":["drums","vocals"],"quality":"high speed"});
            let result = commands.live_command(input.as_object().unwrap(), Signal::new()).await.unwrap();
            assert!(!result.is_error, "{}", result.text);
            let said: Value = serde_json::from_str(&result.text).unwrap();
            assert_eq!(said["pressed"], "Create › Separate Stems to New Audio Tracks");
            assert_eq!(said["newTracks"], json!(["Break Stems", "Vocals Break", "Drums Break"]), "{said}");
            assert_eq!((&said["stems"], &said["quality"], &said["answered"]), (&json!(["vocals", "drums"]), &json!("high speed"), &json!("Separate")));
            assert!(said.get("dialog").is_none() && said.get("merged").is_none(), "{said}");
            // The stems asked for go on before the others go off (Live keeps one on); Merge Stems off; the quality.
            let set = &calls(&hands, "toggles")[0][1];
            let set: Vec<_> = set.as_array().unwrap().iter().map(|t| (t["id"].as_str().unwrap(), t["on"].as_bool().unwrap())).collect();
            assert_eq!(
                set,
                [
                    ("VocalsCheckControl", true),
                    ("DrumsCheckControl", true),
                    ("BassCheckControl", false),
                    ("OthersCheckControl", false),
                    ("MergeStems.MergeStemsCheckControl", false),
                    ("HighQualityCheckControl", false)
                ]
            );
            let order: Vec<_> = hands.hands_calls.borrow().iter().map(|c| c[0].clone()).filter(|m| m == "menu" || m == "toggles" || m == "answer").collect();
            assert_eq!(order, [json!("menu"), json!("toggles"), json!("answer")]);
            assert_eq!(calls(&hands, "answer"), [json!(["answer", "Separate"])]);
            assert_eq!(
                *hands.told.borrow(),
                [
                    json!({"told":"Separating stems: vocals, drums"}),
                    json!({"told":"Separated stems the selected clip"}),
                    json!({"record":"Separated stems the selected clip"})
                ]
            );

            // All four when none are named, Merge Stems left alone (Live takes it with two or three only), and the
            // quality as the producer last left it: here High Quality.
            let (tracks, after) = break_tracks();
            let all = stems_dialog([true; 4], (true, false), true);
            let (commands, hands, _) = set_file_harness(json!({
                "tracks":tracks,"after":after,
                "hands":{"menus":[menus],"dialog":[stems_dialog([true, false, false, false], (true, false), true),separating(),{"open":false}],"toggles":[{"ok":true,"toggles":all["toggles"],"missing":[],"refused":[]}],"answer":[{"ok":true,"pressed":"Separate"}]}
            }))
            .await;
            let result = commands.live_command(json!({"command":"separate_stems","clip":"selected"}).as_object().unwrap(), Signal::new()).await.unwrap();
            let said: Value = serde_json::from_str(&result.text).unwrap();
            assert_eq!((&said["stems"], &said["quality"]), (&json!(["vocals", "drums", "bass", "others"]), &json!("high quality")), "{said}");
            assert!(said.get("merged").is_none(), "greyed out, Merge Stems doesn't merge: {said}");
            let set = &calls(&hands, "toggles")[0][1];
            assert_eq!(set.as_array().unwrap().len(), 4, "{set}");

            // Two stems merged into one track.
            let (tracks, after) = break_tracks();
            let merged = stems_dialog([false, true, true, false], (true, true), true);
            let (commands, _, _) = set_file_harness(json!({
                "tracks":tracks,"after":after,
                "hands":{"menus":[menus],"dialog":[stems_dialog([true, false, false, false], (false, false), true),separating(),{"open":false}],"toggles":[{"ok":true,"toggles":merged["toggles"],"missing":[],"refused":[]}],"answer":[{"ok":true,"pressed":"Separate"}]}
            }))
            .await;
            let input = json!({"command":"separate_stems","clip":"selected","stems":["bass","drums"],"merge":true});
            let said: Value = serde_json::from_str(&commands.live_command(input.as_object().unwrap(), Signal::new()).await.unwrap().text).unwrap();
            assert_eq!((&said["stems"], &said["merged"]), (&json!(["drums", "bass"]), &json!(true)), "{said}");
        })
        .await;
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn what_live_couldnt_separate_is_refused_before_anything_is_pressed() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let menus: Value = serde_json::from_str(CREATE_MENU).unwrap();
            let (commands, hands, _) = set_file_harness(json!({"hands":{"menus":[menus]}})).await;
            for (input, said) in [
                (
                    json!({"command":"separate_stems","clip":"selected","stems":["vocals","drums","bass","others"],"merge":true}),
                    "Live merges two or three stems",
                ),
                (json!({"command":"separate_stems","clip":"selected","stems":["vocals"],"merge":true}), "with 1 leave merge out"),
                (json!({"command":"separate_stems","clip":"selected","stems":["guitar"]}), "not “guitar”"),
                (json!({"command":"separate_stems","clip":"selected","stems":[]}), "at least one stem"),
                (json!({"command":"separate_stems","clip":"selected","quality":"best"}), "quality is high quality or high speed"),
                (json!({"command":"group_tracks","tracks":["A","B"],"stems":["drums"]}), "are for separate_stems"),
                (json!({"command":"separate_stems","clip":"selected","toggles":{"Vocals":"yes"}}), "true (on) or false (off) for Vocals"),
            ] {
                let result = commands.live_command(input.as_object().unwrap(), Signal::new()).await.unwrap();
                assert!(result.is_error && result.text.contains(said), "{input}: {}", result.text);
            }
            assert!(calls(&hands, "menu").is_empty() && calls(&hands, "toggles").is_empty());
        })
        .await;
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn a_dialog_kumi_cant_fill_comes_back_with_its_toggles_and_the_models_answer_finishes_the_command() {
    // Live in another language, its toggles without ids (a Mac, say): Kumi can't tell which is Vocals, so the
    // dialog comes back as it is, and the model's toggles and answer go into it.
    tokio::task::LocalSet::new()
        .run_until(async {
            let menus: Value = serde_json::from_str(CREATE_MENU).unwrap();
            let (tracks, after) = break_tracks();
            let german = json!({"open":true,"title":"Stems trennen","words":[],"buttons":["Trennen","Abbrechen"],"toggles":[
                {"name":"Gesang, Gesang-Stem ein- oder ausschließen.","on":true},{"name":"Schlagzeug","on":false},{"name":"Bass","on":false},
                {"name":"Andere","on":false},{"name":"Stems zusammenführen","on":false,"enabled":false},{"name":"Qualitätsmodus","on":true}]});
            let set = json!([{"name":"Gesang, Gesang-Stem ein- oder ausschließen.","on":false},{"name":"Schlagzeug","on":true},{"name":"Bass","on":false},
                {"name":"Andere","on":false},{"name":"Stems zusammenführen","on":false,"enabled":false},{"name":"Qualitätsmodus","on":true}]);
            let (commands, hands, _) = set_file_harness(json!({
                "tracks":tracks,"after":after,"changesOn":"answer",
                "hands":{"menus":[menus],"dialog":[german,german,german,german,separating(),{"open":false}],"toggles":[{"ok":true,"toggles":set,"missing":[],"refused":[]}],"answer":[{"ok":true,"pressed":"Trennen"}]}
            }))
            .await;
            let result = commands.live_command(json!({"command":"separate_stems","clip":"selected","stems":["drums"]}).as_object().unwrap(), Signal::new()).await.unwrap();
            assert!(!result.is_error, "{}", result.text);
            let said: Value = serde_json::from_str(&result.text).unwrap();
            assert!(said["next"].as_str().unwrap().starts_with("Live's dialog has no toggle “Drums”; its toggles: Gesang, Schlagzeug, Bass, Andere, Stems zusammenführen, Qualitätsmodus. Nothing in it was pressed. Set its toggles by name with toggles"), "{said}");
            // Each toggle by its name, on or off, what it does when Live says, and false for one greyed out.
            assert_eq!(said["dialog"]["toggles"][0], json!({"name":"Gesang","on":true,"about":"Gesang-Stem ein- oder ausschließen."}));
            assert_eq!(said["dialog"]["toggles"][4], json!({"name":"Stems zusammenführen","on":false,"enabled":false}));
            assert!(said.get("newTracks").is_none() && calls(&hands, "toggles").is_empty() && calls(&hands, "answer").is_empty());
            // Nothing separated yet: not said, and not in HISTORY.
            assert!(hands.told.borrow().is_empty(), "{:?}", hands.told.borrow());

            // The model sets them and presses Trennen in one call: Kumi waits for Live's work and says what it made.
            let input = json!({"toggles":{"Schlagzeug":true,"Gesang":false},"answer":"Trennen"});
            let result = commands.live_command(input.as_object().unwrap(), Signal::new()).await.unwrap();
            assert!(!result.is_error, "{}", result.text);
            let said: Value = serde_json::from_str(&result.text).unwrap();
            assert_eq!(said["pressed"], "Trennen");
            assert_eq!(said["newTracks"], json!(["Break Stems", "Vocals Break", "Drums Break"]), "{said}");
            assert!(said["note"].as_str().unwrap().contains("References from before are gone"));
            let set = &calls(&hands, "toggles")[0][1];
            assert_eq!(set, &json!([{"name":"Schlagzeug","on":true},{"name":"Gesang, Gesang-Stem ein- oder ausschließen.","on":false}]));
            assert_eq!(
                *hands.told.borrow(),
                [
                    json!({"told":"Set Schlagzeug on, Gesang off in Live's dialog"}),
                    json!({"told":"Pressed Trennen in Live's dialog"}),
                    json!({"told":"Separated stems the selected clip"}),
                    json!({"record":"Separated stems the selected clip"})
                ]
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn toggles_on_their_own_set_the_open_dialog_and_say_what_live_wouldnt_change() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let open = stems_dialog([true, false, false, false], (false, false), true);
            let set = stems_dialog([true, true, false, false], (false, true), true);
            let (commands, hands, _) =
                set_file_harness(json!({"hands":{"dialog":[open.clone()],"toggles":[{"ok":true,"toggles":set["toggles"],"missing":[],"refused":[]}]}})).await;
            let result = commands.live_command(json!({"toggles":{"drums":true}}).as_object().unwrap(), Signal::new()).await.unwrap();
            assert!(!result.is_error, "{}", result.text);
            let said: Value = serde_json::from_str(&result.text).unwrap();
            assert_eq!(said["dialog"]["toggles"][1], json!({"name":"Drums","on":true,"about":"Include or exclude the Drums stem."}));
            assert_eq!(calls(&hands, "toggles"), [json!(["toggles", [{"name":"Drums, Include or exclude the Drums stem.","id":"DrumsCheckControl","on":true}]])]);
            assert!(calls(&hands, "answer").is_empty(), "nothing pressed");

            // A toggle the dialog doesn't have, and one Live has greyed out: said, and nothing pressed.
            let (commands, hands, _) =
                set_file_harness(json!({"hands":{"dialog":[open.clone()],"toggles":[{"ok":true,"toggles":open["toggles"],"missing":[],"refused":["Merge Stems"]}]}})).await;
            let result = commands.live_command(json!({"toggles":{"Guitar":true},"answer":"Separate"}).as_object().unwrap(), Signal::new()).await.unwrap();
            assert_eq!(result.text, "Live's dialog has no toggle “Guitar”; its toggles: Vocals, Drums, Bass, Others, Merge Stems, Quality Mode.");
            let result = commands.live_command(json!({"toggles":{"Merge Stems":true},"answer":"Separate"}).as_object().unwrap(), Signal::new()).await.unwrap();
            assert_eq!((result.is_error, result.text.as_str()), (true, "Live has “Merge Stems” greyed out right now, so it stayed off."));
            assert!(calls(&hands, "answer").is_empty(), "nothing pressed");

            // A button Live has greyed out (Separate with no stem on) isn't pressed, and it's said.
            let (commands, hands, _) = set_file_harness(json!({"hands":{"answer":[{"ok":false,"error":"disabled","title":"Separate"}]}})).await;
            let result = commands.live_command(json!({"answer":"Separate"}).as_object().unwrap(), Signal::new()).await.unwrap();
            assert_eq!((result.is_error, result.text.as_str()), (true, "Live has “Separate” greyed out right now, so nothing was pressed."));
            assert!(calls(&hands, "dialog").is_empty());

            // No dialog open.
            let (commands, _, _) = set_file_harness(json!({})).await;
            let result = commands.live_command(json!({"toggles":{"Drums":true}}).as_object().unwrap(), Signal::new()).await.unwrap();
            assert_eq!((result.is_error, result.text.as_str()), (true, "Live has no dialog open."));
        })
        .await;
}
