use ableton_mcp_server::live::{Clip, DeterministicLiveSimulator, Maybe};
use kumi_common::js::json::stringify;
use serde_json::{json, Value};

#[test]
fn observed_notes_preserve_wire_order_while_typed_changes_remain_visible() {
    let sim = DeterministicLiveSimulator::new();
    let mut row = sim.state.borrow()["tracks"][0]["clips"][0].clone();
    row["notes"] = json!([{"id":1,"extraFirst":"retained","channel":1,"pitch":36,"start":0,"velocity":90,"duration":0.25,"mute":null}]);
    let mut clip: Clip = serde_json::from_value(row.clone()).unwrap();
    assert_eq!(stringify(&serde_json::to_value(&clip).unwrap()["notes"]), stringify(&row["notes"]));
    clip.notes[0].pitch = 48.0;
    clip.notes[0].id = Maybe::Absent;
    clip.notes[0].extra.insert("added".into(), json!(true));
    row["notes"][0]["pitch"] = json!(48);
    row["notes"][0].as_object_mut().unwrap().shift_remove("id");
    row["notes"][0]["added"] = json!(true);
    let result: Value = serde_json::to_value(&clip).unwrap();
    assert_eq!(stringify(&result["notes"]), stringify(&row["notes"]));
}
