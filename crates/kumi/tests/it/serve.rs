//! `kumi serve --stdio`, the binary as a host starts it: MCP on stdio, branching off before the app reads anything of
//! the producer's own Kumi.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

const WAIT: Duration = Duration::from_secs(20);

/// A runtime's folders: the producer's home (HOME), their own Kumi (`KUMI_HOME` as the host's process has it, and
/// `~/.kumi`), the transport home the runtime is given, and its state folder.
struct Folders {
    _dir: tempfile::TempDir,
    user: PathBuf,
    producers_kumi: PathBuf,
    transport: PathBuf,
    state: PathBuf,
}

impl Folders {
    fn new() -> Folders {
        let dir = tempfile::tempdir().unwrap();
        let user = dir.path().join("user");
        let (producers_kumi, transport, state) =
            (user.join("the-producers-kumi"), dir.path().join("transport"), dir.path().join("plugin/kumi/state"));
        for folder in [&user, &producers_kumi, &user.join(".kumi"), &transport] {
            std::fs::create_dir_all(folder).unwrap();
        }
        Folders { _dir: dir, user, producers_kumi, transport, state }
    }

    fn args(&self, daw: &str) -> Vec<String> {
        ["--stdio", "--state-dir", &self.state.display().to_string(), "--kumi-home", &self.transport.display().to_string(), "--daw", daw]
            .map(String::from)
            .into()
    }

    fn command(&self, daw: &str) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_kumi"));
        command
            .arg("serve")
            .args(self.args(daw))
            .env("HOME", &self.user)
            .env("USERPROFILE", &self.user)
            .env("KUMI_HOME", &self.producers_kumi)
            .env("KUMI_PROCESSES", "none")
            .env_remove("KUMI_NO_UPDATE_CHECK");
        command
    }

    fn state(&self, file: &str) -> Value {
        serde_json::from_slice(&std::fs::read(self.state.join(file)).unwrap()).unwrap()
    }
}

/// What the app would read first, in every Kumi home there is, made so reading it can't go unnoticed: on Unix a
/// FIFO, whose opening blocks (the test then times out), elsewhere text that isn't settings.
fn poison(folder: &Path) {
    for name in ["settings.json", "auth.json"] {
        let path = folder.join(name);
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            let path = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
            assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0, "mkfifo");
        }
        #[cfg(not(unix))]
        std::fs::write(&path, "{ not settings").unwrap();
    }
}

fn poisoned(folder: &Path) -> bool {
    ["settings.json", "auth.json"].iter().all(|name| {
        let path = folder.join(name);
        #[cfg(unix)]
        {
            use std::os::unix::fs::FileTypeExt;
            std::fs::symlink_metadata(&path).is_ok_and(|meta| meta.file_type().is_fifo())
        }
        #[cfg(not(unix))]
        {
            std::fs::read_to_string(&path).is_ok_and(|text| text == "{ not settings")
        }
    })
}

fn listing(folder: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(folder)
        .map(|entries| entries.filter_map(Result::ok).map(|entry| entry.file_name().to_string_lossy().into_owned()).collect())
        .unwrap_or_default();
    names.sort();
    names
}

struct Running {
    child: Child,
    to: Option<ChildStdin>,
    from: Receiver<Value>,
}

impl Running {
    fn start(command: Command) -> Running {
        Running::start_with(command, Stdio::null())
    }

    /// The same, its stderr (the host's log) going to `stderr`.
    fn start_with(mut command: Command, stderr: Stdio) -> Running {
        let mut child = command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(stderr).spawn().unwrap();
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
                Err(RecvTimeoutError::Timeout) => {
                    let _ = self.child.kill();
                    panic!("no answer to {id}: did it open a poisoned file?")
                }
                Err(RecvTimeoutError::Disconnected) => panic!("serve ended before answering {id}"),
            };
            if message["id"] == id {
                return message;
            }
        }
    }

    fn initialize(&mut self) {
        self.send(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "test", "version": "1"}}}));
        let initialized = self.answer(1);
        assert_eq!(initialized["result"]["protocolVersion"], "2025-11-25");
        assert_eq!(initialized["result"]["serverInfo"], json!({"name": "kumi-serve", "version": env!("CARGO_PKG_VERSION")}));
        self.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    }

    /// Its exit code once it stops.
    fn exited(mut self) -> i32 {
        let until = Instant::now() + WAIT;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status.code().unwrap_or(-1);
            }
            if Instant::now() > until {
                let _ = self.child.kill();
                panic!("serve didn't stop");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Ends its input: its exit code.
    fn end(mut self) -> i32 {
        drop(self.to.take());
        self.exited()
    }
}

#[test]
fn serve_answers_status_on_stdio_without_reading_or_writing_the_producers_kumi() {
    let folders = Folders::new();
    for home in [&folders.producers_kumi, &folders.user.join(".kumi"), &folders.transport] {
        poison(home);
    }
    let mut serve = Running::start(folders.command("reaper"));
    let pid = serve.child.id();
    serve.initialize();
    serve.send(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}));
    let tools = serve.answer(2);
    assert!(tools["result"]["tools"].as_array().unwrap().iter().any(|tool| tool["name"] == "status"), "{tools}");
    serve.send(json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "status", "arguments": {"_host": {"profile_id": "default", "session_id": "s1", "task_id": "t1"}}}}));
    let called = serve.answer(3);
    assert_eq!(called["result"]["isError"], false, "{called}");
    let status = &called["result"]["structuredContent"];
    assert_eq!(status["runtime"]["protocol"], "kumi-serve/1");
    assert_eq!(status["runtime"]["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(status["runtime"].get("home"), None, "no path on this computer reaches the host");
    assert_eq!((status["daw"].clone(), status["connected"].clone()), (json!("reaper"), json!(false)));
    assert_eq!((status["project"].clone(), status["owner"].clone()), (Value::Null, Value::Null));
    assert_eq!((status["setup"]["component"]["installed"].clone(), status["setup"]["daw_open"].clone()), (json!(false), json!(false)));
    assert!(status["capabilities"].as_array().unwrap().contains(&json!("status")));

    assert_eq!(serve.end(), 0, "the end of input ends it");
    let written = folders.state("status.json");
    assert_eq!((written["schema"].clone(), written["stale"].clone(), written["daw"].clone()), (json!(1), json!(true), json!("reaper")));
    // HISTORY names the runtime it's from: its rows carry the same generation, an earlier runtime's another.
    assert_eq!(folders.state("history.json")["generation"], status["runtime"]["generation"]);
    assert_eq!(written["owner"], json!({"profile": "default", "session_id": "s1", "pid": pid}));
    assert_eq!(folders.state("owner.json")["running"], false);
    for home in [&folders.producers_kumi, &folders.user.join(".kumi"), &folders.transport] {
        assert!(poisoned(home), "{} untouched", home.display());
        assert_eq!(listing(home), ["auth.json", "settings.json"], "nothing written in {}", home.display());
    }
    assert_eq!(listing(&folders.user), [".kumi", "the-producers-kumi"], "nor anywhere else in the producer's home");
}

#[test]
fn serve_refuses_a_daw_it_doesnt_offer_and_says_how_to_start_it() {
    let folders = Folders::new();
    let output = folders.command("soundcraft").stdin(Stdio::null()).output().unwrap();
    let said = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{said}");
    assert!(said.contains("ableton, reaper or renoise") && said.contains("usage: kumi serve"), "{said}");
    assert!(output.stdout.is_empty());
    assert!(!folders.state.exists(), "nothing made for a runtime that didn't start");
}

/// Every file under `folder`, as text.
fn texts(folder: &Path) -> Vec<(PathBuf, String)> {
    let mut found = vec![];
    let mut folders = vec![folder.to_path_buf()];
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(&folder).unwrap().filter_map(Result::ok) {
            let path = entry.path();
            if path.is_dir() {
                folders.push(path);
            } else if let Ok(bytes) = std::fs::read(&path) {
                found.push((path, String::from_utf8_lossy(&bytes).into_owned()));
            }
        }
    }
    found
}

#[test]
fn serve_keeps_the_hosts_private_key_out_of_its_log_its_folders_and_its_answers() {
    const SENTINEL: &str = "sk-sentinel-9d41b7";
    let folders = Folders::new();
    let log = folders._dir.path().join("serve.log");
    let mut serve = Running::start_with(folders.command("reaper"), Stdio::from(std::fs::File::create(&log).unwrap()));
    serve.initialize();
    let mirelo = json!({"api_key": SENTINEL, "acknowledged": true, "enabled": true, "reservation_id": "r-1"});
    let host = |id: &str| json!({"profile_id": "default", "session_id": "s1", "tool_call_id": id});
    let mut change =
        json!({"title": "Add a track", "steps": [{"tool": "add_track", "input": {}}], "expected_project_key": "p-1", "expected_epoch": 1});
    change["_host"] = host("tc-3");
    change["_mirelo"] = mirelo.clone();
    let calls = [
        ("status", json!({"_host": host("tc-1"), "_mirelo": mirelo.clone()})),
        ("read", json!({"action": "data", "tool": "no_such_reader", "query": {}, "_host": host("tc-2"), "_mirelo": mirelo.clone()})),
        ("change", change),
        ("status", json!({"_mirelo": mirelo})),
        ("status", json!({"_host": host("tc-5"), "_mirelo": {"api_key": SENTINEL.repeat(300)}})),
    ];
    let mut said = vec![];
    for (n, (name, arguments)) in calls.into_iter().enumerate() {
        let id = n as u64 + 2;
        serve.send(json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": name, "arguments": arguments}}));
        said.push(serve.answer(id));
    }
    // What each came to: an error's code, or a change's own refusal (C1's `{state: "refused", code}`).
    let codes: Vec<Value> = said
        .iter()
        .map(|answer| {
            let content = &answer["result"]["structuredContent"];
            content["error"]["code"].as_str().or(content["code"].as_str()).map_or(Value::Null, |code| json!(code))
        })
        .collect();
    assert_eq!(said[0]["result"]["isError"], false, "status runs: {}", said[0]);
    assert!(said[1]["result"]["isError"] == true && codes[1].is_string(), "a read with bad arguments: {}", said[1]);
    assert_eq!(said[2]["result"]["structuredContent"]["state"], "refused", "a change with no DAW: {}", said[2]);
    assert_eq!(codes[2], "not_connected", "a change with no DAW: {}", said[2]);
    assert_eq!((codes[3].clone(), codes[4].clone()), (json!("invalid"), json!("invalid")), "without _host, and oversized");
    assert_eq!(serve.end(), 0, "the end of input ends it");

    for answer in &said {
        assert!(!answer.to_string().contains(SENTINEL), "said: {answer}");
    }
    let logged = std::fs::read_to_string(&log).unwrap();
    assert!(!logged.contains(SENTINEL), "its log keeps the key");
    let kept = texts(&folders.state);
    assert!(kept.len() >= 2, "the state folder was looked through ({} files)", kept.len());
    for (path, text) in kept.iter().chain(&texts(&folders.transport)) {
        assert!(!text.contains(SENTINEL), "{} keeps the key", path.display());
    }
}

#[cfg(unix)]
#[test]
fn serve_works_through_ctrl_c_and_settles_when_terminated() {
    let folders = Folders::new();
    let mut serve = Running::start(folders.command("renoise"));
    let pid = serve.child.id() as libc::pid_t;
    serve.initialize();
    assert_eq!(unsafe { libc::kill(pid, libc::SIGINT) }, 0);
    serve.send(json!({"jsonrpc": "2.0", "id": 2, "method": "ping"}));
    assert_eq!(serve.answer(2)["result"], json!({}), "Ctrl-C is the host's: the runtime works on");
    assert_eq!(unsafe { libc::kill(pid, libc::SIGTERM) }, 0);
    assert_eq!(serve.exited(), 0, "told to stop, it settles and exits");
    assert_eq!(folders.state("status.json")["stale"], true);
    assert_eq!(folders.state("owner.json")["running"], false);
}
