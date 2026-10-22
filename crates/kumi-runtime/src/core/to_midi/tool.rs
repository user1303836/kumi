//! The audio_to_midi tool, the same in every DAW without a converter of its own: renders a track over a stretch, hears it
//! with Kumi's own ears (drums by the chop's hit detector, pitched parts by Basic Pitch) and writes the notes on a new track.
use super::backend::ToMidiBackend;
use super::{drums, pitched, said};
use crate::core::contracts::{JsonObject, KernelTool, Reach, ToolResult};
use crate::core::errors::RuntimeError;
use async_trait::async_trait;
use kumi_common::abort::Signal;
use kumi_common::js::json::stringify;
use serde_json::{json, Value};
use std::rc::Rc;

pub const TOOL: &str = "audio_to_midi";

/// The longest stretch heard (quarter notes): Basic Pitch takes a while on a long one.
const MOST_BEATS: f64 = 128.;

pub struct ToMidi<B: ToMidiBackend> {
    backend: B,
    description: String,
}

impl<B: ToMidiBackend + 'static> ToMidi<B> {
    pub fn new(backend: B) -> Rc<Self> {
        let description = description(backend.words());
        Rc::new(ToMidi { backend, description })
    }

    pub fn tool(self: &Rc<Self>) -> Rc<dyn KernelTool> {
        Rc::new(ToMidiTool { owner: self.clone() })
    }

    async fn run(&self, input: JsonObject, signal: Signal) -> Result<Value, String> {
        let track = input.get("track").and_then(Value::as_str).ok_or("track: the audio track to turn into MIDI")?;
        let what = input.get("as").and_then(Value::as_str).unwrap_or("drums");
        if !matches!(what, "drums" | "melody" | "harmony") {
            return Err("as is drums, melody (the highest note at each moment) or harmony (every note)".into());
        }
        let number = |key: &str| input.get(key).and_then(Value::as_f64);
        let (from, beats) = match number("fromBeat").zip(number("beats")) {
            Some((from, beats)) if from >= 0. && beats > 0. && beats <= MOST_BEATS => (from, beats),
            _ => return Err(format!("fromBeat and beats: the stretch to hear (at most {MOST_BEATS} beats)")),
        };
        let per_second = self.backend.tempo(from, &signal).await? / 60.;
        let (render, name) = self.backend.render(track, from, beats, &signal).await?;
        let seconds = beats / per_second;
        let notes = if what == "drums" {
            let (mono, rate, _) =
                crate::audio::decode::read_mono(&render.file, &signal, render.start, seconds).await.map_err(|error| error.0)?;
            drums(&crate::core::chop::hits::hits(&mono, rate), per_second, beats)
        } else {
            let say = |_: &str| {};
            let heard =
                crate::listening::transcribe::pitched_notes(std::path::Path::new(&render.file), render.start, seconds, &say, &signal)
                    .await?;
            pitched(&heard, per_second, beats, what == "melody")
        };
        let words = said(&notes, what == "drums", |pitch| self.backend.note_name(pitch));
        if notes.is_empty() {
            return Ok(json!({"heard": words, "said": format!("Kumi heard no {what} in “{name}” over that stretch: nothing written")}));
        }
        let options = input.get("options").and_then(Value::as_object).cloned().unwrap_or_default();
        let midi_name = format!("{name} {}", if what == "drums" { "drums" } else { what });
        let made = self.backend.write(track, &midi_name, &notes, what == "drums", from, beats, &options, &signal).await?;
        Ok(
            json!({"heard": words, "track": midi_name, "made": made, "next": "say what was heard; offer to tidy it (quantize) or to try another stretch"}),
        )
    }
}

fn description(words: &super::backend::Words) -> String {
    format!(
        "Turn a track's audio in {app} into MIDI with Kumi's own ears: track (an audio track), as drums (each hit a kick, snare or hat on General MIDI's drum keys, as loud as it hits, a ghost softer), melody (the highest note at each moment, by Basic Pitch) or harmony (every note), over fromBeat and beats (at most 128). The notes go on {written}. options: {options}. Kumi renders the track, so nothing plays meanwhile.",
        app = words.app,
        written = words.written,
        options = words.options
    )
}

fn schema(words: &super::backend::Words) -> JsonObject {
    let options: Value = serde_json::from_str(words.options_schema).unwrap_or_else(|_| json!({"type": "object"}));
    let value = json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["track", "fromBeat", "beats"],
        "properties": {
            "track": {"type": "string", "description": "The audio track to turn into MIDI"},
            "as": {"type": "string", "enum": ["drums", "melody", "harmony"]},
            "fromBeat": {"type": "number", "minimum": 0},
            "beats": {"type": "number", "exclusiveMinimum": 0, "maximum": MOST_BEATS},
            "options": options
        }
    });
    value.as_object().cloned().unwrap_or_default()
}

struct ToMidiTool<B: ToMidiBackend> {
    owner: Rc<ToMidi<B>>,
}

#[async_trait(?Send)]
impl<B: ToMidiBackend + 'static> KernelTool for ToMidiTool<B> {
    fn name(&self) -> &str {
        TOOL
    }

    fn description(&self) -> &str {
        &self.owner.description
    }

    fn input_schema(&self) -> JsonObject {
        schema(self.owner.backend.words())
    }

    async fn execute(&self, input: JsonObject, signal: Signal) -> Result<ToolResult, RuntimeError> {
        Ok(match self.owner.run(input, signal).await {
            Ok(reply) => ToolResult { text: stringify(&reply), ..Default::default() },
            Err(why) => ToolResult { text: why, is_error: true, ..Default::default() },
        })
    }

    fn reach(&self) -> Reach {
        Reach::DawWrite
    }

    fn deferred(&self) -> bool {
        true
    }
}
