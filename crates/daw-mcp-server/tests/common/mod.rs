//! What the host's integration tests share: golden transcripts, and an MCP session on in-memory pipes.
#![allow(dead_code)]
use daw_bridge::Daw;
use daw_mcp_server::host::Host;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// Compares with tests/golden/<name>; KUMI_BLESS=1 rewrites it.
pub fn golden(name: &str, made: &str) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("golden").join(name);
    if std::env::var("KUMI_BLESS").is_ok_and(|value| value == "1") {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, made).unwrap();
        return;
    }
    let kept = std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("{} is missing; run with KUMI_BLESS=1", path.display()));
    assert_eq!(made, kept, "{name} changed; bless it if that's intended");
}

/// A running MCP host on in-memory pipes, with a transcript of the lines both ways.
pub struct Session {
    input: tokio::io::DuplexStream,
    output: tokio::io::Lines<BufReader<tokio::io::DuplexStream>>,
    pub transcript: String,
    replace: Vec<(String, String)>,
    served: tokio::task::JoinHandle<std::io::Result<()>>,
}

impl Session {
    pub fn start(daw: Daw, home: &Path) -> Self {
        Self::start_with(Arc::new(Host::new(daw, home)), home)
    }

    /// A session over a host the test made (one with a spawner of its own, say).
    pub fn start_with(host: Arc<Host>, home: &Path) -> Self {
        let (input, host_input) = tokio::io::duplex(1 << 22);
        let (host_output, output) = tokio::io::duplex(1 << 22);
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

    pub fn hide_port(&mut self, port: u16, name: &str) {
        self.replace.push((format!("\"port\":{port}"), format!("\"port\":\"{name}\"")));
        self.replace.push((format!("\\\"port\\\":{port}"), format!("\\\"port\\\":{name}")));
    }

    /// Sends a message and returns the answer (Null for a notification). The transcript keeps both, with `bulk` set
    /// (long answers cut to their size) when the answer is too long to be worth keeping whole.
    /// Sends a message without waiting for its answer.
    #[allow(dead_code)]
    pub async fn send(&mut self, message: Value) {
        self.input.write_all(format!("{message}\n").as_bytes()).await.unwrap();
    }

    pub async fn call(&mut self, message: Value) -> Value {
        self.call_as(message, false).await
    }

    pub async fn call_as(&mut self, message: Value, bulk: bool) -> Value {
        let line = message.to_string();
        self.input.write_all(format!("{line}\n").as_bytes()).await.unwrap();
        self.transcript.push_str(&format!("> {line}\n"));
        if message.get("id").is_none() {
            return Value::Null;
        }
        // (the bridge's events come between the answers, as notifications: kept in the transcript, not taken for the answer)
        let answer = loop {
            let line = tokio::time::timeout(std::time::Duration::from_secs(20), self.output.next_line()).await.unwrap().unwrap().unwrap();
            let parsed: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
            if parsed.get("id").is_none() && parsed.get("method").is_some() {
                self.transcript.push_str(&format!("< {line}\n"));
                continue;
            }
            break line;
        };
        let mut shown = answer.clone();
        for (text, name) in &self.replace {
            shown = shown.replace(text, name);
        }
        if bulk && shown.len() > 4000 {
            shown = format!("<{} bytes>", shown.len());
        }
        self.transcript.push_str(&format!("< {shown}\n"));
        serde_json::from_str(&answer).unwrap()
    }

    /// Calls a tool; its answer (`structuredContent`) and whether it is an error.
    pub async fn tool_full(&mut self, id: u64, name: &str, arguments: Value, bulk: bool) -> (Value, bool) {
        let answer =
            self.call_as(json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":arguments}}), bulk).await;
        (answer["result"]["structuredContent"].clone(), answer["result"]["isError"] == true)
    }

    pub async fn tool(&mut self, id: u64, name: &str, arguments: Value) -> Value {
        self.tool_full(id, name, arguments, false).await.0
    }

    pub async fn finish(self) -> String {
        drop(self.input);
        self.served.await.unwrap().unwrap();
        self.transcript
    }
}

/// A simulator's transcript with what changes from run to run (nonces, proofs, sessions, the pid wherever it sits,
/// deadlines) replaced by names, one line each: the connection, `>` from the host or `<` from the bridge, the frame.
pub fn normalized(lines: &[daw_mcp_server::simulator::Line]) -> String {
    let mut out = String::new();
    for line in lines {
        let mut value: Value = serde_json::from_str(&line.text).unwrap_or_else(|_| Value::String(line.text.clone()));
        // Credit frames are left out, like heartbeats: how many bytes a host grants depends on the receive buffer
        // its system gave it and on how the bytes arrived.
        if value.get("type").and_then(Value::as_str) == Some("credit") {
            continue;
        }
        if let Some(object) = value.as_object_mut() {
            for key in ["nonce", "proof", "session"] {
                if object.contains_key(key) {
                    object.insert(key.into(), format!("<{key}>").into());
                }
            }
            if object.contains_key("deadlineMs") {
                object.insert("deadlineMs".into(), "<deadline>".into());
            }
            // The host's own name in its hello, whose version moves with each release.
            if object.get("client").and_then(Value::as_str) == Some(concat!("daw-mcp-server/", env!("CARGO_PKG_VERSION"))) {
                object.insert("client".into(), "daw-mcp-server/<version>".into());
            }
        }
        hide_pids(&mut value);
        out.push_str(&format!("{} {} {}\n", line.connection, if line.from_host { ">" } else { "<" }, value));
    }
    out
}

/// Every `pid` in a value, at any depth (a welcome's, a status result's, Logic's nested identity's), made `<pid>`.
fn hide_pids(value: &mut Value) {
    match value {
        Value::Object(object) => {
            if object.contains_key("pid") {
                object.insert("pid".into(), "<pid>".into());
            }
            object.values_mut().for_each(hide_pids);
        }
        Value::Array(items) => items.iter_mut().for_each(hide_pids),
        _ => {}
    }
}
