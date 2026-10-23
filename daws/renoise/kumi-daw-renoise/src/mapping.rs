//! Renoise's backend for the sample map (#401): only Renoise's facts. Its notes are MIDI's numbers (C-5 is 60, its keyzones
//! map 0 to 119, a drum kit starts at C-4), an instrument's samples are read (their zones, layer and transpose) and each one's
//! audio saved to Kumi's folder to be heard, a new instrument of planned files is one instrument_kit change, an instrument's own
//! samples are laid out by one keyzone_map change, and the song's instruments for the tidy come in pages of reads. Core's tool
//! does the rest.
use async_trait::async_trait;
use kumi_common::abort::Signal;
use kumi_daw_own::changes::OwnPlan;
use kumi_daw_own::listening::OwnRenderer;
use kumi_listening::renderer::Renderer;
use kumi_runtime::core::{
    mapping::{
        backend::{Held, MappingBackend, Words},
        Planned,
    },
    refs::RefScheme,
};
use kumi_types::daw::DawProfile;
use serde_json::{json, Map, Value};
use std::rc::Rc;

/// Pages of sample rows read for an instrument (200 a page).
const SAMPLE_PAGES: usize = 2;
/// The most pages of the tidy's reads: its instruments' prints, and the count of their notes (each page about a second of
/// Renoise's time); a song past them is said to be counted only so far.
const TIDY_PAGES: usize = 16;
const COUNT_PAGES: usize = 60;

pub static WORDS: Words = Words {
    app: "Renoise",
    profile: &DawProfile::RENOISE,
    instrument: "instrument",
    // (Renoise's note values are MIDI's: its keyzones map 0 to 119, and instrument_kit's drum kit starts at C-4, 48)
    lowest: 0,
    highest: 119,
    first_key: 48,
};

/// Renoise's note value of a MIDI note: the same number, within its keyzones' range.
fn renoise(midi: i32) -> i64 {
    midi.clamp(0, 119) as i64
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

    /// A paged read of the tool's own (`tidy`, `tidy-notes`): each page's answer, up to `pages` of them, and whether it ended.
    async fn pages(&self, kind: &str, pages: usize, signal: &Signal) -> Result<(Vec<Value>, bool), String> {
        let mut out = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..pages {
            let mut asked = json!({"kind": kind});
            if let Some(cursor) = &cursor {
                asked["cursor"] = cursor.clone().into();
            }
            let page = self.plan.read_through(asked, None, signal).await?;
            let next = page.get("nextCursor").and_then(Value::as_str).map(str::to_owned);
            out.push(page);
            match next {
                Some(next) => cursor = Some(next),
                None => return Ok((out, true)),
            }
        }
        Ok((out, false))
    }
}

/// A row's [low, high], or the whole range.
fn range_of(row: &Value, key: &str, high: i64) -> (i64, i64) {
    let at = |k: usize| row.get(key).and_then(|range| range.get(k)).and_then(Value::as_i64);
    (at(0).unwrap_or(0), at(1).unwrap_or(high))
}

#[async_trait(?Send)]
impl<S: RefScheme + 'static> MappingBackend for RenoiseMapping<S> {
    fn words(&self) -> &'static Words {
        &WORDS
    }

    async fn samples_of(&self, instrument: &str, most: usize, signal: &Signal) -> Result<(String, Vec<Held>), String> {
        let long = self.plan.lengthen(instrument)?;
        let row = self.plan.read_through(json!({"ref": long}), None, signal).await?;
        let name = row.pointer("/items/0/name").and_then(Value::as_str).unwrap_or("").to_string();
        let rows = self.plan.rows_through(json!({"kind": "sample", "parent": long, "limit": 200}), SAMPLE_PAGES, signal).await?;
        // (a slice plays a part of the sliced sample, its keys follow the markers: neither is laid out)
        let rows: Vec<Value> = rows
            .into_iter()
            .filter(|row| row.get("slice") != Some(&Value::Bool(true)) && row.get("keyzoneReadOnly") != Some(&Value::Bool(true)))
            .collect();
        if rows.len() > most {
            return Err(format!("that instrument has {} samples: Kumi maps at most {most} at once", rows.len()));
        }
        let mut held = Vec::new();
        for row in rows {
            let Some(reference) = row.get("ref").and_then(Value::as_str) else { continue };
            let file = match self.renderer.clip_file(reference, signal.clone()).await {
                Ok(Some(file)) => Ok(file),
                Ok(None) => Err("no audio".to_string()),
                Err(error) => Err(error.to_string()),
            };
            let (low, high) = range_of(&row, "noteRange", 119);
            let (soft, loud) = range_of(&row, "velocityRange", 127);
            held.push(Held {
                reference: reference.to_string(),
                name: row.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
                file,
                zone: (low as i32, high as i32),
                velocity: (soft.clamp(0, 127) as u8, loud.clamp(0, 127) as u8),
                transpose: row.get("transpose").and_then(Value::as_i64).unwrap_or(0) as i32,
                note_off: row.get("layer").and_then(Value::as_str) == Some("note-off"),
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
        let (pages, whole) = self.pages("tidy", TIDY_PAGES, signal).await?;
        if !whole {
            return Err("the song has too many instruments and samples to look over in one go".into());
        }
        let mut rows: Vec<Value> =
            pages.iter().flat_map(|page| page.get("items").and_then(Value::as_array).cloned().unwrap_or_default()).collect();
        let (counts, counted) = self.pages("tidy-notes", COUNT_PAGES, signal).await?;
        let mut notes: Map<String, Value> = Map::new();
        let (mut to, mut patterns) = (0, 0);
        for page in &counts {
            for (index, n) in page.get("counts").and_then(Value::as_object).into_iter().flatten() {
                let sum = notes.get(index).and_then(Value::as_u64).unwrap_or(0) + n.as_u64().unwrap_or(0);
                notes.insert(index.clone(), json!(sum));
            }
            to = page.get("to").and_then(Value::as_u64).unwrap_or(to);
            patterns = page.get("patterns").and_then(Value::as_u64).unwrap_or(patterns);
        }
        for row in &mut rows {
            let index = row.get("index").and_then(Value::as_u64).unwrap_or(0).to_string();
            row["notes"] = json!(notes.get(&index).and_then(Value::as_u64).unwrap_or(0));
            if let Some(reference) = row.get("ref").and_then(Value::as_str) {
                row["ref"] = json!(self.plan.link().book.borrow_mut().short_ref(reference));
            }
        }
        Ok(json!({"instruments": rows, "counted": counted, "countedTo": to, "patterns": patterns}))
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
                row["root"] = json!(renoise(placed.base));
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

/// keyzone_map's input for an instrument's own samples, as planned (each base note allows for its sample's transpose).
fn map_input(instrument: &str, planned: &Planned) -> Value {
    let samples: Vec<Value> = planned
        .samples
        .iter()
        .map(|placed| {
            json!({"sampleRef": placed.id, "noteRange": [renoise(placed.zone.0), renoise(placed.zone.1)],
                "velocityRange": [placed.velocity.0, placed.velocity.1], "baseNote": renoise(placed.base)})
        })
        .collect();
    json!({"ref": instrument, "samples": samples, "overlap": if planned.cycle { "cycle" } else { "all" }})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::changes::CHANGES;
    use kumi_runtime::core::mapping::{Found, Placed};
    use kumi_runtime::kernel::schema::problem;

    fn placed(id: &str, root: i32, base: i32, zone: (i32, i32), velocity: (u8, u8)) -> Placed {
        Placed { id: id.into(), name: id.into(), root, base, zone, velocity, round_robin: None, found: Found::Named }
    }

    fn schema_of(tool: &str) -> Value {
        Value::Object(CHANGES.kinds.iter().find(|kind| kind.tool == tool).unwrap().input_schema())
    }

    #[test]
    fn the_plan_goes_to_renoise_as_midi_numbers_and_fits_each_kinds_input() {
        let planned = Planned {
            part: None,
            cycle: true,
            samples: vec![
                placed("/s/C4.wav", 60, 60, (0, 63), (0, 63)),
                placed("/s/C4b.wav", 60, 62, (0, 63), (64, 127)),
                placed("/s/G4.wav", 67, 67, (64, 119), (0, 127)),
            ],
        };
        let kit = kit_input(&planned, "Piano", false);
        assert_eq!(problem(&schema_of("instrument_kit"), &kit), None, "{kit}");
        assert_eq!(kit["samples"][0]["root"], 60, "MIDI 60 is Renoise's C-5, 60");
        assert_eq!(kit["samples"][1]["root"], 62, "a transposed sample's base allows for it");
        assert_eq!(kit["samples"][0]["notes"], json!([0, 63]));
        assert_eq!(kit["samples"][2]["notes"], json!([64, 119]));
        assert!(kit["samples"][2].get("velocity").is_none(), "the whole range is the default");
        assert_eq!(kit["overlap"], "cycle");
        let drums = kit_input(&planned, "Kit", true);
        assert_eq!(problem(&schema_of("instrument_kit"), &drums), None, "{drums}");
        assert_eq!(drums["samples"][0]["note"], 60);
        let map = map_input("1:instrument:2", &Planned { cycle: false, ..planned });
        assert_eq!(problem(&schema_of("keyzone_map"), &map), None, "{map}");
        assert_eq!(map["samples"][1]["baseNote"], 62);
        assert_eq!(map["overlap"], "all");
    }

    #[test]
    fn a_gm_kick_lands_on_renoises_c_3() {
        // (GM's kick is MIDI 36, which Renoise names C-3)
        assert_eq!(renoise(36), 36);
        assert_eq!(DawProfile::RENOISE.note_name(36), "C-3");
        assert_eq!(renoise(130), 119, "past Renoise's keys is its top key");
    }
}
