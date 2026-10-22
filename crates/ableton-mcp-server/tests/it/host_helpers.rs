use ableton_mcp_server::{
    host::{helpers::*, retention::*},
    live::LiveError,
};
use serde_json::{json, Value};
use std::{cell::RefCell, rc::Rc};

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/host-helpers-oracle.json")).unwrap()
}
#[test]
fn host_validation_authority_and_result_helpers_match_source() {
    let data = fixture();
    for row in data["cases"].as_array().unwrap() {
        let args = row["args"].as_array().unwrap();
        let value = &args[0];
        let got: Result<Option<Value>, LiveError> = match row["fn"].as_str().unwrap() {
            "isNonEmptyString" => Ok(Some(json!(is_non_empty_string(value, 128)))),
            "isFiniteAtLeast" => Ok(Some(json!(is_finite_at_least(value, args[1].as_f64().unwrap())))),
            "isIntegerInRange" => Ok(Some(json!(is_integer_in_range(value, args[1].as_f64().unwrap(), args[2].as_f64().unwrap())))),
            "isIdempotencyKey" => Ok(Some(json!(is_idempotency_key(value)))),
            "isDiscoveryFilter" => Ok(Some(json!(is_discovery_filter(value)))),
            "outputSafetyOf" => Ok(Some(output_safety_of(value))),
            "canonicalMutationIdentity" => canonical_mutation_identity(value).map(|v| Some(json!(v))),
            "retainedBytes" => Ok(Some(json!(retained_bytes(value)))),
            "adapterReason" => Ok(Some(json!(adapter_reason(value.as_str().unwrap())))),
            "sameLiveValue" => Ok(Some(json!(same_live_value(Some(value), Some(&args[1]))))),
            "wholeNumberLiveKept" => Ok(whole_number_live_kept(value, args[1].as_f64().unwrap(), &args[2]).map(|v| json!(v))),
            "sceneRestoreFields" => Ok(scene_restore_fields(value.as_object().unwrap()).map(Value::Object)),
            "sceneFieldRestored" => Ok(Some(json!(scene_field_restored(value, args[1].as_str().unwrap(), &args[2], &args[3])))),
            "fitParameterValue" => Ok(Some(json!(fit_parameter_value(value.as_f64().unwrap(), &args[1])))),
            "isRetirableAppliedTransaction" => Ok(Some(json!(is_retirable_applied_transaction(value)))),
            name => panic!("unexpected helper {name}"),
        };
        if let Some(error) = row.get("error") {
            assert_eq!(got.unwrap_err().message(), error.as_str().unwrap(), "{row}");
        } else if row["undefined"] == true {
            assert_eq!(got.unwrap(), None, "{row}");
        } else {
            assert_eq!(
                got.unwrap().map(|v| kumi_common::js::json::stringify(&v)),
                Some(kumi_common::js::json::stringify(&row["result"])),
                "{row}"
            );
        }
    }
}
#[test]
fn shared_retention_matches_source_for_refusals_eviction_expiry_and_recovery() {
    let data = fixture();
    for trace in data["traces"].as_array().unwrap() {
        let retention = Rc::new(TransactionRetention::new(trace["capacity"].as_u64().unwrap() as usize));
        let deletions = Rc::new(RefCell::new(Vec::<Value>::new()));
        let maps: Vec<_> = (0..3)
            .map(|index| {
                let deletions = deletions.clone();
                BoundedTransactionMap::new(
                    retention.clone(),
                    Some(Rc::new(move |value| {
                        let value = value.borrow();
                        deletions.borrow_mut().push(json!([index, value.get("tag").unwrap_or(&Value::Null)]));
                        if value["cleanupError"] == true {
                            Err(LiveError::error("cleanup"))
                        } else {
                            Ok(())
                        }
                    })),
                )
            })
            .collect();
        for (index, (step, expected)) in trace["steps"].as_array().unwrap().iter().zip(trace["results"].as_array().unwrap()).enumerate() {
            let key = step["key"].as_str().unwrap();
            let map = step["map"].as_u64().unwrap_or(0) as usize;
            let mut error = None;
            match step["op"].as_str().unwrap() {
                "set" => error = maps[map].set_at(key, Rc::new(RefCell::new(step["value"].clone())), 1000.0).err(),
                "update" => {
                    if let Some(record) = maps[map].get(key) {
                        record.borrow_mut().as_object_mut().unwrap().extend(step["value"].as_object().unwrap().clone());
                    }
                }
                "delete" => {
                    maps[map].delete(key);
                }
                "flight" => {
                    if step["value"] == true {
                        mark_in_flight(key)
                    } else {
                        clear_in_flight(key)
                    }
                }
                _ => unreachable!(),
            }
            let rows: Vec<Value> =
                maps.iter().map(|map| Value::Array(map.entries().into_iter().map(|(k, v)| json!([k, *v.borrow()])).collect())).collect();
            let mut got = json!({"bytes":retention.bytes(),"maps":rows,"deletions":*deletions.borrow()});
            if let Some(error) = error {
                got["error"] = json!(error.message());
            }
            assert_eq!(
                kumi_common::js::json::stringify(&got),
                kumi_common::js::json::stringify(expected),
                "capacity={} step {index}: {step}",
                trace["capacity"]
            );
        }
        for step in trace["steps"].as_array().unwrap() {
            clear_in_flight(step["key"].as_str().unwrap());
        }
    }
}
#[test]
fn mutation_digest_bounds_and_result_redaction_preserve_host_contract() {
    let mut value = Value::Null;
    for _ in 0..256 {
        value = json!([value]);
    }
    assert!(canonical_mutation_identity(&value).is_ok());
    value = json!([value]);
    assert_eq!(canonical_mutation_identity(&value).unwrap_err().message(), "mutation authority is too deeply nested");
    let value = json!("x".repeat(1_048_577));
    assert_eq!(canonical_mutation_identity(&value).unwrap_err().message(), "mutation authority string is too large");
    assert_eq!(canonical_mutation_identity(&json!({"b":-0.0,"a":"😀"})).unwrap(), r#"{"a":"😀","b":0}"#);
    assert!(!same_live_value(None, Some(&Value::Null)));
    assert!(same_live_value(None, None));
    assert_eq!(
        adapter_tool_error(&json!(4), &LiveError::error("/tmp/secret/session.als"), "Set preview requires fresh authoritative state.")
            ["result"],
        json!({
            "content":[{"type":"text","text":r#"{"reason":"adapter request failed","remediation":"Nothing changed in Live: fix what the reason says (or take another route) and preview again."}"#}],"isError":true
        })
    );
    assert!(nothing_changed(&LiveError::error("request failed: invalid position; nothing changed")));
    assert!(nothing_changed(&LiveError::error("request failed: invalid position; nothing changedé")));
    assert!(!nothing_changed(&LiveError::error("request failed: invalid position; nothing changedMuch")));
}
