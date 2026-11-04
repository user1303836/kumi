//! `kumi serve --stdio`, the binary as a host starts it: MCP on stdio, branching off before the app reads anything of
//! the producer's own Kumi.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

const WAIT: Duration = Duration::from_secs(20);

fn serve(args: &[&str], user: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_kumi"));
    command
        .arg("serve")
        .args(args)
        .env("HOME", user)
        .env("USERPROFILE", user)
        .env("KUMI_HOME", user.join("the-producers-kumi"))
        .env("KUMI_PROCESSES", "none")
        .env_remove("KUMI_NO_UPDATE_CHECK");
    command
}

struct Running {
    child: Child,
    to: Option<ChildStdin>,
    from: Receiver<Value>,
}

impl Running {
    fn start(mut command: Command) -> Running {
        let mut child = command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        let to = child.stdin.take();
        let stdout = child.stdout.take().unwrap();
        let (lines, from) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if lines.send(serde_json::from_str(&line).expect("each line one JSON message")).is_err() {
                    return;
                }
            }
        });
        Running { child, to, from }
    }

    fn send(&mut self, message: Value) {
        let to = self.to.as_mut().unwrap();
        writeln!(to, "{message}").unwrap();
        to.flush().unwrap();
    }

    /// The answer to request `id` (notifications on the way are passed over).
    fn answer(&mut self, id: u64) -> Value {
        let until = Instant::now() + WAIT;
        loop {
            let message = match self.from.recv_timeout(until.saturating_duration_since(Instant::now())) {
                Ok(message) => message,
                Err(RecvTimeoutError::Timeout) => panic!("no answer to {id}"),
                Err(RecvTimeoutError::Disconnected) => panic!("serve ended before answering {id}"),
            };
            if message["id"] == id {
                return message;
            }
        }
    }

    /// Ends its input: its exit code.
    fn end(mut self) -> i32 {
        drop(self.to.take());
        let until = Instant::now() + WAIT;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status.code().unwrap_or(-1);
            }
            if Instant::now() > until {
                let _ = self.child.kill();
                panic!("serve didn't stop when its input ended");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

fn listing(folder: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(folder)
        .map(|entries| entries.filter_map(Result::ok).map(|entry| entry.file_name().to_string_lossy().into_owned()).collect())
        .unwrap_or_default();
    names.sort();
    names
}

#[test]
fn serve_answers_status_on_stdio_without_reading_or_writing_the_producers_kumi() {
    let dir = tempfile::tempdir().unwrap();
    let (user, home, state) = (dir.path().join("user"), dir.path().join("transport"), dir.path().join("plugin/kumi/state"));
    std::fs::create_dir_all(&user).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    // What the app would read first: neither would load.
    std::fs::write(home.join("settings.json"), "{ not settings").unwrap();
    std::fs::write(home.join("auth.json"), "{ not a sign-in").unwrap();
    let args = ["--stdio", "--state-dir", state.to_str().unwrap(), "--kumi-home", home.to_str().unwrap(), "--daw", "reaper"];
    let mut serve = Running::start(serve(&args, &user));
    let pid = serve.child.id();

    serve.send(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "test", "version": "1"}}}));
    let initialized = serve.answer(1);
    assert_eq!(initialized["result"]["protocolVersion"], "2025-11-25");
    assert_eq!(initialized["result"]["serverInfo"], json!({"name": "kumi-serve", "version": env!("CARGO_PKG_VERSION")}));
    serve.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    serve.send(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}));
    let tools = serve.answer(2);
    assert!(tools["result"]["tools"].as_array().unwrap().iter().any(|tool| tool["name"] == "status"), "{tools}");
    serve.send(json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "status", "arguments": {"_host": {"profile_id": "default", "session_id": "s1"}}}}));
    let called = serve.answer(3);
    assert_eq!(called["result"]["isError"], false, "{called}");
    let status = &called["result"]["structuredContent"];
    assert_eq!(status["runtime"], json!({"version": env!("CARGO_PKG_VERSION"), "protocol": "kumi-serve/1"}));
    assert_eq!((status["daw"].clone(), status["connected"].clone()), (json!("reaper"), json!(false)));
    assert_eq!((status["project"].clone(), status["owner"].clone()), (Value::Null, Value::Null));
    assert_eq!((status["setup"]["component"]["installed"].clone(), status["setup"]["daw_open"].clone()), (json!(false), json!(false)));
    assert!(status["capabilities"].as_array().unwrap().contains(&json!("status")));

    assert_eq!(serve.end(), 0, "the end of input ends it");
    let written: Value = serde_json::from_slice(&std::fs::read(state.join("status.json")).unwrap()).unwrap();
    assert_eq!((written["schema"].clone(), written["stale"].clone(), written["daw"].clone()), (json!(1), json!(true), json!("reaper")));
    assert_eq!(written["owner"], json!({"profile": "default", "session_id": "s1", "pid": pid}));
    assert_eq!(std::fs::read_to_string(home.join("settings.json")).unwrap(), "{ not settings");
    assert_eq!(std::fs::read_to_string(home.join("auth.json")).unwrap(), "{ not a sign-in");
    assert_eq!(listing(&home), ["auth.json", "settings.json"], "nothing written in the Kumi home");
    assert!(listing(&user).is_empty(), "nor in the producer's own: {:?}", listing(&user));
}

#[test]
fn serve_refuses_a_daw_it_doesnt_offer_and_says_how_to_start_it() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("state");
    let args = ["--stdio", "--state-dir", state.to_str().unwrap(), "--kumi-home", dir.path().to_str().unwrap(), "--daw", "soundcraft"];
    let output = serve(&args, dir.path()).stdin(Stdio::null()).output().unwrap();
    let said = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{said}");
    assert!(said.contains("ableton, reaper or renoise") && said.contains("usage: kumi serve"), "{said}");
    assert!(output.stdout.is_empty());
    assert!(!state.exists(), "nothing made for a runtime that didn't start");
}
