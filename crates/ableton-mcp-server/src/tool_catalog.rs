//! Declarative MCP schemas, negotiated availability, and deployment policy.
//! Embedded schemas share the reference's data; evaluation is entirely native.

use crate::live::LiveStatus;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    sync::LazyLock,
};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolAvailabilityPrereq {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub always: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub never: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provenance: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capabilities_all: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capabilities_any: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operations_all: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operations_any: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mutation_available: Option<bool>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCatalogEntry {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub annotations: Value,
    pub local: bool,
    pub policy_class: String,
    pub prereq: ToolAvailabilityPrereq,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolPolicyProfile {
    pub classes: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include: Option<Vec<String>>,
    pub description: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AvailabilityRule {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
    pub prereq: ToolAvailabilityPrereq,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PolicyRule {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
    pub policy_class: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CatalogData {
    catalog: Vec<ToolCatalogEntry>,
    classes: Vec<String>,
    profiles: HashMap<String, ToolPolicyProfile>,
    availability_rules: Vec<AvailabilityRule>,
    policy_rules: Vec<PolicyRule>,
}
static DATA: LazyLock<CatalogData> =
    LazyLock::new(|| serde_json::from_str(include_str!("tool-catalog-data.json")).expect("embedded tool catalog"));
pub static TOOL_CATALOG: LazyLock<Vec<ToolCatalogEntry>> = LazyLock::new(|| {
    let mut seen = HashSet::new();
    DATA.catalog
        .iter()
        .map(|entry| {
            assert!(!entry.name.is_empty() && seen.insert(&entry.name), "tool catalog name is invalid: {}", entry.name);
            let availability = resolve_rule(&DATA.availability_rules, &entry.name, |r| (r.name.as_deref(), r.prefix.as_deref()))
                .expect("tool catalog availability rule is missing");
            let policy = resolve_rule(&DATA.policy_rules, &entry.name, |r| (r.name.as_deref(), r.prefix.as_deref()))
                .expect("tool catalog policy class is missing");
            let mut entry = entry.clone();
            entry.local = !entry.name.starts_with("live_");
            entry.prereq = availability.prereq.clone();
            entry.policy_class = policy.policy_class.clone();
            entry
        })
        .collect()
});
pub static TOOL_POLICY_CLASSES: LazyLock<Vec<String>> = LazyLock::new(|| DATA.classes.clone());
pub static TOOL_POLICY_PROFILES: LazyLock<HashMap<String, ToolPolicyProfile>> = LazyLock::new(|| DATA.profiles.clone());
pub static TOOL_AVAILABILITY_RULES: LazyLock<Vec<AvailabilityRule>> = LazyLock::new(|| DATA.availability_rules.clone());
pub static TOOL_POLICY_RULES: LazyLock<Vec<PolicyRule>> = LazyLock::new(|| DATA.policy_rules.clone());

fn resolve_rule<'a, T>(rules: &'a [T], name: &str, fields: impl Fn(&T) -> (Option<&str>, Option<&str>)) -> Option<&'a T> {
    let mut best = None;
    let mut length = 0;
    for rule in rules {
        let (exact, prefix) = fields(rule);
        if let Some(exact) = exact {
            if exact == name {
                return Some(rule);
            }
            continue;
        }
        if let Some(prefix) = prefix {
            if name.starts_with(prefix) && (best.is_none() || prefix.len() > length) {
                best = Some(rule);
                length = prefix.len();
            }
        }
    }
    best
}
pub fn tool_catalog_entry(name: &str) -> Option<&'static ToolCatalogEntry> {
    TOOL_CATALOG.iter().find(|e| e.name == name)
}

pub fn live_mutation_available(status: &LiveStatus) -> bool {
    const CAPABILITIES: &[&str] = &[
        "session.structure",
        "session.midi_clip.create",
        "session.midi_note.write",
        "arrangement.write",
        "audio",
        "audio.capture.resampling",
        "automation",
        "device.parameter.write",
        "devices",
        "browser",
        "routing",
        "recording",
        "mixing",
        "transport",
        "realtime.events",
    ];
    const OPERATIONS: &[&str] = &[
        "transport.set",
        "tempo.set",
        "session.audition-launch",
        "session.audition-stop",
        "session.emergency-stop",
        "session.clip-launch",
        "session.clip-stop",
        "session.capture-midi",
        "scene.capture",
        "track.create",
        "scene.create",
        "clip.create",
        "note.update",
        "note.delete",
        "clip.duplicate",
        "clip.move",
        "arrangement.clip.create",
        "arrangement.clip.move",
        "audio.clip.set",
        "mixer.set",
        "automation.envelope.create",
        "automation.envelope.delete",
        "automation.point.insert",
        "automation.point.delete",
        "browser.load",
        "device.insert",
        "device.enable",
        "device.move",
        "device.parameter.set",
        "routing.set",
        "recording.session",
        "recording.arrangement",
        "realtime.arm",
        "realtime.disarm",
        "locator.add",
    ];
    status.capabilities.iter().any(|c| CAPABILITIES.contains(&c.as_str())) && OPERATIONS.iter().any(|op| status.has_operation(op))
}
pub fn tool_executable(entry: &ToolCatalogEntry, status: &LiveStatus) -> bool {
    let p = &entry.prereq;
    if p.always == Some(true) {
        return true;
    }
    if p.never == Some(true) || !status.connected {
        return false;
    }
    if p.provenance.as_deref().is_some_and(|required| status.provenance.as_ref().map(|p| p.as_str()) != Some(required)) {
        return false;
    }
    let capabilities: HashSet<_> = status.capabilities.iter().map(|c| c.as_str()).collect();
    let operations: HashSet<_> = status.operations.iter().flatten().map(String::as_str).collect();
    if p.capabilities_all.as_ref().is_some_and(|values| !values.iter().all(|v| capabilities.contains(v.as_str())))
        || p.capabilities_any.as_ref().is_some_and(|values| !values.iter().any(|v| capabilities.contains(v.as_str())))
        || p.operations_all.as_ref().is_some_and(|values| !values.iter().all(|v| operations.contains(v.as_str())))
        || p.operations_any.as_ref().is_some_and(|values| !values.iter().any(|v| operations.contains(v.as_str())))
    {
        return false;
    }
    p.mutation_available != Some(true) || live_mutation_available(status)
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolPolicySpec {
    pub profile: String,
    pub allow: Vec<String>,
    pub deny: Vec<String>,
}
impl Default for ToolPolicySpec {
    fn default() -> Self {
        Self { profile: "full".into(), allow: vec![], deny: vec![] }
    }
}
pub static DEFAULT_TOOL_POLICY: LazyLock<ToolPolicySpec> = LazyLock::new(ToolPolicySpec::default);
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ToolPolicyError(pub String);
pub fn tool_policy_matches(pattern: &str, name: &str) -> bool {
    pattern.strip_suffix('*').map_or_else(|| pattern == name, |prefix| name.starts_with(prefix))
}
/// A tool name, or a prefix ending in its only `*`: an inner `*` (as in `*python*`) would match nothing.
fn valid_tool_pattern(value: &str) -> bool {
    (1..=128).contains(&value.len())
        && value.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'*')
        && value.find('*').is_none_or(|at| at + 1 == value.len())
}
// The reference uses JavaScript's `in` operator, including these inherited keys.
const INHERITED_PROFILE_KEYS: &[&str] = &[
    "constructor",
    "__defineGetter__",
    "__defineSetter__",
    "hasOwnProperty",
    "__lookupGetter__",
    "__lookupSetter__",
    "isPrototypeOf",
    "propertyIsEnumerable",
    "toString",
    "valueOf",
    "__proto__",
    "toLocaleString",
];
pub fn parse_tool_policy_spec(value: Option<&Value>) -> Result<ToolPolicySpec, ToolPolicyError> {
    let Some(value) = value else {
        return Ok(ToolPolicySpec::default());
    };
    let fail = |message: &str| ToolPolicyError(message.into());
    let candidate = value.as_object().ok_or_else(|| fail("tool policy must be an object"))?;
    if candidate.keys().any(|k| !["profile", "allow", "deny"].contains(&k.as_str())) {
        return Err(fail("tool policy has unknown keys"));
    }
    let profile = candidate.get("profile").filter(|v| !v.is_null());
    let profile = match profile {
        None => "full",
        Some(v) => v.as_str().ok_or_else(|| fail("tool policy profile is unknown"))?,
    };
    if !TOOL_POLICY_PROFILES.contains_key(profile) && !INHERITED_PROFILE_KEYS.contains(&profile) {
        return Err(fail("tool policy profile is unknown"));
    }
    let list = |key: &str| -> Result<Vec<String>, ToolPolicyError> {
        let Some(value) = candidate.get(key).filter(|v| !v.is_null()) else {
            return Ok(vec![]);
        };
        let invalid = || fail(&format!("tool policy {key} list is invalid"));
        let values = value.as_array().filter(|v| v.len() <= 256).ok_or_else(invalid)?;
        values.iter().map(|v| v.as_str().filter(|v| valid_tool_pattern(v)).map(str::to_owned).ok_or_else(invalid)).collect()
    };
    Ok(ToolPolicySpec { profile: profile.into(), allow: list("allow")?, deny: list("deny")? })
}
pub fn tool_policy_from_env(env: &HashMap<String, String>) -> Result<ToolPolicySpec, ToolPolicyError> {
    let mut value = serde_json::Map::new();
    if let Some(profile) = env.get("ABLETON_MCP_TOOL_POLICY") {
        value.insert("profile".into(), Value::String(profile.clone()));
    }
    for (key, variable) in [("allow", "ABLETON_MCP_TOOL_ALLOW"), ("deny", "ABLETON_MCP_TOOL_DENY")] {
        let values: Vec<_> = env
            .get(variable)
            .into_iter()
            .flat_map(|v| v.split(','))
            .map(|v| kumi_common::js::string::trim(v).to_owned())
            .filter(|v| !v.is_empty())
            .collect();
        value.insert(key.into(), serde_json::json!(values));
    }
    parse_tool_policy_spec(Some(&Value::Object(value)))
}
pub fn tool_allowed_by_policy(entry: &ToolCatalogEntry, policy: &ToolPolicySpec) -> Result<bool, ToolPolicyError> {
    if policy.deny.iter().any(|p| tool_policy_matches(p, &entry.name)) {
        return Ok(false);
    }
    let profile = TOOL_POLICY_PROFILES
        .get(&policy.profile)
        .ok_or_else(|| ToolPolicyError("Cannot read properties of undefined (reading 'includes')".into()))?;
    let in_profile =
        profile.classes.contains(&entry.policy_class) || profile.include.iter().flatten().any(|p| tool_policy_matches(p, &entry.name));
    Ok(in_profile && (policy.allow.is_empty() || policy.allow.iter().any(|p| tool_policy_matches(p, &entry.name))))
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolVisibilityRow {
    pub entry: &'static ToolCatalogEntry,
    pub executable: bool,
    pub policy_allowed: bool,
    pub visible: bool,
}
pub fn resolve_tool_visibility(status: &LiveStatus, policy: &ToolPolicySpec) -> Result<Vec<ToolVisibilityRow>, ToolPolicyError> {
    TOOL_CATALOG
        .iter()
        .map(|entry| {
            let executable = tool_executable(entry, status);
            let policy_allowed = tool_allowed_by_policy(entry, policy)?;
            Ok(ToolVisibilityRow { entry, executable, policy_allowed, visible: executable && policy_allowed })
        })
        .collect()
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDescriptor {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub annotations: Value,
}
pub fn visible_tool_descriptors(status: &LiveStatus, policy: &ToolPolicySpec) -> Result<Vec<ToolDescriptor>, ToolPolicyError> {
    Ok(resolve_tool_visibility(status, policy)?
        .into_iter()
        .filter(|r| r.visible)
        .map(|r| {
            let mut input_schema = r.entry.input_schema.clone();
            if r.entry.name == "live_willington_device_preview" {
                if let Some(kinds) = &status.willington_kinds {
                    input_schema["properties"]["kind"] = serde_json::json!({"type":"string", "enum":kinds});
                }
            }
            ToolDescriptor {
                name: r.entry.name.clone(),
                description: r.entry.description.clone(),
                input_schema,
                annotations: r.entry.annotations.clone(),
            }
        })
        .collect())
}
