//! daw-bridge's conformance vectors, replayed through what this bridge uses of the wire: SHA-256 and HMAC (project
//! ids, discovery keys, the proofs), the framer every connection reads with, the frame grammar, and discovery file
//! names. The server's side of a connection is replayed over real sockets in `src/server.rs`'s tests.

use daw_bridge::auth::{hmac_sha256_hex, sha256_hex};
use daw_bridge::files::{discovery_file_name, discovery_key};
use daw_bridge::frame::Framer;
use daw_bridge::wire::{Frame, Response, MAX_JSON_DEPTH};
use serde_json::{json, Value};

fn vectors(name: &str) -> Value {
    let path = kumi_testkit::workspace_path("crates/daw-bridge/tests/vectors").join(name);
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("no vectors at {}", path.display()))).unwrap()
}

#[test]
fn crypto_vectors_hold_for_the_hashes_the_bridge_uses() {
    let v = vectors("crypto.json");
    let (sha256, hmac) = (v["sha256"].as_array().unwrap(), v["hmac"].as_array().unwrap());
    assert!(!sha256.is_empty() && !hmac.is_empty());
    for case in sha256 {
        assert_eq!(sha256_hex(&hex::decode(case["dataHex"].as_str().unwrap()).unwrap()), case["sha256"]);
    }
    for case in hmac {
        let key = hex::decode(case["keyHex"].as_str().unwrap()).unwrap();
        let data = hex::decode(case["dataHex"].as_str().unwrap()).unwrap();
        assert_eq!(hmac_sha256_hex(&key, &data), case["hmac"]);
    }
    // A project id is the first 32 hex of the same hash.
    assert_eq!(logic_kumi::logic::project_id("/m/Song.logicx"), sha256_hex(b"logic-project:/m/Song.logicx")[..32]);
}

#[test]
fn framing_vectors_hold_for_the_framer_the_connections_read_with() {
    for case in vectors("framing.json")["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let mut framer = Framer::new(case["maxBytes"].as_u64().unwrap() as usize);
        let mut got = vec![];
        'chunks: for chunk in case["chunksHex"].as_array().unwrap() {
            framer.feed(&hex::decode(chunk.as_str().unwrap()).unwrap());
            while let Some(line) = framer.next_line() {
                match line {
                    Ok(line) => got.push(json!({"line": line})),
                    Err(error) => {
                        got.push(json!({"error": error.code()}));
                        break 'chunks;
                    }
                }
            }
        }
        assert_eq!(Value::Array(got), case["expect"], "{name}");
    }
}

#[test]
fn frame_vectors_hold_for_the_frames_the_bridge_reads_and_writes() {
    let v = vectors("frames.json");
    assert_eq!(v["maxJsonDepth"], MAX_JSON_DEPTH);
    for case in v["parse"].as_array().unwrap() {
        assert_eq!(Frame::parse(case["line"].as_str().unwrap()).is_ok(), case["ok"], "{}", case["name"]);
    }
    for case in v["encode"].as_array().unwrap() {
        let depth = case["resultDepth"].as_u64().unwrap() as usize;
        let result: Value = serde_json::from_str(&format!("{}{}", "[".repeat(depth), "]".repeat(depth))).unwrap();
        let sent = matches!(Frame::ok(1, result), Frame::Response(Response { ok: true, .. }));
        assert_eq!(sent, case["sent"], "a result {depth} deep");
    }
}

#[test]
fn discovery_vectors_hold_for_the_file_names_the_bridge_writes() {
    for case in vectors("discovery.json")["cases"].as_array().unwrap() {
        let (daw, resource) = (case["daw"].as_str().unwrap(), case["resource"].as_str().unwrap());
        assert_eq!(discovery_key(resource), case["key"]);
        assert_eq!(discovery_file_name(daw, case["pid"].as_u64().unwrap() as u32, resource), case["fileName"]);
    }
    // This bridge's file is keyed by Logic's bundle identifier, its resource.
    let key = &sha256_hex(b"com.apple.logic10")[..16];
    assert_eq!(discovery_file_name("logic", 4242, "com.apple.logic10"), format!("logic-4242-{key}.json"));
}
