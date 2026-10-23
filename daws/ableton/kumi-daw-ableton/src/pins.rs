//! Things pointed at in Live or Kumi, validated against the current connection and device tree.
use super::{more_changes::bars, references::References};
use kumi_agent::core::contracts::*;
use kumi_common::js::{json::stringify, number::to_string};
use kumi_types::contracts::*;
use regex::Regex;
use serde_json::{json, Value};
use std::sync::LazyLock;
fn text(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(a)) => a.iter().map(|v| text(Some(v))).collect::<Vec<_>>().join(","),
        Some(Value::Object(_)) => "[object Object]".into(),
        Some(v) => stringify(v),
    }
}
/// A right-click event from the retained Live extension. Its trail includes the object itself.
pub fn pointed_pin(event: &JsonObject) -> Option<PinnedNode> {
    let empty = JsonObject::new();
    let data = event.get("payload").and_then(Value::as_object).unwrap_or(&empty);
    let kind = text(data.get("kind"));
    let mut trail: Vec<String> =
        data.get("trail").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect();
    let path = data.get("path").and_then(Value::as_array).cloned().unwrap_or_default();
    if kind.ends_with("selection") {
        let lanes: Vec<_> = data
            .get("lanes")
            .and_then(Value::as_array)
            .or_else(|| data.get("slots").and_then(Value::as_array))
            .into_iter()
            .flatten()
            .filter(|v| v.is_object() || v.is_array())
            .collect();
        let lane = lanes.first()?;
        let reference = lane.get("ref")?.as_str()?;
        let time = data.get("timeSelection");
        let span = time.and_then(|t| Some(PinnedTime { from_beat: t.get("fromBeat")?.as_f64()?, to_beat: t.get("toBeat")?.as_f64()? }));
        let name = lane.get("name").and_then(Value::as_str).unwrap_or("").to_owned();
        return Some(PinnedNode {
            track_ref: if lane.get("kind").and_then(Value::as_str) == Some("track") { reference.into() } else { String::new() },
            r#ref: reference.into(),
            node: if span.is_some() { PinKind::Selection } else { PinKind::ClipSlot },
            name: name.clone(),
            trail: Vec::new(),
            siblings: lanes.iter().skip(1).map(|other| text(other.get("name"))).collect(),
            live: Some(true),
            track: (!name.is_empty()).then_some(name),
            time: span,
            layout: None,
        });
    }
    let reference =
        data.get("ref").and_then(Value::as_str).or_else(|| event.get("ref").and_then(Value::as_str)).filter(|s| !s.is_empty())?;
    let epoch = reference.split(':').next().unwrap_or("");
    let node = match kind.as_str() {
        "track" => PinKind::Track,
        "scene" => PinKind::Scene,
        "clip_slot" => PinKind::ClipSlot,
        kind if kind.ends_with("clip") => PinKind::Clip,
        _ => PinKind::Device,
    };
    let index = path.first().and_then(Value::as_f64);
    let name = data.get("name").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_owned).unwrap_or_else(|| {
        if node == PinKind::Scene && index.is_some() {
            format!("Scene {}", to_string(index.unwrap() + 1.))
        } else {
            String::new()
        }
    });
    if trail.last().is_some_and(|last| last.is_empty() || data.get("name").and_then(Value::as_str) == Some(last)) {
        trail.pop();
    }
    let track = if node != PinKind::Track && node != PinKind::Scene { trail.first().filter(|s| !s.is_empty()).cloned() } else { None };
    Some(PinnedNode {
        track_ref: if node == PinKind::Track {
            reference.into()
        } else if node == PinKind::Scene || index.is_none() {
            String::new()
        } else {
            format!("{epoch}:track:{}", to_string(index.unwrap()))
        },
        r#ref: reference.into(),
        node,
        name,
        trail,
        siblings: Vec::new(),
        live: Some(true),
        track,
        time: None,
        layout: None,
    })
}
static REFERENCE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[0-9]+:([a-z_]+):").unwrap());
/// `tree` is freshly read for Kumi pins; Live pins instead rely on the current epoch.
pub fn pin_context(pin: &PinnedNode, epoch: Option<f64>, tree: Option<&DeviceTree>, book: &mut References) -> JsonObject {
    let kind = serde_json::to_value(pin.node).unwrap();
    let kind = kind.as_str().unwrap();
    if pin.live == Some(true) {
        let epoch = epoch.map(to_string).unwrap_or_else(|| "undefined".into());
        if !pin.r#ref.starts_with(&format!("{epoch}:")) {
            return json!({"gone":format!("The producer pointed at “{}” in Live, but Live's references changed since: say so, and ask what they mean.",pin.name)}).as_object().unwrap().clone();
        }
        let reference_kind = REFERENCE.captures(&pin.r#ref).and_then(|c| c.get(1).map(|m| m.as_str().to_owned()));
        let discovered = match reference_kind.as_deref() {
            Some("track") => Some("track"),
            Some("device") => Some("device"),
            Some("chain") => Some("chain"),
            Some("scene") => Some("scene"),
            Some("clip_slot") => Some("clip-slot"),
            Some("clip") => Some("session-clip"),
            Some("arrangement_clip") => Some("arrangement-clip"),
            _ => None,
        };
        if let Some(discovered) = discovered {
            book.refs.insert(pin.r#ref.clone(), discovered.into());
        }
        let words = if pin.time.is_some() {
            "this part of the Arrangement".into()
        } else {
            format!("this {}", if pin.node == PinKind::ClipSlot { "slot" } else { kind })
        };
        let mut row = base(pin, book.short_ref(&pin.r#ref), &pin.trail, false);
        if let Some(time) = &pin.time {
            row.insert("fromBeat".into(), json!(time.from_beat));
            row.insert("toBeat".into(), json!(time.to_beat));
            row.insert("spans".into(), json!(format!("{} to {}", bars(time.from_beat), bars(time.to_beat))));
        }
        row.insert(
            "note".into(),
            json!(format!(
                "The producer pointed at this in Live (right-click): \"this\", \"{words}\" or \"here\" in their message means it."
            )),
        );
        return row;
    }
    fn walk(devices: &[DeviceNode], trail: &[String], pin: &PinnedNode, found: &mut Vec<(String, Vec<String>)>) {
        for device in devices {
            if pin.node == PinKind::Device && device.name == pin.name {
                found.push((device.r#ref.clone(), trail.to_vec()));
            }
            for chain in device.chains.as_deref().unwrap_or_default() {
                let mut trail = trail.to_vec();
                trail.push(device.name.clone());
                if pin.node == PinKind::Chain && chain.name == pin.name {
                    found.push((chain.r#ref.clone(), trail.clone()));
                }
                trail.push(chain.name.clone());
                walk(chain.devices.as_deref().unwrap_or_default(), &trail, pin, found);
            }
        }
    }
    let mut found = Vec::new();
    if let Some(tree) = tree {
        walk(&tree.devices, &[], pin, &mut found);
    }
    let same = found
        .iter()
        .find(|(reference, _)| *reference == pin.r#ref)
        .or_else(|| found.iter().find(|(_, trail)| trail.join("\0") == pin.trail.join("\0")));
    let Some((reference, trail)) = same else {
        return json!({"gone":format!("The producer pointed at “{}”{} in Kumi, and it isn't there any more: say so, and ask what they mean.",pin.name,pin.track.as_deref().filter(|s|!s.is_empty()).map(|s|format!(" on {s}")).unwrap_or_default())}).as_object().unwrap().clone();
    };
    book.refs.insert(reference.clone(), kind.into());
    let mut row = base(pin, book.short_ref(reference), trail, true);
    if !pin.siblings.is_empty() {
        row.insert("nextTo".into(), json!(pin.siblings.iter().take(12).collect::<Vec<_>>()));
    }
    row.insert(
        "note".into(),
        json!("The producer pointed at this in Kumi: \"this\", \"this device\" or \"this group\" in their message means it."),
    );
    row
}
fn base(pin: &PinnedNode, reference: String, trail: &[String], include_track: bool) -> JsonObject {
    let mut row = JsonObject::new();
    row.insert("ref".into(), json!(reference));
    row.insert("kind".into(), serde_json::to_value(pin.node).unwrap());
    row.insert("name".into(), json!(pin.name));
    if !trail.is_empty() {
        row.insert("in".into(), json!(trail.join(" › ")));
    }
    if let Some(track) = pin.track.as_deref().filter(|s| !s.is_empty() && (include_track || pin.node != PinKind::Track)) {
        row.insert("track".into(), json!(track));
    }
    row
}
