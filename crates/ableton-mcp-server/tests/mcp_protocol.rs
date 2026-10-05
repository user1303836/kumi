//! The parts that exercise `mcp_protocol` and
//! `stdio` directly. The tests there that drive `McpHost` (discovery, tool calls, policy, the stdio
//! `serve` wrapper) belong with the host's port; the wire expectations they assert through the host
//! are checked here against `prepare_mcp_request` and `format_mcp_response` themselves.

#[path = "support/streams.rs"]
mod streams;

use std::cell::RefCell;
use std::future::Future;
use std::rc::Rc;

use ableton_mcp_server::mcp_protocol::{
    format_mcp_response, prepare_mcp_request, ProtocolEra, LEGACY_PROTOCOL_VERSION, MODERN_PROTOCOL_VERSION, MODERN_UNAVAILABLE_TOOLS,
    SUPPORTED_PROTOCOL_VERSIONS,
};
use ableton_mcp_server::stdio::{serve_stdio, HandlerOutcome, RecordContext, RecordHandler, StdioOptions};
use futures::FutureExt;
use kumi_common::abort::Signal;
use serde_json::{json, Map, Value};
use streams::{Pipe, WriterHandle};
use tokio::sync::Notify as Gate;
use tokio::task::{spawn_local, LocalSet};

const VERSION_KEY: &str = "io.modelcontextprotocol/protocolVersion";
const CAPABILITIES_KEY: &str = "io.modelcontextprotocol/clientCapabilities";

fn modern(id: Value, method: &str, params: Value, meta: Value) -> Value {
    let mut full_meta = Map::new();
    full_meta.insert(VERSION_KEY.to_string(), json!(MODERN_PROTOCOL_VERSION));
    full_meta.insert(CAPABILITIES_KEY.to_string(), json!({}));
    for (key, value) in meta.as_object().cloned().unwrap_or_default() {
        full_meta.insert(key, value);
    }
    let mut full_params = params.as_object().cloned().unwrap_or_default();
    full_params.insert("_meta".to_string(), Value::Object(full_meta));
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": full_params })
}

fn initialize() -> Value {
    json!({ "jsonrpc": "2.0", "id": "legacy-init", "method": "initialize", "params": { "protocolVersion": LEGACY_PROTOCOL_VERSION, "capabilities": {}, "clientInfo": { "name": "legacy-test", "version": "1" } } })
}

fn error_code(prepared: &ableton_mcp_server::mcp_protocol::PreparedMcpRequest) -> Option<i64> {
    prepared.error.as_ref().and_then(|error| error["error"]["code"].as_i64())
}

fn handler<F, Fut>(f: F) -> RecordHandler
where
    F: Fn(String, Option<RecordContext>) -> Fut + 'static,
    Fut: Future<Output = HandlerOutcome> + 'static,
{
    Rc::new(move |record, context| f(record, context).boxed_local())
}

async fn local<T>(future: impl Future<Output = T>) -> T {
    LocalSet::new().run_until(future).await
}

async fn tick() {
    for _ in 0..32 {
        tokio::task::yield_now().await;
    }
}

#[test]
fn modern_discovery_is_handshake_free_and_selected_eras_are_not_silently_mixed() {
    // server/discover needs no handshake: it is modern by method, so its metadata is required.
    let discovery = prepare_mcp_request(&modern(json!(1), "server/discover", json!({}), json!({})), None);
    assert!(discovery.modern);
    assert_eq!(discovery.error, None);
    assert_eq!(discovery.input["params"], json!({}), "protocol metadata is stripped before dispatch");
    // A legacy initialize is untouched by the modern layer.
    let legacy = prepare_mcp_request(&initialize(), None);
    assert!(!legacy.modern);
    assert_eq!(legacy.input, initialize());
    // After legacy initialization, a modern request is refused rather than mixed in.
    let mixed = prepare_mcp_request(&modern(json!(3), "tools/list", json!({}), json!({})), Some(ProtocolEra::Legacy));
    assert_eq!(error_code(&mixed), Some(-32602), "selected eras are not silently mixed");
    assert_eq!(
        mixed.error.as_ref().unwrap()["error"]["message"],
        json!("Do not mix protocol eras after legacy initialization; start a new stdio process")
    );
    // server/discover stays available after legacy initialization.
    assert_eq!(prepare_mcp_request(&modern(json!(4), "server/discover", json!({}), json!({})), Some(ProtocolEra::Legacy)).error, None);
}

#[test]
fn modern_calls_work_without_discovery_and_every_request_independently_supplies_metadata() {
    let first = prepare_mcp_request(&modern(json!("reusable"), "tools/list", json!({}), json!({})), None);
    assert!(first.modern && first.error.is_none());
    let second = prepare_mcp_request(
        &modern(json!("reusable"), "tools/list", json!({}), json!({ CAPABILITIES_KEY: { "futureCapability": true } })),
        None,
    );
    assert!(second.modern && second.error.is_none(), "unknown capabilities are tolerated");
    // Once the era is modern, a bare legacy-shaped request lacks its metadata.
    let bare = prepare_mcp_request(&json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }), Some(ProtocolEra::Modern));
    assert_eq!(error_code(&bare), Some(-32602));
    assert_eq!(
        bare.error.as_ref().unwrap()["error"]["message"],
        json!("Modern requests require protocolVersion and clientCapabilities in params._meta")
    );
    assert_eq!(error_code(&prepare_mcp_request(&initialize(), Some(ProtocolEra::Modern))), Some(-32602));
    assert_eq!(
        prepare_mcp_request(&modern(json!(5), "initialize", json!({}), json!({})), None).error.as_ref().unwrap()["error"]["message"],
        json!("Modern requests do not use initialize")
    );
}

#[test]
fn unsupported_versions_and_malformed_metadata_fail_before_adapter_use_and_do_not_select_an_era() {
    let unknown = prepare_mcp_request(&modern(json!(1), "tools/list", json!({}), json!({ VERSION_KEY: "2099-01-01" })), None);
    let error = unknown.error.as_ref().expect("an error frame");
    assert_eq!(error["error"]["code"], json!(-32022));
    assert_eq!(
        error["error"]["data"],
        json!({ "requested": "2099-01-01", "supported": [MODERN_PROTOCOL_VERSION, LEGACY_PROTOCOL_VERSION] })
    );
    assert_eq!(error["id"], json!(1));
    assert_eq!(SUPPORTED_PROTOCOL_VERSIONS, [MODERN_PROTOCOL_VERSION, LEGACY_PROTOCOL_VERSION]);
    // `{ [capabilitiesKey]: undefined }` has no JSON spelling: the key is null or absent.
    let mut without_capabilities = modern(json!(2), "server/discover", json!({}), json!({}));
    without_capabilities["params"]["_meta"].as_object_mut().unwrap().remove(CAPABILITIES_KEY);
    assert_eq!(error_code(&prepare_mcp_request(&without_capabilities, None)), Some(-32602));
    for meta in [
        json!({ CAPABILITIES_KEY: null }),
        json!({ CAPABILITIES_KEY: [] }),
        json!({ CAPABILITIES_KEY: { "sampling": true } }),
        json!({ "bad key": true }),
        json!({ "io.modelcontextprotocol/clientInfo": { "name": "missing-version" } }),
        json!({ "progressToken": false }),
        json!({ "io.modelcontextprotocol/logLevel": "all" }),
    ] {
        let prepared = prepare_mcp_request(&modern(json!(2), "server/discover", json!({}), meta.clone()), None);
        assert_eq!(error_code(&prepared), Some(-32602), "{meta}");
    }
    assert_eq!(error_code(&prepare_mcp_request(&json!({ "jsonrpc": "2.0", "id": 3, "method": "server/discover" }), None)), Some(-32602));
    assert_eq!(prepare_mcp_request(&initialize(), None).error, None, "no era was selected by the failures above");
    let input = modern(json!(4), "tools/list", json!({}), json!({ "com.example/trace": { "private": true } }));
    let before = input.clone();
    let prepared = prepare_mcp_request(&input, None);
    assert_eq!(input, before, "wire parsing does not mutate caller input");
    assert_eq!(prepared.error, None);
}

#[test]
fn each_validation_message_is_exact() {
    let message = |input: Value, era: Option<ProtocolEra>| {
        prepare_mcp_request(&input, era).error.map(|error| error["error"]["message"].as_str().unwrap().to_string())
    };
    let mut extra = modern(json!(1), "tools/list", json!({}), json!({}));
    extra["extra"] = json!(true);
    assert_eq!(message(extra, None).as_deref(), Some("Invalid modern request envelope"));
    assert_eq!(
        message(modern(json!(1), "tools/list", json!({}), json!({ VERSION_KEY: "" })), None).as_deref(),
        Some("Invalid protocol version metadata")
    );
    assert_eq!(
        message(modern(json!(1), "tools/list", json!({}), json!({ VERSION_KEY: LEGACY_PROTOCOL_VERSION })), None).as_deref(),
        Some("Legacy 2025-11-25 requires initialize on a legacy stdio process")
    );
    assert_eq!(
        message(modern(json!(1), "tools/list", json!({}), json!({ "bad key": 1 })), None).as_deref(),
        Some("Invalid or oversized request metadata")
    );
    assert_eq!(
        message(modern(json!(1), "tools/list", json!({}), json!({ "io.modelcontextprotocol/clientInfo": { "name": "x" } })), None)
            .as_deref(),
        Some("Invalid clientInfo metadata")
    );
    assert_eq!(
        message(modern(json!(1), "tools/list", json!({}), json!({ CAPABILITIES_KEY: { "roots": [] } })), None).as_deref(),
        Some("Invalid clientCapabilities metadata")
    );
    assert_eq!(
        message(modern(json!(1), "tools/list", json!({}), json!({ CAPABILITIES_KEY: { "experimental": { "x": 1 } } })), None).as_deref(),
        Some("Invalid experimental capabilities")
    );
    assert_eq!(
        message(modern(json!(1), "tools/list", json!({}), json!({ CAPABILITIES_KEY: { "elicitation": { "form": 1 } } })), None).as_deref(),
        Some("Invalid client capability settings")
    );
    assert_eq!(
        message(modern(json!(1), "tools/list", json!({}), json!({ CAPABILITIES_KEY: { "extensions": { "no-slash": {} } } })), None)
            .as_deref(),
        Some("Invalid extension capabilities")
    );
    assert_eq!(
        message(
            modern(json!(1), "tools/list", json!({}), json!({ CAPABILITIES_KEY: { "extensions": { "com.example/trace": {} } } })),
            None
        ),
        None
    );
    assert_eq!(
        message(modern(json!(1), "tools/list", json!({}), json!({ "progressToken": [] })), None).as_deref(),
        Some("Invalid progress token")
    );
    assert_eq!(message(modern(json!(1), "tools/list", json!({}), json!({ "progressToken": 7 })), None), None);
    assert_eq!(
        message(modern(json!(1), "tools/list", json!({}), json!({ "io.modelcontextprotocol/logLevel": 5 })), None).as_deref(),
        Some("Invalid log level")
    );
    assert_eq!(
        message(modern(json!(1), "tools/call", json!({ "name": 1 }), json!({})), None).as_deref(),
        Some("Invalid tools/call parameters")
    );
    assert_eq!(
        message(modern(json!(1), "tools/call", json!({ "name": "x", "arguments": [] }), json!({})), None).as_deref(),
        Some("Invalid tools/call parameters")
    );
    assert_eq!(
        message(modern(json!(1), "tools/call", json!({ "name": "live_subscribe", "arguments": { "types": ["state"] } }), json!({})), None)
            .as_deref(),
        Some("Legacy push subscriptions are unavailable in modern mode; use snapshot or observe/poll")
    );
    assert_eq!(MODERN_UNAVAILABLE_TOOLS, ["live_subscribe", "live_unsubscribe"]);
    // The id of a failure frame is kept only when it is a usable id.
    let mut bad_id = modern(json!(true), "tools/call", json!({ "name": 1 }), json!({}));
    bad_id["id"] = json!({ "not": "an id" });
    assert_eq!(prepare_mcp_request(&bad_id, None).error.unwrap()["id"], Value::Null);
    // Notifications and malformed envelopes never reach the modern path.
    let notification =
        json!({ "jsonrpc": "2.0", "method": "notifications/initialized", "params": { "_meta": { VERSION_KEY: MODERN_PROTOCOL_VERSION } } });
    let prepared = prepare_mcp_request(&notification, Some(ProtocolEra::Modern));
    assert!(!prepared.modern && prepared.error.is_none() && prepared.input == notification);
    let prepared = prepare_mcp_request(&json!([1, 2]), Some(ProtocolEra::Modern));
    assert!(!prepared.modern && prepared.input == json!([1, 2]));
}

#[test]
fn modern_results_are_complete_privately_uncacheable_lists_and_promote_structured_content() {
    let server_info = json!({ "name": "ableton-mcp-host", "version": "1.0.0" });
    let discovery = format_mcp_response(
        Some(json!({ "jsonrpc": "2.0", "id": 1, "result": { "supportedVersions": [MODERN_PROTOCOL_VERSION, LEGACY_PROTOCOL_VERSION] } })),
        &json!({ "jsonrpc": "2.0", "id": 1, "method": "server/discover" }),
        true,
        &server_info,
    )
    .unwrap();
    assert_eq!(discovery["result"]["resultType"], json!("complete"));
    assert_eq!(discovery["result"]["ttlMs"], json!(0));
    assert_eq!(discovery["result"]["cacheScope"], json!("private"));
    assert_eq!(discovery["result"]["_meta"]["io.modelcontextprotocol/serverInfo"]["name"], json!("ableton-mcp-host"));
    assert_eq!(
        kumi_common::js::json::stringify(&discovery),
        format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{{\"supportedVersions\":[\"{MODERN_PROTOCOL_VERSION}\",\"{LEGACY_PROTOCOL_VERSION}\"],\"resultType\":\"complete\",\"_meta\":{{\"io.modelcontextprotocol/serverInfo\":{{\"name\":\"ableton-mcp-host\",\"version\":\"1.0.0\"}}}},\"ttlMs\":0,\"cacheScope\":\"private\"}}}}"
        ),
        "keys come in the order the TypeScript wrote them"
    );
    for method in ["prompts/list", "resources/list", "resources/read", "tools/list"] {
        let frame = format_mcp_response(
            Some(json!({ "jsonrpc": "2.0", "id": method, "result": {} })),
            &json!({ "method": method }),
            true,
            &server_info,
        )
        .unwrap();
        assert_eq!(frame["result"]["ttlMs"], json!(0), "{method}");
        assert_eq!(frame["result"]["cacheScope"], json!("private"), "{method}");
    }
    let ping =
        format_mcp_response(Some(json!({ "jsonrpc": "2.0", "id": 2, "result": {} })), &json!({ "method": "ping" }), true, &server_info)
            .unwrap();
    assert_eq!(ping["result"], json!({ "resultType": "complete", "_meta": { "io.modelcontextprotocol/serverInfo": server_info } }));
    // Unbound push tools are not listed.
    let listed = format_mcp_response(
        Some(json!({ "jsonrpc": "2.0", "id": 3, "result": { "tools": [{ "name": "live_subscribe" }, { "name": "live_snapshot" }, { "name": "live_unsubscribe" }, "not-a-tool"] } })),
        &json!({ "method": "tools/list" }),
        true,
        &server_info,
    )
    .unwrap();
    assert_eq!(listed["result"]["tools"], json!([{ "name": "live_snapshot" }]));
    // A tool result's one JSON text becomes structuredContent; prose stays text.
    let text = "{\"state\":\"applied\",\"idempotent\":false}";
    let called = format_mcp_response(
        Some(json!({ "jsonrpc": "2.0", "id": 4, "result": { "content": [{ "type": "text", "text": text }], "isError": false, "_meta": { "kept": true } } })),
        &json!({ "method": "tools/call" }),
        true,
        &server_info,
    )
    .unwrap();
    assert_eq!(called["result"]["structuredContent"], json!({ "state": "applied", "idempotent": false }));
    assert_eq!(called["result"]["_meta"], json!({ "kept": true, "io.modelcontextprotocol/serverInfo": server_info }));
    assert!(called["result"].get("ttlMs").is_none());
    let prose = format_mcp_response(
        Some(json!({ "jsonrpc": "2.0", "id": 5, "result": { "content": [{ "type": "text", "text": "plain words" }] } })),
        &json!({ "method": "tools/call" }),
        true,
        &server_info,
    )
    .unwrap();
    assert!(prose["result"].get("structuredContent").is_none());
    // Error codes the modern era does not use are mapped to invalid params; others and legacy frames pass through.
    for (code, method, expected) in [
        (-32002, "resources/read", -32602),
        (-32042, "ping", -32602),
        (-32601, "tools/call", -32602),
        (-32601, "ping", -32601),
        (-32600, "tools/call", -32600),
    ] {
        let frame = format_mcp_response(
            Some(json!({ "jsonrpc": "2.0", "id": 6, "error": { "code": code, "message": "m" } })),
            &json!({ "method": method }),
            true,
            &server_info,
        )
        .unwrap();
        assert_eq!(frame["error"]["code"], json!(expected), "{code} on {method}");
        assert_eq!(frame["error"]["message"], json!("m"));
    }
    let legacy = json!({ "jsonrpc": "2.0", "id": 7, "result": { "tools": [{ "name": "live_subscribe" }] } });
    assert_eq!(format_mcp_response(Some(legacy.clone()), &json!({ "method": "tools/list" }), false, &server_info), Some(legacy));
    assert_eq!(format_mcp_response(None, &json!({ "method": "tools/list" }), true, &server_info), None);
}

#[tokio::test]
async fn stdio_answers_a_request_as_soon_as_its_work_is_done_not_behind_an_earlier_one_a_cancellation_after_the_answer_changes_nothing() {
    local(async {
        let (input, reader) = Pipe::new();
        let (output, writer) = WriterHandle::new();
        let release = Rc::new(Gate::new());
        let completed = Rc::new(Gate::new());
        let (gate, second) = (release.clone(), completed.clone());
        let run = spawn_local(serve_stdio(
            reader,
            writer,
            handler(move |line, _| {
                let (gate, second) = (gate.clone(), second.clone());
                async move {
                    let frame: Value = serde_json::from_str(&line).unwrap();
                    if frame["id"] == json!(1) {
                        gate.notified().await;
                    }
                    if frame["id"] == json!(2) {
                        second.notify_one();
                    }
                    Ok(Some(json!({ "jsonrpc": "2.0", "id": frame["id"], "result": {} }).to_string()))
                }
            }),
            StdioOptions::default(),
        ));
        input.write(&format!("{}\n{}\n", modern(json!(1), "ping", json!({}), json!({})), modern(json!(2), "ping", json!({}), json!({}))));
        completed.notified().await;
        tick().await;
        input.end_with(&format!(
            "{}\n",
            json!({ "jsonrpc": "2.0", "method": "notifications/cancelled", "params": { "requestId": 2, "reason": "no longer needed", "_meta": {} } })
        ));
        // Request 2 was answered while request 1 still ran.
        assert_eq!(output.received_ids(), vec![json!(2)]);
        tick().await;
        release.notify_one();
        run.await.unwrap().unwrap();
        assert_eq!(output.received_ids(), vec![json!(2), json!(1)]);
    })
    .await;
}

#[tokio::test]
async fn stdio_ignores_malformed_cancellation_and_suppresses_post_cancel_rejection_replies() {
    for malformed in [false, true] {
        local(async move {
            let (input, reader) = Pipe::new();
            let (output, writer) = WriterHandle::new();
            let started = Rc::new(Gate::new());
            let release = Rc::new(Gate::new());
            let observed: Rc<RefCell<Option<Signal>>> = Rc::new(RefCell::new(None));
            let (entry, gate, seen) = (started.clone(), release.clone(), observed.clone());
            let run = spawn_local(serve_stdio(
                reader,
                writer,
                handler(move |line, context| {
                    let (entry, gate, seen) = (entry.clone(), gate.clone(), seen.clone());
                    async move {
                        let frame: Value = serde_json::from_str(&line).unwrap();
                        if frame.get("id").is_none() {
                            return Ok(None);
                        }
                        let signal = context.map(|context| context.signal);
                        *seen.borrow_mut() = signal.clone();
                        entry.notify_one();
                        gate.notified().await;
                        if signal.is_some_and(|signal| signal.is_cancelled()) {
                            return Err("cancelled worker rejection".to_string());
                        }
                        Ok(Some(json!({ "jsonrpc": "2.0", "id": 1, "result": {} }).to_string()))
                    }
                }),
                StdioOptions::default(),
            ));
            input.write(&format!("{}\n", modern(json!(1), "ping", json!({}), json!({}))));
            started.notified().await;
            let reason = if malformed { json!(42) } else { json!("cancel") };
            input.end_with(&format!(
                "{}\n",
                json!({ "jsonrpc": "2.0", "method": "notifications/cancelled", "params": { "requestId": 1, "reason": reason } })
            ));
            tick().await;
            assert_eq!(observed.borrow().as_ref().unwrap().is_cancelled(), !malformed);
            release.notify_one();
            run.await.unwrap().unwrap();
            assert_eq!(output.received().is_empty(), !malformed);
        })
        .await;
    }
}
