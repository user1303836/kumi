//! The session on a thread of its own, made as the app makes it (`kumi::cli::run` with `io.threaded`, as main() sets it):
//! once Kumi has ended, what the session kept is written and it has let go of its conversation, for `kumi -p` and the
//! full screen alike.
#[path = "../support/vt.rs"]
mod vt;
use kumi::{
    cli::{run, CliIo},
    input::{ByteListener, TerminalInput},
    tui::tty::TtyOutput,
};
use kumi_daw::io::Answers;
use kumi_types::system::Env;
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    path::Path,
    rc::Rc,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

/// What the producer types: a terminal, or not.
#[derive(Default)]
struct Input {
    tty: bool,
    raw: Cell<bool>,
    data: RefCell<Option<ByteListener>>,
}
impl TerminalInput for Input {
    fn is_tty(&self) -> bool {
        self.tty
    }
    fn is_raw(&self) -> bool {
        self.raw.get()
    }
    fn set_raw_mode(&self, raw: bool) -> std::io::Result<()> {
        self.raw.set(raw);
        Ok(())
    }
    fn resume(&self, listener: ByteListener) {
        *self.data.borrow_mut() = Some(listener);
    }
    fn pause(&self) {
        self.data.borrow_mut().take();
    }
    fn on_end(&self, _: Rc<dyn Fn()>) {}
}
impl Input {
    fn write(&self, text: &str) {
        let listener = self.data.borrow().clone().expect("Kumi reads keys");
        listener(text.as_bytes())
    }
}

/// What Kumi writes: a terminal of 120 by 40, or not.
#[derive(Default)]
struct Output(RefCell<String>, bool);
impl TtyOutput for Output {
    fn is_tty(&self) -> bool {
        self.1
    }
    fn columns(&self) -> Option<i32> {
        self.1.then_some(120)
    }
    fn rows(&self) -> Option<i32> {
        self.1.then_some(40)
    }
    fn write(&self, text: &str) {
        self.0.borrow_mut().push_str(text);
    }
}

const NOTE: &str = "I always start a track with a warm pad.";

/// A model server on this computer whose model keeps a note (`remember`) as it answers the first request, and answers
/// the next with words. `asked` counts the requests.
async fn noting_server(asked: Arc<AtomicUsize>) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else { break };
            let asked = asked.clone();
            tokio::spawn(async move {
                // The request's head, then as much body as it says.
                let (mut raw, mut buffer) = (Vec::new(), [0; 8192]);
                let (head_end, length) = loop {
                    let n = socket.read(&mut buffer).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    raw.extend_from_slice(&buffer[..n]);
                    if let Some(end) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&raw[..end]).to_ascii_lowercase();
                        let length = head
                            .lines()
                            .find_map(|line| line.strip_prefix("content-length:").and_then(|n| n.trim().parse::<usize>().ok()))
                            .unwrap_or(0);
                        break (end + 4, length);
                    }
                };
                while raw.len() < head_end + length {
                    let n = socket.read(&mut buffer).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    raw.extend_from_slice(&buffer[..n]);
                }
                let head = String::from_utf8_lossy(&raw[..head_end]).into_owned();
                let chunk = |delta: Value, finish: Value| {
                    let chunk = json!({"id": "chatcmpl-1", "object": "chat.completion.chunk", "model": "tiny",
                        "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]});
                    format!("data: {chunk}\n\n")
                };
                let (kind, body) = if head.split_whitespace().nth(1).is_some_and(|path| path.ends_with("/chat/completions")) {
                    let body = if asked.fetch_add(1, Ordering::SeqCst) == 0 {
                        let arguments = json!({"note": NOTE, "about": "producer"}).to_string();
                        let call = json!({"index": 0, "id": "call_1", "type": "function",
                            "function": {"name": "remember", "arguments": arguments}});
                        chunk(json!({"role": "assistant", "tool_calls": [call]}), Value::Null) + &chunk(json!({}), json!("tool_calls"))
                    } else {
                        chunk(json!({"content": "Kept it."}), json!("stop"))
                    };
                    ("text/event-stream", body + "data: [DONE]\n\n")
                } else {
                    ("application/json", json!({"object": "list", "data": [{"id": "tiny", "object": "model"}]}).to_string())
                };
                let response =
                    format!("HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                let _ = socket.write_all(response.as_bytes()).await;
            });
        }
    });
    url
}

/// Kumi's home, its model the server at `url`'s.
fn home(url: &str) -> (tempfile::TempDir, Env) {
    let folder = tempfile::tempdir().unwrap();
    let settings = json!({"model": "tiny-server/tiny", "modelServers": [{"name": "Tiny Server", "baseURL": url}],
        "updateCheck": false, "modelCatalog": false, "libraryFolders": [], "whatsNew": false, "musicOffered": true});
    std::fs::write(folder.path().join("settings.json"), settings.to_string()).unwrap();
    let env = Env::from([
        ("KUMI_HOME".into(), folder.path().display().to_string()),
        ("KUMI_NO_UPDATE_CHECK".into(), "1".into()),
        ("KUMI_REMOTE_SCRIPTS_DIR".into(), folder.path().join("Remote Scripts").display().to_string()),
    ]);
    (folder, env)
}

/// Kumi made as the app makes it, its session on a thread of its own: no test's stand-ins, which couldn't cross to it.
fn kumi(env: Env, input: Rc<Input>, out: Rc<Output>, err: Rc<Output>, args: &[&str]) -> impl std::future::Future<Output = i32> {
    let mut io = CliIo::new(input, out, err, env);
    io.args = args.iter().map(|arg| arg.to_string()).collect();
    io.threaded = true;
    run(io, Answers::default())
}

/// What the session kept, read once Kumi has ended: the producer's notes in kumi.db, and whether each conversation it
/// held is let go (its `<id>.lock` free for another Kumi).
fn left(home: &Path) -> (Vec<String>, Vec<(String, bool)>) {
    let notes = kumi_store::read_only(home.join("kumi.db"), |connection| kumi_store::notes::in_use(connection, &kumi_store::Scope::Global))
        .unwrap()
        .into_iter()
        .map(|note| note.text)
        .collect();
    let mut locks = vec![];
    let mut folders = vec![home.join("projects")];
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(&folder).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                folders.push(path);
            } else if path.extension().is_some_and(|extension| extension == "lock") {
                let free = std::fs::OpenOptions::new().read(true).write(true).open(&path).is_ok_and(|file| file.try_lock().is_ok());
                locks.push((path.display().to_string(), free));
            }
        }
    }
    (notes, locks)
}

/// Until `done`, within ten seconds.
async fn until(what: &str, done: impl Fn() -> bool) {
    for _ in 0..2500 {
        if done() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(4)).await;
    }
    panic!("{what} never happened");
}

#[tokio::test(flavor = "current_thread")]
async fn kumi_p_has_written_and_let_go_of_everything_once_it_ends() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let asked = Arc::new(AtomicUsize::new(0));
            let (folder, env) = home(&noting_server(asked.clone()).await);
            let (out, err) = (Rc::new(Output::default()), Rc::new(Output::default()));
            let args = ["-p", "Note how I start a track.", "--inference-only", "-c"];
            let code = tokio::time::timeout(Duration::from_secs(30), kumi(env, Rc::default(), out.clone(), err.clone(), &args))
                .await
                .expect("kumi -p ends");
            assert_eq!(code, 0, "stdout: {}; stderr: {}", out.0.borrow(), err.0.borrow());
            assert_eq!(asked.load(Ordering::SeqCst), 2, "a note, then the answer");
            let (notes, locks) = left(folder.path());
            assert_eq!(notes, [NOTE]);
            assert!(!locks.is_empty(), "the conversation was kept");
            assert!(locks.iter().all(|(_, free)| *free), "still held once Kumi ended: {locks:?}");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn the_full_screen_has_written_and_let_go_of_everything_once_it_ends() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let asked = Arc::new(AtomicUsize::new(0));
            let (folder, env) = home(&noting_server(asked.clone()).await);
            let input = Rc::new(Input { tty: true, ..Default::default() });
            let (out, err) = (Rc::new(Output(RefCell::default(), true)), Rc::new(Output::default()));
            let running = tokio::task::spawn_local(kumi(env, input.clone(), out.clone(), err.clone(), &["--inference-only", "-c"]));
            let term = RefCell::new(vt::VirtualTerminal::new(120, 40));
            let consumed = Cell::new(0);
            let screen = || {
                let written = out.0.borrow();
                term.borrow_mut().write(&written[consumed.get()..]);
                consumed.set(written.len());
                term.borrow().lines().join("\n")
            };
            until("the screen reads keys", || input.data.borrow().is_some()).await;
            // The screen takes keys before the session has started, as it always has: a request sent while it's starting
            // is turned away as busy, so it's sent again until the model is asked.
            for _ in 0..100 {
                input.write("Note how I start a track.\r");
                for _ in 0..50 {
                    if asked.load(Ordering::SeqCst) > 0 {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(4)).await;
                }
                if asked.load(Ordering::SeqCst) > 0 {
                    break;
                }
            }
            for _ in 0..2500 {
                if asked.load(Ordering::SeqCst) == 2 && screen().contains("Kept it.") {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(4)).await;
            }
            if !screen().contains("Kept it.") {
                let plain = regex::Regex::new(r"\x1b\[[0-9;?<>]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(\x07|\x1b\\)")
                    .unwrap()
                    .replace_all(&out.0.borrow(), "\n")
                    .to_string();
                let lines: Vec<&str> = plain.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
                let mut said: Vec<&str> = vec![];
                for line in lines {
                    if said.last() != Some(&line) {
                        said.push(line);
                    }
                }
                let mut logged = vec![];
                let mut folders = vec![folder.path().join("projects")];
                while let Some(at) = folders.pop() {
                    for entry in std::fs::read_dir(&at).into_iter().flatten().flatten() {
                        let path = entry.path();
                        if path.is_dir() {
                            folders.push(path);
                        } else if path.extension().is_some_and(|e| e == "jsonl") {
                            for line in std::fs::read_to_string(&path).unwrap_or_default().lines() {
                                let v: Value = serde_json::from_str(line).unwrap_or(Value::Null);
                                let event = &v["event"];
                                logged.push(format!("{} {} {}", event["type"], event["state"], event["result"]["stopReason"]));
                            }
                        }
                    }
                }
                panic!(
                    "asked {} times:\n{}\n--- log:\n{}\n--- everything written:\n{}",
                    asked.load(Ordering::SeqCst),
                    screen(),
                    logged.join("\n"),
                    said.join("\n")
                );
            }
            input.write("\x03\x03");
            let code = tokio::time::timeout(Duration::from_secs(30), running).await.expect("Kumi ends").unwrap();
            assert_eq!(code, 0, "{}\nstderr: {}", screen(), err.0.borrow());
            let (notes, locks) = left(folder.path());
            assert_eq!(notes, [NOTE]);
            assert!(!locks.is_empty(), "the conversation was kept");
            assert!(locks.iter().all(|(_, free)| *free), "still held once Kumi ended: {locks:?}");
        })
        .await;
}
