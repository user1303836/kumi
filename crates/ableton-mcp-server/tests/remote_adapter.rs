use ableton_mcp_server::{
    bridge::remote_adapter::*,
    live::*,
    registry::{canonical_json, WIRE_CANONICAL_LIMITS},
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use hmac::{Hmac, Mac};
use kumi_common::{abort::Signal, time::now_ms};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
    task::LocalSet,
};
const SECRET: &str = "0123456789abcdef0123456789abcdef";
const EPOCH: &str = "bridge-epoch-0123456789abcdef";
const CHALLENGE: &str = "connection-challenge-0123456789abcdef";
fn signed(mut value: Value) -> Value {
    let mut mac = Hmac::<Sha256>::new_from_slice(SECRET.as_bytes()).unwrap();
    mac.update(canonical_json(&value, &WIRE_CANONICAL_LIMITS).unwrap().as_bytes());
    value["mac"] = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes()).into();
    value
}
fn response(id: &str, result: Value, ok: bool, epoch: &str) -> Value {
    let mut value = json!({"version":"ableton-loopback/v1","id":id,"ok":ok,"bridgeEpoch":epoch,"connectionChallenge":CHALLENGE});
    value[if ok { "result" } else { "error" }] = result;
    signed(value)
}
fn hello(epoch: &str) -> Value {
    response("hello", json!({"protocol":"ableton-live/v1","registryHash":*LIVE_REGISTRY_HASH,"maxDeadlineMs":60000}), true, epoch)
}
fn status(operations: &[&str]) -> Value {
    let mut ops = vec!["status", "snapshot", "discover", "get", "reconnect", "session.playback"];
    ops.extend(operations);
    json!({"connected":true,"adapter":"remote-script","epoch":1,"protocol":"ableton-live/v1","capabilities":[],"registryHash":*LIVE_REGISTRY_HASH,"operations":ops,"provenance":"fake-live"})
}
#[derive(Clone)]
enum Reply {
    Value(Value),
    Error(&'static str),
    Drop,
    Silent,
    Raw(Value),
    Later(u64, Value),
    Events(Value, Vec<Value>),
}
struct Server {
    endpoint: RemoteScriptEndpoint,
    seen: Rc<RefCell<Vec<Value>>>,
    stop: Signal,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}
impl Server {
    async fn start(operations: Vec<&'static str>, answer: impl Fn(&Value, usize) -> Reply + 'static) -> Self {
        Self::configured(operations, answer, |_| hello(EPOCH), |_, s| s).await
    }
    async fn configured(
        operations: Vec<&'static str>,
        answer: impl Fn(&Value, usize) -> Reply + 'static,
        greet: impl Fn(usize) -> Value + 'static,
        adjust: impl Fn(usize, Value) -> Value + 'static,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = Rc::new(RefCell::new(vec![]));
        let stop = Signal::new();
        let observed = seen.clone();
        let cancelled = stop.clone();
        let answer = Rc::new(answer);
        let greet = Rc::new(greet);
        let adjust = Rc::new(adjust);
        tokio::task::spawn_local(async move {
            let mut generation = 0;
            loop {
                let (stream, _) = tokio::select! {_=cancelled.cancelled()=>break,result=listener.accept()=>result.unwrap()};
                generation += 1;
                let connection = generation;
                let answer = answer.clone();
                let seen = observed.clone();
                let mut greeting = greet(connection);
                let delay = greeting.as_object_mut().unwrap().remove("$delay").and_then(|v| v.as_u64()).unwrap_or(0);
                let epoch = greeting["bridgeEpoch"].as_str().unwrap_or(EPOCH).to_owned();
                let adjusted = adjust(connection, status(&operations));
                let cancel = cancelled.clone();
                tokio::task::spawn_local(async move {
                    let (reader, mut writer) = stream.into_split();
                    if delay > 0 {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                    if writer.write_all(format!("{greeting}\n").as_bytes()).await.is_err() {
                        return;
                    }
                    let mut lines = BufReader::new(reader).lines();
                    loop {
                        let line = tokio::select! {_=cancel.cancelled()=>break,line=lines.next_line()=>line};
                        let Ok(Some(line)) = line else { break };
                        let request: Value = serde_json::from_str(&line).unwrap();
                        let mut unsigned = request.clone();
                        let supplied = unsigned.as_object_mut().unwrap().remove("mac").unwrap();
                        assert_eq!(signed(unsigned)["mac"], supplied, "native frame must authenticate with canonical JS JSON");
                        seen.borrow_mut().push(request.clone());
                        let reply =
                            if request["method"] == "status" { Reply::Value(adjusted.clone()) } else { answer(&request, connection) };
                        let id = request["id"].as_str().unwrap();
                        let (frame, events) = match reply {
                            Reply::Value(v) => (response(id, v, true, &epoch), vec![]),
                            Reply::Error(e) => (response(id, json!(e), false, &epoch), vec![]),
                            Reply::Drop => break,
                            Reply::Silent => continue,
                            Reply::Raw(v) => (v, vec![]),
                            Reply::Later(ms, v) => {
                                tokio::time::sleep(Duration::from_millis(ms)).await;
                                (response(id, v, true, &epoch), vec![])
                            }
                            Reply::Events(v, events) => (response(id, v, true, &epoch), events),
                        };
                        if writer.write_all(format!("{frame}\n").as_bytes()).await.is_err() {
                            break;
                        }
                        for event in events {
                            let frame = response("event", json!({"event":event}), true, &epoch);
                            if writer.write_all(format!("{frame}\n").as_bytes()).await.is_err() {
                                break;
                            }
                        }
                    }
                });
            }
        });
        let mut endpoint = RemoteScriptEndpoint::new("127.0.0.1", port, SECRET);
        endpoint.timeout_ms = Some(500.0);
        Self { endpoint, seen, stop }
    }
    async fn connect(&self) -> RemoteScriptLiveAdapter {
        RemoteScriptLiveAdapter::connect(self.endpoint.clone()).await.unwrap()
    }
}
fn context(transaction: &str) -> LiveOperationContext {
    LiveOperationContext {
        deadline_ms: Some(now_ms() as f64 + 5000.0),
        transaction_id: Some(transaction.into()),
        idempotency_key: Some("stable-operation-key".into()),
        ..Default::default()
    }
}
fn authority(request: &Value) -> Option<Value> {
    let digest = hex::encode(Sha256::digest(canonical_json(&request["args"], &WIRE_CANONICAL_LIMITS).unwrap()));
    match request["method"].as_str() {
        Some("preflight") => Some(
            json!({"preflightToken":"p".repeat(32),"confirmation":"c".repeat(32),"operation":request["operation"],"argsDigest":digest,"stateDigest":"a".repeat(64),"impact":"mutates-live","expiresAt":now_ms()+5000}),
        ),
        Some("prepare") => Some(
            json!({"authorityToken":"t".repeat(32),"operation":request["operation"],"argsDigest":digest,"stateDigest":"a".repeat(64),"expiresAt":now_ms()+5000}),
        ),
        _ => None,
    }
}
fn create_track() -> LiveInvocation {
    LiveInvocation::new("track.create", json!({"name":"Owned","kind":"midi","index":1,"expectedStructureRevision":"a".repeat(64)}))
}
fn track_result() -> Value {
    json!({"ref":"1:track:1","objectIdentity":"live:track:1","name":"Owned","kind":"midi","index":1,"createdFingerprint":"f".repeat(64),"ownershipToken":"o".repeat(48)})
}
fn delete_track() -> LiveInvocation {
    LiveInvocation::new(
        "track.delete",
        json!({"ref":"1:track:1","expectedStructureRevision":"b".repeat(64),"expectedObjectIdentity":"live:track:1"}),
    )
}

#[test]
fn digest_reference_fields_and_pad_expansion() {
    assert_eq!(
        digest_references(&json!({"ref":"a","nested":{"deviceRef":"b","clipRefs":["c","a"]},"other":[{"ref":"d"}],"reference":"ignore"}))
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>(),
        ["a", "b", "c", "d"].into_iter().map(str::to_owned).collect()
    );
    let full = json!({"objectIdentity":"live:kick","devices":[{"name":"Simpler"}]});
    let mut rack = json!({"chains":[full],"drumPads":[{"chains":[{"objectIdentity":"live:kick","listedOnRack":true}]},{"chains":[{"objectIdentity":"live:gone","listedOnRack":true}]}]});
    expand_pad_chains(&mut rack);
    assert_eq!(rack["drumPads"][0]["chains"][0], rack["chains"][0]);
    assert_eq!(rack["drumPads"][1]["chains"][0]["devices"], json!([]));
}
#[test]
fn authority_classifications_match_python() {
    let python = include_str!("../../../remote-script/ableton_mcp_remote_script.py");
    for (name, values) in [
        ("READ_ONLY_INVOKES", READ_ONLY_INVOKES),
        ("AUTHORITY_FREE_INVOKES", AUTHORITY_FREE_INVOKES),
        ("TRANSACTION_CREATIONS", TRANSACTION_CREATIONS),
        ("TRANSACTION_DELETIONS", TRANSACTION_DELETIONS),
        ("EXPLICIT_DELETIONS", EXPLICIT_DELETIONS),
    ] {
        let quoted = regex::Regex::new(r#""([^"]+)""#).unwrap();
        let py = regex::Regex::new(&format!(r"_{name} = \{{([^}}]*)\}}")).unwrap().captures(python).unwrap();
        let set = |text: &str| quoted.captures_iter(text).map(|m| m[1].to_owned()).collect::<std::collections::BTreeSet<_>>();
        let native = values.iter().map(|s| s.to_string()).collect();
        assert_eq!(set(&py[1]), native, "{name}");
    }
}
#[tokio::test]
async fn endpoint_fail_closed_before_network() {
    LocalSet::new()
        .run_until(async {
            for (host, port, secret) in [
                ("192.168.1.10", 9000., SECRET),
                ("127.999.0.1", 9000., SECRET),
                ("127.0.0.2", 9000., SECRET),
                ("127.0.0.1", 9000., "short"),
                ("127.0.0.1", 0., SECRET),
                ("localhost", 9000., SECRET),
                ("::1", 1.5, SECRET),
            ] {
                let mut endpoint = RemoteScriptEndpoint::new(host, 9000, secret);
                endpoint.port = port;
                assert!(RemoteScriptLiveAdapter::connect(endpoint).await.err().unwrap().message().contains("loopback"));
            }
        })
        .await;
}
#[tokio::test]
async fn authenticated_hello_and_registry_negotiation() {
    LocalSet::new()
        .run_until(async {
            let forged = Server::configured(
                vec![],
                |_, _| Reply::Silent,
                |_| {
                    let mut h = hello(EPOCH);
                    h["connectionChallenge"] = "forged-connection-challenge".into();
                    h
                },
                |_, s| s,
            )
            .await;
            assert!(RemoteScriptLiveAdapter::connect(forged.endpoint.clone()).await.err().unwrap().message().contains("authentication"));
            // A Remote Script of another bridge version (Live left running through an update): restart Live, which
            // loads the one installed. Its hello says so, and so would its status.
            let older = Server::configured(
                vec![],
                |_, _| Reply::Silent,
                |_| {
                    response(
                        "hello",
                        json!({"protocol":"ableton-live/v1","registryHash":"0".repeat(64),"maxDeadlineMs":60000}),
                        true,
                        EPOCH,
                    )
                },
                |_, s| s,
            )
            .await;
            let refused = RemoteScriptLiveAdapter::connect(older.endpoint.clone()).await.err().unwrap();
            assert_eq!(refused.message(), ableton_mcp_server::bridge::remote_adapter::ANOTHER_BRIDGE);
            assert!(refused.message().contains("restart Live"));
            for change in [
                json!({"epoch":-1,"capabilities":[42]}),
                json!({"capabilities":["session.write"]}),
                json!({"capabilities":["max"]}),
                json!({"operations":["forged.operation"]}),
            ] {
                let peer = Server::configured(
                    vec![],
                    |_, _| Reply::Silent,
                    |_| hello(EPOCH),
                    move |_, mut s| {
                        s.as_object_mut().unwrap().extend(change.as_object().unwrap().clone());
                        s
                    },
                )
                .await;
                assert!(RemoteScriptLiveAdapter::connect(peer.endpoint.clone()).await.is_err());
            }
            let peer = Server::configured(
                vec!["locator.add", "locator.delete"],
                |_, _| Reply::Silent,
                |_| hello(EPOCH),
                |_, mut s| {
                    s["capabilities"] = json!(["arrangement.read", "arrangement.write"]);
                    s
                },
            )
            .await;
            let adapter = peer.connect().await;
            assert!(adapter.status().unwrap().connected);
            adapter.close().await.unwrap();
        })
        .await;
}
#[tokio::test]
async fn discovery_translates_every_kind_and_validates_args() {
    LocalSet::new().run_until(async{
 let peer=Server::start(vec![],|r,_|Reply::Value(json!({"epoch":1,"items":[],"truncated":false,"revision":"1:revision","kind":r["args"]["kind"]}))).await;let adapter=peer.connect().await;
 for kind in LiveDiscoveryKind::ALL.iter().filter(|kind|**kind!=LiveDiscoveryKind::SessionPlayback){let mut request=LiveDiscoveryRequest::of(*kind);request.parent=Some("set:one".into());request.filter=Some(json!({"name":"A"}).as_object().unwrap().clone());request.fields=Some(vec!["name".into()]);request.budget=Some(50);request.limit=Some(4);request.cursor=Some("cursor".into());let result=adapter.discover_async(&request,None).await.unwrap();assert_eq!(result.kind,*kind);assert_eq!(peer.seen.borrow().last().unwrap()["args"],json!({"kind":kind.as_str().replace('-',"_"),"parent":"set:one","filters":{"name":"A"},"requestedFields":["name"],"traversalBudget":50,"limit":4,"cursor":"cursor"}));}
 let before=peer.seen.borrow().len();let mut bad=LiveDiscoveryRequest::of(LiveDiscoveryKind::Track);bad.limit=Some(0);assert!(adapter.discover_async(&bad,None).await.is_err());assert_eq!(peer.seen.borrow().len(),before);adapter.close().await.unwrap();
}).await;
}
#[tokio::test]
async fn authority_retry_and_stable_idempotency() {
    LocalSet::new()
        .run_until(async {
            let invokes = Rc::new(Cell::new(0));
            let count = invokes.clone();
            let mut peer = Server::start(vec!["track.create"], move |r, _| {
                if let Some(v) = authority(r) {
                    return Reply::Value(v);
                }
                count.set(count.get() + 1);
                if count.get() == 1 {
                    Reply::Error("request failed: playhead is moving; retry shortly")
                } else {
                    Reply::Value(track_result())
                }
            })
            .await;
            peer.endpoint.mutation_path = Some(MutationPath::Authority);
            let adapter = peer.connect().await;
            let invocation = create_track();
            for tx in ["same-transaction", "same-transaction", "different-transaction"] {
                let result = adapter.invoke_async(&invocation, Some(&context(tx))).await.unwrap();
                assert!(result.get("ownershipToken").is_none());
            }
            let seen = peer.seen.borrow();
            let keys: Vec<_> = seen.iter().filter(|r| r["method"] == "prepare").map(|r| r["idempotencyKey"].clone()).collect();
            assert_eq!(keys.len(), 4);
            assert_eq!(keys[0], keys[1]);
            assert_eq!(keys[1], keys[2]);
            assert_ne!(keys[2], keys[3]);
            drop(seen);
            adapter.close().await.unwrap();
        })
        .await;
}
#[tokio::test]
async fn creation_tokens_retirement_and_owned_cleanup() {
    LocalSet::new()
        .run_until(async {
            let peer = Server::start(vec!["track.create", "track.delete"], |r, _| {
                if r["method"] == "retire" {
                    Reply::Value(json!({"retired":1}))
                } else if r["operation"] == "track.create" {
                    Reply::Value(track_result())
                } else {
                    assert_eq!(r["ownershipToken"], "o".repeat(48));
                    Reply::Value(json!({"deleted":"1:track:1"}))
                }
            })
            .await;
            let adapter = peer.connect().await;
            let ctx = context("creating-transaction");
            let result = adapter.invoke_async(&create_track(), Some(&ctx)).await.unwrap();
            assert!(result.get("ownershipToken").is_none());
            adapter.retire_transaction_async("creating-transaction", None, false).await.unwrap();
            let before = peer.seen.borrow().len();
            assert!(matches!(
                adapter.invoke_async(&delete_track(), Some(&context("foreign-transaction"))).await,
                Err(LiveError::MutationNotDispatched(_))
            ));
            assert_eq!(peer.seen.borrow().len(), before);
            assert_eq!(adapter.invoke_async(&delete_track(), Some(&ctx)).await.unwrap(), json!({"deleted":"1:track:1"}));
            assert_eq!(peer.seen.borrow().last().unwrap()["method"], "mutate");
            assert!(matches!(adapter.invoke_async(&delete_track(), Some(&ctx)).await, Err(LiveError::MutationNotDispatched(_))));
            adapter.close().await.unwrap();
        })
        .await;
}
#[tokio::test]
async fn explicit_deletion_and_bad_mutation_refused_before_dispatch() {
    LocalSet::new()
        .run_until(async {
            let peer = Server::start(vec!["track.delete", "track.create"], |r, _| {
                assert!(r.get("ownershipToken").is_none());
                Reply::Value(json!({"deleted":"1:track:1"}))
            })
            .await;
            let adapter = peer.connect().await;
            let before = peer.seen.borrow().len();
            let mut invalid = create_track();
            invalid.args.insert("kind".into(), "invalid".into());
            assert!(matches!(adapter.invoke_async(&invalid, None).await, Err(LiveError::MutationNotDispatched(_))));
            assert_eq!(peer.seen.borrow().len(), before);
            let mut delete = delete_track();
            delete.args.insert("explicitDeletion".into(), true.into());
            adapter.invoke_async(&delete, None).await.unwrap();
            adapter.close().await.unwrap();
        })
        .await;
}
#[tokio::test]
async fn cancelled_before_dispatch_vs_uncertain_after_dispatch() {
    LocalSet::new()
        .run_until(async {
            let peer = Server::start(vec![], |_, _| Reply::Silent).await;
            let adapter = peer.connect().await;
            let signal = Signal::new();
            signal.cancel();
            let ctx = LiveOperationContext { signal: Some(signal), ..Default::default() };
            let before = peer.seen.borrow().len();
            let reference = LiveRef::from("1:track:0");
            assert!(adapter.get_async(&reference, Some(&ctx)).await.unwrap_err().message().contains("before dispatch"));
            assert_eq!(peer.seen.borrow().len(), before);
            let signal = Signal::new();
            let abort = signal.clone();
            tokio::task::spawn_local(async move {
                tokio::time::sleep(Duration::from_millis(15)).await;
                abort.cancel();
            });
            let ctx = LiveOperationContext { signal: Some(signal), ..Default::default() };
            assert!(adapter
                .get_async(&reference, Some(&ctx))
                .await
                .unwrap_err()
                .message()
                .contains("uncertain after dispatch cancellation"));
            assert!(!adapter.status().unwrap().connected);
            adapter.close().await.unwrap();
        })
        .await;
}
#[tokio::test]
async fn timeout_closes_connection_and_explicit_deadline_extends_default() {
    LocalSet::new()
        .run_until(async {
            let mut peer = Server::start(vec![], |_, _| Reply::Later(160, json!({"ref":"1:track:0","name":"Track"}))).await;
            peer.endpoint.timeout_ms = Some(100.0);
            let adapter = peer.connect().await;
            let reference = LiveRef::from("1:track:0");
            assert_eq!(
                adapter.get_async(&reference, Some(&LiveOperationContext::with_deadline(now_ms() as f64 + 500.0))).await.unwrap(),
                Some(json!({"ref":"1:track:0","name":"Track"}))
            );
            assert!(adapter.get_async(&reference, None).await.unwrap_err().message().contains("uncertain after dispatch timeout"));
            assert!(!adapter.status().unwrap().connected);
            adapter.close().await.unwrap();
        })
        .await;
}
#[tokio::test]
async fn same_epoch_recovery_replays_stable_mutation_key() {
    LocalSet::new()
        .run_until(async {
            let dropped = Rc::new(Cell::new(false));
            let seen_drop = dropped.clone();
            let peer =
                Server::start(
                    vec!["track.create"],
                    move |_, _| if !seen_drop.replace(true) { Reply::Drop } else { Reply::Value(track_result()) },
                )
                .await;
            let adapter = peer.connect().await;
            let ctx = context("replayed-transaction");
            assert!(adapter.invoke_async(&create_track(), Some(&ctx)).await.is_err());
            adapter.invoke_async(&create_track(), Some(&ctx)).await.unwrap();
            let seen = peer.seen.borrow();
            let keys: Vec<_> = seen.iter().filter(|r| r["method"] == "mutate").map(|r| r["idempotencyKey"].clone()).collect();
            assert_eq!(keys.len(), 2);
            assert_eq!(keys[0], keys[1]);
            drop(seen);
            adapter.close().await.unwrap();
        })
        .await;
}
#[tokio::test]
async fn changed_bridge_or_live_epoch_poisons_reconciliation() {
    LocalSet::new()
        .run_until(async {
            for change_bridge in [false, true] {
                let peer = Server::configured(
                    vec![],
                    |_, _| Reply::Drop,
                    move |connection| hello(if change_bridge && connection > 1 { "replacement-bridge-0123456789" } else { EPOCH }),
                    move |connection, mut s| {
                        if !change_bridge && connection > 1 {
                            s["epoch"] = 2.into();
                        }
                        s
                    },
                )
                .await;
                let adapter = peer.connect().await;
                let reference = LiveRef::from("1:track:0");
                assert!(adapter.get_async(&reference, None).await.is_err());
                assert!(adapter.get_async(&reference, None).await.unwrap_err().message().contains("epoch changed"));
                let before = peer.seen.borrow().len();
                assert!(adapter.get_async(&reference, None).await.unwrap_err().message().contains("poisoned"));
                assert_eq!(peer.seen.borrow().len(), before);
                adapter.close().await.unwrap();
            }
        })
        .await;
}
#[tokio::test]
async fn unknown_signed_response_and_event_sequence_fail_closed() {
    LocalSet::new()
        .run_until(async {
            let peer = Server::start(vec![], |_, _| Reply::Raw(response("never-sent", json!({}), true, EPOCH))).await;
            let adapter = peer.connect().await;
            assert!(adapter.get_async(&LiveRef::from("1:track:0"), None).await.unwrap_err().message().contains("unknown or duplicate"));
            assert!(!adapter.status().unwrap().connected);
            adapter.close().await.unwrap();
            for sequences in [vec![1, 3], vec![1, 1]] {
                let peer = Server::start(vec!["subscribe"], move |_, _| {
                    Reply::Events(
                        json!({"subscribed":true,"subscriptionId":"subscription-one"}),
                        sequences.iter().map(|sequence| json!({"epoch":1,"sequence":sequence,"type":"object","payload":{}})).collect(),
                    )
                })
                .await;
                let adapter = peer.connect().await;
                let events = Rc::new(RefCell::new(vec![]));
                let observed = events.clone();
                let _off = adapter.subscribe(Rc::new(move |e| observed.borrow_mut().push(e.clone()))).unwrap();
                let disconnected = Signal::new();
                let on_disconnect = disconnected.clone();
                let _off_status = adapter.subscribe_status(Rc::new(move |status| {
                    if status.is_some_and(|status| !status.connected) {
                        on_disconnect.cancel();
                    }
                }));
                let _ = adapter.invoke_async(&LiveInvocation::new("subscribe", json!({"types":["object"]})), None).await;
                // The subscribe acknowledgement can arrive before either event. Wait for the
                // invalid event's disconnect, which follows delivery of the valid first event.
                tokio::time::timeout(Duration::from_millis(500), disconnected.cancelled())
                    .await
                    .expect("invalid event sequence must disconnect the adapter");
                assert_eq!(events.borrow().len(), 1);
                assert!(!adapter.status().unwrap().connected);
                adapter.close().await.unwrap();
            }
        })
        .await;
}
#[tokio::test]
async fn state_digest_consumed_once_and_old_transactions_retired() {
    LocalSet::new()
        .run_until(async {
            let mut peer = Server::start(vec!["track.create", "authority.digest"], |r, _| {
                if r["method"] == "retire" {
                    Reply::Value(json!({"retired":1}))
                } else if r["operation"] == "authority.digest" {
                    Reply::Value(json!({"stateDigest":"a".repeat(64),"epoch":1}))
                } else {
                    Reply::Value(track_result())
                }
            })
            .await;
            peer.endpoint.retire_after = Some(2);
            let adapter = peer.connect().await;
            let invocation = create_track();
            adapter.expect_state_digest("first-transaction", &invocation);
            adapter.invoke_async(&invocation, Some(&context("first-transaction"))).await.unwrap();
            adapter.invoke_async(&invocation, Some(&context("first-transaction"))).await.unwrap();
            adapter.invoke_async(&invocation, Some(&context("second-transaction"))).await.unwrap();
            adapter.invoke_async(&invocation, Some(&context("third-transaction"))).await.unwrap();
            tokio::time::sleep(Duration::from_millis(20)).await;
            let seen = peer.seen.borrow();
            let mutations: Vec<_> = seen.iter().filter(|r| r["method"] == "mutate").collect();
            assert_eq!(mutations[0]["stateDigest"], "a".repeat(64));
            assert!(mutations[1].get("stateDigest").is_none());
            let retired: Vec<_> = seen.iter().filter(|r| r["method"] == "retire").map(|r| r["transactionId"].clone()).collect();
            assert_eq!(retired, json!(["first-transaction", "second-transaction"]).as_array().unwrap().clone());
            drop(seen);
            adapter.close().await.unwrap();
        })
        .await;
}
#[tokio::test]
async fn reconnect_callers_keep_independent_deadlines_and_cancellation() {
    LocalSet::new()
        .run_until(async {
            let peer = Server::configured(
                vec![],
                |_, _| Reply::Drop,
                |generation| {
                    let mut value = hello(EPOCH);
                    if generation > 1 {
                        value["$delay"] = 150.into();
                    }
                    value
                },
                |_, s| s,
            )
            .await;
            let adapter = peer.connect().await;
            assert!(adapter.get_async(&LiveRef::from("1:track:0"), None).await.is_err());
            let short = LiveOperationContext::with_deadline(now_ms() as f64 + 40.0);
            let long = LiveOperationContext::with_deadline(now_ms() as f64 + 1000.0);
            let (a, b) = tokio::join!(adapter.refresh_status_async(Some(&short)), adapter.refresh_status_async(Some(&long)));
            assert!(a.unwrap_err().message().contains("deadline"));
            assert!(b.unwrap().connected);
            assert_eq!(peer.seen.borrow().iter().filter(|r| r["method"] == "status").count(), 3);
            adapter.close().await.unwrap();
        })
        .await;
}
#[tokio::test]
async fn subscriptions_restore_with_reset_and_invalid_retry_preserves_sequence() {
    LocalSet::new()
        .run_until(async {
            let subscriptions = Rc::new(Cell::new(0));
            let seen = subscriptions.clone();
            let peer = Server::start(vec!["subscribe"], move |r, _| {
                if r["method"] == "subscribe" {
                    seen.set(seen.get() + 1);
                    Reply::Events(
                        json!({"subscribed":true,"subscriptionId":"one"}),
                        vec![json!({"epoch":1,"sequence":1,"type":"reset","payload":{"resnapshot":true}})],
                    )
                } else {
                    Reply::Drop
                }
            })
            .await;
            let adapter = peer.connect().await;
            let events = Rc::new(RefCell::new(vec![]));
            let observed = events.clone();
            let (event_sent, mut event_received) = tokio::sync::mpsc::unbounded_channel();
            let _off = adapter
                .subscribe(Rc::new(move |e| {
                    observed.borrow_mut().push(e.clone());
                    event_sent.send(()).unwrap();
                }))
                .unwrap();
            let invoke = LiveInvocation::new("subscribe", json!({"types":["transport"]}));
            adapter.invoke_async(&invoke, None).await.unwrap();
            tokio::time::timeout(Duration::from_millis(500), event_received.recv()).await.unwrap().unwrap();
            assert!(adapter.invoke_async(&invoke, Some(&LiveOperationContext::with_deadline(now_ms() as f64 - 1.0))).await.is_err());
            assert_eq!(subscriptions.get(), 1);
            assert!(adapter.get_async(&LiveRef::from("1:track:0"), None).await.is_err());
            adapter.refresh_status_async(None).await.unwrap();
            tokio::time::timeout(Duration::from_millis(500), event_received.recv()).await.unwrap().unwrap();
            assert_eq!(subscriptions.get(), 2);
            assert_eq!(events.borrow().iter().map(|e| e.sequence).collect::<Vec<_>>(), vec![1, 1]);
            adapter.close().await.unwrap();
        })
        .await;
}
#[tokio::test]
async fn canonical_maximum_note_batch_and_registry_result_failure() {
    LocalSet::new().run_until(async{
 let peer=Server::start(vec!["note.add-batch"],|_,_|Reply::Value(json!({"added":512,"noteIds":(0..512).collect::<Vec<_>>(),"notesRevision":"b".repeat(64)}))).await;let adapter=peer.connect().await;let notes:Vec<_>=(0..512).map(|i|json!({"pitch":i%128,"start":i,"duration":0.25,"velocity":100,"channel":1})).collect();let args=json!({"ref":"1:clip:0:0","notes":notes,"expectedClipAuthority":{"expectedObjectIdentity":"live:clip:0","expectedTrackRef":"1:track:0","expectedTrackIdentity":"live:track:0","expectedSlotRef":"1:clip_slot:0:0","expectedSlotIdentity":"live:slot:0","expectedSceneRef":"1:scene:0","expectedSceneIdentity":"live:scene:0"},"expectedNotesRevision":"a".repeat(64)});let result=adapter.invoke_async(&LiveInvocation::new("note.add-batch",args),Some(&context("maximum-note-batch-transaction"))).await.unwrap();assert_eq!(result["added"],512);assert_eq!(result["noteIds"].as_array().unwrap().len(),512);adapter.close().await.unwrap();
 let peer=Server::start(vec![],|_,_|Reply::Value(json!({"missing":"ref"}))).await;let adapter=peer.connect().await;assert!(adapter.get_async(&LiveRef::from("1:track:0"),None).await.unwrap_err().message().contains("registry"));assert!(!adapter.status().unwrap().connected);adapter.close().await.unwrap();
}).await;
}
#[tokio::test]
async fn snapshot_windows_and_large_read_deadline_factor() {
    LocalSet::new()
        .run_until(async {
            let mut peer = Server::start(vec![], |request, _| {
                if request["method"] == "snapshot" {
                    let simulator = DeterministicLiveSimulator::new();
                    let request: LiveSnapshotRequest = serde_json::from_value(request.get("args").cloned().unwrap_or(json!({}))).unwrap();
                    let result = futures::executor::block_on(simulator.snapshot_async(None, Some(&request))).unwrap();
                    let mut result = serde_json::to_value(result).unwrap();
                    result.as_object_mut().unwrap().retain(|key, _| {
                        [
                            "set",
                            "tracks",
                            "scenes",
                            "arrangement",
                            "playback",
                            "browser",
                            "epoch",
                            "selected",
                            "selection",
                            "trackCount",
                            "sceneCount",
                            "window",
                        ]
                        .contains(&key.as_str())
                    });
                    Reply::Later(160, result)
                } else {
                    Reply::Later(160, json!({"ref":"1:track:0"}))
                }
            })
            .await;
            peer.endpoint.timeout_ms = Some(100.0);
            let adapter = peer.connect().await;
            let request = LiveSnapshotRequest { focus: Some(vec![0]), parts: Some(vec![LiveSnapshotPart::Tracks]), ..Default::default() };
            let snapshot = adapter.snapshot_async(None, Some(&request)).await.unwrap();
            assert_eq!(snapshot.window, Some(request));
            assert!(snapshot.set.is_none());
            adapter.snapshot_async(None, None).await.unwrap();
            assert!(peer.seen.borrow().last().unwrap().get("args").is_none());
            let before = peer.seen.borrow().len();
            assert!(adapter.snapshot_async(None, Some(&LiveSnapshotRequest::track_window(0, 0))).await.is_err());
            assert_eq!(peer.seen.borrow().len(), before);
            assert!(adapter.get_async(&LiveRef::from("1:track:0"), None).await.unwrap_err().message().contains("timeout"));
            adapter.close().await.unwrap();
        })
        .await;
}
#[tokio::test]
async fn refusal_classification_and_terminal_retirement_removes_cleanup() {
    LocalSet::new()
        .run_until(async {
            let peer = Server::start(vec!["track.create", "track.delete"], |r, _| {
                if r["method"] == "retire" {
                    assert_eq!(r["terminal"], true);
                    Reply::Value(json!({"retired":1}))
                } else if r["operation"] == "track.create" {
                    Reply::Value(track_result())
                } else {
                    Reply::Error("Live state changed since the preview")
                }
            })
            .await;
            let adapter = peer.connect().await;
            let ctx = context("terminal-transaction");
            adapter.invoke_async(&create_track(), Some(&ctx)).await.unwrap();
            assert!(matches!(adapter.invoke_async(&delete_track(), Some(&ctx)).await, Err(LiveError::MutationNotDispatched(_))));
            adapter.retire_transaction_async("terminal-transaction", None, true).await.unwrap();
            let before = peer.seen.borrow().len();
            assert!(matches!(adapter.invoke_async(&delete_track(), Some(&ctx)).await, Err(LiveError::MutationNotDispatched(_))));
            assert_eq!(peer.seen.borrow().len(), before);
            adapter.close().await.unwrap();
            assert!(adapter.refresh_status_async(None).await.unwrap_err().message().contains("closed"));
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn delivery_probe_preserves_fake_real_and_missing_discovery_evidence() {
    use ableton_mcp_server::delivery::*;
    LocalSet::new()
        .run_until(async {
            for (provenance, empty_scenes, expected_authenticated, expected_live) in
                [("fake-live", false, true, false), ("real-live", false, true, true), ("real-live", true, false, false)]
            {
                let server = Server::configured(
                    vec![],
                    move |request, _| {
                        if request["method"] == "discover" {
                            let kind = request["args"]["kind"].clone();
                            if kind == "session_playback" { return Reply::Value(json!({"ref":"live:1:set:1","epoch":1,"revision":"1","transport":{"playing":false,"arrangementRecord":false,"sessionRecord":false,"position":0,"launchQuantization":{"raw":0,"normalized":"none"},"loop":{"enabled":false,"start":0,"length":4},"punchIn":false,"punchOut":false,"metronome":false,"countIn":0},"firedTargets":[],"playingTargets":[]})); }
                            let items = if kind == "scene" && !empty_scenes {
                                json!([{"ref":"live:1:scene:1"}])
                            } else if kind == "track" {
                                json!([{"ref":"live:1:track:1"}])
                            } else {
                                json!([])
                            };
                            Reply::Value(json!({"epoch":1,"kind":kind,"items":items,"truncated":false,"revision":"1"}))
                        } else {
                            Reply::Value(json!({}))
                        }
                    },
                    |_| hello(EPOCH),
                    move |_, mut status| {
                        status["provenance"] = provenance.into();
                        status
                    },
                )
                .await;
                let folder = tempfile::tempdir().unwrap();
                let secret = folder.path().join("secret");
                write_secret_file(&secret, Some(SECRET)).unwrap();
                let path = folder.path().join("config.json");
                let entry = native_entrypoint(folder.path());
                let config = config_for_bridge(
                    &entry,
                    &json!({"host":"127.0.0.1","port":server.endpoint.port,"secretFile":secret,"timeoutMs":1000}),
                    None,
                    Some(&path),
                    true,
                )
                .unwrap();
                write_config(&path, &config, false).unwrap();
                let report = diagnostics_async(Some(folder.path()), Some(&path)).await;
                assert_eq!(report["authenticatedReachable"], expected_authenticated, "{report}");
                assert_eq!(report["liveConnected"], expected_live, "{report}");
                assert_eq!(report["ready"], false, "missing package must never be ready");
                if expected_authenticated {
                    assert_eq!(report["provenance"], provenance);
                    assert_eq!(report["discoveryKinds"], json!(["set", "scene", "track", "session-playback", "clip-slot"]));
                    assert_eq!(report["readiness"]["releaseCertified"], false);
                } else {
                    assert_eq!(report["diagnosticErrors"], json!(["authenticated-bridge-probe-failed"]));
                }
                let requests = server.seen.borrow();
                let discoveries: Vec<_> = requests.iter().filter(|r| r["method"] == "discover").collect();
                assert_eq!(discoveries.len(), 5);
                assert!(discoveries.iter().all(|r| r["args"]["limit"] == 16 && r["args"]["traversalBudget"] == 256));
                assert_eq!(discoveries[4]["args"]["parent"], "live:1:track:1");
            }
        })
        .await;
}
