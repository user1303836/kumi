//! A request taken back in Live: a whole session over Live's own integration, on a bridge that answers the observation
//! oracle's `basic` Set and makes the changes it's asked to. Kumi undoes the request's change through its own records
//! (Live's `live_undo` of that change), stops when Live refuses an undo, and the request stays then.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

use async_trait::async_trait;
use futures::FutureExt;
use kumi_agent::core::contracts::*;
use kumi_agent::core::session_log::SessionLog;
use kumi_agent::kernel::budget::OBSERVATION_MARKER;
use kumi_agent::mcp::client::{McpEndpoint, StderrStatus};
use kumi_agent::mcp::types::{CallToolResult, Implementation, ListToolsResult};
use kumi_agent::{create_conversation_store, create_session, Session, SessionOptions};
use kumi_common::{abort::Signal, js::json::stringify};
use kumi_daw_ableton::integration::Ableton;
use kumi_daw_ableton::options::{AbletonOptions, HandsSetup};
use kumi_types::contracts::*;
use kumi_types::errors::RuntimeError;
use kumi_types::place::Places;
use serde_json::{json, Value};

/// Live's bridge on the fixtures: the fullest public-tools catalog, the `basic` Set's observation, and every change made
/// as asked. Its undo undoes, or refuses with the words a test gives.
struct Bridge {
    catalog: Vec<Value>,
    replies: HashMap<String, Value>,
    next: Cell<usize>,
    undone: Cell<usize>,
    refusals: RefCell<VecDeque<&'static str>>,
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
        Rc::new(Bridge { catalog, replies, next: Cell::new(0), undone: Cell::new(0), refusals: RefCell::default() })
    }
}
fn reply(body: Value) -> CallToolResult {
    serde_json::from_value(json!({"content":[{"type":"text","text":stringify(&body)}],"structuredContent":body})).unwrap()
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
        Ok(serde_json::from_value(json!({ "tools": self.catalog })).unwrap())
    }
    async fn call(&self, name: &str, args: JsonObject, signal: Signal) -> Result<CallToolResult, RuntimeError> {
        signal.check()?;
        if name == "live_undo" {
            if let Some(refused) = self.refusals.borrow_mut().pop_front() {
                return Ok(serde_json::from_value(json!({"content":[{"type":"text","text":refused}],"isError":true})).unwrap());
            }
            self.undone.set(self.undone.get() + 1);
            return Ok(reply(json!({"state":"undone"})));
        }
        if name == "live_undo_step_begin" {
            return Ok(reply(json!({"stepId":"step"})));
        }
        if name == "live_undo_step_end" {
            return Ok(reply(json!({})));
        }
        if name.ends_with("_preview") {
            let transaction = self.next.get();
            self.next.set(transaction + 1);
            return Ok(reply(json!({"epoch":7,"transactionId":format!("tx{transaction}"),"confirmation":"yes"})));
        }
        if name.ends_with("_apply") {
            return Ok(reply(json!({"state":"applied"})));
        }
        let key = format!("{name}:{}", args.get("kind").and_then(Value::as_str).unwrap_or(""));
        let reply = self.replies.get(&key).cloned().ok_or_else(|| RuntimeError::plain(format!("no reply for {key}")))?;
        Ok(serde_json::from_value(reply).unwrap())
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

/// A model that sets the tempo each request names ("tempo 126") with Live's own tool, keeping the conversation.
struct Tempo {
    tools: Vec<Rc<dyn KernelTool>>,
    history: RefCell<Vec<Value>>,
}
#[async_trait(?Send)]
impl Kernel for Tempo {
    async fn run(&self, input: &str, signal: Signal, _: KernelEmit) -> Result<TurnResult, RuntimeError> {
        let words = input.split(OBSERVATION_MARKER).next().unwrap().trim().to_string();
        self.history.borrow_mut().push(json!({"role":"user","content":words}));
        if let Some(bpm) = words.strip_prefix("tempo ").and_then(|bpm| bpm.parse::<f64>().ok()) {
            let tool = self.tools.iter().find(|tool| tool.name() == "set_tempo").expect("Live's set_tempo").clone();
            let done = tool.execute(json!({"tempo": bpm}).as_object().unwrap().clone(), signal).await?;
            assert!(!done.is_error, "{}", done.text);
        }
        Ok(TurnResult { stop_reason: StopReason::Completed, usage: None })
    }
    fn has_checkpoint(&self) -> bool {
        true
    }
    fn checkpoint(&self) -> Result<KernelCheckpoint, RuntimeError> {
        Ok(KernelCheckpoint { version: 1, messages: self.history.borrow().clone(), origin: None, tools: None })
    }
    fn last_turn(&self) -> Vec<Value> {
        self.history.borrow().last().cloned().into_iter().collect()
    }
    async fn close(&self) {}
}

/// Kumi in Live on `bridge`, its conversations and their logs in `dir`. A change reaches the session and the screen
/// (through the log in front of it) as the app has it from the integration.
async fn kumi(bridge: &Rc<Bridge>, dir: &std::path::Path) -> Session {
    let log = SessionLog::new(dir, Places::LIVE, Rc::new(|text: &str| text.to_owned()));
    let emit = SessionLog::tee(&log, Rc::new(|_| {}));
    let watching: Rc<RefCell<Option<Session>>> = Rc::default();
    let (endpoint, told, watcher) = (bridge.clone(), emit.clone(), watching.clone());
    let integration: IntegrationFactory = Box::new(move |listener| {
        let endpoint = endpoint.clone();
        let mut options = AbletonOptions::new(listener);
        options.connect = Some(Rc::new(move |_| {
            let endpoint = endpoint.clone();
            async move { Ok(endpoint as Rc<dyn McpEndpoint>) }.boxed_local()
        }));
        options.hands = Some(HandsSetup::Disabled);
        options.reconnect_interval_ms = Some(3_600_000);
        let (told, watcher) = (told.clone(), watcher.clone());
        options.on_change = Some(Rc::new(move |change: ChangeRecord| {
            let session = watcher.borrow().clone();
            if let Some(session) = session {
                session.watch(WatchEvent::Change(change.clone()));
            }
            told(SessionEvent::Change { change });
        }));
        Ableton::new(options) as Rc<dyn Integration>
    });
    let kernels: KernelFactory = Rc::new(|options: KernelOptions| {
        let history = options.checkpoint.map(|checkpoint| checkpoint.messages).unwrap_or_default();
        async move { Ok(Rc::new(Tempo { tools: options.tools, history: RefCell::new(history) }) as Rc<dyn Kernel>) }.boxed_local()
    });
    let mut options = SessionOptions::new(kernels, integration, emit);
    options.timeout_ms = Some(10_000);
    options.conversations = Some(create_conversation_store(dir));
    options.log = Some(log);
    let session = create_session(options).unwrap();
    *watching.borrow_mut() = Some(session.clone());
    session.start().await.unwrap();
    session
}

#[tokio::test(flavor = "current_thread")]
async fn a_request_taken_back_in_live_is_undone_through_kumis_records_until_live_refuses_an_undo() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let dir = tempfile::tempdir().unwrap();
            let bridge = Bridge::new();
            let session = kumi(&bridge, dir.path()).await;
            session.submit("tempo 126", None).await.unwrap();
            session.submit("tempo 128", None).await.unwrap();
            let plan = session.take_back_plan().await.unwrap();
            let listed: Vec<(&str, usize)> = plan.iter().map(|asked| (asked.words.as_str(), asked.changes.len())).collect();
            assert_eq!(listed, [("tempo 128", 1), ("tempo 126", 1)]);
            // The last request taken back: Live undoes Kumi's change, through its record.
            let taken = session.take_back(plan[0].at).await.unwrap();
            assert_eq!((taken.requests, taken.words.as_deref(), taken.failed.is_none()), (1, Some("tempo 128"), true));
            assert_eq!(taken.undone.len(), 1);
            assert!(taken.undone[0].title.contains("128"), "{}", taken.undone[0].title);
            assert_eq!(bridge.undone.get(), 1);
            // Live refuses the next one (the producer changed the tempo since): Kumi stops there, and the request stays.
            bridge.refusals.borrow_mut().push_back("Undo refused: the tempo was modified after apply.");
            let plan = session.take_back_plan().await.unwrap();
            let taken = session.take_back(plan[0].at).await.unwrap();
            assert_eq!((taken.requests, taken.undone.len()), (0, 0));
            let failed = taken.failed.expect("the undo Live refused");
            assert_eq!(failed.state, ChangeState::Kept);
            assert_eq!(bridge.undone.get(), 1);
            let left = session.take_back_plan().await.unwrap();
            assert_eq!(left.iter().map(|asked| asked.words.as_str()).collect::<Vec<_>>(), ["tempo 126"]);
            session.close().await.unwrap();
        })
        .await;
}
