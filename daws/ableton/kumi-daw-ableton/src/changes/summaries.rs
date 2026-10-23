//! Producer-facing descriptions of changes, independent of bridge I/O.
use super::*;
use kumi_common::js::string::trim_end;
pub(super) type Lookup<'a> = dyn Fn(&Value) -> Option<KnownTrack> + 'a;
pub(super) fn label(value: Option<&Value>) -> Option<String> {
    value.and_then(Value::as_str).map(trim).filter(|s| !s.is_empty()).map(|s| head(s, 80))
}
pub(super) fn quoted(value: Option<&Value>, fallback: &str) -> String {
    label(value).map(|s| format!("“{s}”")).unwrap_or_else(|| fallback.into())
}
pub(super) fn coalesce<'a>(a: Option<&'a Value>, b: Option<&'a Value>) -> Option<&'a Value> {
    a.filter(|v| !v.is_null()).or(b)
}
pub(super) fn plural(n: f64, one: &str) -> String {
    format!("{} {one}{}", number::to_string(n), if n == 1.0 { "" } else { "s" })
}
pub(super) fn known(track: &Lookup<'_>, value: Option<&Value>) -> Option<KnownTrack> {
    track(value.unwrap_or(&Value::Null))
}
pub(super) fn file_name(value: Option<&Value>) -> Option<Value> {
    value.and_then(Value::as_str).map(|s| {
        let base = s.rsplit(['\\', '/']).next().unwrap_or("");
        let base = match base.rfind('.') {
            Some(at) if at + 1 < base.len() => &base[..at],
            _ => base,
        };
        json!(base)
    })
}
fn shown(before: Option<&str>, after: Option<&str>) -> Option<String> {
    match (before, after) {
        (Some(a), Some(b)) if !a.is_empty() && !b.is_empty() => {
            Some(format!("{} → {}", trim(a).replace(' ', "\u{a0}"), trim(b).replace(' ', "\u{a0}")))
        }
        _ => None,
    }
}
fn mixer_parts(prior: &JsonObject, proposed: &JsonObject, was: &JsonObject, now: &JsonObject) -> Vec<String> {
    let mut parts = Vec::new();
    let volume = finite(proposed.get("volume"));
    let before = finite(prior.get("volume"));
    if let Some(volume) = volume {
        parts.push(
            shown(was.get("volume").and_then(Value::as_str), now.get("volume").and_then(Value::as_str))
                .map(|s| format!("volume {s}"))
                .unwrap_or_else(|| {
                    match before {
                        None => "volume",
                        Some(before) if volume == before => "volume",
                        Some(before) if volume > before => "volume up",
                        _ => "volume down",
                    }
                    .into()
                }),
        );
    }
    if let Some(pan) = finite(proposed.get("pan")) {
        parts.push(
            shown(was.get("pan").and_then(Value::as_str), now.get("pan").and_then(Value::as_str))
                .map(|s| format!("pan {s}"))
                .unwrap_or_else(|| {
                    if pan == 0.0 {
                        "pan centre"
                    } else if pan < 0.0 {
                        "pan left"
                    } else {
                        "pan right"
                    }
                    .into()
                }),
        );
    }
    for (key, on, off) in [("mute", "muted", "unmuted"), ("solo", "soloed", "unsoloed")] {
        if let Some(value) = proposed.get(key).and_then(Value::as_bool) {
            parts.push(if value { on } else { off }.into());
        }
    }
    if finite(proposed.get("cueVolume")).is_some() {
        parts.push("cue volume".into());
    }
    let sends = array(proposed.get("sends"));
    if !sends.is_empty() {
        let before = array(was.get("sends"));
        let after = array(now.get("sends"));
        let mut named = Vec::new();
        for (index, _) in sends.iter().enumerate() {
            if let Some(text) = shown(before.get(index).and_then(Value::as_str), after.get(index).and_then(Value::as_str)) {
                let (a, b) = text.split_once(" → ").unwrap();
                if a != b {
                    named.push(format!("send {} {text}", String::from_utf16_lossy(&[(65usize.wrapping_add(index)) as u16])));
                }
            }
        }
        if named.is_empty() {
            parts.push(if sends.len() == 1 { "send A" } else { "sends" }.into());
        } else {
            parts.extend(named);
        }
    }
    parts
}
fn placement(value: Option<&Value>) -> Option<DevicePlacement> {
    let row = record(value);
    let names = |value: Option<&Value>| {
        value
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(Value::as_str).take(16).map(|s| head(s, 64)).collect::<Vec<_>>())
    };
    let chains = row
        .get("chains")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .take(8)
                .map(|value| {
                    let row = record(Some(value));
                    ChainPlacement {
                        name: label(row.get("name")).unwrap_or_else(|| "Chain".into()),
                        devices: names(row.get("devices")).unwrap_or_default(),
                    }
                })
                .collect::<Vec<_>>()
        })
        .filter(|v| !v.is_empty());
    let devices = names(row.get("devices")).filter(|v| !v.is_empty());
    if chains.is_none() && devices.is_none() {
        return None;
    }
    Some(DevicePlacement {
        devices,
        chains,
        index: finite(row.get("index")).filter(|v| *v >= 0.0),
        chain: finite(row.get("chain")).filter(|v| *v >= 0.0),
        rack: label(row.get("rack")),
    })
}
fn parameter_values(from: Option<f64>, to: Option<f64>, before: Option<&str>, after: Option<&str>) -> String {
    shown(before, after).map(|s| format!(" {s}")).unwrap_or_else(|| match (from, to) {
        (Some(from), Some(to)) => format!(" {} → {}", format_number(from, None), format_number(to, None)),
        _ => String::new(),
    })
}
fn parameters_summary(preview: &JsonObject, input: &JsonObject, track: &Lookup<'_>, applied: &JsonObject) -> ChangeSummary {
    let device = record(preview.get("device"));
    let known = known(track, device.get("trackRef"));
    let name = label(device.get("name"));
    let all: Vec<_> = array(preview.get("parameters")).iter().map(|v| record(Some(v))).collect();
    let moved: Vec<_> = all.iter().copied().filter(|row| finite(row.get("currentValue")) != finite(row.get("proposedValue"))).collect();
    let rows = if moved.is_empty() { all } else { moved };
    let after: Vec<_> = array(applied.get("parameters")).iter().map(|v| record(Some(v))).collect();
    let lines = rows
        .iter()
        .map(|parameter| {
            let after = after.iter().rev().find(|row| row.get("ref") == parameter.get("ref"));
            let values = parameter_values(
                finite(parameter.get("currentValue")),
                finite(parameter.get("proposedValue")),
                label(parameter.get("displayValue")).as_deref(),
                label(after.and_then(|row| row.get("displayValue"))).as_deref(),
            );
            format!(
                "{}{}{values}",
                name.as_ref().map(|s| format!("{s} · ")).unwrap_or_default(),
                label(parameter.get("name")).unwrap_or_else(|| "parameter".into())
            )
        })
        .collect();
    let count = if rows.is_empty() { array(input.get("values")).len() } else { rows.len() };
    ChangeSummary {
        title: format!("{} · {}", name.unwrap_or_else(|| "Device".into()), plural(count as f64, "parameter")),
        lines: Some(lines),
        track: known,
        ..Default::default()
    }
}
pub(super) fn base(
    kind: &ChangeKind,
    preview: &JsonObject,
    input: &JsonObject,
    track: &Lookup<'_>,
    applied: Option<&JsonObject>,
) -> Option<ChangeSummary> {
    let empty = JsonObject::new();
    let applied = applied.unwrap_or(&empty);
    let summary = match kind.tool.as_str() {
        "set_tempo" => {
            let from = finite(preview.get("priorTempo"));
            let to = finite(preview.get("proposedTempo"));
            ChangeSummary {
                title: match (from, to) {
                    (Some(a), Some(b)) => format!("Tempo {} → {} BPM", format_number(a, None), format_number(b, None)),
                    _ => "Tempo changed".into(),
                },
                from,
                to,
                ..Default::default()
            }
        }
        "set_mixer" => {
            let prior = record(preview.get("prior"));
            let proposed = record(preview.get("proposed"));
            let known = known(track, preview.get("trackRef"));
            let parts = mixer_parts(prior, proposed, record(preview.get("priorDisplay")), record(applied.get("display")));
            let from = finite(prior.get("volume"));
            let to = finite(proposed.get("volume"));
            let mut summary = ChangeSummary {
                title: format!(
                    "{} {}",
                    known.as_ref().map(|v| v.name.as_str()).unwrap_or("Track"),
                    if parts.is_empty() { "mixer".into() } else { parts.join(", ") }
                ),
                track: known,
                ..Default::default()
            };
            if from.is_some() && to.is_some() {
                summary.from = from;
                summary.to = to;
                summary.range = Some([0.0, 1.0]);
            }
            summary
        }
        "rename" => {
            let target = record(preview.get("target"));
            let kind = label(target.get("kind")).or_else(|| label(input.get("kind"))).unwrap_or_else(|| "item".into());
            let mut known = if kind == "track" { known(track, coalesce(target.get("ref"), input.get("ref"))) } else { None };
            let name = coalesce(preview.get("proposedName"), input.get("name"));
            if let Some(known) = &mut known {
                if let Some(name) = label(name) {
                    known.name = name;
                }
            }
            ChangeSummary {
                title: format!("Renamed {kind} {} → {}", quoted(target.get("currentName"), "(unnamed)"), quoted(name, "(unnamed)")),
                track: known,
                ..Default::default()
            }
        }
        "add_tracks_and_scenes" => {
            let proposed = array(preview.get("proposed"));
            let tracks: Vec<_> = proposed.iter().filter(|row| row.get("kind").and_then(Value::as_str) == Some("track")).collect();
            let scenes: Vec<_> = proposed.iter().filter(|row| row.get("kind").and_then(Value::as_str) == Some("scene")).collect();
            if tracks.len() == 1 && scenes.is_empty() {
                let kind = if tracks[0].get("trackKind").and_then(Value::as_str) == Some("audio") { "audio" } else { "MIDI" };
                ChangeSummary {
                    title: trim(&format!("Added {kind} track {}", quoted(tracks[0].get("name"), ""))).into(),
                    track: Some(KnownTrack {
                        name: label(tracks[0].get("name")).unwrap_or_else(|| format!("New {kind} track")),
                        color: None,
                    }),
                    ..Default::default()
                }
            } else if scenes.len() == 1 && tracks.is_empty() {
                ChangeSummary::title(trim(&format!("Added scene {}", quoted(scenes[0].get("name"), ""))))
            } else {
                let parts = [
                    (!tracks.is_empty()).then(|| plural(tracks.len() as f64, "track")),
                    (!scenes.is_empty()).then(|| plural(scenes.len() as f64, "scene")),
                ]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();
                ChangeSummary::title(format!("Added {}", if parts.is_empty() { "tracks and scenes".into() } else { parts.join(" and ") }))
            }
        }
        "write_midi_clip" => {
            let proposed = record(preview.get("proposed"));
            let source =
                if proposed.get("notes").is_some_and(Value::is_array) { array(proposed.get("notes")) } else { array(input.get("notes")) };
            let known = known(track, coalesce(record(preview.get("target")).get("trackRef"), input.get("trackRef")));
            let length = finite(coalesce(proposed.get("length"), input.get("length")));
            let notes = drawn_notes(source);
            ChangeSummary {
                title: format!(
                    "New MIDI clip {} · {}",
                    quoted(coalesce(proposed.get("name"), input.get("name")), ""),
                    plural(source.len() as f64, "note")
                )
                .replacen("  ", " ", 1),
                track: known,
                clip: length.filter(|v| *v > 0.0 && !notes.is_empty()).map(|length| ClipPicture { length, notes }),
                ..Default::default()
            }
        }
        "load_sample" => {
            let file = file_name(input.get("filePath"));
            let known = known(track, input.get("trackRef"));
            ChangeSummary {
                title: format!(
                    "Loaded {} into a new Simpler{}",
                    quoted(file.as_ref(), "a sample"),
                    known.as_ref().map(|v| format!(" on {}", v.name)).unwrap_or_default()
                ),
                track: known,
                ..Default::default()
            }
        }
        "load_sample_to_pad" => {
            let file = file_name(input.get("filePath"));
            let note = finite(coalesce(preview.get("note"), input.get("note")));
            ChangeSummary::title(format!(
                "{}{}",
                trim_end(&format!(
                    "Loaded {} onto Drum Rack pad {}",
                    quoted(file.as_ref(), "a sample"),
                    note.map(note_name).unwrap_or_default()
                )),
                if input.get("instrument").and_then(Value::as_str) == Some("Drum Sampler") { " in a Drum Sampler" } else { "" }
            ))
        }
        "load_samples_to_pads" => {
            let pads: Vec<_> = array(input.get("pads")).iter().map(|v| record(Some(v))).collect();
            let notes: Vec<_> = pads.iter().filter_map(|pad| finite(pad.get("note"))).collect();
            let run = notes.len() == pads.len() && notes.len() > 1 && notes.windows(2).all(|pair| pair[1] == pair[0] + 1.0);
            let where_ = if run {
                format!("{}–{}", note_name(notes[0]), note_name(notes[notes.len() - 1]))
            } else {
                notes.iter().map(|n| note_name(*n)).collect::<Vec<_>>().join(", ")
            };
            let every = !pads.is_empty() && pads.iter().all(|p| p.get("instrument").and_then(Value::as_str) == Some("Drum Sampler"));
            let lines = pads
                .iter()
                .map(|pad| {
                    format!(
                        "{}{}",
                        trim_end(&format!(
                            "Loaded {} onto Drum Rack pad {}",
                            quoted(file_name(pad.get("filePath")).as_ref(), "a sample"),
                            finite(pad.get("note")).map(note_name).unwrap_or_default()
                        )),
                        if pad.get("instrument").and_then(Value::as_str) == Some("Drum Sampler") { " in a Drum Sampler" } else { "" }
                    )
                })
                .collect();
            ChangeSummary {
                title: format!(
                    "{}{}",
                    trim_end(&format!("Loaded {} samples onto Drum Rack pads {where_}", pads.len())),
                    if every { " in Drum Samplers" } else { "" }
                ),
                lines: Some(lines),
                ..Default::default()
            }
        }
        "load_device" => {
            let known = known(track, coalesce(preview.get("trackRef"), input.get("trackRef")));
            let devices = placement(applied.get("placement"));
            let into = label(preview.get("chainName"))
                .map(|_| {
                    format!(
                        " into {} (chain {})",
                        label(preview.get("rackName")).unwrap_or_else(|| "the rack".into()),
                        devices
                            .as_ref()
                            .and_then(|d| d.chain)
                            .map(|v| number::to_string(v + 1.0))
                            .unwrap_or_else(|| quoted(preview.get("chainName"), ""))
                    )
                })
                .unwrap_or_default();
            ChangeSummary {
                title: format!(
                    "Loaded {}{into}{}",
                    label(record(preview.get("item")).get("name")).unwrap_or_else(|| "a device".into()),
                    known.as_ref().map(|v| format!(" on {}", v.name)).unwrap_or_default()
                ),
                track: known,
                devices,
                ..Default::default()
            }
        }
        "set_device_parameters" => parameters_summary(preview, input, track, applied),
        "set_device_parameter" => {
            if preview.get("parameters").is_some_and(Value::is_array) {
                parameters_summary(preview, input, track, applied)
            } else {
                let parameter = record(preview.get("parameter"));
                let device = record(preview.get("device"));
                let from = finite(parameter.get("currentValue"));
                let to = finite(coalesce(parameter.get("proposedValue"), input.get("value")));
                let values = parameter_values(
                    from,
                    to,
                    label(parameter.get("displayValue")).as_deref(),
                    label(applied.get("displayValue")).as_deref(),
                );
                let mut summary = ChangeSummary {
                    title: format!(
                        "{}{}{values}",
                        label(device.get("name")).map(|s| format!("{s} · ")).unwrap_or_default(),
                        label(parameter.get("name")).unwrap_or_else(|| "parameter".into())
                    ),
                    track: known(track, device.get("trackRef")),
                    ..Default::default()
                };
                if from.is_some() && to.is_some() {
                    summary.from = from;
                    summary.to = to;
                    if let (Some(min), Some(max)) = (finite(parameter.get("min")), finite(parameter.get("max"))) {
                        if max > min {
                            summary.range = Some([min, max]);
                        }
                    }
                }
                summary
            }
        }
        "edit_rack" => {
            let devices = placement(applied.get("placement"));
            let rack = devices
                .as_ref()
                .and_then(|d| d.rack.clone())
                .or_else(|| label(preview.get("rackName")))
                .unwrap_or_else(|| "the rack".into());
            let action = input.get("action").and_then(Value::as_str);
            let index = finite(coalesce(input.get("index"), input.get("selectedVariationIndex"))).unwrap_or(0.0);
            match action {
                Some("insert-chain") => ChangeSummary {
                    title: format!(
                        "Added chain {}to {rack}",
                        devices.as_ref().and_then(|d| d.chain).map(|v| format!("{} ", number::to_string(v + 1.0))).unwrap_or_default()
                    ),
                    devices,
                    ..Default::default()
                },
                Some("randomize-macros") => ChangeSummary::title(format!("Randomized {rack}'s macros")),
                Some("store-variation") => ChangeSummary::title(format!("Stored a variation of {rack}'s macros")),
                Some("recall-variation" | "delete-variation" | "set") => ChangeSummary::title(format!(
                    "{} variation {} of {rack}",
                    match action {
                        Some("recall-variation") => "Recalled",
                        Some("delete-variation") => "Deleted",
                        _ => "Selected",
                    },
                    number::to_string(index + 1.0)
                )),
                Some("copy-pad") => ChangeSummary::title(format!(
                    "Copied pad {} to {} in {rack}",
                    note_name(finite(input.get("sourceIndex")).unwrap_or(0.0)),
                    note_name(finite(input.get("targetIndex")).unwrap_or(0.0))
                )),
                _ => {
                    let count = finite(applied.get("visibleMacroCount"));
                    let before = finite(record(preview.get("prior")).get("visibleMacroCount"));
                    ChangeSummary {
                        title: if action == Some("add-macro") {
                            format!("Added a macro to {rack}")
                        } else {
                            format!("Removed a macro from {rack}")
                        },
                        from: count.map(|count| before.unwrap_or(count)),
                        to: count,
                        range: count.map(|_| [1.0, 16.0]),
                        ..Default::default()
                    }
                }
            }
        }
        "set_chain_mixer" => {
            let prior = record(preview.get("prior"));
            let proposed = record(preview.get("proposed"));
            let mut parts = mixer_parts(prior, proposed, &empty, &empty);
            if let Some(enabled) = proposed.get("chainActivator").and_then(Value::as_bool) {
                parts.push(if enabled { "on" } else { "off" }.into());
            }
            let from = finite(prior.get("volume"));
            let to = finite(proposed.get("volume"));
            let mut summary = ChangeSummary::title(
                format!(
                    "{}chain {} {}",
                    label(preview.get("rackName")).map(|s| format!("{s} · ")).unwrap_or_default(),
                    quoted(preview.get("chainName"), ""),
                    if parts.is_empty() { "mixer".into() } else { parts.join(", ") }
                )
                .replacen("  ", " ", 1),
            );
            if from.is_some() && to.is_some() {
                summary.from = from;
                summary.to = to;
                summary.range = Some([0.0, 1.0]);
            }
            summary
        }
        "set_locators" => {
            let range = match (finite(input.get("start")), finite(input.get("end"))) {
                (Some(a), Some(b)) => format!(" (beats {}–{})", format_number(a, None), format_number(b, None)),
                _ => String::new(),
            };
            ChangeSummary::title(
                format!("Locators {} and {}{range}", quoted(input.get("startName"), ""), quoted(input.get("endName"), ""))
                    .replacen("  ", " ", 1),
            )
        }
        "set_track_color" => {
            let mut known = known(track, coalesce(preview.get("ref"), input.get("ref")));
            let to = applied.get("color").and_then(hex_color);
            let colors = to
                .as_ref()
                .map(|to| ColorChange { from: known.as_ref().and_then(|v| v.color.clone()).filter(|s| !s.is_empty()), to: to.clone() });
            if let Some(track) = &mut known {
                if let Some(to) = to {
                    track.color = Some(to);
                }
            }
            ChangeSummary {
                title: format!("{} colour changed", known.as_ref().map(|v| v.name.as_str()).unwrap_or("Track")),
                track: known,
                colors,
                ..Default::default()
            }
        }
        "set_sidechain" => {
            static OWNER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^([0-9]+):device:([0-9]+)").unwrap());
            let source = input.get("routingType").and_then(Value::as_str).map(|s| head(s, 80)).unwrap_or_else(|| "another track".into());
            let owner = input.get("deviceRef").and_then(Value::as_str).and_then(|s| OWNER.captures(s));
            let known = owner.and_then(|m| track(&json!(format!("{}:track:{}", &m[1], &m[2]))));
            ChangeSummary {
                title: format!(
                    "{} from {source}{}",
                    if input.get("action").and_then(Value::as_str) == Some("sidechain") { "Sidechain" } else { "Device input" },
                    known.as_ref().map(|v| format!(" on {}", v.name)).unwrap_or_default()
                ),
                track: known,
                ..Default::default()
            }
        }
        _ => return None,
    };
    Some(summary)
}
pub(super) fn drawn_notes(source: &[Value]) -> Vec<ClipNote> {
    source
        .iter()
        .take(512)
        .filter_map(|item| {
            let row = record(Some(item));
            let duration = finite(row.get("duration"))?;
            if duration <= 0.0 {
                return None;
            }
            Some(ClipNote {
                pitch: finite(row.get("pitch"))?,
                start: finite(row.get("start"))?,
                duration,
                velocity: finite(row.get("velocity")).unwrap_or(100.0),
            })
        })
        .collect()
}
