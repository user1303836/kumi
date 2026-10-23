//! Live's plans: the change kinds and actions a step can be, Live's batches and its safety copy. Core runs the plan
//! (`core::plan`).
use super::{
    actions::{ActionKind, ActionSummary, ACTIONS},
    changes::{ChangeKind, CHANGES},
    history::{History, LiveChanges},
    mutations::Mutations,
};
use async_trait::async_trait;
use indexmap::IndexMap;
use kumi_agent::core::plan::{self, ActionOutcome, Batch, ChangeOutcome, PlanHost, StepKind};
pub use kumi_agent::core::plan::{expand_step, Plan, Stop};
use kumi_common::abort::Signal;
use kumi_types::{
    contracts::{ChangeFamily, JsonObject, StreamingCall, ToolResult},
    errors::RuntimeError,
};
use serde_json::{json, Value};
use std::{cell::RefCell, rc::Rc};

/// Actions that start, stop or move what Live plays: a `wait` or a stop after a refused one waits for nothing.
const TRANSPORT: [&str; 5] = ["play", "fire_scene", "launch_clip", "record", "jump_to_locator"];
/// A drum rack's pads loaded together, and a device's parameters set together.
const BATCHES: [Batch; 2] = [
    Batch { tool: "load_sample_to_pad", kind: "load_samples_to_pads", key: "deviceRef", most: 16, what: "pads" },
    Batch { tool: "set_device_parameter", kind: "set_device_parameters", key: "deviceRef", most: 64, what: "parameters" },
];
fn row(value: &Value) -> JsonObject {
    value.as_object().cloned().unwrap_or_default()
}
impl StepKind for ChangeKind {
    fn since(&self) -> Option<&str> {
        self.since.as_deref()
    }
}
impl StepKind for ActionKind {
    fn since(&self) -> Option<&str> {
        self.since.as_deref()
    }
}
impl Mutations {
    pub async fn make_changes(self: &Rc<Self>, input: JsonObject, signal: Signal) -> Result<ToolResult, RuntimeError> {
        plan::make_changes(self, input, signal).await
    }
    pub fn stream_changes(self: &Rc<Self>, signal: Signal, on_start: Rc<dyn Fn()>) -> Box<dyn StreamingCall> {
        plan::stream_changes(self, signal, on_start)
    }
}
#[async_trait(?Send)]
impl PlanHost for Mutations {
    type Changes = LiveChanges;
    type Change = ChangeKind;
    type Action = ActionKind;
    fn history(&self) -> &History {
        &self.parameters.history
    }
    fn names(&self) -> &RefCell<IndexMap<String, String>> {
        &self.names
    }
    fn change_kind(&self, tool: &str) -> Option<&'static ChangeKind> {
        CHANGES.iter().find(|k| k.tool == tool && k.internal != Some(true))
    }
    fn action_kind(&self, tool: &str) -> Option<&'static ActionKind> {
        ACTIONS.iter().find(|k| k.tool == tool)
    }
    fn structural(&self, tool: &str) -> bool {
        CHANGES.iter().any(|kind| kind.tool == tool && kind.family == ChangeFamily::Structure)
    }
    fn moves_transport(&self, tool: &str) -> bool {
        TRANSPORT.contains(&tool)
    }
    fn may_lose_material(&self, tool: &str) -> bool {
        tool.starts_with("delete_") || ["clear_range", "move_clip"].contains(&tool)
    }
    fn supported(&self, since: Option<&str>) -> bool {
        Mutations::supported(self, since)
    }
    fn too_old(&self, since: Option<&str>) -> String {
        Mutations::too_old(self, since)
    }
    fn tempo(&self) -> Option<f64> {
        self.observer.tempo.get()
    }
    fn batches(&self) -> &[Batch] {
        &BATCHES
    }
    fn batch_kind(&self, batch: &Batch) -> &'static ChangeKind {
        CHANGES.iter().find(|kind| kind.tool == batch.kind).unwrap()
    }
    fn batch_offered(&self, batch: &Batch) -> bool {
        let properties = self
            .parameters
            .history
            .connection
            .tools()
            .and_then(|tools| tools.tool(&self.batch_kind(batch).preview))
            .and_then(|tool| tool.input_schema.properties)
            .unwrap_or_default();
        if batch.tool == "load_sample_to_pad" {
            properties
                .get("action")
                .and_then(|v| v.get("enum"))
                .and_then(Value::as_array)
                .is_some_and(|a| a.contains(&json!("load-samples")))
        } else {
            properties.contains_key("values")
        }
    }
    fn batch_input(&self, batch: &Batch, group: &[JsonObject]) -> JsonObject {
        let mut input = row(&json!({"deviceRef":group[0].get("deviceRef").unwrap_or(&Value::Null)}));
        let values: Vec<_> = group
            .iter()
            .map(|item| {
                if batch.tool == "load_sample_to_pad" {
                    let mut pad =
                        row(&json!({"note":item.get("note").unwrap_or(&Value::Null),"sample":item.get("sample").unwrap_or(&Value::Null)}));
                    if item.get("instrument").and_then(Value::as_str) == Some("Drum Sampler") {
                        pad.insert("instrument".into(), json!("Drum Sampler"));
                    }
                    json!(pad)
                } else {
                    let mut param = JsonObject::new();
                    if item.get("parameter").is_some_and(Value::is_string) && !item.contains_key("parameterRef") {
                        param.insert("parameter".into(), item["parameter"].clone());
                    } else {
                        param.insert("parameterRef".into(), item.get("parameterRef").cloned().unwrap_or(Value::Null));
                    }
                    param.insert("value".into(), item.get("value").cloned().unwrap_or(Value::Null));
                    json!(param)
                }
            })
            .collect();
        input.insert(if batch.tool == "load_sample_to_pad" { "pads" } else { "values" }.into(), json!(values));
        input
    }
    async fn change(&self, kind: &'static ChangeKind, input: JsonObject, signal: Signal, confirmed: bool) -> ChangeOutcome {
        Mutations::change(self, kind, input, signal, confirmed).await
    }
    async fn act(&self, action: &'static ActionKind, input: JsonObject, signal: Signal) -> ActionOutcome {
        Mutations::act(self, action, input, signal, false).await
    }
    async fn safety_copy(&self, signal: Signal) -> Result<Option<String>, RuntimeError> {
        self.keep_copy(signal).await
    }
    fn emit_action(&self, done: &ActionSummary) {
        Mutations::emit_action(self, done)
    }
}
