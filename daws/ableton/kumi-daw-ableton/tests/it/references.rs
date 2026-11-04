use kumi_agent::ConnectionState;
use kumi_common::abort::Signal;
use kumi_daw_ableton::create_inference_only_integration;
use kumi_types::errors::RuntimeError;
use serde_json::Value;
use std::{cell::RefCell, rc::Rc};
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
    assert_eq!(observation.instructions, kumi_daw_ableton::context::INSTRUCTIONS);
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
