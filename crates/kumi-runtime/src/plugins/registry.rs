//! Kumi's knowledge of popular plug-ins, matched to a plug-in device by its name, and set against the
//! plug-in's real parameters in Live: which of its knobs are which, which Live lets Kumi turn (the ones
//! configured in the device), and the rest of its thousands, grouped so the model reads them at a glance.

use std::collections::HashSet;
use std::sync::LazyLock;

use indexmap::IndexMap;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::plugins::adapter::OsPaths;
pub use crate::plugins::adapter::PluginAdapter;
use crate::plugins::adapters::ADAPTERS;

/// The adapter for a plug-in device, by the name Live shows for it; `adapters` defaults to Kumi's own.
pub fn adapter_for<'a>(device_name: &str, adapters: Option<&[&'a PluginAdapter]>) -> Option<&'a PluginAdapter> {
    let name = kumi_common::js::string::trim(device_name);
    let adapters: &[&'a PluginAdapter] = match adapters {
        Some(list) => list,
        None => &ADAPTERS,
    };
    adapters.iter().copied().find(|adapter| adapter.matches(name))
}

/// A parameter as Live has it configured in the device: Kumi can turn these.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExposedParameter {
    pub name: String,
    #[serde(rename = "ref")]
    pub reference: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NameGroup {
    pub group: String,
    pub count: usize,
    pub names: Vec<String>,
}

static WHITESPACE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());
static TRAILING_DIGITS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[0-9]+$").unwrap());

/// Names grouped by how they start ("A Level", "A Pan" … into "A …"), each group with its count and first
/// few names, so thousands read in a few lines.
pub fn group_names(names: &[String], max_groups: usize) -> Vec<NameGroup> {
    let mut groups: IndexMap<String, Vec<String>> = IndexMap::new();
    for name in names {
        let words: Vec<&str> = WHITESPACE.split(kumi_common::js::string::trim(name)).collect();
        let key = if words.len() > 1 {
            words[..if words.len() > 2 { 2 } else { 1 }].join(" ")
        } else {
            TRAILING_DIGITS.replace(name, "").into_owned()
        };
        groups.entry(key).or_default().push(name.clone());
    }
    groups
        .into_iter()
        .take(max_groups)
        .map(|(group, members)| NameGroup { count: members.len(), names: members.into_iter().take(6).collect(), group })
        .collect()
}

/// The guide the model reads: the adapter's knowledge (or none, for a plug-in Kumi doesn't know), each hint
/// matched to the plug-in's real parameter names (marked when Live exposes them), what Kumi can turn now,
/// and the rest of the names, grouped.
pub fn plugin_guide(device: &str, adapter: Option<&PluginAdapter>, all: &[String], exposed: &[ExposedParameter]) -> Map<String, Value> {
    let exposed_names: HashSet<String> = exposed.iter().map(|parameter| parameter.name.to_lowercase()).collect();
    let mark =
        |name: &str| if exposed_names.contains(&name.to_lowercase()) { format!("{name} (Kumi can turn it)") } else { name.to_string() };
    let sections = adapter.map(|adapter| {
        adapter
            .sections
            .iter()
            .map(|section| {
                let parameters: Vec<Value> = section
                    .parameters
                    .iter()
                    .map(|hint| {
                        let live: Vec<&String> = all.iter().filter(|name| hint.matches(name)).take(8).collect();
                        let mut entry = Map::new();
                        entry.insert("role".into(), json!(hint.role));
                        entry.insert("about".into(), json!(hint.about));
                        if live.is_empty() {
                            entry.insert("live".into(), json!("not among this plug-in's names (look in the grouped list)"));
                        } else {
                            entry.insert("live".into(), json!(live.iter().map(|name| mark(name)).collect::<Vec<_>>()));
                        }
                        Value::Object(entry)
                    })
                    .collect();
                json!({ "name": section.name, "about": section.about, "parameters": parameters })
            })
            .collect::<Vec<_>>()
    });
    let hidden: Vec<String> = all.iter().filter(|name| !exposed_names.contains(&name.to_lowercase())).cloned().collect();
    let mut guide = Map::new();
    guide.insert(
        "plugin".into(),
        json!(match adapter {
            Some(adapter) => format!("{} ({})", adapter.name, adapter.vendor),
            None => device.to_string(),
        }),
    );
    match adapter {
        Some(adapter) => guide.insert("overview".into(), json!(adapter.overview)),
        None => guide.insert(
            "note".into(),
            json!("Kumi has no notes on this plug-in: go by its parameter names, its manual (read_web), and listening."),
        ),
    };
    guide.insert(
        "canTurn".into(),
        json!(exposed
            .iter()
            .map(|parameter| {
                let mut entry = Map::new();
                entry.insert("name".into(), json!(parameter.name));
                entry.insert("ref".into(), json!(parameter.reference));
                if let Some(display) = parameter.display.as_deref().filter(|display| !display.is_empty()) {
                    entry.insert("now".into(), json!(display));
                }
                Value::Object(entry)
            })
            .collect::<Vec<_>>()),
    );
    guide.insert(
        "parameters".into(),
        json!({ "total": all.len(), "canTurn": exposed.len(), "notConfigured": hidden.len(), "groups": group_names(&hidden, 60) }),
    );
    if let Some(sections) = sections {
        guide.insert("sections".into(), Value::Array(sections));
    }
    if let Some(adapter) = adapter {
        guide.insert("recipes".into(), json!(adapter.recipes));
        guide.insert("beyond".into(), json!(adapter.beyond));
    }
    if !hidden.is_empty() {
        guide.insert("toTurnMore".into(), json!("Live lets Kumi turn only the parameters configured in the device. To add some: the producer clicks Configure in the plug-in's title bar and moves those knobs in its window once (Live keeps them with the Set; Save as Default Configuration in the title bar's menu keeps them for every new one). Kumi can open the plug-in's window with set_device_details (isEditorOpen)."));
    }
    if let Some(wavetable) = adapter.and_then(|adapter| adapter.wavetable) {
        let name = adapter.map(|adapter| adapter.name).unwrap_or_default();
        guide.insert(
            "wavetables".into(),
            json!(format!(
                "{name} reads wavetables ({} samples a frame, up to {} frames): plugin with action wavetable makes one into its folder.",
                wavetable.frame, wavetable.max_frames
            )),
        );
    }
    guide
}

/// A folder from an adapter's "~/…" path, for this computer; None when the adapter has none for this OS.
pub fn folder_for(paths: Option<&OsPaths>) -> Option<String> {
    let path = if cfg!(windows) {
        paths.and_then(|paths| paths.windows)
    } else if cfg!(target_os = "macos") {
        paths.and_then(|paths| paths.mac)
    } else {
        None
    }?;
    if path.is_empty() {
        return None;
    }
    Some(match path.strip_prefix('~') {
        Some(rest) => crate::library::sources::join(&crate::library::sources::homedir(), rest),
        None => path.to_string(),
    })
}
