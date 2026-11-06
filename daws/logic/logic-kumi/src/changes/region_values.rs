//! Two nonstructural properties of an already sole-selected region. Refs locate a candidate; complete fresh
//! observable project/track/region/Inspector evidence authorizes each write. No selection, pane, key or Undo action
//! establishes that evidence. Like the M3 value controllers, identical observable replacement remains an ABA limit.

use serde_json::{json, Value};

use super::nudge::{self, Control};
use super::{changed, quoted, ref_text, uncertain, Applied, Ctx, Done, Epoch, Preview, Restore, Restored};
use crate::ax::{paths, Node, CHEAP};
use crate::project::Shape;
use crate::refs::{bad, failed};
use crate::rows::{self, Header, Position, Region};

const GUIDE: &str = "Select only the target region and show Logic Pro's Inspector and Mixer. Kumi needs complete readable region and owning-track facts; it leaves selection and panes unchanged.";
const LEGACY: &str = "The saved region change has no complete observable identity. Restore that property manually in Logic Pro.";
const FIELDS: &[&str] = &["role", "subrole", "id", "desc", "title", "value", "valueDesc", "help", "enabled", "selected", "set", "act"];
const MAX_PROOF: usize = 256 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Rename,
    Loop,
}

impl Mode {
    fn name(self) -> &'static str {
        match self {
            Self::Rename => "rename",
            Self::Loop => "loop",
        }
    }
    fn property(self) -> &'static str {
        match self {
            Self::Rename => "name",
            Self::Loop => "loop",
        }
    }
}

struct Found {
    name: String,
    lane: usize,
    start: Position,
    end: Position,
    midi: bool,
    selected: bool,
    node: Node,
}

struct Seen {
    state: Value,
    name: Control,
    field_value: String,
    checkbox: Control,
    headers: Vec<Header>,
    regions: Vec<(usize, Region)>,
}

fn refuse(why: &str) -> crate::refs::Fault {
    failed(format!("{why}. {GUIDE}"))
}

fn complete(node: &Node) -> bool {
    node.more == 0 && !node.role.is_empty() && node.children.iter().all(complete)
}

fn boolean(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(on) => Some(*on),
        Value::Number(n) if n.as_f64() == Some(0.0) => Some(false),
        Value::Number(n) if n.as_f64() == Some(1.0) => Some(true),
        _ => None,
    }
}

/// Values are evidence too. Missing optional attributes stay absent/null, never fabricated empty values.
fn semantic(node: &Node) -> Value {
    let mut out = json!({"role": node.role, "value": node.value, "children": node.children.iter().map(semantic).collect::<Vec<_>>()});
    for (key, value) in [
        ("subrole", &node.subrole),
        ("id", &node.id),
        ("title", &node.title),
        ("desc", &node.desc),
        ("valueDesc", &node.value_desc),
        ("help", &node.help),
    ] {
        if !value.is_empty() {
            out[key] = value.clone().into();
        }
    }
    out
}

fn raw_name(item: &Node) -> Done<String> {
    let fields: Vec<_> = item.children.iter().filter(|n| n.role == "AXTextField").collect();
    let [field] = fields.as_slice() else {
        return Err(refuse("A track header has no unique full name field"));
    };
    let name = if !field.desc.is_empty() { field.desc.clone() } else { field.value.as_str().unwrap_or_default().to_string() };
    if name.trim().is_empty() {
        return Err(refuse("A track header name is unknown"));
    }
    Ok(name)
}

/// This narrow grammar is the actual English AX help observed in the original scenes. Omitted lower units mean
/// one in that grammar. Unknown formats refuse; there is no public reader's partial/defaulted position fallback,
/// and no conversion through a guessed meter or division setting.
fn bounds(help: &str) -> Done<(Position, Position, bool)> {
    let text = help.strip_prefix("Region starts at ").ok_or_else(|| refuse("A region has no known complete position description"))?;
    let (start, rest) = text.split_once(" and ends at ").ok_or_else(|| refuse("A region has no complete end position"))?;
    let (end, kind) = rest.split_once(',').ok_or_else(|| refuse("A region kind is unknown"))?;
    let kind = kind.split_once('.').map(|(kind, _)| kind.trim()).ok_or_else(|| refuse("A region kind is incomplete"))?;
    let midi = match kind {
        "MIDI region" => true,
        "audio region" => false,
        _ => return Err(refuse("The region kind isn't known MIDI or audio")),
    };
    let position = |text: &str| -> Done<Position> {
        let words: Vec<_> = text.split_whitespace().collect();
        if words.is_empty() || words.len() % 2 != 0 {
            return Err(refuse("A region position is malformed"));
        }
        let mut values = [1u32; 4];
        let mut last = None;
        for pair in words.chunks_exact(2) {
            let index = match pair[1] {
                "bar" | "bars" => 0,
                "beat" | "beats" => 1,
                "division" | "divisions" => 2,
                "tick" | "ticks" => 3,
                _ => return Err(refuse("A region position unit is unknown")),
            };
            if last.is_none() && index != 0 || last.is_some_and(|last| index <= last) {
                return Err(refuse("A region position is incomplete or ambiguous"));
            }
            values[index] = pair[0]
                .parse::<u32>()
                .ok()
                .filter(|n| (1..=1_000_000).contains(n))
                .ok_or_else(|| refuse("A region position value is invalid"))?;
            last = Some(index);
        }
        Ok(Position { bar: values[0], beat: values[1], division: values[2], tick: values[3] })
    };
    let (start, end) = (position(start)?, position(end)?);
    let key = |p: Position| (p.bar, p.beat, p.division, p.tick);
    if key(start) >= key(end) {
        return Err(refuse("A region's complete bounds are invalid"));
    }
    Ok((start, end, midi))
}

fn region(item: &Node, lane: usize) -> Done<Found> {
    if !complete(item) || item.role != "AXLayoutItem" || item.desc.trim().is_empty() {
        return Err(refuse("A region's observable fields are incomplete"));
    }
    let (start, end, midi) = bounds(&item.help)?;
    let selected = item.selected.ok_or_else(|| refuse("A region's selection is unknown"))?;
    let names: Vec<_> = item.children.iter().filter(|n| n.role == "AXTextField").collect();
    let [name] = names.as_slice() else {
        return Err(refuse("A region has no unique native name label"));
    };
    if name.desc != item.desc || !name.value.is_null() && name.value.as_str() != Some(item.desc.as_str()) {
        return Err(refuse("A region's native name labels disagree"));
    }
    Ok(Found { name: item.desc.clone(), lane, start, end, midi, selected, node: item.clone() })
}

fn controlled_name(value: &mut Value, name: &str) {
    for key in ["desc", "title", "value", "valueDesc"] {
        if value[key].as_str() == Some(name) {
            value[key] = json!({"controlled": "name"});
        }
    }
}

fn region_identity(found: &Found, mode: Mode, controlled: bool) -> Value {
    let mut item = semantic(&found.node);
    if mode == Mode::Rename && controlled {
        controlled_name(&mut item, &found.name);
        for child in item["children"].as_array_mut().unwrap() {
            if child["role"] == "AXTextField" {
                controlled_name(child, &found.name);
            }
        }
    }
    if mode == Mode::Loop && controlled {
        for child in item["children"].as_array_mut().unwrap() {
            if child["role"] == "AXHandle" && child["desc"] == "loop" {
                if !child["value"].is_null() {
                    child["value"] = json!({"controlled": "loop"});
                }
                if child.get("valueDesc").is_some() {
                    child["valueDesc"] = json!({"controlled": "loop"});
                }
            }
        }
    }
    json!({"node": item, "start": found.start.to_json(), "end": found.end.to_json(), "kind": if found.midi { "midi" } else { "audio" }})
}

fn pane_ready(ctx: &mut Ctx) -> Done<()> {
    let bar = nudge::read(ctx.project.control_bar(ctx.ax))?;
    for label in ["Inspector", "Mixer"] {
        let matches: Vec<_> = bar.children.iter().filter(|n| n.role == "AXCheckBox" && ctx.project.strings.is(&n.desc, label)).collect();
        if bar.more != 0 || matches.len() != 1 || boolean(&matches[0].value) != Some(true) {
            return Err(refuse("The Inspector and Mixer must already be visible"));
        }
    }
    Ok(())
}

fn owning_track(ctx: &mut Ctx, name: &str) -> Done<Value> {
    let path = ctx.project.layout.mixer.clone().ok_or_else(|| refuse("The owning track's Mixer strip isn't shown"))?;
    let area = nudge::read(ctx.ax.tree(&ctx.project.root, &path, 4, 256, Some(CHEAP)))?;
    if !complete(&area) {
        return Err(refuse("The complete Mixer strip facts are unavailable"));
    }
    let mut matches = vec![];
    for strip in area.children.iter().filter(|n| n.role == "AXLayoutItem") {
        let parts = paths::Strip::of(strip, &ctx.project.strings);
        if parts.bounce.is_some() {
            continue;
        }
        let Some(field) = parts.name else {
            return Err(refuse("A Mixer strip has no readable owner name"));
        };
        if strip.children.iter().filter(|n| n.role == "AXTextField" && ctx.project.strings.is(&n.desc, "name")).count() != 1 {
            return Err(refuse("A Mixer strip's owner name is ambiguous"));
        }
        if field.value_text() == name {
            matches.push((strip, parts));
        }
    }
    let [(strip, parts)] = matches.as_slice() else {
        return Err(refuse("The owning track has no unique Mixer strip"));
    };
    let kind = if parts.instrument.is_some() {
        "instrument"
    } else if parts.inputs.iter().any(|n| ctx.project.strings.starts_with(&n.desc, "Bus")) {
        "aux"
    } else if parts.record.is_some() && !parts.inputs.is_empty() && parts.inputs.iter().all(|n| !n.desc.trim().is_empty()) {
        "audio"
    } else {
        return Err(refuse("The owning track kind is unknown"));
    };
    Ok(json!({"name": name, "kind": kind, "strip": semantic(strip)}))
}

/// Returns the full Region section and its two controls. A shared UI widget id is captured as UI shape, never as
/// a region id. `editing` permits only the known text-field value between SET and Confirm; it cannot select a target.
fn inspector(ctx: &mut Ctx, found: &Found, mode: Mode, editing: Option<&str>) -> Done<(Value, Control, String, Control, bool)> {
    let path = ctx.project.layout.inspector.clone().ok_or_else(|| refuse("The Region Inspector isn't visible"))?;
    let tree = nudge::read(ctx.ax.tree(&ctx.project.root, &path, 8, 256, Some(FIELDS)))?;
    let mut sections = vec![];
    tree.find_all(
        &|n| {
            n.role == "AXGroup"
                && n.children.iter().any(|c| {
                    c.role == "AXStaticText" && ctx.project.strings.is(c.value.as_str().unwrap_or_default().trim_end_matches(':'), "Region")
                })
        },
        &mut sections,
    );
    let [section] = sections.as_slice() else {
        return Err(refuse("The Inspector has no unique Region section"));
    };
    if !complete(&tree) {
        return Err(refuse("The complete Region Inspector facts are clipped or unreadable"));
    }
    let names: Vec<_> = section.children.iter().filter(|n| n.role == "AXTextField").collect();
    let [name] = names.as_slice() else {
        return Err(refuse("The Region Inspector has no unique name field"));
    };
    let field_value = name.value.as_str().filter(|s| !s.trim().is_empty()).ok_or_else(|| refuse("The Inspector region name is unknown"))?;
    if editing.unwrap_or(&found.name) != field_value {
        return Err(changed("the Inspector doesn't name the sole-selected region (or its known pending name)"));
    }
    let mut loop_rows = vec![];
    section.find_all(
        &|n| {
            n.role == "AXRow"
                && n.children.iter().any(|c| {
                    c.role == "AXTextField" && ctx.project.strings.is(c.value.as_str().unwrap_or_default().trim_end_matches(':'), "Loop")
                })
        },
        &mut loop_rows,
    );
    let [row] = loop_rows.as_slice() else {
        return Err(refuse("The Region Inspector has no unique Loop row"));
    };
    let checkboxes: Vec<_> = row.children.iter().filter(|n| n.role == "AXCheckBox").collect();
    let [checkbox] = checkboxes.as_slice() else {
        return Err(refuse("The Region Inspector has no unique Loop control"));
    };
    let looped = boolean(&checkbox.value).ok_or_else(|| refuse("The Inspector Loop value is unknown"))?;
    let name_index = section.children.iter().position(|n| std::ptr::eq(n, *name)).unwrap();
    let checkbox_index = row.children.iter().position(|n| std::ptr::eq(n, *checkbox)).unwrap();
    if section.path.is_empty()
        || row.path.is_empty()
        || name.path != crate::ax::join(&section.path, name_index)
        || checkbox.path != crate::ax::join(&row.path, checkbox_index)
        || name.path == checkbox.path
    {
        return Err(refuse("The Inspector property controls have no unique native path identity"));
    }
    let control = match mode {
        Mode::Rename => *name,
        Mode::Loop => *checkbox,
    };
    if control.path.is_empty()
        || control.enabled != Some(true)
        || mode == Mode::Rename && (!name.settable.iter().any(|v| v == "AXValue") || !name.actions.iter().any(|v| v == "AXConfirm"))
        || mode == Mode::Loop && !checkbox.actions.iter().any(|v| v == "AXPress")
    {
        return Err(refuse("The Region property isn't known writable"));
    }
    let mut normalized = semantic(section);
    fn normalize(node: &Node, out: &mut Value, path: &str, mode: Mode, name: &str) {
        if node.path == path {
            if mode == Mode::Rename {
                controlled_name(out, name);
            }
            out["value"] = json!({"controlled": mode.property()});
            if mode == Mode::Loop && out.get("valueDesc").is_some() {
                out["valueDesc"] = json!({"controlled": "loop"});
            }
        }
        for (child, out) in node.children.iter().zip(out["children"].as_array_mut().unwrap()) {
            normalize(child, out, path, mode, name);
        }
    }
    normalize(
        section,
        &mut normalized,
        match mode {
            Mode::Rename => &name.path,
            Mode::Loop => &checkbox.path,
        },
        mode,
        field_value,
    );
    Ok((
        normalized,
        Control::new(&ctx.project.root, &name.path),
        field_value.to_string(),
        Control::new(&ctx.project.root, &checkbox.path),
        looped,
    ))
}

fn observe(ctx: &mut Ctx, place: usize, mode: Mode, editing: Option<&str>) -> Done<Seen> {
    nudge::read(super::tracks::ensure_layout(ctx))?;
    let project = super::cycle::project(ctx)?;
    pane_ready(ctx)?;
    let raw_headers = nudge::read(ctx.project.headers(ctx.ax))?;
    let contents = nudge::read(ctx.project.contents(ctx.ax))?;
    if !complete(&raw_headers)
        || !complete(&contents)
        || raw_headers.children.iter().any(|n| n.role != "AXLayoutItem")
        || contents.children.iter().any(|n| n.role != "AXLayoutArea")
        || raw_headers.children.len() != contents.children.len()
    {
        return Err(refuse("The complete ordered track and region lists aren't readable"));
    }
    let names = raw_headers.children.iter().map(raw_name).collect::<Done<Vec<_>>>()?;
    let mut regions = vec![];
    for (lane, node) in contents.children.iter().enumerate() {
        if node.children.iter().any(|n| n.role != "AXLayoutItem") {
            return Err(refuse("A region lane contains an unrecognized item"));
        }
        for item in &node.children {
            regions.push(region(item, lane + 1)?);
        }
    }
    let found = regions
        .get(place.checked_sub(1).ok_or_else(|| bad("region ref must be positive"))?)
        .ok_or_else(|| changed("the target region went"))?;
    if !found.selected || regions.iter().filter(|r| r.selected).count() != 1 {
        return Err(refuse("The target region must already be selected alone"));
    }
    let key = region_identity(found, mode, true);
    if regions.iter().filter(|other| other.lane == found.lane && region_identity(other, mode, true) == key).count() != 1 {
        return Err(refuse("The target region's observable identity is ambiguous"));
    }
    let owner_name = &names[found.lane - 1];
    if names.iter().filter(|name| *name == owner_name).count() != 1 {
        return Err(refuse("The owning track's full name is ambiguous"));
    }
    let owner = owning_track(ctx, owner_name)?;
    let (inspector, name, field_value, checkbox, looped) = inspector(ctx, found, mode, editing)?;
    let handles: Vec<_> = found.node.children.iter().filter(|n| n.role == "AXHandle" && n.desc == "loop").collect();
    if handles.len() > 1 || handles.iter().any(|n| !n.value.is_null() && boolean(&n.value) != Some(looped)) {
        return Err(refuse("The region's optional Loop handle and Inspector value disagree or are unreadable"));
    }
    let mut at = 0;
    let lanes: Vec<_> = contents
        .children
        .iter()
        .map(|lane| {
            let items: Vec<_> = lane
                .children
                .iter()
                .map(|_| {
                    let index = at;
                    at += 1;
                    region_identity(&regions[index], mode, index + 1 == place)
                })
                .collect();
            let mut lane_node = lane.clone();
            lane_node.children.clear();
            json!({"node": semantic(&lane_node), "regions": items})
        })
        .collect();
    let identity = json!({"version": 1, "mode": mode.name(), "project": project, "headers": raw_headers.children.iter().map(semantic).collect::<Vec<_>>(),
        "lanes": lanes, "owner": owner, "inspector": inspector, "target": place});
    let state = json!({"name": found.name, "loop": looped, "identity": identity});
    if serde_json::to_vec(&state).map_or(true, |bytes| bytes.len() > MAX_PROOF) {
        return Err(refuse("The complete region proof exceeds its bounded size"));
    }
    // Recheck the full selection/list after reading the shared Inspector, and the saved project last. No cached
    // sole-selection or Inspector value can authorize a different region following an intervening producer edit.
    if nudge::read(ctx.project.contents(ctx.ax))? != contents
        || nudge::read(ctx.project.headers(ctx.ax))? != raw_headers
        || super::cycle::project(ctx)? != state["identity"]["project"]
    {
        return Err(changed("the project, owning track or region selection changed during the read"));
    }
    let headers = raw_headers.children.iter().enumerate().map(|(index, node)| Header::of(node, index, &ctx.project.strings)).collect();
    let regions = regions
        .iter()
        .map(|r| {
            (
                r.lane,
                Region {
                    track_index: r.lane,
                    name: rows::clip(&r.name),
                    start: r.start,
                    end: r.end,
                    midi: r.midi,
                    selected: r.selected,
                    looped: false,
                },
            )
        })
        .collect();
    Ok(Seen { state, name, field_value, checkbox, headers, regions })
}

fn baseline(ctx: &Ctx, seen: &Seen) -> Done<()> {
    let shape = Shape {
        headers: seen.headers.iter().map(|h| h.name.clone()).collect(),
        regions: seen.regions.iter().map(|(track, r)| (*track, r.name.clone(), r.start.bar, r.start.beat, r.end.bar, r.end.beat)).collect(),
    };
    if ctx.project.shape.as_ref() != Some(&shape) {
        return Err(changed("the cached region refs don't match the current project; read the project again before changing it"));
    }
    Ok(())
}

fn wanted(input: &Value, mode: Mode) -> Done<Value> {
    match mode {
        Mode::Rename => input["name"]
            .as_str()
            .filter(|s| !s.trim().is_empty() && s.chars().count() <= 256 && !s.chars().any(char::is_control))
            .map(|s| json!(s))
            .ok_or_else(|| bad("name must be one nonempty line of at most 256 characters")),
        Mode::Loop => input["loop"].as_bool().map(|v| json!(v)).ok_or_else(|| bad("loop must be true or false")),
    }
}

/// Pure version-one shape validation rejects incomplete journals before helper calls. This does not authenticate
/// captured facts: structurally valid evidence is compared with fresh native observations before every write.
fn saved(value: &Value, mode: Mode) -> Done<usize> {
    fn text(value: &Value) -> bool {
        value.as_str().is_some_and(|s| !s.trim().is_empty())
    }
    fn marker(value: &Value) -> bool {
        *value == json!({"controlled":"name"}) || *value == json!({"controlled":"loop"})
    }
    fn tree(node: &Value, depth: usize) -> bool {
        let Some(fields) = node.as_object() else { return false };
        depth <= 16
            && text(&node["role"])
            && fields.contains_key("value")
            && fields
                .keys()
                .all(|key| ["role", "subrole", "id", "title", "desc", "value", "valueDesc", "help", "children"].contains(&key.as_str()))
            && ["subrole", "id", "title", "desc", "valueDesc", "help"]
                .iter()
                .all(|key| node.get(*key).is_none_or(|v| v.is_string() || marker(v)))
            && node["children"].as_array().is_some_and(|children| children.len() <= 256 && children.iter().all(|n| tree(n, depth + 1)))
    }
    fn populated(node: &Value, role: &str) -> bool {
        node["role"] == role && tree(node, 0) && node["children"].as_array().is_some_and(|children| !children.is_empty())
    }
    fn position(value: &Value) -> bool {
        value.as_object().is_some_and(|fields| fields.len() == 4)
            && ["bar", "beat", "division", "tick"].iter().all(|key| value[*key].as_u64().is_some_and(|n| (1..=1_000_000).contains(&n)))
    }
    let proof = &value["identity"];
    let place = proof["target"].as_u64().and_then(|n| usize::try_from(n).ok()).filter(|n| *n > 0).ok_or_else(|| failed(LEGACY))?;
    let project = &proof["project"];
    let path = project["path"].as_str().filter(|p| p.starts_with('/') && p.len() <= crate::logic::MAX_PATH);
    let headers = proof["headers"].as_array().ok_or_else(|| failed(LEGACY))?;
    let lanes = proof["lanes"].as_array().ok_or_else(|| failed(LEGACY))?;
    let owner = &proof["owner"];
    if value.as_object().is_none_or(|fields| fields.len() != 3)
        || !text(&value["name"])
        || value["loop"].as_bool().is_none()
        || proof.as_object().is_none_or(|fields| fields.len() != 8)
        || proof["version"] != 1
        || proof["mode"] != mode.name()
        || project.as_object().is_none_or(|fields| fields.len() != 3)
        || !text(&project["name"])
        || path.is_none_or(|path| project["id"].as_str() != Some(crate::logic::project_id(path).as_str()))
        || headers.is_empty()
        || headers.len() > 256
        || headers.len() != lanes.len()
        || headers.iter().any(|header| !populated(header, "AXLayoutItem"))
        || owner.as_object().is_none_or(|fields| fields.len() != 3)
        || !text(&owner["name"])
        || !matches!(owner["kind"].as_str(), Some("audio" | "instrument" | "aux"))
        || !populated(&owner["strip"], "AXLayoutItem")
        || !populated(&proof["inspector"], "AXGroup")
        || serde_json::to_vec(value).map_or(true, |bytes| bytes.len() > MAX_PROOF)
    {
        return Err(failed(LEGACY));
    }
    let mut count = 0usize;
    for lane in lanes {
        if lane.as_object().is_none_or(|fields| fields.len() != 2)
            || lane["node"]["role"] != "AXLayoutArea"
            || !tree(&lane["node"], 0)
            || lane["node"]["children"] != json!([])
        {
            return Err(failed(LEGACY));
        }
        let regions = lane["regions"].as_array().filter(|items| items.len() <= 256).ok_or_else(|| failed(LEGACY))?;
        for region in regions {
            if region.as_object().is_none_or(|fields| fields.len() != 4)
                || !populated(&region["node"], "AXLayoutItem")
                || !text(&region["node"]["help"])
                || !position(&region["start"])
                || !position(&region["end"])
                || !matches!(region["kind"].as_str(), Some("midi" | "audio"))
            {
                return Err(failed(LEGACY));
            }
            count += 1;
        }
    }
    if place > count {
        return Err(failed(LEGACY));
    }
    Ok(place)
}

fn same_identity(a: &Value, b: &Value) -> bool {
    a["identity"] == b["identity"]
}
fn public(value: &Value, mode: Mode) -> Value {
    json!({(mode.property()): value[mode.property()]})
}
fn title(before: &Value, after: &Value, mode: Mode) -> String {
    match mode {
        Mode::Rename => {
            format!("Renamed region {} → {}", quoted(before["name"].as_str().unwrap()), quoted(after["name"].as_str().unwrap()))
        }
        Mode::Loop => {
            format!("region {} {}", quoted(after["name"].as_str().unwrap()), if after["loop"] == true { "looped" } else { "not looped" })
        }
    }
}

fn write(ctx: &mut Ctx, seen: Seen, to: &Value, mode: Mode) -> Done<Seen> {
    if serde_json::to_vec(to).map_or(true, |bytes| bytes.len() > MAX_PROOF) {
        return Err(bad("the requested complete region state exceeds its bounded proof size"));
    }
    let place = saved(&seen.state, mode)?;
    let fresh = observe(ctx, place, mode, None)?;
    if fresh.state != seen.state {
        return Err(changed("the complete region fence changed before the property write"));
    }
    if fresh.state == *to {
        return Ok(fresh);
    }
    let after = match mode {
        Mode::Rename => {
            let text = to["name"].as_str().unwrap();
            let reply = nudge::wrote(ctx.ax.set(&fresh.name.root, &fresh.name.path, "AXValue", json!(text), nudge::STEP_SETTLE_MS))?;
            if reply["before"].as_str() != Some(fresh.field_value.as_str()) || reply["after"].as_str() != Some(text) {
                return Err(uncertain("Logic Pro didn't verify the region name SET; no Confirm or other action was sent"));
            }
            let pending = observe(ctx, place, mode, Some(text)).map_err(|f| uncertain(f.1))?;
            if !same_identity(&pending.state, &fresh.state)
                || pending.state["loop"] != fresh.state["loop"]
                || pending.state["name"] != fresh.state["name"] && pending.state["name"] != to["name"]
            {
                return Err(uncertain("the region or its selection changed after SET; no Confirm or other action was sent"));
            }
            let reply = nudge::wrote(ctx.ax.action(&pending.name.root, &pending.name.path, "AXConfirm", 250))?;
            if reply["before"].as_str() != Some(text) || reply["after"].as_str() != Some(text) {
                return Err(uncertain("Logic Pro didn't verify the region name Confirm; no further action was sent"));
            }
            observe(ctx, place, mode, None).map_err(|f| uncertain(f.1))?
        }
        Mode::Loop => {
            let reply = nudge::wrote(ctx.ax.action(&fresh.checkbox.root, &fresh.checkbox.path, "AXPress", 250))?;
            // Even attributeUnsupported may mean Logic already toggled the bit. It remains uncertain and returns
            // immediately through wrote(), without another read, resend or compensating action.
            if boolean(&reply["before"]) != fresh.state["loop"].as_bool() || boolean(&reply["after"]) != to["loop"].as_bool() {
                return Err(uncertain("Logic Pro didn't verify the Region Loop press; no further action was sent"));
            }
            observe(ctx, place, mode, None).map_err(|f| uncertain(f.1))?
        }
    };
    if after.state != *to {
        return Err(uncertain(
            "Logic Pro's complete region property readback differs from the requested state; no further action was sent",
        ));
    }
    Ok(after)
}

pub fn preview(ctx: &mut Ctx, input: &Value, mode: Mode) -> Done<Preview> {
    let wanted = wanted(input, mode)?;
    let place = super::parse_ref(ctx, input, "ref", "region", Epoch::Current, 1)?[0];
    let seen = observe(ctx, place, mode, None)?;
    baseline(ctx, &seen)?;
    let mut after = seen.state.clone();
    after[mode.property()] = wanted;
    let mut display =
        json!({"title": title(&seen.state, &after, mode), "before": public(&seen.state, mode), "after": public(&after, mode)});
    if after == seen.state {
        display["unchanged"] = true.into();
    }
    Ok(Preview {
        target: json!({"ref": ref_text(ctx, "region", &[place]), "kind": "region"}),
        prior: seen.state.clone(),
        fence: seen.state,
        display,
    })
}

pub fn apply(ctx: &mut Ctx, input: &Value, fence: &Value, mode: Mode) -> Done<Applied> {
    let wanted = wanted(input, mode)?;
    let place = super::parse_ref(ctx, input, "ref", "region", Epoch::Current, 1)?[0];
    if saved(fence, mode)? != place {
        return Err(changed("the region ref doesn't match its captured identity"));
    }
    let seen = observe(ctx, place, mode, None)?;
    baseline(ctx, &seen)?;
    if seen.state != *fence {
        return Err(changed("the region, owning track, project or Inspector changed since the preview"));
    }
    let before = seen.state.clone();
    let mut to = before.clone();
    to[mode.property()] = wanted;
    let after = write(ctx, seen, &to, mode)?;
    ctx.project.observe(&after.headers, &after.regions);
    Ok(Applied {
        applied: after.state.clone(),
        target: Some(json!({"ref": ref_text(ctx, "region", &[place]), "kind": "region"})),
        material: Some(json!({"undo": {"mode": "value"}})),
        display: json!({"title": title(&before, &after.state, mode), "before": public(&before, mode), "after": public(&after.state, mode)}),
        step: (before != after.state).then_some(match mode {
            Mode::Rename => "Renaming",
            Mode::Loop => "Set Loop",
        }),
        ..Applied::default()
    })
}

pub fn restore(ctx: &mut Ctx, restore: &Restore, mode: Mode) -> Done<Restored> {
    let place = saved(restore.prior, mode)?;
    if saved(restore.applied, mode)? != place
        || !same_identity(restore.prior, restore.applied)
        || restore.prior[match mode {
            Mode::Rename => "loop",
            Mode::Loop => "name",
        }] != restore.applied[match mode {
            Mode::Rename => "loop",
            Mode::Loop => "name",
        }]
    {
        return Err(failed(LEGACY));
    }
    let target = super::parse_ref(ctx, restore.target, "ref", "region", Epoch::Any, 1)?[0];
    if target != place {
        return Err(failed(LEGACY));
    }
    let seen = observe(ctx, place, mode, None)?;
    if !same_identity(&seen.state, restore.prior) {
        return Err(changed("the complete region identity changed; restore that property manually"));
    }
    let before = seen.state.clone();
    let after = if before == *restore.prior {
        seen
    } else {
        if before != *restore.applied {
            return Err(changed("the region property changed in Logic Pro; discard cannot override its complete fence"));
        }
        write(ctx, seen, restore.prior, mode)?
    };
    ctx.project.observe(&after.headers, &after.regions);
    Ok(Restored::by_value(
        after.state.clone(),
        json!({"title": format!("{} (put back)", title(&before, &after.state, mode)), "before": public(&before, mode), "after": public(&after.state, mode)}),
    ))
}

#[cfg(test)]
#[path = "region_values_tests.rs"]
mod tests;
