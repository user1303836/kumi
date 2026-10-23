//! Live menu command shapes and matching, independent of the live integration.
use crate::{core::contracts::JsonObject, hands::MenuItem};
use kumi_common::js::string::trim;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::LazyLock;
pub const LIVE_COMMAND_TOOL: &str = "live_command";
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Target {
    None,
    Track,
    Tracks,
    Clip,
    TrackOrClip,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Command {
    pub titles: Vec<String>,
    pub target: Target,
    pub done: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dialog: Option<bool>,
}
static DATA: LazyLock<Value> = LazyLock::new(|| serde_json::from_str(include_str!("assets/live-command.json")).unwrap());
pub static COMMANDS: LazyLock<indexmap::IndexMap<String, Command>> = LazyLock::new(|| {
    DATA["commands"].as_object().unwrap().iter().map(|(key, value)| (key.clone(), serde_json::from_value(value.clone()).unwrap())).collect()
});
pub static LIVE_COMMAND_DESCRIPTION: LazyLock<String> = LazyLock::new(|| DATA["description"].as_str().unwrap().into());
pub static LIVE_COMMAND_SCHEMA: LazyLock<JsonObject> = LazyLock::new(|| DATA["schema"].as_object().unwrap().clone());
fn norm(title: &str) -> String {
    static PLURAL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?-u:\b)(track|clip|scene)s(?-u:\b)").unwrap());
    let title = title.strip_suffix('…').or_else(|| title.strip_suffix("...")).unwrap_or(title);
    PLURAL.replace_all(&trim(title).to_lowercase(), "$1").into_owned()
}
pub fn find_item<'a>(items: &'a [MenuItem], titles: &[String]) -> Option<&'a MenuItem> {
    for title in titles {
        let wanted = norm(title);
        if let Some(item) = items.iter().find(|item| norm(item.path.last().map(String::as_str).unwrap_or("")) == wanted) {
            return Some(item);
        }
    }
    for title in titles {
        let wanted = norm(title);
        if let Some(item) = items.iter().find(|item| norm(item.path.last().map(String::as_str).unwrap_or("")).starts_with(&wanted)) {
            return Some(item);
        }
    }
    None
}
pub fn shortcut(item: &MenuItem) -> Option<String> {
    let key = item.key.as_ref().filter(|s| !s.is_empty())?;
    // Windows' menus write their shortcut out ("Ctrl+N").
    if item.modifiers.is_none() && key.chars().count() > 1 {
        return Some(key.clone());
    }
    let modifiers = item.modifiers.filter(|n| n.is_finite()).unwrap_or(0.0).trunc().rem_euclid(4294967296.0) as u32;
    Some(format!(
        "{}{}{}{}{key}",
        if modifiers & 4 != 0 { "⌃" } else { "" },
        if modifiers & 2 != 0 { "⌥" } else { "" },
        if modifiers & 1 != 0 { "⇧" } else { "" },
        if modifiers & 8 != 0 { "" } else { "⌘" }
    ))
}
