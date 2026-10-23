//! The project's name the app shows (the header, the terminal's title) now comes from the observation: the DAW's
//! name for the project, carried on the observation event. It used to be read from the label's words. Over every
//! case of Live's observation oracle, and over Set names and files the oracle doesn't have, Live's integration gives
//! the same name both ways, so the app reads as it did on real Live, not only on fixtures.

use async_trait::async_trait;
use futures::FutureExt;
use kumi::tui::app::{observed_name, set_name_from, shown_name};
use kumi_common::abort::{Signal, SignalExt};
use kumi_daw_ableton::{
    connection::{ConnectionOptions, LiveConnection},
    observation::{ObservationHost, ObservedChange, Observer},
    remember::Remember,
};
use kumi_runtime::{
    core::{contracts::*, errors::RuntimeError},
    mcp::{
        client::{McpEndpoint, StderrStatus},
        types::{CallToolResult, Implementation, ListToolsResult},
    },
};
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::{Rc, Weak},
};

static ORACLE: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| kumi_testkit::oracle("kumi-daw-ableton/observation-oracle.json"));

#[derive(Clone)]
struct Listener {
    on: Rc<Cell<bool>>,
    callback: Rc<dyn Fn()>,
}

/// Live's bridge as the oracle recorded it: each call answered in order, with its effects (a reconnect, a lost
/// connection, a cancelled turn). With `by_kind`, each call is answered by its tool and kind instead, in any order.
struct Fixture {
    case: Value,
    by_kind: Option<HashMap<String, Value>>,
    config: RefCell<Value>,
    calls: RefCell<Vec<Value>>,
    disconnects: RefCell<Vec<Listener>>,
    owner: RefCell<Weak<LiveConnection>>,
    changes: RefCell<Vec<ObservedChange>>,
}
#[async_trait(?Send)]
impl McpEndpoint for Fixture {
    fn pid(&self) -> Option<u32> {
        None
    }
    fn server_info(&self) -> Option<Implementation> {
        let version = self.case["settings"].get("version").cloned().unwrap_or(json!("1.0.73"));
        Some(serde_json::from_value(json!({"name": "fixture", "version": version})).unwrap())
    }
    async fn list(&self, _: Option<&str>, signal: Signal) -> Result<ListToolsResult, RuntimeError> {
        signal.check()?;
        let missing = self.config.borrow()["missing"].as_array().cloned().unwrap_or_default();
        let tools: Vec<Value> = ["live_status", "live_discover", "live_song_state", "live_project_info"]
            .iter()
            .filter(|name| !missing.contains(&json!(name)))
            .map(|name| json!({"name": name, "inputSchema": {"type": "object"}}))
            .collect();
        Ok(serde_json::from_value(json!({ "tools": tools })).unwrap())
    }
    async fn call(&self, name: &str, args: JsonObject, signal: Signal) -> Result<CallToolResult, RuntimeError> {
        signal.check()?;
        if let Some(replies) = &self.by_kind {
            let key = format!("{name}:{}", args.get("kind").and_then(Value::as_str).unwrap_or(""));
            let reply = replies.get(&key).cloned().ok_or_else(|| RuntimeError::plain(format!("no reply for {key}")))?;
            return Ok(serde_json::from_value(reply).unwrap());
        }
        let index = self.calls.borrow().len();
        let call = json!({"name": name, "args": args});
        self.calls.borrow_mut().push(call.clone());
        assert_eq!(call, self.case["calls"][index], "{} dispatch {index}", self.case["label"]);
        let entry = &self.case["responses"][index];
        match entry["effect"].as_str() {
            Some("invalidate") => self.owner.borrow().upgrade().unwrap().invalidate(),
            Some("disconnect") => {
                for listener in self.disconnects.borrow().clone() {
                    if listener.on.get() {
                        (listener.callback)();
                    }
                }
            }
            Some("abort") => {
                signal.cancel();
                return Err(RuntimeError::Aborted);
            }
            _ => {}
        }
        tokio::task::yield_now().await;
        if let Some(message) = entry["throw"].as_str() {
            return Err(RuntimeError::plain(message));
        }
        serde_json::from_value(entry["reply"].clone()).map_err(|e| RuntimeError::plain(format!("fixture call {index}: {e}")))
    }
    fn on_catalog_changed(&self, _: Rc<dyn Fn()>) -> Box<dyn Fn()> {
        Box::new(|| {})
    }
    fn on_disconnect(&self, callback: Rc<dyn Fn()>) -> Box<dyn Fn()> {
        let on = Rc::new(Cell::new(true));
        self.disconnects.borrow_mut().push(Listener { on: on.clone(), callback });
        Box::new(move || on.set(false))
    }
    fn stderr_status(&self) -> StderrStatus {
        StderrStatus { bytes: 0, truncated: false }
    }
    async fn close(&self) -> Result<(), RuntimeError> {
        Ok(())
    }
}
#[async_trait(?Send)]
impl ObservationHost for Fixture {
    fn reset_turn(&self, _: bool) {}
    fn changes(&self) -> Vec<ObservedChange> {
        self.changes.borrow().clone()
    }
    fn definitions(&self) -> Vec<Rc<dyn KernelTool>> {
        Vec::new()
    }
    async fn restore_after_crash(&self, _: &str, _: Option<&str>, _: Signal) -> Result<Option<String>, RuntimeError> {
        Ok(None)
    }
}

/// Live's integration on a fixture: its connection, what it remembers of the Set, and its observer.
async fn integration(case: Value, by_kind: Option<HashMap<String, Value>>) -> (Rc<Fixture>, Rc<LiveConnection>, Rc<Remember>, Observer) {
    let endpoint = Rc::new(Fixture {
        case,
        by_kind,
        config: RefCell::new(json!({})),
        calls: RefCell::default(),
        disconnects: RefCell::default(),
        owner: RefCell::new(Weak::new()),
        changes: RefCell::default(),
    });
    let mut options = ConnectionOptions::new(Rc::new(|_, _| {}));
    let out = endpoint.clone();
    options.connect = Some(Rc::new(move |_| {
        let endpoint: Rc<dyn McpEndpoint> = out.clone();
        async move { Ok(endpoint) }.boxed_local()
    }));
    options.now = Some(Rc::new(|| chrono::DateTime::parse_from_rfc3339("2026-10-03T12:00:00Z").unwrap().with_timezone(&chrono::Utc)));
    options.generation = Some("connection".into());
    options.reconnect_interval_ms = Some(3_600_000);
    let connection = LiveConnection::new(options);
    *endpoint.owner.borrow_mut() = Rc::downgrade(&connection);
    connection.start(Signal::new()).await.unwrap();
    let remember = Remember::new(connection.clone(), None, None);
    let observer = Observer::new(connection.clone(), remember.clone());
    (endpoint, connection, remember, observer)
}

/// The name the app shows for an observation is the one it read from the label before; the event carries the DAW's
/// name for the project when there is one. Says whether the observation had the project's identity.
fn same_name(observation: &Observation, label: &str) -> bool {
    let before = set_name_from(&observation.label);
    if let Some(identity) = &observation.identity {
        // The DAW's name, as the app shows it (trimmed; none for one over lines), is the label's.
        assert_eq!(shown_name(&identity.name), before, "{label}: the name against the label {:?}", observation.label);
        assert_eq!(identity.daw, DawId::Ableton, "{label}");
        assert_eq!(identity.saved_at, observation.saved_at, "{label}");
    }
    let event = serde_json::to_value(SessionEvent::observed(observation)).unwrap();
    assert_eq!(event["type"], "observation", "{label}");
    assert_eq!(event["label"], json!(observation.label), "{label}");
    assert_eq!(event.get("name").cloned(), observation.identity.as_ref().map(|identity| json!(identity.name)), "{label}: the event's name");
    assert_eq!(observed_name(&event), before, "{label}: what the app shows");
    // An event from before the name was carried reads as it always did.
    assert_eq!(observed_name(&json!({"type": "observation", "label": observation.label})), before, "{label}: an older event");
    observation.identity.is_some()
}

#[tokio::test(flavor = "current_thread")]
async fn every_oracle_case_names_the_set_as_its_label_did() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let fixture: Value = serde_json::from_str(&ORACLE).unwrap();
            let (mut observed, mut named, mut shown) = (0, 0, std::collections::BTreeSet::new());
            for original in fixture["cases"].as_array().unwrap() {
                let mut case = original.clone();
                case["responses"] = json!(case["responses"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|id| fixture["values"][id.as_u64().unwrap() as usize].clone())
                    .collect::<Vec<_>>());
                let (endpoint, connection, remember, observer) = integration(case.clone(), None).await;
                for (index, config) in case["turns"].as_array().unwrap().iter().enumerate() {
                    *endpoint.config.borrow_mut() = config.clone();
                    for (key, cell) in
                        [("reconnected", &connection.reconnected), ("lost", &connection.lost), ("available", &connection.available)]
                    {
                        if let Some(value) = config[key].as_bool() {
                            cell.set(value)
                        }
                    }
                    for change in config["changes"].as_array().into_iter().flatten() {
                        let record: ChangeRecord = serde_json::from_value(change["record"].clone()).unwrap();
                        let within = change["within"].as_str().is_some_and(|s| !s.is_empty());
                        let mut changes = endpoint.changes.borrow_mut();
                        match changes.iter_mut().find(|kept| kept.record.id == record.id) {
                            Some(kept) => *kept = ObservedChange { record, within },
                            None => changes.push(ObservedChange { record, within }),
                        }
                    }
                    let signal = Signal::new();
                    if config["abort"] == true {
                        signal.cancel();
                    }
                    let hints = config.get("hints").map(|h| ObserveHints {
                        pinned: h.get("pinned").map(|v| serde_json::from_value(v.clone()).unwrap()),
                        continuing: h["continuing"].as_bool(),
                    });
                    let label = format!("{} turn {index}", case["label"]);
                    let result = observer.observe(endpoint.as_ref(), signal, hints).await;
                    // The replay is the oracle's: the same label it recorded, or the same failure.
                    match (&result, &case["results"][index]["value"]) {
                        (Ok(observation), expected) => assert_eq!(json!(observation.label), expected["label"], "{label}"),
                        (Err(_), expected) => assert!(expected.get("error").is_some(), "{label}: {expected}"),
                    }
                    if let Ok(observation) = result {
                        observed += 1;
                        if same_name(&observation, &label) {
                            named += 1;
                            shown.insert(set_name_from(&observation.label));
                        }
                    }
                }
                remember.cancel_timer();
                connection.close().await.unwrap();
                assert_eq!(json!(*endpoint.calls.borrow()), case["calls"], "{} all dispatches", case["label"]);
            }
            // Every turn that read Live named its Set (the rest had no Live access); the unsaved Set's name and the long
            // names are among them.
            assert!(observed > 80 && observed - named <= 3, "{named} named of {observed} observed");
            assert!(shown.contains(&Some("(unnamed/unsaved)".into())) && shown.contains(&Some("Renamed".into())), "{shown:?}");
            assert!(shown.iter().flatten().any(|name| name.chars().count() > 60 && name.starts_with('🦀')), "{shown:?}");
        })
        .await;
}

/// The oracle's `basic` Set with another name and file.
fn named(fixture: &Value, name: &Value, file: &str) -> HashMap<String, Value> {
    let basic = fixture["cases"].as_array().unwrap().iter().find(|case| case["label"] == "basic").unwrap();
    let mut replies = HashMap::new();
    for (call, id) in basic["calls"].as_array().unwrap().iter().zip(basic["responses"].as_array().unwrap()) {
        let kind = call["args"]["kind"].as_str().unwrap_or("");
        let mut reply = fixture["values"][id.as_u64().unwrap() as usize]["reply"].clone();
        if call["name"] == "live_discover" && kind == "set" {
            let mut content: Value = serde_json::from_str(reply["content"][0]["text"].as_str().unwrap()).unwrap();
            for body in [&mut content, &mut reply["structuredContent"]] {
                body["items"][0]["name"] = name.clone();
                body["items"][0]["filePath"] = json!(file);
            }
            reply["content"][0]["text"] = json!(content.to_string());
        }
        replies.insert(format!("{}:{kind}", call["name"].as_str().unwrap()), reply);
    }
    replies
}

#[tokio::test(flavor = "current_thread")]
async fn odd_untitled_and_template_set_names_read_as_their_labels_did() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let fixture: Value = serde_json::from_str(&ORACLE).unwrap();
            let folder = tempfile::tempdir().unwrap();
            let file = |path: &str| {
                let path = folder.path().join(path);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(&path, b"").unwrap();
                path.to_string_lossy().into_owned()
            };
            let files = ["".to_string(), file("Night Drive Project/Night Drive.als"), file("User Library/Templates/Default.als")];
            let long = "Ab — ".repeat(80);
            let names = [
                json!("Night Drive"),
                json!("Untitled"),
                json!("Default"),
                json!(""),
                json!("   "),
                Value::Null,
                json!("  padded  "),
                json!("A — B"),
                json!("trailing —"),
                json!("— leading"),
                json!("Current open Set: inside"),
                json!("line\nbreak"),
                json!("carriage\rreturn"),
                json!("para\u{2028}sep"),
                json!("tab\there"),
                json!("quote \"x\" \\ back"),
                json!("名前のセット"),
                json!("مجموعة الليل"),
                json!("e\u{301}te\u{301}"),
                json!("🎛️ Set"),
                json!(long),
            ];
            let mut seen = 0;
            for file in &files {
                for name in &names {
                    let label = format!("name {name} in {file:?}");
                    let (endpoint, connection, remember, observer) =
                        integration(json!({"label": label, "settings": {}}), Some(named(&fixture, name, file))).await;
                    let observation =
                        observer.observe(endpoint.as_ref(), Signal::new(), None).await.unwrap_or_else(|e| panic!("{label}: {e}"));
                    assert!(same_name(&observation, &label), "{label}: no identity");
                    let identity = observation.identity.as_ref().unwrap();
                    assert_eq!(identity.file.as_deref().unwrap_or(""), file.as_str(), "{label}");
                    seen += 1;
                    remember.cancel_timer();
                    connection.close().await.unwrap();
                }
            }
            assert_eq!(seen, files.len() * names.len());
        })
        .await;
}
