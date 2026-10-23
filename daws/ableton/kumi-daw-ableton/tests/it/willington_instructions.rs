//! Willington's words follow its switch each time the model's session is made: Live's integration adds them at the
//! end of the system text, reading the switch again for every new kernel. (Moved from the session tests when
//! Willington moved behind the integration: the same states, the same offered tools, the same words.)

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use async_trait::async_trait;
use futures::FutureExt;
use kumi_agent::core::contracts::*;
use kumi_agent::mcp::client::{McpEndpoint, StderrStatus};
use kumi_agent::mcp::types::{CallToolResult, Implementation, ListToolsResult};
use kumi_agent::{create_session, SessionOptions};
use kumi_common::abort::{Signal, SignalExt};
use kumi_daw_ableton::integration::Ableton;
use kumi_daw_ableton::options::{AbletonOptions, HandsSetup};
use kumi_daw_ableton::willington::WillingtonSwitch;
use kumi_types::contracts::*;
use kumi_types::errors::RuntimeError;
use serde_json::{json, Value};

/// Live's bridge on the fixtures: the fullest public-tools catalog, less the tools a test takes away, answering the
/// observation oracle's `basic` Set.
struct Bridge {
    catalog: Vec<Value>,
    without: RefCell<Vec<&'static str>>,
    replies: HashMap<String, Value>,
    changed: RefCell<Vec<Rc<dyn Fn()>>>,
}
impl Bridge {
    fn new() -> Rc<Self> {
        let source: Value = serde_json::from_str(&kumi_testkit::oracle("kumi-daw-ableton/public-tools-oracle.json")).unwrap();
        let cases = source["cases"].as_array().unwrap();
        let size = |case: &Value| case["definitions"].as_array().unwrap().len();
        let fullest = cases.iter().fold(&cases[0], |best, case| if size(case) > size(best) { case } else { best });
        let catalog =
            fullest["catalog"].as_array().unwrap().iter().map(|id| source["pool"][id.as_u64().unwrap() as usize].clone()).collect();
        let observed: Value = serde_json::from_str(&kumi_testkit::oracle("kumi-daw-ableton/observation-oracle.json")).unwrap();
        let basic = observed["cases"].as_array().unwrap().iter().find(|case| case["label"] == "basic").unwrap();
        let replies = basic["calls"]
            .as_array()
            .unwrap()
            .iter()
            .zip(basic["responses"].as_array().unwrap())
            .map(|(call, id)| {
                let key = format!("{}:{}", call["name"].as_str().unwrap(), call["args"]["kind"].as_str().unwrap_or(""));
                (key, observed["values"][id.as_u64().unwrap() as usize]["reply"].clone())
            })
            .collect();
        Rc::new(Bridge { catalog, without: RefCell::new(vec![]), replies, changed: RefCell::new(vec![]) })
    }
    /// The bridge offers these tools again, and says its catalog changed.
    fn offer_all(&self) {
        self.without.borrow_mut().clear();
        for listener in self.changed.borrow().clone() {
            listener();
        }
    }
}
#[async_trait(?Send)]
impl McpEndpoint for Bridge {
    fn pid(&self) -> Option<u32> {
        None
    }
    fn server_info(&self) -> Option<Implementation> {
        Some(serde_json::from_value(json!({"name": "fixture", "version": "1.0.92"})).unwrap())
    }
    async fn list(&self, _: Option<&str>, signal: Signal) -> Result<ListToolsResult, RuntimeError> {
        signal.check()?;
        let without = self.without.borrow();
        let tools: Vec<&Value> = self.catalog.iter().filter(|tool| !without.iter().any(|name| tool["name"] == *name)).collect();
        Ok(serde_json::from_value(json!({ "tools": tools })).unwrap())
    }
    async fn call(&self, name: &str, args: JsonObject, signal: Signal) -> Result<CallToolResult, RuntimeError> {
        signal.check()?;
        let key = format!("{name}:{}", args.get("kind").and_then(Value::as_str).unwrap_or(""));
        let reply = self.replies.get(&key).cloned().ok_or_else(|| RuntimeError::plain(format!("no reply for {key}")))?;
        Ok(serde_json::from_value(reply).unwrap())
    }
    fn on_catalog_changed(&self, listener: Rc<dyn Fn()>) -> Box<dyn Fn()> {
        self.changed.borrow_mut().push(listener);
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

struct Idle;
#[async_trait(?Send)]
impl Kernel for Idle {
    async fn run(&self, _: &str, _: Signal, _: KernelEmit) -> Result<TurnResult, RuntimeError> {
        Ok(TurnResult { stop_reason: StopReason::Completed, usage: None })
    }
    async fn close(&self) {}
}

#[tokio::test(flavor = "current_thread")]
async fn willington_instructions_follow_the_switch_each_time_the_kernel_is_made() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let switch = Rc::new(Cell::new(WillingtonSwitch::Off));
            let bridge = Bridge::new();
            // Kumi can change the Set (make_changes), but the bridge offers neither Willington's mapping nor Python in Live.
            bridge.without.borrow_mut().extend(["live_willington_device_preview", "live_willington_device_apply", "live_run_python"]);
            let made: Rc<RefCell<Vec<KernelOptions>>> = Rc::default();
            let record = made.clone();
            let kernels: KernelFactory = Rc::new(move |options| {
                record.borrow_mut().push(options);
                async { Ok(Rc::new(Idle) as Rc<dyn Kernel>) }.boxed_local()
            });
            let (endpoint, read) = (bridge.clone(), switch.clone());
            let integration: IntegrationFactory = Box::new(move |listener| {
                let endpoint = endpoint.clone();
                let mut options = AbletonOptions::new(listener);
                options.connect = Some(Rc::new(move |_| {
                    let endpoint = endpoint.clone();
                    async move { Ok(endpoint as Rc<dyn McpEndpoint>) }.boxed_local()
                }));
                options.hands = Some(HandsSetup::Disabled);
                options.reconnect_interval_ms = Some(3_600_000);
                let read = read.clone();
                options.willington = Some(Rc::new(move || Some(read.get())));
                Ableton::new(options) as Rc<dyn Integration>
            });
            let mut options = SessionOptions::new(kernels, integration, Rc::new(|_| {}));
            options.timeout_ms = Some(10_000);
            let session = create_session(options).unwrap();
            let again = || async {
                session.reconfigure().await.unwrap();
                session.refresh().await.unwrap();
                made.borrow().last().unwrap().instructions.clone()
            };
            session.start().await.unwrap();
            let off = made.borrow()[0].instructions.clone();
            let tools: Vec<String> = made.borrow()[0].tools.iter().map(|tool| tool.name().to_string()).collect();
            assert!(tools.iter().any(|name| name == "make_changes"), "{tools:?}");
            assert!(!tools.iter().any(|name| name == "edit_rack_mapping" || name == "run_python"), "{tools:?}");
            assert!(off.contains("/willington turns the bindings on"));
            // Turned on a moment ago, before the bridge offers their tools: nothing said about them yet.
            switch.set(WillingtonSwitch::JustOn);
            let base = again().await;
            assert!(!base.contains("Willington"), "{base}");
            // The words go at the very end, after everything else in the system text.
            let end = |text: &str| text.strip_prefix(&base).and_then(|rest| rest.strip_prefix("\n\n")).map(str::to_string);
            let words = end(&off).unwrap_or_else(|| panic!("Willington's words end the system text:\n{off}"));
            assert!(!words.contains("\n\n"), "one paragraph: {words}");
            // On for a while, for a Live they have no bindings for: the next kernel says so, without pointing at /willington.
            switch.set(WillingtonSwitch::On);
            let on = again().await;
            assert!(on.contains("none fit the Live that's open") && !on.contains("/willington"));
            assert!(end(&on).is_some_and(|words| words.contains("none fit the Live that's open")), "{on}");
            // On, with Willington's mapping and Python in Live offered: the next kernel says modulators map, and how,
            // rather than that they can't (a tutorial's LFOs were rebuilt with automation when it said so).
            bridge.offer_all();
            let mapped = again().await;
            assert!(mapped.contains("map with map_modulator") && !mapped.contains("can't be mapped"), "{mapped}");
            let tools: Vec<String> = made.borrow().last().unwrap().tools.iter().map(|tool| tool.name().to_string()).collect();
            assert!(tools.iter().any(|name| name == "edit_rack_mapping") && tools.iter().any(|name| name == "run_python"), "{tools:?}");
            // Switched off between two kernels: the very next one reads the switch again.
            switch.set(WillingtonSwitch::Off);
            let off_again = again().await;
            assert!(
                off_again.contains("/willington turns the bindings on") && !off_again.contains("map with map_modulator"),
                "{off_again}"
            );
            session.close().await.unwrap();
        })
        .await;
}
