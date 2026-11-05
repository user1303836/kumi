//! Validation of the native version-one proof for the two controlled Region Inspector values.
//! This grants a public restore capability, never locates or selects a native entity.
use serde_json::{json, Value};

pub(crate) const MANUAL: &str = "The saved region change has no complete observable identity. Restore that property manually in Logic Pro.";
const LIMIT: usize = 256 * 1024;

fn marker(value: &Value, property: &str) -> bool {
    *value == json!({"controlled":property})
}
fn plain(value: &Value) -> bool {
    match value {
        Value::Array(values) => values.iter().all(plain),
        Value::Object(fields) => !fields.contains_key("controlled") && fields.values().all(plain),
        _ => true,
    }
}
fn semantic(node: &Value) -> bool {
    let Some(fields) = node.as_object() else { return false };
    fields.keys().all(|key| ["role", "subrole", "id", "title", "desc", "value", "valueDesc", "help", "children"].contains(&key.as_str()))
        && node["role"].as_str().is_some_and(|role| !role.is_empty())
        && node.get("value").is_some_and(plain)
        && ["subrole", "id", "title", "desc", "valueDesc", "help"].iter().all(|key| node.get(*key).is_none_or(Value::is_string))
        && node["children"].as_array().is_some_and(|children| children.iter().all(semantic))
}
fn position(value: &Value) -> Option<[u64; 4]> {
    let fields = ["bar", "beat", "division", "tick"].map(|key| value[key].as_u64().filter(|n| (1..=1_000_000).contains(n)));
    let [Some(bar), Some(beat), Some(division), Some(tick)] = fields else { return None };
    Some([bar, beat, division, tick])
}
fn bounds(help: &str) -> Option<([u64; 4], [u64; 4], &str)> {
    let text = help.strip_prefix("Region starts at ")?;
    let (start, rest) = text.split_once(" and ends at ")?;
    let (end, kind) = rest.split_once(',')?;
    let kind = match kind.split_once('.')?.0.trim() {
        "MIDI region" => "midi",
        "audio region" => "audio",
        _ => return None,
    };
    let parse = |text: &str| {
        let words: Vec<_> = text.split_whitespace().collect();
        if words.is_empty() || words.len() % 2 != 0 {
            return None;
        }
        let mut result = [1; 4];
        let mut previous = None;
        for pair in words.chunks_exact(2) {
            let index = match pair[1] {
                "bar" | "bars" => 0,
                "beat" | "beats" => 1,
                "division" | "divisions" => 2,
                "tick" | "ticks" => 3,
                _ => return None,
            };
            if previous.is_none() && index != 0 || previous.is_some_and(|last| index <= last) {
                return None;
            }
            result[index] = pair[0].parse::<u64>().ok().filter(|n| (1..=1_000_000).contains(n))?;
            previous = Some(index);
        }
        Some(result)
    };
    let (start, end) = (parse(start)?, parse(end)?);
    (start < end).then_some((start, end, kind))
}
fn name_field(header: &Value) -> Option<String> {
    let fields: Vec<_> = header["children"].as_array()?.iter().filter(|node| node["role"] == "AXTextField").collect();
    let [field] = fields.as_slice() else { return None };
    field["desc"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| field["value"].as_str().filter(|s| !s.trim().is_empty()))
        .map(str::to_owned)
}
fn restore_name(node: &mut Value, name: &str) {
    for key in ["desc", "title", "value", "valueDesc"] {
        if marker(&node[key], "name") {
            node[key] = json!(name);
        }
    }
}
fn region(item: &Value, target: bool, mode: &str, name: &str, looped: bool) -> bool {
    let mut node = item["node"].clone();
    if target && mode == "rename" {
        if !marker(&node["desc"], "name") {
            return false;
        }
        restore_name(&mut node, name);
        if let Some(children) = node["children"].as_array_mut() {
            for child in children.iter_mut().filter(|child| child["role"] == "AXTextField") {
                restore_name(child, name);
            }
        }
    }
    if target {
        let Some(children) = node["children"].as_array_mut() else { return false };
        let handles: Vec<_> = children
            .iter()
            .enumerate()
            .filter(|(_, child)| child["role"] == "AXHandle" && child["desc"] == "loop")
            .map(|(i, _)| i)
            .collect();
        if handles.len() > 1 {
            return false;
        }
        if let Some(index) = handles.first() {
            let handle = &mut children[*index];
            if mode == "loop" {
                if !handle["value"].is_null() {
                    if !marker(&handle["value"], "loop") {
                        return false;
                    }
                    handle["value"] = json!(looped);
                }
                if handle.get("valueDesc").is_some() {
                    if !marker(&handle["valueDesc"], "loop") {
                        return false;
                    }
                    handle["valueDesc"] = json!("controlled Loop description");
                }
            } else if !handle["value"].is_null()
                && handle["value"].as_bool() != Some(looped)
                && handle["value"].as_f64() != Some(if looped { 1.0 } else { 0.0 })
            {
                return false;
            }
        }
    }
    if item.as_object().is_none_or(|fields| fields.len() != 4) || node["role"] != "AXLayoutItem" || !semantic(&node) {
        return false;
    }
    let Some(region_name) = node["desc"].as_str().filter(|name| !name.trim().is_empty()) else { return false };
    if target && region_name != name {
        return false;
    }
    let Some(children) = node["children"].as_array() else { return false };
    let names: Vec<_> = children.iter().filter(|child| child["role"] == "AXTextField").collect();
    let [field] = names.as_slice() else { return false };
    if field["desc"] != region_name || !field["value"].is_null() && field["value"] != region_name {
        return false;
    }
    let Some((start, end, kind)) = node["help"].as_str().and_then(bounds) else { return false };
    position(&item["start"]) == Some(start) && position(&item["end"]) == Some(end) && item["kind"] == kind
}
fn normalize_region(item: &Value, mode: &str) -> Value {
    let mut item = item.clone();
    if mode == "rename" {
        let name = item["node"]["desc"].as_str().map(str::to_owned);
        if let Some(name) = name {
            fn normalize(node: &mut Value, name: &str) {
                for key in ["desc", "title", "value", "valueDesc"] {
                    if node[key] == name {
                        node[key] = json!({"controlled":"name"});
                    }
                }
            }
            normalize(&mut item["node"], &name);
            if let Some(children) = item["node"]["children"].as_array_mut() {
                for child in children.iter_mut().filter(|child| child["role"] == "AXTextField") {
                    normalize(child, &name);
                }
            }
        }
    }
    if mode == "loop" {
        if let Some(children) = item["node"]["children"].as_array_mut() {
            for child in children.iter_mut().filter(|child| child["role"] == "AXHandle" && child["desc"] == "loop") {
                if !child["value"].is_null() {
                    child["value"] = json!({"controlled":"loop"});
                }
                if child.get("valueDesc").is_some() {
                    child["valueDesc"] = json!({"controlled":"loop"});
                }
            }
        }
    }
    item
}
fn inspector(tree: &Value, mode: &str, name: &str, looped: bool) -> bool {
    let mut tree = tree.clone();
    if tree["role"] != "AXGroup" {
        return false;
    }
    let Some(children) = tree["children"].as_array_mut() else { return false };
    if children
        .iter()
        .filter(|node| node["role"] == "AXStaticText" && node["value"].as_str().is_some_and(|v| v.trim_end_matches(':') == "Region"))
        .count()
        != 1
    {
        return false;
    }
    let names: Vec<_> = children.iter().enumerate().filter(|(_, node)| node["role"] == "AXTextField").map(|(i, _)| i).collect();
    let [name_index] = names.as_slice() else { return false };
    if mode == "rename" {
        if !marker(&children[*name_index]["value"], "name") {
            return false;
        }
        restore_name(&mut children[*name_index], name);
        children[*name_index]["value"] = json!(name);
    } else if children[*name_index]["value"] != name {
        return false;
    }
    fn loop_rows<'a>(node: &'a mut Value, rows: &mut Vec<&'a mut Value>) {
        if node["role"] == "AXRow"
            && node["children"].as_array().is_some_and(|items| {
                items.iter().any(|n| n["role"] == "AXTextField" && n["value"].as_str().is_some_and(|v| v.trim_end_matches(':') == "Loop"))
            })
        {
            rows.push(node);
            return;
        }
        if let Some(children) = node["children"].as_array_mut() {
            for child in children {
                loop_rows(child, rows);
            }
        }
    }
    let mut rows = vec![];
    loop_rows(&mut tree, &mut rows);
    let [row] = rows.as_mut_slice() else { return false };
    let checkboxes: Vec<_> =
        row["children"].as_array().into_iter().flatten().enumerate().filter(|(_, n)| n["role"] == "AXCheckBox").map(|(i, _)| i).collect();
    let [checkbox] = checkboxes.as_slice() else { return false };
    if mode == "loop" {
        if !marker(&row["children"][*checkbox]["value"], "loop") {
            return false;
        }
        row["children"][*checkbox]["value"] = json!(looped);
        if marker(&row["children"][*checkbox]["valueDesc"], "loop") {
            row["children"][*checkbox]["valueDesc"] = json!("controlled Loop description");
        }
    } else {
        let shown = &row["children"][*checkbox]["value"];
        if shown.as_bool() != Some(looped) && shown.as_f64() != Some(if looped { 1.0 } else { 0.0 }) {
            return false;
        }
    }
    drop(rows);
    semantic(&tree)
}

fn strip_kind(strip: &Value, owner: &Value) -> Option<&'static str> {
    let children = strip["children"].as_array()?;
    let names: Vec<_> = children.iter().filter(|node| node["role"] == "AXTextField" && node["desc"] == "name").collect();
    let [name] = names.as_slice() else { return None };
    if name["value"] != owner["name"] || children.iter().any(|node| node["role"] == "AXButton" && node["desc"] == "bounce") {
        return None;
    }
    let mut instrument = false;
    let mut inputs = vec![];
    let mut record = false;
    let mut last_slot = "";
    let mut inserts = vec![];
    let mut midi_slot = false;
    for node in children {
        let desc = node["desc"].as_str().unwrap_or_default();
        let help = node["help"].as_str().unwrap_or_default();
        if node["role"] == "AXButton" {
            if desc == "record" {
                record = true;
            }
            if help.starts_with("Input slot") {
                inputs.push(desc);
            }
            if help.starts_with("Instrument slot") {
                instrument = true;
                last_slot = "instrument";
            } else if help.starts_with("Send slot") || desc == "send button" {
                last_slot = "send";
            } else if help.starts_with("Audio Effect slot") || desc == "audio plug-in" {
                last_slot = "insert";
                inserts.push(false);
            } else if help.starts_with("MIDI Effect slot") || desc == "MIDI plug-in" {
                last_slot = "midi";
                midi_slot = true;
            }
        } else if node["role"] == "AXGroup"
            && last_slot == "insert"
            && node["children"]
                .as_array()
                .is_some_and(|children| children.iter().any(|node| node["role"] == "AXCheckBox" && node["desc"] == "bypass"))
        {
            inserts.push(true);
        }
    }
    instrument |= midi_slot && inserts.last() == Some(&true);
    if instrument {
        Some("instrument")
    } else if inputs.iter().any(|desc| desc.starts_with("Bus")) {
        Some("aux")
    } else if record && !inputs.is_empty() && inputs.iter().all(|desc| !desc.trim().is_empty()) {
        Some("audio")
    } else {
        None
    }
}

pub(crate) fn complete(state: &Value, kind: &str) -> bool {
    let mode = match kind {
        "region-rename" => "rename",
        "region-loop" => "loop",
        _ => return false,
    };
    let Some(name) = state["name"].as_str().filter(|s| !s.trim().is_empty()) else { return false };
    let Some(looped) = state["loop"].as_bool() else { return false };
    if state.as_object().is_none_or(|fields| fields.len() != 3) || serde_json::to_vec(state).map_or(true, |bytes| bytes.len() > LIMIT) {
        return false;
    }
    let proof = &state["identity"];
    if proof.as_object().is_none_or(|fields| fields.len() != 8) || proof["version"] != 1 || proof["mode"] != mode {
        return false;
    }
    let project = &proof["project"];
    let Some(path) = project["path"].as_str().filter(|p| p.starts_with('/') && p.len() <= 1024) else { return false };
    if project.as_object().is_none_or(|fields| fields.len() != 3)
        || project["id"].as_str() != Some(&daw_bridge::auth::sha256_hex(format!("logic-project:{path}").as_bytes())[..32])
        || !project["name"].as_str().is_some_and(|s| !s.is_empty())
    {
        return false;
    }
    let (Some(headers), Some(lanes)) = (proof["headers"].as_array(), proof["lanes"].as_array()) else { return false };
    if headers.is_empty() || headers.len() != lanes.len() || !headers.iter().all(|node| node["role"] == "AXLayoutItem" && semantic(node)) {
        return false;
    }
    let Some(names) = headers.iter().map(name_field).collect::<Option<Vec<_>>>() else { return false };
    let Some(target) = proof["target"].as_u64().and_then(|v| usize::try_from(v).ok()).filter(|n| *n > 0) else { return false };
    let mut at = 0;
    let mut found = None;
    for (lane_index, lane) in lanes.iter().enumerate() {
        if lane.as_object().is_none_or(|fields| fields.len() != 2)
            || lane["node"]["role"] != "AXLayoutArea"
            || !semantic(&lane["node"])
            || lane["node"]["children"] != json!([])
        {
            return false;
        }
        let Some(items) = lane["regions"].as_array() else { return false };
        for item in items {
            at += 1;
            if !region(item, at == target, mode, name, looped) {
                return false;
            }
            if at == target {
                found = Some((lane_index, item));
            }
        }
    }
    let Some((lane_index, target_item)) = found else { return false };
    let owner = &proof["owner"];
    if owner.as_object().is_none_or(|fields| fields.len() != 3)
        || owner["name"] != names[lane_index]
        || names.iter().filter(|name| **name == names[lane_index]).count() != 1
        || !matches!(owner["kind"].as_str(), Some("audio" | "instrument" | "aux"))
        || owner["strip"]["role"] != "AXLayoutItem"
        || !semantic(&owner["strip"])
        || strip_kind(&owner["strip"], owner) != owner["kind"].as_str()
    {
        return false;
    }
    let target_key = normalize_region(target_item, mode);
    if lanes[lane_index]["regions"].as_array().unwrap().iter().filter(|item| normalize_region(item, mode) == target_key).count() != 1 {
        return false;
    }
    inspector(&proof["inspector"], mode, name, looped)
}

pub(crate) fn restorable(kind: &str, prior: &Value, reply: &Value) -> bool {
    let applied = &reply["applied"];
    complete(prior, kind)
        && complete(applied, kind)
        && prior["identity"] == applied["identity"]
        && prior[if kind == "region-rename" { "loop" } else { "name" }] == applied[if kind == "region-rename" { "loop" } else { "name" }]
        && reply.pointer("/material/undo") == Some(&json!({"mode":"value"}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::simulator::logic::LogicScript;

    #[test]
    fn optional_loop_handle_null_is_not_false_and_only_exposed_known_bits_may_be_normalized() {
        let script = LogicScript::new();
        let facts = script.synthetic_region_values();
        assert!(facts[0]["loop"]["identity"]["lanes"][1]["regions"][0]["node"]["children"][1]["value"].is_null());
        for (kind, mode) in [("region-rename", "rename"), ("region-loop", "loop")] {
            let mut state = facts[0][mode].clone();
            let handle = &mut state["identity"]["lanes"][1]["regions"][0]["node"]["children"][1];
            handle["value"] = if mode == "loop" { json!({"controlled":"loop"}) } else { json!(false) };
            handle["valueDesc"] = if mode == "loop" { json!({"controlled":"loop"}) } else { json!("Off") };
            assert!(complete(&state, kind));
            if mode == "loop" {
                state["loop"] = json!(true);
                assert!(complete(&state, kind));
            } else {
                state["identity"]["lanes"][1]["regions"][0]["node"]["children"][1]["value"] = json!(true);
                assert!(!complete(&state, kind), "an exposed handle cannot disagree with the Inspector");
            }
        }
    }

    #[test]
    fn capabilities_require_complete_version_one_facts_and_only_the_controlled_property_may_differ() {
        let script = LogicScript::new();
        let facts = script.synthetic_region_values();
        for (kind, mode, property, wanted) in
            [("region-rename", "rename", "name", json!("Lead")), ("region-loop", "loop", "loop", json!(true))]
        {
            let prior = &facts[0][mode];
            assert!(complete(prior, kind), "the explicit synthetic proof is complete: {mode}");
            let mut after = prior.clone();
            after[property] = wanted;
            let reply = json!({"applied":after,"material":{"undo":{"mode":"value"}}});
            assert!(restorable(kind, prior, &reply));
            for pointer in [
                "/identity/version",
                "/identity/project/path",
                "/identity/headers",
                "/identity/lanes/1/regions/0/start/tick",
                "/identity/owner/strip/children",
                "/identity/inspector/children",
                "/identity/target",
                "/loop",
                "/name",
            ] {
                let mut unknown = prior.clone();
                *unknown.pointer_mut(pointer).unwrap() = Value::Null;
                assert!(!complete(&unknown, kind), "{mode}: {pointer}");
                assert!(!restorable(kind, &unknown, &reply));
            }
            let mut replacement = prior.clone();
            replacement["identity"]["owner"]["strip"]["children"]
                .as_array_mut()
                .unwrap()
                .push(json!({"role":"AXButton","value":"producer plug-in","children":[]}));
            assert!(complete(&replacement, kind));
            assert!(!restorable(kind, &replacement, &reply), "unchanged controlled values do not prove unchanged owner identity");
            let mut oversized = prior.clone();
            oversized["identity"]["owner"]["strip"]["value"] = json!("x".repeat(256 * 1024));
            assert!(!complete(&oversized, kind));
            assert!(!restorable(kind, prior, &json!({"applied":after})), "no legacy hint grants permission");
            let mut duplicate = prior.clone();
            let mut second = duplicate["identity"]["lanes"][1]["regions"][0].clone();
            if mode == "rename" {
                restore_name(&mut second["node"], "Other name");
                restore_name(&mut second["node"]["children"][0], "Other name");
            }
            duplicate["identity"]["lanes"][1]["regions"].as_array_mut().unwrap().push(second);
            assert!(!complete(&duplicate, kind), "same observable material cannot be disambiguated by ordinal or name");
        }
    }
}
