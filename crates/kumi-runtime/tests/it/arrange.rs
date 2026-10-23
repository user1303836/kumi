use async_trait::async_trait;
use futures::{future::LocalBoxFuture, FutureExt};
use kumi_common::{abort::Signal, js::json::stringify};
use kumi_runtime::{core::contracts::JsonObject, integrations::ableton::arrange::*, RuntimeError};
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
fn oracle() -> Value {
    serde_json::from_str(&kumi_testkit::oracle("kumi-runtime/arrange-oracle.json")).unwrap()
}
fn cases(data: &Value, kind: &str) -> Vec<Value> {
    data["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["type"] == kind)
        .map(|row| {
            let mut result = row.clone();
            for (key, value) in result.as_object_mut().unwrap() {
                if key != "type" {
                    *value = data["values"][value.as_u64().unwrap() as usize].clone();
                }
            }
            result
        })
        .collect()
}
fn normalized(value: Value) -> Value {
    serde_json::from_str(&stringify(&value)).unwrap()
}
#[test]
fn arrangement_requests_compilation_descriptions_and_section_lines_match_source() {
    let data = oracle();
    assert_eq!(*ARRANGE_SCHEMA, data["schema"].as_object().unwrap().clone());
    assert_eq!(*ARRANGE_DESCRIPTION, data["description"]);
    for case in cases(&data, "parse") {
        let result = match arrange_request(case["input"].as_object().unwrap()) {
            Ok(r) => json!(r),
            Err(e) => json!(e),
        };
        assert_eq!(normalized(result), case["value"], "{case}");
    }
    for case in cases(&data, "describe") {
        let material = serde_json::from_value(case["material"].clone()).unwrap();
        assert_eq!(normalized(json!(describe(&material))), case["value"], "{case}");
    }
    for case in cases(&data, "compile") {
        let material = serde_json::from_value(case["material"].clone()).unwrap();
        let request = serde_json::from_value(case["request"].clone()).unwrap();
        let result = match compile(
            &material,
            &request,
            Shortens { midi: case["shortens"]["midi"].as_bool().unwrap(), audio: case["shortens"]["audio"].as_bool().unwrap() },
        ) {
            Ok(plan) => {
                assert_eq!(json!(section_lines(&plan)), case["lines"], "{case}");
                json!(plan)
            }
            Err(e) => json!(e),
        };
        assert_eq!(normalized(result), case["value"], "{case}");
    }
}
struct Host {
    config: Value,
    calls: Rc<RefCell<Vec<Value>>>,
    ids: RefCell<Vec<String>>,
    count: Cell<u64>,
    signal: Signal,
}
impl Host {
    fn new(config: Value) -> Self {
        Self { config, calls: Rc::new(RefCell::new(vec![])), ids: RefCell::new(vec![]), count: Cell::new(0), signal: Signal::new() }
    }
    fn call(&self, value: Value) {
        self.calls.borrow_mut().push(value);
    }
}
#[async_trait(?Send)]
impl ArrangeHost for Host {
    fn tempo(&self) -> Option<f64> {
        Some(self.config["tempo"].as_f64().unwrap_or(124.0))
    }
    fn beats_per_bar(&self) -> f64 {
        self.config["bpb"].as_f64().unwrap_or(4.0)
    }
    async fn read(&self, kind: &str, extra: JsonObject, _: Signal) -> Result<Vec<JsonObject>, RuntimeError> {
        self.call(json!(["read", kind, extra]));
        if self.config["failRead"] == kind || self.config.get("failReadParent").is_some_and(|p| Some(p) == extra.get("parent")) {
            return Err(RuntimeError::plain("read failed"));
        }
        if self.config["pendingRead"] == kind {
            futures::future::pending::<()>().await;
        }
        let key = format!("{kind}|{}", extra.get("parent").and_then(Value::as_str).unwrap_or("undefined"));
        Ok(serde_json::from_value(
            self.config["reads"].get(&key).or_else(|| self.config["reads"].get(kind)).cloned().unwrap_or_else(|| json!([])),
        )
        .unwrap())
    }
    fn offers(&self, tool: &str) -> bool {
        !self.config["unavailable"].as_array().is_some_and(|a| a.contains(&json!(tool)))
    }
    async fn change(&self, tool: &str, input: JsonObject, _: Signal) -> Result<Made, RuntimeError> {
        let count = self.count.get() + 1;
        self.count.set(count);
        self.call(json!(["change", tool, input]));
        let id = format!("c{count}");
        self.ids.borrow_mut().push(id.clone());
        if self.config["cancelAt"].as_u64() == Some(count) {
            self.signal.cancel();
        }
        if self.config["failAt"].as_u64() == Some(count) {
            return Err(RuntimeError::plain("change failed"));
        }
        Ok(Made { id: id.clone(), reference: (self.config["missingRef"] != true).then(|| format!("made:{id}")) })
    }
    async fn undo(&self, id: &str, signal: Signal) -> Result<bool, RuntimeError> {
        self.call(json!(["undo", id, signal.is_cancelled()]));
        if self.config["throwUndo"] == true {
            return Err(RuntimeError::plain("undo failed"));
        }
        Ok(self.config["failUndo"] != true)
    }
    async fn quietly(&self, work: LocalBoxFuture<'_, Result<Built, RuntimeError>>) -> Result<QuietBuilt, RuntimeError> {
        self.call(json!(["quietly"]));
        let value = work.await?;
        Ok(QuietBuilt { value, ids: self.ids.borrow().clone() })
    }
    fn record(&self, title: &str, ids: &[String], apart: &[String]) -> Option<String> {
        self.call(json!(["record", title, ids, apart]));
        Some("group".into())
    }
    async fn undo_step(&self) -> Result<UndoStep, RuntimeError> {
        self.call(json!(["open"]));
        let calls = self.calls.clone();
        let fail = self.config["failClose"] == true;
        Ok(UndoStep {
            opened: true,
            close: Box::new(move || {
                async move {
                    calls.borrow_mut().push(json!(["close"]));
                    if fail {
                        Err(RuntimeError::plain("close failed"))
                    } else {
                        Ok(())
                    }
                }
                .boxed_local()
            }),
        })
    }
    async fn keep_copy(&self, _: Signal) -> Result<Option<String>, RuntimeError> {
        self.call(json!(["keepCopy"]));
        Ok(self.config["copy"].as_str().map(str::to_owned))
    }
    fn tell(&self, title: &str) {
        self.call(json!(["tell", title]));
    }
}
#[tokio::test]
async fn arrangement_reads_and_partial_failure_cleanup_match_source_call_for_call() {
    let data = oracle();
    for case in cases(&data, "read") {
        let host = Host::new(case["config"].clone());
        let loop_ = case.get("loop").map(|l| serde_json::from_value(l.clone()).unwrap());
        let request = case.get("request").map(|r| serde_json::from_value(r.clone()).unwrap());
        let value = read_material(&host, host.signal.clone(), loop_.as_ref(), request.as_ref()).await.unwrap();
        assert_eq!(normalized(json!(value)), case["value"], "{case}");
        assert_eq!(normalized(json!(*host.calls.borrow())), case["calls"], "{case}");
    }
    for case in cases(&data, "run") {
        let host = Host::new(case["config"].clone());
        let result = arrange(case["input"].as_object().unwrap().clone(), &host, host.signal.clone()).await;
        let value = match result {
            Ok(result) => {
                let mut value = json!(result);
                if let Ok(mut body) = serde_json::from_str::<Value>(value["text"].as_str().unwrap()) {
                    body["seconds"] = json!(0);
                    assert_eq!(stringify(&body), stringify(&case["value"]["text"]), "serialized tool response; config {}", case["config"]);
                    value["text"] = body;
                }
                value
            }
            Err(error) => json!({"error":error.to_string()}),
        };
        assert_eq!(normalized(value), case["value"], "config {}", case["config"]);
        assert_eq!(normalized(json!(*host.calls.borrow())), case["calls"], "config {}", case["config"]);
    }
}
