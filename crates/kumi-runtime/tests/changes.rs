use async_trait::async_trait;
use kumi_common::js::json::stringify;
use kumi_runtime::{integrations::ableton::changes::*, RuntimeError};
use serde_json::{json, Value};
use std::{cell::RefCell, collections::HashSet};
fn oracle() -> Value {
    serde_json::from_str(include_str!("support/changes-oracle.json")).unwrap()
}
struct Context {
    calls: RefCell<Vec<Value>>,
    parameters: Vec<ParameterRange>,
    display: bool,
}
#[async_trait(?Send)]
impl ChangeContext for Context {
    async fn sample(&self, path: &str) -> Option<SampleFile> {
        self.calls.borrow_mut().push(json!(["sample", path]));
        path.starts_with("/samples/").then(|| SampleFile { path: path.into(), folder: "/samples".into() })
    }
    async fn parameters(&self, device: &str) -> Result<Vec<ParameterRange>, RuntimeError> {
        self.calls.borrow_mut().push(json!(["parameters", device]));
        Ok(self.parameters.clone())
    }
    async fn ranges(&self, device: &str) -> Result<Vec<ParameterRange>, RuntimeError> {
        self.calls.borrow_mut().push(json!(["ranges", device]));
        Ok(self.parameters.clone())
    }
    async fn pick(&self, selector: SampleSelector) -> Result<Option<SampleFile>, RuntimeError> {
        self.calls.borrow_mut().push(json!(["pick", selector]));
        Ok((!selector.words.iter().any(|w| w == "missing"))
            .then(|| SampleFile { path: "/samples/Picked.wav".into(), folder: "/samples".into() }))
    }
    fn has_value_for(&self) -> bool {
        self.display
    }
    async fn value_for(&self, reference: &str, text: &str) -> Result<Result<f64, String>, RuntimeError> {
        self.calls.borrow_mut().push(json!(["valueFor", reference, text]));
        Ok(if text == "-6 dB" { Ok(0.625) } else { Err("unplaceable".into()) })
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
    assert_eq!(serde_json::to_value(&*CHANGES).unwrap(), data["kinds"]);
    assert_eq!(*SAMPLE_INPUT, data["sampleInput"].as_object().unwrap().clone());
    assert_eq!(*REFERENCE_FIELDS, serde_json::from_value::<Vec<String>>(data["referenceFields"].clone()).unwrap());
    assert_eq!(*HOST_TOOLS, serde_json::from_value::<HashSet<String>>(data["hostTools"].clone()).unwrap());
    assert_eq!(*UNDO_DESCRIPTION, data["undoDescription"]);
}

#[test]
fn all_change_summaries_match_complete_source_outputs() {
    let data: Value = serde_json::from_str(include_str!("support/change-summaries-oracle.json")).unwrap();
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
        kumi_runtime::integrations::ableton::more_changes::set_meter(meter[0].as_f64().unwrap(), meter[1].as_f64().unwrap());
        let result = kind(&case).summarize(
            case["preview"].as_object().unwrap(),
            case["input"].as_object().unwrap(),
            &track,
            case.get("applied").and_then(Value::as_object),
        );
        let actual: Value = serde_json::from_str(&stringify(&serde_json::to_value(result).unwrap())).unwrap();
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
    kumi_runtime::integrations::ableton::more_changes::set_meter(4.0, 4.0);
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
    kumi_runtime::integrations::ableton::more_changes::set_meter(4.0, 4.0);
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
