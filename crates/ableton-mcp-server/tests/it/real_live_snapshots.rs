//! Snapshots as the real Remote Script answers them, recorded in Live 12.4: Session and Arrangement
//! clips (MIDI and audio), a take lane, racks with chains, a Drum Rack, a clip envelope and Max for
//! Live devices. The simulator's rows carry fields the Remote Script never sends, so only frames like
//! these show what the adapter must accept.
use ableton_mcp_server::{
    bridge::remote_adapter::expand_pad_chains,
    live::{check_snapshot_answer, LiveSnapshot, LiveSnapshotRequest},
};
use serde_json::{json, Value};

/// Numbers as f64: Live's integers and the typed snapshot's floats are the same JSON numbers.
fn numbers_as_floats(value: &Value) -> Value {
    match value {
        Value::Number(number) => json!(number.as_f64().unwrap()),
        Value::Array(items) => Value::Array(items.iter().map(numbers_as_floats).collect()),
        Value::Object(map) => Value::Object(map.iter().map(|(key, item)| (key.clone(), numbers_as_floats(item))).collect()),
        other => other.clone(),
    }
}

/// The first path where two JSON values differ, with what each side has there.
fn difference(expected: &Value, actual: &Value, path: &str) -> Option<String> {
    match (expected, actual) {
        (Value::Object(left), Value::Object(right)) => left
            .iter()
            .find_map(|(key, item)| match right.get(key) {
                Some(other) => difference(item, other, &format!("{path}.{key}")),
                None => Some(format!("{path}.{key}: dropped (was {item})")),
            })
            .or_else(|| right.keys().find(|key| !left.contains_key(*key)).map(|key| format!("{path}.{key}: added ({})", right[key]))),
        (Value::Array(left), Value::Array(right)) if left.len() == right.len() => {
            left.iter().zip(right).enumerate().find_map(|(index, (item, other))| difference(item, other, &format!("{path}[{index}]")))
        }
        _ => (expected != actual).then(|| format!("{path}: {expected} became {actual}")),
    }
}

#[test]
fn real_remote_script_snapshots_parse_and_keep_exactly_their_fields() {
    let fixture: Value =
        serde_json::from_reader(flate2::read::GzDecoder::new(&include_bytes!("fixtures/real-live-snapshots.json.gz")[..])).unwrap();
    let frames = fixture["frames"].as_array().unwrap();
    assert_eq!(frames.len(), 8);
    for frame in frames {
        let args = &frame["args"];
        let request: LiveSnapshotRequest = serde_json::from_value(args.clone()).unwrap();
        // As the remote adapter reads an answer.
        let mut result = frame["result"].clone();
        expand_pad_chains(&mut result);
        let snapshot: LiveSnapshot = serde_json::from_value(result.clone()).unwrap_or_else(|error| panic!("snapshot {args}: {error}"));
        let snapshot = check_snapshot_answer(snapshot, &request).unwrap_or_else(|error| panic!("snapshot {args}: {error}"));
        // What the host passes on is what Live sent: no field dropped, none made up.
        let written = serde_json::to_value(&snapshot).unwrap();
        if let Some(found) = difference(&numbers_as_floats(&result), &numbers_as_floats(&written), "snapshot") {
            panic!("snapshot {args}: {found}");
        }
    }
}

#[test]
fn discovery_results_move_into_the_value_serde_would_make() {
    use ableton_mcp_server::live::{LiveDiscoveryKind, LiveDiscoveryResult};
    let fixture: Value =
        serde_json::from_reader(flate2::read::GzDecoder::new(&include_bytes!("fixtures/real-live-snapshots.json.gz")[..])).unwrap();
    let tracks: Vec<_> =
        fixture["frames"][0]["result"]["tracks"].as_array().unwrap().iter().map(|t| t.as_object().unwrap().clone()).collect();
    for (kind, cursor) in [(LiveDiscoveryKind::Track, None), (LiveDiscoveryKind::SessionClip, Some("next-page".to_string()))] {
        let result = LiveDiscoveryResult {
            epoch: 1218806785596302,
            items: tracks.clone(),
            truncated: cursor.is_some(),
            revision: "r1".into(),
            kind,
            next_cursor: cursor,
        };
        let expected = serde_json::to_value(&result).unwrap();
        let moved = result.into_value();
        assert_eq!(moved, expected);
        // Key order too: the text a client reads is the same.
        assert_eq!(kumi_common::js::json::stringify(&moved), kumi_common::js::json::stringify(&expected));
    }
}
