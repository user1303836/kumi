use async_trait::async_trait;
use futures::FutureExt;
use kumi_common::{abort::Signal, js::json::stringify};
use kumi_runtime::{
    core::{contracts::JsonObject, errors::RuntimeError},
    integrations::ableton::{
        connection::{ConnectionOptions, LiveConnection},
        project::{Baseline, ProjectStore},
        remember::{CurrentProject, Remember},
    },
    mcp::{
        client::{McpEndpoint, StderrStatus},
        types::{CallToolResult, Implementation, ListToolsResult},
    },
};
use serde_json::{json, Value};
use std::{
    cell::RefCell,
    rc::{Rc, Weak},
};
struct Fixture {
    config: Value,
    calls: RefCell<Vec<Value>>,
    storage: RefCell<Vec<Value>>,
    owner: RefCell<Weak<Remember>>,
}
fn current() -> Rc<CurrentProject> {
    // The test store takes the Set's path as its project, so what it records reads as before.
    Rc::new(CurrentProject {
        identity: "song".into(),
        name: "Set".into(),
        path: Some("/saved.als".into()),
        project: Some("/saved.als".into()),
        unsaved: false,
    })
}
#[async_trait(?Send)]
impl McpEndpoint for Fixture {
    fn pid(&self) -> Option<u32> {
        None
    }
    fn server_info(&self) -> Option<Implementation> {
        None
    }
    async fn list(&self, _: Option<&str>, _: Signal) -> Result<ListToolsResult, RuntimeError> {
        let names = ["live_project_info", "live_project_snapshot_export", "live_project_snapshot_diff"];
        Ok(serde_json::from_value(json!({"tools":names.iter().filter(|name|!self.config["missing"].as_array().is_some_and(|a|a.contains(&json!(name)))).map(|name|json!({"name":name,"inputSchema":{"type":"object"}})).collect::<Vec<_>>()})).unwrap())
    }
    async fn call(&self, name: &str, args: JsonObject, _: Signal) -> Result<CallToolResult, RuntimeError> {
        self.calls.borrow_mut().push(json!({"name":name,"args":args}));
        if self.config["throw"].as_str() == Some(name) {
            return Err(RuntimeError::plain("upstream failed"));
        }
        if self.config["replace"] == true {
            *self.owner.borrow().upgrade().unwrap().current.borrow_mut() = Some(current());
        }
        let body = if name == "live_project_info" {
            self.config.get("info").cloned().unwrap_or_else(|| json!({"path":"/saved.als","exists":true}))
        } else if name == "live_project_snapshot_diff" {
            if self.config["diffError"] == true {
                return Err(RuntimeError::plain("diff failed"));
            }
            self.config.get("diff").cloned().unwrap_or_else(|| json!({"items":[]}))
        } else {
            let index = args.get("cursor").and_then(Value::as_str).and_then(|s| s.parse::<usize>().ok()).unwrap_or(0);
            let mut page = JsonObject::new();
            if index + 1 < self.config["pages"].as_u64().unwrap_or(1) as usize {
                page.insert("nextCursor".into(), json!((index + 1).to_string()));
            }
            json!({"artifact":{"id":self.config.get("artifact").cloned().unwrap_or(json!("b"))},"page":page,"records":[]})
        };
        Ok(serde_json::from_value(json!({"content":[{"type":"text","text":stringify(&body)}],"structuredContent":body})).unwrap())
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
#[async_trait(?Send)]
impl ProjectStore for Fixture {
    async fn load(&self, project: &str) -> Result<Option<Baseline>, RuntimeError> {
        self.storage.borrow_mut().push(json!({"op":"load","path":project}));
        if self.config["loadError"] == true {
            return Err(RuntimeError::plain("load failed"));
        }
        Ok(self.config.get("baseline").map(|v| serde_json::from_value(v.clone()).unwrap()))
    }
    async fn save(&self, _project: &str, baseline: &Baseline) -> Result<(), RuntimeError> {
        self.storage.borrow_mut().push(json!({"op":"save","value":baseline}));
        if self.config["saveError"] == true {
            Err(RuntimeError::plain("save failed"))
        } else {
            Ok(())
        }
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
fn eq(actual: &Value, expected: &Value, label: &str) {
    assert_eq!(stringify(&canonical(actual)), stringify(&canonical(expected)), "{label}");
}
#[tokio::test(flavor = "current_thread")]
async fn saved_set_memory_export_queue_and_catch_up_match_source() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let fixture: Value = serde_json::from_str(include_str!("support/remember-oracle.json")).unwrap();
            for case in fixture["cases"].as_array().unwrap() {
                let config = &case["config"];
                let endpoint = Rc::new(Fixture {
                    config: config.clone(),
                    calls: RefCell::new(Vec::new()),
                    storage: RefCell::new(Vec::new()),
                    owner: RefCell::new(Weak::new()),
                });
                let mut options = ConnectionOptions::new(Rc::new(|_, _| {}));
                let out = endpoint.clone();
                options.connect = Some(Rc::new(move |_| {
                    let endpoint: Rc<dyn McpEndpoint> = out.clone();
                    async move { Ok(endpoint) }.boxed_local()
                }));
                options.now =
                    Some(Rc::new(|| chrono::DateTime::parse_from_rfc3339("2026-10-03T12:00:00Z").unwrap().with_timezone(&chrono::Utc)));
                let connection = LiveConnection::new(options);
                connection.start(Signal::new()).await.unwrap();
                connection.tools().unwrap().refresh(Signal::new()).await.unwrap();
                connection.available.set(config["available"] != false);
                connection.lost.set(config["lost"] == true);
                let caught = Rc::new(RefCell::new(Vec::new()));
                let out = caught.clone();
                let store = if config["noStore"] == true { None } else { Some(endpoint.clone() as Rc<dyn ProjectStore>) };
                let memory = Remember::new(
                    connection.clone(),
                    store,
                    Some(Rc::new(move |value| out.borrow_mut().push(serde_json::to_value(value).unwrap()))),
                );
                *endpoint.owner.borrow_mut() = Rc::downgrade(&memory);
                *memory.current.borrow_mut() = match config.get("project") {
                    None => Some(current()),
                    Some(Value::Null) => None,
                    Some(p) => Some(Rc::new(CurrentProject {
                        identity: p["identity"].as_str().unwrap().into(),
                        name: p["name"].as_str().unwrap().into(),
                        path: p["path"].as_str().map(str::to_owned),
                        project: p["path"].as_str().map(str::to_owned),
                        unsaved: false,
                    })),
                };
                let result: Result<Value, RuntimeError> = match case["operation"].as_str().unwrap() {
                    "path" => Ok(json!(memory.project_path(Signal::new()).await)),
                    "export" => memory.export_pages(Signal::new()).await.map(|v| json!(v)),
                    "save" => {
                        memory.save_now(None).await;
                        Ok(Value::Null)
                    }
                    "twice" => {
                        let a = memory.save_now(None);
                        let b = memory.save_now(None);
                        futures::join!(a, b);
                        Ok(json!([null, null]))
                    }
                    "catch" => {
                        memory.catch_up("song".into(), "Set".into(), config["afterReconnect"] == true);
                        memory.pending().await;
                        Ok(json!(*memory.context.borrow()))
                    }
                    _ => panic!("fixture operation"),
                };
                let value = result.unwrap_or_else(|e| json!({"error":e.to_string()}));
                let label = case["label"].as_str().unwrap();
                eq(&value, &case["value"], label);
                eq(&json!(*endpoint.calls.borrow()), &case["calls"], &format!("{label} calls"));
                eq(&json!(*endpoint.storage.borrow()), &case["storage"], &format!("{label} storage"));
                eq(&json!(*caught.borrow()), &case["caught"], &format!("{label} catch-up"));
                connection.close().await.unwrap();
            }
        })
        .await;
}
