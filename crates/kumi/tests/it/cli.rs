//! Native CLI dispatch and session binding: production stores, I/O and model controller.
use kumi::{
    cli::{help, run, CliIo},
    input::{ByteListener, TerminalInput},
    tui::tty::TtyOutput,
};
use kumi_daw_ableton::app::AbletonFactory;
use kumi_daw_ableton::create_inference_only_integration;
use kumi_types::system::Env;
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
#[derive(Default)]
struct Input {
    tty: bool,
    raw: Cell<bool>,
    data: RefCell<Option<ByteListener>>,
    end: RefCell<Option<Rc<dyn Fn()>>>,
    ready: tokio::sync::Notify,
}
impl TerminalInput for Input {
    fn is_tty(&self) -> bool {
        self.tty
    }
    fn is_raw(&self) -> bool {
        self.raw.get()
    }
    fn set_raw_mode(&self, v: bool) -> std::io::Result<()> {
        self.raw.set(v);
        Ok(())
    }
    fn resume(&self, f: ByteListener) {
        *self.data.borrow_mut() = Some(f);
        self.ready.notify_one();
    }
    fn pause(&self) {
        self.data.borrow_mut().take();
    }
    fn on_end(&self, f: Rc<dyn Fn()>) {
        *self.end.borrow_mut() = Some(f)
    }
}
impl Input {
    async fn wait_ready(&self) {
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while self.data.borrow().is_none() {
                self.ready.notified().await;
            }
        })
        .await
        .expect("CLI did not register its input listener");
    }
    fn write(&self, s: &str) {
        let f = self.data.borrow().clone().expect("wait for the CLI input listener before writing");
        f(s.as_bytes())
    }
    fn end(&self) {
        let f = self.end.borrow().clone();
        if let Some(f) = f {
            f()
        }
    }
}
#[derive(Default)]
struct Output(RefCell<String>, tokio::sync::Notify);
impl TtyOutput for Output {
    fn is_tty(&self) -> bool {
        false
    }
    fn columns(&self) -> Option<i32> {
        None
    }
    fn rows(&self) -> Option<i32> {
        None
    }
    fn write(&self, s: &str) {
        self.0.borrow_mut().push_str(s);
        self.1.notify_one();
    }
}
struct Fixture {
    folder: tempfile::TempDir,
    input: Rc<Input>,
    out: Rc<Output>,
    err: Rc<Output>,
    env: Env,
}
impl Fixture {
    fn new(tty: bool) -> Self {
        let folder = tempfile::tempdir().unwrap();
        let env = Env::from([
            ("KUMI_HOME".into(), folder.path().display().to_string()),
            ("KUMI_NO_UPDATE_CHECK".into(), "1".into()),
            ("KUMI_UI".into(), "plain".into()),
            ("KUMI_REMOTE_SCRIPTS_DIR".into(), folder.path().join("Remote Scripts").display().to_string()),
        ]);
        Self { folder, input: Rc::new(Input { tty, ..Default::default() }), out: Rc::default(), err: Rc::default(), env }
    }
    fn io(&self, args: &[&str]) -> CliIo {
        let mut io = CliIo::new(self.input.clone(), self.out.clone(), self.err.clone(), self.env.clone());
        io.args = args.iter().map(|s| s.to_string()).collect();
        io
    }
    fn settings(&self, value: Value) {
        std::fs::write(self.folder.path().join("settings.json"), serde_json::to_vec(&value).unwrap()).unwrap();
    }
    fn output(&self) -> String {
        self.out.0.borrow().clone()
    }
    async fn wait_output(&self, text: &str) {
        let received = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while !self.out.0.borrow().contains(text) {
                self.out.1.notified().await;
            }
        })
        .await;
        assert!(received.is_ok(), "missing {text:?}; stdout: {}; stderr: {}", self.output(), self.err.0.borrow());
    }
}
fn unused_factory() -> AbletonFactory {
    AbletonFactory(Rc::new(|_| panic!("this command must not create an Ableton integration")))
}
macro_rules! case {
    ($name:ident,$body:expr) => {
        #[tokio::test(flavor = "current_thread")]
        async fn $name() {
            tokio::task::LocalSet::new().run_until($body).await;
        }
    };
}
// `kumi export` with nothing kept says what to do in the words of the DAW asked for (--daw), else the producer's.
case!(with_nothing_kept_kumi_export_says_so_in_the_daws_words, async {
    let f = Fixture::new(false);
    for (args, place) in
        [(&["export"][..], "a Set"), (&["--daw", "reaper", "export"], "a project"), (&["--daw", "renoise", "export"], "a song")]
    {
        f.err.0.borrow_mut().clear();
        assert_eq!(run(f.io(args), unused_factory()).await, 1, "{args:?}");
        let said = f.err.0.borrow().clone();
        assert!(said.contains(&format!("talk with Kumi about {place} first.")), "{args:?}: {said}");
    }
});

case!(help_version_errors_and_checkout_commands, async {
    let f = Fixture::new(false);
    assert_eq!(run(f.io(&["--version"]), unused_factory()).await, 0);
    assert_eq!(f.output(), format!("Kumi {}\n", kumi_types::version::KUMI_VERSION));
    assert!(f.err.0.borrow().is_empty());
    assert!(!f.folder.path().join("settings.json").exists());
    f.out.0.borrow_mut().clear();
    assert_eq!(run(f.io(&["--help"]), unused_factory()).await, 0);
    assert_eq!(f.output(), help(false));
    assert!(f.output().contains("cargo build --release --workspace"));
    assert!(!f.output().contains("npm"));
    assert!(!f.output().contains("Node"));
    assert!(help(true).contains("kumi update --rollback"));
    assert!(!help(false).contains("update --rollback"));
    // Help leads with kumi itself, which connects to the DAW the producer opens, whichever it is: no option to learn.
    // kumi bridge is Live's way by hand, and --daw keeps Kumi to one DAW.
    let installed = help(true);
    let first: Vec<&str> = installed.lines().skip_while(|l| *l != "First run:").skip(1).take_while(|l| !l.is_empty()).collect();
    assert!(
        first[0].starts_with("  kumi ") && first[0].contains("Sign in, then open your DAW (Live, REAPER or Renoise), before or after Kumi"),
        "{installed}"
    );
    assert!(first.iter().any(|l| l.starts_with("  kumi bridge") && l.contains("Or, with Live closed:")), "{installed}");
    assert!(
        first.iter().any(|l| l.starts_with("  kumi --daw live|reaper|renoise ") && l.contains("Work with that DAW only")),
        "{installed}"
    );
    for asked in [&["-h"][..], &["help"], &["help", "bridge"], &["model", "--help"]] {
        f.out.0.borrow_mut().clear();
        assert_eq!(run(f.io(asked), unused_factory()).await, 0, "{asked:?}");
        assert_eq!(f.output(), help(false), "{asked:?}");
    }
    assert_eq!(run(f.io(&["--unknown"]), unused_factory()).await, 1);
    assert_eq!(f.err.0.borrow().as_str(), "Kumi: No option called --unknown; cargo run -p kumi -- --help lists the commands.\n");
    f.err.0.borrow_mut().clear();
    assert_eq!(run(f.io(&["udpate"]), unused_factory()).await, 1);
    assert!(f.err.0.borrow().contains("No command called udpate (did you mean update?); cargo run -p kumi -- --help lists the commands."));
    f.out.0.borrow_mut().clear();
    assert_eq!(run(f.io(&["update", "--rollback"]), unused_factory()).await, 1);
    assert!(f.output().contains("Check out the commit you want with git, then run: cargo build --release --workspace"));
    f.out.0.borrow_mut().clear();
    assert_eq!(run(f.io(&["uninstall"]), unused_factory()).await, 1);
    assert!(f.output().contains("your files are in ~/.kumi"));
});
case!(model_command_preserves_existing_settings_and_reports_environment_override, async {
    let mut f = Fixture::new(false);
    let before = json!({"model":"anthropic/claude-sonnet-5-5","effort":"high","panelTab":"history","updateCheck":false,"libraryFolders":["/music/library"],"modelServers":[{"name":"Studio","baseURL":"http://127.0.0.1:1234/v1","apiKey":"studio-private-token"}],"voice":{"send":true,"language":"ja","microphone":"Scarlett 2i2 USB"}});
    f.settings(before.clone());
    assert_eq!(run(f.io(&["model"]), unused_factory()).await, 0);
    assert!(f.output().contains("Model: anthropic/claude-sonnet-5-5."));
    f.out.0.borrow_mut().clear();
    f.env.insert("KUMI_MODEL".into(), "ollama/qwen3:8b".into());
    assert_eq!(run(f.io(&["model", "openai-codex/gpt-6-astra"]), unused_factory()).await, 0);
    assert_eq!(f.output(), "Model set to openai-codex/gpt-6-astra.\nKUMI_MODEL=ollama/qwen3:8b currently overrides it.\n");
    let saved: Value = serde_json::from_slice(&std::fs::read(f.folder.path().join("settings.json")).unwrap()).unwrap();
    for key in ["effort", "panelTab", "updateCheck", "libraryFolders", "modelServers", "voice"] {
        assert_eq!(saved[key], before[key], "{key}");
    }
    assert_eq!(saved["model"], "openai-codex/gpt-6-astra");
});
case!(sign_in_choice_non_tty_and_invalid_choice, async {
    let f = Fixture::new(false);
    assert_eq!(run(f.io(&["login"]), unused_factory()).await, 1);
    assert!(f.err.0.borrow().contains("login <provider>, with provider one of openai-codex, anthropic, openai, opencode."));
    let f = Fixture::new(true);
    let task = tokio::task::spawn_local(run(f.io(&["login"]), unused_factory()));
    f.input.wait_ready().await;
    assert!(f.output().contains("How do you want to sign in?"));
    assert!(f.output().contains("Choose 1–4:"));
    f.input.write("1.5\n");
    assert_eq!(task.await.unwrap(), 1);
    assert!(f.output().ends_with("Nothing chosen; nothing changed.\n"));
    assert!(!f.folder.path().join("auth.json").exists());
});
case!(existing_credentials_remain_usable_and_logout_keeps_other_providers, async {
    use kumi_ai::auth::store::{open_credential_store, CredentialStore};
    let f = Fixture::new(false);
    let auth = f.folder.path().join("auth.json");
    std::fs::write(&auth,serde_json::to_vec(&json!({"version":1,"credentials":{"anthropic":{"type":"api-key","key":"sk-ant-saved-private-000000"},"openai":{"type":"api-key","key":"sk-saved-private-000000"}}})).unwrap()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&auth, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let original = std::fs::read(&auth).unwrap();
    assert_eq!(run(f.io(&["auth"]), unused_factory()).await, 0);
    assert!(!f.output().contains("sk-ant-saved"));
    assert_eq!(std::fs::read(&auth).unwrap(), original);
    let store = open_credential_store(&auth);
    assert!(store.get("anthropic").await.unwrap().is_some());
    assert_eq!(run(f.io(&["logout", "anthropic"]), unused_factory()).await, 0);
    assert!(store.get("anthropic").await.unwrap().is_none());
    assert!(store.get("openai").await.unwrap().is_some());
});
case!(inference_only_reports_missing_signin_and_closes_pipe_without_live, async {
    let f = Fixture::new(false);
    f.settings(json!({"model":"anthropic/claude-sonnet-5-5","updateCheck":false,"modelCatalog":false,"libraryFolders":[]}));
    let task = tokio::task::spawn_local(run(f.io(&["--inference-only"]), unused_factory()));
    f.input.wait_ready().await;
    f.input.write("hello\n");
    f.wait_output("Not signed in to Anthropic").await;
    f.input.write("/status\n/quit\n");
    f.input.end();
    assert_eq!(tokio::time::timeout(std::time::Duration::from_secs(10), task).await.unwrap().unwrap(), 0);
    assert!(f.output().contains("Kumi"), "{}", f.output());
    assert!(f.output().contains("Kumi closed"));
    assert!(f.err.0.borrow().is_empty());
    assert!(!f.input.raw.get());
});
case!(live_binding_passes_callbacks_and_stores_to_native_session, async {
    let mut f = Fixture::new(false);
    let config = f.folder.path().join("bridge.json");
    std::fs::write(&config, "{}").unwrap();
    f.env.insert("KUMI_BRIDGE_CONFIG".into(), config.display().to_string());
    f.settings(json!({"model":"anthropic/claude-sonnet-5-5","updateCheck":false,"modelCatalog":false}));
    let called = Rc::new(Cell::new(false));
    let mark = called.clone();
    let judged = Rc::new(RefCell::new(None));
    let tell = judged.clone();
    let factory = AbletonFactory(Rc::new(move |options: kumi_daw_ableton::AbletonOptions| {
        mark.set(true);
        *tell.borrow_mut() = options.on_judge.clone();
        assert!(options.bridge_config.is_some());
        assert!(options.project_store.is_some());
        assert!(options.restore_file.as_ref().unwrap().ends_with("audition-restore.json"));
        assert!(
            options.on_focus.is_some()
                && options.on_pointed.is_some()
                && options.on_transport.is_some()
                && options.on_change.is_some()
                && options.on_action.is_some()
                && options.on_watch.is_some()
                && options.on_catch_up.is_some()
                && options.on_audition.is_some()
                && options.on_judge.is_some()
        );
        let connection = options.on_connection;
        create_inference_only_integration(Rc::new(move |state| connection(state, None)))
    }));
    let task = tokio::task::spawn_local(run(f.io(&["--bridge-config", config.to_str().unwrap()]), factory));
    f.input.wait_ready().await;
    // The judge's rounds reach the session (the loop's decisions) and the screen.
    let on_judge = judged.borrow().clone().expect("the judge's callback");
    on_judge(judged_round());
    f.wait_output("[judged] Round 1 · loudness").await;
    f.wait_output("[judged]   kept: loudness improved").await;
    f.input.write("/quit\n");
    f.input.end();
    assert_eq!(tokio::time::timeout(std::time::Duration::from_secs(10), task).await.unwrap().unwrap(), 0);
    assert!(called.get());
    assert!(f.err.0.borrow().is_empty());
});
fn judged_round() -> kumi_listening::round::Round {
    use kumi_listening::round::{Round, RoundKind};
    Round {
        round: 1,
        kind: RoundKind::Judged,
        heard: "the mix, bars 1–9".into(),
        target: Some("Loudness".into()),
        change: None,
        changes: vec![],
        rows: vec![],
        kept: Some(true),
        why: Some("loudness improved".into()),
        rebalanced: None,
        listener: None,
        problems: vec![],
        next: None,
        met: false,
        listens: 2,
        elapsed_ms: 0,
    }
}
case!(a_database_that_cant_open_leaves_notes_in_their_files_and_says_so, async {
    let f = Fixture::new(false);
    f.settings(json!({"model":"anthropic/claude-sonnet-5-5","updateCheck":false,"modelCatalog":false,"libraryFolders":[]}));
    let memory = f.folder.path().join("memory.json");
    std::fs::write(
        &memory,
        r#"{"version":1,"notes":[{"id":"p1","text":"Mixes on headphones","at":1000},{"id":"p2","text":"Likes short reverbs","at":2000}]}"#,
    )
    .unwrap();
    // Not a database this Kumi can open.
    let database = f.folder.path().join("kumi.db");
    std::fs::write(&database, "not a database").unwrap();
    let task = tokio::task::spawn_local(run(f.io(&["--inference-only"]), unused_factory()));
    f.input.wait_ready().await;
    f.wait_output("Your notes, techniques and lessons stay in their files this time").await;
    // Kumi refuses commands while it gets ready: ask until it answers.
    let answered = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while !f.output().contains("[memory] About you") {
            f.input.write("/memory\n");
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
    })
    .await;
    assert!(answered.is_ok(), "{}", f.output());
    f.wait_output("[memory] About you: p1 Mixes on headphones · p2 Likes short reverbs").await;
    f.input.write("/forget p1\n");
    f.wait_output("[memory] Forgot: Mixes on headphones").await;
    f.input.write("/memory\n");
    f.wait_output("[memory] About you: p2 Likes short reverbs").await;
    f.input.write("/quit\n");
    f.input.end();
    assert_eq!(tokio::time::timeout(std::time::Duration::from_secs(10), task).await.unwrap().unwrap(), 0);
    let kept: Value = serde_json::from_slice(&std::fs::read(&memory).unwrap()).unwrap();
    assert_eq!(kept["notes"].as_array().unwrap().len(), 1, "the file was written");
    assert_eq!(std::fs::read(&database).unwrap(), b"not a database", "and not the database");
    assert!(!f.folder.path().join("kumi.db-wal").exists());
});
case!(continue_and_resume_in_live_leave_another_daws_conversation_to_that_daw, async {
    // Review L4: the newest conversation is REAPER's; Live's -c takes up Live's, and -r of REAPER's says whose it is.
    use kumi_agent::ConversationStore;
    let f = Fixture::new(false);
    f.settings(json!({"model":"anthropic/claude-sonnet-5-5","updateCheck":false,"modelCatalog":false,"libraryFolders":[]}));
    let (set, project) = ("a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1", "reaper-b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2");
    let said = |text: &str, at: i64| -> kumi_agent::SavedConversation {
        serde_json::from_value(json!({"savedAt":at,"checkpoint":{"version":1,"messages":[{"role":"user","content":text},{"role":"assistant","content":"done"}]}}))
            .unwrap()
    };
    let store = kumi_agent::core::conversations::create_conversation_store_in(
        f.folder.path().join("projects"),
        None,
        kumi::daw::registry::places(),
    );
    store.save(set, "mv66me0m3fa91c", &said("make the bass wider", 1)).await.unwrap();
    store.save(project, "mv66zz0m3fa91c", &said("Mix notes for the song", 2)).await.unwrap();
    drop(store);
    assert_eq!(run(f.io(&["-r", "mv66zz0m3fa91c", "--inference-only"]), unused_factory()).await, 1);
    let err = f.err.0.borrow().clone();
    assert!(
        err.contains("“Mix notes for the song” is about a REAPER project: cargo run -p kumi -- --daw reaper -r mv66zz0m3fa91c carries it on in REAPER."),
        "{err}"
    );
    let task = tokio::task::spawn_local(run(f.io(&["-c", "--inference-only"]), unused_factory()));
    f.input.wait_ready().await;
    f.wait_output("Back to your conversation").await;
    f.input.write("/status\n");
    f.wait_output("Conversation: make the bass wider · mv66me0m3fa91c").await;
    f.input.write("/quit\n");
    f.input.end();
    assert_eq!(tokio::time::timeout(std::time::Duration::from_secs(10), task).await.unwrap().unwrap(), 0);
});
case!(continue_and_resume_take_up_a_kept_conversation_or_say_why_not, async {
    use kumi_agent::ConversationStore;
    let f = Fixture::new(false);
    f.settings(json!({"model":"anthropic/claude-sonnet-5-5","updateCheck":false,"modelCatalog":false,"libraryFolders":[]}));
    let (a, b) = ("a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1", "b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2");
    let said = |text: &str, at: i64| -> kumi_agent::SavedConversation {
        serde_json::from_value(json!({"savedAt":at,"checkpoint":{"version":1,"messages":[{"role":"user","content":text},{"role":"assistant","content":"done"}]}}))
            .unwrap()
    };
    let store = kumi_agent::create_conversation_store(f.folder.path().join("projects"));
    store.save(a, "mv66me0m3fa91c", &said("make the bass wider", 1)).await.unwrap();
    store.save(b, "mv66zz0m3fa91c", &said("tighten the drums", 2)).await.unwrap();
    store.rename(a, "mv66me0m3fa91c", Some("Bass ideas")).await.unwrap();
    drop(store);
    // A word that names none: said, exit 1, before anything starts.
    assert_eq!(run(f.io(&["-r", "nothing", "--inference-only"]), unused_factory()).await, 1);
    assert!(f.err.0.borrow().contains("No conversation nothing. cargo run -p kumi -- -r lists them."), "{}", f.err.0.borrow());
    // -c: the one had last, in any Set; -r: by its name. /status names it, and the exit says how to come back.
    for (args, title, id) in [
        (vec!["-c", "--inference-only"], "tighten the drums", "mv66zz0m3fa91c"),
        (vec!["-r", "bass", "ideas", "--inference-only"], "Bass ideas", "mv66me0m3fa91c"),
    ] {
        f.out.0.borrow_mut().clear();
        let task = tokio::task::spawn_local(run(f.io(&args), unused_factory()));
        f.input.wait_ready().await;
        f.wait_output("Back to your conversation").await;
        f.input.write("/status\n");
        f.wait_output(&format!("Conversation: {title} · {id}")).await;
        f.input.write("/quit\n");
        f.input.end();
        assert_eq!(tokio::time::timeout(std::time::Duration::from_secs(10), task).await.unwrap().unwrap(), 0);
        assert!(f.output().contains(&format!("Continue this conversation: cargo run -p kumi -- -r {id}")), "{}", f.output());
    }
});

/// A model server on this computer, as llama.cpp's or vLLM's answers: its one model, and every chat answered with
/// `words`, a delta each.
async fn model_server(words: &'static [&'static str]) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else { break };
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
                let (kind, body) = if head.split_whitespace().nth(1).is_some_and(|path| path.ends_with("/chat/completions")) {
                    let mut body = String::new();
                    for (at, word) in words.iter().enumerate() {
                        let finish = if at + 1 == words.len() { json!("stop") } else { Value::Null };
                        let chunk = json!({"id": "chatcmpl-1", "object": "chat.completion.chunk", "model": "tiny",
                            "choices": [{"index": 0, "delta": {"content": word}, "finish_reason": finish}]});
                        body.push_str(&format!("data: {chunk}\n\n"));
                    }
                    body.push_str("data: [DONE]\n\n");
                    ("text/event-stream", body)
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
case!(print_answers_one_request_and_ends, async {
    let f = Fixture::new(false);
    let url = model_server(&["A low shelf ", "lifts or cuts ", "everything below it."]).await;
    f.settings(json!({"model": "tiny-server/tiny", "modelServers": [{"name": "Tiny Server", "baseURL": url}],
        "updateCheck": false, "modelCatalog": false, "libraryFolders": []}));
    let ask = |json: bool| {
        let mut args = vec!["-p", "What does a low shelf do?", "--inference-only"];
        if json {
            args.push("--json");
        }
        tokio::time::timeout(std::time::Duration::from_secs(30), run(f.io(&args), unused_factory()))
    };
    assert_eq!(ask(false).await.expect("kumi -p ends"), 0, "stdout: {}; stderr: {}", f.output(), f.err.0.borrow());
    assert_eq!(f.output(), "A low shelf lifts or cuts everything below it.\n");
    assert!(f.err.0.borrow().is_empty(), "{}", f.err.0.borrow());
    // Each event the session sends, a JSON line each.
    f.out.0.borrow_mut().clear();
    assert_eq!(ask(true).await.expect("kumi -p --json ends"), 0, "stderr: {}", f.err.0.borrow());
    let events: Vec<Value> = f.output().lines().map(|line| serde_json::from_str(line).unwrap()).collect();
    let words: String = events.iter().filter(|e| e["type"] == "text").map(|e| e["text"].as_str().unwrap()).collect();
    assert_eq!(words, "A low shelf lifts or cuts everything below it.");
    let ends: Vec<&Value> = events.iter().filter(|e| e["type"] == "turn-complete").map(|e| &e["result"]["stopReason"]).collect();
    assert_eq!(ends, [&json!("completed")]);
});
case!(print_says_what_went_wrong_and_how_to_use_it, async {
    let f = Fixture::new(false);
    f.settings(json!({"model": "anthropic/claude-sonnet-5-5", "updateCheck": false, "modelCatalog": false, "libraryFolders": []}));
    assert_eq!(run(f.io(&["-p", "hello", "--inference-only"]), unused_factory()).await, 1);
    assert_eq!(f.output(), "");
    assert!(f.err.0.borrow().contains("Not signed in to Anthropic"), "{}", f.err.0.borrow());
    for (args, said) in [
        (&["-p"][..], "-p \"<request>\" [--json]"),
        (&["-p", "hello", "-r"], "With -p, say which conversation"),
        (&["--json"], "No option called --json"),
    ] {
        f.err.0.borrow_mut().clear();
        assert_eq!(run(f.io(args), unused_factory()).await, 1, "{args:?}");
        assert!(f.err.0.borrow().contains(said), "{args:?}: {}", f.err.0.borrow());
    }
});

case!(print_answers_through_a_daws_bridge_too, async {
    let mut f = Fixture::new(false);
    let url = model_server(&["Done: ", "the kick is 2 dB louder."]).await;
    let config = f.folder.path().join("bridge.json");
    std::fs::write(&config, "{}").unwrap();
    f.env.insert("KUMI_BRIDGE_CONFIG".into(), config.display().to_string());
    f.settings(json!({"model": "tiny-server/tiny", "modelServers": [{"name": "Tiny Server", "baseURL": url}],
        "updateCheck": false, "modelCatalog": false, "libraryFolders": []}));
    // The DAW's integration as the app makes it, over a bridge that's here.
    let made = Rc::new(Cell::new(false));
    let mark = made.clone();
    let factory = AbletonFactory(Rc::new(move |options: kumi_daw_ableton::AbletonOptions| {
        mark.set(true);
        assert!(options.bridge_config.is_some());
        let connection = options.on_connection;
        create_inference_only_integration(Rc::new(move |state| connection(state, None)))
    }));
    let args = ["-p", "make the kick louder", "--bridge-config", config.to_str().unwrap()];
    let code = tokio::time::timeout(std::time::Duration::from_secs(30), run(f.io(&args), factory)).await.expect("kumi -p ends");
    assert_eq!(code, 0, "stdout: {}; stderr: {}", f.output(), f.err.0.borrow());
    assert!(made.get(), "the DAW's integration was made");
    assert_eq!(f.output(), "Done: the kick is 2 dB louder.\n");
});
