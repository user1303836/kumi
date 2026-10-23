//! The M2 contract's naming rule for refs, as a check any bridge's answers can be run through.
//!
//! Kumi shortens a ref by the key it sits under: a field that holds one ref is `ref` or `parent`, or its name ends in
//! `Ref`; a field that holds a list of refs ends in `Refs`. A ref under any other key reaches the model long, with its
//! GUID, and breaks the turn's short refs. [`misplaced_refs`] finds those.

use serde_json::Value;

/// The kinds a ref names, for every DAW (`r:track:{GUID}`, `E:pattern:3`, Logic's `1:region:2`).
const REF_KINDS: [&str; 20] = [
    "aitem",
    "track",
    "item",
    "take",
    "fx",
    "param",
    "send",
    "marker",
    "region",
    "note",
    "plugin",
    "env",
    "pattern",
    "seq",
    "sequence",
    "instrument",
    "sample",
    "phrase",
    "device",
    "line",
];

/// Whether text is shaped like a ref: `<prefix>:<kind>:<rest>`, one word with no spaces.
pub fn looks_like_ref(text: &str) -> bool {
    if text.chars().any(char::is_whitespace) {
        return false;
    }
    let mut parts = text.splitn(3, ':');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(prefix), Some(kind), Some(rest)) => {
            !prefix.is_empty()
                && prefix.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
                && REF_KINDS.contains(&kind)
                && !rest.is_empty()
        }
        _ => false,
    }
}

/// Whether a key may hold one ref.
pub fn key_holds_a_ref(key: &str) -> bool {
    key == "ref" || key == "parent" || key.ends_with("Ref")
}

/// Whether a key may hold a list of refs.
pub fn key_holds_refs(key: &str) -> bool {
    key.ends_with("Refs")
}

/// The paths (`items[2].track`) of the ref-shaped strings in an answer that sit under a key that may not hold one.
pub fn misplaced_refs(value: &Value) -> Vec<String> {
    let mut found = Vec::new();
    walk(value, "", None, &mut found);
    found
}

fn walk(value: &Value, path: &str, key: Option<&str>, found: &mut Vec<String>) {
    match value {
        Value::String(text) if looks_like_ref(text) => {
            if !key.is_some_and(key_holds_a_ref) {
                found.push(format!("{path} = {text}"));
            }
        }
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                let here = format!("{path}[{i}]");
                match item {
                    Value::String(text) if looks_like_ref(text) => {
                        if !key.is_some_and(key_holds_refs) {
                            found.push(format!("{here} = {text}"));
                        }
                    }
                    other => walk(other, &here, None, found),
                }
            }
        }
        Value::Object(fields) => {
            for (name, field) in fields {
                let here = if path.is_empty() { name.clone() } else { format!("{path}.{name}") };
                walk(field, &here, Some(name), found);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn refs_are_told_from_names_and_other_text() {
        for ref_ in ["r:track:{0A1B}", "r:track:master", "r:param:{AB}:3", "e7:pattern:3", "1a15-2:device:2.1.4", "r:env:{AB}:VOLENV2"] {
            assert!(looks_like_ref(ref_), "{ref_}");
        }
        // Logic's: positional, with an epoch, for its tracks, regions, notes, plug-ins, sends and markers.
        for ref_ in ["1:track:2", "1:region:1", "3:note:1.4", "1:plugin:1.1", "1:send:2.1", "2:marker:1"] {
            assert!(looks_like_ref(ref_), "{ref_}");
        }
        assert!(!looks_like_ref("1:tempo:1") && !looks_like_ref("1:project:1"));
        for text in ["Drums", "bass: track: loud", "track:3", "r:nothing:{AB}", "r:track:", "c1.sim-1.2", "36591:2", ""] {
            assert!(!looks_like_ref(text), "{text}");
        }
    }

    #[test]
    fn a_ref_under_ref_parent_or_a_name_ending_in_ref_is_in_place() {
        let answer = json!({
            "ref": "r:track:{A}",
            "parent": "r:track:{B}",
            "items": [{"ref": "r:item:{C}", "takeRef": "r:take:{D}", "trackRefs": ["r:track:{A}", "r:track:{B}"], "childRefs": []}],
            "selection": {"trackRef": "r:track:{A}", "param": {"fxRef": "r:fx:{E}", "name": "Gain"}},
            "name": "bass: track: loud",
        });
        assert_eq!(misplaced_refs(&answer), Vec::<String>::new());
    }

    #[test]
    fn a_ref_anywhere_else_is_found_with_its_path() {
        let answer = json!({
            "items": [{"ref": "r:send:{A}:0:{B}", "dest": "r:track:{B}", "children": ["r:fx:{C}"]}],
            "selection": {"track": "r:track:{A}", "takeRefs": "r:take:{D}", "itemRef": ["r:item:{E}"]},
        });
        let mut found = misplaced_refs(&answer);
        found.sort();
        assert_eq!(
            found,
            [
                "items[0].children[0] = r:fx:{C}",
                "items[0].dest = r:track:{B}",
                "selection.itemRef[0] = r:item:{E}",
                "selection.takeRefs = r:take:{D}",
                "selection.track = r:track:{A}",
            ]
        );
    }
}
