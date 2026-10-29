//! The session on a thread of its own, made as the app makes it (`kumi::cli::run` with `io.threaded`, as main() sets it):
//! once Kumi has ended, what the session kept is written and it has let go of its conversation and its database, for
//! `kumi -p`, the full screen and plain mode alike.
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
    end: RefCell<Option<Rc<dyn Fn()>>>,
    /// Read already as Kumi starts reading: handed over as it resumes, as a reader thread's pump can.
    ahead: RefCell<Vec<u8>>,
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
        *self.data.borrow_mut() = Some(listener.clone());
        let ahead = std::mem::take(&mut *self.ahead.borrow_mut());
        if !ahead.is_empty() {
            listener(&ahead);
        }
    }
    fn pause(&self) {
        self.data.borrow_mut().take();
    }
    fn on_end(&self, end: Rc<dyn Fn()>) {
        *self.end.borrow_mut() = Some(end);
    }
}
impl Input {
    fn write(&self, text: &str) {
        let listener = self.data.borrow().clone().expect("Kumi reads keys");
        listener(text.as_bytes())
    }
    /// What's typed has ended (the producer's ctrl+d, a script's last line).
    fn end(&self) {
        let end = self.end.borrow().clone();
        if let Some(end) = end {
            end()
        }
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
/// held is let go (its `<id>.lock` free for another Kumi). Kumi's database is closed by then. The session opens it on
/// its thread and closes it only as the thread ends, after what the session does as it ends, and SQLite folds the
/// write-ahead log back into kumi.db and deletes it as the last connection closes. (Asked first: reading the notes opens
/// kumi.db again.)
fn left(home: &Path) -> (Vec<String>, Vec<(String, bool)>) {
    assert!(!home.join("kumi.db-wal").exists(), "kumi.db is still open once Kumi ended: Kumi didn't wait for the session's thread");
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
            // Sent at once, while the session is likely still starting: it waits for the session (#728).
            input.write("Note how I start a track.\r");
            until("the answer", || asked.load(Ordering::SeqCst) == 2 && screen().contains("Kept it.")).await;
            // Kumi idle again (its answer done), so ctrl+c quits rather than stops it.
            until("Kumi idle", || screen().contains("Ready")).await;
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

/// Plain mode, two lines piped in as it starts: the first is answered once the session has started, the second is said
/// to wait (not sent as a second request the session turns away), and once Kumi ends all is written and let go.
#[tokio::test(flavor = "current_thread")]
async fn plain_mode_has_written_and_let_go_of_everything_once_it_ends() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let asked = Arc::new(AtomicUsize::new(0));
            let (folder, mut env) = home(&noting_server(asked.clone()).await);
            env.insert("KUMI_UI".into(), "plain".into());
            let input = Rc::new(Input::default());
            let (out, err) = (Rc::new(Output::default()), Rc::new(Output::default()));
            let running = tokio::task::spawn_local(kumi(env, input.clone(), out.clone(), err.clone(), &["--inference-only", "-c"]));
            until("plain mode reads lines", || input.data.borrow().is_some()).await;
            input.write("Note how I start a track.\nAnd keep it short.\n");
            until("the answer", || asked.load(Ordering::SeqCst) >= 2 && out.0.borrow().contains("Kept it.")).await;
            let said = format!("{}{}", out.0.borrow(), err.0.borrow());
            assert!(said.contains("No second turn was submitted."), "{said}");
            assert!(!said.contains("Session is busy"), "{said}");
            input.end();
            let code = tokio::time::timeout(Duration::from_secs(30), running).await.expect("Kumi ends").unwrap();
            assert_eq!(code, 0, "stdout: {}; stderr: {}", out.0.borrow(), err.0.borrow());
            assert_eq!(asked.load(Ordering::SeqCst), 2, "the first line alone was asked");
            let (notes, locks) = left(folder.path());
            assert_eq!(notes, [NOTE]);
            assert!(!locks.is_empty(), "the conversation was kept");
            assert!(locks.iter().all(|(_, free)| *free), "still held once Kumi ended: {locks:?}");
        })
        .await;
}

/// Plain mode, a line read already as it starts reading (handed over as its reader resumes, before the session was asked
/// to start): it waits for the session, rather than going ahead of its Start.
#[tokio::test(flavor = "current_thread")]
async fn plain_mode_answers_a_line_read_before_its_session_was_asked_to_start() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let asked = Arc::new(AtomicUsize::new(0));
            let (folder, mut env) = home(&noting_server(asked.clone()).await);
            env.insert("KUMI_UI".into(), "plain".into());
            let input = Rc::new(Input { ahead: RefCell::new(b"Note how I start a track.\n".to_vec()), ..Default::default() });
            let (out, err) = (Rc::new(Output::default()), Rc::new(Output::default()));
            let running = tokio::task::spawn_local(kumi(env, input.clone(), out.clone(), err.clone(), &["--inference-only", "-c"]));
            until("the answer", || asked.load(Ordering::SeqCst) >= 2 && out.0.borrow().contains("Kept it.")).await;
            let said = format!("{}{}", out.0.borrow(), err.0.borrow());
            assert!(!said.contains("Session is"), "{said}");
            input.end();
            let code = tokio::time::timeout(Duration::from_secs(30), running).await.expect("Kumi ends").unwrap();
            assert_eq!(code, 0, "{said}");
            assert_eq!(left(folder.path()).0, [NOTE]);
        })
        .await;
}
