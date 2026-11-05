use async_trait::async_trait;
use kumi_common::js::json::stringify;
use kumi_daw_ableton::changes::*;
use kumi_types::errors::RuntimeError;
use serde_json::{json, Value};
use std::{cell::RefCell, collections::HashSet};
fn oracle() -> Value {
    serde_json::from_str(&kumi_testkit::oracle("kumi-daw-ableton/changes-oracle.json")).unwrap()
}
struct Context {
    calls: RefCell<Vec<Value>>,
    parameters: Vec<ParameterRange>,
    display: bool,
    /// The pads a Drum Rack shows: none read, when there are none.
    pads: Option<Vec<DrumPad>>,
}
#[async_trait(?Send)]
impl ChangeContext for Context {
    async fn sample(&self, path: &str) -> Result<Result<SampleFile, NoSample>, RuntimeError> {
        self.calls.borrow_mut().push(json!(["sample", path]));
        Ok(path.starts_with("/samples/").then(|| SampleFile { path: path.into(), folder: "/samples".into() }).ok_or(NoSample::NotThere))
    }
    async fn parameters(&self, device: &str) -> Result<Vec<ParameterRange>, RuntimeError> {
        self.calls.borrow_mut().push(json!(["parameters", device]));
        Ok(self.parameters.clone())
    }
    async fn ranges(&self, device: &str) -> Result<Vec<ParameterRange>, RuntimeError> {
        self.calls.borrow_mut().push(json!(["ranges", device]));
        Ok(self.parameters.clone())
    }
    async fn pick(&self, selector: SampleSelector) -> Result<Result<SampleFile, NoSample>, RuntimeError> {
        self.calls.borrow_mut().push(json!(["pick", selector]));
        Ok((!selector.words.iter().any(|w| w == "missing"))
            .then(|| SampleFile { path: "/samples/Picked.wav".into(), folder: "/samples".into() })
            .ok_or(NoSample::NotThere))
    }
    fn has_value_for(&self) -> bool {
        self.display
    }
    async fn value_for(&self, reference: &str, text: &str) -> Result<Result<f64, String>, RuntimeError> {
        self.calls.borrow_mut().push(json!(["valueFor", reference, text]));
        Ok(if text == "-6 dB" { Ok(0.625) } else { Err("unplaceable".into()) })
    }
    async fn drum_pads(&self, device: &str) -> Option<Vec<DrumPad>> {
        self.calls.borrow_mut().push(json!(["drumPads", device]));
        self.pads.clone()
    }
    async fn mixer_for_display(&self, track: &str, asked: &Value) -> Result<Result<Value, String>, RuntimeError> {
        self.calls.borrow_mut().push(json!(["mixerForDisplay", track, asked]));
        // As Live 12.4 shows it: its own labels for the fader and the sends (to a thousandth of a dB, "2.555 dB"), and
        // its own pan labels.
        let level = |label: &Value| match label.as_str() {
            Some("-6.0 dB") => json!({"value": 0.6667, "shown": "-6.0 dB"}),
            Some("2.0 dB") => json!({"value": 0.9112, "shown": "2.0 dB"}),
            Some("-inf dB") => json!({"value": 0.0, "shown": "-inf dB"}),
            Some("-12.0 dB") => json!({"value": 0.5223, "shown": "-12.0 dB"}),
            Some(_) => json!({"near": ["2.555 dB", "2.556 dB"]}),
            None => Value::Null,
        };
        let pan = match asked["pan"].as_str() {
            Some("25L") => json!({"value": -0.5, "shown": "25L"}),
            Some(_) => json!({"near": ["49L", "50L"]}),
            None => Value::Null,
        };
        let sends: Vec<Value> = asked["sends"].as_array().map(|sends| sends.iter().map(level).collect()).unwrap_or_default();
        Ok(Ok(json!({"volume": level(&asked["volume"]), "pan": pan, "sends": sends})))
    }
}
fn kind(case: &Value) -> &'static ChangeKind {
    CHANGES.iter().find(|k| k.tool == case["tool"]).unwrap()
}
#[tokio::test]
async fn change_preparation_and_parameter_explanations_match_complete_source_results() {
    let data = oracle();
    for case in data["prepared"].as_array().unwrap() {
        let context = Context {
            calls: RefCell::new(vec![]),
            parameters: serde_json::from_value(data["parameters"].clone()).unwrap(),
            display: case["display"].as_bool().unwrap(),
            pads: None,
        };
        let result = match kind(case).prepare(case["input"].as_object().unwrap(), &context).await.unwrap() {
            Ok(value) => Value::Object(value),
            Err(why) => json!(why),
        };
        assert_eq!(stringify(&result), stringify(&case["value"]), "{case}");
        assert_eq!(json!(*context.calls.borrow()), case["calls"], "{case}");
    }
    for case in data["explanations"].as_array().unwrap() {
        let context = Context {
            calls: RefCell::new(vec![]),
            parameters: serde_json::from_value(data["parameters"].clone()).unwrap(),
            display: false,
            pads: None,
        };
        assert_eq!(
            serde_json::to_value(
                kind(case).explain(case["error"].as_str().unwrap(), case["input"].as_object().unwrap(), &context).await.unwrap()
            )
            .unwrap(),
            case["value"]
        );
    }
}
#[test]
fn change_schemas_outputs_permanence_and_human_messages_match_source() {
    let data = oracle();
    for case in data["schemas"].as_array().unwrap() {
        assert_eq!(Value::Object(kind(case).schema(case["input"].as_object().unwrap())), case["value"]);
    }
    for case in data["produced"].as_array().unwrap() {
        assert_eq!(serde_json::to_value(kind(case).produces(case["input"].as_object().unwrap())).unwrap(), case["value"]);
    }
    for case in data["permanent"].as_array().unwrap() {
        assert_eq!(serde_json::to_value(kind(case).permanent(case["input"].as_object().unwrap())).unwrap(), case["value"]);
    }
    for case in data["undo"].as_array().unwrap() {
        assert_eq!(undo_note(case["message"].as_str().unwrap()), case["value"]);
    }
    for case in data["numbers"].as_array().unwrap() {
        let value = case["value"].as_f64().unwrap();
        assert_eq!(note_name(value), case["note"]);
        assert_eq!(serde_json::to_value(hex_color(&case["value"])).unwrap(), case["color"]);
        assert_eq!(format_number(value, None), case["format"]);
    }
    let kinds = crate::mixer_in_db::legacy_kinds(&crate::pads_in_order::legacy_json(&serde_json::to_value(&*CHANGES).unwrap()));
    assert_eq!(kinds, data["kinds"]);
    assert_eq!(*SAMPLE_INPUT, data["sampleInput"].as_object().unwrap().clone());
    assert_eq!(*REFERENCE_FIELDS, serde_json::from_value::<Vec<String>>(data["referenceFields"].clone()).unwrap());
    assert_eq!(
        HOST_TOOLS.iter().cloned().collect::<HashSet<_>>(),
        serde_json::from_value::<HashSet<String>>(data["hostTools"].clone()).unwrap()
    );
    assert_eq!(*UNDO_DESCRIPTION, data["undoDescription"]);
}

#[test]
fn all_change_summaries_match_complete_source_outputs() {
    let data: Value = serde_json::from_str(&kumi_testkit::oracle("kumi-daw-ableton/change-summaries-oracle.json")).unwrap();
    let track =
        |value: &Value| value.as_str().and_then(|key| data["tracks"].get(key)).map(|value| serde_json::from_value(value.clone()).unwrap());
    let values = data["values"].as_array().unwrap();
    for compact in data["cases"].as_array().unwrap() {
        let mut case = compact.clone();
        for (key, value) in case.as_object_mut().unwrap() {
            if key != "tool" {
                *value = values[value.as_u64().unwrap() as usize].clone();
            }
        }
        let meter = &case["meter"];
        kumi_daw_ableton::more_changes::set_meter(meter[0].as_f64().unwrap(), meter[1].as_f64().unwrap());
        let result = kind(&case).summarize(
            case["preview"].as_object().unwrap(),
            case["input"].as_object().unwrap(),
            &track,
            case.get("applied").and_then(Value::as_object),
        );
        let mut actual: Value = serde_json::from_str(&stringify(&serde_json::to_value(result).unwrap())).unwrap();
        // Pictures came after the port, with tests of their own (changes/pictures.rs): the summaries are the source's.
        actual.as_object_mut().unwrap().remove("picture");
        assert_eq!(actual, case["value"], "{case}");
    }
}
#[test]
fn a_move_that_replaced_clips_says_what_and_kumi_keeps_it() {
    let kind = CHANGES.iter().find(|k| k.tool == "move_clip").unwrap();
    let preview = json!({"replaces":[
        {"name":"Chorus","start":12,"end":16,"from":12,"to":14,"whole":false},
        {"name":"Fill","start":9,"end":10,"from":9,"to":10,"whole":true}
    ]});
    let input = json!({"clipRef":"1:arrangement_clip:0:0","position":8});
    let summary = kind.summarize(preview.as_object().unwrap(), input.as_object().unwrap(), &|_| None, None);
    assert_eq!(summary.title, "Moved a clip to bar 3, replacing bar 4 to bar 4 beat 3 of “Chorus”, “Fill”");
    assert_eq!(
        kind.replaced(preview.as_object().unwrap()).as_deref(),
        Some("Kumi can't bring back what the move replaced; Live's own undo can.")
    );
    assert_eq!(kind.replaced(json!({}).as_object().unwrap()), None, "a move that replaced nothing undoes in Kumi");
    let mut cut_first = preview.clone();
    cut_first["payload"] = json!({"clearFirst":[{"trackRef":"1:track:0","fromBeat":12,"toBeat":14}]});
    assert!(kind.replaced(cut_first.as_object().unwrap()).unwrap().ends_with("in two steps (the move, then the cut)."));
    // A move between two audio clips cuts each: Live's undo takes the move and both cuts.
    cut_first["payload"]["clearFirst"] =
        json!([{"trackRef":"1:track:0","fromBeat":12,"toBeat":14},{"trackRef":"1:track:0","fromBeat":16,"toBeat":17}]);
    assert!(kind.replaced(cut_first.as_object().unwrap()).unwrap().ends_with("in 3 steps (the move, then each cut)."));
}

#[test]
fn set_audio_clip_offers_no_fades_live_cant_set() {
    // Live's API has no clip fades: the model isn't offered the bridge's fields for them, and they're refused if given.
    let kind = CHANGES.iter().find(|kind| kind.tool == "set_audio_clip").unwrap();
    let host = json!({"type":"object","properties":{"clipRef":{"type":"string"},"gain":{"type":"number"},"fadeInLength":{"type":"number"},"fadeOutLength":{"type":"number"}}});
    let schema = kind.schema(host.as_object().unwrap());
    assert_eq!(schema["properties"].as_object().unwrap().keys().collect::<Vec<_>>(), ["clipRef", "gain"]);
    assert!(kind.has("prepare") && !kind.description.contains("fadeInLength"));
}

#[test]
fn a_new_arrangement_clip_says_what_it_replaced() {
    // Live lays a new clip over the clips it lands on, as a drop does: the summary names each, with the bars it cut.
    kumi_daw_ableton::more_changes::set_meter(4.0, 4.0);
    let kind = CHANGES.iter().find(|kind| kind.tool == "add_arrangement_clip").unwrap();
    let clips: Vec<_> = [("Hats", 16.0, 24.0), ("Fill", 26.0, 28.0), ("Hook", 40.0, 44.0)]
        .iter()
        .map(|(name, start, end)| json!({"name":name,"start":start,"endTime":end}).as_object().unwrap().clone())
        .collect();
    let replaces = laid_over(&clips, 20.0, 28.0);
    assert_eq!(replaces.len(), 2, "Hook is clear of it");
    let preview = json!({"payload":{"trackRef":"1:track:0","position":20,"length":8},"replaces":replaces});
    let input = json!({"trackRef":"1:track:0","position":20,"length":8,"name":"Keys"});
    let summary = kind.summarize(preview.as_object().unwrap(), input.as_object().unwrap(), &|_| None, None);
    assert_eq!(summary.title, "New Arrangement clip “Keys” at bar 6 (2 bars), replacing bar 6 to bar 7 of “Hats”, “Fill”");
    assert_eq!(
        kind.replaced(preview.as_object().unwrap()).as_deref(),
        Some("Kumi can't bring back what the new clip replaced; Live's own undo can.")
    );
    // An audio clip: its file's length, known once it's made, sets what it covers.
    let preview =
        json!({"payload":{"trackRef":"1:track:0","position":20,"filePath":"/samples/Vox.wav"},"replaces":laid_over(&clips, 20.0, 52.0)});
    let input = json!({"trackRef":"1:track:0","position":20,"sample":"/samples/Vox.wav"});
    let summary = kind.summarize(preview.as_object().unwrap(), input.as_object().unwrap(), &|_| None, None);
    assert_eq!(summary.title, "New Arrangement audio clip “Vox” at bar 6, replacing bar 6 to bar 7 of “Hats”, “Fill”, “Hook”");
    // Nothing under it: nothing said, and Kumi's undo takes it back.
    let clear = json!({"payload":{"trackRef":"1:track:0","position":0,"length":4},"replaces":[]});
    assert!(!kind.summarize(clear.as_object().unwrap(), input.as_object().unwrap(), &|_| None, None).title.contains("replacing"));
    assert_eq!(kind.replaced(clear.as_object().unwrap()), None);
}

#[test]
fn a_new_clip_whose_cut_kumi_couldnt_tell_leaves_its_undo_to_live() {
    // A read of the track's clips that failed, or clips cut past what the read before explains: Kumi's own undo, which
    // deletes the new clip, wouldn't bring back what was cut, so it's left to Live's.
    let kind = CHANGES.iter().find(|kind| kind.tool == "add_arrangement_clip").unwrap();
    let unknown = json!({"payload":{"trackRef":"1:track:0","position":0,"length":4},"replaces":[],"replacesUnknown":true});
    assert_eq!(
        kind.replaced(unknown.as_object().unwrap()).as_deref(),
        Some("Kumi couldn't tell what the new clip cut where it landed, so it leaves taking it back to Live's own undo.")
    );
}

#[test]
fn an_arrangement_copy_says_so_and_another_track_is_named() {
    kumi_daw_ableton::more_changes::set_meter(4.0, 4.0);
    let kind = CHANGES.iter().find(|kind| kind.tool == "move_clip").unwrap();
    let bass = |value: &Value| (value == "7:track:2").then(|| serde_json::from_value(json!({"name":"Bass"})).unwrap());
    let summary = |input: Value| kind.summarize(json!({}).as_object().unwrap(), input.as_object().unwrap(), &bass, None).title;
    assert_eq!(summary(json!({"clipRef":"7:arrangement_clip:1:0","position":16,"keepSource":true})), "Copied a clip to bar 5");
    assert_eq!(
        summary(json!({"clipRef":"7:arrangement_clip:1:0","position":16,"targetTrackRef":"7:track:2"})),
        "Moved a clip to bar 5 on Bass"
    );
    assert_eq!(
        summary(json!({"clipRef":"7:arrangement_clip:1:0","position":16,"keepSource":true,"targetTrackRef":"7:track:2"})),
        "Copied a clip to bar 5 on Bass"
    );
    assert_eq!(summary(json!({"clipRef":"7:arrangement_clip:1:0","position":16})), "Moved a clip to bar 5");
}

#[test]
fn native_editing_maps_to_guarded_tools_and_discloses_group_history_limit() {
    let kind = CHANGES.iter().find(|kind| kind.tool == "group_tracks").unwrap();
    assert_eq!(kind.preview, "live_native_editing_preview");
    assert_eq!(kind.apply, "live_native_editing_apply");
    assert!(HOST_TOOLS.contains(&kind.preview));
    assert!(HOST_TOOLS.contains(&kind.apply));
    assert!(kind.replaced(json!({"undoable":false}).as_object().unwrap()).unwrap().contains("no Kumi history inverse"));
    assert!(kind.replaced(json!({"undoable":true}).as_object().unwrap()).is_none());
}

#[test]
fn every_change_kind_has_a_history_summary() {
    let empty = serde_json::Map::new();
    for kind in CHANGES.iter() {
        let summary = kind.summarize(&empty, &empty, &|_| None, None);
        assert!(!summary.title.is_empty(), "{} needs a HISTORY title", kind.tool);
    }
}

#[test]
fn native_titles_use_visible_targets_and_only_changed_scene_settings() {
    kumi_daw_ableton::more_changes::set_meter(4.0, 4.0);
    let scene = CHANGES.iter().find(|kind| kind.tool == "set_scene_follow_actions").unwrap();
    let preview = json!({"prior":{"value":{"action_a":4,"chance_a":100,"chance_b":0,"loop_count":1,"time":4}},
        "proposed":{"value":{"action_a":4,"chance_a":35,"chance_b":65,"loop_count":2,"time":0.3333333333333333}}});
    assert_eq!(
        scene.summarize(preview.as_object().unwrap(), json!({"ref":"7:scene:1"}).as_object().unwrap(), &|_| None, None).title,
        "Scene 2 Follow Actions: time 0.33 beats, loop count 2, chance 35% / 65%"
    );
    let note = CHANGES.iter().find(|kind| kind.tool == "set_note_expression").unwrap();
    let bass = |value: &Value| (value == "7:track:2").then(|| serde_json::from_value(json!({"name":"Bass"})).unwrap());
    assert_eq!(
        note.summarize(
            json!({"prior":{"note":{"pitch":60,"start_time":4}}}).as_object().unwrap(),
            json!({"ref":"7:clip:2:0","noteId":987,"dimension":"pressure"}).as_object().unwrap(),
            &bass,
            None
        )
        .title,
        "Changed pressure on C3 at bar 2 in Bass"
    );
}

/// A kit's samples given no note fill the rack's empty pads in order, row by row from its first (C1, C#1, D1, D#1, then
/// E1 above on a new rack): past the pads that hold something and the notes given to the others.
#[tokio::test]
async fn samples_given_no_note_fill_the_racks_next_empty_pads_in_order() {
    let change = |tool: &str| CHANGES.iter().find(|kind| kind.tool == tool).unwrap();
    let rack = |shown: std::ops::Range<u8>, holding: &[u8]| {
        Some(shown.map(|note| DrumPad { note, name: format!("Pad {note}"), holds: holding.contains(&note) }).collect::<Vec<_>>())
    };
    let context = |pads| Context { calls: RefCell::new(vec![]), parameters: vec![], display: false, pads };
    let one = |input: Value, context: Context| async move {
        let prepared = change("load_sample_to_pad").prepare(input.as_object().unwrap(), &context).await.unwrap();
        (prepared.map(|out| out["note"].clone()), context.calls.into_inner())
    };
    let kick = json!({"deviceRef": "d1", "sample": "/samples/Kick.wav"});
    // One sample: the rack's first empty pad (C1 on a new rack; D1 past two that hold sounds; the first a scrolled
    // rack shows).
    assert_eq!(one(kick.clone(), context(rack(36..52, &[]))).await.0, Ok(json!(36)));
    assert_eq!(one(kick.clone(), context(rack(36..52, &[36, 37]))).await.0, Ok(json!(38)));
    assert_eq!(one(kick.clone(), context(rack(52..68, &[]))).await.0, Ok(json!(52)));
    // A note given is the producer's pad, read from nothing.
    let (given, calls) = one(json!({"deviceRef": "d1", "sample": "/samples/Kick.wav", "note": 42}), context(rack(36..52, &[]))).await;
    assert_eq!((given, calls), (Ok(json!(42)), vec![json!(["sample", "/samples/Kick.wav"])]));
    // A full rack, or one Kumi can't read: said.
    let full = one(kick.clone(), context(rack(36..52, &(36..52).collect::<Vec<_>>()))).await.0.unwrap_err();
    assert!(full.contains("0 of them are empty, for 1 sample:") && full.contains("C1 to D#2"), "{full}");
    assert!(one(kick, context(None)).await.0.unwrap_err().contains("couldn't read the Drum Rack's pads"));
    // A kit, loaded as one change: the pads read once, its samples on C1 to D#1 in the order given; a note given
    // keeps its pad, and the others go round it.
    let kit = |pads: Value, context: Context| async move {
        let input = json!({"deviceRef": "d1", "pads": pads});
        let prepared = change("load_samples_to_pads").prepare(input.as_object().unwrap(), &context).await.unwrap().unwrap();
        let notes: Vec<Value> = prepared["pads"].as_array().unwrap().iter().map(|pad| pad["note"].clone()).collect();
        let reads = context.calls.into_inner().into_iter().filter(|call| call[0] == "drumPads").count();
        (notes, reads)
    };
    let four = json!([{"sample": "/samples/Kick.wav"}, {"sample": "/samples/Clap.wav"}, {"sample": "/samples/Hat.wav"}, {"sample": "/samples/Open.wav"}]);
    assert_eq!(kit(four, context(rack(36..52, &[]))).await, (vec![json!(36), json!(37), json!(38), json!(39)], 1));
    let round = json!([{"sample": "/samples/Kick.wav"}, {"sample": "/samples/Clap.wav", "note": 37}, {"sample": "/samples/Hat.wav"}]);
    assert_eq!(kit(round, context(rack(36..52, &[36]))).await, (vec![json!(38), json!(37), json!(39)], 1));
    // Every note given: nothing read.
    let named = json!([{"sample": "/samples/Kick.wav", "note": 36}, {"sample": "/samples/Snare.wav", "note": 38}]);
    assert_eq!(kit(named, context(None)).await, (vec![json!(36), json!(38)], 0));
}

fn mixer_context() -> Context {
    Context { calls: RefCell::new(vec![]), parameters: vec![], display: false, pads: None }
}

/// The level a producer names in dB (or a pan as Live shows it) is set where Live itself shows it: Live's fader curve
/// isn't linear, so no formula gives "+2 dB".
#[tokio::test]
async fn a_mixer_level_given_as_live_shows_it_is_set_where_live_shows_it() {
    let mixer = kind(&json!({"tool": "set_mixer"}));
    // As before when no level is given as Live shows it: nothing read.
    let context = mixer_context();
    let raw = json!({"trackRef": "1:track:0", "volume": 0.8});
    assert_eq!(mixer.prepare(raw.as_object().unwrap(), &context).await.unwrap().unwrap(), *raw.as_object().unwrap());
    assert!(context.calls.borrow().is_empty());
    // Each one asked of Live by its label, and set at the value Live shows it at.
    let context = mixer_context();
    let given = json!({"trackRef": "1:track:0", "volumeDb": 2, "panDisplay": "25l", "sendsDb": [-12, "-inf"], "mute": false});
    let prepared = mixer.prepare(given.as_object().unwrap(), &context).await.unwrap().unwrap();
    assert_eq!(
        Value::Object(prepared),
        json!({"trackRef": "1:track:0", "mute": false, "volume": 0.9112, "pan": -0.5, "sends": [0.5223, 0.0]})
    );
    assert_eq!(
        context.calls.borrow()[0],
        json!(["mixerForDisplay", "1:track:0", {"volume": "2.0 dB", "pan": "25L", "sends": ["-12.0 dB", "-inf dB"]}])
    );
    // A level Live shows nowhere is refused, with the nearest it shows; nothing is set.
    let context = mixer_context();
    let between = json!({"trackRef": "1:track:0", "volumeDb": 2.5555});
    let refused = mixer.prepare(between.as_object().unwrap(), &context).await.unwrap().unwrap_err();
    assert!(refused.contains("2.5555 dB") && refused.contains("2.555 dB") && refused.contains("2.556 dB"), "{refused}");
    // Both forms of one control at once, or a level that isn't one, is refused before Live is asked.
    let context = mixer_context();
    for wrong in [
        json!({"trackRef": "1:track:0", "volume": 0.8, "volumeDb": -6}),
        json!({"trackRef": "1:track:0", "volumeDb": "loud"}),
        json!({"trackRef": "1:track:0", "sendsDb": [-6, "half"]}),
    ] {
        assert!(mixer.prepare(wrong.as_object().unwrap(), &context).await.unwrap().is_err(), "{wrong}");
    }
    assert!(context.calls.borrow().is_empty());
}

#[test]
fn set_mixers_schema_offers_levels_as_live_shows_them_beside_its_own() {
    let mixer = kind(&json!({"tool": "set_mixer"}));
    let bridge = json!({"type": "object", "additionalProperties": false, "required": ["trackRef"], "properties": {
        "trackRef": {"type": "string"}, "volume": {"type": "number", "minimum": 0, "maximum": 1}
    }});
    let schema = Value::Object(mixer.schema(bridge.as_object().unwrap()));
    for field in ["trackRef", "volume", "volumeDb", "panDisplay", "sendsDb"] {
        assert!(schema["properties"].get(field).is_some(), "{field}: {schema}");
    }
    assert_eq!(schema["required"], json!(["trackRef"]));
}

/// The mixer script, run as Live runs it against a stand-in whose labels are as uneven as Live's: a level is found by
/// what it is ("-40 dB" is -40.0 dB), set inside the values Live shows it at, and one Live shows nowhere (or past the
/// fader's ends) gets the labels either side, never a value.
#[test]
fn the_mixer_script_finds_a_level_by_lives_own_labels() {
    use std::io::Write;
    let stand_in = r#"
import json, math
NBSP = u'\xa0'
class P:
    def __init__(self, lo, hi, fmt): self.min, self.max, self.fmt = lo, hi, fmt
    def str_for_value(self, v): return self.fmt(v)
def level(v):
    db = 6 + 85 * math.log10(v) if v > 0 else float('-inf')
    if db < -70: return '-inf' + NBSP + 'dB'
    if abs(db) >= 30: return '%d%sdB' % (round(db), NBSP)
    text = ('%.3f' % db).rstrip('0')
    text = text + '0' if text.endswith('.') else text
    return ('0.0' if text == '-0.0' else text) + NBSP + 'dB'
def pan(v):
    p = int(round(v * 50))
    return 'C' if p == 0 else ('%dL' % -p if p < 0 else '%dR' % p)
class Mixer:
    def __init__(self): self.volume, self.panning, self.sends = P(0.0, 1.0, level), P(-1.0, 1.0, pan), [P(0.0, 1.0, level)]
class Track:
    mixer_device = Mixer()
obj = Track()
"#;
    let cases = json!([
        {"volume": "-40.0 dB", "pan": "25L", "sends": ["-inf dB", "-12.0 dB"]},
        {"volume": "2.5555 dB"},
        {"volume": "7.0 dB"},
        {"volume": "2.0 dB", "pan": "C"},
    ]);
    let source = format!(
        "{stand_in}\nCASES = json.loads({})\nSCRIPT = {}\nout = []\nfor ARGS in CASES:\n    exec(SCRIPT)\n    out.append(result)\nprint(json.dumps(out))\n",
        stringify(&Value::String(stringify(&cases))),
        stringify(&Value::String(kumi_daw_ableton::display::MIXER_DISPLAY_SCRIPT.into()))
    );
    let mut child = std::process::Command::new("python3")
        .arg("-")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(source.as_bytes()).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let out: Value = serde_json::from_slice(&output.stdout).unwrap();
    let db = |label: &Value| -> f64 {
        let text = label.as_str().unwrap().replace('\u{a0}', " ");
        let number = text.trim_end_matches("dB").trim();
        if number == "-inf" {
            f64::NEG_INFINITY
        } else {
            number.parse().unwrap()
        }
    };
    // By level, whatever Live's spelling: asked "-40.0 dB", found where Live shows "-40 dB".
    assert_eq!(db(&out[0]["volume"]["shown"]), -40.0, "{out}");
    assert_eq!(out[0]["pan"]["shown"], "25L");
    assert!((out[0]["pan"]["value"].as_f64().unwrap() + 0.5).abs() < 0.01, "inside the values shown 25L: {out}");
    assert_eq!(out[0]["sends"][0]["value"], 0.0, "-inf is the fader's end");
    assert_eq!(out[0]["sends"][1], json!({"missing": true}), "a send the track hasn't");
    // A level Live shows nowhere: the labels either side, and no value.
    let near = out[1]["volume"]["near"].as_array().unwrap();
    assert!(out[1]["volume"].get("value").is_none() && db(&near[0]) < 2.5555 && 2.5555 < db(&near[1]), "{out}");
    // Past the fader's top: its two ends.
    let ends = out[2]["volume"]["near"].as_array().unwrap();
    assert_eq!((db(&ends[0]), db(&ends[1])), (f64::NEG_INFINITY, 6.0), "{out}");
    assert_eq!((db(&out[3]["volume"]["shown"]), out[3]["pan"]["shown"].clone()), (2.0, json!("C")), "{out}");
}
