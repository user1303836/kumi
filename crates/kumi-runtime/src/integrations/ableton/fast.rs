//! Atomic parameter scripts run by Live's Python host.
use kumi_common::js::json::stringify;
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FastTarget {
    Ref {
        #[serde(rename = "ref")]
        reference: String,
    },
    Parameter {
        device: String,
        index: usize,
        name: String,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FastFound {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<usize>,
    pub name: String,
    pub min: f64,
    pub max: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub items: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grid: Option<Vec<(f64, String)>>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FastSet {
    pub name: String,
    pub prior: f64,
    pub value: f64,
    pub prior_display: String,
    pub display: String,
    pub min: f64,
    pub max: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FastRevert {
    Ref {
        #[serde(rename = "ref")]
        reference: String,
        prior: f64,
        applied: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    Parameter {
        device: String,
        index: usize,
        name: String,
        prior: f64,
        applied: f64,
    },
}
pub(super) fn with_args(marker: &str, args: &Value, body: &str) -> String {
    format!("# kumi:{marker}\nimport json\nARGS = json.loads({})\n{body}", stringify(&Value::String(stringify(args))))
}
pub fn find_script(items: &Value) -> String {
    with_args("fast-find", items, include_str!("assets/fast-find.py"))
}
pub fn set_script(items: &Value) -> String {
    with_args("fast-set", items, include_str!("assets/fast-set.py"))
}
pub fn revert_script(items: &Value) -> String {
    with_args("fast-revert", items, include_str!("assets/fast-revert.py"))
}
