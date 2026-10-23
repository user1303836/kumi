use kumi_common::{abort::Signal, js::json::stringify};
use kumi_runtime::{
    create_inference_only_integration,
    integrations::ableton::{
        references::{slim_mixers, References},
        BRIDGE_TOOLS,
    },
    ConnectionState, RuntimeError,
};
use serde_json::{json, Value};
use std::{cell::RefCell, rc::Rc};
#[test]
fn authoritative_reference_lifetimes_short_names_and_cursor_queries_match_source() {
    let data: Value = serde_json::from_str(&kumi_testkit::oracle("kumi-runtime/references-oracle.json")).unwrap();
    assert_eq!(json!(&*BRIDGE_TOOLS), data["bridgeTools"]);
    let mut book = References::default();
    for case in data["cases"].as_array().unwrap() {
        let args = &case["args"];
        let result = match case["method"].as_str().unwrap() {
            "registerRows" => book
                .register_rows(
                    args[0].as_str().unwrap(),
                    &serde_json::from_value::<Vec<_>>(args[1].clone()).unwrap(),
                    args[2].as_object().unwrap(),
                    args.get(3).and_then(Value::as_str),
                )
                .map(|_| Value::Null)
                .map_err(|e| e.to_string()),
            "validateParentAndCursor" => {
                book.validate_parent_and_cursor(args[0].as_object().unwrap()).map(|_| Value::Null).map_err(|e| e.to_string())
            }
            "shortRef" => Ok(json!(book.short_ref(args[0].as_str().unwrap()))),
            "shorten" => Ok(book.shorten(&args[0])),
            "lengthen" => Ok(book.lengthen(&args[0])),
            "requireFreshReferences" => {
                book.require_fresh_references(args[0].as_object().unwrap()).map(|_| Value::Null).map_err(|e| e.to_string())
            }
            "slimMixers" => Ok(json!(slim_mixers(args[0].as_object().unwrap()))),
            "encode" => {
                let result = book.encode(
                    &serde_json::from_value(args[0].clone()).unwrap(),
                    args[1].as_f64().unwrap(),
                    args[2].as_bool().unwrap(),
                    "2026-10-03T12:00:00.000Z",
                    "connection",
                );
                Ok(json!({"text":result.text,"isError":result.is_error}))
            }
            "invalidate" => {
                book.invalidate();
                Ok(Value::Null)
            }
            _ => panic!("unknown case {case}"),
        };
        let result = result.unwrap_or_else(|e| json!({"error":e}));
        assert_eq!(stringify(&result), stringify(&case["value"]), "{case}");
        assert_eq!(json!(book.refs.iter().collect::<Vec<_>>()), case["refs"], "{case}");
        assert_eq!(json!(book.cursors.iter().collect::<Vec<_>>()), case["cursors"], "{case}");
        assert_eq!(json!(book.known.iter().collect::<Vec<_>>()), case["known"], "{case}");
    }
}
#[tokio::test]
async fn inference_only_observation_and_closed_cancellation_order_match_source() {
    let states = Rc::new(RefCell::new(vec![]));
    let seen = states.clone();
    let integration = create_inference_only_integration(Rc::new(move |state| seen.borrow_mut().push(state)));
    let before = chrono::Utc::now();
    let observation = integration.observe(Signal::new(), None).await.unwrap();
    let after = chrono::Utc::now();
    let context: Value = serde_json::from_str(&observation.context).unwrap();
    let at = chrono::DateTime::parse_from_rfc3339(context["observedAt"].as_str().unwrap()).unwrap();
    assert!(at.timestamp_millis() >= before.timestamp_millis() && at.timestamp_millis() <= after.timestamp_millis());
    assert_eq!(observation.key, "inference-only");
    assert_eq!(observation.revision.as_deref(), Some("no-live"));
    assert_eq!(observation.label, "Inference-only — No Live access");
    assert_eq!(observation.instructions, kumi_runtime::integrations::ableton::context::INSTRUCTIONS);
    assert_eq!(context["mode"], "inference-only");
    assert!(observation.tools.is_empty() && observation.project.is_none() && observation.tracks.is_none());
    integration.start(Signal::new()).await.unwrap();
    integration.start(Signal::new()).await.unwrap();
    assert_eq!(*states.borrow(), [ConnectionState::Disconnected, ConnectionState::Disconnected]);
    let cancelled = Signal::new();
    cancelled.cancel();
    assert!(matches!(integration.start(cancelled.clone()).await, Err(RuntimeError::Aborted)));
    integration.close().await.unwrap();
    integration.close().await.unwrap();
    assert_eq!(integration.start(Signal::new()).await.unwrap_err().to_string(), "Integration is closed");
    assert_eq!(integration.observe(Signal::new(), None).await.err().unwrap().to_string(), "Integration is closed");
    assert!(matches!(integration.observe(cancelled, None).await, Err(RuntimeError::Aborted)));
}
