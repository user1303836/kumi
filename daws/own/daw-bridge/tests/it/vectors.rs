//! The wire's conformance vectors in tests/vectors/: made here from this crate's code and compared with the files, so
//! any change to the wire shows up as a diff. KUMI_BLESS=1 writes them. The REAPER extension and the Renoise tool's
//! tests replay the same files.
use daw_bridge::auth::{hmac_sha256_hex, proof, proof_message, sha256_hex, Role, Secret};
use daw_bridge::files::{discovery_file_name, discovery_key};
use daw_bridge::frame::{Framer, MAX_PREAUTH_LINE_BYTES};
use daw_bridge::handshake::{CloseReason, Offer, ServerAction, ServerConnection};
use daw_bridge::wire::{Frame, Identity, Project, Response, MAX_JSON_DEPTH, MAX_PAYLOAD_DEPTH, PROTOCOL};
use serde_json::{json, Value};
use std::path::PathBuf;

const SECRET: &str = "kumi-test-secret-0123456789abcdefghijklmnopqrstuvwxyz";
const CLIENT_NONCE: &str = "0123456789abcdef0123456789abcdef";
const SERVER_NONCE: &str = "fedcba9876543210fedcba9876543210";
const SESSION: &str = "5e55105e55105e55105e55105e55105e";

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("vectors")
}

fn check(name: &str, made: Value) {
    let path = dir().join(name);
    let mut text = serde_json::to_string_pretty(&made).unwrap();
    text.push('\n');
    if std::env::var("KUMI_BLESS").is_ok_and(|value| value == "1") {
        std::fs::create_dir_all(dir()).unwrap();
        std::fs::write(&path, &text).unwrap();
        return;
    }
    let kept = std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("{} is missing; run with KUMI_BLESS=1", path.display()));
    assert_eq!(
        serde_json::from_str::<Value>(&kept).unwrap(),
        made,
        "{name} differs from the code; bless it if the wire changed on purpose"
    );
}

#[test]
fn crypto_vectors() {
    // RFC 4231 test cases 1-4, 6 and 7 (5 is truncated output), then lengths around SHA-256's padding boundaries.
    let rfc = [
        ("0b".repeat(20), hex::encode("Hi There"), "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"),
        (hex::encode("Jefe"), hex::encode("what do ya want for nothing?"), "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"),
        ("aa".repeat(20), "dd".repeat(50), "773ea91e36800e46854db8ebd09181a72959098b3ef8c122d9635514ced565fe"),
        ((1..=25u8).map(|b| format!("{b:02x}")).collect(), "cd".repeat(50), "82558a389a443c0ea4cc819899f2083a85f0faa3e578f8077a2e3ff46729665b"),
        (
            "aa".repeat(131),
            hex::encode("Test Using Larger Than Block-Size Key - Hash Key First"),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54",
        ),
        (
            "aa".repeat(131),
            hex::encode("This is a test using a larger than block-size key and a larger than block-size data. The key needs to be hashed before being used by the HMAC algorithm."),
            "9b09ffa71b942fcb27635fbcd5b0e944bfdc63644f0713938a7f51535c3a35e2",
        ),
    ];
    let mut hmac = vec![];
    for (key, data, want) in rfc {
        let made = hmac_sha256_hex(&hex::decode(&key).unwrap(), &hex::decode(&data).unwrap());
        assert_eq!(made, want);
        hmac.push(json!({"keyHex": key, "dataHex": data, "hmac": want}));
    }
    let sha = [
        ("", "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"),
        ("abc", "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"),
        ("abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq", "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"),
    ];
    let mut sha256 = vec![];
    for (text, want) in sha {
        assert_eq!(sha256_hex(text.as_bytes()), want);
        sha256.push(json!({"dataHex": hex::encode(text), "sha256": want}));
    }
    for length in [55, 56, 63, 64, 65, 119, 120, 127, 128, 1000] {
        let data: Vec<u8> = (0..length).map(|i| (i * 7 % 256) as u8).collect();
        sha256.push(json!({"dataHex": hex::encode(&data), "sha256": sha256_hex(&data)}));
        let key: Vec<u8> = (0..length).map(|i| (255 - i % 256) as u8).collect();
        hmac.push(json!({"keyHex": hex::encode(&key), "dataHex": hex::encode(&data), "hmac": hmac_sha256_hex(&key, &data)}));
    }
    check(
        "crypto.json",
        json!({"about": "SHA-256 and HMAC-SHA256 over hex bytes: RFC 4231 cases 1-4, 6, 7, and lengths around the padding boundaries.", "sha256": sha256, "hmac": hmac}),
    );
}

#[test]
fn handshake_vectors() {
    let secrets = [SECRET.to_string(), "!".repeat(32), format!("{}~", "Z".repeat(511))];
    let cases: Vec<Value> = secrets
        .iter()
        .enumerate()
        .map(|(index, text)| {
            let secret = Secret::new(text.clone()).unwrap();
            let client_nonce = if index == 0 { CLIENT_NONCE.to_string() } else { format!("{index:0>32}") };
            let server = proof(&secret, Role::Server, &client_nonce, SERVER_NONCE);
            let client = proof(&secret, Role::Client, &client_nonce, SERVER_NONCE);
            json!({
                "secret": text,
                "clientNonce": client_nonce,
                "serverNonce": SERVER_NONCE,
                "serverMessage": proof_message(Role::Server, &client_nonce, SERVER_NONCE),
                "serverProof": server,
                "clientMessage": proof_message(Role::Client, &client_nonce, SERVER_NONCE),
                "clientProof": client,
            })
        })
        .collect();
    let secret_files = json!([
        {"text": format!("{SECRET}\n"), "secret": SECRET},
        {"text": format!("{SECRET}\r\n"), "secret": SECRET},
        {"text": SECRET, "secret": SECRET},
        {"text": format!("{SECRET}\n\n"), "error": "bad-byte"},
        {"text": format!(" {SECRET}"), "error": "bad-byte"},
        {"text": format!("{}\n", "a".repeat(31)), "error": "too-short"},
        {"text": "a".repeat(513), "error": "too-long"},
        {"text": format!("{}\u{e9}", "a".repeat(32)), "error": "bad-byte"},
    ]);
    for case in secret_files.as_array().unwrap() {
        let read = Secret::from_file_text(case["text"].as_str().unwrap());
        match case.get("secret") {
            Some(want) => assert_eq!(read.unwrap().expose(), want),
            None => assert!(read.is_err()),
        }
    }
    check(
        "handshake.json",
        json!({
            "about": "Proofs are lowercase hex HMAC-SHA256(key = the secret's bytes, message = serverMessage or clientMessage). secretFiles: a secret file's text and the secret read from it, or why it is refused.",
            "protocol": PROTOCOL,
            "cases": cases,
            "secretFiles": secret_files,
        }),
    );
}

#[test]
fn framing_vectors() {
    let cases = [
        ("two lines in one read", 64, vec![b"{\"a\":1}\n{\"b\":2}\n".to_vec()]),
        ("a line split across reads", 64, vec![b"{\"a\"".to_vec(), b":1}".to_vec(), b"\n".to_vec()]),
        ("carriage return dropped, blank lines skipped", 64, vec![b"\n\r\n{\"a\":1}\r\n\n".to_vec()]),
        ("blank lines count toward the next line's limit", 8, vec![b"\n\r\n{\"a\":1}\n".to_vec()]),
        ("a line and the blank lines before it at exactly the limit", 8, vec![b"\n{\"a\":1}\n\n\n{}\n".to_vec()]),
        ("blank lines alone over the limit", 8, vec![b"\n\n\n\n".to_vec(), b"\n\n\n\n".to_vec(), b"\n".to_vec()]),
        ("blank lines and a held line over the limit", 8, vec![b"\n\n\n\n\n{\"a\":".to_vec()]),
        ("a line of exactly the limit", 8, vec![b"{\"a\":12}\n".to_vec()]),
        ("one byte over the limit with its newline", 8, vec![b"{\"a\":123}\n".to_vec()]),
        ("over the limit before any newline", 8, vec![b"{\"a\":1234".to_vec()]),
        ("the limit before any newline excludes a carriage return", 8, vec![b"{\"a\":12}\r".to_vec(), b"\n".to_vec()]),
        ("the limit excludes the carriage return", 8, vec![b"{\"a\":12}\r\n".to_vec()]),
        ("invalid UTF-8", 64, vec![b"{\"a\":\"\xff\"}\n".to_vec()]),
        ("UTF-8 split across reads", 64, vec![b"{\"a\":\"\xc3".to_vec(), b"\xa9\"}\n".to_vec()]),
        ("nothing after an error", 8, vec![b"{\"a\":123456}\n{}\n".to_vec()]),
    ];
    let made: Vec<Value> = cases
        .into_iter()
        .map(|(name, max, chunks)| {
            let mut framer = Framer::new(max);
            let mut expect = vec![];
            for chunk in &chunks {
                framer.feed(chunk);
                while let Some(line) = framer.next_line() {
                    match line {
                        Ok(line) => expect.push(json!({"line": line})),
                        Err(error) => {
                            expect.push(json!({"error": error.code()}));
                            break;
                        }
                    }
                }
                if expect.last().is_some_and(|last| last.get("error").is_some()) {
                    break;
                }
            }
            json!({"name": name, "maxBytes": max, "chunksHex": chunks.iter().map(hex::encode).collect::<Vec<_>>(), "expect": expect})
        })
        .collect();
    check(
        "framing.json",
        json!({
            "about": "Feed each chunk in order and take lines until none; an error ends the stream. maxBytes bounds a line without its newline (and a trailing carriage return) together with the blank lines skipped before it, their carriage returns and newlines included; blank lines alone, and a line not yet ended, are bounded the same way.",
            "cases": made,
        }),
    );
}

/// `levels` arrays inside one another, the innermost empty.
fn nest(levels: usize) -> String {
    format!("{}{}", "[".repeat(levels), "]".repeat(levels))
}

/// A request line whose params are `levels` objects inside one another.
fn request_nested(levels: usize) -> String {
    format!(
        "{{\"type\":\"request\",\"id\":1,\"method\":\"ping\",\"params\":{}{{}}{}}}\n",
        "{\"a\":".repeat(levels - 1),
        "}".repeat(levels - 1)
    )
}

#[test]
fn frame_vectors() {
    let request_with =
        |id: &str, deadline: &str| format!("{{\"type\":\"request\",\"id\":{id},\"method\":\"ping\",\"deadlineMs\":{deadline}}}");
    let welcome_with = |credit: &str| {
        format!("{{\"type\":\"welcome\",\"session\":\"{}\",\"daw\":\"renoise\",\"bridge\":\"0.1.0\",\"dawVersion\":\"3.5.4\",\"pid\":1,\"resource\":\"/r\",\"project\":null,\"maxFrameBytes\":67108864,\"heartbeatMs\":1000,\"creditBytes\":{credit}}}", "0".repeat(32))
    };
    let heartbeat = |ms: &str| format!("{{\"type\":\"heartbeat\",\"ms\":{ms}}}");
    let event = |data: &str| format!("{{\"type\":\"event\",\"seq\":1,\"name\":\"x\",\"data\":{data}}}");
    let lines = [
        ("an event whose data nests to the limit", event(&nest(MAX_PAYLOAD_DEPTH))),
        ("an event whose data nests one level past the limit", event(&nest(MAX_PAYLOAD_DEPTH + 1))),
        ("an event whose data nests past serde_json's own limit", event(&nest(300))),
        ("a request whose params nest to the limit", request_nested(MAX_PAYLOAD_DEPTH).trim_end().to_string()),
        ("a request whose params nest one level past the limit", request_nested(MAX_PAYLOAD_DEPTH + 1).trim_end().to_string()),
        ("brackets that never close", format!("{{\"type\":\"event\",\"data\":{}", "[".repeat(10_000))),
        // Numbers are judged by how they are written: a float, an exponent or -0 is not an unsigned integer.
        ("a request id written 1", request_with("1", "5")),
        ("a request id written 1.0", request_with("1.0", "5")),
        ("a request id written 1e0", request_with("1e0", "5")),
        ("a request id written 1E2", request_with("1E2", "5")),
        ("a request id written -0", request_with("-0", "5")),
        ("a request id of 2^53 - 1", request_with("9007199254740991", "5")),
        ("a request id of 2^53", request_with("9007199254740992", "5")),
        ("a deadline written 1.5", request_with("1", "1.5")),
        ("a deadline written 1e3", request_with("1", "1e3")),
        ("a deadline of 2^64 - 1", request_with("1", "18446744073709551615")),
        ("a deadline of 2^64", request_with("1", "18446744073709551616")),
        ("a deadline of null", request_with("1", "null")),
        ("a heartbeat of 0", heartbeat("0")),
        ("a heartbeat written -0", heartbeat("-0")),
        ("a heartbeat written 0.0", heartbeat("0.0")),
        ("a heartbeat out of range", heartbeat("1e400")),
        ("an event number written 2.0", "{\"type\":\"event\",\"seq\":2.0,\"name\":\"x\"}".to_string()),
        ("a credit of 65536", "{\"type\":\"credit\",\"bytes\":65536}".to_string()),
        ("a credit of 0", "{\"type\":\"credit\",\"bytes\":0}".to_string()),
        ("a credit written 1.0", "{\"type\":\"credit\",\"bytes\":1.0}".to_string()),
        ("a credit of 2^53", "{\"type\":\"credit\",\"bytes\":9007199254740992}".to_string()),
        ("a credit without bytes", "{\"type\":\"credit\"}".to_string()),
        ("a welcome naming creditBytes", welcome_with("65536")),
        ("a welcome naming creditBytes written 1.5", welcome_with("1.5")),
        ("a name with a surrogate pair", "{\"type\":\"event\",\"seq\":1,\"name\":\"\\ud83d\\ude00\"}".to_string()),
        ("a name with a lone high surrogate", "{\"type\":\"event\",\"seq\":1,\"name\":\"\\ud800x\"}".to_string()),
        ("a name with a lone low surrogate", "{\"type\":\"event\",\"seq\":1,\"name\":\"\\udc00\"}".to_string()),
    ];
    let parse: Vec<Value> =
        lines.iter().map(|(name, line)| json!({"name": name, "line": line, "ok": Frame::parse(line).is_ok()})).collect();
    // A response whose result is `depth` arrays inside one another: sent as it is, or answered with an error.
    let encode: Vec<Value> = [1, 2, MAX_PAYLOAD_DEPTH - 1, MAX_PAYLOAD_DEPTH, MAX_PAYLOAD_DEPTH + 1, 100]
        .into_iter()
        .map(|depth| {
            let result = serde_json::from_str::<Value>(&nest(depth)).unwrap();
            let sent = matches!(Frame::ok(1, result), Frame::Response(Response { ok: true, .. }));
            json!({"resultDepth": depth, "sent": sent})
        })
        .collect();
    check(
        "frames.json",
        json!({
            "about": "Frame limits and grammar. maxJsonDepth: a frame nests at most this deep, its own object counting as the first level, on every reader and writer. parse: Frame::parse on line accepts it (ok) or refuses it; an unsigned integer member (id, seq, ms, deadlineMs, pid, maxFrameBytes, heartbeatMs) is judged by the text it was written as, and a surrogate escape must be half of a pair. encode: a response whose result is resultDepth arrays inside one another is sent (sent), or answered with an error response instead; a bridge never writes a line it would refuse to read.",
            "maxJsonDepth": MAX_JSON_DEPTH,
            "parse": parse,
            "encode": encode,
        }),
    );
}

fn identity() -> (Identity, Offer) {
    (
        Identity {
            preset_files: Vec::new(),
            daw: "reaper".into(),
            bridge: "0.1.0".into(),
            daw_version: "7.82/macOS-arm64".into(),
            pid: 4242,
            resource: "/Users/kumi/Library/Application Support/REAPER".into(),
            project: Some(Project { name: Some("Song.rpp".into()), path: Some("/Users/kumi/Music/Song.rpp".into()), ..Project::default() }),
        },
        Offer::default(),
    )
}

fn hello(protocol: &str, nonce: &str) -> String {
    format!("{{\"type\":\"hello\",\"protocol\":\"{protocol}\",\"nonce\":\"{nonce}\",\"client\":\"daw-mcp-server/0.1.0\"}}\n")
}

fn auth_line(role_secret: &str) -> String {
    let secret = Secret::new(role_secret).unwrap();
    format!("{{\"type\":\"auth\",\"proof\":\"{}\"}}\n", proof(&secret, Role::Client, CLIENT_NONCE, SERVER_NONCE))
}

/// Replays `steps` (bytes in, or a tick) against a server and records what it does.
fn run(steps: &[Step]) -> Vec<Value> {
    let secret = Secret::new(SECRET).unwrap();
    let mut server = ServerConnection::with_nonces(secret, SERVER_NONCE.into(), SESSION.into(), 0);
    let mut out = vec![];
    let mut clock = 0;
    for step in steps {
        let (input, actions) = match step {
            Step::Bytes(bytes) => (json!({"inHex": hex::encode(bytes)}), server.on_bytes(bytes, clock, &mut identity)),
            Step::BytesAt(ms, bytes) => {
                clock = *ms;
                (json!({"inHex": hex::encode(bytes), "atMs": ms}), server.on_bytes(bytes, clock, &mut identity))
            }
            Step::Tick(ms) => {
                clock = *ms;
                (json!({"tickMs": ms}), server.on_tick(clock))
            }
        };
        let expect: Vec<Value> = actions
            .into_iter()
            .map(|action| match action {
                ServerAction::Send(bytes) => {
                    let line = String::from_utf8(bytes).unwrap();
                    json!({"send": serde_json::from_str::<Value>(line.trim_end()).unwrap()})
                }
                ServerAction::Close(reason) => json!({"close": match reason {
                    CloseReason::NotJson | CloseReason::Http => "silent".to_string(),
                    CloseReason::Refused(reason) => format!("refused:{reason}"),
                    CloseReason::Protocol(_) => "protocol".to_string(),
                }}),
                ServerAction::Authenticated => json!({"authenticated": true}),
                ServerAction::Request(request) => json!({"request": serde_json::to_value(request).unwrap()}),
            })
            .collect();
        let mut row = input;
        row["expect"] = expect.into();
        out.push(row);
    }
    out
}

enum Step {
    Bytes(Vec<u8>),
    /// Bytes read at a later time.
    BytesAt(u64, Vec<u8>),
    Tick(u64),
}

fn bytes(text: impl AsRef<[u8]>) -> Step {
    Step::Bytes(text.as_ref().to_vec())
}

fn bytes_at(ms: u64, text: impl AsRef<[u8]>) -> Step {
    Step::BytesAt(ms, text.as_ref().to_vec())
}

#[test]
fn server_vectors() {
    let good_hello = hello(PROTOCOL, CLIENT_NONCE);
    let cases: Vec<(&str, Vec<Step>)> = vec![
        ("the full handshake, then a request, an unknown frame and a ping", vec![
            bytes(&good_hello),
            bytes(auth_line(SECRET)),
            bytes("{\"type\":\"request\",\"id\":1,\"method\":\"status\",\"params\":{}}\n{\"type\":\"later\"}\n{\"type\":\"request\",\"id\":2,\"method\":\"ping\"}\n"),
        ]),
        ("the hello split across reads", vec![bytes(&good_hello[..10]), bytes(&good_hello[10..])]),
        ("an HTTP request is closed without a reply", vec![bytes("GET / HTTP/1.1\r\nHost: 127.0.0.1:9770\r\n\r\n")]),
        ("a WebSocket upgrade is closed without a reply", vec![bytes("GET /ws HTTP/1.1\r\nUpgrade: websocket\r\n\r\n")]),
        ("the HTTP/2 preface is closed without a reply", vec![bytes("PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n")]),
        ("a byte order mark first is closed without a reply", vec![bytes(format!("\u{feff}{good_hello}"))]),
        ("whitespace first is closed without a reply", vec![bytes(format!(" {good_hello}"))]),
        ("a line over 1024 bytes before auth", vec![bytes(format!("{{\"type\":\"hello\",\"pad\":\"{}\"}}\n", "x".repeat(MAX_PREAUTH_LINE_BYTES)))]),
        ("1025 bytes before auth without a newline", vec![bytes(format!("{{{}", "x".repeat(MAX_PREAUTH_LINE_BYTES)))]),
        ("another protocol", vec![bytes(hello("kumi-daw/2", CLIENT_NONCE))]),
        ("an uppercase nonce", vec![bytes(hello(PROTOCOL, &CLIENT_NONCE.to_uppercase()))]),
        ("a short nonce", vec![bytes(hello(PROTOCOL, &CLIENT_NONCE[1..]))]),
        ("not JSON after the first byte", vec![bytes("{hello}\n")]),
        ("a JSON object without a type", vec![bytes("{\"protocol\":\"kumi-daw/1\"}\n")]),
        ("a request before auth", vec![bytes("{\"type\":\"request\",\"id\":1,\"method\":\"status\"}\n")]),
        ("a second hello instead of auth", vec![bytes(&good_hello), bytes(&good_hello)]),
        ("a wrong proof", vec![bytes(&good_hello), bytes(auth_line(&"w".repeat(40)))]),
        ("the server's own proof sent back", vec![
            bytes(&good_hello),
            bytes(format!(
                "{{\"type\":\"auth\",\"proof\":\"{}\"}}\n",
                proof(&Secret::new(SECRET).unwrap(), Role::Server, CLIENT_NONCE, SERVER_NONCE)
            )),
        ]),
        ("an uppercase proof", vec![bytes(&good_hello), bytes(auth_line(SECRET).to_uppercase().replace("TYPE", "type").replace("AUTH", "auth").replace("PROOF", "proof"))]),
        ("the handshake timeout", vec![bytes(&good_hello), Step::Tick(2999), Step::Tick(3000)]),
        ("bytes that keep coming don't stretch the handshake", vec![
            bytes(&good_hello),
            bytes_at(1000, "\n"),
            bytes_at(2000, "\n"),
            bytes_at(2999, "\n"),
            bytes_at(3000, "\n"),
        ]),
        ("blank lines before auth count toward the line limit", vec![
            bytes(&good_hello),
            bytes("\n".repeat(MAX_PREAUTH_LINE_BYTES)),
            bytes("\n"),
        ]),
        ("blank lines and the auth after them over the line limit", vec![
            bytes(&good_hello),
            bytes(format!("{}{}", "\r\n".repeat(470), auth_line(SECRET))),
        ]),
        ("after auth a line that isn't JSON closes without a reply", vec![bytes(&good_hello), bytes(auth_line(SECRET)), bytes("nope\n")]),
        ("after auth invalid UTF-8 closes without a reply", vec![bytes(&good_hello), bytes(auth_line(SECRET)), bytes(b"{\"a\":\"\xff\"}\n")]),
        ("after auth a request with id 0 closes", vec![bytes(&good_hello), bytes(auth_line(SECRET)), bytes("{\"type\":\"request\",\"id\":0,\"method\":\"ping\"}\n")]),
        ("after auth no timeout", vec![bytes(&good_hello), bytes(auth_line(SECRET)), Step::Tick(60_000)]),
    ];
    let made: Vec<Value> = cases.into_iter().map(|(name, steps)| json!({"name": name, "steps": run(&steps)})).collect();
    check(
        "server.json",
        json!({
            "about": "A bridge's side of a connection. Accept it at time 0 with the secret, serverNonce and session below; for each step feed inHex at the clock's time (first moving the clock to atMs when given), or move the clock to tickMs and run the timer, and expect, in order: send (compare as JSON; for a welcome compare type, session, maxFrameBytes and heartbeatMs, the rest is the bridge's own), close (silent: no reply; refused:<reason>: after the refusal; protocol: a bad line after auth, no reply), authenticated, request (the request to answer). Nothing is read after a close. overCapacity: what a bridge sends, before reading, to a connection over its limits, then closes.",
            "secret": SECRET,
            "serverNonce": SERVER_NONCE,
            "session": SESSION,
            "welcomeIdentity": serde_json::to_value(identity().0).unwrap(),
            "overCapacity": serde_json::from_slice::<Value>(&daw_bridge::handshake::refuse_connection()).unwrap(),
            "cases": made,
        }),
    );
}

#[test]
fn discovery_vectors() {
    let resources = [
        "/Users/kumi/Library/Application Support/REAPER",
        "C:\\Users\\kumi\\AppData\\Roaming\\REAPER",
        "/home/kumi/.config/Renoise/V3.5.4",
        "/Users/kumi/Library/Preferences/Renoise/V3.5.4",
        "C:\\Users\\Kümi\\AppData\\Roaming\\Renoise\\V3.5.4",
        "/Volumes/Lab/REAPER portable",
    ];
    let cases: Vec<Value> = resources
        .iter()
        .enumerate()
        .map(|(index, resource)| {
            let daw = if resource.contains("Renoise") { "renoise" } else { "reaper" };
            // Renoise on Windows can't learn its pid: 0.
            let pid = if index == 4 { 0 } else { 1000 + index as u32 };
            json!({"daw": daw, "pid": pid, "resource": resource, "key": discovery_key(resource), "fileName": discovery_file_name(daw, pid, resource)})
        })
        .collect();
    check(
        "discovery.json",
        json!({
            "about": "key = the first 16 lowercase hex of SHA-256(the resource path's UTF-8 bytes, exactly as given); file name = <daw>-<pid>-<key>.json, pid 0 when the bridge can't learn it.",
            "cases": cases,
        }),
    );
}
