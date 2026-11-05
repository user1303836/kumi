use super::schema;
use crate::serve::{change, listen};
use kumi_agent::kernel::schema::problem;
use serde_json::{json, Value};

fn source_input(source_schema: Value) -> Value {
    json!({"type": "object", "required": ["source"], "properties": {"source": source_schema}})
}

fn rejects(schema: &Value, inputs: impl IntoIterator<Item = Value>) {
    for input in inputs {
        assert!(problem(schema, &input).is_some(), "accepted {input}");
    }
}

#[test]
fn change_source_accepts_each_real_form_and_refuses_mixed_or_incomplete_forms() {
    let schema = source_input(change::source_schema());
    for source in [
        json!({"sound_id": "s1", "map_revision": "r1"}),
        json!({"artifact_id": "a1"}),
        json!({"file": "melody.wav", "provenance": {"host_tool": "file_picker"}}),
        json!({"file": "melody.wav", "provenance": {"host_tool": "file_picker", "tool_call_id": "call1"}}),
    ] {
        let input = json!({"source": source});
        assert!(problem(&schema, &input).is_none(), "{input}");
    }
    rejects(
        &schema,
        [
            json!({"source": "melody.wav"}),
            json!({"source": {}}),
            json!({"source": {"sound_id": "s1"}}),
            json!({"source": {"map_revision": "r1"}}),
            json!({"source": {"file": "melody.wav"}}),
            json!({"source": {"provenance": {"host_tool": "file_picker"}}}),
            json!({"source": {"sound_id": "s1", "map_revision": "r1", "artifact_id": "a1"}}),
            json!({"source": {"sound_id": "s1", "map_revision": "r1", "file": "melody.wav", "provenance": {"host_tool": "file_picker"}}}),
            json!({"source": {"artifact_id": "a1", "file": "melody.wav", "provenance": {"host_tool": "file_picker"}}}),
            json!({"source": {"artifact_id": "a1", "path": "/private/source.wav"}}),
        ],
    );
}

#[test]
fn change_source_checks_inner_types_provenance_and_actual_limits() {
    let schema = source_input(change::source_schema());
    rejects(
        &schema,
        [
            json!({"source": {"sound_id": 1, "map_revision": "r1"}}),
            json!({"source": {"sound_id": "s1", "map_revision": false}}),
            json!({"source": {"artifact_id": 1}}),
            json!({"source": {"file": 1, "provenance": {"host_tool": "picker"}}}),
            json!({"source": {"file": "x.wav", "provenance": "picker"}}),
            json!({"source": {"file": "x.wav", "provenance": {}}}),
            json!({"source": {"file": "x.wav", "provenance": {"host_tool": false}}}),
            json!({"source": {"file": "x.wav", "provenance": {"host_tool": "picker", "tool_call_id": 7}}}),
            json!({"source": {"file": "x.wav", "provenance": {"host_tool": "picker", "path": "/private/source.wav"}}}),
        ],
    );
    for (mut source, key, max) in [
        (json!({"sound_id": "s1", "map_revision": "r1"}), "sound_id", 128),
        (json!({"sound_id": "s1", "map_revision": "r1"}), "map_revision", 64),
        (json!({"artifact_id": "a1"}), "artifact_id", 128),
        (json!({"file": "x.wav", "provenance": {"host_tool": "picker"}}), "file", 4096),
    ] {
        source[key] = json!("x".repeat(max));
        assert!(problem(&schema, &json!({"source": source.clone()})).is_none());
        for bad in [String::new(), "x".repeat(max + 1)] {
            source[key] = json!(bad);
            assert!(problem(&schema, &json!({"source": source.clone()})).is_some());
        }
    }
    for (key, max, empty_allowed) in [("host_tool", 128, false), ("tool_call_id", 256, true)] {
        let mut source = json!({"file": "x.wav", "provenance": {"host_tool": "picker"}});
        source["provenance"][key] = json!("x".repeat(max));
        assert!(problem(&schema, &json!({"source": source.clone()})).is_none());
        source["provenance"][key] = json!("x".repeat(max + 1));
        assert!(problem(&schema, &json!({"source": source.clone()})).is_some());
        source["provenance"][key] = json!("");
        assert_eq!(problem(&schema, &json!({"source": source})).is_none(), empty_allowed);
    }
}

#[test]
fn listen_source_checks_each_real_form_and_its_own_fields_before_alternatives() {
    let schema = source_input(listen::source_schema());
    for source in [json!({"file": "x.wav"}), json!({"sound_id": "s1", "map_revision": "r1"}), json!({"artifact_id": "a1"})] {
        assert!(problem(&schema, &json!({"source": source})).is_none());
    }
    rejects(
        &schema,
        [
            json!({"source": "x.wav"}),
            json!({"source": null}),
            json!({"source": {}}),
            json!({"source": {"sound_id": "s1"}}),
            json!({"source": {"map_revision": "r1"}}),
            json!({"source": {"file": "x.wav", "artifact_id": "a1"}}),
            json!({"source": {"file": "x.wav", "sound_id": "s1", "map_revision": "r1"}}),
            json!({"source": {"artifact_id": "a1", "sound_id": "s1", "map_revision": "r1"}}),
            json!({"source": {"file": "x.wav", "map_revision": "r1"}}),
            json!({"source": {"artifact_id": "a1", "map_revision": "r1"}}),
            json!({"source": {"file": "x.wav", "provenance": {"host_tool": "picker"}}}),
        ],
    );
    for (mut source, key) in [
        (json!({"file": "x.wav"}), "file"),
        (json!({"sound_id": "s1", "map_revision": "r1"}), "sound_id"),
        (json!({"sound_id": "s1", "map_revision": "r1"}), "map_revision"),
        (json!({"artifact_id": "a1"}), "artifact_id"),
    ] {
        for invalid in [json!(7), json!(false), json!(null), json!({}), json!("")] {
            source[key] = invalid;
            let why = problem(&schema, &json!({"source": source.clone()})).unwrap();
            assert!(why.contains(&format!("source.{key}")), "{why}");
        }
    }
}

#[test]
fn source_hints_follow_supplied_fields_instead_of_the_first_object_form() {
    let change = source_input(change::source_schema());
    for (source, field) in [
        (json!({"file": "x.wav"}), "source.provenance"),
        (json!({"file": "x.wav", "provenance": {"host_tool": 7}}), "source.provenance.host_tool"),
        (json!({"artifact_id": 7}), "source.artifact_id"),
        (json!({"map_revision": "r1"}), "source.sound_id"),
        (json!({}), "source.artifact_id"),
    ] {
        let why = problem(&change, &json!({"source": source})).unwrap();
        assert!(why.contains(field), "{why}");
    }
    let listen = source_input(listen::source_schema());
    let why = problem(&listen, &json!({"source": {"sound_id": "s1"}})).unwrap();
    assert!(why.contains("source.map_revision"), "{why}");
}

#[test]
fn generate_schema_accepts_all_actions_and_rejects_mixed_or_incomplete_calls() {
    let schema = schema();
    let quote_id = format!("q-{}", "a".repeat(32));
    for input in [
        json!({"action": "quote", "kind": "sfx", "prompt": "glass hit"}),
        json!({"action": "quote", "kind": "sfx", "prompt": "glass hit", "seconds": 0.1}),
        json!({"action": "quote", "kind": "audio_to_midi", "source": {"artifact_id": "a1"}}),
        json!({"action": "quote", "kind": "audio_to_midi", "source": {"sound_id": "s1", "map_revision": "r1"}, "preview": true}),
        json!({"action": "quote", "kind": "audio_to_midi", "source": {"file": "x.wav", "provenance": {"host_tool": "picker"}}, "preview": false}),
        json!({"action": "create", "quote_id": quote_id}),
        json!({"action": "job", "job_id": "j1-1"}),
    ] {
        assert!(problem(&schema, &input).is_none(), "{input}");
    }
    rejects(
        &schema,
        [
            json!({}),
            json!({"action": "quote"}),
            json!({"action": "quote", "kind": "sfx"}),
            json!({"action": "quote", "kind": "audio_to_midi"}),
            json!({"action": "create"}),
            json!({"action": "job"}),
            json!({"action": "quote", "kind": "sfx", "prompt": "hit", "source": {"artifact_id": "a1"}}),
            json!({"action": "quote", "kind": "sfx", "prompt": "hit", "preview": true}),
            json!({"action": "quote", "kind": "audio_to_midi", "source": {"artifact_id": "a1"}, "prompt": "hit"}),
            json!({"action": "quote", "kind": "audio_to_midi", "source": {"artifact_id": "a1"}, "seconds": 5}),
            // Only `not` excludes kind from the otherwise-valid create form.
            json!({"action": "create", "quote_id": quote_id, "kind": "sfx"}),
            json!({"action": "create", "quote_id": quote_id, "job_id": "j1-1"}),
            json!({"action": "job", "job_id": "j1-1", "quote_id": quote_id}),
            json!({"action": "quote", "kind": "sfx", "prompt": "", "seconds": 5}),
            json!({"action": "quote", "kind": "sfx", "prompt": "x".repeat(501)}),
            json!({"action": "quote", "kind": "sfx", "prompt": "hit", "seconds": 0}),
            json!({"action": "quote", "kind": "sfx", "prompt": "hit", "seconds": "5"}),
            json!({"action": "quote", "kind": "audio_to_midi", "source": {"file": "x.wav", "provenance": {"host_tool": 7}}}),
            json!({"action": "quote", "kind": "audio_to_midi", "source": {"artifact_id": "a1"}, "preview": "true"}),
            json!({"action": "create", "quote_id": "q-invalid"}),
            json!({"action": "job", "job_id": "j-invalid"}),
        ],
    );
}

#[test]
fn generate_hints_choose_matching_discriminators_and_do_not_echo_private_metadata() {
    let schema = schema();
    for (input, field, unrelated) in [
        (json!({"action": "create"}), "quote_id", "prompt"),
        (json!({"action": "job"}), "job_id", "prompt"),
        (json!({"action": "quote", "kind": "audio_to_midi"}), "source", "prompt"),
        (json!({"action": "quote", "kind": "sfx"}), "prompt", "source"),
        (json!({"action": "create", "quote_id": format!("q-{}", "a".repeat(32)), "kind": "sfx"}), "kind", "prompt"),
    ] {
        let why = problem(&schema, &input).unwrap();
        assert!(why.contains(field), "{why}");
        assert!(!why.contains(unrelated), "{why}");
    }
    let secret = "private-api-key-must-not-appear";
    let input = json!({"action": "quote", "kind": "sfx", "prompt": "hit", "_mirelo": {"api_key": secret}});
    let why = problem(&schema, &input).unwrap();
    assert!(!why.contains(secret));
    assert!(!why.contains("api_key"));
}

#[test]
fn edit_schema_accepts_task_specific_windows_and_refuses_every_wrong_kind_field() {
    let schema = schema();
    let source = json!({"file":"source.wav", "provenance":{"host_tool":"picker"}});
    let valid = [
        json!({"action":"quote", "kind":"video_to_sfx", "source":source}),
        json!({"action":"quote", "kind":"video_to_sfx", "source":{"artifact_id":"a1"}, "seconds":1.2346, "prompt":"wind"}),
        json!({"action":"quote", "kind":"extend", "source":source, "append_seconds":1.2346}),
        json!({"action":"quote", "kind":"inpaint", "source":source, "region_start_seconds":0, "region_end_seconds":1.2346}),
    ];
    for input in &valid {
        assert!(problem(&schema, input).is_none(), "{input}");
    }
    for kind in ["video_to_sfx", "extend", "inpaint"] {
        let preview = json!({"action":"quote", "kind":kind, "source":source, "preview":true});
        assert!(problem(&schema, &preview).is_none(), "{preview}");
    }
    let fields =
        [("seconds", json!(1)), ("append_seconds", json!(1)), ("region_start_seconds", json!(0)), ("region_end_seconds", json!(1))];
    for (input, allowed) in [
        (&valid[0], &["seconds"][..]),
        (&valid[2], &["append_seconds"][..]),
        (&valid[3], &["region_start_seconds", "region_end_seconds"][..]),
    ] {
        for (field, value) in &fields {
            if !allowed.contains(field) {
                let mut wrong = input.clone();
                wrong[*field] = value.clone();
                assert!(problem(&schema, &wrong).is_some(), "accepted wrong-kind field: {wrong}");
            }
        }
    }
    for mut input in [
        json!({"action":"create", "quote_id":format!("q-{}", "a".repeat(32))}),
        json!({"action":"job", "job_id":"j1-1"}),
        json!({"action":"quote", "kind":"sfx", "prompt":"hit"}),
        json!({"action":"quote", "kind":"audio_to_midi", "source":source}),
    ] {
        for (field, value) in &fields[1..] {
            input[*field] = value.clone();
            assert!(problem(&schema, &input).is_some(), "accepted wrong-action field: {input}");
            input.as_object_mut().unwrap().remove(*field);
        }
    }
    rejects(
        &schema,
        [
            json!({"action":"quote", "kind":"extend", "source":source}),
            json!({"action":"quote", "kind":"extend", "source":source, "preview":false}),
            json!({"action":"quote", "kind":"inpaint", "source":source, "region_start_seconds":0}),
            json!({"action":"quote", "kind":"inpaint", "source":source, "region_end_seconds":1}),
            json!({"action":"quote", "kind":"video_to_sfx", "source":{"sound_id":"s1", "map_revision":"r1"}}),
        ],
    );
    for (input, field) in [(&valid[1], "seconds"), (&valid[2], "append_seconds"), (&valid[3], "region_end_seconds")] {
        for value in [json!(0), json!(-1), json!("1"), Value::Null] {
            let mut wrong = input.clone();
            wrong[field] = value;
            assert!(problem(&schema, &wrong).is_some(), "{wrong}");
        }
    }
}
