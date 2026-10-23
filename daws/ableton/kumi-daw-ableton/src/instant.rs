//! Live's side of a fixed phrase (core/phrases.rs): the change each one makes, on the Set as last read. The plan
//! itself is core's (`instant_steps`).

use serde_json::{json, Value};

use super::set_model::SetModel;
use kumi_runtime::core::phrases::{instant_steps, InstantVocab, Intent, PhraseTrack, Switch};

/// Live's changes for the fixed phrases.
pub struct LiveInstant;

impl InstantVocab for LiveInstant {
    fn tempo(&self, bpm: f64) -> Value {
        json!({"tool": "set_tempo", "input": {"tempo": bpm}})
    }
    fn track(&self, reference: &str, switch: Switch, on: bool) -> Value {
        match switch {
            Switch::Mute => json!({"tool": "set_mixer", "input": {"trackRef": reference, "mute": on}}),
            Switch::Solo => json!({"tool": "set_mixer", "input": {"trackRef": reference, "solo": on}}),
            Switch::Arm => json!({"tool": "set_routing", "input": {"trackRef": reference, "arm": on}}),
        }
    }
    fn loop_on(&self, start: f64, length: f64) -> Value {
        json!({"tool": "set_transport", "input": {"loopEnabled": true, "loopStart": start, "loopLength": length}})
    }
    fn loop_off(&self) -> Value {
        json!({"tool": "set_transport", "input": {"loopEnabled": false}})
    }
    fn metronome(&self, on: bool) -> Value {
        json!({"tool": "set_transport", "input": {"metronome": on}})
    }
}

/// The make_changes steps for an intent in the Set as last read: None when it can't run as asked. A track counts by
/// place among the Set's own (regular and group) tracks, and Main is never one a phrase names.
pub fn steps(intent: &Intent, model: &SetModel, beats_per_bar: f64) -> Option<Value> {
    let tracks: Vec<PhraseTrack> = model
        .tracks
        .iter()
        .map(|node| PhraseTrack {
            reference: &node.reference,
            name: &node.name,
            own: matches!(node.kind.as_deref(), Some("regular" | "group") | None),
            master: node.kind.as_deref() == Some("main"),
        })
        .collect();
    instant_steps(&LiveInstant, intent, &tracks, beats_per_bar)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kumi_runtime::core::{contracts::JsonObject, phrases::TrackPick};

    fn model() -> SetModel {
        let rows: Vec<JsonObject> = [
            ("1:track:0", "Drums", "regular"),
            ("1:track:1", "Bass", "regular"),
            ("1:track:2", "Keys", "group"),
            ("1:return:0", "A-Reverb", "return"),
            ("1:main", "Main", "main"),
        ]
        .iter()
        .map(|(reference, name, kind)| json!({"ref": reference, "name": name, "kind": kind}).as_object().unwrap().clone())
        .collect();
        SetModel::next(&SetModel::default(), &rows, &[], true)
    }

    fn plan(intent: Intent) -> Option<Value> {
        steps(&intent, &model(), 3.0)
    }

    #[test]
    fn each_phrase_is_one_step_of_its_change() {
        let name = |text: &str| TrackPick::Name(text.into());
        assert_eq!(plan(Intent::Tempo { bpm: 124.0 }).unwrap()["steps"][0], json!({"tool": "set_tempo", "input": {"tempo": 124.0}}));
        assert_eq!(
            plan(Intent::Track { track: name("bass"), switch: Switch::Mute, on: true }).unwrap(),
            json!({"steps": [{"tool": "set_mixer", "input": {"trackRef": "1:track:1", "mute": true}}], "final": true})
        );
        assert_eq!(
            plan(Intent::Track { track: name("drums"), switch: Switch::Solo, on: false }).unwrap()["steps"][0]["input"],
            json!({"trackRef": "1:track:0", "solo": false})
        );
        assert_eq!(
            plan(Intent::Track { track: TrackPick::Position(3), switch: Switch::Arm, on: true }).unwrap()["steps"][0],
            json!({"tool": "set_routing", "input": {"trackRef": "1:track:2", "arm": true}})
        );
        assert_eq!(
            plan(Intent::Track { track: name("a-reverb"), switch: Switch::Mute, on: true }).unwrap()["steps"][0]["input"]["trackRef"],
            "1:return:0",
            "a return by its name"
        );
        // In 3/4, bars 9 to 16 start at beat 24 and last 24 beats.
        assert_eq!(
            plan(Intent::Loop { from_bar: 9, to_bar: 16 }).unwrap()["steps"][0]["input"],
            json!({"loopEnabled": true, "loopStart": 24.0, "loopLength": 24.0})
        );
        assert_eq!(plan(Intent::LoopOff).unwrap()["steps"][0]["input"], json!({"loopEnabled": false}));
        assert_eq!(
            plan(Intent::Metronome { on: true }).unwrap()["steps"][0],
            json!({"tool": "set_transport", "input": {"metronome": true}})
        );
    }

    #[test]
    fn a_track_that_isnt_there_or_isnt_one_makes_no_plan() {
        let pick = |track: TrackPick| plan(Intent::Track { track, switch: Switch::Mute, on: true });
        assert_eq!(pick(TrackPick::Name("guitar".into())), None);
        assert_eq!(pick(TrackPick::Name("main".into())), None, "Main isn't a track to mute");
        assert_eq!(pick(TrackPick::Position(4)), None, "returns don't count as places");
    }
}
