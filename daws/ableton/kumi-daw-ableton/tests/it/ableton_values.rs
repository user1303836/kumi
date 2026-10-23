use kumi_common::js::json::stringify;
use kumi_runtime::{
    integrations::ableton::{context, display::*, fast::*},
    mcp::types::CallToolResult,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    io::Write,
    process::{Command, Stdio},
};
fn oracle() -> Value {
    serde_json::from_str(&kumi_testkit::oracle("kumi-runtime/ableton-values-oracle.json")).unwrap()
}
fn hash(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}
fn number(value: f64) -> Value {
    if value.is_nan() {
        json!("NaN")
    } else if value == f64::INFINITY {
        json!("Infinity")
    } else if value == f64::NEG_INFINITY {
        json!("-Infinity")
    } else {
        json!(value)
    }
}
fn close(actual: &Value, expected: &Value) {
    if let (Some(a), Some(b)) = (actual.as_f64(), expected.as_f64()) {
        assert!((a - b).abs() <= 1e-12 * a.abs().max(b.abs()).max(1.0), "{a} != {b}");
    } else if let (Some(a), Some(b)) = (actual.as_object(), expected.as_object()) {
        assert_eq!(a.len(), b.len());
        for (k, v) in a {
            close(v, &b[k]);
        }
    } else if let (Some(a), Some(b)) = (actual.as_array(), expected.as_array()) {
        assert_eq!(a.len(), b.len());
        for (a, b) in a.iter().zip(b) {
            close(a, b);
        }
    } else {
        assert_eq!(actual, expected);
    }
}
#[test]
fn parameter_text_and_interpolation_match_reference_for_units_steps_and_boundaries() {
    let data = oracle();
    for case in data["parsed"].as_array().unwrap() {
        let result =
            parse_display(case["text"].as_str().unwrap()).map(|r| json!({"value":number(r.value),"unit":r.unit})).unwrap_or(Value::Null);
        close(&result, &case["value"]);
    }
    for case in data["values"].as_array().unwrap() {
        let map: DisplayMap = serde_json::from_value(data["maps"][case["map"].as_u64().unwrap() as usize].clone()).unwrap();
        let target: DisplayTarget = serde_json::from_value(case["target"].clone()).unwrap();
        let result = match value_for_display(&map, target) {
            Ok(n) => number(n),
            Err(why) => json!(why),
        };
        close(&result, &case["value"]);
    }
}
#[test]
fn discovery_validates_results_identity_epochs_fields_and_page_keys() {
    for case in oracle()["contexts"].as_array().unwrap() {
        let input = &case["input"];
        let result = match case["fn"].as_str().unwrap() {
            "discoveryArgs" => context::discovery_args(input.as_object().unwrap()).map(Value::Object),
            "setIdentity" => context::set_identity(input.as_object().unwrap()).map(Value::String),
            "queryKey" => Ok(Value::String(context::query_key(input.as_object().unwrap()))),
            name => {
                let result: CallToolResult = serde_json::from_value(input.clone()).unwrap();
                match name {
                    "payload" => context::payload(&result),
                    "statusPayload" => context::status_payload(&result),
                    "discoveryPayload" => context::discovery_payload(&result, "track", 7.0),
                    _ => unreachable!(),
                }
                .map(Value::Object)
            }
        };
        let actual = match result {
            Ok(value) => json!({"value":value}),
            Err(error) => json!({"error":error.to_string()}),
        };
        let expected = if let Some(value) = case.get("value") { json!({"value":value}) } else { json!({"error":case["error"]}) };
        assert_eq!(stringify(&actual), stringify(&expected), "{case}");
    }
}
#[test]
fn host_scripts_and_instruction_assets_preserve_source_bytes() {
    let data = oracle();
    assert_eq!(hash(context::INSTRUCTIONS), data["assets"]["instructions"]);
    assert_eq!(hash(DISPLAY_MAP_SCRIPT), data["assets"]["display"]);
    assert_eq!(*context::FIELDS, data["assets"]["fields"].as_object().unwrap().clone());
    assert_eq!(*context::PARENTS, data["assets"]["parents"].as_object().unwrap().clone());
    for case in data["scripts"].as_array().unwrap() {
        let script = script(case);
        assert_eq!(hash(&script), case["hash"], "{case}");
    }
}
fn script(case: &Value) -> String {
    match case["kind"].as_str().unwrap() {
        "find" => find_script(&case["args"]),
        "set" => set_script(&case["args"]),
        "revert" => revert_script(&case["args"]),
        other => panic!("{other}"),
    }
}
#[test]
fn scripts_execute_in_python_with_atomic_rollback_deleted_devices_and_safe_undo() {
    let data = oracle();
    let codes: Vec<_> = data["scripts"].as_array().unwrap().iter().map(script).collect();
    let source = format!("CODES = {}\n{}", stringify(&Value::String(stringify(&json!(codes)))), include_str!("../support/fast-live.py"));
    let mut child = Command::new("python3").arg("-").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(source.as_bytes()).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    close(&serde_json::from_slice::<Value>(&output.stdout).unwrap(), &data["executed"]);
}
