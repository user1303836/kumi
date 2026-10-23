//! The plug-in guide and wavetable tool's model-facing shape.
use crate::core::contracts::JsonObject;
use serde_json::Value;
use std::sync::LazyLock;
pub const PLUGIN_TOOL: &str = "plugin";
static DATA: LazyLock<Value> = LazyLock::new(|| serde_json::from_str(include_str!("assets/plugin-tool.json")).unwrap());
pub static PLUGIN_DESCRIPTION: LazyLock<String> = LazyLock::new(|| DATA["description"].as_str().unwrap().into());
pub static PLUGIN_SCHEMA: LazyLock<JsonObject> = LazyLock::new(|| DATA["schema"].as_object().unwrap().clone());
