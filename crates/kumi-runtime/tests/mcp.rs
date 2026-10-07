use async_trait::async_trait;
use kumi_common::abort::{self, Controller, Signal};
use kumi_runtime::{
    core::{contracts::JsonObject, errors::RuntimeError},
    mcp::{
        allowed_tools::{AllowedTools, CallOptions},
        client::{connect_mcp, LinearReadBuffer, McpEndpoint, Options, StderrStatus},
        types::{CallToolResult, Implementation, ListToolsResult},
    },
};
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    collections::HashSet,
    path::PathBuf,
    rc::Rc,
    time::{Duration, Instant},
};
use tokio::{task::LocalSet, time::sleep};

fn signal() -> Signal {
    Controller::new().signal
}
fn object(value: Value) -> JsonObject {
    value.as_object().unwrap().clone()
}
fn data(result: &CallToolResult) -> Value {
    let value = serde_json::to_value(result).unwrap();
    value.get("structuredContent").cloned().unwrap_or_else(|| serde_json::from_str(&text(result)).unwrap())
}
fn text(result: &CallToolResult) -> String {
    let value = serde_json::to_value(result).unwrap();
    value["content"].as_array().unwrap().iter().filter_map(|part| part.get("text").and_then(Value::as_str)).collect()
}
fn node() -> PathBuf {
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|directory| directory.join(if cfg!(windows) { "node.exe" } else { "node" }))
        .find(|path| path.is_file())
        .expect("MCP SDK interoperability tests need Node and: npm ci --prefix crates/kumi-runtime/tests/support")
}
fn options(mode: &str, timeout: u64) -> Options {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    Options {
        entry: Some(node()),
        args: vec![PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/support/mcp-server.mjs").to_string_lossy().into(), mode.into()],
        signal: signal(),
        timeout_ms: Some(timeout),
        connect_timeout_ms: Some(timeout.max(10_000)),
        cwd: Some(root),
        ..Default::default()
    }
}
async fn open(mode: &str, timeout: u64) -> (Rc<dyn McpEndpoint>, AllowedTools) {
    let client = connect_mcp(options(mode, timeout)).await.unwrap();
    let tools = AllowedTools::new(client.clone(), HashSet::new());
    (client, tools)
}
async fn call(tools: &AllowedTools, name: &str, args: Value) -> Result<CallToolResult, RuntimeError> {
    tools.call(name, object(args), signal(), CallOptions::default()).await
}
#[cfg(unix)]
fn running(pid: Option<u32>) -> bool {
    pid.is_some_and(|pid| unsafe { libc::kill(pid as i32, 0) == 0 })
}
#[cfg(windows)]
fn running(pid: Option<u32>) -> bool {
    let Some(pid) = pid else { return false };
    std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH"])
        .output()
        .is_ok_and(|out| String::from_utf8_lossy(&out.stdout).contains(&pid.to_string()))
}
// Source node:test cases run sequentially. Keep independent SDK startups and the
// 65 MB buffer check from competing, while preserving concurrency within each case.
static MCP_CASE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
macro_rules! local_test {
    ($name:ident, $body:block) => { #[tokio::test] async fn $name() {
        let _case = MCP_CASE.lock().await;
        LocalSet::new().run_until(async $body).await
    } };
}

local_test!(real_sdk_stdio_initialization_bounded_pagination_and_exact_four_tool_schema_intersection, {
    let (client, tools) = open("normal", 2000).await;
    let pid = client.pid();
    tools.refresh(signal()).await.unwrap();
    let mut names: Vec<_> = tools.list().iter().map(|tool| tool.name.clone()).collect();
    names.sort();
    assert_eq!(names, ["live_discover", "live_note_read", "live_status", "server_status"]);
    assert_eq!(
        serde_json::to_value(&tools.list()[0].input_schema).unwrap(),
        json!({"type":"object","properties":{"action":{"type":"string"}},"additionalProperties":true})
    );
    assert_eq!(data(&call(&tools, "live_status", json!({})).await.unwrap())["provenance"], "synthetic-fixture");
    assert_eq!(client.server_info().unwrap().name, "kumi-synthetic-fixture");
    let generation = tools.generation();
    tools.refresh(signal()).await.unwrap();
    assert_eq!(tools.generation(), generation);
    tools.close().await.unwrap();
    tools.close().await.unwrap();
    assert!(!running(pid));
});

local_test!(unknown_and_mutation_calls_never_reach_the_server_fresh_call_time_catalog_gate_rejects_old_tools, {
    let (client, tools) = open("normal", 2000).await;
    tools.refresh(signal()).await.unwrap();
    for name in ["mutation_0", "live_tempo_apply", "tools/call", "new_unsafe_tool"] {
        assert!(call(&tools, name, json!({})).await.is_err());
    }
    assert_eq!(data(&call(&tools, "server_status", json!({})).await.unwrap())["calls"], json!(["server_status"]));
    client.call("server_status", object(json!({"action":"notify"})), signal()).await.unwrap();
    assert!(call(&tools, "live_note_read", json!({})).await.unwrap_err().to_string().contains("catalog"));
    tools.refresh(signal()).await.unwrap();
    assert!(!tools.list().iter().any(|tool| tool.name == "live_note_read"));
    for name in ["live_note_read", "new_unsafe_tool"] {
        assert!(call(&tools, name, json!({})).await.is_err());
    }
    assert_eq!(
        data(&call(&tools, "server_status", json!({})).await.unwrap())["calls"],
        json!(["server_status", "server_status", "server_status"])
    );
    tools.close().await.unwrap();
});

local_test!(an_answer_arriving_after_its_request_was_cancelled_doesnt_cost_the_connection, {
    let (client, tools) = open("normal", 2000).await;
    let disconnected = Rc::new(Cell::new(false));
    let changed = disconnected.clone();
    let _unlisten = client.on_disconnect(Rc::new(move || changed.set(true)));
    tools.refresh(signal()).await.unwrap();
    assert!(client.call("server_status", object(json!({"action":"late"})), abort::timeout(50)).await.is_err());
    sleep(Duration::from_millis(400)).await;
    assert!(!disconnected.get());
    assert_eq!(data(&call(&tools, "server_status", json!({})).await.unwrap())["fixture"], true);
    tools.close().await.unwrap();
});

local_test!(catalog_loops_excess_tools_and_duplicates_fail_closed, {
    for mode in ["repeat-cursor", "excessive", "duplicate", "catalog-bytes"] {
        let (_, tools) = open(mode, 2000).await;
        assert!(tools.refresh(signal()).await.is_err());
        assert!(tools.list().is_empty());
        tools.close().await.unwrap();
    }
});

local_test!(mcp_is_error_structured_data_are_preserved_oversized_output_becomes_explicit_narrowing_error, {
    let (_, tools) = open("normal", 2000).await;
    tools.refresh(signal()).await.unwrap();
    let failed = call(&tools, "server_status", json!({"action":"error"})).await.unwrap();
    assert_eq!(failed.is_error, Some(true));
    assert_eq!(data(&failed)["reason"], "expected-error");
    let large = call(&tools, "server_status", json!({"action":"oversized"})).await.unwrap();
    assert_eq!(large.is_error, Some(true));
    assert!(text(&large).contains("too large; narrow"));
    assert!(serde_json::to_vec(&large).unwrap().len() < 1024);
    tools.close().await.unwrap();
});

local_test!(timeouts_and_signal_cancellation_reach_the_mcp_request_and_leave_the_client_usable, {
    let (_, tools) = open("normal", 150).await;
    tools.refresh(signal()).await.unwrap();
    assert!(call(&tools, "server_status", json!({"action":"delay"})).await.unwrap_err().to_string().contains("MCP request"));
    assert!(tools.call("server_status", object(json!({"action":"delay"})), abort::timeout(20), CallOptions::default()).await.is_err());
    sleep(Duration::from_millis(20)).await;
    assert_eq!(data(&call(&tools, "server_status", json!({})).await.unwrap())["cancelled"], 2);
    tools.close().await.unwrap();
});

local_test!(unexpected_child_exit_invalidates_catalog_and_reports_disconnected, {
    let (client, tools) = open("normal", 2000).await;
    tools.refresh(signal()).await.unwrap();
    let disconnected = Rc::new(Cell::new(false));
    let changed = disconnected.clone();
    let _unlisten = client.on_disconnect(Rc::new(move || changed.set(true)));
    assert!(call(&tools, "server_status", json!({"action":"exit"})).await.is_err());
    sleep(Duration::from_millis(10)).await;
    assert!(disconnected.get());
    assert!(tools.list().is_empty());
    assert!(call(&tools, "live_status", json!({})).await.is_err());
    tools.close().await.unwrap();
});

local_test!(child_environment_excludes_inference_credentials_and_stderr_is_drained_without_retaining_payloads, {
    // Run this test in its own process before setting the parent's fixture credentials.
    if std::env::var_os("KUMI_TEST_ENV_PROBE").is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "child_environment_excludes_inference_credentials_and_stderr_is_drained_without_retaining_payloads"])
            .env("KUMI_TEST_ENV_PROBE", "1")
            .env("AI_GATEWAY_API_KEY", "fixture-secret-never-copy")
            .env("ANTHROPIC_API_KEY", "fixture-secret-never-copy")
            .env("KUMI_AUTH_FILE", "fixture-secret-never-copy")
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stdout));
        return;
    }
    let (client, tools) = open("stderr", 2000).await;
    tools.refresh(signal()).await.unwrap();
    assert_eq!(data(&call(&tools, "server_status", json!({})).await.unwrap())["secretPresent"], false);
    assert!(client.stderr_status().truncated);
    assert!(client.stderr_status().bytes <= 64 * 1024);
    assert!(!serde_json::to_string(&client.stderr_status()).unwrap().contains("secret"));
    tools.close().await.unwrap();
});

local_test!(the_bridge_is_asked_to_expose_exactly_kumis_tools_or_only_reads_when_none_are_named, {
    let (_, tools) = open("normal", 2000).await;
    tools.refresh(signal()).await.unwrap();
    let status = data(&call(&tools, "server_status", json!({})).await.unwrap());
    assert_eq!(status["toolPolicy"], "read-only");
    assert_eq!(status["toolAllow"], Value::Null);
    tools.close().await.unwrap();
    let mut options = options("normal", 2000);
    options.allow_tools = ["live_tempo_apply", "live_status", "live_tempo_preview", "live_status"].map(String::from).to_vec();
    let client = connect_mcp(options).await.unwrap();
    let tools = AllowedTools::new(client, HashSet::new());
    tools.refresh(signal()).await.unwrap();
    let status = data(&call(&tools, "server_status", json!({})).await.unwrap());
    assert_eq!(status["toolPolicy"], "full");
    assert_eq!(status["toolAllow"], "live_status,live_tempo_apply,live_tempo_preview");
    tools.close().await.unwrap();
});

local_test!(kumis_own_calls_may_bring_back_more_than_the_models_64_kb_the_models_stay_bounded, {
    let (_, tools) = open("normal", 2000).await;
    tools.refresh(signal()).await.unwrap();
    assert_eq!(call(&tools, "server_status", json!({"action":"oversized"})).await.unwrap().is_error, Some(true));
    assert_ne!(
        tools.call("server_status", object(json!({"action":"oversized"})), signal(), CallOptions { host: true }).await.unwrap().is_error,
        Some(true)
    );
    tools.close().await.unwrap();
});

local_test!(the_bridges_word_on_bad_arguments_comes_back_as_a_tool_error_other_failures_stay_generic, {
    let (_, tools) = open("normal", 2000).await;
    tools.refresh(signal()).await.unwrap();
    let rejected = call(&tools, "server_status", json!({"action":"invalid-params"})).await.unwrap();
    assert_eq!(rejected.is_error, Some(true));
    assert_eq!(
        serde_json::to_value(rejected.content).unwrap(),
        json!([{"type":"text","text":"The bridge rejected the arguments: trackRef is required"}])
    );
    tools.close().await.unwrap();
});

local_test!(bounded_sdk_shutdown_terminates_only_the_owned_stubborn_child, {
    let (sibling, sibling_tools) = open("normal", 2000).await;
    let (client, tools) = open("stubborn", 2000).await;
    let pid = client.pid();
    tokio::time::timeout(Duration::from_secs(8), tools.close()).await.unwrap().unwrap();
    sleep(Duration::from_millis(20)).await;
    assert!(!running(pid));
    assert!(running(sibling.pid()));
    sibling_tools.refresh(signal()).await.unwrap();
    assert_eq!(data(&call(&sibling_tools, "server_status", json!({})).await.unwrap())["fixture"], true);
    sibling_tools.close().await.unwrap();
});

local_test!(missing_capabilities_cannot_be_called_even_under_the_allowlist, {
    let (_, tools) = open("missing", 2000).await;
    tools.refresh(signal()).await.unwrap();
    assert_eq!(tools.list().iter().map(|tool| tool.name.as_str()).collect::<Vec<_>>(), ["server_status"]);
    assert!(call(&tools, "live_status", json!({})).await.unwrap_err().to_string().contains("available"));
    tools.close().await.unwrap();
});

local_test!(a_bridge_that_exits_while_a_child_holds_its_pipes_is_seen_closed, {
    // A host a bridge launched can keep the bridge's stdout and stderr open after the bridge is gone: its requests
    // fail once it's gone, not at their timeouts.
    let (_, tools) = open("normal", 20_000).await;
    tools.refresh(signal()).await.unwrap();
    let started = Instant::now();
    assert!(call(&tools, "server_status", json!({"action":"exit-held"})).await.is_err());
    assert!(started.elapsed() < Duration::from_secs(10), "{:?}", started.elapsed());
    tools.close().await.unwrap();
});

local_test!(oversized_protocol_frame_closes_transport_and_invalidates_old_descriptors, {
    let (_, tools) = open("normal", 2000).await;
    tools.refresh(signal()).await.unwrap();
    assert!(call(&tools, "server_status", json!({"action":"frame"})).await.is_err());
    assert!(tools.list().is_empty());
    tools.close().await.unwrap();
});

local_test!(a_bridge_that_cant_start_says_why_in_its_own_words, {
    // The bridge's last word ("mcp-host: …") is what the producer reads, not "check the built bridge…": Live left
    // running through an update needs a restart, not another update.
    let failure = connect_mcp(options("fatal", 2000)).await.err().unwrap().to_string();
    assert_eq!(
        failure,
        "Kumi's bridge didn't start: Live is running another version of Kumi's bridge than the one installed (Live loads it when it starts): restart Live"
    );
    // Anything else on its stderr stays unread, as before.
    assert!(!failure.contains("noise"));
});

local_test!(failed_startup_closes_the_owned_child_already_aborted_startup_creates_none, {
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("pid");
    let mut opts = options("no-init", 200);
    opts.connect_timeout_ms = None;
    opts.args.push(pid_file.to_string_lossy().into());
    assert!(connect_mcp(opts).await.err().unwrap().to_string().contains("connection failed"));
    assert!(!running(Some(std::fs::read_to_string(&pid_file).unwrap().parse().unwrap())));
    std::fs::remove_file(&pid_file).unwrap();
    let mut opts = options("no-init", 200);
    opts.args.push(pid_file.to_string_lossy().into());
    opts.signal.cancel();
    assert!(connect_mcp(opts).await.is_err());
    assert!(!pid_file.exists());
});

local_test!(actual_built_bridge_interoperates_without_config_and_truthfully_reports_unavailable_live, {
    let entry = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug").join(if cfg!(windows) {
        "ableton-mcp-server.exe"
    } else {
        "ableton-mcp-server"
    });
    if !entry.is_file() {
        assert_ne!(std::env::var("KUMI_TEST_BRIDGE").as_deref(), Ok("1"), "Build the standalone bridge before the interoperability check");
        eprintln!("Standalone bridge is not built; build it and set KUMI_TEST_BRIDGE=1 for required interoperability verification");
        return;
    }
    let client =
        connect_mcp(Options { entry: Some(entry), signal: signal(), cwd: Some(std::env::temp_dir()), ..Default::default() }).await.unwrap();
    let tools = AllowedTools::new(client, HashSet::new());
    tools.refresh(signal()).await.unwrap();
    let status = data(&call(&tools, "live_status", json!({})).await.unwrap());
    assert_eq!(status["connected"], false);
    assert_ne!(status["provenance"], "real-live");
    assert_eq!(status["adapter"], "unavailable");
    assert!(!tools.list().iter().any(|tool| tool.name == "live_discover"));
    tools.close().await.unwrap();
});

#[derive(Default)]
struct ChangingEndpoint {
    lists: Cell<usize>,
    announce: RefCell<Option<Rc<dyn Fn()>>>,
    limits: RefCell<Vec<Value>>,
    small: bool,
}
#[async_trait(?Send)]
impl McpEndpoint for ChangingEndpoint {
    fn pid(&self) -> Option<u32> {
        None
    }
    fn server_info(&self) -> Option<Implementation> {
        None
    }
    fn stderr_status(&self) -> StderrStatus {
        StderrStatus { bytes: 0, truncated: false }
    }
    fn on_catalog_changed(&self, listener: Rc<dyn Fn()>) -> Box<dyn Fn()> {
        *self.announce.borrow_mut() = Some(listener);
        Box::new(|| {})
    }
    fn on_disconnect(&self, _: Rc<dyn Fn()>) -> Box<dyn Fn()> {
        Box::new(|| {})
    }
    async fn close(&self) -> Result<(), RuntimeError> {
        Ok(())
    }
    async fn list(&self, _: Option<&str>, _: Signal) -> Result<ListToolsResult, RuntimeError> {
        let lists = self.lists.get() + 1;
        self.lists.set(lists);
        sleep(Duration::from_millis(20)).await;
        if lists == 1 {
            if let Some(announce) = self.announce.borrow().as_ref() {
                announce();
            }
        }
        Ok(serde_json::from_value(json!({"tools":[{"name":"live_status","inputSchema":{"type":"object"}},{"name":"live_discover","inputSchema":{"type":"object"}}]})).unwrap())
    }
    async fn call(&self, _: &str, args: JsonObject, _: Signal) -> Result<CallToolResult, RuntimeError> {
        self.limits.borrow_mut().push(args.get("limit").cloned().unwrap_or(Value::Null));
        Ok(serde_json::from_value(if self.small && args.get("limit").and_then(Value::as_u64).unwrap_or(0) > 100 {
            json!({"isError":true,"content":[{"type":"text","text":"discovery limit is invalid"}]})
        } else {
            json!({"content":[{"type":"text","text":"{}"}]})
        })
        .unwrap())
    }
}
local_test!(reads_sent_together_share_one_reading_of_the_catalog_and_a_change_announced_meanwhile_starts_it_over, {
    let endpoint = Rc::new(ChangingEndpoint::default());
    let tools = AllowedTools::new(endpoint.clone(), HashSet::new());
    let results = futures::future::join_all((0..4).map(|_| tools.refresh(signal()))).await;
    for result in results {
        result.unwrap();
    }
    assert!(tools.has("live_status"));
    assert_eq!(endpoint.lists.get(), 2);
    tools.close().await.unwrap();
});
local_test!(a_caller_that_gives_up_leaves_the_shared_reading_to_the_others_that_joined_it, {
    // The first caller's short deadline (a status probe's) ended the reading for everyone who joined it.
    let endpoint = Rc::new(ChangingEndpoint::default());
    let tools = AllowedTools::new(endpoint.clone(), HashSet::new());
    let (hasty, patient) = tokio::join!(tools.refresh(abort::timeout(5)), tools.refresh(signal()));
    assert!(matches!(hasty, Err(RuntimeError::Aborted)), "{hasty:?}");
    patient.unwrap();
    assert!(tools.has("live_status"));
    tools.close().await.unwrap();
});
local_test!(a_discovery_page_the_remote_script_refuses_as_too_big_is_asked_again_at_100_rows_and_from_then_on, {
    let endpoint = Rc::new(ChangingEndpoint { small: true, ..Default::default() });
    let tools = AllowedTools::new(endpoint.clone(), HashSet::from(["live_discover".into()]));
    for kind in ["track", "device"] {
        assert_eq!(
            tools
                .call("live_discover", object(json!({"kind":kind,"limit":100_000})), signal(), CallOptions { host: true })
                .await
                .unwrap()
                .is_error,
            None
        );
    }
    assert_eq!(*endpoint.limits.borrow(), [json!(100_000), json!(100), json!(100)]);
    tools.close().await.unwrap();
});
local_test!(a_bridge_message_of_megabytes_a_big_sets_page_arrives_whole_read_in_linear_time, {
    let (_, tools) = open("normal", 2000).await;
    tools.refresh(signal()).await.unwrap();
    let start = Instant::now();
    let result = tools.call("server_status", object(json!({"action":"large"})), signal(), CallOptions { host: true }).await.unwrap();
    assert_eq!(text(&result).len(), 5 * 1024 * 1024);
    assert!(start.elapsed() < Duration::from_secs(5));
    assert!(!data(&call(&tools, "server_status", json!({})).await.unwrap())["calls"].as_array().unwrap().is_empty());
    tools.close().await.unwrap();
});
#[test]
fn the_bridges_read_buffer_looks_at_each_byte_once_however_the_message_is_cut_into_chunks() {
    let _case = MCP_CASE.blocking_lock();
    let mut buffer = LinearReadBuffer::new(128 * 1024 * 1024);
    let chunk = vec![b' '; 64 * 1024];
    let start = Instant::now();
    buffer.append(br#"{"jsonrpc":"2.0","id":1,"result":{"text":""#).unwrap();
    for _ in 0..1000 {
        buffer.append(&chunk).unwrap();
        assert!(buffer.read_message().unwrap().is_none());
    }
    buffer.append(b"\"}}\n{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{}}\n{\"jsonrpc\"").unwrap();
    let first = serde_json::to_value(buffer.read_message().unwrap().unwrap()).unwrap();
    assert_eq!(first["id"], 1);
    assert_eq!(first["result"]["text"].as_str().unwrap().len(), 1000 * 64 * 1024);
    assert_eq!(serde_json::to_value(buffer.read_message().unwrap().unwrap()).unwrap()["id"], 2);
    assert!(buffer.read_message().unwrap().is_none());
    buffer.append(b":\"2.0\",\"id\":3,\"result\":{}}\r\n").unwrap();
    assert_eq!(serde_json::to_value(buffer.read_message().unwrap().unwrap()).unwrap()["id"], 3);
    assert!(start.elapsed() < Duration::from_millis(600), "{:?}", start.elapsed());
    assert!(LinearReadBuffer::new(10).append(&[0; 11]).unwrap_err().to_string().contains("exceeded maximum size of 10 bytes"));
}
