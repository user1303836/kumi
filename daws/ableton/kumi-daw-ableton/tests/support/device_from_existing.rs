use serde_json::{json, Value};

/// What make_device's description says since it makes a new device from one that's already there (#353).
const SINCE: &str = " To change a device that's already there, Kumi's or anyone's, give from: alone, Kumi answers with the device in its \
                     notation, each box by name; with patch, controls or remove saying only what changes, it makes a new device \
                     from it and never writes the original's file.";

/// Keep replaying the frozen catalog for make_device as it was before it opened devices that are already there (#353):
/// its description without the sentence on from, and its schema without from and remove. Both new fields are checked as
/// they are here first, so they're set aside, not ignored; the first-request goldens pin the whole of it as it is now.
pub fn legacy(description: &str, schema: &Value) -> (String, Value) {
    let description = description.strip_suffix(SINCE).unwrap_or_else(|| panic!("make_device's sentence on from: {description}"));
    let mut schema = schema.clone();
    let properties = schema["properties"].as_object_mut().unwrap();
    assert_eq!(
        properties.shift_remove("from"),
        Some(
            json!({ "type": "string", "description": "A device that's already there to start from: its file, its item in Live's Browser (user_library/…) or its name in the User Library" })
        )
    );
    assert_eq!(
        properties.shift_remove("remove"),
        Some(
            json!({ "type": "array", "items": { "type": "string" }, "description": "With from: its boxes or controls by name, or its cords (\"a.1 -> b\"), to take out" })
        )
    );
    (description.to_string(), schema)
}
