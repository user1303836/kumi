//! The plugin host's guarded undo (LOW 4): Live's integration keeps what Live read back of each value a change set,
//! and its undo reads each again right before Live is asked, refusing one the producer changed by hand since.

use async_trait::async_trait;
use futures::FutureExt;
use kumi_agent::core::context::HostPolicy;
use kumi_agent::core::contracts::{Integration, UndoRefusal};
use kumi_agent::mcp::client::{McpEndpoint, StderrStatus};
use kumi_agent::mcp::types::{CallToolResult, Implementation, ListToolsResult};
use kumi_common::abort::Signal;
use kumi_daw_ableton::{integration::Ableton, options::AbletonOptions};
use kumi_types::contracts::{ChangeState, JsonObject};
use kumi_types::errors::RuntimeError;
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

const TRACK: &str = "7:track:0";
const DEVICE: &str = "7:device:0:0";
const NAMES: [&str; 2] = ["Frequency", "Drive"];

fn parameter(index: usize) -> String {
    format!("7:parameter:{DEVICE}:{index}")
}

/// A value as Live keeps it: a float32.
fn live(value: f64) -> f64 {
    value as f32 as f64
}

/// A small Set: its tempo, one track's mixer (two sends) and one device's two parameters, which a test can change as a
/// producer's hand would. Every undo Live is asked for is counted.
#[derive(Default)]
struct Set {
    tempo: Cell<f64>,
    volume: Cell<f64>,
    pan: Cell<f64>,
    sends: RefCell<Vec<f64>>,
    parameters: RefCell<Vec<f64>>,
    /// Each preview's transaction: what it would set.
    previewed: RefCell<HashMap<String, Value>>,
    /// Each applied transaction: what was there before.
    applied: RefCell<HashMap<String, Value>>,
    undos: Cell<usize>,
    next: Cell<usize>,
}

impl Set {
    fn new() -> Rc<Set> {
        let set = Set::default();
        set.tempo.set(120.);
        set.volume.set(live(0.85));
        set.pan.set(0.);
        *set.sends.borrow_mut() = vec![0., 0.];
        *set.parameters.borrow_mut() = vec![live(0.5), live(0.2)];
        Rc::new(set)
    }
    fn transaction(&self, kind: &str) -> String {
        self.next.set(self.next.get() + 1);
        format!("{kind}{}", self.next.get())
    }
    fn mixer(&self) -> Value {
        let sends = self.sends.borrow().clone();
        json!({"volume": live(self.volume.get()), "pan": live(self.pan.get()), "sends": sends.iter().map(|s| live(*s)).collect::<Vec<_>>(),
            "mute": false, "solo": false, "volumeDisplay": format!("{:.1} dB", self.volume.get() * 10. - 8.5),
            "panDisplay": if self.pan.get() == 0. { "C".to_owned() } else { format!("{:.0}R", self.pan.get() * 50.) },
            "sendDisplays": sends.iter().map(|s| format!("{:.1} dB", s * 10.)).collect::<Vec<_>>()})
    }
    fn shown(value: f64) -> String {
        format!("{:.0} Hz", value * 1000.)
    }
    fn discover(&self, args: &JsonObject) -> Value {
        let kind = args["kind"].as_str().unwrap();
        let items = match kind {
            "set" => vec![json!({"ref": "7:set:song", "objectIdentity": "song", "tempo": live(self.tempo.get())})],
            "track" => vec![json!({"ref": TRACK, "name": "Drums", "mixer": self.mixer()})],
            "parameter" => self
                .parameters
                .borrow()
                .iter()
                .enumerate()
                .map(|(index, value)| {
                    json!({"ref": parameter(index), "parentRef": DEVICE, "name": NAMES[index], "value": live(*value),
                        "displayValue": Set::shown(*value), "min": 0, "max": 1})
                })
                .collect(),
            _ => vec![],
        };
        json!({"epoch": 7, "kind": kind, "items": items, "truncated": false, "revision": "r1"})
    }
    fn answer(&self, name: &str, args: &JsonObject) -> Value {
        match name {
            "live_status" => json!({"connected": true, "adapter": "real-live", "epoch": 7}),
            "live_discover" => self.discover(args),
            "live_tempo_preview" => {
                let id = self.transaction("t");
                self.previewed.borrow_mut().insert(id.clone(), json!({"tempo": args["tempo"]}));
                json!({"transactionId": id, "epoch": 7, "confirmation": "apply", "target": "7:set:song",
                    "priorTempo": live(self.tempo.get()), "proposedTempo": args["tempo"]})
            }
            "live_mixer_preview" => {
                let id = self.transaction("m");
                let mixer = self.mixer();
                let asked: JsonObject = args.iter().filter(|(key, _)| *key != "trackRef").map(|(k, v)| (k.clone(), v.clone())).collect();
                let prior: JsonObject = asked.keys().map(|key| (key.clone(), mixer[key].clone())).collect();
                let shown: JsonObject = asked
                    .keys()
                    .filter_map(|key| {
                        let display = match key.as_str() {
                            "volume" => "volumeDisplay",
                            "pan" => "panDisplay",
                            "sends" => "sendDisplays",
                            _ => return None,
                        };
                        Some((key.clone(), mixer[display].clone()))
                    })
                    .collect();
                self.previewed.borrow_mut().insert(id.clone(), Value::Object(asked.clone()));
                json!({"transactionId": id, "epoch": 7, "trackRef": TRACK, "prior": prior, "priorDisplay": shown, "proposed": asked,
                    "confirmation": "apply"})
            }
            "live_device_parameter_preview" => {
                let id = self.transaction("p");
                let values = self.parameters.borrow().clone();
                let index = |reference: &Value| (0..2).find(|i| parameter(*i) == reference.as_str().unwrap()).unwrap();
                let row = |reference: &Value, asked: &Value| {
                    let at = index(reference);
                    json!({"ref": reference, "name": NAMES[at], "currentValue": live(values[at]), "proposedValue": asked,
                        "displayValue": Set::shown(values[at]), "min": 0, "max": 1})
                };
                let (asked, body) = match args.get("parameters") {
                    Some(rows) => {
                        let asked: Vec<Value> =
                            rows.as_array().unwrap().iter().map(|r| json!([index(&r["parameterRef"]), r["value"]])).collect();
                        let shown: Vec<Value> = rows.as_array().unwrap().iter().map(|r| row(&r["parameterRef"], &r["value"])).collect();
                        (asked, json!({"parameters": shown}))
                    }
                    None => (
                        vec![json!([index(&args["parameterRef"]), args["value"]])],
                        json!({"parameter": row(&args["parameterRef"], &args["value"])}),
                    ),
                };
                self.previewed.borrow_mut().insert(id.clone(), json!({"parameters": asked}));
                let mut body = body;
                body["transactionId"] = json!(id);
                body["epoch"] = json!(7);
                body["confirmation"] = json!("apply");
                body["device"] = json!({"ref": DEVICE, "name": "EQ"});
                body
            }
            "live_tempo_apply" | "live_mixer_apply" | "live_device_parameter_apply" => {
                let id = args["transactionId"].as_str().unwrap().to_owned();
                let asked = self.previewed.borrow()[&id].clone();
                let before = json!({"tempo": self.tempo.get(), "volume": self.volume.get(), "pan": self.pan.get(),
                    "sends": self.sends.borrow().clone(), "parameters": self.parameters.borrow().clone()});
                self.applied.borrow_mut().insert(id.clone(), before);
                if let Some(tempo) = asked.get("tempo").and_then(Value::as_f64) {
                    self.tempo.set(tempo);
                    return json!({"transactionId": id, "state": "applied", "tempo": live(tempo)});
                }
                if let Some(rows) = asked.get("parameters").and_then(Value::as_array) {
                    for row in rows {
                        self.parameters.borrow_mut()[row[0].as_u64().unwrap() as usize] = row[1].as_f64().unwrap();
                    }
                    let at = rows[0][0].as_u64().unwrap() as usize;
                    // A batch answers what it asked for, not what Live kept.
                    let value = if rows.len() == 1 { json!(live(self.parameters.borrow()[at])) } else { rows[0][1].clone() };
                    return json!({"transactionId": id, "state": "applied", "value": value});
                }
                if let Some(volume) = asked.get("volume").and_then(Value::as_f64) {
                    self.volume.set(volume);
                }
                if let Some(pan) = asked.get("pan").and_then(Value::as_f64) {
                    self.pan.set(pan);
                }
                if let Some(sends) = asked.get("sends").and_then(Value::as_array) {
                    for (at, send) in sends.iter().enumerate() {
                        self.sends.borrow_mut()[at] = send.as_f64().unwrap();
                    }
                }
                json!({"transactionId": id, "state": "applied", "display": {"volume": self.mixer()["volumeDisplay"]}})
            }
            "live_undo" => {
                let id = args["transactionId"].as_str().unwrap().to_owned();
                self.undos.set(self.undos.get() + 1);
                let before = self.applied.borrow()[&id].clone();
                self.tempo.set(before["tempo"].as_f64().unwrap());
                self.volume.set(before["volume"].as_f64().unwrap());
                self.pan.set(before["pan"].as_f64().unwrap());
                *self.sends.borrow_mut() = serde_json::from_value(before["sends"].clone()).unwrap();
                *self.parameters.borrow_mut() = serde_json::from_value(before["parameters"].clone()).unwrap();
                json!({"transactionId": id, "state": "undone"})
            }
            "live_run_python" => self.python(args["code"].as_str().unwrap()),
            _ => json!({}),
        }
    }
    /// The fast path's scripts, as Live runs them: setting parameters, reading them, and putting them back.
    fn python(&self, code: &str) -> Value {
        let line = code.lines().find_map(|line| line.strip_prefix("ARGS = json.loads(")).unwrap();
        let given: Value = serde_json::from_str(&serde_json::from_str::<String>(line.strip_suffix(')').unwrap()).unwrap()).unwrap();
        let index = |t: &Value| (0..2).find(|i| Some(parameter(*i).as_str()) == t["ref"].as_str()).unwrap();
        let same = |a: f64, b: f64| (a - b).abs() <= 1e-6 * 1f64.max(a.abs()).max(b.abs());
        let result = match code.lines().next().unwrap() {
            "# kumi:fast-set" => {
                let mut rows = Vec::new();
                for t in given.as_array().unwrap() {
                    let at = index(t);
                    let prior = live(self.parameters.borrow()[at]);
                    self.parameters.borrow_mut()[at] = t["value"].as_f64().unwrap();
                    let now = live(self.parameters.borrow()[at]);
                    rows.push(json!({"name": NAMES[at], "prior": prior, "priorDisplay": Set::shown(prior), "min": 0., "max": 1.,
                        "applied": now, "value": now, "display": Set::shown(now)}));
                }
                json!({"device": "EQ", "track": null, "items": rows})
            }
            "# kumi:fast-read" => json!(given
                .as_array()
                .unwrap()
                .iter()
                .map(|t| {
                    let value = live(self.parameters.borrow()[index(t)]);
                    json!({"value": value, "display": Set::shown(value)})
                })
                .collect::<Vec<_>>()),
            "# kumi:fast-revert" => {
                self.undos.set(self.undos.get() + 1);
                let (mut back, mut moved) = (0, Vec::new());
                for t in given.as_array().unwrap().iter().rev() {
                    let at = index(t);
                    if !same(live(self.parameters.borrow()[at]), t["applied"].as_f64().unwrap()) {
                        moved.push(NAMES[at]);
                        continue;
                    }
                    self.parameters.borrow_mut()[at] = t["prior"].as_f64().unwrap();
                    back += 1;
                }
                json!({"back": back, "moved": moved, "gone": []})
            }
            other => panic!("unexpected script {other}"),
        };
        json!({"ok": true, "result": result, "stdout": "", "error": null})
    }
}

struct Bridge {
    set: Rc<Set>,
    fast: bool,
}

#[async_trait(?Send)]
impl McpEndpoint for Bridge {
    fn pid(&self) -> Option<u32> {
        None
    }
    fn server_info(&self) -> Option<Implementation> {
        Some(serde_json::from_value(json!({"name": "fixture", "version": "1.0.99"})).unwrap())
    }
    async fn list(&self, _: Option<&str>, _: Signal) -> Result<ListToolsResult, RuntimeError> {
        let mut names = vec![
            "live_status",
            "live_discover",
            "live_undo",
            "live_transaction_release",
            "live_tempo_preview",
            "live_tempo_apply",
            "live_mixer_preview",
            "live_mixer_apply",
            "live_device_parameter_preview",
            "live_device_parameter_apply",
        ];
        if self.fast {
            names.push("live_run_python");
        }
        let tools: Vec<Value> = names.iter().map(|name| json!({"name": name, "inputSchema": {"type": "object"}})).collect();
        Ok(serde_json::from_value(json!({ "tools": tools })).unwrap())
    }
    async fn call(&self, name: &str, args: JsonObject, _: Signal) -> Result<CallToolResult, RuntimeError> {
        let body = self.set.answer(name, &args);
        Ok(serde_json::from_value(json!({"content": [{"type": "text", "text": body.to_string()}], "structuredContent": body})).unwrap())
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

/// Live's integration over `set` for a host with `policy`, connected, knowing the Set's refs; `fast`, with Live's Python.
async fn live_on(set: &Rc<Set>, policy: HostPolicy, fast: bool) -> Rc<Ableton> {
    let endpoint: Rc<dyn McpEndpoint> = Rc::new(Bridge { set: set.clone(), fast });
    let mut options = AbletonOptions::new(Rc::new(|_, _| {}));
    options.policy = policy;
    options.connect = Some(Rc::new(move |_| {
        let endpoint = endpoint.clone();
        async move { Ok(endpoint) }.boxed_local()
    }));
    let integration = Ableton::new(options);
    let connection = integration.connection.clone();
    connection.start(Signal::new()).await.unwrap();
    connection.tools().unwrap().refresh(Signal::new()).await.unwrap();
    connection.available.set(true);
    connection.epoch.set(Some(7.));
    let mut book = connection.references.borrow_mut();
    for (reference, kind) in
        [(TRACK.to_owned(), "track"), (DEVICE.to_owned(), "device"), (parameter(0), "parameter"), (parameter(1), "parameter")]
    {
        book.refs.insert(reference, kind.into());
    }
    drop(book);
    integration
}

fn plugin() -> HostPolicy {
    HostPolicy::plugin(std::env::temp_dir().join("kumi-undo-guard-listen"))
}

/// Makes one change through the integration's plan: its id.
async fn change(integration: &Ableton, tool: &str, input: Value) -> String {
    let steps = json!({"steps": [{"tool": tool, "input": input}]});
    let done = integration.mutations.make_changes(steps.as_object().unwrap().clone(), Signal::new()).await.unwrap();
    assert!(!done.is_error, "{}", done.text);
    integration.history.entries.borrow().keys().last().unwrap().clone()
}

fn refused(undone: Result<Result<kumi_types::contracts::ChangeRecord, UndoRefusal>, RuntimeError>) -> UndoRefusal {
    match undone {
        Ok(Err(refusal)) => refusal,
        other => panic!("not refused: {other:?}"),
    }
}

fn state(integration: &Ableton, id: &str) -> ChangeState {
    integration.history.entries.borrow()[id].borrow().record.state
}

async fn local<F: std::future::Future<Output = ()>>(test: F) {
    tokio::task::LocalSet::new().run_until(test).await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_tempo_changed_by_hand_since_is_never_undone_and_an_untouched_one_is() {
    local(async {
        let set = Set::new();
        let integration = live_on(&set, plugin(), false).await;
        let id = change(&integration, "set_tempo", json!({"tempo": 124})).await;
        // The producer moves it on in Live.
        set.tempo.set(130.);
        let preview = integration.undo_preview(&id, Signal::new()).await.unwrap().unwrap();
        assert!(preview.text.contains("130 BPM") && preview.text.contains("120 BPM"), "now → as the undo puts it: {preview:?}");
        assert_eq!(preview.refusal.as_ref().map(|r| r.code), Some("changed_since"));
        let refusal = refused(integration.guarded_undo(&id, Signal::new()).await);
        assert_eq!(refusal.code, "changed_since");
        assert_eq!((set.undos.get(), set.tempo.get(), state(&integration, &id)), (0, 130., ChangeState::Applied), "Live wasn't asked");
        // Back where Kumi set it: the undo goes through.
        set.tempo.set(124.);
        assert!(integration.undo_preview(&id, Signal::new()).await.unwrap().unwrap().refusal.is_none());
        let record = integration.guarded_undo(&id, Signal::new()).await.unwrap().unwrap();
        assert_eq!((record.state, set.undos.get(), set.tempo.get()), (ChangeState::Undone, 1, 120.));
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_mixer_is_checked_by_what_live_read_back_and_only_the_sends_it_named() {
    local(async {
        let set = Set::new();
        let integration = live_on(&set, plugin(), false).await;
        // 0.85 comes back from Live as a float32: an untouched fader isn't taken for a hand's.
        let id = change(&integration, "set_mixer", json!({"trackRef": TRACK, "volume": 0.6, "sends": [0.3]})).await;
        set.sends.borrow_mut()[1] = 0.7;
        let record = integration.guarded_undo(&id, Signal::new()).await.unwrap().unwrap();
        assert_eq!((record.state, set.undos.get()), (ChangeState::Undone, 1), "send B wasn't the change's");
        assert_eq!(set.volume.get(), live(0.85));
        // A send it named, moved by hand: refused, with nothing asked of Live.
        let id = change(&integration, "set_mixer", json!({"trackRef": TRACK, "volume": 0.6, "sends": [0.3]})).await;
        set.sends.borrow_mut()[0] = 0.31;
        let refusal = refused(integration.guarded_undo(&id, Signal::new()).await);
        assert!(refusal.reason.contains("send A"), "{refusal:?}");
        assert_eq!((set.undos.get(), set.volume.get()), (1, 0.6));
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn one_parameter_changed_since_leaves_the_whole_batch_and_live_is_never_asked() {
    local(async {
        let set = Set::new();
        let integration = live_on(&set, plugin(), false).await;
        // (a batch is Kumi's own change, not a plan's step: a goal's or a tune's)
        let kind = kumi_daw_ableton::changes::CHANGES.iter().find(|kind| kind.tool == "set_device_parameters").unwrap();
        let batch = json!({"deviceRef": DEVICE, "parameters": [{"parameterRef": parameter(0), "value": 0.3}, {"parameterRef": parameter(1), "value": 0.7}]});
        let done = integration.mutations.change(kind, batch.as_object().unwrap().clone(), Signal::new(), true).await;
        assert!(!done.is_error, "{}", done.text);
        let id = integration.history.entries.borrow().keys().last().unwrap().clone();
        set.parameters.borrow_mut()[1] = 0.75;
        let refusal = refused(integration.guarded_undo(&id, Signal::new()).await);
        assert!(refusal.reason.contains("Drive") && !refusal.reason.contains("Frequency"), "{refusal:?}");
        assert_eq!((set.undos.get(), set.parameters.borrow().clone()), (0, vec![0.3, 0.75]), "neither was put back");
        // Untouched (the batch's answer was only what it asked for; Live's read is what counts).
        set.parameters.borrow_mut()[1] = 0.7;
        let record = integration.guarded_undo(&id, Signal::new()).await.unwrap().unwrap();
        assert_eq!((record.state, set.undos.get()), (ChangeState::Undone, 1));
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn the_fast_path_is_read_before_its_revert_runs() {
    local(async {
        let set = Set::new();
        let integration = live_on(&set, plugin(), true).await;
        let id =
            change(&integration, "set_device_parameter", json!({"deviceRef": DEVICE, "parameterRef": parameter(0), "value": 0.9})).await;
        set.parameters.borrow_mut()[0] = 0.4;
        let preview = integration.undo_preview(&id, Signal::new()).await.unwrap().unwrap();
        assert!(preview.text.contains("400 Hz") && preview.text.contains("500 Hz"), "{preview:?}");
        refused(integration.guarded_undo(&id, Signal::new()).await);
        assert_eq!((set.undos.get(), set.parameters.borrow()[0]), (0, 0.4), "the revert didn't run");
        set.parameters.borrow_mut()[0] = 0.9;
        let record = integration.guarded_undo(&id, Signal::new()).await.unwrap().unwrap();
        assert_eq!((record.state, set.undos.get(), set.parameters.borrow()[0]), (ChangeState::Undone, 1, live(0.5)));
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_change_with_nothing_kept_is_refused_and_standalone_undo_is_as_before() {
    local(async {
        // Past the most kept, or made before the guard: nothing to check by, so nothing is undone blind.
        let set = Set::new();
        let integration = live_on(&set, plugin(), false).await;
        let id = change(&integration, "set_tempo", json!({"tempo": 124})).await;
        integration.history.host.guard.forget(&id);
        assert!(integration.undo_preview(&id, Signal::new()).await.unwrap().unwrap().refusal.is_some());
        refused(integration.guarded_undo(&id, Signal::new()).await);
        assert_eq!(set.undos.get(), 0);
        // Standalone Kumi keeps nothing and checks nothing: Live's own undo goes as it did.
        let set = Set::new();
        let integration = live_on(&set, HostPolicy::standalone(), false).await;
        let id = change(&integration, "set_tempo", json!({"tempo": 124})).await;
        set.tempo.set(130.);
        assert_eq!(integration.undo_preview(&id, Signal::new()).await.unwrap(), None);
        let record = integration.guarded_undo(&id, Signal::new()).await.unwrap().unwrap();
        assert_eq!((record.state, set.undos.get()), (ChangeState::Undone, 1));
    })
    .await;
}
