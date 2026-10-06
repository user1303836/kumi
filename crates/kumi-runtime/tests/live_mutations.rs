#[path = "../../../tests/support/fixture_paths.rs"]
mod fixture_paths;
use async_trait::async_trait;
use futures::FutureExt;
use kumi_common::{
    abort::{Signal, SignalExt},
    js::json::stringify,
};
use kumi_runtime::{
    core::{
        contracts::{Integration, JsonObject, ToolResult},
        errors::RuntimeError,
    },
    integrations::ableton::{
        actions::ACTIONS, arrange::ArrangeHost, changes::CHANGES, connection::LiveConnection, integration::Ableton, mutations::Mutations,
        observation::ObservationHost, options::AbletonOptions, remember::CurrentProject, watch::Watch,
    },
    mcp::{
        client::{McpEndpoint, StderrStatus},
        types::{CallToolResult, Implementation, ListToolsResult},
    },
};
thread_local! { static FIXTURE_ROOT: RefCell<String> = const { RefCell::new(String::new()) }; }
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    rc::{Rc, Weak},
};

struct Fixture {
    case: Value,
    tools: Value,
    calls: RefCell<Vec<Value>>,
    lists: Cell<usize>,
    original: RefCell<Signal>,
    connection: RefCell<Weak<LiveConnection>>,
}
#[async_trait(?Send)]
impl McpEndpoint for Fixture {
    fn pid(&self) -> Option<u32> {
        None
    }
    fn server_info(&self) -> Option<Implementation> {
        Some(
            serde_json::from_value(
                json!({"name":"fixture","version":self.case["config"].get("version").cloned().unwrap_or(json!("1.0.73"))}),
            )
            .unwrap(),
        )
    }
    async fn list(&self, _: Option<&str>, _: Signal) -> Result<ListToolsResult, RuntimeError> {
        self.lists.set(self.lists.get() + 1);
        let contains = |key: &str, name: &Value| self.case["config"][key].as_array().is_some_and(|a| a.contains(name));
        Ok(serde_json::from_value(json!({"tools":self.tools.as_array().unwrap().iter().filter(|name|!contains("missing",name)&&!(self.lists.get()==1&&contains("initiallyMissing",name))).map(|name|json!({"name":name,"inputSchema":self.case["config"]["schemas"].get(name.as_str().unwrap()).cloned().unwrap_or(json!({"type":"object"}))})).collect::<Vec<_>>()})).unwrap())
    }
    async fn call(&self, name: &str, args: JsonObject, signal: Signal) -> Result<CallToolResult, RuntimeError> {
        signal.check()?;
        let index = self.calls.borrow().len();
        let call = json!({"name":name,"args":args});
        eq(&call, &self.case["calls"][index], &format!("{} dispatch {index}", self.case["label"]));
        self.calls.borrow_mut().push(call);
        let response = &self.case["responses"][index];
        if response["cancel"] == true {
            self.original.borrow().cancel();
        }
        if response["bump"] == true {
            let connection = self.connection.borrow().upgrade().unwrap();
            connection.lease.set(connection.lease.get() + 1);
        }
        if let Some(error) = response["throw"].as_str() {
            return Err(RuntimeError::plain(error));
        }
        serde_json::from_value(response["reply"].clone()).map_err(|e| RuntimeError::plain(e.to_string()))
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
        v => v.clone(),
    }
}
fn normalized(value: &Value) -> String {
    let home = kumi_runtime::library::sources::homedir();
    let value = fixture_paths::map_strings(value, &|text| fixture_paths::normalize_root(text, &home, "<home>"));
    let value = FIXTURE_ROOT
        .with(|root| fixture_paths::map_strings(&value, &|text| fixture_paths::normalize_root(text, root.borrow().as_str(), "<fixture>")));
    let text = stringify(&canonical(&value));
    let text = regex::Regex::new(r"[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}").unwrap().replace_all(&text, "<uuid>");
    regex::Regex::new(r"\bc\d+\b").unwrap().replace_all(&text, "<change>").into_owned()
}
fn eq(actual: &Value, expected: &Value, label: &str) {
    assert_eq!(normalized(actual), normalized(expected).replace("npm run kumi --", &kumi_runtime::command::KUMI), "{label}");
}
fn tool_result(result: ToolResult) -> Value {
    let mut value = json!(result);
    value["isError"] = json!(result.is_error);
    value
}
async fn streaming(mutations: &Rc<Mutations>, operation: &Value, signal: Signal) -> Result<Value, RuntimeError> {
    let starts = Rc::new(Cell::new(0));
    let out = starts.clone();
    let stream = mutations.stream_changes(signal.clone(), Rc::new(move || out.set(out.get() + 1)));
    let mut progress = Vec::new();
    for chunk in operation["chunks"].as_array().into_iter().flatten() {
        stream.push(chunk.as_str().unwrap());
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
        progress.push(json!({"started":stream.started()}));
    }
    if operation["cancel"] == true {
        signal.cancel();
    }
    if operation["abandon"] == true {
        stream.abandon().await;
        return Ok(json!({"abandoned":true,"started":stream.started(),"onStarts":starts.get(),"progress":progress}));
    }
    let result = stream.finish(operation["finish"].as_object().cloned()).await?;
    Ok(json!({"result":tool_result(result),"started":stream.started(),"onStarts":starts.get(),"progress":progress}))
}
#[tokio::test(flavor = "current_thread")]
async fn preview_apply_history_reference_retirement_and_actions_match_source() {
    tokio::task::LocalSet::new().run_until(replay()).await;
}
async fn replay() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("Set.als"), "last saved Set").unwrap();
    let root = directory.path().to_string_lossy().replace('\\', "/");
    FIXTURE_ROOT.with(|value| *value.borrow_mut() = root.clone());
    let fixture: Value = serde_json::from_str(include_str!("support/mutations-oracle.json")).unwrap();
    for (case_index, original) in fixture["cases"].as_array().unwrap().iter().enumerate() {
        let mut case = original.clone();
        case["responses"] = json!(case["responses"]
            .as_array()
            .unwrap()
            .iter()
            .map(|id| fixture["values"][id.as_u64().unwrap() as usize].clone())
            .collect::<Vec<_>>());
        let endpoint = Rc::new(Fixture {
            case: case.clone(),
            tools: fixture["toolNames"].clone(),
            calls: RefCell::new(Vec::new()),
            lists: Cell::new(0),
            original: RefCell::new(Signal::new()),
            connection: RefCell::new(Weak::new()),
        });
        let config = &case["config"];
        let events = Rc::new(RefCell::new(Vec::<Value>::new()));
        let actions = Rc::new(RefCell::new(Vec::<Value>::new()));
        let watch_events = Rc::new(RefCell::new(Vec::<Value>::new()));
        let disks = Rc::new(RefCell::new(Vec::<Value>::new()));
        let mut options = AbletonOptions::new(Rc::new(|_, _| {}));
        let out = endpoint.clone();
        options.connect = Some(Rc::new(move |_| {
            let endpoint: Rc<dyn McpEndpoint> = out.clone();
            async move { Ok(endpoint) }.boxed_local()
        }));
        options.now = Some(Rc::new(|| chrono::DateTime::parse_from_rfc3339("2026-10-03T12:00:00Z").unwrap().with_timezone(&chrono::Utc)));
        options.generation = Some("connection".into());
        options.fast = Some(config["fast"].as_bool().unwrap_or(false));
        options.change_timeout_ms = Some(50);
        let out = events.clone();
        options.on_change = Some(Rc::new(move |record| out.borrow_mut().push(json!(record))));
        let out = actions.clone();
        options.on_action = Some(Rc::new(move |event| out.borrow_mut().push(json!(event))));
        let out = watch_events.clone();
        options.on_watch = Some(Rc::new(move |on| out.borrow_mut().push(json!(on))));
        let out = disks.clone();
        let disk = config["disk"].as_str().map(str::to_owned);
        options.low_disk = Some(Rc::new(move |folder, bytes, what| {
            out.borrow_mut().push(json!([folder, bytes, what]));
            let result = disk.clone();
            async move { result }.boxed_local()
        }));
        let integration = Ableton::new(options);
        let connection = integration.connection.clone();
        *endpoint.connection.borrow_mut() = Rc::downgrade(&connection);
        connection.start(Signal::new()).await.unwrap();
        connection.tools().unwrap().refresh(Signal::new()).await.unwrap();
        connection.available.set(config["available"].as_bool().unwrap_or(true));
        connection.lost.set(config["lost"].as_bool().unwrap_or(false));
        connection.epoch.set((config["noEpoch"] != true).then_some(7.0));
        *connection.set.borrow_mut() = config["set"].as_str().map(str::to_owned);
        {
            let mut book = connection.references.borrow_mut();
            for row in config["refs"].as_array().into_iter().flatten() {
                book.refs.insert(row[0].as_str().unwrap().into(), row[1].as_str().unwrap().into());
            }
            for reference in config["shorts"].as_array().into_iter().flatten() {
                book.short_ref(reference.as_str().unwrap());
            }
            for row in config["known"].as_array().into_iter().flatten() {
                book.known.insert(row[0].as_str().unwrap().into(), serde_json::from_value(row[1].clone()).unwrap());
            }
            for row in config["cursors"].as_array().into_iter().flatten() {
                book.cursors.insert(row[0].as_str().unwrap().into(), row[1].as_str().unwrap().into());
            }
        }
        let history = integration.history.clone();
        let remember = history.remember.clone();
        if let Some(project) = config.get("project") {
            let path = project["path"].as_str().map(|s| s.replace("<fixture>", &root));
            *remember.current.borrow_mut() = Some(Rc::new(CurrentProject {
                identity: project["identity"].as_str().unwrap().into(),
                project: path.as_deref().map(kumi_runtime::integrations::ableton::project::project_id_of),
                path,
                name: project["name"].as_str().unwrap().into(),
            }));
        }
        history.changes_this_turn.set(config["count"].as_u64().unwrap_or(0) as usize);
        let parameters = integration.mutations.parameters.clone();
        let observer = integration.observer.clone();
        observer.tempo.set(Some(120.0));
        let mutations = integration.mutations.clone();
        let arrangement = mutations.arrange_host();
        let watcher = Watch::new(mutations.clone());
        for sample in config["samples"].as_array().into_iter().flatten() {
            mutations
                .samples
                .samples
                .borrow_mut()
                .insert(sample["path"].as_str().unwrap().into(), serde_json::from_value(sample.clone()).unwrap());
        }
        for (index, operation) in case["operations"].as_array().unwrap().iter().enumerate() {
            let signal = Signal::new();
            if operation["abort"] == true {
                signal.cancel();
            }
            *endpoint.original.borrow_mut() = signal.clone();
            if let Some(set) = operation["set"].as_str() {
                *connection.set.borrow_mut() = Some(set.into());
            }
            let work = async {
                let input = fixture_paths::map_strings(&operation["input"], &|text| text.replace("<fixture>", &root))
                    .as_object()
                    .cloned()
                    .unwrap_or_default();
                if let Some(service) = operation["service"].as_str() {
                    let result: Result<Value, RuntimeError> = match service {
                        "public" => match integration.definitions().into_iter().find(|t| Some(t.name()) == operation["tool"].as_str()) {
                            Some(tool) => tool.execute(input, signal).await.map(tool_result),
                            None => Err(RuntimeError::plain("Tool not offered")),
                        },
                        "plan" => mutations.make_changes(input, signal).await.map(tool_result),
                        "watch" => watcher.execute(input, signal).await.map(tool_result),
                        "stream" => streaming(&mutations, operation, signal).await,
                        "clip" => mutations.clip_file(operation["named"].as_str().unwrap(), signal).await.map(|v| json!(v)),
                        "copy" => mutations.keep_copy(signal).await.map(|v| json!(v)),
                        "step" => mutations.step(operation["tool"].as_str().unwrap(), input, signal).await.map(|v| json!(v)),
                        "arrange" => arrangement.change(operation["tool"].as_str().unwrap(), input, signal).await.map(|made| {
                            let mut value = json!({"id":made.id});
                            if let Some(reference) = made.reference {
                                value["ref"] = json!(reference);
                            }
                            value
                        }),
                        "offers" => Ok(json!(arrangement.offers(operation["tool"].as_str().unwrap()))),
                        "undoStep" => match arrangement.undo_step().await {
                            Err(e) => Err(e),
                            Ok(step) => {
                                let opened = step.opened;
                                (step.close)().await.map(|_| json!(opened))
                            }
                        },
                        "tell" => {
                            arrangement.tell(operation["title"].as_str().unwrap());
                            Ok(Value::Null)
                        }
                        _ => panic!("unknown service"),
                    };
                    return match result {
                        Ok(v) => v,
                        Err(RuntimeError::Aborted) => json!({"error":"cancelled"}),
                        Err(e) => json!({"error":e.to_string()}),
                    };
                }
                if operation["action"] == true {
                    let kind = ACTIONS.iter().find(|k| k.tool == operation["tool"].as_str().unwrap()).unwrap();
                    json!(mutations.act(kind, input, signal, operation["cleanup"] == true).await)
                } else {
                    let kind = CHANGES.iter().find(|k| k.tool == operation["tool"].as_str().unwrap()).unwrap();
                    json!(mutations.change(kind, input, signal, operation["settled"] == true).await)
                }
            };
            let value = if operation["quiet"] == true { history.quietly(Some(&mut Vec::new()), work).await } else { work.await };
            let changes: Vec<_> = history.entries.borrow().values().map(|entry| json!(*entry.borrow())).collect();
            let state = {
                let mut book = connection.references.borrow_mut();
                let names: Vec<_> = book
                    .named_references()
                    .into_iter()
                    .map(|reference| {
                        let short = book.short_ref(&reference);
                        json!([reference, short])
                    })
                    .collect();
                json!({"changes":changes,"changesThisTurn":history.changes_this_turn.get(),"refs":book.refs.iter().collect::<Vec<_>>(),"known":book.known.iter().collect::<Vec<_>>(),"names":names,"cursors":book.cursors.iter().collect::<Vec<_>>(),"tempo":observer.tempo.get(),"lease":connection.lease.get(),"found":parameters.fast_found.borrow().iter().collect::<Vec<_>>()})
            };
            let label = format!("{} case {case_index} step {index}", case["label"]);
            eq(&value, &case["results"][index]["value"], &label);
            eq(&state, &case["results"][index]["state"], &format!("{label} state"));
        }
        let label = format!("{} case {case_index}", case["label"]);
        eq(&json!(*endpoint.calls.borrow()), &case["calls"], &format!("{label} calls"));
        eq(&json!(*events.borrow()), &case["events"], &format!("{label} events"));
        eq(&json!(*actions.borrow()), &case["actions"], &format!("{label} actions"));
        eq(&json!(*watch_events.borrow()), &case["watchEvents"], &format!("{label} watch events"));
        eq(&json!(*disks.borrow()), &case["disks"], &format!("{label} disk"));
        eq(&json!(endpoint.lists.get()), &case["listCalls"], &format!("{label} lists"));
        let keys: Vec<_> =
            endpoint.calls.borrow().iter().filter_map(|call| call["args"]["idempotencyKey"].as_str().map(str::to_owned)).collect();
        let unique: std::collections::HashSet<_> = keys.iter().collect();
        assert_eq!(keys.len(), unique.len(), "{label} reuses mutation keys");
        remember.cancel_timer();
        integration.close().await.unwrap();
    }
}
