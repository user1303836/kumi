use async_trait::async_trait;
use futures::FutureExt;
use kumi_common::{
    abort::{Signal, SignalExt},
    js::json::stringify,
};
use kumi_runtime::{
    core::{contracts::JsonObject, errors::RuntimeError},
    integrations::ableton::{
        changes::CHANGES,
        connection::{ConnectionOptions, LiveConnection},
        history::{FastResult, History, PYTHON_RAN},
        parameters::Parameters,
        remember::Remember,
    },
    mcp::{
        client::{McpEndpoint, StderrStatus},
        types::{CallToolResult, Implementation, ListToolsResult},
    },
};
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
struct Fixture {
    case: Value,
    calls: RefCell<Vec<Value>>,
    original: RefCell<Signal>,
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
        let names = ["live_discover", "live_run_python"];
        Ok(serde_json::from_value(json!({"tools":names.iter().filter(|name|!self.case["config"]["missing"].as_array().is_some_and(|a|a.contains(&json!(name)))).map(|name|json!({"name":name,"inputSchema":{"type":"object"}})).collect::<Vec<_>>()})).unwrap())
    }
    async fn call(&self, name: &str, args: JsonObject, signal: Signal) -> Result<CallToolResult, RuntimeError> {
        signal.check()?;
        let index = self.calls.borrow().len();
        let call = json!({"name":name,"args":args});
        eq(&call, &self.case["calls"][index], &format!("{} dispatch {index}", self.case["label"]));
        self.calls.borrow_mut().push(call);
        let entry = &self.case["responses"][index];
        if entry["cancel"] == true {
            self.original.borrow().cancel();
        }
        if let Some(error) = entry["throw"].as_str() {
            return Err(RuntimeError::plain(error));
        }
        serde_json::from_value(entry["reply"].clone()).map_err(|e| RuntimeError::plain(e.to_string()))
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
    regex::Regex::new(r"\bc\d+\b").unwrap().replace_all(&stringify(&canonical(value)), "<change>").into_owned()
}
fn eq(actual: &Value, expected: &Value, label: &str) {
    // The checkout command changes with the runtime; installed users still invoke `kumi`.
    assert_eq!(normalized(actual), normalized(expected).replace("npm run kumi --", &kumi_runtime::command::KUMI), "{label}")
}
#[tokio::test(flavor = "current_thread")]
async fn a_python_failure_after_the_code_ran_counts_as_sent() {
    // Live may have changed once the code ran; a refusal before it ran changed nothing.
    tokio::task::LocalSet::new()
        .run_until(async {
            for (kind, sent) in [(PYTHON_RAN, true), ("ValueError", false)] {
                let answer = json!({"ok":false,"result":null,"stdout":"","error":{"type":kind,"message":"broken","traceback":""}});
                let case = json!({"label":kind,"config":{},"calls":[{"name":"live_run_python","args":{"code":"x = 1","mode":"exec","timeoutMs":10000}}],
                    "responses":[{"reply":{"content":[{"type":"text","text":stringify(&answer)}]}}]});
                let endpoint = Rc::new(Fixture { case, calls: RefCell::new(Vec::new()), original: RefCell::new(Signal::new()) });
                let mut options = ConnectionOptions::new(Rc::new(|_, _| {}));
                let out = endpoint.clone();
                options.connect = Some(Rc::new(move |_| {
                    let endpoint: Rc<dyn McpEndpoint> = out.clone();
                    async move { Ok(endpoint) }.boxed_local()
                }));
                let connection = LiveConnection::new(options);
                connection.start(Signal::new()).await.unwrap();
                connection.tools().unwrap().refresh(Signal::new()).await.unwrap();
                let history = History::new(connection.clone(), Remember::new(connection.clone(), None, None), None, None);
                match history.run_fast("x = 1".into(), Signal::new()).await.unwrap() {
                    FastResult::Error { sent: got, .. } => assert_eq!(got, sent, "{kind}"),
                    FastResult::Result(_) => panic!("{kind}: taken as done"),
                }
                connection.close().await.unwrap();
            }
        })
        .await;
}
#[tokio::test(flavor = "current_thread")]
async fn parameter_lookup_display_cache_and_atomic_changes_match_source() {
    tokio::task::LocalSet::new().run_until(async{
 let fixture:Value=serde_json::from_str(include_str!("support/parameters-oracle.json")).unwrap();
 for (case_index,original) in fixture["cases"].as_array().unwrap().iter().enumerate(){
  let mut case=original.clone();case["responses"]=json!(case["responses"].as_array().unwrap().iter().map(|id|fixture["values"][id.as_u64().unwrap() as usize].clone()).collect::<Vec<_>>());
  let endpoint=Rc::new(Fixture{case:case.clone(),calls:RefCell::new(Vec::new()),original:RefCell::new(Signal::new())});let config=&case["config"];
  let mut options=ConnectionOptions::new(Rc::new(|_,_|{}));let out=endpoint.clone();options.connect=Some(Rc::new(move |_|{let endpoint:Rc<dyn McpEndpoint>=out.clone();async move{Ok(endpoint)}.boxed_local()}));options.now=Some(Rc::new(||chrono::DateTime::parse_from_rfc3339("2026-10-03T12:00:00Z").unwrap().with_timezone(&chrono::Utc)));
  let connection=LiveConnection::new(options);connection.start(Signal::new()).await.unwrap();connection.tools().unwrap().refresh(Signal::new()).await.unwrap();
  let events=Rc::new(RefCell::new(Vec::<Value>::new()));let out=events.clone();let remember=Remember::new(connection.clone(),None,None);let history=Rc::new(History::new(connection.clone(),remember,None,Some(Rc::new(move|record|out.borrow_mut().push(serde_json::to_value(record).unwrap())))));let parameters=Parameters::new(history.clone(),config["fast"].as_bool());
  for reference in config["shorts"].as_array().into_iter().flatten(){connection.references.borrow_mut().short_ref(reference.as_str().unwrap());}
  for row in config["known"].as_array().into_iter().flatten(){connection.references.borrow_mut().known.insert(row[0].as_str().unwrap().into(),serde_json::from_value(row[1].clone()).unwrap());}
  for (index,operation) in case["operations"].as_array().unwrap().iter().enumerate(){
   let signal=Signal::new();if operation["abort"]==true{signal.cancel();}*endpoint.original.borrow_mut()=signal.clone();if operation["invalidate"]==true{connection.invalidate();}
   let result:Result<Value,RuntimeError>=match operation["op"].as_str(){
    Some("enabled")=>Ok(json!(parameters.fast_on())),
    Some("map")=>parameters.value_for_text(operation["ref"].as_str().unwrap(),operation["text"].as_str().unwrap(),signal).await.map(|v|match v{Ok(value)=>json!(value),Err(text)=>json!(text)}),
    Some("parameters")=>parameters.device_parameters(operation["ref"].clone(),serde_json::from_value(operation["fields"].clone()).unwrap(),signal).await.map(|v|json!(v)),
    _=>{let kind=CHANGES.iter().find(|k|k.tool==operation["tool"].as_str().unwrap_or("set_device_parameter")).unwrap();parameters.fast_parameters(kind,operation["input"].as_object().unwrap(),signal).await.map(|v|serde_json::to_value(v).unwrap())}
   };
   let value=match result{Ok(v)=>v,Err(RuntimeError::Aborted)=>json!({"error":"cancelled"}),Err(e)=>json!({"error":e.to_string()})};
   let changes:Vec<_>=history.entries.borrow().values().map(|entry|serde_json::to_value(&*entry.borrow()).unwrap()).collect();let state=json!({"maps":parameters.display_maps.borrow().iter().collect::<Vec<_>>(),"found":parameters.fast_found.borrow().iter().collect::<Vec<_>>(),"fastGeneration":parameters.fast_generation.get().map(|v|v as i64).unwrap_or(-1),"changes":changes,"changesThisTurn":history.changes_this_turn.get()});
   let label=format!("{} case {case_index} step {index}",case["label"]);eq(&value,&case["results"][index]["value"],&label);eq(&state,&case["results"][index]["state"],&format!("{label} state"));
  }
  eq(&json!(*endpoint.calls.borrow()),&case["calls"],&format!("{} all calls",case["label"]));eq(&json!(*events.borrow()),&case["events"],&format!("{} events",case["label"]));connection.close().await.unwrap();
 }
}).await;
}

/// Live answering each read of a parameter's units with the next device's, in turn.
struct Devices {
    maps: Vec<Value>,
    reads: Cell<usize>,
}
#[async_trait(?Send)]
impl McpEndpoint for Devices {
    fn pid(&self) -> Option<u32> {
        None
    }
    fn server_info(&self) -> Option<Implementation> {
        Some(serde_json::from_value(json!({"name":"fixture","version":"1.0.73"})).unwrap())
    }
    async fn list(&self, _: Option<&str>, _: Signal) -> Result<ListToolsResult, RuntimeError> {
        Ok(serde_json::from_value(json!({"tools":(["live_discover","live_run_python"].iter().map(|name|json!({"name":name,"inputSchema":{"type":"object"}})).collect::<Vec<_>>())})).unwrap())
    }
    async fn call(&self, name: &str, _: JsonObject, _: Signal) -> Result<CallToolResult, RuntimeError> {
        assert_eq!(name, "live_run_python");
        let map = &self.maps[self.reads.get().min(self.maps.len() - 1)];
        self.reads.set(self.reads.get() + 1);
        let payload = json!({"ok":true,"result":map});
        Ok(serde_json::from_value(json!({"content":[{"type":"text","text":stringify(&payload)}],"structuredContent":payload})).unwrap())
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
#[tokio::test(flavor = "current_thread")]
async fn a_device_put_in_anothers_place_has_its_parameters_units_read_again() {
    tokio::task::LocalSet::new()
        .run_until(async {
            // Drive (0–6 dB) at the first parameter's place, then, after the next look at the Set, a device whose first
            // parameter reads 0–100 %: the producer swapped it in, so the same positional ref names it.
            let endpoint = Rc::new(Devices {
                maps: vec![
                    json!({"index":0,"name":"Drive","min":0,"max":1,"items":[],"grid":[[0,"0 dB"],[0.5,"3 dB"],[1,"6 dB"]]}),
                    json!({"index":0,"name":"Dry/Wet","min":0,"max":1,"items":[],"grid":[[0,"0 %"],[0.5,"50 %"],[1,"100 %"]]}),
                ],
                reads: Cell::new(0),
            });
            let mut options = ConnectionOptions::new(Rc::new(|_, _| {}));
            let out = endpoint.clone();
            options.connect = Some(Rc::new(move |_| {
                let endpoint: Rc<dyn McpEndpoint> = out.clone();
                async move { Ok(endpoint) }.boxed_local()
            }));
            let connection = LiveConnection::new(options);
            connection.start(Signal::new()).await.unwrap();
            connection.tools().unwrap().refresh(Signal::new()).await.unwrap();
            let remember = Remember::new(connection.clone(), None, None);
            let parameters = Parameters::new(Rc::new(History::new(connection.clone(), remember, None, None)), None);
            assert_eq!(parameters.value_for_text("7:parameter:0", "3 dB", Signal::new()).await.unwrap(), Ok(0.5));
            assert_eq!(parameters.value_for_text("7:parameter:0", "6 dB", Signal::new()).await.unwrap(), Ok(1.), "read once a look");
            assert_eq!(endpoint.reads.get(), 1);
            connection.invalidate();
            assert_eq!(parameters.value_for_text("7:parameter:0", "50 %", Signal::new()).await.unwrap(), Ok(0.5));
            assert_eq!(endpoint.reads.get(), 2);
            connection.close().await.unwrap();
        })
        .await;
}

/// A fake Live behind the bridge: each run_python runs Kumi's script in Python against one device whose parameters
/// keep their values (in `state`, a JSON file) from one call to the next.
struct PythonLive {
    state: std::path::PathBuf,
}
const FAKE_LIVE: &str = r#"
import json, sys
STATE = sys.argv[1]
state = json.load(open(STATE))
class Param:
    def __init__(self, i): self.i = i
    name = property(lambda s: state["params"][s.i]["name"])
    min = property(lambda s: float(state["params"][s.i]["min"]))
    max = property(lambda s: float(state["params"][s.i]["max"]))
    is_enabled = True
    is_quantized = False
    def _get(self): return state["params"][self.i]["value"]
    def _set(self, v): state["params"][self.i]["value"] = float(v)
    value = property(_get, _set)
    def str_for_value(self, v): return "%.3f" % v
    canonical_parent = property(lambda s: Device())
class Track:
    name = "Bass"
class Device:
    name = "Saturator"
    canonical_parent = property(lambda s: Track())
    parameters = property(lambda s: [Param(i) for i in range(len(state["params"]))])
class Refs:
    def get(self, ref):
        if ref == "7:device:0:0": return Device()
        if ref.startswith("7:parameter:0:0:"): return Param(int(ref.rsplit(":", 1)[1]))
        raise KeyError(ref)
class Bridge: refs = Refs()
space = {"bridge": Bridge()}
exec(compile(sys.stdin.read(), "kumi", "exec"), space)
json.dump(state, open(STATE, "w"))
# The bridge hands a Live object back as its ref.
print(json.dumps({"ok": True, "result": space.get("result")}, default=lambda thing: {"ref": "7:track:0"}))
"#;
#[async_trait(?Send)]
impl McpEndpoint for PythonLive {
    fn pid(&self) -> Option<u32> {
        None
    }
    fn server_info(&self) -> Option<Implementation> {
        Some(serde_json::from_value(json!({"name":"fixture","version":"1.0.73"})).unwrap())
    }
    async fn list(&self, _: Option<&str>, _: Signal) -> Result<ListToolsResult, RuntimeError> {
        Ok(serde_json::from_value(json!({"tools":(["live_discover","live_run_python"].iter().map(|name|json!({"name":name,"inputSchema":{"type":"object"}})).collect::<Vec<_>>())})).unwrap())
    }
    async fn call(&self, name: &str, args: JsonObject, _: Signal) -> Result<CallToolResult, RuntimeError> {
        use std::io::Write;
        assert_eq!(name, "live_run_python");
        let mut child = std::process::Command::new("python3")
            .args(["-c", FAKE_LIVE, self.state.to_str().unwrap()])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(args["code"].as_str().unwrap().as_bytes()).unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        let payload: Value = serde_json::from_slice(&output.stdout).unwrap();
        Ok(serde_json::from_value(json!({"content":[{"type":"text","text":stringify(&payload)}],"structuredContent":payload})).unwrap())
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
#[tokio::test(flavor = "current_thread")]
async fn one_change_that_sets_a_parameter_twice_is_undone_to_its_first_value() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let folder = tempfile::tempdir().unwrap();
            let state = folder.path().join("live.json");
            std::fs::write(&state, r#"{"params":[{"name":"Drive","value":0.2,"min":0,"max":1}]}"#).unwrap();
            let value = || serde_json::from_str::<Value>(&std::fs::read_to_string(&state).unwrap()).unwrap()["params"][0]["value"].clone();
            let endpoint = Rc::new(PythonLive { state: state.clone() });
            let mut options = ConnectionOptions::new(Rc::new(|_, _| {}));
            let out = endpoint.clone();
            options.connect = Some(Rc::new(move |_| {
                let endpoint: Rc<dyn McpEndpoint> = out.clone();
                async move { Ok(endpoint) }.boxed_local()
            }));
            let connection = LiveConnection::new(options);
            connection.start(Signal::new()).await.unwrap();
            connection.tools().unwrap().refresh(Signal::new()).await.unwrap();
            let remember = Remember::new(connection.clone(), None, None);
            let history = Rc::new(History::new(connection.clone(), remember, None, None));
            let parameters = Parameters::new(history.clone(), None);
            // Drive by its ref, then by its name: one parameter, set twice in one change.
            let kind = CHANGES.iter().find(|k| k.tool == "set_device_parameters").unwrap();
            let input = json!({"deviceRef":"7:device:0:0","values":[{"parameterRef":"7:parameter:0:0:0","value":0.5},{"parameter":"Drive","value":0.8}]});
            let outcome = parameters.fast_parameters(kind, input.as_object().unwrap(), Signal::new()).await.unwrap();
            assert!(!outcome.is_error, "{}", outcome.text);
            assert_eq!(value(), json!(0.8));
            let change = serde_json::from_str::<Value>(&outcome.text).unwrap()["change"].as_str().unwrap().to_owned();
            let undone = history.undo(&change, Signal::new(), false).await.unwrap();
            assert!(!undone.is_error, "{}", undone.text);
            assert_eq!(value(), json!(0.2), "back to where it was before the change");
            connection.close().await.unwrap();
        })
        .await;
}
