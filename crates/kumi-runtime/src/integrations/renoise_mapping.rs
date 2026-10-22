//! Renoise's backend for the sample map (#401): only Renoise's facts. Its notes ("C-4", Renoise's 48, sounds as MIDI 60), the
//! keys its keyzones map (C-0 to B-9), an instrument's samples (read, and each one's audio saved to Kumi's folder to be heard),
//! a new instrument of planned files (instrument_kit, one change) or an instrument's own samples laid out (keyzone_map, one
//! change), and the song's instruments for the tidy (one read). Core's tool does the rest.
use super::own_changes::OwnPlan;
use super::own_listening::OwnRenderer;
use super::renoise_lines::{note_text, note_value};
use crate::core::{
    mapping::{
        backend::{Held, MappingBackend, Words},
        Planned,
    },
    refs::RefScheme,
};
use crate::listening::renderer::Renderer;
use async_trait::async_trait;
use kumi_common::abort::Signal;
use serde_json::{json, Value};
use std::rc::Rc;

/// Pages of sample rows read for an instrument (200 a page).
const SAMPLE_PAGES: usize = 2;

pub static WORDS: Words = Words {
    app: "Renoise",
    instrument: "instrument",
    notes: "C-4, C#4, or Renoise's number (48 is C-4)",
    // (Renoise's notes 0 to 119 are MIDI 12 to 131: C-4, its 48, sounds as MIDI 60)
    lowest: 12,
    highest: 131,
    first_key: 60,
};

/// Renoise's note of a MIDI note.
fn renoise(midi: i32) -> i64 {
    (midi - 12).clamp(0, 119) as i64
}

pub struct RenoiseMapping<S: RefScheme + 'static> {
    plan: Rc<OwnPlan<S>>,
    renderer: Rc<OwnRenderer<S>>,
}

impl<S: RefScheme + 'static> RenoiseMapping<S> {
    pub fn new(plan: Rc<OwnPlan<S>>, renderer: Rc<OwnRenderer<S>>) -> Self {
        RenoiseMapping { plan, renderer }
    }

    /// One change by its tool: what Renoise answered, or why not.
    async fn change(&self, tool: &str, input: Value, signal: &Signal) -> Result<Value, String> {
        let outcome = self.plan.change_by(tool, input.as_object().cloned().unwrap_or_default(), signal.clone()).await;
        let answer: Value = serde_json::from_str(&outcome.text).unwrap_or_else(|_| json!({"said": outcome.text}));
        if outcome.is_error || outcome.stops {
            return Err(answer.get("changed").and_then(Value::as_str).map(str::to_owned).unwrap_or(outcome.text));
        }
        Ok(answer)
    }
}

#[async_trait(?Send)]
impl<S: RefScheme + 'static> MappingBackend for RenoiseMapping<S> {
    fn words(&self) -> &'static Words {
        &WORDS
    }

    fn note_name(&self, midi: i32) -> String {
        note_text(renoise(midi) as u8)
    }

    fn note_of(&self, value: &Value) -> Option<i32> {
        let renoise = match value {
            Value::Number(n) => n.as_i64().filter(|n| (0..=119).contains(n))? as i32,
            Value::String(text) => note_value(text.trim())? as i32,
            _ => return None,
        };
        Some(renoise + 12)
    }

    async fn samples_of(&self, instrument: &str, signal: &Signal) -> Result<(String, Vec<Held>), String> {
        let long = self.plan.lengthen(instrument)?;
        let row = self.plan.read_through(json!({"ref": long}), None, signal).await?;
        let name = row.pointer("/items/0/name").and_then(Value::as_str).unwrap_or("").to_string();
        let rows = self.plan.rows_through(json!({"kind": "sample", "parent": long, "limit": 200}), SAMPLE_PAGES, signal).await?;
        let mut held = Vec::new();
        for row in rows {
            // (a slice plays a part of the sliced sample, its keys follow the markers: neither is laid out)
            if row.get("slice") == Some(&Value::Bool(true)) || row.get("keyzoneReadOnly") == Some(&Value::Bool(true)) {
                continue;
            }
            let Some(reference) = row.get("ref").and_then(Value::as_str) else { continue };
            let file = match self.renderer.clip_file(reference, signal.clone()).await {
                Ok(Some(file)) => Ok(file),
                Ok(None) => Err("no audio".to_string()),
                Err(error) => Err(error.to_string()),
            };
            held.push(Held {
                reference: reference.to_string(),
                name: row.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
                file,
            });
        }
        Ok((name, held))
    }

    async fn make(&self, planned: &Planned, name: &str, drums: bool, signal: &Signal) -> Result<Value, String> {
        self.change("instrument_kit", kit_input(planned, name, drums), signal).await
    }

    async fn remap(&self, instrument: &str, planned: &Planned, signal: &Signal) -> Result<Value, String> {
        let long = self.plan.lengthen(instrument)?;
        self.change("keyzone_map", map_input(&long, planned), signal).await
    }

    async fn instruments(&self, signal: &Signal) -> Result<Value, String> {
        let answer = self.plan.read_through(json!({"kind": "tidy"}), None, signal).await?;
        let rows: Vec<Value> = answer
            .get("items")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|row| {
                let mut row = row.clone();
                if let Some(reference) = row.get("ref").and_then(Value::as_str) {
                    row["ref"] = json!(self.plan.link().book.borrow_mut().short_ref(reference));
                }
                row
            })
            .collect();
        Ok(json!({"instruments": rows, "counted": answer.get("counted").and_then(Value::as_bool).unwrap_or(false)}))
    }
}

/// instrument_kit's input for a planned instrument of sound files: a drum kit's keys, or a multi-sample's roots and zones.
fn kit_input(planned: &Planned, name: &str, drums: bool) -> Value {
    let samples: Vec<Value> = planned
        .samples
        .iter()
        .map(|placed| {
            let mut row = json!({"path": placed.id, "name": placed.name});
            if drums {
                row["note"] = json!(renoise(placed.root));
            } else {
                row["root"] = json!(renoise(placed.root));
                row["notes"] = json!([renoise(placed.zone.0), renoise(placed.zone.1)]);
            }
            if placed.velocity != (0, 127) {
                row["velocity"] = json!([placed.velocity.0, placed.velocity.1]);
            }
            row
        })
        .collect();
    let mut input = json!({"samples": samples, "layout": if drums { "drumkit" } else { "keys" }, "name": name});
    if planned.cycle {
        input["overlap"] = json!("cycle");
    }
    input
}

/// keyzone_map's input for an instrument's own samples, as planned.
fn map_input(instrument: &str, planned: &Planned) -> Value {
    let samples: Vec<Value> = planned
        .samples
        .iter()
        .map(|placed| {
            json!({"sampleRef": placed.id, "noteRange": [renoise(placed.zone.0), renoise(placed.zone.1)],
                "velocityRange": [placed.velocity.0, placed.velocity.1], "baseNote": renoise(placed.root)})
        })
        .collect();
    json!({"ref": instrument, "samples": samples, "overlap": if planned.cycle { "cycle" } else { "all" }})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::mapping::{Found, Placed};
    use crate::integrations::renoise_changes::CHANGES;
    use crate::kernel::schema::problem;

    fn placed(id: &str, root: i32, zone: (i32, i32), velocity: (u8, u8)) -> Placed {
        Placed { id: id.into(), name: id.into(), root, zone, velocity, round_robin: None, found: Found::Named }
    }

    fn schema_of(tool: &str) -> Value {
        Value::Object(CHANGES.kinds.iter().find(|kind| kind.tool == tool).unwrap().input_schema())
    }

    #[test]
    fn the_plan_goes_to_renoise_in_its_own_notes_and_fits_each_kinds_input() {
        let planned = Planned {
            part: None,
            cycle: true,
            samples: vec![
                placed("/s/C3.wav", 60, (12, 63), (0, 63)),
                placed("/s/C3b.wav", 60, (12, 63), (64, 127)),
                placed("/s/G3.wav", 67, (64, 131), (0, 127)),
            ],
        };
        let kit = kit_input(&planned, "Piano", false);
        assert_eq!(problem(&schema_of("instrument_kit"), &kit), None, "{kit}");
        assert_eq!(kit["samples"][0]["root"], 48, "MIDI 60 is Renoise's C-4, 48");
        assert_eq!(kit["samples"][0]["notes"], json!([0, 51]));
        assert_eq!(kit["samples"][2]["notes"], json!([52, 119]));
        assert!(kit["samples"][2].get("velocity").is_none(), "the whole range is the default");
        assert_eq!(kit["overlap"], "cycle");
        let drums = kit_input(&planned, "Kit", true);
        assert_eq!(problem(&schema_of("instrument_kit"), &drums), None, "{drums}");
        assert_eq!(drums["samples"][0]["note"], 48);
        let map = map_input("1:instrument:2", &Planned { cycle: false, ..planned });
        assert_eq!(problem(&schema_of("keyzone_map"), &map), None, "{map}");
        assert_eq!(map["samples"][1]["baseNote"], 48);
        assert_eq!(map["overlap"], "all");
    }

    #[test]
    fn notes_read_and_said_as_renoise_writes_them() {
        assert_eq!(renoise(60), 48);
        assert_eq!(note_text(renoise(61) as u8), "C#4");
        assert_eq!(renoise(5), 0, "below C-0 is C-0");
    }
}
