//! The MCP host end to end: JSON-RPC in, the tools against simulated bridges found through their discovery files,
//! JSON-RPC out, with golden transcripts (tests/golden/*-mcp.ndjson; KUMI_BLESS=1 rewrites them).
use daw_bridge::files::ensure_secret;
use daw_bridge::paths::BridgePaths;
use daw_bridge::Daw;
use daw_mcp_server::host::Host;
use daw_mcp_server::simulator::{Simulator, SimulatorConfig};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

fn golden(name: &str, made: &str) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("golden").join(name);
    if std::env::var("KUMI_BLESS").is_ok_and(|value| value == "1") {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, made).unwrap();
        return;
    }
    let kept = std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("{} is missing; run with KUMI_BLESS=1", path.display()));
    assert_eq!(made, kept, "{name} changed; bless it if that's intended");
}

/// A running MCP host on in-memory pipes.
struct Session {
    input: tokio::io::DuplexStream,
    output: tokio::io::Lines<BufReader<tokio::io::DuplexStream>>,
    transcript: String,
    replace: Vec<(String, String)>,
    served: tokio::task::JoinHandle<std::io::Result<()>>,
}

impl Session {
    fn start(daw: Daw, home: &Path) -> Self {
        let (input, host_input) = tokio::io::duplex(1 << 20);
        let (host_output, output) = tokio::io::duplex(1 << 20);
        let host = Arc::new(Host::new(daw, home));
        let served = tokio::spawn(daw_mcp_server::stdio::serve(host, host_input, host_output));
        let pid = std::process::id();
        let replace = vec![
            (home.to_string_lossy().into_owned(), "<home>".into()),
            (format!("\"pid\":{pid}"), "\"pid\":\"<pid>\"".into()),
            (format!("\\\"pid\\\":{pid}"), "\\\"pid\\\":<pid>".into()),
            // The host's own version (serverInfo), which moves with each release.
            (format!("bridge\",\"version\":\"{}\"}}", env!("CARGO_PKG_VERSION")), "bridge\",\"version\":\"<version>\"}".into()),
        ];
        Self { input, output: BufReader::new(output).lines(), transcript: String::new(), replace, served }
    }

    fn hide_port(&mut self, port: u16, name: &str) {
        self.replace.push((format!("\"port\":{port}"), format!("\"port\":\"{name}\"")));
        self.replace.push((format!("\\\"port\\\":{port}"), format!("\\\"port\\\":{name}")));
    }

    async fn call(&mut self, message: Value) -> Value {
        let line = message.to_string();
        self.input.write_all(format!("{line}\n").as_bytes()).await.unwrap();
        self.transcript.push_str(&format!("> {line}\n"));
        if message.get("id").is_none() {
            return Value::Null;
        }
        let answer = tokio::time::timeout(std::time::Duration::from_secs(10), self.output.next_line()).await.unwrap().unwrap().unwrap();
        let mut shown = answer.clone();
        for (text, name) in &self.replace {
            shown = shown.replace(text, name);
        }
        self.transcript.push_str(&format!("< {shown}\n"));
        serde_json::from_str(&answer).unwrap()
    }

    async fn tool(&mut self, id: u64, name: &str, arguments: Value) -> Value {
        let answer = self.call(json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":arguments}})).await;
        answer["result"]["structuredContent"].clone()
    }

    async fn finish(self) -> String {
        drop(self.input);
        self.served.await.unwrap().unwrap();
        self.transcript
    }
}

async fn transcript_for(daw: Daw, config: fn(daw_bridge::auth::Secret, &str) -> SimulatorConfig, resources: [&str; 2]) -> String {
    transcript_with(daw, config, resources, "bounce").await
}

/// The same, with `unknown` the tool asked for that the DAW doesn't have (REAPER and Renoise have no bounce; Logic has
/// its own since M4).
async fn transcript_with(
    daw: Daw,
    config: fn(daw_bridge::auth::Secret, &str) -> SimulatorConfig,
    resources: [&str; 2],
    unknown: &str,
) -> String {
    let home = tempfile::tempdir().unwrap();
    let paths = BridgePaths::new(home.path(), daw);
    let mut session = Session::start(daw, home.path());
    session.call(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"kumi","version":"test"}}})).await;
    session.call(json!({"jsonrpc":"2.0","method":"notifications/initialized"})).await;
    session.call(json!({"jsonrpc":"2.0","id":2,"method":"tools/list"})).await;

    // Not set up: no secret yet.
    let status = session.tool(3, "status", json!({})).await;
    assert_eq!(status["connected"], false);

    // Set up, but nothing running.
    let secret = ensure_secret(&paths.secret()).unwrap();
    assert_eq!(session.tool(4, "discover", json!({})).await["instances"], json!([]));

    // One running.
    let mut first = config(secret.clone(), resources[0]);
    first.discovery_dir = Some(paths.discovery());
    let first = Simulator::start(first).await.unwrap();
    session.hide_port(first.address().port(), "<port-1>");
    let status = session.tool(5, "status", json!({})).await;
    assert_eq!(status["connected"], true);
    assert_eq!(status["resource"], resources[0]);

    // A second instance with another resource folder: both found (newest first), and each can be picked.
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let mut second = config(secret.clone(), resources[1]);
    second.discovery_dir = Some(paths.discovery());
    let second = Simulator::start(second).await.unwrap();
    session.hide_port(second.address().port(), "<port-2>");
    let found = session.tool(6, "discover", json!({})).await;
    assert_eq!(found["instances"].as_array().unwrap().len(), 2);
    assert!(found["instances"].as_array().unwrap().iter().all(|row| row["connected"] == true));
    let picked = session.tool(7, "status", json!({"resource": resources[1]})).await;
    assert_eq!(picked["resource"], resources[1]);
    assert_eq!(picked["instances"], 2);
    assert_eq!(session.tool(8, "status", json!({"resource": "/nowhere"})).await["connected"], false);

    // The picked one quits: status says so; the other is still there.
    second.stop();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(session.tool(9, "status", json!({"resource": resources[1]})).await["connected"], false);
    assert_eq!(session.tool(10, "status", json!({})).await["resource"], resources[0]);

    // A stale discovery file (its process gone) is removed.
    let mut stale = first.discovery();
    stale.pid = 999_999_999;
    std::fs::write(paths.discovery().join(stale.file_name()), serde_json::to_string(&stale).unwrap()).unwrap();
    assert_eq!(session.tool(11, "discover", json!({})).await["staleRemoved"], 1);

    // Errors: an unknown tool, bad arguments, an unknown method.
    session.call(json!({"jsonrpc":"2.0","id":12,"method":"tools/call","params":{"name":unknown,"arguments":{}}})).await;
    session.call(json!({"jsonrpc":"2.0","id":13,"method":"tools/call","params":{"name":"status","arguments":{"pid":"x"}}})).await;
    session.call(json!({"jsonrpc":"2.0","id":14,"method":"resources/list"})).await;
    session.call(json!({"jsonrpc":"2.0","id":15,"method":"ping"})).await;
    first.stop();
    session.finish().await
}

#[tokio::test]
async fn reaper_mcp_transcript() {
    let transcript = transcript_for(Daw::Reaper, SimulatorConfig::reaper, ["/lab/REAPER", "/lab/REAPER portable"]).await;
    golden("reaper-mcp.ndjson", &transcript);
}

#[tokio::test]
async fn renoise_mcp_transcript() {
    let transcript = transcript_for(Daw::Renoise, SimulatorConfig::renoise, ["/lab/Renoise/V3.5.4", "/lab/Renoise/V3.5.3"]).await;
    golden("renoise-mcp.ndjson", &transcript);
}

/// Logic's resource is its bundle id; its status names the permission its bridge lacks, which the status tool passes
/// on. (The host never starts a bridge here: a discovery file always answers.)
#[tokio::test]
async fn logic_mcp_transcript() {
    let transcript = transcript_with(Daw::Logic, SimulatorConfig::logic, ["com.apple.logic10", "com.apple.mobilelogic"], "freeze").await;
    assert!(transcript.contains("\\\"permission\\\":null"), "{transcript}");
    golden("logic-mcp.ndjson", &transcript);
}

#[tokio::test]
async fn initialize_negotiates_the_protocol_version() {
    let home = tempfile::tempdir().unwrap();
    let mut session = Session::start(Daw::Reaper, home.path());
    let old = session
        .call(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{}}}))
        .await;
    assert_eq!(old["result"]["protocolVersion"], "2025-06-18");
    let future = session
        .call(json!({"jsonrpc":"2.0","id":"two","method":"initialize","params":{"protocolVersion":"2099-01-01","capabilities":{}}}))
        .await;
    assert_eq!(future["result"]["protocolVersion"], "2025-11-25");
    assert_eq!(future["id"], "two");
    let junk = session.call(json!({"jsonrpc":"1.0","id":3,"method":"ping"})).await;
    assert_eq!(junk["error"]["code"], -32600);
    session.finish().await;
}

#[tokio::test]
async fn a_bridge_without_a_pid_is_checked_by_connecting() {
    let home = tempfile::tempdir().unwrap();
    let paths = BridgePaths::new(home.path(), Daw::Renoise);
    let secret = ensure_secret(&paths.secret()).unwrap();
    let mut config = SimulatorConfig::renoise(secret, "C:\\Users\\kumi\\AppData\\Roaming\\Renoise\\V3.5.4");
    config.identity.pid = 0;
    config.discovery_dir = Some(paths.discovery());
    let simulator = Simulator::start(config).await.unwrap();
    let mut session = Session::start(Daw::Renoise, home.path());
    let status = session.tool(1, "status", json!({})).await;
    assert_eq!((status["connected"].clone(), status["pid"].clone()), (json!(true), json!(0)));

    // Gone without removing its file (a crash): the port refuses, so the file is stale.
    let mut stale = simulator.discovery();
    stale.resource = "C:\\elsewhere\\V3.5.4".into();
    stale.port = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap().local_addr().unwrap().port();
    std::fs::write(paths.discovery().join(stale.file_name()), serde_json::to_string(&stale).unwrap()).unwrap();
    let found = session.tool(2, "discover", json!({})).await;
    assert_eq!(found["staleRemoved"], 1);
    assert_eq!(found["instances"].as_array().unwrap().len(), 1);
    assert!(!paths.discovery().join(stale.file_name()).exists());
    simulator.stop();
    session.finish().await;
}

/// What the bridge says as it happens (a track inserted where refs are positions) reaches the MCP client as
/// `notifications/daw_event`, between the answers.
#[tokio::test]
async fn the_bridges_events_reach_the_client_as_notifications() {
    let home = tempfile::tempdir().unwrap();
    let paths = BridgePaths::new(home.path(), Daw::Renoise);
    let secret = ensure_secret(&paths.secret()).unwrap();
    let mut config = SimulatorConfig::renoise(secret, "/lab/Renoise/V3.5.4");
    config.discovery_dir = Some(paths.discovery());
    let simulator = Simulator::start(config).await.unwrap();
    let mut session = Session::start(Daw::Renoise, home.path());
    assert_eq!(session.tool(1, "status", json!({})).await["connected"], json!(true));
    simulator.emit("tracks", json!({"count": 8, "made": [1], "gone": [], "layout": 1}));
    let line = tokio::time::timeout(std::time::Duration::from_secs(10), session.output.next_line()).await.unwrap().unwrap().unwrap();
    let event: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(
        event,
        json!({"jsonrpc": "2.0", "method": "notifications/daw_event", "params": {"name": "tracks", "data": {"count": 8, "made": [1], "gone": [], "layout": 1}}})
    );
    simulator.stop();
    session.finish().await;
}
