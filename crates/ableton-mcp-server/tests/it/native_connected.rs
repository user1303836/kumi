//! Exercise the shipped stdio executable through an authenticated loopback peer.
//! The peer is explicitly fake-live and delegates state/authority checks to the simulator.
use ableton_mcp_server::{
    delivery::{config_for_bridge, read_any_config, write_config, write_secret_file},
    live::*,
    registry::{canonical_json, WIRE_CANONICAL_LIMITS},
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::Sha256;
use std::{cell::RefCell, collections::HashMap, path::Path, process::Stdio, rc::Rc, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, Lines},
    net::TcpListener,
    process::{Child, ChildStderr, ChildStdin, ChildStdout, Command},
};

const SECRET: &str = "0123456789abcdef0123456789abcdef";
const EPOCH: &str = "native-process-fixture-bridge-epoch";
const CHALLENGE: &str = "native-process-fixture-connection-challenge";
fn signed(mut value: Value) -> Value {
    let mut mac = Hmac::<Sha256>::new_from_slice(SECRET.as_bytes()).unwrap();
    mac.update(canonical_json(&value, &WIRE_CANONICAL_LIMITS).unwrap().as_bytes());
    value["mac"] = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes()).into();
    value
}
fn response(id: &str, result: Result<Value, LiveError>) -> Value {
    let mut value =
        json!({"version":"ableton-loopback/v1","id":id,"ok":result.is_ok(),"bridgeEpoch":EPOCH,"connectionChallenge":CHALLENGE});
    match result {
        Ok(result) => value["result"] = result,
        Err(error) => value["error"] = json!(error.message()),
    }
    signed(value)
}
async fn answer(sim: &DeterministicLiveSimulator, request: &Value, ledger: &mut HashMap<String, Value>) -> Result<Value, LiveError> {
    Ok(match request["method"].as_str().unwrap() {
        "status" => {
            let mut status = serde_json::to_value(sim.status()?).unwrap();
            status["adapter"] = json!("remote-script");
            status["registryHash"] = json!(*LIVE_REGISTRY_HASH);
            status["provenance"] = json!("fake-live");
            status
        }
        "snapshot" => {
            let requested = request.get("args").map(|args| serde_json::from_value::<LiveSnapshotRequest>(args.clone()).unwrap());
            let mut snapshot = serde_json::to_value(sim.snapshot_async(None, requested.as_ref()).await?).unwrap();
            // LiveObjectMapper._snapshot_rows emits these Remote Script parts. The simulator
            // also carries extension-only state which is not part of the loopback snapshot.
            snapshot.as_object_mut().unwrap().retain(|key, _| {
                ["set", "tracks", "scenes", "arrangement", "playback", "selection", "epoch", "trackCount", "sceneCount", "window"]
                    .contains(&key.as_str())
            });
            snapshot
        }
        "discover" => {
            let mut args = request["args"].clone();
            args["kind"] = json!(args["kind"].as_str().unwrap().replace('_', "-"));
            for (wire, native) in [("filters", "filter"), ("requestedFields", "fields"), ("traversalBudget", "budget")] {
                if let Some(value) = args.as_object_mut().unwrap().remove(wire) {
                    args[native] = value;
                }
            }
            let discovery: LiveDiscoveryRequest = serde_json::from_value(args).unwrap();
            let result = sim.discover_async(&discovery, None).await?;
            if discovery.kind == LiveDiscoveryKind::SessionPlayback {
                json!(result.items[0])
            } else {
                let mut result = serde_json::to_value(result).unwrap();
                result["kind"] = json!(discovery.kind.as_str().replace('-', "_"));
                result
            }
        }
        "get" => sim.get(&LiveRef::from(request["ref"].as_str().unwrap()))?.unwrap_or(Value::Null),
        "invoke" => sim.invoke(&LiveInvocation::new(request["operation"].as_str().unwrap(), request["args"].clone()))?,
        "mutate" => {
            let key = canonical_json(&json!([request["transactionId"], request["idempotencyKey"]]), &WIRE_CANONICAL_LIMITS).unwrap();
            let identity = json!([request["operation"], request["args"]]);
            if let Some(prior) = ledger.get(&key) {
                assert_eq!(prior["identity"], identity, "a retained mutation key cannot change authority");
                prior["result"].clone()
            } else {
                assert!(request["transactionId"].as_str().is_some_and(|s| s.len() >= 8));
                assert!(request["idempotencyKey"].as_str().is_some_and(|s| s.len() >= 8));
                let result = sim.invoke(&LiveInvocation::new(request["operation"].as_str().unwrap(), request["args"].clone()))?;
                ledger.insert(key, json!({"identity":identity,"result":result}));
                result
            }
        }
        "retire" => json!({"retired":0}),
        other => panic!("unexpected bridge method: {other}"),
    })
}

struct Client {
    input: ChildStdin,
    output: Lines<BufReader<ChildStdout>>,
    errors: Option<ChildStderr>,
    process: Option<Child>,
    modern: bool,
    sequence: u64,
}
impl Client {
    async fn send(&mut self, request: &Value) {
        self.input.write_all(format!("{request}\n").as_bytes()).await.unwrap();
        self.input.flush().await.unwrap();
    }
    async fn call(&mut self, method: &str, mut params: Value) -> Value {
        self.sequence += 1;
        let id = self.sequence;
        if self.modern {
            params["_meta"] =
                json!({"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}});
        }
        self.send(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})).await;
        loop {
            let line =
                tokio::time::timeout(Duration::from_secs(15), self.output.next_line()).await.expect("native response deadline").unwrap();
            let Some(line) = line else {
                let mut diagnostics = String::new();
                tokio::time::timeout(Duration::from_secs(5), self.errors.as_mut().unwrap().read_to_string(&mut diagnostics))
                    .await
                    .expect("native diagnostic deadline")
                    .unwrap();
                let status = tokio::time::timeout(Duration::from_secs(5), self.process.as_mut().unwrap().wait())
                    .await
                    .expect("native exit deadline")
                    .unwrap();
                let tool = params.get("name").and_then(Value::as_str).unwrap_or("none");
                panic!("native stdout ended before response to {method} request {id}, tool {tool} ({status}): {diagnostics}");
            };
            let response: Value = serde_json::from_str(&line).expect("stdout must contain only MCP JSON");
            if response.get("id").is_none() {
                assert!(response["method"].is_string(), "{response}");
                continue;
            }
            assert_eq!(response["id"], id, "{response}");
            assert!(response.get("error").is_none(), "{response}");
            return response["result"].clone();
        }
    }
    async fn tool(&mut self, name: &str, arguments: Value) -> Value {
        let result = self.call("tools/call", json!({"name":name,"arguments":arguments})).await;
        assert_eq!(result["isError"], false, "{name}: {result}");
        serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap()
    }
}

#[tokio::test(flavor = "current_thread")]
async fn native_process_reads_mutates_replays_and_undoes_through_authenticated_tcp() {
    tokio::task::LocalSet::new().run_until(async {
        for modern in [false, true] {
            let folder = tempfile::tempdir().unwrap();
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            let sim = Rc::new(DeterministicLiveSimulator::new());
            let requests = Rc::new(RefCell::new(vec![]));
            let peer_sim = sim.clone();
            let seen = requests.clone();
            let peer = tokio::task::spawn_local(async move {
                let (stream, _) = listener.accept().await.unwrap();
                let (reader, mut writer) = stream.into_split();
                let hello = response("hello", Ok(json!({"protocol":"ableton-live/v1","registryHash":*LIVE_REGISTRY_HASH,"maxDeadlineMs":60000})));
                writer.write_all(format!("{hello}\n").as_bytes()).await.unwrap();
                let mut input = BufReader::new(reader).lines();
                let mut ledger = HashMap::new();
                let mut sequence = 0;
                while let Some(line) = input.next_line().await.unwrap() {
                    let request: Value = serde_json::from_str(&line).unwrap();
                    let mut unsigned = request.clone();
                    let supplied = unsigned.as_object_mut().unwrap().remove("mac").unwrap();
                    assert_eq!(signed(unsigned)["mac"], supplied, "request HMAC");
                    sequence += 1;
                    assert_eq!(request["sequence"], sequence);
                    assert_eq!(request["bridgeEpoch"], EPOCH);
                    assert_eq!(request["connectionChallenge"], CHALLENGE);
                    seen.borrow_mut().push(request.clone());
                    let result = answer(&peer_sim, &request, &mut ledger).await;
                    let reply = response(request["id"].as_str().unwrap(), result);
                    writer.write_all(format!("{reply}\n").as_bytes()).await.unwrap();
                }
            });
            let secret = folder.path().join("bridge.secret");
            write_secret_file(&secret, Some(SECRET)).unwrap();
            let config_path = folder.path().join("bridge.json");
            let config = config_for_bridge(Path::new(env!("CARGO_BIN_EXE_ableton-mcp-server")), &json!({"host":"127.0.0.1","port":port,"secretFile":secret,"timeoutMs":5000}), None, Some(&config_path), true).unwrap();
            write_config(&config_path, &config, false).unwrap();
            read_any_config(&config_path).expect("the fixture must establish a readable owner-only bridge configuration");
            let mut process = Command::new(env!("CARGO_BIN_EXE_ableton-mcp-server"))
                .args(["--config", config_path.to_str().unwrap()])
                .current_dir(folder.path()).env("ABLETON_MCP_EXTENSION", "off")
                .env_remove("ABLETON_MCP_TOOL_POLICY").env_remove("ABLETON_MCP_TOOL_ALLOW").env_remove("ABLETON_MCP_TOOL_DENY")
                .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
                .kill_on_drop(true).spawn().unwrap();
            let mut client = Client { input:process.stdin.take().unwrap(), output:BufReader::new(process.stdout.take().unwrap()).lines(), errors:process.stderr.take(), process:Some(process), modern, sequence:0 };
            if !modern {
                let initialized = client.call("initialize", json!({"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"native-wire-test","version":"1"}})).await;
                assert_eq!(initialized["protocolVersion"], "2025-11-25");
                client.send(&json!({"jsonrpc":"2.0","method":"notifications/initialized"})).await;
            }
            let status = client.tool("live_status", json!({})).await;
            assert_eq!(status["connected"], true);
            assert_eq!(status["adapter"], "remote-script");
            assert_eq!(status["provenance"], "fake-live");
            let tools = client.call("tools/list", json!({})).await;
            for name in ["live_snapshot", "live_discover", "live_tempo_preview", "live_tempo_apply", "live_change", "live_undo"] {
                assert!(tools["tools"].as_array().unwrap().iter().any(|tool| tool["name"] == name), "missing {name}");
            }
            let snapshot = client.tool("live_snapshot", json!({})).await;
            assert_eq!(snapshot["snapshot"]["set"]["tempo"], 120);
            let tracks = client.tool("live_discover", json!({"kind":"track","limit":1,"budget":1000})).await;
            assert_eq!(tracks["items"][0]["ref"], "track:track-1");
            let preview = client.tool("live_tempo_preview", json!({"tempo":124})).await;
            let transaction = preview["transactionId"].clone();
            assert_eq!(preview["priorTempo"], 120);
            assert_eq!(sim.state.borrow()["set"]["tempo"].as_f64(), Some(120.0));
            let apply = json!({"transactionId":transaction,"confirmation":"apply","idempotencyKey":"native-tempo-apply"});
            assert_eq!(client.tool("live_tempo_apply", apply.clone()).await["state"], "applied");
            assert_eq!(sim.state.borrow()["set"]["tempo"].as_f64(), Some(124.0));
            assert_eq!(client.tool("live_tempo_apply", apply).await["idempotent"], true);
            let undo = json!({"transactionId":transaction,"confirmation":"undo","idempotencyKey":"native-tempo-undo"});
            assert_eq!(client.tool("live_undo", undo.clone()).await["state"], "undone");
            assert_eq!(client.tool("live_undo", undo).await["idempotent"], true);
            assert_eq!(sim.state.borrow()["set"]["tempo"].as_f64(), Some(120.0));
            let change = json!({"tool":"live_device_parameter_preview","args":{"deviceRef":"device:utility-1","parameterRef":"parameter:gain-1","value":0.7},"idempotencyKey":"native-parameter-change"});
            let changed = client.tool("live_change", change.clone()).await;
            assert_eq!(changed["state"], "applied");
            assert_eq!(changed["change"]["preview"], "live_device_parameter_preview");
            assert_eq!(sim.state.borrow()["tracks"][0]["devices"][0]["parameters"][0]["value"].as_f64(), Some(f64::from(0.7_f32)));
            assert_eq!(client.tool("live_change", change).await["idempotent"], true);
            let undo = json!({"transactionId":changed["transactionId"],"confirmation":"undo","idempotencyKey":"native-parameter-undo"});
            assert_eq!(client.tool("live_undo", undo).await["state"], "undone");
            assert_eq!(sim.state.borrow()["tracks"][0]["devices"][0]["parameters"][0]["value"], 0.5);
            let final_snapshot = client.tool("live_snapshot", json!({})).await;
            assert_eq!(final_snapshot["snapshot"]["set"]["tempo"], 120);
            assert_eq!(final_snapshot["snapshot"]["tracks"][0]["devices"][0]["parameters"][0]["value"], 0.5);
            let mut errors = client.errors.take().unwrap();
            let mut process = client.process.take().unwrap();
            drop(client);
            let output = tokio::time::timeout(Duration::from_secs(15), process.wait()).await.unwrap().unwrap();
            let mut diagnostics = String::new();
            errors.read_to_string(&mut diagnostics).await.unwrap();
            assert!(output.success(), "{diagnostics}");
            assert!(diagnostics.is_empty(), "{diagnostics}");
            tokio::time::timeout(Duration::from_secs(5), peer).await.unwrap().unwrap();
            let requests = requests.borrow();
            let mutations: Vec<_> = requests.iter().filter(|request| request["method"] == "mutate").collect();
            assert_eq!(mutations.iter().map(|request| &request["operation"]).collect::<Vec<_>>(), vec!["tempo.set", "tempo.set", "device.parameter.set", "device.parameter.set"], "replaying host calls must not repeat writes");
            assert_eq!(mutations[0]["args"]["expectedTempo"], 120);
            assert_eq!(mutations[1]["args"]["expectedTempo"], 124);
            assert_eq!(mutations[0]["args"]["expectedObjectIdentity"], "simulator:set:set-1");
            assert_eq!(mutations[0]["transactionId"], mutations[1]["transactionId"]);
            assert_ne!(mutations[0]["idempotencyKey"], mutations[1]["idempotencyKey"]);
            assert!(requests.iter().any(|request| request["method"] == "snapshot"));
            assert!(requests.iter().any(|request| request["method"] == "discover"));
            assert!(requests.iter().any(|request| request["method"] == "get"));
        }
    }).await;
}
