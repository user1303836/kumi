//! Tools matching the native LYNX control operations.
use crate::{client::Client, extensions};
use async_trait::async_trait;
use kumi_agent::core::context::IntegrationContext;
use kumi_common::abort::{Signal, SignalExt};
use kumi_types::{contracts::*, errors::RuntimeError};
use serde_json::{json, Value};
use std::{cell::Cell, rc::Rc};

const OPS: &[(&str, &str, &str, bool)] = &[
    ("lynx_inspect", "session.inspect", "Read the current LYNX project, tempo, transport, channels, mixer, patterns and Playlist. Names are data. Indices are zero-based.", false),
    ("lynx_pattern_notes", "pattern.notes", "Read every note of a LYNX pattern by its zero-based index. Times are ticks, 960 ticks per quarter note; key is MIDI 0–127, velocity is 0–1, pan is -1–1. The response includes revision; read before replacing notes and pass that revision as expectedRevision.", false),
    ("lynx_tempo_set", "tempo.set", "Set the current LYNX project's tempo in BPM, as one native undo step.", true),
    ("lynx_mixer_set", "mixer.set", "Set a LYNX mixer insert's volume (0–1 native fader), pan (-1–1), mute or solo. Zero-based index, 0 is Master. Changes use LYNX's native undo.", true),
    ("lynx_channel_set", "channel.set", "Set a LYNX channel's volume (0–1), pan (-1–1) or mute. Uses its zero-based index and LYNX's native undo.", true),
    ("lynx_pattern_replace_notes", "pattern.replace_notes", "Replace ALL notes in one pattern as a native undo step. First read its notes and revision, preserve every note/field not requested to change, and supply the complete list plus that revision as expectedRevision. A concurrent edit is refused; read notes again before trying a new edit. Times are ticks at PPQ 960. Never use this to append only the new notes.", true),
    ("lynx_transport_play", "transport.play", "Start LYNX playback in its current Song/Pattern mode.", true),
    ("lynx_transport_stop", "transport.stop", "Stop LYNX playback.", true),
    ("lynx_history_undo", "history.undo", "Undo the last step in LYNX's shared native history, which can include the producer's own edits. Only use when explicitly asked to undo. This is not an undo of an arbitrary Kumi history row.", true),
    ("lynx_history_redo", "history.redo", "Redo the next step in LYNX's shared native history, only when explicitly asked.", true),
];

pub fn tools(
    client: Client,
    project: String,
    revision: u64,
    capabilities: &Value,
    context: IntegrationContext,
    serial: Rc<Cell<u64>>,
) -> Vec<Rc<dyn KernelTool>> {
    let known_revision = Rc::new(Cell::new(revision));
    OPS.iter()
        .chain(extensions::OPS.iter())
        .filter(|(_, op, _, _)| extensions::enabled(op, capabilities))
        .map(|&(name, op, description, writes)| {
            Rc::new(LynxTool {
                client: client.clone(),
                project: project.clone(),
                known_revision: known_revision.clone(),
                context: context.clone(),
                serial: serial.clone(),
                name,
                op,
                description,
                writes,
            }) as Rc<dyn KernelTool>
        })
        .collect()
}

fn number(min: f64, max: f64) -> Value {
    json!({"type":"number","minimum":min,"maximum":max})
}
fn schema(op: &str) -> JsonObject {
    let index = json!({"type":"integer","minimum":0,"maximum":100000});
    let (properties, required) = match op {
        "tempo.set" => (json!({"bpm":number(10.,522.)}), vec!["bpm"]),
        "mixer.set" => (
            json!({"index":index,"volume":number(0.,1.),"pan":number(-1.,1.),"muted":{"type":"boolean"},"solo":{"type":"boolean"}}),
            vec!["index"],
        ),
        "channel.set" => (json!({"index":index,"volume":number(0.,1.),"pan":number(-1.,1.),"muted":{"type":"boolean"}}), vec!["index"]),
        "pattern.notes" => (json!({"index":index}), vec!["index"]),
        "pattern.replace_notes" => (
            json!({"index":index,"expectedRevision":{"type":"integer","minimum":0,"maximum":9007199254740991u64,"description":"The revision returned by the pattern.notes read used for this replacement."},"notes":{"type":"array","maxItems":10000,"items":{"type":"object","properties":{
            "start":{"type":"integer","minimum":0,"maximum":1000000000},"length":{"type":"integer","minimum":1,"maximum":1000000000},
            "channel":index,"key":{"type":"integer","minimum":0,"maximum":127},"velocity":number(0.,1.),"pan":number(-1.,1.),"muted":{"type":"boolean"},
            "finePitch":number(-120.,120.),"slide":{"type":"boolean"},"porta":{"type":"boolean"},"release":number(0.,1.),"modX":number(0.,1.),"modY":number(0.,1.),"colour":{"type":"integer","minimum":0,"maximum":15},"group":{"type":"integer","minimum":0,"maximum":1000000000}
        },"required":["start","length","channel","key","velocity","pan","muted","finePitch","slide","porta","release","modX","modY","colour","group"],"additionalProperties":false}}}),
            vec!["index", "notes", "expectedRevision"],
        ),
        _ => {
            return extensions::schema(op).unwrap_or_else(|| {
                json!({"type":"object","properties":{},"required":[],"additionalProperties":false}).as_object().unwrap().clone()
            })
        }
    };
    json!({"type":"object","properties":properties,"required":required,"additionalProperties":false}).as_object().unwrap().clone()
}

fn validate(input: &JsonObject, schema: &Value) -> Result<(), String> {
    validate_object(input, schema)
}

fn validate_object(input: &JsonObject, schema: &Value) -> Result<(), String> {
    let properties = schema["properties"].as_object().ok_or("Invalid operation schema")?;
    for field in schema["required"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        if !input.contains_key(field) {
            return Err(format!("Missing {field}"));
        }
    }
    for (name, value) in input {
        let spec = properties.get(name).ok_or_else(|| format!("Unknown field {name}"))?;
        if !validate_value(value, spec)? {
            return Err(format!("Invalid {name}; use the stated type and range"));
        }
        if name == "notes" {
            for note in value.as_array().into_iter().flatten() {
                if note["start"].as_f64().unwrap_or(0.) + note["length"].as_f64().unwrap_or(0.) > 1_000_000_000. {
                    return Err("A note extends beyond the native tick bound".into());
                }
            }
        }
    }
    Ok(())
}

fn validate_value(value: &Value, spec: &Value) -> Result<bool, String> {
    if spec["enum"].as_array().is_some_and(|choices| !choices.contains(value)) {
        return Ok(false);
    }
    Ok(match spec["type"].as_str() {
        Some("number") | Some("integer") => value.as_f64().is_some_and(|n| {
            n.is_finite()
                && (spec["type"] != "integer" || n.fract() == 0.)
                && n >= spec["minimum"].as_f64().unwrap_or(f64::MIN)
                && n <= spec["maximum"].as_f64().unwrap_or(f64::MAX)
        }),
        Some("boolean") => value.is_boolean(),
        Some("string") => value.as_str().is_some_and(|text| {
            let length = text.chars().count() as u64;
            length >= spec["minLength"].as_u64().unwrap_or(0)
                && length <= spec["maxLength"].as_u64().unwrap_or(4096)
                && !text.chars().any(char::is_control)
        }),
        Some("object") => {
            if let Some(object) = value.as_object() {
                validate_object(object, spec)?;
                true
            } else {
                false
            }
        }
        Some("array") => {
            if let Some(items) = value.as_array() {
                if items.len() < spec["minItems"].as_u64().unwrap_or(0) as usize
                    || items.len() > spec["maxItems"].as_u64().unwrap_or(10000) as usize
                {
                    false
                } else {
                    for item in items {
                        if !validate_value(item, &spec["items"])? {
                            return Ok(false);
                        }
                    }
                    true
                }
            } else {
                false
            }
        }
        _ => false,
    })
}

struct LynxTool {
    client: Client,
    project: String,
    known_revision: Rc<Cell<u64>>,
    context: IntegrationContext,
    serial: Rc<Cell<u64>>,
    name: &'static str,
    op: &'static str,
    description: &'static str,
    writes: bool,
}

#[async_trait(?Send)]
impl KernelTool for LynxTool {
    fn name(&self) -> &str {
        self.name
    }
    fn description(&self) -> &str {
        self.description
    }
    fn input_schema(&self) -> JsonObject {
        schema(self.op)
    }
    fn reach(&self) -> Reach {
        if self.writes {
            Reach::DawWrite
        } else {
            Reach::DawRead
        }
    }
    fn concurrent(&self) -> bool {
        false
    }
    async fn execute(&self, mut input: JsonObject, signal: Signal) -> Result<ToolResult, RuntimeError> {
        signal.check()?;
        if let Err(error) = validate(&input, &Value::Object(self.input_schema())).and_then(|_| extensions::validate(self.op, &input)) {
            return Ok(ToolResult::error(error));
        }
        if matches!(self.op, "mixer.set" | "channel.set") && input.len() == 1 {
            return Ok(ToolResult::error("Supply at least one setting to change"));
        }
        if self.writes {
            input.insert("expectedProjectId".into(), self.project.clone().into());
            // A full-pattern replacement is tied to the exact notes read by the model.
            // Other writes use the last revision this catalog showed or changed itself.
            if self.op != "pattern.replace_notes" {
                input.insert("expectedRevision".into(), self.known_revision.get().into());
            }
        }
        let result = match self.client.call(self.op, Value::Object(input), &signal).await {
            Ok(result) => result,
            Err(error) => return Ok(ToolResult::error(error.to_string())),
        };
        let response_project = if self.op == "session.inspect" {
            result.pointer("/project/id").and_then(Value::as_str)
        } else {
            result.get("projectId").and_then(Value::as_str)
        };
        if response_project != Some(self.project.as_str()) {
            if let Some(again) = &self.context.on_look_again {
                again();
            }
            return Ok(ToolResult::error(
                "LYNX project changed or its response identity is missing; inspect the session again before editing",
            ));
        }
        let Some(revision) = result.get("revision").and_then(Value::as_u64) else {
            return Ok(ToolResult::error(
                "LYNX response lacks a valid revision; a change may have run. Inspect the session before editing again",
            ));
        };
        self.known_revision.set(revision);
        if self.writes {
            if let Some(again) = &self.context.on_look_again {
                again();
            }
            if !self.op.starts_with("transport.") {
                if let Some(changed) = &self.context.on_change {
                    let id = self.serial.get() + 1;
                    self.serial.set(id);
                    changed(ChangeRecord {
                        id: format!("lynx-{id}"),
                        family: match self.op {
                            "tempo.set" => ChangeFamily::Tempo,
                            "pattern.replace_notes" | "playlist.edit" => ChangeFamily::Clip,
                            "plugin.parameters_set" => ChangeFamily::Parameter,
                            "effect.load" | "effect.remove" => ChangeFamily::Device,
                            _ => ChangeFamily::Mixer,
                        },
                        title: self.name.trim_start_matches("lynx_").replace('_', " "),
                        track: None,
                        from: None,
                        to: None,
                        range: None,
                        colors: None,
                        clip: None,
                        devices: None,
                        state: ChangeState::Kept,
                        score: None,
                        note: Some("Uses LYNX's shared native undo history; ask to undo the last native step.".into()),
                        at: chrono::Utc::now().timestamp_millis(),
                        picture: None,
                        batch: None,
                    });
                }
            }
        }
        Ok(ToolResult::text(serde_json::to_string(&result).unwrap_or_else(|_| "null".into())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn complete_note() -> Value {
        json!({"start":120,"length":960,"channel":0,"key":60,"velocity":0.8,"pan":-0.2,
            "muted":true,"finePitch":4.5,"slide":true,"porta":true,"release":0.3,
            "modX":0.4,"modY":0.6,"colour":3,"group":7})
    }
    #[test]
    fn edits_reject_unbounded_values_and_incomplete_note_metadata() {
        let check = |op, input: Value| validate(input.as_object().unwrap(), &Value::Object(schema(op)));
        assert!(check("mixer.set", json!({"index":0,"volume":2})).is_err());
        assert!(check("tempo.set", json!({"bpm":0})).is_err());
        assert!(check("pattern.replace_notes", json!({"index":0,"notes":[]})).is_err());
        assert!(check("pattern.replace_notes", json!({"index":0,"notes":[],"expectedRevision":-1})).is_err());
        assert!(check(
            "pattern.replace_notes",
            json!({"index":0,"expectedRevision":0,"notes":[{"start":0,"length":1,"channel":0,"key":60,"velocity":0.8}]})
        )
        .is_err());
        let note = complete_note();
        for field in note.as_object().unwrap().keys() {
            let mut incomplete = note.clone();
            incomplete.as_object_mut().unwrap().remove(field);
            assert!(
                check("pattern.replace_notes", json!({"index":0,"expectedRevision":0,"notes":[incomplete]})).is_err(),
                "missing {field}"
            );
        }
        let mut unknown = note.clone();
        unknown["hidden"] = json!(true);
        assert!(check("pattern.replace_notes", json!({"index":0,"expectedRevision":0,"notes":[unknown]})).is_err());
        assert!(check("pattern.replace_notes", json!({"index":0,"expectedRevision":0,"notes":[note]})).is_ok());
    }
    fn check_extension(op: &str, input: Value) -> Result<(), String> {
        let object = input.as_object().unwrap();
        validate(object, &Value::Object(schema(op)))?;
        extensions::validate(op, object)
    }

    #[test]
    fn parameter_batches_require_observed_values_and_unambiguous_targets() {
        let target = json!({"kind":"instrument","channelIndex":2});
        assert!(check_extension("plugin.parameters_set", json!({"target":target,"parameters":[{"index":3,"value":0.7}]})).is_err());
        assert!(check_extension(
            "plugin.parameters_set",
            json!({"target":target,"parameters":[{"index":3,"value":0.7,"expectedValue":0.2},{"index":3,"value":0.3,"expectedValue":0.2}]})
        )
        .is_err());
        assert!(check_extension("plugin.parameters_set", json!({"target":{"kind":"instrument","channelIndex":2,"mixerIndex":0},"parameters":[{"index":3,"value":0.7,"expectedValue":0.2}]})).is_err());
        assert!(check_extension(
            "plugin.parameters_set",
            json!({"target":target,"parameters":[{"index":3,"value":1.1,"expectedValue":0.2}]})
        )
        .is_err());
        assert!(check_extension("plugin.parameters_set", json!({"target":target,"parameters":[]})).is_err());
        assert!(check_extension("plugin.parameters_set", json!({"target":target,"parameters":[{"index":3.0,"value":0.7,"expectedValue":0.2},{"index":3,"value":0.3,"expectedValue":0.2}]})).is_err());
        assert!(check_extension("plugin.parameters_set", json!({"target":{"kind":"effect","mixerIndex":6,"effectIndex":1},"parameters":[{"index":3,"value":0.7,"expectedValue":0.2},{"index":4,"value":0.1,"expectedValue":0.6}]})).is_ok());
        assert!(check_extension("plugin.inspect", json!({"target":{"kind":"effect","mixerIndex":6},"limit":128})).is_err());
        assert!(check_extension("plugin.inspect", json!({"limit":257})).is_err());
        assert!(check_extension("effect.load", json!({"mixerIndex":0,"effectId":"native:1","path":"/tmp/another-plugin"})).is_err());
        assert!(check_extension("effect.load", json!({"mixerIndex":0,"effectId":"native:1","effectIndex":10})).is_err());
    }

    #[test]
    fn playlist_transactions_reject_ambiguous_sources_unknown_fields_and_tick_overflow() {
        let check = |edits| check_extension("playlist.edit", json!({"edits":edits}));
        let clip = "000000000000002a";
        assert!(check(json!([])).is_err());
        assert!(check(json!([{"action":"add","kind":"pattern","track":0,"start":0,"length":960}])).is_err());
        assert!(check(json!([{"action":"add","kind":"pattern","track":0,"start":0,"length":960,"pattern":0,"channel":1}])).is_err());
        assert!(check(json!([{"action":"add","kind":"pattern","track":0,"start":0,"length":960,"pattern":0,"gain":0.5}])).is_err());
        assert!(check(json!([{"action":"add","kind":"audio","track":500,"start":0,"length":960,"channel":0}])).is_err());
        assert!(check(json!([{"action":"add","kind":"audio","track":0,"start":1000000000,"length":960,"channel":0}])).is_err());
        assert!(check(json!([{"action":"update","clipId":clip}])).is_err());
        assert!(check(json!([{"action":"update","clipId":clip,"pattern":1}])).is_err());
        assert!(check(json!([{"action":"remove","clipId":clip,"muted":true}])).is_err());
        assert!(check(json!([{"action":"remove","clipId":"0000000000000000"}])).is_err());
        assert!(check(json!([{"action":"remove","clipId":"not-a-hex-id-0001"}])).is_err());
        assert!(check(json!([{"action":"update","clipId":clip,"start":1920,"dropOtherFields":true}])).is_err());
        assert!(check(json!([{"action":"update","clipId":clip,"start":1920},{"action":"duplicate","clipId":clip,"track":3,"start":3840},{"action":"add","kind":"audio","track":4,"start":7680,"length":1920,"channel":9,"gain":0.8,"offsetFraction":0.25}])).is_ok());
        assert!(check_extension("playlist.inspect", json!({"offset":250,"limit":1000})).is_ok());
        assert!(check_extension("playlist.inspect", json!({"limit":1001})).is_err());
    }
}
