use serde_json::{json, Value};

/// Keep replaying the frozen catalog for the original guide/wavetable contract.
/// The added file action and complete current schema are pinned by first-request
/// goldens; this projection removes only that explicit extension from the replay.
pub fn legacy(description: &str, schema: &Value) -> (String, Value) {
    assert!(description.contains("vst3 catalog/create/edit"));
    let original = description.split_once("Work with a plug-in").unwrap().1.split_once(" prepare-preset:").unwrap().0;
    let description = format!("Work with a plug-in{original}");
    let mut schema = schema.clone();
    assert_eq!(schema["required"], json!(["action"]));
    schema["required"] = json!(["device", "action"]);
    let properties = schema["properties"].as_object_mut().unwrap();
    assert!(properties.remove("file").is_some());
    assert!(properties.remove("name").is_some());
    let actions = properties["action"]["enum"].as_array().unwrap();
    assert_eq!(actions, &vec![json!("guide"), json!("wavetable"), json!("prepare-preset")]);
    properties.get_mut("action").unwrap()["enum"] = json!(["guide", "wavetable"]);
    (description, schema)
}
